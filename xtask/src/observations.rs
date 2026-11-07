use crate::{EvalResult, fail};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BleedReference {
    pub segments: Vec<BleedSegment>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BleedSegment {
    pub id: String,
    pub track: String,
    pub start_ms: u64,
    pub end_ms: u64,
    pub kind: BleedKind,
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BleedKind {
    BleedDuplicate,
    GenuineShortReply,
    GenuineOther,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BleedHypothesis {
    pub annotations: Vec<BleedAnnotation>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BleedAnnotation {
    pub segment_id: String,
    pub hidden_as_bleed: bool,
}
#[derive(Clone, Debug, Default, Serialize)]
pub struct BleedScore {
    pub evaluated_lines: u64,
    pub annotated_as_bleed: u64,
    pub reference_duplicates: u64,
    pub visible_duplicates: u64,
    pub genuine_short_replies: u64,
    pub lost_genuine_short_replies: u64,
    pub false_hidden_genuine_lines: u64,
    pub visible_duplicate_rate: Option<f64>,
    pub annotation_rate: Option<f64>,
}
impl BleedScore {
    fn finish(&mut self) {
        self.visible_duplicate_rate = (self.evaluated_lines > 0)
            .then(|| self.visible_duplicates as f64 / self.evaluated_lines as f64);
        self.annotation_rate = (self.evaluated_lines > 0)
            .then(|| self.annotated_as_bleed as f64 / self.evaluated_lines as f64);
    }
    pub fn accumulate(&mut self, other: &Self) {
        self.evaluated_lines += other.evaluated_lines;
        self.annotated_as_bleed += other.annotated_as_bleed;
        self.reference_duplicates += other.reference_duplicates;
        self.visible_duplicates += other.visible_duplicates;
        self.genuine_short_replies += other.genuine_short_replies;
        self.lost_genuine_short_replies += other.lost_genuine_short_replies;
        self.false_hidden_genuine_lines += other.false_hidden_genuine_lines;
        self.finish();
    }
}
pub fn bleed(reference: BleedReference, hypothesis: BleedHypothesis) -> EvalResult<BleedScore> {
    let mut annotations = BTreeMap::new();
    for row in hypothesis.annotations {
        if row.segment_id.trim().is_empty()
            || annotations
                .insert(row.segment_id, row.hidden_as_bleed)
                .is_some()
        {
            return Err(fail("duplicate/empty bleed hypothesis segment ID"));
        }
    }
    let mut seen = BTreeSet::new();
    let mut score = BleedScore::default();
    for segment in reference.segments {
        if segment.id.trim().is_empty()
            || !seen.insert(segment.id.clone())
            || !matches!(segment.track.as_str(), "mic" | "system")
            || segment.start_ms >= segment.end_ms
        {
            return Err(fail(
                "invalid/duplicate bleed reference interval, track or ID",
            ));
        }
        let hidden = annotations.remove(&segment.id).ok_or_else(|| {
            fail(format!(
                "missing explicit bleed hypothesis for{}",
                segment.id
            ))
        })?;
        score.evaluated_lines += 1;
        score.annotated_as_bleed += u64::from(hidden);
        match segment.kind {
            BleedKind::BleedDuplicate => {
                score.reference_duplicates += 1;
                score.visible_duplicates += u64::from(!hidden);
            }
            BleedKind::GenuineShortReply => {
                score.genuine_short_replies += 1;
                score.lost_genuine_short_replies += u64::from(hidden);
                score.false_hidden_genuine_lines += u64::from(hidden);
            }
            BleedKind::GenuineOther => score.false_hidden_genuine_lines += u64::from(hidden),
        }
    }
    if !annotations.is_empty() {
        return Err(fail(
            "bleed hypotheses contain segments without ground truth",
        ));
    }
    score.finish();
    Ok(score)
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MeasurementMetadata {
    pub machine: String,
    pub source: String,
    pub os: Option<String>,
    pub profile: Option<String>,
    pub provider: Option<String>,
    pub notes: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Observations {
    pub clock: String,
    pub metadata: MeasurementMetadata,
    #[serde(default)]
    pub latency: Vec<LatencySample>,
    #[serde(default)]
    pub resources: Vec<ResourceSample>,
}
#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LatencyKind {
    Partial,
    Final,
    SpeakerLabel,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LatencySample {
    pub kind: LatencyKind,
    pub reference_time_ms: f64,
    pub emitted_time_ms: f64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceSample {
    pub measured_at_ms: f64,
    pub cpu_total_percent: Option<f64>,
    pub rss_bytes: Option<u64>,
}
#[derive(Clone, Debug, Serialize)]
pub struct Percentiles {
    pub count: usize,
    pub p50: f64,
    pub p95: f64,
    pub p99: f64,
    pub max: f64,
}
pub fn percentiles(samples: &[f64]) -> Option<Percentiles> {
    if samples.is_empty() {
        return None;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    let at = |q: f64| sorted[((q * sorted.len() as f64).ceil() as usize).saturating_sub(1)];
    Some(Percentiles {
        count: sorted.len(),
        p50: at(0.5),
        p95: at(0.95),
        p99: at(0.99),
        max: *sorted.last().unwrap(),
    })
}
#[derive(PartialEq)]
struct ResourceContext {
    machine: String,
    os: Option<String>,
    profile: Option<String>,
    provider: Option<String>,
}
#[derive(Default)]
pub struct Measurements {
    resource_context: Option<ResourceContext>,
    pub partial: Vec<f64>,
    pub final_text: Vec<f64>,
    pub speaker_label: Vec<f64>,
    pub cpu: Vec<f64>,
    pub rss: Vec<f64>,
    pub metadata: Vec<MeasurementMetadata>,
}
#[derive(Serialize)]
pub struct MeasurementReport {
    pub partial_ms: Option<Percentiles>,
    pub final_text_ms: Option<Percentiles>,
    pub speaker_label_ms: Option<Percentiles>,
    pub cpu_total_percent: Option<Percentiles>,
    pub cpu_sample_mean_percent: Option<f64>,
    pub rss_bytes: Option<Percentiles>,
    pub metadata: Vec<MeasurementMetadata>,
}
impl Measurements {
    pub fn add(&mut self, input: Observations) -> EvalResult<()> {
        if input.clock != "monotonic_session_ms"
            || input.metadata.machine.trim().is_empty()
            || input.metadata.source.trim().is_empty()
        {
            return Err(fail(
                "observations require monotonic_session_ms clock and machine/source metadata",
            ));
        }
        if input.latency.is_empty() && input.resources.is_empty() {
            return Err(fail("observation file contains no measurements"));
        }
        if !input.resources.is_empty() {
            let context = ResourceContext {
                machine: input.metadata.machine.clone(),
                os: input.metadata.os.clone(),
                profile: input.metadata.profile.clone(),
                provider: input.metadata.provider.clone(),
            };
            if self
                .resource_context
                .as_ref()
                .is_some_and(|previous| previous != &context)
            {
                return Err(fail(
                    "resource samples mix machines, OS, profiles or providers; use separate manifests",
                ));
            }
            self.resource_context = Some(context);
        }
        for sample in input.latency {
            if !sample.reference_time_ms.is_finite()
                || !sample.emitted_time_ms.is_finite()
                || sample.reference_time_ms < 0.0
                || sample.emitted_time_ms < sample.reference_time_ms
            {
                return Err(fail(
                    "latency requires finite ordered monotonic reference/emission timestamps",
                ));
            }
            let values = match sample.kind {
                LatencyKind::Partial => &mut self.partial,
                LatencyKind::Final => &mut self.final_text,
                LatencyKind::SpeakerLabel => &mut self.speaker_label,
            };
            values.push(sample.emitted_time_ms - sample.reference_time_ms);
        }
        for sample in input.resources {
            if !sample.measured_at_ms.is_finite()
                || sample.measured_at_ms < 0.0
                || sample.cpu_total_percent.is_none() && sample.rss_bytes.is_none()
            {
                return Err(fail("invalid or empty resource sample"));
            }
            if let Some(cpu) = sample.cpu_total_percent {
                if !cpu.is_finite() || !(0.0..=100.0).contains(&cpu) {
                    return Err(fail(
                        "CPU measurement must be percent of the total machine in0..100",
                    ));
                }
                self.cpu.push(cpu);
            }
            if let Some(rss) = sample.rss_bytes {
                if rss == 0 {
                    return Err(fail("omit unmeasured RSS rather than reporting zero"));
                }
                self.rss.push(rss as f64);
            }
        }
        self.metadata.push(input.metadata);
        Ok(())
    }
    pub fn report(self) -> MeasurementReport {
        MeasurementReport {
            partial_ms: percentiles(&self.partial),
            final_text_ms: percentiles(&self.final_text),
            speaker_label_ms: percentiles(&self.speaker_label),
            cpu_total_percent: percentiles(&self.cpu),
            cpu_sample_mean_percent: (!self.cpu.is_empty())
                .then(|| self.cpu.iter().sum::<f64>() / self.cpu.len() as f64),
            rss_bytes: percentiles(&self.rss),
            metadata: self.metadata,
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn manually_annotated_bleed_preserves_short_reply_denominators() {
        let reference=serde_json::from_str(r#"{"segments":[{"id":"dup","track":"mic","start_ms":0,"end_ms":1000,"kind":"bleed_duplicate"},{"id":"reply","track":"mic","start_ms":1000,"end_ms":1300,"kind":"genuine_short_reply"}]}"#).unwrap();
        let hypothesis=serde_json::from_str(r#"{"annotations":[{"segment_id":"dup","hidden_as_bleed":false},{"segment_id":"reply","hidden_as_bleed":true}]}"#).unwrap();
        let result = bleed(reference, hypothesis).unwrap();
        assert_eq!(result.visible_duplicate_rate, Some(0.5));
        assert_eq!(result.annotation_rate, Some(0.5));
        assert_eq!(result.lost_genuine_short_replies, 1);
    }
    #[test]
    fn nearest_rank_percentiles_have_a_fixed_small_sample_definition() {
        let p = percentiles(&[9.0, 1.0, 3.0, 5.0]).unwrap();
        assert_eq!(p.p50, 3.0);
        assert_eq!(p.p95, 9.0);
        assert_eq!(p.p99, 9.0);
        assert!(percentiles(&[]).is_none());
    }
    #[test]
    fn machine_resource_budgets_never_average_incomparable_hardware() {
        let mut measurements = Measurements::default();
        let sample = |machine: &str| {
            serde_json::from_value(serde_json::json!({"clock":"monotonic_session_ms","metadata":{"machine":machine,"source":"fictional"},"resources":[{"measured_at_ms":1,"cpu_total_percent":10.0,"rss_bytes":1000}]})).unwrap()
        };
        measurements.add(sample("machine-a")).unwrap();
        assert!(measurements.add(sample("machine-b")).is_err());
    }
}
