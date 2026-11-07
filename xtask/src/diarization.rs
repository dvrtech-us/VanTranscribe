use crate::{EvalResult, fail};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

/// Exactly pyannote.metrics(collar=0.25): total width, +/-125ms.
pub const COLLAR_SECONDS: f64 = 0.25;
const MAX_SPEAKERS: usize = 256;
fn quantize(time: f64) -> f64 {
    (time * 1_000_000_000.0).round() / 1_000_000_000.0
}
#[derive(Clone, Debug)]
pub struct Turn {
    pub speaker: String,
    pub start: f64,
    pub end: f64,
}
#[derive(Clone, Copy, Debug)]
pub struct Span {
    pub start: f64,
    pub end: f64,
}
#[derive(Clone, Debug, Default, Serialize)]
pub struct DiarizationScore {
    pub reference_speaker_seconds: f64,
    pub hypothesis_speaker_seconds: f64,
    pub missed_speaker_seconds: f64,
    pub false_alarm_speaker_seconds: f64,
    pub confused_speaker_seconds: f64,
    pub scored_uem_seconds: f64,
    pub reference_speakers: u64,
    pub hypothesis_speakers: u64,
    pub jer_speaker_error_sum: f64,
    pub der: f64,
    pub jer: f64,
    pub mapping: BTreeMap<String, String>,
}
impl DiarizationScore {
    pub fn finish(&mut self) {
        let errors = self.missed_speaker_seconds
            + self.false_alarm_speaker_seconds
            + self.confused_speaker_seconds;
        self.der = if self.reference_speaker_seconds > 0.0 {
            errors / self.reference_speaker_seconds
        } else if errors > 0.0 {
            1.0
        } else {
            0.0
        };
        // The pyannote JER convention is1 when no reference speaker survives.
        self.jer = if self.reference_speakers > 0 {
            self.jer_speaker_error_sum / self.reference_speakers as f64
        } else {
            1.0
        };
    }
    pub fn accumulate(&mut self, other: &Self) {
        self.reference_speaker_seconds += other.reference_speaker_seconds;
        self.hypothesis_speaker_seconds += other.hypothesis_speaker_seconds;
        self.missed_speaker_seconds += other.missed_speaker_seconds;
        self.false_alarm_speaker_seconds += other.false_alarm_speaker_seconds;
        self.confused_speaker_seconds += other.confused_speaker_seconds;
        self.scored_uem_seconds += other.scored_uem_seconds;
        self.reference_speakers += other.reference_speakers;
        self.hypothesis_speakers += other.hypothesis_speakers;
        self.jer_speaker_error_sum += other.jer_speaker_error_sum;
        self.finish();
    }
}
fn number(value: &str, context: &str) -> EvalResult<f64> {
    let n = value
        .parse::<f64>()
        .map_err(|_| fail(format!("invalid time in {context}")))?;
    if !n.is_finite() || !(0.0..=1_000_000_000.0).contains(&n) {
        return Err(fail(format!(
            "nonfinite, negative or excessive time in {context}"
        )));
    }
    Ok(quantize(n))
}
pub fn parse_rttm(text: &str, recording: &str, channel: &str) -> EvalResult<Vec<Turn>> {
    parse_rttm_inner(text, recording, channel, false)
}
pub fn parse_hypothesis_rttm(text: &str, recording: &str, channel: &str) -> EvalResult<Vec<Turn>> {
    parse_rttm_inner(text, recording, channel, true)
}
fn parse_rttm_inner(
    text: &str,
    recording: &str,
    channel: &str,
    allow_missing_hypothesis: bool,
) -> EvalResult<Vec<Turn>> {
    let mut turns = vec![];
    let mut records = 0;
    for (index, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.len() != 10 || fields[0] != "SPEAKER" {
            return Err(fail(format!(
                "RTTM line{} must be a10-column SPEAKER record",
                index + 1
            )));
        }
        records += 1;
        if records > 200_000 {
            return Err(fail("RTTM exceeds200000 records; split evaluation units"));
        }
        let start = number(fields[3], "RTTM onset")?;
        let duration = number(fields[4], "RTTM duration")?;
        let end = quantize(start + duration);
        if duration <= 0.0
            || end <= start
            || !end.is_finite()
            || end > 1_000_000_000.0
            || fields[7] == "<NA>"
        {
            return Err(fail(format!(
                "invalid RTTM interval/identity on line{}",
                index + 1
            )));
        }
        if fields[1] == recording && fields[2] == channel {
            turns.push(Turn {
                speaker: fields[7].into(),
                start,
                end,
            });
        }
    }
    if records > 0 && turns.is_empty() && !allow_missing_hypothesis {
        return Err(fail(format!(
            "RTTM has no records for recording {recording}, channel {channel}; use an explicitly empty file for known empty speech"
        )));
    }
    let mut by_speaker = BTreeMap::<&str, Vec<&Turn>>::new();
    for turn in &turns {
        by_speaker.entry(&turn.speaker).or_default().push(turn);
    }
    for (speaker, mut rows) in by_speaker {
        rows.sort_by(|a, b| a.start.total_cmp(&b.start));
        if rows.windows(2).any(|pair| pair[1].start < pair[0].end) {
            return Err(fail(format!(
                "overlapping/duplicate RTTM intervals for one identity {speaker}"
            )));
        }
    }
    Ok(turns)
}
pub fn parse_uem(text: &str, recording: &str, channel: &str) -> EvalResult<Vec<Span>> {
    let mut spans = vec![];
    for (index, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.len() != 4 {
            return Err(fail(format!(
                "UEM line{} must contain recording channel start end",
                index + 1
            )));
        }
        let start = number(fields[2], "UEM start")?;
        let end = number(fields[3], "UEM end")?;
        if start >= end {
            return Err(fail("UEM requires start<end"));
        }
        if fields[0] == recording && fields[1] == channel {
            spans.push(Span { start, end });
        }
    }
    if spans.is_empty() {
        return Err(fail(format!(
            "UEM is missing recording {recording}, channel {channel}"
        )));
    }
    spans.sort_by(|a, b| a.start.total_cmp(&b.start));
    let mut merged: Vec<Span> = vec![];
    for span in spans {
        if let Some(last) = merged.last_mut()
            && span.start <= last.end
        {
            last.end = last.end.max(span.end);
        } else {
            merged.push(span);
        }
    }
    Ok(merged)
}
#[derive(Clone, Copy)]
enum Kind {
    Reference(usize),
    Hypothesis(usize),
    Uem,
    Collar,
}
struct Event {
    time: f64,
    kind: Kind,
    start: bool,
}
/// Maximum overlap assignment, independently scoped to one recording/channel.
/// Zero-weight and dummy assignments are left unmapped, as in pyannote.
pub fn maximum_assignment(weights: &[Vec<f64>]) -> Vec<Option<usize>> {
    let rows = weights.len();
    let columns = weights.first().map_or(0, Vec::len);
    if rows == 0 || columns == 0 {
        return vec![None; rows];
    }
    let n = rows.max(columns);
    let max = weights.iter().flatten().copied().fold(0.0, f64::max);
    let mut u = vec![0.0; n + 1];
    let mut v = vec![0.0; n + 1];
    let mut p = vec![0_usize; n + 1];
    let mut way = vec![0_usize; n + 1];
    for row in 1..=n {
        p[0] = row;
        let mut column = 0;
        let mut minimum = vec![f64::INFINITY; n + 1];
        let mut used = vec![false; n + 1];
        loop {
            used[column] = true;
            let active = p[column];
            let mut delta = f64::INFINITY;
            let mut next = 0;
            for j in 1..=n {
                if used[j] {
                    continue;
                }
                let weight = if active <= rows && j <= columns {
                    weights[active - 1][j - 1]
                } else {
                    0.0
                };
                let cost = max - weight - u[active] - v[j];
                if cost < minimum[j] {
                    minimum[j] = cost;
                    way[j] = column;
                }
                if minimum[j] < delta {
                    delta = minimum[j];
                    next = j;
                }
            }
            for j in 0..=n {
                if used[j] {
                    u[p[j]] += delta;
                    v[j] -= delta;
                } else {
                    minimum[j] -= delta;
                }
            }
            column = next;
            if p[column] == 0 {
                break;
            }
        }
        loop {
            let previous = way[column];
            p[column] = p[previous];
            column = previous;
            if column == 0 {
                break;
            }
        }
    }
    let mut answer = vec![None; rows];
    for j in 1..=n {
        let row = p[j];
        if row > 0 && row <= rows && j <= columns && weights[row - 1][j - 1] > 0.0 {
            answer[row - 1] = Some(j - 1)
        }
    }
    answer
}
pub fn score(
    reference: &[Turn],
    hypothesis: &[Turn],
    uem: &[Span],
) -> EvalResult<DiarizationScore> {
    // Decimal RTTM onsets/durations often meet at0.1+0.2 versus0.3. Round
    // event geometry to1ns before sweep/set operations, preventing tiny binary
    // roundoff overlaps from dropping an adjacent interval of the same speaker.
    let canonical = |turns: &[Turn]| -> EvalResult<Vec<Turn>> {
        turns
            .iter()
            .map(|turn| {
                if !turn.start.is_finite()
                    || !turn.end.is_finite()
                    || turn.start < 0.0
                    || turn.end <= turn.start
                {
                    return Err(fail("invalid diarization interval"));
                }
                Ok(Turn {
                    speaker: turn.speaker.clone(),
                    start: quantize(turn.start),
                    end: quantize(turn.end),
                })
            })
            .collect()
    };
    let reference = canonical(reference)?;
    let hypothesis = canonical(hypothesis)?;
    let reference = reference.as_slice();
    let hypothesis = hypothesis.as_slice();
    for turns in [reference, hypothesis] {
        let mut speakers = BTreeMap::<&str, Vec<&Turn>>::new();
        for turn in turns {
            if turn.speaker.trim().is_empty()
                || !turn.start.is_finite()
                || !turn.end.is_finite()
                || turn.start < 0.0
                || turn.start >= turn.end
                || turn.end > 1_000_000_000.0
            {
                return Err(fail("invalid diarization interval/identity"));
            }
            speakers.entry(&turn.speaker).or_default().push(turn);
        }
        for mut intervals in speakers.into_values() {
            intervals.sort_by(|a, b| a.start.total_cmp(&b.start));
            if intervals.windows(2).any(|pair| pair[1].start < pair[0].end) {
                return Err(fail(
                    "one speaker has overlapping/duplicate annotation intervals",
                ));
            }
        }
    }
    if uem.iter().any(|span| {
        !span.start.is_finite()
            || !span.end.is_finite()
            || span.start < 0.0
            || span.start >= span.end
            || span.end > 1_000_000_000.0
    }) {
        return Err(fail("invalid UEM interval"));
    }
    if uem.is_empty() {
        return Err(fail("an explicit nonempty UEM is required"));
    }
    let ref_labels: Vec<_> = reference
        .iter()
        .map(|turn| turn.speaker.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let hyp_labels: Vec<_> = hypothesis
        .iter()
        .map(|turn| turn.speaker.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let ri: BTreeMap<_, _> = ref_labels
        .iter()
        .enumerate()
        .map(|(i, label)| (label.as_str(), i))
        .collect();
    let hi: BTreeMap<_, _> = hyp_labels
        .iter()
        .enumerate()
        .map(|(i, label)| (label.as_str(), i))
        .collect();
    let mut events = vec![];
    let mut add = |start, end, kind| {
        events.push(Event {
            time: quantize(start),
            kind,
            start: true,
        });
        events.push(Event {
            time: quantize(end),
            kind,
            start: false,
        });
    };
    for turn in reference {
        add(
            turn.start,
            turn.end,
            Kind::Reference(ri[turn.speaker.as_str()]),
        );
        for boundary in [turn.start, turn.end] {
            add(
                boundary - COLLAR_SECONDS / 2.0,
                boundary + COLLAR_SECONDS / 2.0,
                Kind::Collar,
            );
        }
    }
    for turn in hypothesis {
        add(
            turn.start,
            turn.end,
            Kind::Hypothesis(hi[turn.speaker.as_str()]),
        );
    }
    for span in uem {
        add(span.start, span.end, Kind::Uem);
    }
    events.sort_by(|a, b| a.time.total_cmp(&b.time).then(a.start.cmp(&b.start)));
    let mut r = BTreeSet::new();
    let mut h = BTreeSet::new();
    let mut inside = 0_i64;
    let mut collars = 0_i64;
    let mut rd = vec![0.0; ref_labels.len()];
    let mut hd = vec![0.0; hyp_labels.len()];
    let mut cooccur = BTreeMap::<(usize, usize), f64>::new();
    let mut evaluated_r = BTreeSet::new();
    let mut evaluated_h = BTreeSet::new();
    let mut score = DiarizationScore::default();
    let mut paired_seconds = 0.0;
    let mut operations = 0_usize;
    let mut index = 0;
    while index < events.len() {
        let at = events[index].time;
        while index < events.len() && events[index].time == at {
            let e = &events[index];
            match e.kind {
                Kind::Reference(id) => {
                    if e.start {
                        r.insert(id);
                    } else {
                        r.remove(&id);
                    }
                }
                Kind::Hypothesis(id) => {
                    if e.start {
                        h.insert(id);
                    } else {
                        h.remove(&id);
                    }
                }
                Kind::Uem => inside += if e.start { 1 } else { -1 },
                Kind::Collar => collars += if e.start { 1 } else { -1 },
            }
            index += 1;
        }
        if index == events.len() {
            break;
        }
        let duration = events[index].time - at;
        if inside <= 0 || collars > 0 || duration <= 0.0 {
            continue;
        }
        score.scored_uem_seconds += duration;
        for &id in &r {
            rd[id] += duration;
            evaluated_r.insert(id);
        }
        for &id in &h {
            hd[id] += duration;
            evaluated_h.insert(id);
        }
        if evaluated_r.len() > MAX_SPEAKERS || evaluated_h.len() > MAX_SPEAKERS {
            return Err(fail(
                "scored gallery exceeds256 speakers per recording/channel",
            ));
        }
        operations = operations.saturating_add(r.len().saturating_mul(h.len()));
        if operations > 20_000_000 {
            return Err(fail("excessively dense overlap; split the evaluation unit"));
        }
        for &a in &r {
            for &b in &h {
                *cooccur.entry((a, b)).or_default() += duration;
            }
        }
        paired_seconds += r.len().min(h.len()) as f64 * duration;
        score.missed_speaker_seconds += r.len().saturating_sub(h.len()) as f64 * duration;
        score.false_alarm_speaker_seconds += h.len().saturating_sub(r.len()) as f64 * duration;
    }
    let rids: Vec<_> = evaluated_r.into_iter().collect();
    let hids: Vec<_> = evaluated_h.into_iter().collect();
    let weights: Vec<Vec<_>> = rids
        .iter()
        .map(|a| {
            hids.iter()
                .map(|b| cooccur.get(&(*a, *b)).copied().unwrap_or(0.0))
                .collect()
        })
        .collect();
    let assignment = maximum_assignment(&weights);
    let mut correct = 0.0;
    for (i, &a) in rids.iter().enumerate() {
        if let Some(j) = assignment[i] {
            let b = hids[j];
            let intersection = weights[i][j];
            correct += intersection;
            score.jer_speaker_error_sum +=
                (rd[a] + hd[b] - 2.0 * intersection) / (rd[a] + hd[b] - intersection);
            score
                .mapping
                .insert(ref_labels[a].clone(), hyp_labels[b].clone());
        } else {
            score.jer_speaker_error_sum += 1.0;
        }
    }
    score.reference_speaker_seconds = rd.iter().sum();
    score.hypothesis_speaker_seconds = hd.iter().sum();
    score.reference_speakers = rids.len() as u64;
    score.hypothesis_speakers = hids.len() as u64;
    score.confused_speaker_seconds = (paired_seconds - correct).max(0.0);
    score.finish();
    Ok(score)
}
#[cfg(test)]
mod tests {
    use super::*;
    fn t(speaker: &str, start: f64, end: f64) -> Turn {
        Turn {
            speaker: speaker.into(),
            start,
            end,
        }
    }
    fn u(end: f64) -> Vec<Span> {
        vec![Span { start: 0.0, end }]
    }
    fn near(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-9, "{a} != {b}")
    }
    #[test]
    fn speaker_permutation_overlap_and_half_width_collar_have_known_counts() {
        let result = score(
            &[t("A", 0.0, 4.0), t("B", 2.0, 6.0)],
            &[t("X", 0.0, 6.0)],
            &u(6.0),
        )
        .unwrap();
        // Reference collars remove [0,.125],[1.875,2.125],
        // [3.875,4.125],[5.875,6]. Remaining overlap is1.75s.
        near(result.reference_speaker_seconds, 7.0);
        near(result.missed_speaker_seconds, 1.75);
        near(result.der, 0.5);
        near(result.jer, 2.0 / 3.0);
        let perfect = score(
            &[t("A", 0.0, 4.0), t("B", 2.0, 6.0)],
            &[t("Y", 0.0, 4.0), t("X", 2.0, 6.0)],
            &u(6.0),
        )
        .unwrap();
        near(perfect.der, 0.0);
        near(perfect.jer, 0.0);
        assert_eq!(perfect.mapping["A"], "Y");
        assert_eq!(perfect.mapping["B"], "X");
    }
    #[test]
    fn misses_false_alarms_confusion_and_outside_uem_are_scored_separately() {
        let result = score(
            &[t("A", 0.0, 4.0), t("B", 4.0, 8.0)],
            &[
                t("X", 0.0, 2.0),
                t("X", 4.0, 8.0),
                t("Z", 8.0, 10.0),
                t("OUT", 50.0, 60.0),
            ],
            &u(10.0),
        )
        .unwrap();
        near(result.reference_speaker_seconds, 7.5);
        near(result.missed_speaker_seconds, 1.875);
        near(result.false_alarm_speaker_seconds, 1.875);
        near(result.confused_speaker_seconds, 1.875);
        near(result.der, 0.75);
        assert!(!result.mapping.values().any(|label| label == "OUT"));
        near(result.jer, (1.0 + 1.875 / 5.625) / 2.0);
    }
    #[test]
    fn empty_speech_and_no_scored_reference_follow_explicit_pyannote_edges() {
        let empty = score(&[], &[], &u(2.0)).unwrap();
        near(empty.der, 0.0);
        near(empty.jer, 1.0);
        let false_alarm = score(&[], &[t("X", 0.0, 2.0)], &u(2.0)).unwrap();
        near(false_alarm.der, 1.0);
        near(false_alarm.false_alarm_speaker_seconds, 2.0);
        near(false_alarm.jer, 1.0);
        let missed = score(&[t("A", 0.0, 2.0)], &[], &u(2.0)).unwrap();
        near(missed.der, 1.0);
        near(missed.jer, 1.0);
    }
    #[test]
    fn collar_omits_timing_errors_inside_125ms_and_keeps_outside_error() {
        let inside = score(&[t("A", 1.0, 3.0)], &[t("X", 1.1, 2.9)], &u(4.0)).unwrap();
        near(inside.der, 0.0);
        let outside = score(&[t("A", 1.0, 3.0)], &[t("X", 1.25, 2.75)], &u(4.0)).unwrap();
        near(outside.reference_speaker_seconds, 1.75);
        near(outside.missed_speaker_seconds, 0.25);
        near(outside.der, 1.0 / 7.0);
    }
    #[test]
    fn global_assignment_is_not_greedy_or_per_segment() {
        assert_eq!(
            maximum_assignment(&[vec![9.0, 8.0], vec![8.0, 0.0]]),
            [Some(1), Some(0)]
        );
        assert_eq!(maximum_assignment(&[vec![0.0], vec![1.0]]), [None, Some(0)]);
        let r = score(
            &[t("A", 0.0, 2.0), t("B", 2.0, 4.0)],
            &[t("X", 0.0, 4.0)],
            &u(4.0),
        )
        .unwrap();
        near(r.confused_speaker_seconds, 1.75);
        near(r.der, 0.5);
    }
    #[test]
    fn malformed_or_missing_reference_ids_and_self_overlap_fail() {
        assert!(parse_rttm("SPEAKER m 1 NaN 1 <NA> <NA> a <NA> <NA>", "m", "1").is_err());
        assert!(parse_rttm("SPEAKER other 1 0 1 <NA> <NA> a <NA> <NA>", "m", "1").is_err());
        assert!(
            parse_rttm(
                "SPEAKER m 1 0 2 <NA> <NA> a <NA> <NA>\nSPEAKER m 1 1 2 <NA> <NA> a <NA> <NA>",
                "m",
                "1"
            )
            .is_err()
        );
        assert!(parse_uem("m 1 2 1", "m", "1").is_err());
        assert!(parse_uem("other 1 0 1", "m", "1").is_err());
    }
    #[test]
    fn invalid_direct_intervals_fail_and_reverse_order_adjacent_segments_remain_active() {
        assert!(score(&[t("A", 2.0, 1.0)], &[], &u(4.0)).is_err());
        let reference = [t("A", 2.0, 4.0), t("A", 0.0, 2.0)];
        let perfect = score(&reference, &[t("X", 0.0, 4.0)], &u(4.0)).unwrap();
        near(perfect.der, 0.0);
        near(perfect.jer, 0.0);
    }
    #[test]
    fn decimal_rttm_adjacency_does_not_create_binary_roundoff_gaps_or_overlaps() {
        let reference = parse_rttm(
            "SPEAKER m 1 0.1 0.2 <NA> <NA> A <NA> <NA>\nSPEAKER m 1 0.3 0.7 <NA> <NA> A <NA> <NA>",
            "m",
            "1",
        )
        .unwrap();
        let measured = score(&reference, &[t("X", 0.1, 1.0)], &u(2.0)).unwrap();
        near(measured.der, 0.0);
        near(measured.jer, 0.0);
    }
    #[test]
    fn absent_hypothesis_recording_is_a_miss_but_absent_reference_is_an_error() {
        let other = "SPEAKER other 1 0 1 <NA> <NA> X <NA> <NA>";
        assert!(parse_rttm(other, "m", "1").is_err());
        let hypothesis = parse_hypothesis_rttm(other, "m", "1").unwrap();
        near(
            score(&[t("A", 0.0, 2.0)], &hypothesis, &u(2.0))
                .unwrap()
                .der,
            1.0,
        );
    }
}
