use serde_json::{Value, json};
use std::{
    process::Command,
    time::{Duration, Instant},
};
pub(super) struct Telemetry {
    start: Instant,
    previous: Instant,
    resources: Vec<Value>,
    unavailable: bool,
}
impl Telemetry {
    pub fn new() -> Self {
        Self {
            start: Instant::now(),
            previous: Instant::now() - Duration::from_secs(2),
            resources: vec![],
            unavailable: false,
        }
    }
    pub fn sample(&mut self) {
        if self.previous.elapsed() < Duration::from_secs(1) {
            return;
        }
        self.previous = Instant::now();
        let result = Command::new("ps")
            .args(["-p", &std::process::id().to_string(), "-o", "%cpu=,rss="])
            .output();
        let Some((cpu, rss)) = result.ok().filter(|r| r.status.success()).and_then(|r| {
            let text = String::from_utf8(r.stdout).ok()?;
            let mut words = text.split_whitespace();
            Some((
                words.next()?.parse::<f64>().ok()?,
                words.next()?.parse::<u64>().ok()?,
            ))
        }) else {
            self.unavailable = true;
            return;
        };
        let cores = std::thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(1);
        if !cpu.is_finite() || cpu < 0.0 || rss == 0 {
            self.unavailable = true;
            return;
        }
        self.resources.push(json!({"measured_at_ms":self.start.elapsed().as_secs_f64()*1000.0,"cpu_total_percent":(cpu/cores as f64).min(100.0),"rss_bytes":rss*1024}));
    }
    pub fn observations(&mut self) -> Value {
        self.previous = self.start;
        self.sample();
        let os = Command::new("uname")
            .args(["-srm"])
            .output()
            .ok()
            .filter(|r| r.status.success())
            .map(|r| String::from_utf8_lossy(&r.stdout).trim().to_string());
        let hardware = Command::new("sysctl")
            .args(["-n", "hw.model"])
            .output()
            .ok()
            .filter(|r| r.status.success())
            .map(|r| String::from_utf8_lossy(&r.stdout).trim().to_string())
            .unwrap_or_else(|| std::env::consts::ARCH.into());
        json!({"clock":"monotonic_session_ms","metadata":{"machine":hardware,"source":"actual sequential production core inference runner","os":os,"profile":if cfg!(debug_assertions){"debug"}else{"release"},"provider":"cpu","notes":format!("ps %cpu / logical CPU count, RSS KiB*1024 sampled no more often than1second between native calls; estimates, not interval CPU or peakRSS; no capture/UI target latency measurement. resource_probe_unavailable={}",self.unavailable)},"latency":[],"resources":self.resources})
    }
}
