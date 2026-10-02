// `MediaEntry` is re-exported from the shared crate (the raw feed item plugins
// produce). `ReleaseCandidate` is the enriched form produced by
// `process_entry_with_mappings()` after merging scoring, mapping, pack analysis,
// and filtering.
use chrono::{DateTime, Utc};
use jumbie_shared::mapping::{EpisodeInfo, MappingRule};
pub use jumbie_shared::types::MediaEntry;
use jumbie_shared::types::MultiTarget;
use jumbie_shared::types::ReleaseRank;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize)]
// ReleaseCandidate is deliberately flat (no nested enums/trait objects): it is
// serialized to JSON for the WebSocket API, stored as a JSON blob in the download
// queue, and sorted by score. Flattening avoids repeated serde round-trips and keeps
// the winner-selection hot path allocation-free.
pub struct ReleaseCandidate {
    pub title: String,
    pub download_url: Option<String>,
    pub episode_info: EpisodeInfo,
    pub mapping: Arc<MappingRule>,
    pub meta_date: Option<DateTime<Utc>>,
    pub score: i32,
    pub score_breakdown: Vec<String>,
    pub description: Option<String>,
    pub file_list: Vec<String>,
    pub guid: Option<String>,
    pub needed_episodes: Vec<i32>,
    pub unneeded_count: i32,
    pub seeders: u32,
    pub leechers: u32,
    pub indexer: String,
    pub size_bytes: u64,
    pub download_id: Option<String>,
    /// Additional (series, season, episode) targets that want this same source file.
    /// Populated by merge_shared_downloads. The file is downloaded once and copied
    /// to each target's destination after organize completes.
    #[serde(default)]
    pub multi_targets: Vec<MultiTarget>,
    /// Resolved release group/submitter — plugin-provided or parsed from title.
    /// Preserved here so it can flow into the episode record at insert time.
    #[serde(default)]
    pub submitter: Option<String>,
}

// Ordering is by score only — equality means "equally preferred". Ranking for
// winner selection goes through `rank()` / `compare_release_rank_desc`, which adds
// the deterministic date/seeders tie-breakers.
impl PartialEq for ReleaseCandidate {
    fn eq(&self, other: &Self) -> bool {
        self.score == other.score
    }
}

impl Eq for ReleaseCandidate {}

impl PartialOrd for ReleaseCandidate {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ReleaseCandidate {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.score.cmp(&other.score)
    }
}

impl ReleaseCandidate {
    /// Canonical ranking key (score, publish date, seeders) — see
    /// [`jumbie_shared::types::compare_release_rank_desc`].
    pub fn rank(&self) -> ReleaseRank {
        ReleaseRank {
            score: self.score,
            published: self.meta_date,
            seeders: self.seeders,
        }
    }
}
