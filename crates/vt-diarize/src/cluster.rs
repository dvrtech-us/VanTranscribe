use crate::*;
use std::collections::{BTreeMap, BTreeSet};
const MAX_CLUSTERS: usize = 64;
/// Clusters freed by the duplicate-merge pass when the cap is reached, so the
/// pass does not rerun on every following open.
const CAP_HEADROOM: usize = 8;
/// Speech after which a cluster is an established (visible) speaker.
const ESTABLISHED_MS: u64 = 3000;
const MAX_EXEMPLARS: usize = 20;
use crate::diagnostics::TrackDiagnostics;
use crate::models::{ASSIGN, MARGIN, MERGE};
#[derive(Clone, Default)]
pub(crate) struct ConstraintGraph {
    pub roots: BTreeMap<String, String>,
    pub conflicts: BTreeSet<String>,
    edges: BTreeSet<(String, String)>,
}
impl ConstraintGraph {
    pub(crate) fn new(constraints: &[PairConstraint]) -> Self {
        fn find(parents: &mut BTreeMap<String, String>, id: &str) -> String {
            let mut next = id.to_string();
            let mut path = Vec::new();
            while parents.get(&next).is_some_and(|p| p != &next) {
                path.push(next.clone());
                next = parents[&next].clone();
            }
            for item in path {
                parents.insert(item, next.clone());
            }
            next
        }
        let mut parents = BTreeMap::new();
        for c in constraints {
            for id in [&c.left_turn_id, &c.right_turn_id] {
                parents.entry(id.clone()).or_insert_with(|| id.clone());
            }
        }
        for c in constraints
            .iter()
            .filter(|c| c.kind == ConstraintKind::MustLink)
        {
            let a = find(&mut parents, &c.left_turn_id);
            let b = find(&mut parents, &c.right_turn_id);
            if a != b {
                let (root, other) = if a < b { (a, b) } else { (b, a) };
                parents.insert(other, root);
            }
        }
        let mut graph = Self::default();
        for id in parents.keys().cloned().collect::<Vec<_>>() {
            let root = find(&mut parents, &id);
            graph.roots.insert(id, root);
        }
        for c in constraints
            .iter()
            .filter(|c| c.kind == ConstraintKind::CannotLink)
        {
            let a = graph.root(&c.left_turn_id).to_owned();
            let b = graph.root(&c.right_turn_id).to_owned();
            if a == b {
                graph.conflicts.insert(a.clone());
            }
            graph.edges.insert(Self::key(&a, &b));
        }
        graph
    }
    fn root<'a>(&'a self, id: &'a str) -> &'a str {
        self.roots.get(id).map(String::as_str).unwrap_or(id)
    }
    fn key(a: &str, b: &str) -> (String, String) {
        if a < b {
            (a.into(), b.into())
        } else {
            (b.into(), a.into())
        }
    }
}
#[derive(Clone)]
struct Cluster {
    id: String,
    examples: Vec<Vec<f32>>,
    members: BTreeSet<String>,
    speech_ms: u64,
}
#[derive(Clone)]
pub(crate) struct Clusters {
    stats: TrackDiagnostics,
    prefix: String,
    groups: Vec<Cluster>,
    next: u32,
    constraints: Vec<PairConstraint>,
    /// Merges made while streaming (must-link joins and cap merges), in order.
    /// `refine` starts its published history from these.
    forced_merges: Vec<SpeakerMerge>,
    graph: ConstraintGraph,
    /// Ids that no longer exist: merged (`Some(into)`) or evicted (`None`).
    /// Bounded by the number of clusters ever opened (<= embeddings).
    retired: BTreeMap<String, Option<String>>,
}
pub(crate) fn normalize(mut value: Vec<f32>) -> Result<Vec<f32>> {
    if value.is_empty() || value.len() > 4096 || value.iter().any(|x| !x.is_finite()) {
        return Err(DiarizeError::Engine);
    }
    let norm = value.iter().map(|x| x * x).sum::<f32>().sqrt();
    if !norm.is_finite() || norm < 1e-10 {
        return Err(DiarizeError::Engine);
    }
    for v in &mut value {
        *v /= norm;
    }
    Ok(value)
}
fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return -1.;
    }
    a.iter()
        .zip(b)
        .map(|(x, y)| x * y)
        .sum::<f32>()
        .clamp(-1., 1.)
}
fn mean(examples: &[Vec<f32>]) -> Vec<f32> {
    let mut m = vec![0.; examples[0].len()];
    for e in examples {
        for (a, b) in m.iter_mut().zip(e) {
            *a += b;
        }
    }
    normalize(m).unwrap_or_else(|_| examples[0].clone())
}
enum Room {
    Open,
    Join(usize, f32),
    Full,
}
/// The best candidate when it passes ASSIGN and beats the runner-up by MARGIN.
fn clear_winner(scores: &[(usize, f32)]) -> Option<(usize, f32)> {
    scores.first().copied().filter(|(_, score)| {
        *score >= ASSIGN && score - scores.get(1).map(|s| s.1).unwrap_or(-1.) >= MARGIN
    })
}
fn prune(group: &mut Cluster) {
    if group.examples.len() > MAX_EXEMPLARS {
        let center = mean(&group.examples);
        let worst = group
            .examples
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| cosine(a, &center).total_cmp(&cosine(b, &center)))
            .unwrap()
            .0;
        group.examples.remove(worst);
    }
}
impl Clusters {
    pub(crate) fn new(prefix: &str, constraints: &[PairConstraint]) -> Self {
        Self {
            stats: TrackDiagnostics::default(),
            prefix: prefix.into(),
            groups: Vec::new(),
            next: 1,
            constraints: constraints.to_vec(),
            forced_merges: Vec::new(),
            graph: ConstraintGraph::new(constraints),
            retired: BTreeMap::new(),
        }
    }
    fn forbidden(&self, a: &BTreeSet<String>, b: &BTreeSet<String>) -> bool {
        if self.graph.edges.is_empty() {
            return false;
        }
        let a = a
            .iter()
            .filter_map(|id| self.graph.roots.get(id))
            .collect::<BTreeSet<_>>();
        let b = b
            .iter()
            .filter_map(|id| self.graph.roots.get(id))
            .collect::<BTreeSet<_>>();
        a.iter().any(|left| {
            b.iter().any(|right| {
                self.graph
                    .edges
                    .contains(&ConstraintGraph::key(left, right))
            })
        })
    }
    /// Returns `None` when the embedding is deliberately left unattributed
    /// (an ambiguous margin failure with best >= MERGE, or the cluster cap
    /// could not be relieved); the caller must surface that turn as
    /// Unknown/uncertain with no speaker id. Reaching the cap is an error only
    /// for a constraint endpoint that cannot get a cluster.
    pub(crate) fn assign(
        &mut self,
        turn: &str,
        embedding: Vec<f32>,
        duration: u64,
    ) -> Result<Option<(String, f32, bool)>> {
        let singleton = BTreeSet::from([turn.to_string()]);
        let mut required = self
            .groups
            .iter()
            .enumerate()
            .filter(|(_, g)| {
                self.graph.roots.contains_key(turn)
                    && g.members
                        .iter()
                        .any(|member| self.graph.root(member) == self.graph.root(turn))
            })
            .map(|(i, _)| i)
            .collect::<Vec<_>>();
        if required.len() > 1 {
            for (position, &a) in required.iter().enumerate() {
                for &b in &required[position + 1..] {
                    if self.forbidden(&self.groups[a].members, &self.groups[b].members) {
                        return Err(DiarizeError::Constraint);
                    }
                }
            }
            let base = required[0];
            for &other in required[1..].iter().rev() {
                let merge = self.merge_pair(base, other);
                self.record_merge(merge);
            }
            required.truncate(1);
        }
        let scores = self.scores(&embedding, &singleton);
        let selected = if let Some(&required) = required.first() {
            if self.forbidden(&singleton, &self.groups[required].members) {
                return Err(DiarizeError::Constraint);
            }
            Some((
                required,
                cosine(&embedding, &mean(&self.groups[required].examples)),
            ))
        } else {
            clear_winner(&scores)
        };
        // Margin failure: best >= ASSIGN but too close to the runner-up.
        let margin_failure = match (required.is_empty(), selected, scores.first(), scores.get(1)) {
            (true, None, Some(&(best_i, best)), Some(&(second_i, _))) if best >= ASSIGN => {
                Some((best, best_i, second_i))
            }
            _ => None,
        };
        if let Some((best, best_i, second_i)) = margin_failure {
            let centroids = cosine(
                &mean(&self.groups[best_i].examples),
                &mean(&self.groups[second_i].examples),
            );
            self.stats.record_margin_failure(best, centroids);
            // A high-similarity but ambiguous window is left unattributed: no
            // cluster is opened and no exemplar, member or speech time is added.
            // Opening a singleton on it would make the next window of the same
            // voice ambiguous again. Constraint anchors keep the old behaviour so
            // must-link/cannot-link checks always see them in a cluster.
            if best >= MERGE && !self.graph.roots.contains_key(turn) {
                self.stats.ambiguous_unattributed += 1;
                return Ok(None);
            }
        }
        let (index, score) = if let Some(selected) = selected {
            self.stats.assigned_existing += 1;
            selected
        } else {
            match self.make_room(turn, &embedding, &singleton)? {
                Room::Join(index, score) => {
                    self.stats.assigned_existing += 1;
                    self.stats.cap_reassigned += 1;
                    return Ok(Some(self.add(index, score, turn, embedding, duration)));
                }
                Room::Full => return Ok(None),
                Room::Open => {}
            }
            self.stats.record_open(
                self.groups.is_empty(),
                scores.first().map(|s| s.1),
                scores.get(1).map(|s| s.1),
            );
            let id = format!("{}:speaker-{}", self.prefix, self.next);
            self.next += 1;
            self.groups.push(Cluster {
                id,
                examples: vec![],
                members: BTreeSet::new(),
                speech_ms: 0,
            });
            (self.groups.len() - 1, 1.)
        };
        self.stats.peak_clusters = self.stats.peak_clusters.max(self.groups.len() as u32);
        Ok(Some(self.add(index, score, turn, embedding, duration)))
    }
    fn add(
        &mut self,
        index: usize,
        score: f32,
        turn: &str,
        embedding: Vec<f32>,
        duration: u64,
    ) -> (String, f32, bool) {
        let group = &mut self.groups[index];
        group.examples.push(embedding);
        group.members.insert(turn.into());
        group.speech_ms += duration;
        prune(group);
        (group.id.clone(), score, group.speech_ms >= ESTABLISHED_MS)
    }
    fn record_merge(&mut self, merge: SpeakerMerge) {
        self.retired
            .insert(merge.from.clone(), Some(merge.into.clone()));
        self.forced_merges.push(merge);
    }
    /// Merge the most similar non-forbidden pairs with centroid cosine >=
    /// MERGE, highest first, until at most `target` clusters remain. Without
    /// `established`, only pairs with at least one provisional cluster are
    /// eligible; with it, only pairs of two established clusters. The cluster
    /// with more speech absorbs the other (older on a tie). Ties between pairs
    /// keep the lowest indices, so replay is deterministic.
    fn cap_merge(&mut self, established: bool, target: usize) {
        let mut centroids = self
            .groups
            .iter()
            .map(|g| mean(&g.examples))
            .collect::<Vec<_>>();
        while self.groups.len() > target {
            let mut best: Option<(usize, usize, f32)> = None;
            for a in 0..self.groups.len() {
                for b in a + 1..self.groups.len() {
                    let both = self.groups[a].speech_ms >= ESTABLISHED_MS
                        && self.groups[b].speech_ms >= ESTABLISHED_MS;
                    if both != established {
                        continue;
                    }
                    let score = cosine(&centroids[a], &centroids[b]);
                    if score < MERGE
                        || best.is_some_and(|(_, _, s)| score <= s)
                        || self.forbidden(&self.groups[a].members, &self.groups[b].members)
                    {
                        continue;
                    }
                    best = Some((a, b, score));
                }
            }
            let Some((a, b, _)) = best else {
                break;
            };
            let (into, from) = if self.groups[b].speech_ms > self.groups[a].speech_ms {
                (b, a)
            } else {
                (a, b)
            };
            let merge = self.merge_pair(into, from);
            self.record_merge(merge);
            self.stats.cap_merges += 1;
            centroids.remove(from);
            let into = if from < into { into - 1 } else { into };
            centroids[into] = mean(&self.groups[into].examples);
        }
    }
    /// Remove the oldest single-member cluster with < 3 s speech and no
    /// constraint-anchored member. Oldest first: a singleton that never gained
    /// a second window is the likeliest stray, while the newest one may be a
    /// new voice that has not had time to speak again.
    fn cap_evict(&mut self) {
        let Some(index) = self.groups.iter().position(|g| {
            g.members.len() == 1
                && g.speech_ms < ESTABLISHED_MS
                && !g.members.iter().any(|m| self.graph.roots.contains_key(m))
        }) else {
            return;
        };
        let group = self.groups.remove(index);
        self.retired.insert(group.id, None);
        self.stats.cap_evictions += 1;
        self.stats.cap_evicted_ms += group.speech_ms;
    }
    /// Current id for a speaker id handed out earlier: follows merges, and is
    /// `None` for an evicted cluster. The bool is "established" (>= 3 s).
    pub(crate) fn resolve<'a>(&'a self, id: &'a str) -> Option<(&'a str, bool)> {
        let mut id = id;
        while let Some(next) = self.retired.get(id) {
            id = next.as_deref()?;
        }
        self.groups
            .iter()
            .find(|g| g.id == id)
            .map(|g| (g.id.as_str(), g.speech_ms >= ESTABLISHED_MS))
    }
    pub(crate) fn retired_count(&self) -> usize {
        self.retired.len()
    }
    /// Candidate clusters (cannot-link partners removed) by cosine, best first.
    fn scores(&self, embedding: &[f32], singleton: &BTreeSet<String>) -> Vec<(usize, f32)> {
        let mut scores = self
            .groups
            .iter()
            .enumerate()
            .filter(|(_, g)| !self.forbidden(singleton, &g.members))
            .map(|(i, g)| (i, cosine(embedding, &mean(&g.examples))))
            .collect::<Vec<_>>();
        scores.sort_by(|a, b| b.1.total_cmp(&a.1));
        scores
    }
    /// Called only when a new cluster is needed (so `required` was empty and
    /// no earlier index is reused afterwards: merges and evictions reorder
    /// `groups`). Below the cap it does nothing. At the cap, in order:
    /// 1. merge the most similar duplicate pairs (centroid cosine >= MERGE)
    ///    that involve a provisional cluster, until CAP_HEADROOM slots are free;
    /// 2. evict the oldest provisional, unanchored singleton (one slot);
    /// 3. merge the single most similar pair of established clusters.
    ///
    /// After any merge the window is re-scored and may now clearly join an
    /// existing cluster. With no free slot the window is unattributed (`Err`
    /// for a constraint endpoint, which must not silently lose its cluster).
    fn make_room(
        &mut self,
        turn: &str,
        embedding: &[f32],
        singleton: &BTreeSet<String>,
    ) -> Result<Room> {
        if self.groups.len() < MAX_CLUSTERS {
            return Ok(Room::Open);
        }
        let merges = self.stats.cap_merges;
        self.cap_merge(false, MAX_CLUSTERS - CAP_HEADROOM);
        if self.groups.len() >= MAX_CLUSTERS {
            self.cap_evict();
        }
        if self.groups.len() >= MAX_CLUSTERS {
            self.cap_merge(true, MAX_CLUSTERS - 1);
        }
        if self.stats.cap_merges > merges
            && let Some((index, score)) = clear_winner(&self.scores(embedding, singleton))
        {
            return Ok(Room::Join(index, score));
        }
        if self.groups.len() >= MAX_CLUSTERS {
            if self.graph.roots.contains_key(turn) {
                self.stats.capacity_refused = true;
                return Err(DiarizeError::Capacity(CapacityLimit::Clusters));
            }
            self.stats.cap_unattributed += 1;
            return Ok(Room::Full);
        }
        Ok(Room::Open)
    }
    /// Counters only; singleton count is derived from current membership.
    pub(crate) fn diagnostics(&self) -> TrackDiagnostics {
        let mut d = self.stats.clone();
        d.clusters = self.groups.len() as u32;
        d.singleton_clusters = self.groups.iter().filter(|g| g.members.len() == 1).count() as u32;
        d.embeddings_assigned = d.assigned_existing + d.opened_total;
        d
    }
    /// Merges cluster `b` into cluster `a` (either order of indices).
    fn merge_pair(&mut self, a: usize, b: usize) -> SpeakerMerge {
        let removed = self.groups.remove(b);
        let a = if b < a { a - 1 } else { a };
        let into = &mut self.groups[a];
        let change = SpeakerMerge {
            from: removed.id,
            into: into.id.clone(),
        };
        into.members.extend(removed.members);
        into.speech_ms += removed.speech_ms;
        for e in removed.examples {
            into.examples.push(e);
            prune(into);
        }
        change
    }
    pub(crate) fn refine(&mut self, prior: Option<usize>) -> Vec<SpeakerMerge> {
        // Constrained agglomeration on bounded streaming seed clusters. The
        // silhouette search evaluates a capped exemplar sample, never N² audio.
        let mut best = self.clone();
        let mut best_merges = self.forced_merges.clone();
        let mut history = self.forced_merges.clone();
        let mut best_score = self.silhouette(prior);
        loop {
            let mut candidate = None;
            for a in 0..self.groups.len() {
                for b in a + 1..self.groups.len() {
                    if self.forbidden(&self.groups[a].members, &self.groups[b].members) {
                        continue;
                    }
                    let score = cosine(
                        &mean(&self.groups[a].examples),
                        &mean(&self.groups[b].examples),
                    );
                    if score >= MERGE && candidate.is_none_or(|(_, _, s)| score > s) {
                        candidate = Some((a, b, score));
                    }
                }
            }
            let Some((a, b, _)) = candidate else {
                break;
            };
            history.push(self.merge_pair(a, b));
            let score = self.silhouette(prior);
            if score >= best_score {
                best_score = score;
                best = self.clone();
                best_merges = history.clone();
            }
        }
        *self = best;
        best_merges
    }
    fn silhouette(&self, prior: Option<usize>) -> f32 {
        if self.groups.len() < 2 {
            return -1.;
        }
        let mut total = 0.;
        let mut count: f32 = 0.;
        for (i, g) in self.groups.iter().enumerate() {
            let own = g.examples.iter().take(2).collect::<Vec<_>>();
            for (j, x) in own.iter().enumerate() {
                count += 1.;
                if own.len() < 2 {
                    continue;
                }
                let a = own
                    .iter()
                    .enumerate()
                    .filter(|(k, _)| *k != j)
                    .map(|(_, y)| 1. - cosine(x, y))
                    .sum::<f32>()
                    / (own.len() - 1) as f32;
                let b = self
                    .groups
                    .iter()
                    .enumerate()
                    .filter(|(k, _)| *k != i)
                    .map(|(_, other)| {
                        let sample = other.examples.iter().take(2).collect::<Vec<_>>();
                        sample.iter().map(|y| 1. - cosine(x, y)).sum::<f32>() / sample.len() as f32
                    })
                    .fold(f32::INFINITY, f32::min);
                let denom = a.max(b);
                if denom > 1e-8 {
                    total += (b - a) / denom;
                }
            }
        }
        total / count.max(1.)
            - prior
                .map(|n| 0.01 * self.groups.len().abs_diff(n) as f32)
                .unwrap_or(0.)
    }
    pub(crate) fn speaker_for(&self, turn: &str) -> Option<&str> {
        self.groups
            .iter()
            .find(|g| g.members.contains(turn))
            .map(|g| g.id.as_str())
    }
    pub(crate) fn speakers(&self, track: &str) -> Vec<Speaker> {
        self.groups
            .iter()
            .map(|g| Speaker {
                id: g.id.clone(),
                track: track.into(),
                label: format!("Speaker {}", g.id.rsplit('-').next().unwrap_or("?")),
                speech_ms: g.speech_ms,
                provisional: g.speech_ms < ESTABLISHED_MS,
            })
            .collect()
    }
    pub(crate) fn violated_constraint_turns(&self, seen: &BTreeSet<String>) -> BTreeSet<String> {
        let mut violated = BTreeSet::new();
        for c in &self.constraints {
            if !seen.contains(&c.left_turn_id) || !seen.contains(&c.right_turn_id) {
                continue;
            }
            let a = self.speaker_for(&c.left_turn_id);
            let b = self.speaker_for(&c.right_turn_id);
            if c.kind == ConstraintKind::MustLink && (a.is_none() || b.is_none() || a != b)
                || c.kind == ConstraintKind::CannotLink && a.is_some() && a == b
            {
                violated.insert(c.left_turn_id.clone());
                violated.insert(c.right_turn_id.clone());
            }
        }
        violated
    }
    pub(crate) fn check_constraints(&self, seen: &BTreeSet<String>) -> Result<()> {
        if self.violated_constraint_turns(seen).is_empty() {
            Ok(())
        } else {
            Err(DiarizeError::Constraint)
        }
    }
}
