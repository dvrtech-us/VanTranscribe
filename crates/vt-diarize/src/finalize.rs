//! Constraint failures produce explicit reviews and conservative unknown regions,
//! while native/model/capacity failures remain hard errors.
use crate::{
    cluster::{Clusters, ConstraintGraph},
    *,
};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
pub(crate) struct Record {
    pub turn: String,
    pub embedding: Vec<f32>,
    pub duration: u64,
}
struct Candidate {
    pair: PairConstraint,
    anchors: Vec<Anchor>,
    prior_only: bool,
}
struct Plan {
    valid: Vec<Candidate>,
    reviews: Vec<ConstraintReview>,
    masked: BTreeSet<String>,
}
fn intersects(turn: &SpeakerTurn, anchor: &Anchor) -> bool {
    turn.track == anchor.track && turn.start_ms < anchor.end_ms && anchor.start_ms < turn.end_ms
}
fn anchor(turn: &SpeakerTurn) -> Anchor {
    Anchor {
        track: turn.track.clone(),
        start_ms: turn.start_ms,
        end_ms: turn.end_ms,
    }
}
fn track_bounds(turns: &[SpeakerTurn], track: Option<&str>) -> Vec<Anchor> {
    let mut ranges = BTreeMap::<String, (u64, u64)>::new();
    for t in turns
        .iter()
        .filter(|t| track.is_none_or(|track| t.track == track))
    {
        let range = ranges
            .entry(t.track.clone())
            .or_insert((t.start_ms, t.end_ms));
        range.0 = range.0.min(t.start_ms);
        range.1 = range.1.max(t.end_ms);
    }
    ranges
        .into_iter()
        .map(|(track, (start_ms, end_ms))| Anchor {
            track,
            start_ms,
            end_ms,
        })
        .collect()
}
impl Plan {
    fn review(
        &mut self,
        turns: &[SpeakerTurn],
        provenance: String,
        reason: &str,
        mut anchors: Vec<Anchor>,
    ) {
        if anchors
            .iter()
            .any(|a| !matches!(a.track.as_str(), "mic" | "system") || a.end_ms <= a.start_ms)
        {
            anchors
                .retain(|a| matches!(a.track.as_str(), "mic" | "system") && a.end_ms > a.start_ms);
            anchors.extend(track_bounds(turns, None));
        }
        for turn in turns {
            if anchors.iter().any(|a| intersects(turn, a)) {
                self.masked.insert(turn.id.clone());
            }
        }
        self.reviews.push(ConstraintReview {
            provenance,
            reason: reason.into(),
            anchors,
        });
    }
    fn cascade(&mut self, turns: &[SpeakerTurn]) {
        const DEPENDENT_NOTE: &str = " Other corrections sharing these excerpts are temporarily deferred, not marked invalid. Fix the original correction identified by this review, then refine again.";
        loop {
            let removed = self
                .valid
                .iter()
                .enumerate()
                .filter(|(_, c)| {
                    self.masked.contains(&c.pair.left_turn_id)
                        || self.masked.contains(&c.pair.right_turn_id)
                })
                .map(|(i, _)| i)
                .collect::<Vec<_>>();
            if removed.is_empty() {
                break;
            }
            for i in removed.into_iter().rev() {
                let c = self.valid.remove(i);
                let endpoints = turns
                    .iter()
                    .filter(|t| t.id == c.pair.left_turn_id || t.id == c.pair.right_turn_id)
                    .collect::<Vec<_>>();
                let mut sources = self
                    .reviews
                    .iter()
                    .enumerate()
                    .filter(|(_, review)| {
                        endpoints
                            .iter()
                            .any(|t| review.anchors.iter().any(|a| intersects(t, a)))
                    })
                    .map(|(i, _)| i)
                    .collect::<Vec<_>>();
                // Every mask is rooted in a review. Retain that root source so
                // undo never suggests deleting a valid dependent correction.
                if sources.is_empty() && !self.reviews.is_empty() {
                    sources.push(0);
                }
                for source in sources {
                    let review = &mut self.reviews[source];
                    if !review.reason.contains(DEPENDENT_NOTE) {
                        review.reason.push_str(DEPENDENT_NOTE);
                    }
                    for anchor in &c.anchors {
                        if !review.anchors.iter().any(|a| {
                            a.track == anchor.track
                                && a.start_ms == anchor.start_ms
                                && a.end_ms == anchor.end_ms
                        }) {
                            review.anchors.push(anchor.clone());
                        }
                    }
                }
                for turn in turns {
                    if c.anchors.iter().any(|a| intersects(turn, a)) {
                        self.masked.insert(turn.id.clone());
                    }
                }
            }
        }
    }
}
pub(crate) fn mic_cannot_link(config: &DiarizerConfig) -> bool {
    config.constraints.iter().any(|c| {
        c.kind == ConstraintKind::CannotLink
            && (c.left_turn_id.starts_with("mic:") || c.right_turn_id.starts_with("mic:"))
    }) || config.temporal_constraints.iter().any(|c| {
        c.kind == ConstraintKind::CannotLink && (c.left.track == "mic" || c.right.track == "mic")
    })
}
fn plan_constraints(
    turns: &[SpeakerTurn],
    records: &[Record],
    config: &DiarizerConfig,
    mic_clustered: bool,
) -> Plan {
    let mut plan = Plan {
        valid: Vec::new(),
        reviews: Vec::new(),
        masked: BTreeSet::new(),
    };
    let embedded = records
        .iter()
        .map(|r| r.turn.as_str())
        .collect::<BTreeSet<_>>();
    let mut pending = Vec::<(PairConstraint, Vec<Anchor>)>::new();
    for c in &config.constraints {
        let a = turns.iter().find(|t| t.id == c.left_turn_id);
        let b = turns.iter().find(|t| t.id == c.right_turn_id);
        if let (Some(a), Some(b)) = (a, b) {
            pending.push((c.clone(), vec![anchor(a), anchor(b)]));
        } else {
            let track = c
                .left_turn_id
                .split(':')
                .next()
                .filter(|t| matches!(*t, "mic" | "system"));
            plan.review(turns,c.provenance.clone(),"A saved correction refers to a turn that is unavailable in this run. Remove this correction, refine again, then choose new clean excerpts.",track_bounds(turns,track));
        }
    }
    for c in &config.temporal_constraints {
        let anchors = vec![c.left.clone(), c.right.clone()];
        if anchors
            .iter()
            .any(|a| turns.iter().any(|t| t.overlap && intersects(t, a)))
        {
            plan.review(turns,c.provenance.clone(),"A saved correction intersects overlapping voices. Remove this correction, refine again, then choose new clean excerpts.",anchors);
            continue;
        }
        match resolve_temporal_constraints(turns,std::slice::from_ref(c)) {
            Ok(pairs)=>pending.push((pairs.into_iter().next().unwrap(),anchors)),
            Err(_)=>plan.review(turns,c.provenance.clone(),"A saved correction could not be matched uniquely to enough speech in this run. Remove this correction, refine again, then choose new clean excerpts.",anchors),
        }
    }
    for (pair, anchors) in pending {
        let a = turns.iter().find(|t| t.id == pair.left_turn_id).unwrap();
        let b = turns.iter().find(|t| t.id == pair.right_turn_id).unwrap();
        if pair.provenance.trim().is_empty() || a.track != b.track || a.overlap || b.overlap {
            plan.review(
                turns,
                pair.provenance,
                "This speaker correction has invalid or overlapping endpoints and needs review.",
                anchors,
            );
            continue;
        }
        let prior_only = config.mode == MeetingMode::Remote && !mic_clustered && a.track == "mic";
        if prior_only && pair.kind == ConstraintKind::CannotLink {
            plan.review(turns,pair.provenance,"This microphone correction was added after audio used the Me channel prior. Refine again to compute separate voices.",anchors);
            continue;
        }
        if prior_only {
            if a.speaker_ids != ["mic:me"] || b.speaker_ids != ["mic:me"] {
                plan.review(turns,pair.provenance,"The microphone correction has insufficient clean speech. Remove this correction, refine again, then choose new clean excerpts.",anchors);
                continue;
            }
        } else if !embedded.contains(pair.left_turn_id.as_str())
            || !embedded.contains(pair.right_turn_id.as_str())
        {
            plan.review(turns,pair.provenance,"A speaker correction has insufficient clean embedding evidence. Remove this correction, refine again, then choose new clean excerpts.",anchors);
            continue;
        }
        plan.valid.push(Candidate {
            pair,
            anchors,
            prior_only,
        });
    }
    let pairs = plan
        .valid
        .iter()
        .filter(|c| !c.prior_only)
        .map(|c| c.pair.clone())
        .collect::<Vec<_>>();
    let graph = ConstraintGraph::new(&pairs);
    let invalid = plan
        .valid
        .iter()
        .enumerate()
        .filter(|(_, c)| {
            graph
                .roots
                .get(&c.pair.left_turn_id)
                .is_some_and(|root| graph.conflicts.contains(root))
                || graph
                    .roots
                    .get(&c.pair.right_turn_id)
                    .is_some_and(|root| graph.conflicts.contains(root))
        })
        .map(|(i, _)| i)
        .collect::<Vec<_>>();
    for i in invalid.into_iter().rev() {
        let c = plan.valid.remove(i);
        plan.review(turns,c.pair.provenance,"Saved same-person and different-person corrections conflict. Remove or revise one before assigning these voices.",c.anchors);
    }
    plan.cascade(turns);
    plan
}
enum ReplayFailure {
    Constraint(BTreeSet<String>),
    Hard(DiarizeError),
}
type ReplayResult = std::result::Result<Replayed, ReplayFailure>;
struct Replayed {
    clusters: [Clusters; 2],
    merges: Vec<SpeakerMerge>,
    scores: BTreeMap<String, f32>,
}
fn replay(records: &[Record], plan: &Plan, prior: Option<usize>) -> ReplayResult {
    let constraints = plan
        .valid
        .iter()
        .filter(|c| !c.prior_only)
        .map(|c| c.pair.clone())
        .collect::<Vec<_>>();
    let mut clusters = [
        Clusters::new("mic", &constraints),
        Clusters::new("system", &constraints),
    ];
    let mut scores = BTreeMap::new();
    for record in records.iter().filter(|r| !plan.masked.contains(&r.turn)) {
        let i = usize::from(!record.turn.starts_with("mic:"));
        let (_, score, _) = clusters[i]
            .assign(&record.turn, record.embedding.clone(), record.duration)
            .map_err(|error| match error {
                DiarizeError::Constraint => {
                    ReplayFailure::Constraint(BTreeSet::from([record.turn.clone()]))
                }
                other => ReplayFailure::Hard(other),
            })?;
        scores.insert(record.turn.clone(), score);
    }
    let mut merges = Vec::new();
    for (i, cluster) in clusters.iter_mut().enumerate() {
        let prefix = if i == 0 { "mic:" } else { "system:" };
        let seen = records
            .iter()
            .filter(|r| r.turn.starts_with(prefix) && !plan.masked.contains(&r.turn))
            .map(|r| r.turn.clone())
            .collect();
        if cluster.check_constraints(&seen).is_err() {
            return Err(ReplayFailure::Constraint(
                cluster.violated_constraint_turns(&seen),
            ));
        }
        merges.extend(cluster.refine(prior));
        if cluster.check_constraints(&seen).is_err() {
            return Err(ReplayFailure::Constraint(
                cluster.violated_constraint_turns(&seen),
            ));
        }
    }
    Ok(Replayed {
        clusters,
        merges,
        scores,
    })
}
fn implicated_component(plan: &Plan, seeds: &BTreeSet<String>) -> BTreeSet<String> {
    let mut graph = BTreeMap::<&str, Vec<&str>>::new();
    for candidate in &plan.valid {
        let c = &candidate.pair;
        graph
            .entry(&c.left_turn_id)
            .or_default()
            .push(&c.right_turn_id);
        graph
            .entry(&c.right_turn_id)
            .or_default()
            .push(&c.left_turn_id);
    }
    let mut component = seeds.clone();
    let mut queue = VecDeque::from_iter(seeds.iter().cloned());
    while let Some(next) = queue.pop_front() {
        if let Some(neighbors) = graph.get(next.as_str()) {
            for neighbor in neighbors {
                if component.insert((*neighbor).into()) {
                    queue.push_back((*neighbor).into());
                }
            }
        }
    }
    component
}
fn replay_with_isolation(
    records: &[Record],
    turns: &[SpeakerTurn],
    plan: &mut Plan,
    prior: Option<usize>,
    attempt: &mut impl FnMut(&[Record], &Plan, Option<usize>) -> ReplayResult,
) -> Result<Replayed> {
    // Each retry removes an implicated component, so the original number of
    // candidates plus one is a strict bound even with multiple failures.
    let bound = plan.valid.len() + 1;
    for _ in 0..bound {
        match attempt(records, plan, prior) {
            Ok(result) => return Ok(result),
            Err(ReplayFailure::Hard(error)) => return Err(error),
            Err(ReplayFailure::Constraint(seeds)) => {
                let component = implicated_component(plan, &seeds);
                let removed = plan
                    .valid
                    .iter()
                    .enumerate()
                    .filter(|(_, c)| {
                        component.contains(&c.pair.left_turn_id)
                            || component.contains(&c.pair.right_turn_id)
                    })
                    .map(|(i, _)| i)
                    .collect::<Vec<_>>();
                if removed.is_empty() {
                    return Err(DiarizeError::Engine);
                }
                for i in removed.into_iter().rev() {
                    let c = plan.valid.remove(i);
                    plan.review(turns,c.pair.provenance,"This connected group of corrections could not be safely reconciled. Remove this correction, refine again, then choose new clean excerpts.",c.anchors);
                }
                plan.cascade(turns);
            }
        }
    }
    Err(DiarizeError::Engine)
}
pub(crate) fn finish(
    turns: Vec<SpeakerTurn>,
    records: &[Record],
    config: &DiarizerConfig,
    mic_clustered: bool,
    overlap_links: &[(String, Vec<String>)],
) -> Result<DiarizationResult> {
    finish_using_replay(turns, records, config, mic_clustered, overlap_links, replay)
}
fn finish_using_replay(
    mut turns: Vec<SpeakerTurn>,
    records: &[Record],
    config: &DiarizerConfig,
    mic_clustered: bool,
    overlap_links: &[(String, Vec<String>)],
    mut attempt: impl FnMut(&[Record], &Plan, Option<usize>) -> ReplayResult,
) -> Result<DiarizationResult> {
    let mut plan = plan_constraints(&turns, records, config, mic_clustered);
    let Replayed {
        clusters,
        merges,
        scores,
    } = replay_with_isolation(
        records,
        &turns,
        &mut plan,
        config.live_speaker_prior,
        &mut attempt,
    )?;
    let mut speakers = clusters[0].speakers("mic");
    speakers.extend(clusters[1].speakers("system"));
    for turn in &mut turns {
        if plan.masked.contains(&turn.id) {
            mask(turn);
            continue;
        }
        let cluster = &clusters[usize::from(turn.track != "mic")];
        if let Some(id) = cluster.speaker_for(&turn.id) {
            let info = speakers.iter().find(|s| s.id == id).unwrap();
            turn.speaker_ids = vec![id.into()];
            turn.provisional = info.provisional;
            turn.uncertain = info.provisional;
            turn.label = if info.provisional {
                "Unknown".into()
            } else {
                info.label.clone()
            };
            turn.score = scores.get(&turn.id).copied();
            turn.provenance = if config.phase == Phase::Live {
                "live-cluster"
            } else {
                "refined-cluster"
            }
            .into();
        } else if !(config.mode == MeetingMode::Remote
            && !mic_clustered
            && turn.track == "mic"
            && turn.speaker_ids == ["mic:me"])
        {
            turn.speaker_ids.clear();
            turn.score = None;
            turn.uncertain = true;
            turn.label = if turn.overlap { "Overlap" } else { "Unknown" }.into();
        }
        if !turn.speaker_ids.is_empty()
            && plan
                .valid
                .iter()
                .any(|c| c.pair.left_turn_id == turn.id || c.pair.right_turn_id == turn.id)
        {
            turn.provenance = "user".into();
            turn.score = None;
        }
    }
    for (overlap, links) in overlap_links {
        let ids = links
            .iter()
            .filter_map(|id| {
                clusters[usize::from(!id.starts_with("mic:"))]
                    .speaker_for(id)
                    .map(str::to_owned)
            })
            .collect::<BTreeSet<_>>();
        if let Some(turn) = turns
            .iter_mut()
            .find(|t| t.id == *overlap && !plan.masked.contains(&t.id))
        {
            turn.speaker_ids = if ids.len() == links.len() {
                ids.into_iter().collect()
            } else {
                Vec::new()
            };
        }
    }
    let me_duration = turns
        .iter()
        .filter(|t| t.speaker_ids == ["mic:me"])
        .map(|t| t.end_ms - t.start_ms)
        .sum();
    if me_duration > 0 {
        speakers.push(Speaker {
            id: "mic:me".into(),
            track: "mic".into(),
            label: "Me".into(),
            speech_ms: me_duration,
            provisional: false,
        });
    }
    turns.sort_by_key(|t| (t.start_ms, t.track.clone()));
    let context_warning = match config.phase {
        Phase::Live => {
            "Canonical segmentation input is 10 s; 5 s source windows are zero-padded, with 2.5 s ownership stride. Offline agglomeration starts from bounded online seed clusters and cannot automatically split a mistaken seed without supplied constraints."
        }
        Phase::Refined => {
            "Canonical segmentation input is 10 s; refined windows use 10 s of real source context with 2.5 s ownership stride. Short final tails are zero-padded and clipped to real source. Offline agglomeration starts from bounded online seed clusters and cannot automatically split a mistaken seed without supplied constraints."
        }
    };
    let mut warnings=vec!["Experimental uncalibrated speaker clustering; labels are not verified identities. English/channel-specific DER, thresholds, overlap attribution and long-call acceptance remain unvalidated.".into(),context_warning.into()];
    if !plan.reviews.is_empty() {
        warnings.push(format!("{} saved speaker correction(s) need review. Affected voices remain unknown; transcript processing can continue.",plan.reviews.len()));
    }
    Ok(DiarizationResult {
        turns,
        speakers,
        merges,
        embedding_version: EMBEDDING_VERSION.into(),
        calibrated: false,
        warnings,
        constraint_reviews: plan.reviews,
        segmentation_context: None,
    })
}
fn mask(turn: &mut SpeakerTurn) {
    turn.speaker_ids.clear();
    turn.label = "Unknown".into();
    turn.uncertain = true;
    turn.provisional = false;
    turn.provenance = "constraint-review".into();
    turn.score = None;
}
/// Pending live constraints may not assign an identity before final replay.
pub(crate) fn mask_live(turn: &mut SpeakerTurn, config: &DiarizerConfig) {
    if config
        .constraints
        .iter()
        .any(|c| c.left_turn_id == turn.id || c.right_turn_id == turn.id)
        || config
            .temporal_constraints
            .iter()
            .any(|c| intersects(turn, &c.left) || intersects(turn, &c.right))
    {
        mask(turn);
    }
}

#[cfg(test)]
pub(crate) fn finish_with_component_failure_for_test(
    turns: Vec<SpeakerTurn>,
    records: &[Record],
    config: &DiarizerConfig,
    failed_turn: &str,
) -> Result<(DiarizationResult, usize)> {
    let mut attempts = 0;
    let result = finish_using_replay(turns, records, config, true, &[], |records, plan, prior| {
        attempts += 1;
        if plan
            .valid
            .iter()
            .any(|c| c.pair.left_turn_id == failed_turn || c.pair.right_turn_id == failed_turn)
        {
            Err(ReplayFailure::Constraint(BTreeSet::from([
                failed_turn.to_owned()
            ])))
        } else {
            replay(records, plan, prior)
        }
    });
    result.map(|result| (result, attempts))
}
