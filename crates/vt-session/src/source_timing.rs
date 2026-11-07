use serde::{Deserialize, Serialize};
use vt_capture::CaptureClock;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TimingProvenance {
    UptimeRaw,
    LatestAudioObservation,
    SavedPlaybackPosition,
    UntimedNote,
    #[default]
    Unavailable,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SourceTiming {
    pub synchronized: bool,
    pub provenance: TimingProvenance,
    pub elapsed_ms: u64,
    pub latest_audio_ms: Option<u64>,
    pub warning: Option<String>,
}
#[derive(Default)]
pub(crate) struct SourceTimeline {
    pub(crate) origin_ns: Option<u64>,
    latest_audio_ns: Option<u64>,
    frozen_ms: Option<u64>,
    invalid_anchor: bool,
}
impl SourceTimeline {
    pub(crate) fn anchor(
        &mut self,
        clock: CaptureClock,
        origin: u64,
        observed: u64,
        now: Option<u64>,
    ) {
        let valid = clock == CaptureClock::UptimeRaw
            && now.is_some_and(|now| {
                origin <= observed && origin <= now && observed <= now.saturating_add(1_000_000)
            });
        if !valid || self.origin_ns.is_some_and(|old| old != origin) {
            self.invalid_anchor = true;
            self.origin_ns = None;
        } else if !self.invalid_anchor {
            self.origin_ns = Some(origin);
        }
    }
    pub(crate) fn observe(&mut self, end_ns: u64) {
        self.latest_audio_ns = Some(self.latest_audio_ns.unwrap_or(0).max(end_ns));
    }
    pub(crate) fn snapshot(&self, now: Option<u64>) -> SourceTiming {
        let exact = self
            .origin_ns
            .zip(now)
            .and_then(|(origin, now)| now.checked_sub(origin))
            .filter(|_| !self.invalid_anchor);
        let synchronized = exact.is_some()
            || (self.frozen_ms.is_some() && self.origin_ns.is_some() && !self.invalid_anchor);
        let elapsed_ms = self
            .frozen_ms
            .unwrap_or_else(|| exact.unwrap_or(self.latest_audio_ns.unwrap_or(0)) / 1_000_000);
        SourceTiming {
            synchronized,
            provenance: if synchronized {
                TimingProvenance::UptimeRaw
            } else if self.latest_audio_ns.is_some() {
                TimingProvenance::LatestAudioObservation
            } else {
                TimingProvenance::Unavailable
            },
            elapsed_ms: elapsed_ms.max(self.latest_audio_ns.unwrap_or(0) / 1_000_000),
            latest_audio_ms: self.latest_audio_ns.map(|n| n / 1_000_000),
            warning: if synchronized {
                None
            } else {
                Some(if self.latest_audio_ns.is_some() {"Source-clock synchronization is unavailable. Timestamps use the latest observed audio position and may lag the current moment."} else {"Source-clock synchronization is unavailable. Notes require a source-clock anchor or an observed audio position."}.into())
            },
        }
    }
    pub(crate) fn finish(&mut self) {
        self.frozen_ms = Some(self.latest_audio_ns.unwrap_or(0) / 1_000_000);
    }
    pub(crate) fn note_position(&self, now: Option<u64>) -> Option<SourceTiming> {
        let value = self.snapshot(now);
        (value.synchronized || value.latest_audio_ms.is_some()).then_some(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shared_clock_excludes_app_startup_and_preserves_source_positions() {
        let mut clock = SourceTimeline::default();
        assert!(clock.note_position(None).is_none());
        clock.anchor(
            CaptureClock::UptimeRaw,
            10_000_000_000,
            10_005_000_000,
            Some(10_010_000_000),
        );
        clock.observe(100_000_000);
        let note = clock.note_position(Some(10_125_000_000)).unwrap();
        assert!(note.synchronized);
        assert_eq!(note.elapsed_ms, 125);
        assert_eq!(note.latest_audio_ms, Some(100));
        // Five seconds spent in app/helper launch never enter this source epoch.
        assert_ne!(note.elapsed_ms, 5125);
        clock.finish();
        assert!(clock.snapshot(None).synchronized);
        assert_eq!(clock.snapshot(Some(99_000_000_000)).elapsed_ms, 100);
    }
    #[test]
    fn legacy_unknown_and_changed_anchors_remain_explicitly_approximate() {
        let mut clock = SourceTimeline::default();
        clock.observe(300_000_000);
        let fallback = clock.note_position(None).unwrap();
        assert!(!fallback.synchronized);
        assert_eq!(
            fallback.provenance,
            TimingProvenance::LatestAudioObservation
        );
        assert!(fallback.warning.is_some());
        clock.anchor(CaptureClock::Unknown, 0, 0, Some(100));
        assert!(!clock.snapshot(Some(1_000_000_000)).synchronized);
        let mut changed = SourceTimeline::default();
        changed.anchor(CaptureClock::UptimeRaw, 100, 200, Some(300));
        changed.anchor(CaptureClock::UptimeRaw, 101, 200, Some(300));
        assert!(!changed.snapshot(Some(300)).synchronized);
    }
}
