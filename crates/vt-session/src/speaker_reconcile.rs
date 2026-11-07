//! Conservative, meeting-local continuity between diarization passes. Overlap
//! preserves cluster labels only; it does not verify a person's human identity.
use std::collections::{BTreeMap, BTreeSet};
use vt_diarize::SpeakerTurn;

const MAX_IDENTITIES_PER_TRACK: usize = 128;

#[derive(Clone, Debug, Default, serde::Serialize)]
pub(super) struct ReconciliationPlan {
    /// New run-local cluster ID -> previous stable meeting-local identity ID.
    pub matches: BTreeMap<String, String>,
    pub reviews: Vec<ReconciliationReview>,
}
#[derive(Clone, Debug, serde::Serialize)]
pub(super) struct ReconciliationReview {
    pub kind: String,
    pub previous_ids: Vec<String>,
    pub current_ids: Vec<String>,
    pub reason: String,
}
#[derive(Default)]
struct Cluster {
    coverage: Vec<(u64, u64)>,
    reliable: Vec<(u64, u64)>,
}
impl Cluster {
    fn duration(&self) -> u64 {
        self.coverage.iter().map(|(a, b)| b - a).sum()
    }
}
type Tracks = BTreeMap<String, BTreeMap<String, Cluster>>;

/// Turns with uncertain/provisional/overlap flags contribute no identity evidence.
/// Their single-ID duration still counts in the denominator: a brief reliable
/// sliver cannot justify inheriting a name across a mostly uncertain cluster.
fn clusters(turns: &[SpeakerTurn]) -> Tracks {
    let mut tracks = Tracks::new();
    for turn in turns {
        if !matches!(turn.track.as_str(), "mic" | "system")
            || turn.end_ms <= turn.start_ms
            || turn.speaker_ids.len() != 1
            || turn.speaker_ids[0].is_empty()
        {
            continue;
        }
        let cluster = tracks
            .entry(turn.track.clone())
            .or_default()
            .entry(turn.speaker_ids[0].clone())
            .or_default();
        cluster.coverage.push((turn.start_ms, turn.end_ms));
        if !turn.overlap
            && !turn.uncertain
            && !turn.provisional
            && !["unknown", "unclear", "overlap"]
                .iter()
                .any(|label| turn.label.eq_ignore_ascii_case(label))
        {
            cluster.reliable.push((turn.start_ms, turn.end_ms));
        }
    }
    for track in tracks.values_mut() {
        for cluster in track.values_mut() {
            cluster.coverage = union(std::mem::take(&mut cluster.coverage));
            cluster.reliable = union(std::mem::take(&mut cluster.reliable));
        }
    }
    tracks
}
fn union(mut ranges: Vec<(u64, u64)>) -> Vec<(u64, u64)> {
    ranges.sort_unstable();
    let mut merged: Vec<(u64, u64)> = Vec::new();
    for (start, end) in ranges {
        if let Some(last) = merged.last_mut()
            && start <= last.1
        {
            last.1 = last.1.max(end);
        } else {
            merged.push((start, end));
        }
    }
    merged
}
fn overlap(a: &[(u64, u64)], b: &[(u64, u64)]) -> u64 {
    let (mut i, mut j, mut total) = (0, 0, 0);
    while i < a.len() && j < b.len() {
        total += a[i].1.min(b[j].1).saturating_sub(a[i].0.max(b[j].0));
        if a[i].1 <= b[j].1 {
            i += 1;
        } else {
            j += 1;
        }
    }
    total
}
fn covers(overlap: u64, duration: u64, percent: u32) -> bool {
    duration != 0 && u128::from(overlap) * 100 >= u128::from(duration) * u128::from(percent)
}
fn named<'a>(names: &'a BTreeMap<String, String>, id: &str) -> Option<&'a str> {
    names
        .get(id)
        .map(String::as_str)
        .filter(|name| !name.trim().is_empty())
}
fn review(
    kind: &str,
    previous_ids: Vec<String>,
    current_ids: Vec<String>,
    reason: &str,
) -> ReconciliationReview {
    ReconciliationReview {
        kind: kind.into(),
        previous_ids,
        current_ids,
        reason: reason.into(),
    }
}

pub(super) fn reconcile(
    previous: &[SpeakerTurn],
    current: &[SpeakerTurn],
    names: &BTreeMap<String, String>,
) -> ReconciliationPlan {
    let previous = clusters(previous);
    let current = clusters(current);
    let mut plan = ReconciliationPlan::default();
    let mut conflicting_ids = BTreeSet::new();
    for track in ["mic", "system"] {
        let (Some(old), Some(new)) = (previous.get(track), current.get(track)) else {
            continue;
        };
        let ids_old: Vec<_> = old.keys().cloned().collect();
        let ids_new: Vec<_> = new.keys().cloned().collect();
        if old.len() > MAX_IDENTITIES_PER_TRACK || new.len() > MAX_IDENTITIES_PER_TRACK {
            plan.reviews.push(review("capacity", ids_old, ids_new, "Too many cluster identities for bounded reconciliation; no labels were inherited on this track."));
            continue;
        }
        let duration_old: Vec<_> = old.values().map(Cluster::duration).collect();
        let duration_new: Vec<_> = new.values().map(Cluster::duration).collect();
        let matrix: Vec<Vec<_>> = old
            .values()
            .map(|a| {
                new.values()
                    .map(|b| overlap(&a.reliable, &b.reliable))
                    .collect()
            })
            .collect();
        let mut blocked_old = BTreeSet::new();
        let mut blocked_new = BTreeSet::new();
        let mut proposed = BTreeMap::<usize, usize>::new();

        // Non-bijective evidence is handled first, never hidden by a one-to-one solver.
        for (i, id) in ids_old.iter().enumerate() {
            let children: Vec<_> = ids_new
                .iter()
                .enumerate()
                .filter(|(j, _)| covers(matrix[i][*j], duration_old[i], 30))
                .map(|(j, _)| j)
                .collect();
            if children.len() < 2 {
                continue;
            }
            blocked_old.insert(i);
            blocked_new.extend(children.iter().copied());
            if named(names, id).is_some() {
                plan.reviews.push(review("split", vec![id.clone()], children.iter().map(|j| ids_new[*j].clone()).collect(), "A named previous cluster has at least two current clusters covering 30% or more of its duration. Review the split before inheriting its label."));
            }
            // An unnamed split also receives new IDs; it has no user name to resolve.
        }
        for (j, id) in ids_new.iter().enumerate() {
            let parents: Vec<_> = ids_old
                .iter()
                .enumerate()
                .filter(|(i, _)| covers(matrix[*i][j], duration_old[*i], 50))
                .map(|(i, _)| i)
                .collect();
            if parents.len() < 2 {
                continue;
            }
            let named_parents: Vec<_> = parents
                .iter()
                .copied()
                .filter(|i| named(names, &ids_old[*i]).is_some())
                .collect();
            let complex =
                blocked_new.contains(&j) || parents.iter().any(|i| blocked_old.contains(i));
            if named_parents.len() > 1 {
                plan.reviews.push(review("merge", parents.iter().map(|i| ids_old[*i].clone()).collect(), vec![id.clone()], "Multiple named previous cluster identities map mostly to this current cluster. Names alone do not prove those identities are the same; review before inheriting a label."));
            } else if complex && !named_parents.is_empty() {
                plan.reviews.push(review("ambiguous", parents.iter().map(|i| ids_old[*i].clone()).collect(), vec![id.clone()], "Split and merge evidence intersect for a named cluster. No label was inherited without review."));
            } else if !complex && named_parents.len() == 1 {
                proposed.insert(j, named_parents[0]);
            }
            blocked_old.extend(parents);
            blocked_new.insert(j);
        }

        let rows: Vec<_> = (0..ids_old.len())
            .filter(|i| !blocked_old.contains(i))
            .collect();
        let columns: Vec<_> = (0..ids_new.len())
            .filter(|j| !blocked_new.contains(j))
            .collect();
        let weights: Vec<Vec<_>> = rows
            .iter()
            .map(|i| columns.iter().map(|j| matrix[*i][*j]).collect())
            .collect();
        let assignments = hungarian(&weights);
        let best = assignment_score(&weights, &assignments);
        let mut ambiguous_old = BTreeSet::new();
        let mut ambiguous_new = BTreeSet::new();
        let mut ambiguity_reviews = BTreeSet::new();
        for (row, column) in assignments.iter().enumerate() {
            let Some(column) = *column else {
                continue;
            };
            let (i, j) = (rows[row], columns[column]);
            if !covers(matrix[i][j], duration_old[i].max(duration_new[j]), 50) {
                continue;
            }
            // Equal globally optimal alternatives are not a basis for stable identity.
            let mut without = weights.clone();
            without[row][column] = 0;
            let alternative = hungarian(&without);
            if assignment_score(&without, &alternative) == best {
                let mut previous_ids = BTreeSet::new();
                let mut current_ids = BTreeSet::new();
                for (r, (&assigned, &other)) in assignments.iter().zip(&alternative).enumerate() {
                    if assigned == other {
                        continue;
                    }
                    ambiguous_old.insert(rows[r]);
                    previous_ids.insert(ids_old[rows[r]].clone());
                    for col in [assigned, other].into_iter().flatten() {
                        ambiguous_new.insert(columns[col]);
                        current_ids.insert(ids_new[columns[col]].clone());
                    }
                }
                ambiguity_reviews.insert((
                    previous_ids.into_iter().collect::<Vec<_>>(),
                    current_ids.into_iter().collect::<Vec<_>>(),
                ));
            } else {
                proposed.insert(j, i);
            }
        }
        for (previous_ids, current_ids) in ambiguity_reviews {
            plan.reviews.push(review("ambiguous", previous_ids, current_ids, "More than one duration-overlap assignment has the same optimal weight. No tied label was inherited."));
        }
        for (j, i) in proposed {
            if ambiguous_old.contains(&i)
                || ambiguous_new.contains(&j)
                || conflicting_ids.contains(&ids_new[j])
            {
                continue;
            }
            if let Some(existing) = plan.matches.get(&ids_new[j])
                && existing != &ids_old[i]
            {
                let mut previous_ids = vec![existing.clone(), ids_old[i].clone()];
                previous_ids.sort();
                previous_ids.dedup();
                plan.matches.remove(&ids_new[j]);
                conflicting_ids.insert(ids_new[j].clone());
                plan.reviews.push(review("track_conflict", previous_ids, vec![ids_new[j].clone()], "A current cluster ID is reused across tracks with different prior identities. Cross-track evidence cannot choose a label."));
            } else {
                plan.matches.insert(ids_new[j].clone(), ids_old[i].clone());
            }
        }
    }
    plan
}

/// Rectangular maximum-weight assignment, padded with unmatched zero-weight columns.
/// Integer Hungarian potentials avoid float rounding near the percentage gates.
fn hungarian(weights: &[Vec<u64>]) -> Vec<Option<usize>> {
    let n = weights.len();
    let columns = weights.first().map_or(0, Vec::len);
    if n == 0 || columns == 0 {
        return vec![None; n];
    }
    let m = columns.max(n);
    let (mut u, mut v, mut p, mut way) = (
        vec![0i128; n + 1],
        vec![0i128; m + 1],
        vec![0usize; m + 1],
        vec![0usize; m + 1],
    );
    for i in 1..=n {
        p[0] = i;
        let mut j0 = 0;
        let mut min = vec![i128::MAX / 4; m + 1];
        let mut used = vec![false; m + 1];
        loop {
            used[j0] = true;
            let i0 = p[j0];
            let (mut delta, mut j1) = (i128::MAX / 4, 0);
            for j in 1..=m {
                if used[j] {
                    continue;
                }
                let weight = if j <= columns {
                    weights[i0 - 1][j - 1]
                } else {
                    0
                };
                let cost = -i128::from(weight) - u[i0] - v[j];
                if cost < min[j] {
                    min[j] = cost;
                    way[j] = j0;
                }
                if min[j] < delta {
                    delta = min[j];
                    j1 = j;
                }
            }
            for j in 0..=m {
                if used[j] {
                    u[p[j]] += delta;
                    v[j] -= delta;
                } else {
                    min[j] -= delta;
                }
            }
            j0 = j1;
            if p[j0] == 0 {
                break;
            }
        }
        loop {
            let j1 = way[j0];
            p[j0] = p[j1];
            j0 = j1;
            if j0 == 0 {
                break;
            }
        }
    }
    let mut result = vec![None; n];
    for j in 1..=columns {
        if p[j] > 0 && weights[p[j] - 1][j - 1] > 0 {
            result[p[j] - 1] = Some(j - 1);
        }
    }
    result
}
fn assignment_score(weights: &[Vec<u64>], assignments: &[Option<usize>]) -> u128 {
    assignments
        .iter()
        .enumerate()
        .filter_map(|(i, j)| j.map(|j| u128::from(weights[i][j])))
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn turn(id: &str, start: u64, end: u64) -> SpeakerTurn {
        SpeakerTurn {
            id: format!("{id}-{start}-{end}"),
            track: "system".into(),
            start_ms: start,
            end_ms: end,
            speaker_ids: vec![id.into()],
            label: "Speaker".into(),
            provenance: "refined-cluster".into(),
            score: None,
            overlap: false,
            uncertain: false,
            provisional: false,
        }
    }
    fn names(values: &[(&str, &str)]) -> BTreeMap<String, String> {
        values
            .iter()
            .map(|(id, name)| ((*id).into(), (*name).into()))
            .collect()
    }
    #[test]
    fn named_split_is_reviewed_unnamed_split_receives_new_ids_and_exact_threshold_is_inclusive() {
        let old = [turn("stable", 0, 1000)];
        let new = [turn("a", 0, 300), turn("b", 300, 1000)];
        let named = reconcile(&old, &new, &names(&[("stable", "Jane")]));
        assert!(named.matches.is_empty());
        assert_eq!(named.reviews.len(), 1);
        assert_eq!(named.reviews[0].kind, "split");
        assert_eq!(named.reviews[0].current_ids, ["a", "b"]);
        let unnamed = reconcile(&old, &new, &BTreeMap::new());
        assert!(unnamed.matches.is_empty());
        assert!(unnamed.reviews.is_empty());
    }
    #[test]
    fn merge_inherits_exactly_one_named_stable_id_and_rejects_conflicting_named_ids() {
        let old = [turn("old-a", 0, 500), turn("old-b", 500, 1000)];
        let new = [turn("new", 0, 1000)];
        let one = reconcile(&old, &new, &names(&[("old-b", "Jane")]));
        assert_eq!(one.matches.get("new").map(String::as_str), Some("old-b"));
        assert!(one.reviews.is_empty());
        for both in [
            names(&[("old-a", "Jane"), ("old-b", "John")]),
            names(&[("old-a", "Jane"), ("old-b", "Jane")]),
        ] {
            let conflict = reconcile(&old, &new, &both);
            assert!(conflict.matches.is_empty());
            assert_eq!(conflict.reviews[0].kind, "merge");
        }
        assert!(reconcile(&old, &new, &BTreeMap::new()).matches.is_empty());
    }
    #[test]
    fn one_to_one_requires_half_of_both_cluster_durations_and_never_crosses_tracks() {
        let old = [turn("old", 0, 1000)];
        let enough = reconcile(&old, &[turn("new", 500, 1500)], &BTreeMap::new());
        assert_eq!(enough.matches.get("new").map(String::as_str), Some("old"));
        assert!(
            reconcile(&old, &[turn("new", 0, 2001)], &BTreeMap::new())
                .matches
                .is_empty()
        );
        let mut other = turn("new", 0, 1000);
        other.track = "mic".into();
        assert!(
            reconcile(&old, &[other], &BTreeMap::new())
                .matches
                .is_empty()
        );
    }
    #[test]
    fn uncertain_overlap_provisional_unknown_and_duplicated_windows_do_not_inflate_evidence() {
        let old = [turn("old", 0, 1000), turn("old", 0, 1000)];
        let new = [turn("new", 0, 1000), turn("new", 0, 1000)];
        assert_eq!(reconcile(&old, &new, &BTreeMap::new()).matches.len(), 1);
        for mode in 0..4 {
            let mut unreliable = turn("new", 0, 1000);
            match mode {
                0 => unreliable.overlap = true,
                1 => unreliable.uncertain = true,
                2 => unreliable.provisional = true,
                _ => unreliable.label = "Unknown".into(),
            }
            assert!(
                reconcile(&old, &[unreliable], &BTreeMap::new())
                    .matches
                    .is_empty()
            );
        }
        let mut unclear = turn("new", 100, 1000);
        unclear.uncertain = true;
        assert!(
            reconcile(&old, &[turn("new", 0, 100), unclear], &BTreeMap::new())
                .matches
                .is_empty()
        );
    }
    #[test]
    fn intersecting_split_and_merge_never_steal_a_named_identity() {
        let old = [turn("split", 0, 1000), turn("named", 1000, 1500)];
        let new = [turn("a", 0, 400), turn("b", 400, 1500)];
        let plan = reconcile(&old, &new, &names(&[("named", "Jane")]));
        assert!(plan.matches.is_empty());
        assert!(plan.reviews.iter().any(|review| review.kind == "ambiguous"));
    }
    #[test]
    fn hungarian_maximizes_total_overlap_instead_of_greedy_row_choices_and_handles_rectangles() {
        let weights = vec![vec![10, 9], vec![9, 0]];
        let assignment = hungarian(&weights);
        assert_eq!(assignment, [Some(1), Some(0)]);
        assert_eq!(assignment_score(&weights, &assignment), 18);
        assert_eq!(
            hungarian(&[vec![5, 4], vec![4, 0], vec![0, 0]]),
            [Some(1), Some(0), None]
        );
        assert_eq!(hungarian(&[vec![1, 8, 3]]), [Some(1)]);
        assert!(hungarian(&[]).is_empty());
        assert_eq!(hungarian(&[vec![]]), [None]);
    }
    #[test]
    fn reused_current_ids_across_tracks_are_reviewed_when_stable_ids_disagree() {
        let mut mic_old = turn("mic-stable", 0, 1000);
        mic_old.track = "mic".into();
        let mut mic_new = turn("reused", 0, 1000);
        mic_new.track = "mic".into();
        let plan = reconcile(
            &[mic_old, turn("system-stable", 0, 1000)],
            &[mic_new, turn("reused", 0, 1000)],
            &BTreeMap::new(),
        );
        assert!(plan.matches.is_empty());
        assert_eq!(plan.reviews[0].kind, "track_conflict");
    }
    #[test]
    fn identity_capacity_is_bounded_and_reviewed_without_partial_inheritance() {
        let old = (0..129)
            .map(|i| turn(&format!("old-{i}"), i * 100, i * 100 + 100))
            .collect::<Vec<_>>();
        let plan = reconcile(&old, &[turn("new", 0, 100)], &BTreeMap::new());
        assert!(plan.matches.is_empty());
        assert_eq!(plan.reviews[0].kind, "capacity");
    }
}
