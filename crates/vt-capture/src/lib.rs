//! Bounded line transport for the native macOS capture helper. Capture runs in
//! the helper process; blocking pipe reads belong on a session worker thread.
use serde::{Deserialize, Serialize};
use std::{
    io::{BufRead, BufReader, Read, Write},
    path::Path,
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    thread,
    time::{Duration, Instant},
};
use thiserror::Error;

const MAX_EVENT_BYTES: usize = 4 * 1024 * 1024;
const MAX_INVENTORY_BYTES: usize = 256 * 1024;
const INVENTORY_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Error)]
pub enum CaptureError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("invalid capture configuration: {0}")]
    Invalid(String),
    #[error("capture helper emitted an oversized event")]
    EventTooLarge,
    #[error("Read-only audio discovery timed out. Try again after checking audio devices.")]
    InventoryTimeout,
}
pub type Result<T> = std::result::Result<T, CaptureError>;

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CaptureClock {
    UptimeRaw,
    #[default]
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OutputRouteKind {
    BuiltInSpeakers,
    Headphones,
    External,
    #[default]
    Unknown,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OutputTransport {
    BuiltIn,
    Usb,
    Bluetooth,
    Hdmi,
    DisplayPort,
    Aggregate,
    Virtual,
    Other,
    #[default]
    Unknown,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OutputRouteReason {
    Eligible,
    NoReference,
    NoActiveOutput,
    Headphones,
    ExternalOutput,
    OutputEvidenceUnavailable,
    ProcessEvidenceUnavailable,
    MixedProcessOutputs,
    #[default]
    Unknown,
}

impl OutputRouteReason {
    pub fn explanation(self) -> &'static str {
        match self {
            Self::Eligible => {
                "Built-in speakers and a matching system-audio reference are verified."
            }
            Self::NoReference => "A system-audio reference is unavailable.",
            Self::NoActiveOutput => "No active output process can establish the playback route.",
            Self::Headphones => "Headphones or a connected output jack are in use.",
            Self::ExternalOutput => "The output uses an external, wireless or virtual device.",
            Self::OutputEvidenceUnavailable => {
                "The output cannot be verified as built-in speakers."
            }
            Self::ProcessEvidenceUnavailable => {
                "An active process output route could not be verified."
            }
            Self::MixedProcessOutputs => {
                "Active processes render to different or nondefault outputs."
            }
            Self::Unknown => "The output route is unknown.",
        }
    }
}

/// A conservative observation, never a claim that the default device is the
/// meeting application's route. No process or device identifiers are persisted.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct OutputRoute {
    pub kind: OutputRouteKind,
    pub transport: OutputTransport,
    pub default_output_known: bool,
    pub data_source_known: bool,
    pub speaker_terminal: bool,
    pub headphone_jack_connected: Option<bool>,
    pub active_output_processes: u32,
    pub process_devices_known: bool,
    pub all_processes_same_output: bool,
    pub global_tap: bool,
    pub reference_available: bool,
    pub reason: OutputRouteReason,
}

impl OutputRoute {
    /// Transport alone never establishes speakers or a matching echo reference.
    pub fn aec_eligible(&self) -> bool {
        self.kind == OutputRouteKind::BuiltInSpeakers
            && self.transport == OutputTransport::BuiltIn
            && self.default_output_known
            && self.data_source_known
            && self.speaker_terminal
            && self.headphone_jack_connected == Some(false)
            && self.active_output_processes > 0
            && self.process_devices_known
            && self.all_processes_same_output
            && self.global_tap
            && self.reference_available
            && self.reason == OutputRouteReason::Eligible
    }
}

/// Input transport class for one capture epoch. Only the class is reported;
/// device names and UIDs never leave the helper.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InputTransport {
    BuiltIn,
    Bluetooth,
    Usb,
    Aggregate,
    Virtual,
    Other,
    #[default]
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RebuildReason {
    DeviceChange,
    ProcessExclusionChange,
    #[default]
    #[serde(other)]
    Unknown,
}

/// What first made the helper look for the change that caused a rebuild.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RebuildTrigger {
    /// A Core Audio property listener fired.
    Notification,
    /// The two-second configuration poll.
    Poll,
    /// System wake after sleep.
    Wake,
    /// The helper's own observation while a new stream settles.
    SettleCheck,
    #[default]
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RebuildResult {
    Ok,
    Failed,
    Cancelled,
    #[default]
    #[serde(other)]
    Unknown,
}

/// The helper activity during which a slow Core Audio call happened.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticPhase {
    Idle,
    Stop,
    Start,
    Fingerprint,
    Route,
    #[default]
    #[serde(other)]
    Unknown,
}

/// Capture timing evidence (helper capability `capture_diagnostics`). Every
/// field is a duration, count, enumerated reason or public API name; no device
/// name or UID is carried. These are observations only: nothing here should
/// stop or restart capture.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CaptureDiagnostic {
    /// About once per second from the helper control loop. `callbacks` is the
    /// process-lifetime IOProc callback count (atomic, monotonic); `control_ticks`
    /// counts control-loop iterations; `max_emit_ns` is the longest single stdout
    /// write since the previous heartbeat (pipe backpressure); `queued_packets`
    /// is the ring backlog awaiting the control thread. Missing heartbeats mean
    /// the control thread is blocked.
    Heartbeat {
        #[serde(default)]
        epoch: u32,
        callbacks: u64,
        control_ticks: u64,
        #[serde(default)]
        queued_packets: u64,
        #[serde(default)]
        max_emit_ns: u64,
        #[serde(default)]
        at_ns: u64,
    },
    /// One completed (or failed) rebuild. `trigger_age_ns` runs from the first
    /// observation of the trigger to the start of the rebuild (debounce plus any
    /// blocked fingerprint read); `stop_ns`/`start_ns` time the two halves.
    Rebuild {
        #[serde(default)]
        reason: RebuildReason,
        #[serde(default)]
        trigger: RebuildTrigger,
        previous_epoch: u32,
        epoch: u32,
        #[serde(default)]
        trigger_age_ns: u64,
        #[serde(default)]
        fingerprint_ns: u64,
        #[serde(default)]
        stop_ns: u64,
        #[serde(default)]
        start_ns: u64,
        #[serde(default)]
        result: RebuildResult,
    },
    /// A single Core Audio call that blocked the control thread for more than
    /// 100 ms. `call` is an API name, optionally with a property selector code.
    SlowCall {
        #[serde(deserialize_with = "api_call_name")]
        call: String,
        duration_ns: u64,
        #[serde(default)]
        phase: DiagnosticPhase,
    },
    /// Stream facts for one epoch, emitted once its device has started.
    Epoch {
        epoch: u32,
        sample_rate: u32,
        mic_channels: u16,
        #[serde(default)]
        system_channels: u16,
        #[serde(default)]
        quantum: u32,
        #[serde(default)]
        input_transport: InputTransport,
    },
    /// The configuration baseline was fixed after the new stream settled.
    /// `absorbed` counts post-start changes folded into it rather than rebuilt;
    /// `stable=false` means it was fixed at the ten-second limit.
    Settled {
        epoch: u32,
        settle_ns: u64,
        #[serde(default)]
        absorbed: u32,
        #[serde(default)]
        stable: bool,
    },
    /// A diagnostic from a newer helper, or one that failed validation.
    #[serde(other)]
    Unknown,
}

fn api_call_name<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<String, D::Error> {
    let value = String::deserialize(deserializer)?;
    let valid = !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b':' || b == b'#');
    Ok(if valid { value } else { "other".into() })
}

/// A diagnostic can never end capture: malformed ones decode as `Unknown`.
fn lenient_diagnostic<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<CaptureDiagnostic>, D::Error> {
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(value.map(|value| serde_json::from_value(value).unwrap_or(CaptureDiagnostic::Unknown)))
}

const KNOWN_EVENT_TYPES: [&str; 8] = [
    "source_clock",
    "output_route",
    "format",
    "audio",
    "gap",
    "level",
    "status",
    "error",
];

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CaptureEvent {
    SourceClock {
        clock: CaptureClock,
        origin_ns: u64,
        observed_ns: u64,
    },
    OutputRoute {
        epoch: u32,
        start_ns: u64,
        route: OutputRoute,
    },
    Format {
        track: String,
        sample_rate: u32,
        channels: u16,
        epoch: u32,
        start_ns: u64,
    },
    Audio {
        track: String,
        epoch: u32,
        first_frame: u64,
        host_time_ns: u64,
        samples: Vec<f32>,
    },
    /// `[start_ns, end_ns)` is lost on this track. For `reason == "overflow"`
    /// the helper coalesces drop runs separated by under one second of delivered
    /// audio: the range runs from the first dropped frame to the last, and
    /// `dropped_frames + discarded_frames` (both exact, at the epoch rate) is its
    /// length, where discarded frames were captured between runs but not
    /// published. `overflow_runs` counts the coalesced drop runs. All three are
    /// zero for other reasons and for helpers that predate coalescing.
    Gap {
        track: String,
        epoch: u32,
        start_ns: u64,
        end_ns: u64,
        reason: String,
        #[serde(default)]
        dropped_frames: u64,
        #[serde(default)]
        discarded_frames: u64,
        #[serde(default)]
        overflow_runs: u32,
    },
    Level {
        track: String,
        value: f32,
    },
    /// Human-readable progress, or (helper capability `capture_diagnostics`)
    /// a typed diagnostic. Diagnostics travel as status events so a parent that
    /// predates them ignores them instead of failing on an unknown type.
    Status {
        #[serde(default)]
        message: String,
        #[serde(
            default,
            deserialize_with = "lenient_diagnostic",
            skip_serializing_if = "Option::is_none"
        )]
        diagnostic: Option<CaptureDiagnostic>,
    },
    Error {
        message: String,
        fatal: bool,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct InputDevice {
    pub id: String,
    pub name: String,
    pub is_default: bool,
    pub bluetooth: bool,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DeviceList {
    pub devices: Vec<InputDevice>,
    pub system_supported: bool,
    /// Old helpers omit the handshake; probe new commands only when advertised
    /// by the already-established, read-only list-devices command.
    #[serde(default)]
    pub helper_protocol: u32,
    #[serde(default)]
    pub supported_commands: Vec<String>,
    #[serde(default)]
    pub capture_capabilities: Vec<String>,
}

pub struct CaptureProcess {
    child: Child,
    input: Option<ChildStdin>,
    output: Option<ChildStdout>,
}
impl CaptureProcess {
    pub fn spawn(helper: impl AsRef<Path>, mode: &str, parent_pid: u32) -> Result<Self> {
        Self::spawn_with_device(helper, mode, parent_pid, None)
    }
    pub fn spawn_with_device(
        helper: impl AsRef<Path>,
        mode: &str,
        parent_pid: u32,
        mic_device: Option<&str>,
    ) -> Result<Self> {
        if mode != "remote" && mode != "in_person" {
            return Err(CaptureError::Invalid(
                "mode must be remote or in_person".into(),
            ));
        }
        let mut command = Command::new(helper.as_ref());
        command
            .args(["--mode", mode, "--parent-pid", &parent_pid.to_string()])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        if let Some(device) = mic_device {
            command.args(["--mic-device", device]);
        }
        let mut child = command.spawn()?;
        let input = child.stdin.take();
        let output = child.stdout.take();
        Ok(Self {
            child,
            input,
            output,
        })
    }
    pub fn take_events(&mut self) -> Result<CaptureEvents> {
        self.output
            .take()
            .map(|output| CaptureEvents {
                reader: BufReader::new(output),
                finished: false,
            })
            .ok_or_else(|| {
                CaptureError::Invalid("capture event stream has already been taken".into())
            })
    }
    /// Requests shutdown without waiting. The event reader must keep draining
    /// until EOF so pipe backpressure cannot block the helper's final flush.
    pub fn request_stop(&mut self) -> Result<()> {
        if let Some(mut input) = self.input.take()
            && let Err(error) = input.write_all(b"stop\n")
            && error.kind() != std::io::ErrorKind::BrokenPipe
        {
            return Err(error.into());
        }
        Ok(())
    }
    pub fn stop(&mut self) -> Result<()> {
        let requested = self.request_stop();
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if self.child.try_wait()?.is_some() {
                return requested;
            }
            if Instant::now() >= deadline {
                self.child.kill()?;
                self.child.wait()?;
                return requested;
            }
            thread::sleep(Duration::from_millis(20));
        }
    }
    pub fn id(&self) -> u32 {
        self.child.id()
    }
}
impl Drop for CaptureProcess {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

pub struct CaptureEvents {
    reader: BufReader<ChildStdout>,
    finished: bool,
}
impl Iterator for CaptureEvents {
    type Item = Result<CaptureEvent>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.finished {
            return None;
        }
        match read_event(&mut self.reader) {
            Ok(None) => {
                self.finished = true;
                None
            }
            Ok(Some(event)) => Some(Ok(event)),
            Err(error) => {
                self.finished = true;
                Some(Err(error))
            }
        }
    }
}

fn read_event(reader: &mut impl BufRead) -> Result<Option<CaptureEvent>> {
    let mut bytes = Vec::new();
    let count = reader
        .take((MAX_EVENT_BYTES + 1) as u64)
        .read_until(b'\n', &mut bytes)?;
    if count == 0 {
        return Ok(None);
    }
    if bytes.len() > MAX_EVENT_BYTES {
        return Err(CaptureError::EventTooLarge);
    }
    match serde_json::from_slice(&bytes) {
        Ok(event) => Ok(Some(event)),
        Err(error) => {
            // An event type from a newer helper is ignorable status, not a
            // transport failure. Malformed known events still fail.
            if let Ok(serde_json::Value::Object(object)) =
                serde_json::from_slice::<serde_json::Value>(&bytes)
                && let Some(serde_json::Value::String(kind)) = object.get("type")
                && !KNOWN_EVENT_TYPES.contains(&kind.as_str())
            {
                return Ok(Some(CaptureEvent::Status {
                    message: String::new(),
                    diagnostic: Some(CaptureDiagnostic::Unknown),
                }));
            }
            Err(error.into())
        }
    }
}

/// Known applications are exact bundle identifiers shared with the native helper.
/// Browser microphone use is advisory; it is not proof of a meeting.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MeetingAppKind {
    Meeting,
    Browser,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MeetingApp {
    pub id: String,
    pub name: String,
    pub kind: MeetingAppKind,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MeetingAppSnapshot {
    pub complete: bool,
    pub apps: Vec<MeetingApp>,
    pub error: Option<String>,
}

pub fn known_meeting_apps() -> &'static [MeetingApp] {
    static CATALOG: std::sync::OnceLock<Vec<MeetingApp>> = std::sync::OnceLock::new();
    CATALOG.get_or_init(|| {
        let mut result = Vec::new();
        for line in include_str!("../resources/meeting-apps.def").lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with("//") {
                continue;
            }
            let fields = line
                .strip_prefix("VT_MEETING_APP(")
                .and_then(|v| v.strip_suffix(')'))
                .expect("catalog macro");
            let [id, name, kind]: [String; 3] =
                serde_json::from_str(&format!("[{fields}]")).expect("catalog fields");
            let kind =
                serde_json::from_value(serde_json::Value::String(kind)).expect("catalog kind");
            assert!(
                !result.iter().any(|app: &MeetingApp| app.id == id),
                "duplicate catalog ID"
            );
            result.push(MeetingApp { id, name, kind });
        }
        result
    })
}

pub fn list_devices(helper: impl AsRef<Path>) -> Result<DeviceList> {
    list_devices_with_timeout(helper.as_ref(), INVENTORY_TIMEOUT)
}

fn list_devices_with_timeout(helper: &Path, timeout: Duration) -> Result<DeviceList> {
    let mut budget = ReadOnlyBudget::new(timeout);
    Ok(serde_json::from_slice(&read_only(
        helper,
        ReadOnlyCommand::ListDevices,
        &mut budget,
    )?)?)
}

/// Always performs the established, safe device handshake before the new flag.
/// Incomplete observations must never be interpreted as an app having stopped.
/// Transport errors likewise provide no absence evidence.
pub fn meeting_apps(helper: impl AsRef<Path>) -> Result<MeetingAppSnapshot> {
    meeting_apps_with_timeout(helper.as_ref(), INVENTORY_TIMEOUT)
}

fn meeting_apps_with_timeout(helper: &Path, timeout: Duration) -> Result<MeetingAppSnapshot> {
    let mut budget = ReadOnlyBudget::new(timeout);
    let devices: DeviceList = serde_json::from_slice(&read_only(
        helper,
        ReadOnlyCommand::ListDevices,
        &mut budget,
    )?)?;
    if devices.helper_protocol < 4
        || !devices
            .supported_commands
            .iter()
            .any(|s| s == "meeting_apps")
    {
        return Ok(MeetingAppSnapshot {
            complete: false,
            apps: Vec::new(),
            error: Some(
                "This capture helper does not support meeting-app observation. Update the helper."
                    .into(),
            ),
        });
    }
    let mut snapshot: MeetingAppSnapshot = serde_json::from_slice(&read_only(
        helper,
        ReadOnlyCommand::MeetingApps,
        &mut budget,
    )?)
    .map_err(|_| {
        CaptureError::Invalid("Meeting-app observation returned malformed output.".into())
    })?;
    if snapshot.apps.len() > known_meeting_apps().len() {
        return Err(CaptureError::Invalid(
            "Meeting-app observation exceeded its catalog bound.".into(),
        ));
    }
    for app in &snapshot.apps {
        if !known_meeting_apps().iter().any(|known| known == app) {
            return Err(CaptureError::Invalid(
                "Meeting-app observation returned an unknown or inconsistent app.".into(),
            ));
        }
    }
    // Native diagnostics are deliberately not relayed: helper output cannot add
    // raw process metadata to the public observation API.
    if snapshot.complete && snapshot.error.is_some() {
        return Err(CaptureError::Invalid(
            "Meeting-app observation has contradictory completeness evidence.".into(),
        ));
    }
    snapshot.error = (!snapshot.complete).then(|| {
        "Some microphone activity could not be attributed. Meeting-app status is unknown.".into()
    });
    snapshot.apps.sort_by(|a, b| a.id.cmp(&b.id));
    snapshot.apps.dedup_by(|a, b| a.id == b.id);
    Ok(snapshot)
}

#[derive(Clone, Copy)]
enum ReadOnlyCommand {
    ListDevices,
    MeetingApps,
}
impl ReadOnlyCommand {
    fn flag(self) -> &'static str {
        match self {
            Self::ListDevices => "--list-devices",
            Self::MeetingApps => "--meeting-apps",
        }
    }
}
struct ReadOnlyBudget {
    deadline: Instant,
    remaining_bytes: usize,
}
impl ReadOnlyBudget {
    fn new(timeout: Duration) -> Self {
        Self {
            deadline: Instant::now() + timeout,
            remaining_bytes: MAX_INVENTORY_BYTES,
        }
    }
}

#[cfg(unix)]
fn read_only(
    helper: &Path,
    command: ReadOnlyCommand,
    budget: &mut ReadOnlyBudget,
) -> Result<Vec<u8>> {
    use nix::fcntl::{FcntlArg, OFlag, fcntl};
    use std::os::unix::process::CommandExt;
    struct InventoryChild {
        child: Child,
        complete: bool,
    }
    impl Drop for InventoryChild {
        fn drop(&mut self) {
            if !self.complete {
                // A helper descendant may retain stdout after its parent exits.
                // Its isolated process group cannot escape the read-only deadline.
                if let Ok(pid) = i32::try_from(self.child.id()) {
                    let _ = nix::sys::signal::killpg(
                        nix::unistd::Pid::from_raw(pid),
                        nix::sys::signal::Signal::SIGKILL,
                    );
                }
            }
            if !matches!(self.child.try_wait(), Ok(Some(_))) {
                let _ = self.child.kill();
                let _ = self.child.wait();
            }
        }
    }
    if Instant::now() >= budget.deadline {
        return Err(CaptureError::InventoryTimeout);
    }
    let mut child = InventoryChild {
        child: Command::new(helper)
            .arg(command.flag())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()?,
        complete: false,
    };
    let mut stdout = child.child.stdout.take().ok_or_else(|| {
        CaptureError::Invalid("Read-only discovery output is unavailable.".into())
    })?;
    let flags = fcntl(&stdout, FcntlArg::F_GETFL).map_err(std::io::Error::from)?;
    fcntl(
        &stdout,
        FcntlArg::F_SETFL(OFlag::from_bits_retain(flags) | OFlag::O_NONBLOCK),
    )
    .map_err(std::io::Error::from)?;
    let mut bytes = Vec::new();
    let mut eof = false;
    let mut buffer = [0; 8192];
    loop {
        match stdout.read(&mut buffer) {
            Ok(0) => eof = true,
            Ok(count) => {
                if count > budget.remaining_bytes {
                    return Err(CaptureError::EventTooLarge);
                }
                budget.remaining_bytes -= count;
                bytes.extend_from_slice(&buffer[..count]);
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => (),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error.into()),
        }
        if let Some(status) = child.child.try_wait()? {
            if !status.success() {
                return Err(CaptureError::Invalid(
                    "Read-only audio discovery failed. Retry after checking audio availability."
                        .into(),
                ));
            }
            if eof {
                child.complete = true;
                return Ok(bytes);
            }
        }
        if Instant::now() >= budget.deadline {
            return Err(CaptureError::InventoryTimeout);
        }
        thread::sleep(Duration::from_millis(2));
    }
}

#[cfg(not(unix))]
fn read_only(
    _helper: &Path,
    _command: ReadOnlyCommand,
    _budget: &mut ReadOnlyBudget,
) -> Result<Vec<u8>> {
    Err(CaptureError::Invalid(
        "Native audio discovery is unavailable on this platform.".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn source_clock_transport_preserves_the_common_epoch_and_unknown_clocks_stay_unknown() {
        let event:CaptureEvent=serde_json::from_str(r#"{"type":"source_clock","clock":"uptime_raw","origin_ns":1000000000,"observed_ns":1000123456}"#).unwrap();
        assert!(matches!(
            event,
            CaptureEvent::SourceClock {
                clock: CaptureClock::UptimeRaw,
                origin_ns: 1_000_000_000,
                observed_ns: 1_000_123_456
            }
        ));
        let unknown: CaptureEvent = serde_json::from_str(
            r#"{"type":"source_clock","clock":"future_clock","origin_ns":1,"observed_ns":2}"#,
        )
        .unwrap();
        assert!(matches!(
            unknown,
            CaptureEvent::SourceClock {
                clock: CaptureClock::Unknown,
                ..
            }
        ));
    }

    #[cfg(unix)]
    fn inventory_helper(body: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let folder =
            std::env::temp_dir().join(format!("vt-inventory-test-{}-{nonce}", std::process::id()));
        std::fs::create_dir(&folder).unwrap();
        let path = folder.join("helper");
        std::fs::write(&path,format!("#!/usr/bin/env python3\nimport sys,time\nassert sys.argv[1:]==['--list-devices']\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        path
    }
    #[cfg(unix)]
    #[test]
    fn device_inventory_caps_time_and_bytes_and_only_uses_the_safe_command() {
        let success = inventory_helper(
            "print('{\"devices\":[],\"system_supported\":true,\"helper_protocol\":3,\"supported_commands\":[\"source_clock\"]}')",
        );
        let devices = list_devices_with_timeout(&success, Duration::from_secs(1)).unwrap();
        assert_eq!(devices.helper_protocol, 3);
        std::fs::remove_dir_all(success.parent().unwrap()).unwrap();
        let hung = inventory_helper("time.sleep(30)");
        let begin = Instant::now();
        assert!(matches!(
            list_devices_with_timeout(&hung, Duration::from_millis(80)),
            Err(CaptureError::InventoryTimeout)
        ));
        assert!(begin.elapsed() < Duration::from_secs(2));
        std::fs::remove_dir_all(hung.parent().unwrap()).unwrap();
        let oversized = inventory_helper(&format!(
            "sys.stdout.write('x'*{});sys.stdout.flush();time.sleep(30)",
            MAX_INVENTORY_BYTES + 1
        ));
        assert!(matches!(
            list_devices_with_timeout(&oversized, Duration::from_secs(2)),
            Err(CaptureError::EventTooLarge)
        ));
        std::fs::remove_dir_all(oversized.parent().unwrap()).unwrap();
    }
    #[cfg(unix)]
    fn meeting_helper(body: &str) -> std::path::PathBuf {
        let path = inventory_helper("");
        std::fs::write(&path, format!("#!/usr/bin/env python3\nimport sys,time,json,os\nfrom pathlib import Path\nbase=Path(__file__).parent\nwith (base/'commands').open('a') as log: log.write(sys.argv[1]+'\\n')\n{body}\n")).unwrap();
        path
    }

    #[cfg(unix)]
    #[test]
    fn meeting_handshake_never_sends_new_flags_to_legacy_or_unadvertised_helpers() {
        for inventory in [
            r#"{"devices":[],"system_supported":true}"#,
            r#"{"devices":[],"system_supported":true,"helper_protocol":3,"supported_commands":["meeting_apps"]}"#,
            r#"{"devices":[],"system_supported":true,"helper_protocol":4,"supported_commands":[]}"#,
        ] {
            let helper = meeting_helper(&format!(
                "if sys.argv[1]=='--list-devices': print({inventory:?})\nelse: (base/'CAPTURE_STARTED').touch(); time.sleep(30)"
            ));
            let snapshot = meeting_apps(&helper).unwrap();
            assert!(!snapshot.complete);
            assert!(snapshot.apps.is_empty() && snapshot.error.is_some());
            assert_eq!(
                std::fs::read_to_string(helper.parent().unwrap().join("commands")).unwrap(),
                "--list-devices\n"
            );
            assert!(!helper.parent().unwrap().join("CAPTURE_STARTED").exists());
            std::fs::remove_dir_all(helper.parent().unwrap()).unwrap();
        }
    }

    #[cfg(unix)]
    fn snapshot_helper(
        snapshot: &str,
        inventory_delay: f32,
        snapshot_delay: f32,
    ) -> std::path::PathBuf {
        meeting_helper(&format!(
            "if sys.argv[1]=='--list-devices':\n time.sleep({inventory_delay}); print('{{\"devices\":[],\"system_supported\":true,\"helper_protocol\":4,\"supported_commands\":[\"meeting_apps\"]}}')\nelif sys.argv[1]=='--meeting-apps':\n time.sleep({snapshot_delay}); print({snapshot:?})\nelse: raise Exception('unexpected command')"
        ))
    }

    #[cfg(unix)]
    #[test]
    fn meeting_snapshots_are_catalog_validated_deduplicated_and_privacy_normalized() {
        let helper = snapshot_helper(
            r#"{"complete":true,"apps":[{"id":"us.zoom.xos","name":"Zoom","kind":"meeting"},{"id":"com.apple.Safari","name":"Safari","kind":"browser"},{"id":"us.zoom.xos","name":"Zoom","kind":"meeting"}],"error":null}"#,
            0.0,
            0.0,
        );
        let snapshot = meeting_apps(&helper).unwrap();
        assert!(snapshot.complete);
        assert_eq!(snapshot.apps.len(), 2);
        assert_eq!(snapshot.apps[0].kind, MeetingAppKind::Browser);
        assert_eq!(
            std::fs::read_to_string(helper.parent().unwrap().join("commands")).unwrap(),
            "--list-devices\n--meeting-apps\n"
        );
        std::fs::remove_dir_all(helper.parent().unwrap()).unwrap();
        for invalid in [
            r#"{"complete":true,"apps":[{"id":"com.google.Chrome.helper","name":"Google Chrome","kind":"browser"}],"error":null}"#,
            r#"{"complete":true,"apps":[{"id":"com.google.Chrome","name":"Google Chrome","kind":"meeting"}],"error":null}"#,
            r#"{"complete":true,"apps":[{"id":"us.zoom.xos","name":"untrusted private title","kind":"meeting"}],"error":null}"#,
            r#"{"complete":true,"apps":[],"error":"private diagnostic"}"#,
            r#"{"complete":true,"apps":[],"error":null,"pid":123}"#,
            r#"{"complete":false,"apps":[{"id":"us.zoom.xos","name":"Zoom","kind":"private diagnostic"}],"error":null}"#,
            "not JSON",
        ] {
            let helper = snapshot_helper(invalid, 0.0, 0.0);
            let error = meeting_apps(&helper).unwrap_err().to_string();
            assert!(!error.contains("private"), "{error}");
            std::fs::remove_dir_all(helper.parent().unwrap()).unwrap();
        }
        let helper = snapshot_helper(
            r#"{"complete":false,"apps":[],"error":"private PID and path"}"#,
            0.0,
            0.0,
        );
        let snapshot = meeting_apps(&helper).unwrap();
        assert!(!snapshot.complete && !snapshot.error.unwrap().contains("private"));
        std::fs::remove_dir_all(helper.parent().unwrap()).unwrap();
        assert_eq!(known_meeting_apps().len(), 11);
    }

    #[cfg(unix)]
    #[test]
    fn meeting_observation_shares_one_deadline_and_byte_budget_and_reaps_on_failure() {
        let helper = snapshot_helper(r#"{"complete":true,"apps":[],"error":null}"#, 0.08, 0.08);
        let begin = Instant::now();
        assert!(matches!(
            meeting_apps_with_timeout(&helper, Duration::from_millis(140)),
            Err(CaptureError::InventoryTimeout)
        ));
        assert!(begin.elapsed() < Duration::from_secs(2));
        std::fs::remove_dir_all(helper.parent().unwrap()).unwrap();
        let helper = meeting_helper(&format!(
            "(base/'pid').write_text(str(os.getpid()))\nif sys.argv[1]=='--list-devices': print(json.dumps(dict(devices=[],system_supported=True,helper_protocol=4,supported_commands=['meeting_apps'],padding='x'*{})))\nelse: sys.stdout.write('x'*10000); sys.stdout.flush(); time.sleep(30)",
            MAX_INVENTORY_BYTES - 5000
        ));
        assert!(matches!(
            meeting_apps(&helper),
            Err(CaptureError::EventTooLarge)
        ));
        let pid: i32 = std::fs::read_to_string(helper.parent().unwrap().join("pid"))
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(
            nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None),
            Err(nix::errno::Errno::ESRCH)
        );
        std::fs::remove_dir_all(helper.parent().unwrap()).unwrap();
        // A reaped parent with a descendant retaining the pipe cannot evade timeout.
        // Allow the fixture to start under concurrent builds before testing pipe cleanup.
        let helper = meeting_helper(
            "(base/'pid').write_text(str(os.getpid()))\nif os.fork()==0: time.sleep(30); os._exit(0)\nos._exit(0)",
        );
        assert!(matches!(
            meeting_apps_with_timeout(&helper, Duration::from_secs(3)),
            Err(CaptureError::InventoryTimeout)
        ));
        let pid: i32 = std::fs::read_to_string(helper.parent().unwrap().join("pid"))
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(
            nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None),
            Err(nix::errno::Errno::ESRCH)
        );
        std::fs::remove_dir_all(helper.parent().unwrap()).unwrap();
    }

    #[test]
    fn readonly_device_handshake_distinguishes_legacy_helpers() {
        let old: DeviceList =
            serde_json::from_str(r#"{"devices":[],"system_supported":true}"#).unwrap();
        assert_eq!(old.helper_protocol, 0);
        assert!(old.supported_commands.is_empty());
        let new: DeviceList = serde_json::from_str(r#"{"devices":[],"system_supported":true,"helper_protocol":2,"supported_commands":["list_devices","output_route"]}"#).unwrap();
        assert_eq!(new.helper_protocol, 2);
        assert!(
            new.supported_commands
                .iter()
                .any(|command| command == "output_route")
        );
    }
    fn speaker_route() -> OutputRoute {
        OutputRoute {
            kind: OutputRouteKind::BuiltInSpeakers,
            transport: OutputTransport::BuiltIn,
            default_output_known: true,
            data_source_known: true,
            speaker_terminal: true,
            headphone_jack_connected: Some(false),
            active_output_processes: 2,
            process_devices_known: true,
            all_processes_same_output: true,
            global_tap: true,
            reference_available: true,
            reason: OutputRouteReason::Eligible,
        }
    }

    #[test]
    fn output_route_roundtrips_typed_evidence_and_hardware_epoch() {
        let event = CaptureEvent::OutputRoute {
            epoch: 4,
            start_ns: 123_456,
            route: speaker_route(),
        };
        let bytes = serde_json::to_vec(&event).unwrap();
        let decoded: CaptureEvent = serde_json::from_slice(&bytes).unwrap();
        let CaptureEvent::OutputRoute {
            epoch,
            start_ns,
            route,
        } = decoded
        else {
            panic!("wrong event")
        };
        assert_eq!((epoch, start_ns), (4, 123_456));
        assert_eq!(route, speaker_route());
        assert!(route.aec_eligible());
        for name in [
            "built_in",
            "usb",
            "bluetooth",
            "hdmi",
            "display_port",
            "aggregate",
            "virtual",
            "other",
            "unknown",
        ] {
            let transport: OutputTransport = serde_json::from_str(&format!("\"{name}\"")).unwrap();
            let mut route = speaker_route();
            route.transport = transport;
            assert_eq!(route.aec_eligible(), name == "built_in");
        }
    }

    #[test]
    fn aec_requires_every_positive_fact_not_an_eligible_claim_alone() {
        let changes: [fn(&mut OutputRoute); 13] = [
            |r| r.kind = OutputRouteKind::Headphones,
            |r| r.kind = OutputRouteKind::Unknown,
            |r| r.default_output_known = false,
            |r| r.data_source_known = false,
            |r| r.speaker_terminal = false,
            |r| r.headphone_jack_connected = Some(true),
            |r| r.headphone_jack_connected = None,
            |r| r.active_output_processes = 0,
            |r| r.process_devices_known = false,
            |r| r.all_processes_same_output = false,
            |r| r.global_tap = false,
            |r| r.reference_available = false,
            |r| r.reason = OutputRouteReason::Unknown,
        ];
        for change in changes {
            let mut route = speaker_route();
            change(&mut route);
            assert!(!route.aec_eligible(), "unexpected eligibility: {route:?}");
        }
        let partial: OutputRoute =
            serde_json::from_str(r#"{"transport":"built_in","reason":"eligible"}"#).unwrap();
        assert!(!partial.aec_eligible());
        assert_eq!(OutputRoute::default().reason, OutputRouteReason::Unknown);
        assert!(!OutputRoute::default().aec_eligible());
    }

    #[test]
    fn events_decode_without_losing_hardware_positions() {
        let event:CaptureEvent=serde_json::from_str(r#"{"type":"audio","track":"mic","epoch":2,"first_frame":512,"host_time_ns":123456789,"samples":[0.25,-0.5]}"#).unwrap();
        assert!(matches!(
            event,
            CaptureEvent::Audio {
                epoch: 2,
                first_frame: 512,
                host_time_ns: 123456789,
                ..
            }
        ));
    }
    fn decode(line: &str) -> CaptureEvent {
        read_event(&mut std::io::Cursor::new(format!("{line}\n")))
            .unwrap()
            .unwrap()
    }

    fn diagnostic(line: &str) -> CaptureDiagnostic {
        match decode(line) {
            CaptureEvent::Status {
                diagnostic: Some(diagnostic),
                ..
            } => diagnostic,
            other => panic!("not a diagnostic: {other:?}"),
        }
    }

    // Literal lines in the helper's sorted-key JSON shape.
    #[test]
    fn capture_diagnostics_decode_from_status_events() {
        assert_eq!(
            diagnostic(
                r#"{"diagnostic":{"at_ns":5000000000,"callbacks":4700,"control_ticks":98,"epoch":1,"kind":"heartbeat","max_emit_ns":1200,"queued_packets":3},"message":"Capture diagnostic.","type":"status"}"#
            ),
            CaptureDiagnostic::Heartbeat {
                epoch: 1,
                callbacks: 4700,
                control_ticks: 98,
                queued_packets: 3,
                max_emit_ns: 1200,
                at_ns: 5_000_000_000,
            }
        );
        assert_eq!(
            diagnostic(
                r#"{"diagnostic":{"epoch":4,"fingerprint_ns":2100000,"kind":"rebuild","previous_epoch":3,"reason":"device_change","result":"ok","start_ns":180000000,"stop_ns":36000000000,"trigger":"notification","trigger_age_ns":76000000},"message":"Capture diagnostic.","type":"status"}"#
            ),
            CaptureDiagnostic::Rebuild {
                reason: RebuildReason::DeviceChange,
                trigger: RebuildTrigger::Notification,
                previous_epoch: 3,
                epoch: 4,
                trigger_age_ns: 76_000_000,
                fingerprint_ns: 2_100_000,
                stop_ns: 36_000_000_000,
                start_ns: 180_000_000,
                result: RebuildResult::Ok,
            }
        );
        assert_eq!(
            diagnostic(
                r#"{"diagnostic":{"call":"AudioDeviceStop","duration_ns":35000000000,"kind":"slow_call","phase":"stop"},"message":"Capture diagnostic.","type":"status"}"#
            ),
            CaptureDiagnostic::SlowCall {
                call: "AudioDeviceStop".into(),
                duration_ns: 35_000_000_000,
                phase: DiagnosticPhase::Stop,
            }
        );
        assert!(matches!(
            diagnostic(
                r#"{"diagnostic":{"call":"AudioObjectGetPropertyData:prs#","duration_ns":150000000,"kind":"slow_call","phase":"fingerprint"},"message":"Capture diagnostic.","type":"status"}"#
            ),
            CaptureDiagnostic::SlowCall { call, phase: DiagnosticPhase::Fingerprint, .. } if call == "AudioObjectGetPropertyData:prs#"
        ));
        assert_eq!(
            diagnostic(
                r#"{"diagnostic":{"epoch":2,"input_transport":"bluetooth","kind":"epoch","mic_channels":1,"quantum":512,"sample_rate":16000,"system_channels":2},"message":"Capture diagnostic.","type":"status"}"#
            ),
            CaptureDiagnostic::Epoch {
                epoch: 2,
                sample_rate: 16_000,
                mic_channels: 1,
                system_channels: 2,
                quantum: 512,
                input_transport: InputTransport::Bluetooth,
            }
        );
        assert_eq!(
            diagnostic(
                r#"{"diagnostic":{"absorbed":2,"epoch":2,"kind":"settled","settle_ns":1250000000,"stable":true},"message":"Capture diagnostic.","type":"status"}"#
            ),
            CaptureDiagnostic::Settled {
                epoch: 2,
                settle_ns: 1_250_000_000,
                absorbed: 2,
                stable: true,
            }
        );
        for name in [
            "built_in",
            "bluetooth",
            "usb",
            "aggregate",
            "virtual",
            "other",
            "unknown",
            "thunderbolt",
        ] {
            let transport: InputTransport = serde_json::from_str(&format!("\"{name}\"")).unwrap();
            assert_eq!(
                transport == InputTransport::Unknown,
                matches!(name, "unknown" | "thunderbolt")
            );
        }
    }

    #[test]
    fn diagnostics_tolerate_unknown_fields_values_and_kinds_without_ending_capture() {
        // Unknown fields and enumeration values from a newer helper.
        assert!(matches!(
            diagnostic(
                r#"{"diagnostic":{"call":"AudioDeviceStart","duration_ns":1,"future":[1],"kind":"slow_call","phase":"future_phase"},"message":"","type":"status"}"#
            ),
            CaptureDiagnostic::SlowCall {
                phase: DiagnosticPhase::Unknown,
                ..
            }
        ));
        assert!(matches!(
            diagnostic(
                r#"{"diagnostic":{"epoch":1,"kind":"rebuild","previous_epoch":0,"reason":"future","result":"future","trigger":"future"},"type":"status"}"#
            ),
            CaptureDiagnostic::Rebuild {
                reason: RebuildReason::Unknown,
                trigger: RebuildTrigger::Unknown,
                result: RebuildResult::Unknown,
                ..
            }
        ));
        // Unknown kind, malformed required fields and non-object payloads.
        for payload in [
            r#"{"kind":"future_kind","x":1}"#,
            r#"{"kind":"heartbeat","callbacks":"many"}"#,
            r#"{"kind":"heartbeat"}"#,
            r#""text""#,
        ] {
            assert_eq!(
                diagnostic(&format!(
                    r#"{{"type":"status","message":"m","diagnostic":{payload}}}"#
                )),
                CaptureDiagnostic::Unknown
            );
        }
        // A call name is only ever an API name/selector code.
        assert!(matches!(
            diagnostic(
                r#"{"diagnostic":{"call":"AudioDeviceStart for My Headphones","duration_ns":1,"kind":"slow_call"},"type":"status"}"#
            ),
            CaptureDiagnostic::SlowCall { call, .. } if call == "other"
        ));
        // Plain status from older helpers still decodes, without a diagnostic.
        assert!(matches!(
            decode(r#"{"type":"status","message":"Microphone is recording."}"#),
            CaptureEvent::Status { message, diagnostic: None } if message == "Microphone is recording."
        ));
        // An unknown event type is ignorable status; malformed known types still fail.
        assert!(matches!(
            decode(r#"{"type":"future_event","value":1}"#),
            CaptureEvent::Status {
                diagnostic: Some(CaptureDiagnostic::Unknown),
                ..
            }
        ));
        for line in [
            r#"{"type":"gap","track":"mic"}"#,
            r#"{"no_type":true}"#,
            r#"{"type":7}"#,
            "[]",
        ] {
            assert!(
                read_event(&mut std::io::Cursor::new(format!("{line}\n"))).is_err(),
                "{line}"
            );
        }
    }

    #[test]
    fn a_parent_that_predates_diagnostics_reads_them_as_plain_status() {
        // The previous protocol's status shape: unknown fields are ignored.
        #[derive(Deserialize)]
        #[serde(tag = "type", rename_all = "snake_case")]
        enum Previous {
            Status { message: String },
        }
        let line = r#"{"diagnostic":{"callbacks":1,"control_ticks":1,"kind":"heartbeat"},"message":"Capture diagnostic.","type":"status"}"#;
        let Previous::Status { message } = serde_json::from_str(line).unwrap();
        assert_eq!(message, "Capture diagnostic.");
        let event = CaptureEvent::Status {
            message: "Capture diagnostic.".into(),
            diagnostic: Some(CaptureDiagnostic::Settled {
                epoch: 1,
                settle_ns: 2,
                absorbed: 0,
                stable: true,
            }),
        };
        let encoded = serde_json::to_string(&event).unwrap();
        let Previous::Status { .. } = serde_json::from_str(&encoded).unwrap();
        assert_eq!(
            diagnostic(&encoded),
            CaptureDiagnostic::Settled {
                epoch: 1,
                settle_ns: 2,
                absorbed: 0,
                stable: true,
            }
        );
        let plain = serde_json::to_string(&CaptureEvent::Status {
            message: "x".into(),
            diagnostic: None,
        })
        .unwrap();
        assert!(!plain.contains("diagnostic"));
    }

    #[test]
    fn coalesced_overflow_gaps_carry_exact_frame_accounting() {
        let CaptureEvent::Gap {
            start_ns,
            end_ns,
            reason,
            dropped_frames,
            discarded_frames,
            overflow_runs,
            ..
        } = decode(
            r#"{"discarded_frames":720,"dropped_frames":1600,"end_ns":1290000000,"epoch":2,"overflow_runs":10,"reason":"overflow","start_ns":1000000000,"track":"mic","type":"gap"}"#,
        )
        else {
            panic!("wrong event")
        };
        assert_eq!(reason, "overflow");
        assert_eq!(
            (dropped_frames, discarded_frames, overflow_runs),
            (1600, 720, 10)
        );
        // At 8 kHz the range is exactly the dropped plus discarded frames.
        assert_eq!(
            end_ns - start_ns,
            (dropped_frames + discarded_frames) * 1_000_000_000 / 8_000
        );
        let CaptureEvent::Gap {
            dropped_frames,
            discarded_frames,
            overflow_runs,
            ..
        } = decode(
            r#"{"end_ns":2,"epoch":0,"reason":"device_change","start_ns":1,"track":"system","type":"gap"}"#,
        )
        else {
            panic!("wrong event")
        };
        assert_eq!((dropped_frames, discarded_frames, overflow_runs), (0, 0, 0));
    }

    #[test]
    fn invalid_mode_is_rejected_before_process_creation() {
        assert!(CaptureProcess::spawn("missing", "invalid", 0).is_err());
    }

    #[test]
    fn oversized_and_malformed_events_fail_without_unbounded_reads() {
        let mut oversized = std::io::Cursor::new(vec![b'x'; MAX_EVENT_BYTES + 100]);
        assert!(matches!(
            read_event(&mut oversized),
            Err(CaptureError::EventTooLarge)
        ));
        assert_eq!(oversized.position(), (MAX_EVENT_BYTES + 1) as u64);
        assert!(read_event(&mut std::io::Cursor::new(b"not json\n")).is_err());
        assert!(
            read_event(&mut std::io::Cursor::new(b""))
                .unwrap()
                .is_none()
        );
    }

    #[cfg(unix)]
    #[test]
    fn requesting_stop_keeps_final_events_available_until_eof() {
        use std::os::unix::fs::PermissionsExt;
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("vt-capture-stop-{}-{nonce}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        let helper = dir.join("helper");
        std::fs::write(&helper, b"#!/bin/sh\nprintf '%s\\n' '{\"type\":\"status\",\"message\":\"recording\"}'\nIFS= read -r action\n[ \"$action\" = stop ] || exit 1\nprintf '%s\\n' '{\"type\":\"status\",\"message\":\"final flush\"}'\n").unwrap();
        std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o700)).unwrap();
        let mut process = CaptureProcess::spawn(&helper, "in_person", std::process::id()).unwrap();
        let events = process.take_events().unwrap();
        process.request_stop().unwrap();
        let events = events.collect::<Result<Vec<_>>>().unwrap();
        assert_eq!(events.len(), 2);
        assert!(
            matches!(&events[1], CaptureEvent::Status { message, .. } if message == "final flush")
        );
        process.stop().unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }
}
