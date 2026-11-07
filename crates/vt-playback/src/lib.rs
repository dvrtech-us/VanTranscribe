//! Rust-core saved-audio playback. Loading never opens or starts an output stream.
//! Manifest/file reads and bounded FLAC decoding stay on a worker; the output
//! callback only consumes a preallocated SPSC queue and applies track gains.
mod manifest;
mod output;
mod reader;
pub use reader::{SavedAudioPacket, SavedAudioReader};
#[cfg(test)]
mod reader_tests;

use manifest::{Manifest, Timeline};
use rtrb::{Consumer, Producer, RingBuffer};
use serde::Serialize;
use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    thread::{self, JoinHandle},
    time::Duration,
};
use thiserror::Error;

const QUEUE_FRAMES: usize = 8192;
const RENDER_BATCH: usize = 512;
const PAUSED: u8 = 0;
const PLAYING: u8 = 1;
const BUFFERING: u8 = 2;
const STOPPED: u8 = 3;
const ENDED: u8 = 4;
const FAILED: u8 = 5;

#[derive(Debug, Error)]
pub enum PlaybackError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Database(#[from] rusqlite::Error),
    #[error("Invalid saved audio: {0}")]
    Invalid(String),
    #[error("Audio decoding failed: {0}")]
    Decode(String),
    #[error("Audio output failed: {0}")]
    Output(String),
    #[error("Playback worker is unavailable.")]
    Unavailable,
}
pub type Result<T> = std::result::Result<T, PlaybackError>;

#[derive(Clone, Debug, Serialize)]
pub struct PlaybackSnapshot {
    pub meeting_id: String,
    pub status: String,
    pub position_ms: u64,
    pub duration_ms: u64,
    pub mic_volume: f32,
    pub system_volume: f32,
    pub error: Option<String>,
}
#[derive(Clone, Debug, Serialize)]
pub struct Waveform {
    pub meeting_id: String,
    pub duration_ms: u64,
    pub mic: Vec<f32>,
    pub system: Vec<f32>,
}
#[derive(Clone, Copy, Debug)]
pub struct OutputFormat {
    pub sample_rate: u32,
    pub channels: u16,
}
impl OutputFormat {
    pub fn validate(self) -> Result<()> {
        if !(8_000..=192_000).contains(&self.sample_rate) || !(1..=8).contains(&self.channels) {
            return Err(PlaybackError::Output("Unsupported output geometry.".into()));
        }
        Ok(())
    }
}
/// Created and destroyed on the playback worker; implementations need not be Send.
pub trait AudioSink {
    fn play(&self) -> Result<()>;
}
pub struct OpenedSink {
    pub format: OutputFormat,
    pub sink: Box<dyn AudioSink>,
}
/// The factory moves to the worker. Each seek/resume creates a fresh paused sink,
/// so previously queued/hardware-buffered audio cannot leak across control changes.
pub trait AudioSinkFactory: Send + 'static {
    fn open(&mut self, renderer: AudioRenderer) -> Result<OpenedSink>;
}

struct Shared {
    playing: AtomicBool,
    cancel: AtomicBool,
    generation: AtomicU64,
    position_ns: AtomicU64,
    status: AtomicU8,
    mic: AtomicU32,
    system: AtomicU32,
    error: Mutex<Option<String>>,
}
impl Shared {
    fn new() -> Self {
        Self {
            playing: AtomicBool::new(false),
            cancel: AtomicBool::new(false),
            generation: AtomicU64::new(0),
            position_ns: AtomicU64::new(0),
            status: AtomicU8::new(PAUSED),
            mic: AtomicU32::new(1.0f32.to_bits()),
            system: AtomicU32::new(1.0f32.to_bits()),
            error: Mutex::new(None),
        }
    }
    fn fail(&self, error: String) {
        self.playing.store(false, Ordering::Release);
        self.status.store(FAILED, Ordering::Release);
        if let Ok(mut stored) = self.error.lock() {
            *stored = Some(error);
        }
    }
}
#[derive(Clone, Copy)]
struct Frame {
    generation: u64,
    position_ns: u64,
    mic: [f32; 2],
    system: [f32; 2],
    end: bool,
}
/// Allocation-free callback consumer, also usable by deterministic injected sinks.
pub struct AudioRenderer {
    queue: Consumer<Frame>,
    shared: Arc<Shared>,
    generation: u64,
}
impl AudioRenderer {
    pub fn render(&mut self, samples: &mut [f32], channels: u16) {
        self.render_samples(samples, channels);
    }
    pub fn report_error(&self, error: impl Into<String>) {
        if self.generation == self.shared.generation.load(Ordering::Acquire)
            && !self.shared.cancel.load(Ordering::Acquire)
        {
            self.shared.fail(error.into());
        }
    }
    pub(crate) fn render_samples<T: cpal::Sample + cpal::FromSample<f32>>(
        &mut self,
        samples: &mut [T],
        channels: u16,
    ) {
        for sample in samples.iter_mut() {
            *sample = T::EQUILIBRIUM;
        }
        if !(1..=8).contains(&channels) {
            return;
        }
        for frame in samples.chunks_exact_mut(usize::from(channels)) {
            if !self.shared.playing.load(Ordering::Acquire)
                || self.shared.cancel.load(Ordering::Acquire)
                || self.generation != self.shared.generation.load(Ordering::Acquire)
            {
                break;
            }
            let Ok(next) = self.queue.pop() else {
                self.shared.status.store(BUFFERING, Ordering::Release);
                break;
            };
            if next.generation != self.shared.generation.load(Ordering::Acquire) {
                break;
            }
            if next.end {
                self.shared
                    .position_ns
                    .store(next.position_ns, Ordering::Release);
                self.shared.playing.store(false, Ordering::Release);
                self.shared.status.store(ENDED, Ordering::Release);
                break;
            }
            let mic = f32::from_bits(self.shared.mic.load(Ordering::Relaxed));
            let system = f32::from_bits(self.shared.system.load(Ordering::Relaxed));
            let stereo = [
                (next.mic[0] * mic + next.system[0] * system).clamp(-1.0, 1.0),
                (next.mic[1] * mic + next.system[1] * system).clamp(-1.0, 1.0),
            ];
            if channels == 1 {
                frame[0] = T::from_sample((stereo[0] + stereo[1]) * 0.5);
            } else {
                frame[0] = T::from_sample(stereo[0]);
                frame[1] = T::from_sample(stereo[1]);
            }
            // Extra output channels remain silent. Position follows consumed source frames,
            // never wall time; an underrun inserts silence without skipping source speech.
            self.shared
                .position_ns
                .store(next.position_ns, Ordering::Release);
            self.shared.status.store(PLAYING, Ordering::Release);
        }
    }
}

enum Action {
    Play,
    Pause,
    Seek(u64),
    Stop,
}
struct Request {
    action: Action,
    reply: mpsc::Sender<Result<()>>,
}
pub struct Playback {
    meeting: String,
    duration_ns: u64,
    shared: Arc<Shared>,
    sender: SyncSender<Request>,
    worker: Option<JoinHandle<()>>,
}
impl Playback {
    pub fn open(root: &Path, meeting_id: &str) -> Result<Self> {
        Self::with_sink(root, meeting_id, output::CpalFactory)
    }
    pub fn with_sink(
        root: &Path,
        meeting_id: &str,
        factory: impl AudioSinkFactory,
    ) -> Result<Self> {
        let manifest = Manifest::open(root, meeting_id)?;
        let duration_ns = manifest.duration_ns;
        let shared = Arc::new(Shared::new());
        let state = shared.clone();
        let (sender, receiver) = mpsc::sync_channel(32);
        let worker = thread::Builder::new()
            .name("saved-audio-playback".into())
            .spawn(move || {
                let failures = state.clone();
                if catch_unwind(AssertUnwindSafe(|| {
                    run_worker(Timeline::new(manifest), Box::new(factory), state, receiver)
                }))
                .is_err()
                {
                    failures.fail("Playback worker stopped unexpectedly.".into());
                }
            })?;
        Ok(Self {
            meeting: meeting_id.into(),
            duration_ns,
            shared,
            sender,
            worker: Some(worker),
        })
    }
    fn request(&self, action: Action) -> Result<()> {
        let (reply, receiver) = mpsc::channel();
        self.sender
            .send(Request { action, reply })
            .map_err(|_| PlaybackError::Unavailable)?;
        receiver.recv().map_err(|_| PlaybackError::Unavailable)?
    }
    pub fn play(&self) -> Result<()> {
        self.request(Action::Play)
    }
    pub fn pause(&self) -> Result<()> {
        self.shared.playing.store(false, Ordering::Release);
        self.request(Action::Pause)
    }
    pub fn seek(&self, position_ms: u64) -> Result<()> {
        let ns = position_ms.saturating_mul(1_000_000).min(self.duration_ns);
        self.request(Action::Seek(ns))
    }
    pub fn stop(&self) -> Result<()> {
        self.shared.playing.store(false, Ordering::Release);
        self.request(Action::Stop)
    }
    pub fn set_volumes(&self, mic: f32, system: f32) -> Result<()> {
        if !mic.is_finite()
            || !system.is_finite()
            || !(0.0..=1.0).contains(&mic)
            || !(0.0..=1.0).contains(&system)
        {
            return Err(PlaybackError::Invalid(
                "Track volumes must be finite values from 0 to 1.".into(),
            ));
        }
        self.shared.mic.store(mic.to_bits(), Ordering::Release);
        self.shared
            .system
            .store(system.to_bits(), Ordering::Release);
        Ok(())
    }
    pub fn snapshot(&self) -> PlaybackSnapshot {
        let status = match self.shared.status.load(Ordering::Acquire) {
            PLAYING => "playing",
            BUFFERING => "buffering",
            STOPPED => "stopped",
            ENDED => "ended",
            FAILED => "error",
            _ => "paused",
        };
        PlaybackSnapshot {
            meeting_id: self.meeting.clone(),
            status: status.into(),
            position_ms: self
                .shared
                .position_ns
                .load(Ordering::Acquire)
                .min(self.duration_ns)
                / 1_000_000,
            duration_ms: self.duration_ns.div_ceil(1_000_000),
            mic_volume: f32::from_bits(self.shared.mic.load(Ordering::Acquire)),
            system_volume: f32::from_bits(self.shared.system.load(Ordering::Acquire)),
            error: self
                .shared
                .error
                .lock()
                .ok()
                .and_then(|value| value.clone()),
        }
    }
}
impl Drop for Playback {
    fn drop(&mut self) {
        self.shared.playing.store(false, Ordering::Release);
        self.shared.cancel.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

struct Running {
    sink: Box<dyn AudioSink>,
    queue: Producer<Frame>,
    rate: u32,
    origin_ns: u64,
    generated: u64,
    end_queued: bool,
    generation: u64,
}
fn fill(timeline: &mut Timeline, shared: &Shared, running: &mut Running) -> Result<()> {
    for _ in 0..RENDER_BATCH.min(running.queue.slots()) {
        if shared.cancel.load(Ordering::Acquire) || running.end_queued {
            break;
        }
        let at = running.origin_ns.saturating_add(
            (u128::from(running.generated) * 1_000_000_000 / u128::from(running.rate)) as u64,
        );
        let end = at >= timeline.manifest.duration_ns;
        let tracks = if end {
            [[0.0; 2]; 2]
        } else {
            timeline.frame(at)?
        };
        let next = running
            .origin_ns
            .saturating_add(
                (u128::from(running.generated + 1) * 1_000_000_000 / u128::from(running.rate))
                    as u64,
            )
            .min(timeline.manifest.duration_ns);
        let frame = Frame {
            generation: running.generation,
            position_ns: if end {
                timeline.manifest.duration_ns
            } else {
                next
            },
            mic: tracks[0],
            system: tracks[1],
            end,
        };
        running
            .queue
            .push(frame)
            .map_err(|_| PlaybackError::Output("Playback read-ahead queue overflowed.".into()))?;
        running.generated += 1;
        running.end_queued = end;
    }
    Ok(())
}
fn start(
    timeline: &mut Timeline,
    factory: &mut dyn AudioSinkFactory,
    shared: &Arc<Shared>,
) -> Result<Running> {
    if shared.status.load(Ordering::Acquire) == ENDED
        || shared.position_ns.load(Ordering::Acquire) >= timeline.manifest.duration_ns
    {
        shared.position_ns.store(0, Ordering::Release);
    }
    let generation = shared.generation.fetch_add(1, Ordering::AcqRel) + 1;
    let (queue, consumer) = RingBuffer::new(QUEUE_FRAMES);
    let opened = factory.open(AudioRenderer {
        queue: consumer,
        shared: shared.clone(),
        generation,
    })?;
    opened.format.validate()?;
    let mut running = Running {
        sink: opened.sink,
        queue,
        rate: opened.format.sample_rate,
        origin_ns: shared.position_ns.load(Ordering::Acquire),
        generated: 0,
        end_queued: false,
        generation,
    };
    // Prefill on the worker; playback remains silent until these verified frames exist.
    while running.queue.slots() > 0 && !running.end_queued && !shared.cancel.load(Ordering::Acquire)
    {
        fill(timeline, shared, &mut running)?;
    }
    if let Ok(mut error) = shared.error.lock() {
        *error = None;
    }
    shared.status.store(PLAYING, Ordering::Release);
    shared.playing.store(true, Ordering::Release);
    running.sink.play()?;
    Ok(running)
}
fn run_worker(
    mut timeline: Timeline,
    mut factory: Box<dyn AudioSinkFactory>,
    shared: Arc<Shared>,
    receiver: Receiver<Request>,
) {
    let mut running: Option<Running> = None;
    while !shared.cancel.load(Ordering::Acquire) {
        let request = if running.is_some() {
            receiver.try_recv().ok()
        } else {
            receiver.recv_timeout(Duration::from_millis(5)).ok()
        };
        if let Some(Request { action, reply }) = request {
            let result = catch_unwind(AssertUnwindSafe(|| -> Result<()> {
                match action {
                    Action::Play => {
                        if shared.playing.load(Ordering::Acquire) && running.is_some() {
                            return Ok(());
                        }
                        shared.playing.store(false, Ordering::Release);
                        running = None;
                        running = Some(start(&mut timeline, factory.as_mut(), &shared)?);
                    }
                    Action::Pause => {
                        shared.playing.store(false, Ordering::Release);
                        running = None;
                        shared.generation.fetch_add(1, Ordering::AcqRel);
                        shared.status.store(PAUSED, Ordering::Release);
                    }
                    Action::Stop => {
                        shared.playing.store(false, Ordering::Release);
                        running = None;
                        shared.position_ns.store(0, Ordering::Release);
                        shared.generation.fetch_add(1, Ordering::AcqRel);
                        shared.status.store(STOPPED, Ordering::Release);
                        if let Ok(mut error) = shared.error.lock() {
                            *error = None;
                        }
                    }
                    Action::Seek(ns) => {
                        let resume = shared.playing.swap(false, Ordering::AcqRel);
                        running = None;
                        shared.position_ns.store(ns, Ordering::Release);
                        shared.generation.fetch_add(1, Ordering::AcqRel);
                        shared.status.store(PAUSED, Ordering::Release);
                        if resume && ns < timeline.manifest.duration_ns {
                            running = Some(start(&mut timeline, factory.as_mut(), &shared)?);
                        }
                    }
                }
                Ok(())
            }))
            .unwrap_or_else(|_| {
                Err(PlaybackError::Output(
                    "Playback worker operation stopped unexpectedly.".into(),
                ))
            });
            if let Err(error) = &result {
                shared.fail(error.to_string());
                running = None;
            }
            let _ = reply.send(result);
        }
        if let Some(active) = running.as_mut() {
            if !shared.playing.load(Ordering::Acquire) {
                running = None;
            } else if let Err(error) = fill(&mut timeline, &shared, active) {
                shared.fail(error.to_string());
                running = None;
            } else if active.queue.slots() < RENDER_BATCH || active.end_queued {
                thread::sleep(Duration::from_millis(2));
            }
        }
    }
    shared.playing.store(false, Ordering::Release);
    drop(running);
}

/// Computes at most 2,048 peak bins per track using one decoded chunk at a time.
/// This is an offline worker API: it does no output and never writes a cache/database.
pub fn waveform(root: &Path, meeting_id: &str, bins: usize) -> Result<Waveform> {
    if !(1..=2048).contains(&bins) {
        return Err(PlaybackError::Invalid(
            "Waveform requires 1–2048 bins.".into(),
        ));
    }
    let manifest = Manifest::open(root, meeting_id)?;
    let mut peaks = [vec![0.0f32; bins], vec![0.0f32; bins]];
    for (track, output) in peaks.iter_mut().enumerate() {
        for chunk in &manifest.tracks[track] {
            if chunk.path.is_none() {
                continue;
            }
            let decoded = manifest.decode(chunk)?;
            for (index, frame) in decoded.iter().enumerate() {
                let time = chunk.start_ns + (index as u64 * 1_000_000_000 / u64::from(chunk.rate));
                let bin = (u128::from(time) * bins as u128 / u128::from(manifest.duration_ns))
                    .min((bins - 1) as u128) as usize;
                output[bin] = output[bin].max(frame[0].abs().max(frame[1].abs()));
            }
        }
    }
    let [mic, system] = peaks;
    Ok(Waveform {
        meeting_id: manifest.meeting,
        duration_ms: manifest.duration_ns.div_ceil(1_000_000),
        mic,
        system,
    })
}

#[cfg(test)]
mod tests;
