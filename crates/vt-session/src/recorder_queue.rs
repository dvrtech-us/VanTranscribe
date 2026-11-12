//! The recording session's audio recorder thread and its bounded queue.
//!
//! The capture consumer loop (`session::record_session`) must never wait for
//! the disk or the database: while it waits it cannot drain the helper, feed
//! live inference, or handle pause/stop. All `vt_record::Recording` work (chunk
//! and gap manifest rows, FLAC encoding, fsyncs, renames, the final status) runs
//! on one `audio-recorder` thread owned by the session. The loop hands it work
//! in the order it observed it, through one FIFO shared by both tracks, so
//! per-track contiguity, epochs and gap order reach the manifest unchanged.
//!
//! The loop never blocks on the queue: every lock here is held only for O(queue)
//! bookkeeping, never across I/O. Everything the loop used to learn from a write
//! (chunk end time, sequence, epoch) it computes itself from the audio it hands
//! over; what only the recorder can know (lock waits, write latency, a fatal
//! error, a skipped chunk-clock row) is read back with [`RecorderQueue::status`].
//!
//! Bound: at most [`RECORDER_QUEUE_LIMIT`] of audio per track and
//! [`RECORDER_QUEUE_MAX_BYTES`] of samples in total may wait (queued plus the
//! one item being written). A block that would exceed either is not queued; an
//! explicit `recorder_backlog` gap covering exactly that block's frames and time
//! is queued instead (merged with the previous one while it is still waiting),
//! so `vt_record`'s contiguity rules hold and the loss is exact. Once a track
//! drops, it keeps dropping (one growing gap) until its backlog is at most half
//! the bound, so a recorder slightly slower than real time loses audio in a
//! few long stretches instead of hundreds of short ones.

use std::{
    collections::VecDeque,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc, Condvar, Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

use vt_core::{AudioBlock, GapReason, StreamFormat, TrackKind};
use vt_record::{Gap, LockWaits, MANIFEST_LOCK_BUDGET, Recording};

/// Audio per track that may wait for the recorder. 60 s absorbs any lock the
/// recorder waits out without poisoning (`MANIFEST_LOCK_BUDGET`, 60 s) and any
/// realistic disk stall. Worst case for 48 kHz mono mic + 48 kHz stereo system
/// audio (f32 samples): 60 s x 48,000 x (1 + 2) x 4 B = 34.56 MB.
pub(crate) const RECORDER_QUEUE_LIMIT: Duration = Duration::from_secs(60);
/// Hard ceiling on queued sample memory, for native formats far above 48 kHz
/// or with many channels (e.g. 96 kHz stereo on both tracks reaches it after
/// about 44 s instead of using 92 MB).
pub(crate) const RECORDER_QUEUE_MAX_BYTES: u64 = 64 * 1024 * 1024;
/// A backlog at least this long is reported once as a health warning.
pub(crate) const RECORDER_BACKLOG_WARN: Duration = Duration::from_secs(5);
/// How long stop waits for queued audio to be written. A recorder waiting for a
/// database lock poisons itself (a fatal error) after `MANIFEST_LOCK_BUDGET`, so
/// this only bounds stalls the recorder cannot detect itself (a hung file
/// system); the extra 30 s writes a full 60 s queue at normal speed.
pub(crate) const RECORDER_DRAIN_DEADLINE: Duration =
    MANIFEST_LOCK_BUDGET.saturating_add(Duration::from_secs(30));
/// After the drain deadline, how long stop lets the recorder complete the item
/// it is writing (and, if the queue is then empty, finish the manifest: that
/// still counts as success). No new item is started meanwhile.
pub(crate) const ABANDON_GRACE: Duration = Duration::from_secs(5);
/// After the grace, manifest lock waits are cancelled (they end within about
/// 250 ms) and stop waits this long for the thread to release the meeting
/// lock. A thread still stuck in file I/O runs the late recovery when it ends.
pub(crate) const ABORT_WAIT: Duration = Duration::from_secs(2);
/// Longest a stop can wait for the recorder: drain, grace and abort wait.
pub(crate) const RECORDER_STOP_BOUND: Duration = RECORDER_DRAIN_DEADLINE
    .saturating_add(ABANDON_GRACE)
    .saturating_add(ABORT_WAIT);
/// How often a drain reports progress (UI "saving audio" state).
const DRAIN_PROGRESS: Duration = Duration::from_millis(250);

pub(crate) const RECORDER_BACKLOG_WARNING: &str = "Saving recorded audio fell more than 5 s behind capture; recording continues and the waiting audio is saved as the disk or database catches up.";
pub(crate) const RECORDER_DROP_WARNING: &str = "Saving recorded audio fell more than 60 s behind capture, so some audio could not be kept; each missing stretch is marked as a recorder_backlog gap in the saved audio. Recording continues.";
pub(crate) const RECORDER_PANIC_ERROR: &str = "The audio recorder stopped unexpectedly";
pub(crate) const RECORDER_DRAIN_ERROR: &str = "Saving recorded audio did not finish in time after the recording stopped. Audio saved before then is kept and recovered; audio still waiting to be saved was lost and is marked as a recorder_interrupted gap.";
/// Appended when the recorder still held the recording after the abort wait.
pub(crate) const RECORDER_RECOVERY_PENDING: &str = "The recorder has not released the recording yet; its saved audio is recovered as soon as it does, or at the next start.";
pub(crate) const RECORDER_STOPPED_ERROR: &str = "The audio recorder has already stopped.";

/// The source (hardware) observation behind one chunk, saved to
/// `app_audio_clock` just before the chunk is published. `seq` is the chunk's
/// manifest sequence as predicted by the loop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ClockObservation {
    pub seq: u64,
    pub source_epoch: u32,
    pub source_frame: u64,
    pub host_ns: u64,
}

/// Saves one chunk-clock row. It runs inside a capture-loop scope, so it waits
/// at most a few ms for the in-process writer (plus SQLite's short busy
/// timeout). A lock error (`store::sqlite_busy`) skips the row and is reported
/// through [`RecorderQueue::take_clock_busy`]; any other error is fatal to the
/// recording, as it was on the capture loop.
pub(crate) type ClockSink =
    Box<dyn FnMut(TrackKind, ClockObservation) -> anyhow::Result<()> + Send>;

/// What happened to a block handed to [`RecorderQueue::push_chunk`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Admission {
    Queued,
    /// Not queued: a `recorder_backlog` gap covers it. `new_gap` is false when
    /// the block extended a gap that was still waiting.
    Dropped {
        new_gap: bool,
    },
}

/// What happened to a gap handed to [`RecorderQueue::push_gap`]. A gap that
/// directly follows a still-queued gap of the same reason on the same track
/// extends it instead of adding a row; `epoch` is then the earlier gap's
/// epoch, which the loop takes as the track's last queued epoch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct GapAdmission {
    pub merged: bool,
    pub epoch: u32,
}

/// Recorder progress as seen by the capture loop.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Status {
    /// Audio waiting (queued or being written), the larger of the two tracks.
    pub queued_ns: u64,
    /// Upper bound of the encoded bytes still to be written.
    pub pending_bytes: u64,
    pub max_queued_ns: u64,
    /// Blocks dropped because the queue was full, and their total duration.
    pub drops: u64,
    pub dropped_ns: u64,
    /// Longest single chunk write, its clock row included.
    pub max_write: Duration,
    pub lock_waits: LockWaits,
    /// Set once a fatal recorder error (or panic) has ended the recording.
    pub failed: Option<String>,
}

#[derive(Clone)]
pub(crate) struct Config {
    pub queue_limit: Duration,
    pub max_bytes: u64,
    pub drain_deadline: Duration,
    pub abandon_grace: Duration,
    /// Test hook run on the recorder thread before each item: `(track,
    /// is_chunk)`. It may sleep (a stalled disk) or panic.
    #[cfg(test)]
    pub before: Option<Arc<dyn Fn(TrackKind, bool) + Send + Sync>>,
    /// Test hook run in the thread's own bookkeeping after each item.
    #[cfg(test)]
    pub after: Option<Arc<dyn Fn() + Send + Sync>>,
    /// Test override of the manifest lock budget.
    #[cfg(test)]
    pub lock_budget: Option<Duration>,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            queue_limit: RECORDER_QUEUE_LIMIT,
            max_bytes: RECORDER_QUEUE_MAX_BYTES,
            drain_deadline: RECORDER_DRAIN_DEADLINE,
            abandon_grace: ABANDON_GRACE,
            #[cfg(test)]
            before: None,
            #[cfg(test)]
            after: None,
            #[cfg(test)]
            lock_budget: None,
        }
    }
}

/// Recorder overrides a test installs on `AppService` before a recording.
#[cfg(test)]
#[derive(Clone, Default)]
pub(crate) struct TestHook {
    pub queue_limit: Option<Duration>,
    pub drain_deadline: Option<Duration>,
    pub abandon_grace: Option<Duration>,
    /// Replaces the 35 s base bound of `wait_for_shutdown`.
    pub shutdown_wait: Option<Duration>,
    pub before: Option<Arc<dyn Fn(TrackKind, bool) + Send + Sync>>,
    /// Counts capture-channel sends that found its 128 slots full.
    pub channel_full: Option<Arc<std::sync::atomic::AtomicU64>>,
}

enum Work {
    Chunk {
        block: AudioBlock,
        clock: Option<ClockObservation>,
    },
    Gap {
        format: StreamFormat,
        gap: Gap,
    },
}
struct Item {
    id: u64,
    track: TrackKind,
    work: Work,
    /// Audio duration and sample count a chunk holds (zero for a gap).
    ns: u64,
    samples: u64,
}

#[derive(Default)]
struct State {
    queue: VecDeque<Item>,
    next_id: u64,
    /// Per track: the last item queued, when it is a gap still waiting (a
    /// further drop or helper gap of the same reason extends it).
    tail_gap: [Option<u64>; 2],
    /// Per track: dropping until the backlog is at most half the bound.
    dropping: [bool; 2],
    /// Counted until written, so the item being written still counts.
    queued_ns: [u64; 2],
    queued_samples: u64,
    queued_chunks: u64,
    max_queued_ns: u64,
    drops: u64,
    dropped_ns: u64,
    max_write: Duration,
    lock_waits: LockWaits,
    clock_busy: Option<String>,
    failed: Option<String>,
    closing: bool,
    /// Stop starting queued items (the drain deadline passed, or the queue
    /// was dropped unfinished). An empty queue while closing still finishes.
    abandon: bool,
    /// The drain gave up on a thread still busy after the abort wait: it runs
    /// the late cleanup (recovery) itself when it finally ends.
    late: bool,
    /// Set when the thread has ended: `Ok` only after `Recording::finish`.
    finished: Option<Result<(), String>>,
}
impl State {
    fn status(&self) -> Status {
        Status {
            queued_ns: self.queued_ns[0].max(self.queued_ns[1]),
            pending_bytes: self
                .queued_samples
                .saturating_mul(3)
                .saturating_add(self.queued_chunks.saturating_mul(4096)),
            max_queued_ns: self.max_queued_ns,
            drops: self.drops,
            dropped_ns: self.dropped_ns,
            max_write: self.max_write,
            lock_waits: self.lock_waits,
            failed: self.failed.clone(),
        }
    }
    fn push(&mut self, track: TrackKind, work: Work, ns: u64, samples: u64) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.queue.push_back(Item {
            id,
            track,
            work,
            ns,
            samples,
        });
        id
    }
    fn stopped(&self) -> Option<String> {
        self.failed.clone().or_else(|| {
            (self.finished.is_some() || self.closing).then(|| RECORDER_STOPPED_ERROR.to_owned())
        })
    }
    /// The still-queued gap that ends this track's queue, if any.
    fn tail_gap(&mut self, t: usize) -> Option<&mut Gap> {
        let id = self.tail_gap[t]?;
        match self.queue.iter_mut().rev().find(|item| item.id == id) {
            Some(Item {
                work: Work::Gap { gap, .. },
                ..
            }) => Some(gap),
            _ => None,
        }
    }
    fn clear_counts(&mut self) {
        self.queue.clear();
        self.queued_ns = [0, 0];
        self.queued_samples = 0;
        self.queued_chunks = 0;
    }
}

#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    /// Signals the recorder thread: new work, close, or abandon.
    wake: Condvar,
    /// Signals waiters: progress or the end of the thread.
    done: Condvar,
    /// Cancels the recording's manifest lock waits (see `set_abort_signal`).
    abort: Arc<AtomicBool>,
}
impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

fn index(track: TrackKind) -> usize {
    usize::from(track == TrackKind::System)
}

/// Same arithmetic as `vt_record` uses for a chunk's end time.
pub(crate) fn block_ns(frames: u64, rate: u32) -> u64 {
    frames.saturating_mul(1_000_000_000) / u64::from(rate)
}

/// A recorder thread fed by a bounded, ordered, non-blocking queue.
pub(crate) struct RecorderQueue {
    shared: Arc<Shared>,
    config: Config,
    thread: Option<JoinHandle<()>>,
}

impl RecorderQueue {
    /// `late_cleanup` runs on the recorder thread only if a drain gave up on
    /// it (see [`Self::finish`]), after it has released the meeting lock.
    pub(crate) fn start(
        mut recording: Recording,
        clock: ClockSink,
        config: Config,
        late_cleanup: Box<dyn FnOnce() + Send>,
    ) -> anyhow::Result<Self> {
        let shared = Arc::new(Shared::default());
        recording.set_abort_signal(shared.abort.clone());
        #[cfg(test)]
        if let Some(budget) = config.lock_budget {
            recording.set_lock_budget(budget);
        }
        let worker = shared.clone();
        #[cfg(test)]
        let hooks = (config.before.clone(), config.after.clone());
        let thread = std::thread::Builder::new()
            .name("audio-recorder".into())
            .spawn(move || {
                // Any unwind, including one in this thread's own bookkeeping,
                // ends the recorder with the panic as its fatal error.
                let body = catch_unwind(AssertUnwindSafe(|| {
                    recorder_thread(
                        &worker,
                        recording,
                        clock,
                        #[cfg(test)]
                        hooks,
                    )
                }));
                let mut state = worker.lock();
                if let Err(payload) = body {
                    let error = panic_text(payload.as_ref());
                    if state.failed.is_none() {
                        state.failed = Some(error.clone());
                    }
                    if state.finished.is_none() {
                        state.finished = Some(Err(error));
                    }
                    state.clear_counts();
                }
                let late = state.late;
                drop(state);
                worker.done.notify_all();
                if late {
                    late_cleanup();
                }
            })?;
        Ok(Self {
            shared,
            config,
            thread: Some(thread),
        })
    }

    /// Queues one positioned block, or (queue full) an exact gap in its place.
    /// Never blocks. Once the recorder has failed or stopped the block is not
    /// taken: the error is returned and the loop must not advance its position.
    pub(crate) fn push_chunk(
        &self,
        track: TrackKind,
        block: AudioBlock,
        clock: Option<ClockObservation>,
    ) -> Result<Admission, String> {
        let frames = block.frames();
        let ns = block_ns(frames, block.format.sample_rate);
        let samples = block.samples.len() as u64;
        let mut state = self.shared.lock();
        if let Some(error) = state.stopped() {
            return Err(error);
        }
        let t = index(track);
        let limit = u64::try_from(self.config.queue_limit.as_nanos()).unwrap_or(u64::MAX);
        if state.dropping[t] && state.queued_ns[t] <= limit / 2 {
            state.dropping[t] = false;
        }
        let fits = !state.dropping[t]
            && state.queued_ns[t].saturating_add(ns) <= limit
            && state
                .queued_samples
                .saturating_add(samples)
                .saturating_mul(4)
                <= self.config.max_bytes;
        if fits {
            state.tail_gap[t] = None;
            state.queued_ns[t] += ns;
            state.queued_samples += samples;
            state.queued_chunks += 1;
            state.max_queued_ns = state.max_queued_ns.max(state.queued_ns[t]);
            state.push(track, Work::Chunk { block, clock }, ns, samples);
            drop(state);
            self.shared.wake.notify_all();
            return Ok(Admission::Queued);
        }
        state.dropping[t] = true;
        state.drops += 1;
        state.dropped_ns += ns;
        let start = block.position.host_time_ns;
        let end = start.saturating_add(ns);
        let position = block.position;
        if let Some(gap) = state.tail_gap(t)
            && gap.reason == GapReason::RecorderBacklog
            && gap.end_ns == start
            && gap.epoch == position.epoch
            && gap.first_frame + gap.frame_count == position.first_frame
        {
            gap.end_ns = end;
            gap.frame_count += frames;
            return Ok(Admission::Dropped { new_gap: false });
        }
        let id = state.push(
            track,
            Work::Gap {
                format: block.format,
                gap: Gap {
                    epoch: position.epoch,
                    first_frame: position.first_frame,
                    frame_count: frames,
                    start_ns: start,
                    end_ns: end,
                    reason: GapReason::RecorderBacklog,
                },
            },
            0,
            0,
        );
        state.tail_gap[t] = Some(id);
        drop(state);
        self.shared.wake.notify_all();
        Ok(Admission::Dropped { new_gap: true })
    }

    /// Queues a gap (pause, helper overflow, device change). Never dropped; a
    /// gap that directly follows a still-queued gap of the same reason extends
    /// it, so a flapping device cannot grow the queue without bound.
    pub(crate) fn push_gap(
        &self,
        track: TrackKind,
        format: StreamFormat,
        gap: Gap,
    ) -> Result<GapAdmission, String> {
        let mut state = self.shared.lock();
        if let Some(error) = state.stopped() {
            return Err(error);
        }
        let t = index(track);
        if gap.frame_count == 0
            && let Some(tail) = state.tail_gap(t)
            && tail.reason == gap.reason
            && tail.end_ns == gap.start_ns
            && ((tail.epoch == gap.epoch && tail.first_frame + tail.frame_count == gap.first_frame)
                || (tail.epoch.checked_add(1) == Some(gap.epoch) && gap.first_frame == 0))
        {
            tail.end_ns = gap.end_ns;
            return Ok(GapAdmission {
                merged: true,
                epoch: tail.epoch,
            });
        }
        let id = state.push(track, Work::Gap { format, gap }, 0, 0);
        state.tail_gap[t] = Some(id);
        drop(state);
        self.shared.wake.notify_all();
        Ok(GapAdmission {
            merged: false,
            epoch: gap.epoch,
        })
    }

    pub(crate) fn status(&self) -> Status {
        self.shared.lock().status()
    }

    /// The cause of the first chunk-clock row skipped for a lock, once.
    pub(crate) fn take_clock_busy(&self) -> Option<String> {
        self.shared.lock().clock_busy.take()
    }

    /// Asks the recorder to write everything queued, then finish the manifest,
    /// calling `progress` about every 250 ms. Within the drain deadline this
    /// is the recorder's own result. Past it, no further item is started; the
    /// item being written may still complete within the grace, and if the
    /// queue is then empty the manifest is finished and the result is success.
    /// Otherwise lock waits are cancelled, the thread gets the abort wait to
    /// release the meeting lock, and an explicit error is returned (with
    /// [`RECORDER_RECOVERY_PENDING`] if it is still busy; it then runs the
    /// late cleanup itself when it ends).
    pub(crate) fn finish(mut self, mut progress: impl FnMut(&Status)) -> Result<(), String> {
        let deadline = Instant::now() + self.config.drain_deadline;
        let mut state = self.shared.lock();
        state.closing = true;
        self.shared.wake.notify_all();
        state = self.wait_until(state, deadline, &mut progress);
        if state.finished.is_none() {
            state.abandon = true;
            self.shared.wake.notify_all();
            let grace = Instant::now() + self.config.abandon_grace;
            state = self.wait_until(state, grace, &mut progress);
        }
        if state.finished.is_none() {
            self.shared.abort.store(true, Ordering::SeqCst);
            self.shared.wake.notify_all();
            let abort = Instant::now() + ABORT_WAIT;
            state = self.wait_until(state, abort, &mut progress);
        }
        let result = match state.finished.clone() {
            Some(Ok(())) => Ok(()),
            Some(Err(error)) if !state.abandon => Err(error),
            Some(Err(_)) => Err(format!(
                "{RECORDER_DRAIN_ERROR} (waited {} s)",
                self.config.drain_deadline.as_secs()
            )),
            None => {
                state.late = true;
                Err(format!(
                    "{RECORDER_DRAIN_ERROR} (waited {} s) {RECORDER_RECOVERY_PENDING}",
                    self.config.drain_deadline.as_secs()
                ))
            }
        };
        let finished = state.finished.is_some();
        let status = state.status();
        drop(state);
        progress(&status);
        if finished && let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        result
    }

    fn wait_until<'a>(
        &'a self,
        mut state: MutexGuard<'a, State>,
        until: Instant,
        progress: &mut impl FnMut(&Status),
    ) -> MutexGuard<'a, State> {
        while state.finished.is_none() {
            let now = Instant::now();
            if now >= until {
                break;
            }
            let status = state.status();
            drop(state);
            progress(&status);
            state = self.shared.lock();
            if state.finished.is_none() {
                state = self
                    .shared
                    .done
                    .wait_timeout(state, DRAIN_PROGRESS.min(until - now))
                    .unwrap_or_else(|e| e.into_inner())
                    .0;
            }
        }
        state
    }

    /// Simulates the process dying with work still queued: the thread stops
    /// after its current item without finishing the manifest. Waits for it.
    #[cfg(test)]
    pub(crate) fn abandon_and_wait(mut self) {
        self.shared.lock().abandon = true;
        self.shared.wake.notify_all();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for RecorderQueue {
    /// Not finished (an early return or panic in the session): the recorder
    /// stops after its current write and never marks the manifest finished.
    fn drop(&mut self) {
        let mut state = self.shared.lock();
        if state.finished.is_none() {
            state.abandon = true;
        }
        drop(state);
        self.shared.wake.notify_all();
    }
}

enum Next {
    Item(Item),
    Close,
    Abandon,
}

fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    let detail = payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned());
    match detail {
        Some(detail) => format!("{RECORDER_PANIC_ERROR}: {detail}"),
        None => format!("{RECORDER_PANIC_ERROR}."),
    }
}

/// Writes one item. `Ok(Some(d))` is a chunk write that took `d`.
fn write_item(
    recording: &mut Recording,
    clock: &mut ClockSink,
    shared: &Shared,
    item: &Item,
) -> Result<Option<Duration>, String> {
    match &item.work {
        Work::Chunk {
            block,
            clock: observation,
        } => {
            // Persist the original hardware observation before publication.
            // `app_audio_clock` has no live reader, so a lock skips the row;
            // like a capture-loop write it queues for the writer only briefly.
            let started = Instant::now();
            if let Some(observation) = observation {
                let saved = {
                    let _brief = crate::db::CaptureLoopScope::enter();
                    clock(item.track, *observation)
                };
                match saved {
                    Ok(()) => (),
                    Err(error) if crate::store::sqlite_busy(&error) => {
                        let mut state = shared.lock();
                        if state.clock_busy.is_none() {
                            state.clock_busy = Some(crate::store::describe_error(&error));
                        }
                    }
                    Err(error) => return Err(error.to_string()),
                }
            }
            recording
                .write_chunk(item.track, block)
                .map_err(|error| error.to_string())?;
            Ok(Some(started.elapsed()))
        }
        Work::Gap { format, gap } => {
            recording
                .write_gap(item.track, *format, *gap)
                .map_err(|error| error.to_string())?;
            Ok(None)
        }
    }
}

/// The `before` and `after` test hooks handed to the recorder thread.
#[cfg(test)]
type TestHooks = (
    Option<Arc<dyn Fn(TrackKind, bool) + Send + Sync>>,
    Option<Arc<dyn Fn() + Send + Sync>>,
);

fn recorder_thread(
    shared: &Shared,
    mut recording: Recording,
    mut clock: ClockSink,
    #[cfg(test)] hooks: TestHooks,
) {
    #[cfg(test)]
    let (before, after) = hooks;
    let end = loop {
        let next = {
            let mut state = shared.lock();
            loop {
                // Abandon only while unwritten work remains: with the queue
                // empty a requested close still finishes the manifest.
                if state.abandon && !state.queue.is_empty() {
                    break Next::Abandon;
                }
                if let Some(item) = state.queue.pop_front() {
                    let t = index(item.track);
                    if state.tail_gap[t] == Some(item.id) {
                        state.tail_gap[t] = None;
                    }
                    break Next::Item(item);
                }
                if state.closing {
                    break Next::Close;
                }
                if state.abandon {
                    break Next::Abandon;
                }
                state = shared.wake.wait(state).unwrap_or_else(|e| e.into_inner());
            }
        };
        let item = match next {
            Next::Item(item) => item,
            Next::Close => break Next::Close,
            Next::Abandon => break Next::Abandon,
        };
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            #[cfg(test)]
            if let Some(before) = &before {
                before(item.track, matches!(item.work, Work::Chunk { .. }));
            }
            write_item(&mut recording, &mut clock, shared, &item)
        }));
        #[cfg(test)]
        if let Some(after) = &after {
            after();
        }
        let mut state = shared.lock();
        let t = index(item.track);
        state.queued_ns[t] = state.queued_ns[t].saturating_sub(item.ns);
        state.queued_samples = state.queued_samples.saturating_sub(item.samples);
        if matches!(item.work, Work::Chunk { .. }) {
            state.queued_chunks = state.queued_chunks.saturating_sub(1);
        }
        state.lock_waits = recording.lock_waits();
        let failure = match outcome {
            Ok(Ok(Some(took))) => {
                state.max_write = state.max_write.max(took);
                None
            }
            Ok(Ok(None)) => None,
            Ok(Err(error)) => Some((error, false)),
            Err(payload) => Some((panic_text(payload.as_ref()), true)),
        };
        if let Some((error, panicked)) = failure {
            state.failed = Some(error.clone());
            state.clear_counts();
            drop(state);
            shared.done.notify_all();
            if panicked {
                // State unknown after a panic: release the meeting lock without
                // marking the manifest finished; `recover` reconciles it.
                drop(recording);
            } else {
                // As before the queue: finish is attempted after an error (a
                // poisoned writer refuses it at once) and its result ignored.
                let _ = catch_unwind(AssertUnwindSafe(move || recording.finish()));
            }
            let mut state = shared.lock();
            state.finished = Some(Err(error));
            drop(state);
            shared.done.notify_all();
            return;
        }
        drop(state);
        shared.done.notify_all();
    };
    let result = match end {
        Next::Close => match catch_unwind(AssertUnwindSafe(move || recording.finish())) {
            Ok(result) => result.map_err(|error| error.to_string()),
            Err(payload) => Err(panic_text(payload.as_ref())),
        },
        _ => {
            drop(recording);
            Err("The audio recorder was abandoned before it finished.".into())
        }
    };
    let mut state = shared.lock();
    if let Err(error) = &result
        && state.failed.is_none()
        && !state.abandon
    {
        state.failed = Some(error.clone());
    }
    state.finished = Some(result);
    drop(state);
    shared.done.notify_all();
}

#[cfg(test)]
#[path = "recorder_queue_tests.rs"]
mod tests;
