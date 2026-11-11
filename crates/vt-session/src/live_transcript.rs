//! Live transcript persistence, off the recognizer thread.
//!
//! The speech worker hands each final line to a [`TranscriptSink`] and returns
//! to recognition at once; it never waits for, or fails because of, the
//! database. A dedicated writer thread owned by the recording session saves
//! what is queued in batches through `store::save_live_segments` (IMMEDIATE
//! transaction, point operations only, short lock budget). Lock contention
//! keeps the batch queued and retries it with the next line or after
//! [`RETRY_INTERVAL`]. A line the database rejects for a deterministic reason is
//! dropped as a `segment_rejected` inference gap; lines evicted from the bounded
//! queue, or still unsaved when the writer closes, become `persistence_failed`
//! gaps. Stop closes and joins the writer before the live run is sealed.
use crate::{
    AppService, Result, Segment, backlog,
    speaker_pipeline::PERSISTENCE_FAILED_GAP,
    store::{self, LiveSaveBudget},
};
use std::{
    collections::{BTreeSet, VecDeque},
    sync::{Arc, Condvar, Mutex, MutexGuard},
    thread,
    time::{Duration, Instant},
};
use vt_core::TrackKind;

/// Unsaved live lines kept (queued plus carried forward after lock contention).
/// Finals arrive roughly once per utterance, so this is many minutes of speech.
pub(crate) const QUEUE_LIMIT: usize = 512;
/// Lines saved by one transaction; bounds how long one save holds the lock.
pub(crate) const BATCH_LIMIT: usize = 64;
/// After lock contention outlasts the budget, the next attempt waits for a new
/// line or this interval, whichever comes first.
pub(crate) const RETRY_INTERVAL: Duration = Duration::from_secs(1);
/// At stop, unsaved lines are retried for at most this long, then become gaps.
pub(crate) const CLOSE_DEADLINE: Duration = Duration::from_secs(5);
/// Gap reason for a line the database rejected for a non-lock reason.
pub(crate) const SEGMENT_REJECTED_GAP: &str = "segment_rejected";
pub(crate) const DROPPED_WARNING: &str = "Some live transcript lines could not be saved because the database stayed busy. Raw audio is preserved; refine saved audio to recover them.";

pub(crate) fn busy_warning(cause: &str) -> String {
    format!(
        "Live transcript lines could not be saved yet; they are kept and retried. Raw audio is preserved. Cause: {cause}"
    )
}
pub(crate) fn rejected_warning(cause: &str) -> String {
    format!(
        "A live transcript line could not be saved and was skipped. Raw audio is preserved; refine saved audio to recover it. Cause: {cause}"
    )
}

#[derive(Clone)]
struct Item {
    generation: u64,
    track: TrackKind,
    segment: Segment,
}
#[derive(Default)]
struct State {
    queue: VecDeque<Item>,
    /// The first `in_flight` queued lines are being saved and are not evicted.
    in_flight: usize,
    arrivals: u64,
    closed: bool,
}
#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    wake: Condvar,
}
impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Producer handle used by the speech worker. Pushing never blocks on SQLite.
#[derive(Clone)]
pub(crate) struct TranscriptSink {
    shared: Arc<Shared>,
    service: Arc<AppService>,
}
impl TranscriptSink {
    /// Queues one final line. Callers first publish it to the runtime view
    /// (`Runtime::live_finals`) so the UI shows it before it is saved.
    pub(crate) fn push(&self, generation: u64, track: TrackKind, segment: Segment) {
        let evicted = {
            let mut state = self.shared.lock();
            state.queue.push_back(Item {
                generation,
                track,
                segment,
            });
            state.arrivals = state.arrivals.wrapping_add(1);
            let evicted = if state.queue.len() > QUEUE_LIMIT {
                let oldest = state.in_flight;
                state.queue.remove(oldest)
            } else {
                None
            };
            self.shared.wake.notify_all();
            evicted
        };
        if let Some(item) = evicted {
            dropped(&self.service, &[item], PERSISTENCE_FAILED_GAP);
            self.service.backlog_warn(DROPPED_WARNING);
        }
    }
}

/// The session-owned writer thread. `finish` (or drop) closes the queue,
/// lets the writer drain it within [`CLOSE_DEADLINE`], and joins it.
pub(crate) struct TranscriptWriter {
    sink: TranscriptSink,
    worker: Option<thread::JoinHandle<()>>,
}
impl TranscriptWriter {
    pub(crate) fn start(
        service: Arc<AppService>,
        meeting: String,
        run: String,
        budget: LiveSaveBudget,
    ) -> Result<Self> {
        let shared = Arc::new(Shared::default());
        let sink = TranscriptSink {
            shared: shared.clone(),
            service: service.clone(),
        };
        let worker = thread::Builder::new()
            .name("live-transcript-writer".into())
            .spawn(move || write_loop(&service, &shared, &meeting, &run, budget))?;
        Ok(Self {
            sink,
            worker: Some(worker),
        })
    }
    pub(crate) fn sink(&self) -> TranscriptSink {
        self.sink.clone()
    }
    pub(crate) fn finish(mut self) {
        self.close();
    }
    fn close(&mut self) {
        {
            let mut state = self.sink.shared.lock();
            state.closed = true;
            self.sink.shared.wake.notify_all();
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
impl Drop for TranscriptWriter {
    fn drop(&mut self) {
        self.close();
    }
}

fn write_loop(
    service: &AppService,
    shared: &Shared,
    meeting: &str,
    run: &str,
    budget: LiveSaveBudget,
) {
    // Set after lock contention: retry on the next arrival or after the interval.
    let mut blocked: Option<(u64, Instant)> = None;
    let mut close_deadline: Option<Instant> = None;
    loop {
        let batch = {
            let mut state = shared.lock();
            loop {
                if state.closed {
                    let deadline =
                        *close_deadline.get_or_insert_with(|| Instant::now() + CLOSE_DEADLINE);
                    if state.queue.is_empty() {
                        return;
                    }
                    if Instant::now() >= deadline {
                        let rest = state.queue.drain(..).collect::<Vec<_>>();
                        drop(state);
                        dropped(service, &rest, PERSISTENCE_FAILED_GAP);
                        service.backlog_warn(DROPPED_WARNING);
                        return;
                    }
                    if blocked.is_some_and(|(_, at)| at.elapsed() < budget.backoff) {
                        state = shared
                            .wake
                            .wait_timeout(state, budget.backoff)
                            .unwrap_or_else(|e| e.into_inner())
                            .0;
                        continue;
                    }
                    break;
                }
                match blocked {
                    _ if state.queue.is_empty() => {
                        state = shared.wake.wait(state).unwrap_or_else(|e| e.into_inner());
                    }
                    Some((arrivals, at))
                        if arrivals == state.arrivals && at.elapsed() < RETRY_INTERVAL =>
                    {
                        let left = RETRY_INTERVAL.saturating_sub(at.elapsed());
                        state = shared
                            .wake
                            .wait_timeout(state, left)
                            .unwrap_or_else(|e| e.into_inner())
                            .0;
                    }
                    _ => break,
                }
            }
            let count = state.queue.len().min(BATCH_LIMIT);
            state.in_flight = count;
            state.queue.iter().take(count).cloned().collect::<Vec<_>>()
        };
        let (resolved, busy) = save_batch(service, meeting, run, &batch, budget);
        let mut state = shared.lock();
        state.in_flight = 0;
        state
            .queue
            .retain(|item| !resolved.contains(&item.segment.id));
        blocked = busy.then(|| (state.arrivals, Instant::now()));
        drop(state);
        forget_pending(service, &resolved);
    }
}

/// Saves `batch` in one transaction. A deterministic failure is isolated by
/// saving each line on its own, so one bad line never blocks later ones.
/// Returns the IDs that no longer need saving (saved or rejected) and whether
/// lock contention left some unsaved.
fn save_batch(
    service: &AppService,
    meeting: &str,
    run: &str,
    batch: &[Item],
    budget: LiveSaveBudget,
) -> (BTreeSet<String>, bool) {
    let segments = batch
        .iter()
        .map(|item| item.segment.clone())
        .collect::<Vec<_>>();
    let mut resolved = BTreeSet::new();
    match store::save_live_segments(&service.root, meeting, run, &segments, budget) {
        Ok(_) => {
            resolved.extend(segments.into_iter().map(|s| s.id));
            return (resolved, false);
        }
        Err(error) if store::sqlite_busy(&error) => {
            service.backlog_warn(&busy_warning(&store::describe_error(&error)));
            return (resolved, true);
        }
        Err(error) if batch.len() == 1 => {
            reject(service, &batch[0], &error);
            resolved.insert(batch[0].segment.id.clone());
            return (resolved, false);
        }
        Err(_) => (),
    }
    for item in batch {
        match store::save_live_segments(
            &service.root,
            meeting,
            run,
            std::slice::from_ref(&item.segment),
            budget,
        ) {
            Ok(_) => {
                resolved.insert(item.segment.id.clone());
            }
            Err(error) if store::sqlite_busy(&error) => {
                service.backlog_warn(&busy_warning(&store::describe_error(&error)));
                return (resolved, true);
            }
            Err(error) => {
                reject(service, item, &error);
                resolved.insert(item.segment.id.clone());
            }
        }
    }
    (resolved, false)
}

fn reject(service: &AppService, item: &Item, error: &anyhow::Error) {
    dropped(service, std::slice::from_ref(item), SEGMENT_REJECTED_GAP);
    service.backlog_warn(&rejected_warning(&store::describe_error(error)));
}

/// Records dropped lines as STT inference gaps and removes them from the view.
fn dropped(service: &AppService, items: &[Item], reason: &str) {
    {
        let mut controller = service.backlog.lock().unwrap_or_else(|e| e.into_inner());
        for item in items {
            let start = item.segment.start_ms;
            let end = item.segment.end_ms.max(start.saturating_add(1));
            controller.gap(
                backlog::Worker::Stt,
                item.track,
                item.generation,
                start,
                end,
                reason,
            );
        }
    }
    let ids = items
        .iter()
        .map(|item| item.segment.id.clone())
        .collect::<BTreeSet<_>>();
    forget_pending(service, &ids);
}

fn forget_pending(service: &AppService, ids: &BTreeSet<String>) {
    if ids.is_empty() {
        return;
    }
    if let Ok(mut runtime) = service.runtime.lock() {
        runtime
            .live_finals
            .retain(|segment| !ids.contains(&segment.id));
    }
}

impl AppService {
    pub(crate) fn backlog_warn(&self, message: &str) {
        self.backlog
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .warn(message);
    }
}

#[cfg(test)]
#[path = "live_transcript_tests.rs"]
mod tests;
