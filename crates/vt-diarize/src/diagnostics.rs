//! Fixed-size counters describing why clustering behaved as it did. No audio,
//! embeddings or text are stored; counters survive an error.
use crate::models::{ASSIGN, MARGIN, MERGE};
use serde::{Deserialize, Serialize};

/// Which hard limit produced `DiarizeError::Capacity`.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CapacityLimit {
    Constraints,
    Turns,
    Embeddings,
    Clusters,
}
impl std::fmt::Display for CapacityLimit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Constraints => "constraints",
            Self::Turns => "turns",
            Self::Embeddings => "embeddings",
            Self::Clusters => "clusters",
        })
    }
}
/// Best-score buckets: 20 buckets of 0.1 covering [-1, 1].
pub const SCORE_BUCKETS: usize = 20;
/// Margin buckets: 10 equal buckets covering [0, MARGIN) (a margin failure is < MARGIN).
pub const MARGIN_BUCKETS: usize = 10;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct TrackDiagnostics {
    pub windows: u32,
    pub turns: u32,
    pub embeddings_assigned: u32,
    pub assigned_existing: u32,
    pub opened_total: u32,
    pub opened_no_candidate: u32,
    pub opened_below_assign: u32,
    pub opened_margin_failure: u32,
    pub opened_all_candidates_cannot_link: u32,
    pub clusters: u32,
    pub peak_clusters: u32,
    pub singleton_clusters: u32,
    pub capacity_refused: bool,
    /// Best cosine for opens caused by best < ASSIGN; bucket i = [-1+0.1i, -1+0.1(i+1)).
    pub best_below_assign_hist: [u32; SCORE_BUCKETS],
    /// Best cosine for opens caused by margin failure (best >= ASSIGN).
    pub best_margin_failure_hist: [u32; SCORE_BUCKETS],
    /// Margin (best - second) for margin-failure opens; bucket i = [MARGIN*i/10, MARGIN*(i+1)/10).
    pub margin_failure_hist: [u32; MARGIN_BUCKETS],
    /// Every margin failure (best >= ASSIGN, best - second < MARGIN, no must-link
    /// cluster), whether it opened a cluster or was left unattributed.
    #[serde(default)]
    pub margin_failures: u32,
    /// Margin failures whose best score was >= MERGE.
    #[serde(default)]
    pub margin_failure_best_at_least_merge: u32,
    /// Centroid-to-centroid cosine of the two top candidates on margin failures;
    /// same buckets as the best-score histograms.
    #[serde(default)]
    pub margin_failure_centroid_hist: [u32; SCORE_BUCKETS],
    /// Margin failures with best >= MERGE left without a speaker (no cluster
    /// opened, no exemplar added); the turn surfaces as Unknown.
    #[serde(default)]
    pub ambiguous_unattributed: u32,
    /// Cluster-cap handling (never an error for ordinary turns): duplicate
    /// clusters merged to make room.
    #[serde(default)]
    pub cap_merges: u32,
    /// Provisional singleton clusters removed to make room; their turn is Unknown.
    #[serde(default)]
    pub cap_evictions: u32,
    /// Speech (ms) of the evicted singletons.
    #[serde(default)]
    pub cap_evicted_ms: u64,
    /// Windows left unattributed because the cap could not be relieved.
    #[serde(default)]
    pub cap_unattributed: u32,
    /// Windows that, after a cap merge, cleanly joined an existing cluster
    /// instead of opening a new one.
    #[serde(default)]
    pub cap_reassigned: u32,
}
fn score_bucket(v: f32) -> usize {
    (((v + 1.) * 10.).floor().max(0.) as usize).min(SCORE_BUCKETS - 1)
}
impl TrackDiagnostics {
    /// True when the per-track cluster cap changed any assignment.
    pub fn cap_reached(&self) -> bool {
        self.cap_merges + self.cap_evictions + self.cap_unattributed > 0
    }
    /// Record a margin failure: the best score and the cosine between the two
    /// top candidates' centroids.
    pub(crate) fn record_margin_failure(&mut self, best: f32, centroids: f32) {
        self.margin_failures += 1;
        if best >= MERGE {
            self.margin_failure_best_at_least_merge += 1;
        }
        self.margin_failure_centroid_hist[score_bucket(centroids)] += 1;
    }
    /// Record why a new cluster was opened. `candidates` are cosine scores after
    /// cannot-link removal, best first.
    pub(crate) fn record_open(&mut self, no_groups: bool, best: Option<f32>, second: Option<f32>) {
        self.opened_total += 1;
        match best {
            None if no_groups => self.opened_no_candidate += 1,
            None => self.opened_all_candidates_cannot_link += 1,
            Some(b) if b < ASSIGN => {
                self.opened_below_assign += 1;
                self.best_below_assign_hist[score_bucket(b)] += 1;
            }
            Some(b) => {
                self.opened_margin_failure += 1;
                self.best_margin_failure_hist[score_bucket(b)] += 1;
                let m = (b - second.unwrap_or(-1.)).max(0.);
                let i = ((m / MARGIN * MARGIN_BUCKETS as f32) as usize).min(MARGIN_BUCKETS - 1);
                self.margin_failure_hist[i] += 1;
            }
        }
    }
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Diagnostics {
    pub windows: u32,
    pub turns: u32,
    pub embeddings: u32,
    pub capacity_limit: Option<CapacityLimit>,
    pub capacity_track: Option<String>,
    /// Input audio time (ms since that track's origin clock) when the limit fired.
    pub capacity_at_ms: Option<u64>,
    pub mic: TrackDiagnostics,
    pub system: TrackDiagnostics,
}
