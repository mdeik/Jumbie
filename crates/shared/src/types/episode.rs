use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::media_format::FileKind;
use crate::types::{MediaInfo, ReleaseDates};

/// A subtitle or nfo sidecar attached to an episode. Auxiliary files never
/// replace the episode's playable file; an episode may have many subtitles but
/// at most one nfo.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AuxiliaryFile {
    /// Absolute path of the sidecar on disk.
    pub path: String,
    pub kind: FileKind,
}

/// Tracks how complete a series is — derived from comparing expected vs.
/// organized episode counts. Computed server-side in one place to avoid
/// scattered frontend logic.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum CompletionStatus {
    /// Zero episodes have files on disk.
    NotStarted,
    /// Some episodes have files, but not all expected ones.
    Partial,
    /// All expected episodes have files on disk.
    Complete,
}

impl CompletionStatus {
    pub fn compute(total_expected: i32, total_organized: i32) -> Self {
        if total_organized == 0 {
            Self::NotStarted
        } else if total_organized >= total_expected {
            Self::Complete
        } else {
            Self::Partial
        }
    }

    /// Numeric rank for sorting: NotStarted=0, Partial=1, Complete=2.
    pub fn rank(&self) -> i32 {
        match self {
            Self::NotStarted => 0,
            Self::Partial => 1,
            Self::Complete => 2,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EpisodeViewModel {
    pub unique_id: String,
    pub season: String,
    pub episode: i32,
    pub header: String,
    pub title: Option<String>,
    pub status: String,
    pub quality_profile_id: Option<String>,
    pub size: u64,
    pub submitter: Option<String>,
    pub path: Option<String>,
    /// First path the episode's origin content was ever seen at. Survives
    /// unassign, move, rename, and copy; None when unknown or pruned.
    #[serde(default)]
    pub original_path: Option<String>,
    pub media_info: Option<MediaInfo>,
    pub fingerprint: Option<String>,
    pub created_at: Option<String>,
    pub file_acquired_at: Option<String>,
    #[serde(default)]
    pub monitored: bool,
    pub dates: ReleaseDates<String>,
    #[serde(default)]
    pub metadata_ids: HashMap<String, String>,
    pub description: Option<String>,
    pub runtime: Option<i32>,
    pub image_url: Option<String>,
    pub metadata_source: Option<String>,
    /// The original release title from when this episode was downloaded.
    #[serde(default)]
    pub release_title: Option<String>,
    /// For multi-part files: the individual parts of this episode.
    #[serde(default)]
    pub parts: Vec<crate::types::EpisodePartInfo>,
    /// Subtitle and nfo sidecars attached to this episode (never its playable file).
    #[serde(default)]
    pub auxiliary_files: Vec<AuxiliaryFile>,
    /// When `true`, this episode cell is only shown in the grid because a
    /// downloaded file exists on disk, not because it falls within the
    /// season's configured `cell_count` range. Rendered with a ghost/dimmed
    /// appearance to distinguish it from in-range episodes.
    #[serde(default)]
    pub show_only_downloaded: bool,
    /// The episode has at least one `main` file association — a direct file or a
    /// multipart part. DB state only; never touches disk.
    #[serde(default)]
    pub assigned: bool,
    /// The episode's `main` file(s) are physically present on disk: the direct file
    /// for a single/multi-episode file, or all currently assigned parts for a
    /// multipart episode. Auxiliary files never contribute.
    #[serde(default)]
    pub disk_present: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CalendarEpisode {
    pub series_title: String,
    /// Series UUID — lets the frontend open the episode-details modal from
    /// the calendar without a per-episode lookup (SSoT: series-scoped modal data).
    pub series_id: String,
    pub episode_id: String,
    pub season: String,
    pub episode: i32,
    pub episode_title: Option<String>,
    pub dates: ReleaseDates<String>,
    #[serde(default)]
    pub eff_date: String,
    pub status: String,
    /// The episode has at least one `main` file association. DB state only.
    #[serde(default)]
    pub assigned: bool,
    /// The episode's `main` file(s) are physically present on disk.
    #[serde(default)]
    pub disk_present: bool,
}

/// Combined response for the calendar endpoint — returns episodes for a date range.
/// Episode detail modals are fetched on demand via the single-episode endpoint
/// (`fetch_episode_details_{id}`), keeping this hot, polled response lean.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalendarResponse {
    pub episodes: Vec<CalendarEpisode>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetadataEpisodeData {
    pub season: i32,
    pub episode: i32,
    pub metadata_id: String,
    pub title: String,
    pub description: Option<String>,
    pub runtime: Option<i32>,
    pub image_url: Option<String>,
    pub meta_date: Option<String>, // Usually airstamp or airdate from metadata provider
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncMetadataRequest {
    pub episodes: Vec<MetadataEpisodeData>,
}

/// Returned by the backend-side fetch_metadata endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetadataSyncStatus {
    pub synced_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "backend", derive(sqlx::FromRow))]
pub struct RetryItem {
    pub id: i64,
    pub operation: String,
    pub source_path: String,
    pub destination_path: Option<String>,
    pub episode_id: Option<String>,
    pub error_message: Option<String>,
    pub retry_count: i32,
    pub max_retries: i32,
    #[serde(with = "crate::serde_utc::option_naive_utc")]
    pub next_retry_at: Option<chrono::NaiveDateTime>,
    pub status: String,
}
