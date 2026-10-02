use serde::{Deserialize, Serialize};

/// A single episode's intention recorded at queue time.
/// Immutable once stored — the source processor decides once which episodes
/// this download genuinely targets (`keep = true`) and which are pack overspill
/// (`keep = false`). Smart-link uses this to route files without re-deriving
/// intent from the DB or filename heuristics.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EpisodeIntention {
    /// Episode number in the local (DB) numbering space — the same space as
    /// `episodes.episode`.
    pub episode_num: i32,
    /// Episode number releases use (local + the season's episode offset).
    /// Smart-link matches parsed filenames against this.
    pub source_episode_num: i32,
    pub episode_id: String,
    pub score: i32,
    /// If false, this file should never be kept — route to `unneeded_files`
    /// without fingerprinting or offense tracking.
    pub keep: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "backend", derive(sqlx::FromRow))]
pub struct DownloadQueueItem {
    pub id: i64,
    #[serde(with = "crate::serde_utc::naive_utc")]
    pub downloaded_at: chrono::NaiveDateTime,
    pub media_name: String,
    pub media_link: String,
    pub series_title: String,
    pub season: Option<String>,
    /// The known episode number. `None` means the episode is not yet resolved —
    /// smart-link will determine the episode(s) from files on disk when the
    /// download completes. This is the single source of truth for whether a
    /// queue item's episode context is known at queue time.
    pub episode: Option<i32>,
    /// Last episode in a multi-episode range (e.g. 4 for S01E01-04). None for single episodes.
    #[serde(default)]
    #[cfg_attr(feature = "backend", sqlx(default))]
    pub episode_end: Option<i32>,
    /// Episode ID FK, nullable because series-level search downloads don't know
    /// their episode until smart-link resolves files on disk. None when pending.
    pub episode_id: Option<String>,
    pub score: i32,
    /// True when the user explicitly requested this download (search modal or the
    /// episode's Auto Search) rather than the background scanner; at organize time it
    /// temporarily enables upgrade evaluation for a fair comparison against existing files.
    pub is_user_requested: bool,
    /// True when the user picked this specific release (link + download_id) rather
    /// than the system selecting it by score. Manual items render a `Manual` badge
    /// and are exempt from no-progress autoresolve. Unlike `is_user_requested`,
    /// which is also set by user-initiated auto-search, this marks a deliberate
    /// release choice.
    #[serde(default)]
    #[cfg_attr(feature = "backend", sqlx(default))]
    pub is_manual: bool,
    /// JSON array of additional (series_id, season, episode) targets sharing this download.
    /// None when this is a single-target download.
    #[serde(default)]
    #[cfg_attr(feature = "backend", sqlx(default))]
    pub multi_targets: Option<String>,
    pub status: String,
    /// The download client's torrent hash, recorded at queue time by
    /// `enrich_and_enqueue` (source plugins supply it, e.g. Nyaa's infoHash).
    /// Used for dispatch and duplicate-queue detection.
    #[serde(default)]
    #[cfg_attr(feature = "backend", sqlx(default))]
    pub download_id: Option<String>,
    pub downloader_id: Option<String>,
    /// The stable ID of the download client that accepted this item (e.g. "qBittorrent").
    /// Used to route pause/resume/delete commands directly without iterating all clients.
    /// None for legacy items that pre-date this field.
    #[serde(default)]
    #[cfg_attr(feature = "backend", sqlx(default))]
    pub client_id: Option<String>,
    /// Added to support progress bars in the UI, not persisted in DB
    #[serde(default)]
    #[cfg_attr(feature = "backend", sqlx(default))]
    pub progress: Option<f32>,
    /// Transient: true when progress has not advanced for the configured
    /// no-progress threshold. Computed at read time from `no_progress_since`,
    /// never persisted.
    #[serde(default)]
    #[cfg_attr(feature = "backend", sqlx(default))]
    pub no_progress: bool,
    /// Transient: minutes without progress, for the warning tooltip.
    #[serde(default)]
    #[cfg_attr(feature = "backend", sqlx(default))]
    pub no_progress_minutes: Option<i64>,
    /// Whether the assigned download client supports pause/resume. Transient — not persisted.
    #[serde(default)]
    #[cfg_attr(feature = "backend", sqlx(default))]
    pub supports_pause_resume: bool,
    /// Error message from the last failure. Persisted in DB. None for non-failed items.
    #[serde(default)]
    #[cfg_attr(feature = "backend", sqlx(default))]
    pub error_message: Option<String>,
    /// UUID FK linking this queue item to its series mapping.
    /// Always set for items created post-migration. Empty string for legacy items.
    #[serde(default)]
    #[cfg_attr(feature = "backend", sqlx(default))]
    pub series_id: String,
    /// Explicit flag: whether this queue item represents a season/complete pack.
    /// Never re-derived from filename. False for legacy items.
    #[serde(default)]
    #[cfg_attr(feature = "backend", sqlx(default))]
    pub is_season_pack: bool,
    /// Category to pass to the download client. E.g. "Series", "Movies".
    /// Empty string means use the client's default.
    #[serde(default)]
    #[cfg_attr(feature = "backend", sqlx(default))]
    pub category: String,
    /// How many times content-path resolution has been retried (exponential backoff).
    /// 0 means not currently in a retry cycle.
    #[serde(default)]
    #[cfg_attr(feature = "backend", sqlx(default))]
    pub retry_count: i32,
    /// Last sampled download progress. Used to detect a stalled transfer.
    #[serde(default)]
    #[cfg_attr(feature = "backend", sqlx(default))]
    pub last_progress: Option<f32>,
    /// When progress last advanced. NULL until the first sample. A `Downloading`
    /// item whose `now - no_progress_since` exceeds the configured threshold is
    /// reported as no-progress.
    #[serde(default, with = "crate::serde_utc::option_naive_utc")]
    #[cfg_attr(feature = "backend", sqlx(default))]
    pub no_progress_since: Option<chrono::NaiveDateTime>,
    /// Next allowed retry time for content-path resolution. NULL when not in retry.
    #[serde(default, with = "crate::serde_utc::option_naive_utc")]
    #[cfg_attr(feature = "backend", sqlx(default))]
    pub next_retry_at: Option<chrono::NaiveDateTime>,
    /// JSON array of EpisodeIntention, recorded at queue time.
    /// Immutable — consumed by smart-link to route files and by organize
    /// to get per-episode scores for upgrade evaluation.
    /// None for legacy items or manual uploads.
    #[serde(default)]
    #[cfg_attr(feature = "backend", sqlx(default))]
    pub episode_intentions: Option<String>,
    /// Scoring inputs stored alongside the score for retroactive rescore.
    /// These are the raw values passed to ReleaseProfile::calculate() at
    /// queue time — stored here so finalize_download can propagate them to
    /// release_metadata for every episode covered by this download.
    #[serde(default)]
    #[cfg_attr(feature = "backend", sqlx(default))]
    pub scoring_size_bytes: Option<i64>,
    #[serde(default)]
    #[cfg_attr(feature = "backend", sqlx(default))]
    pub scoring_seeders: Option<i32>,
    #[serde(default)]
    #[cfg_attr(feature = "backend", sqlx(default))]
    pub scoring_episode_count: Option<i32>,
    /// Submitter/release-group resolved at plugin boundary.
    /// Carried through from source plugin → download queue → release_metadata.
    /// None for legacy items or sources that didn't provide one.
    #[serde(default)]
    #[cfg_attr(feature = "backend", sqlx(default))]
    pub submitter: Option<String>,
    /// Quality profile UUID snapshotted from the series mapping at queue time.
    /// Written to the episode row at organize time (the mapping may change
    /// between queue and completion, so this snapshot is authoritative).
    #[serde(default)]
    #[cfg_attr(feature = "backend", sqlx(default))]
    pub quality_profile_id: Option<String>,
    /// Parsed version number from the release title (e.g. v2).
    /// Defaults to 1 if no version marker was found.
    #[serde(default = "default_queue_version")]
    #[cfg_attr(feature = "backend", sqlx(default))]
    pub version: i32,
}

/// A single additional target for a shared download.
/// When the same source file (magnet/URL) matches multiple (series, season, episode)
/// combinations, the extra targets are stored here so the file is copied to each
/// destination after download completes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MultiTarget {
    pub series_id: String,
    pub season: i32,
    pub episode: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AddQueueResult {
    /// A new queue entry was created; carries its row id.
    Added {
        id: i64,
    },
    Replaced(Box<DownloadQueueItem>),
    Skipped,
    /// The new target was merged into an existing queue entry that shares the same
    /// media_link.  The download was already queued — no new entry was created.
    Merged {
        existing_id: i64,
        new_targets: usize,
    },
}

impl AddQueueResult {
    /// The queue row this result refers to, when one is known.
    ///
    /// `None` only for [`AddQueueResult::Skipped`] — nothing new was queued (a
    /// skip can also mean an equal-or-better entry already exists, whose id is not
    /// tracked here).
    pub fn queue_id(&self) -> Option<i64> {
        match self {
            AddQueueResult::Added { id } => Some(*id),
            AddQueueResult::Replaced(item) => Some(item.id),
            AddQueueResult::Merged { existing_id, .. } => Some(*existing_id),
            AddQueueResult::Skipped => None,
        }
    }

    /// Stable lowercase outcome label, matching `SearchResult::queue_action`
    /// (`"added"`, `"replaced"`, `"skipped"`, `"merged"`).
    pub fn outcome(&self) -> &'static str {
        match self {
            AddQueueResult::Added { .. } => "added",
            AddQueueResult::Replaced(_) => "replaced",
            AddQueueResult::Skipped => "skipped",
            AddQueueResult::Merged { .. } => "merged",
        }
    }
}

/// Response to `POST /api/downloads`.
///
/// The request only *queues* the download — the download client is assigned later
/// by the organizer. `queue_id` lets a caller poll `GET /api/queue` for the
/// assigned `client_id`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AddDownloadResponse {
    /// What the queue did: `"added"`, `"replaced"`, `"skipped"`, or `"merged"`.
    pub outcome: String,
    /// Queue row id, when one is known (`None` for a plain skip).
    pub queue_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenameQueueItem {
    pub series_id: String,
    pub series_title: String,
    pub affected_episodes: usize,
    pub collision_count: usize,
    pub causes: Vec<String>,
    pub absolute_numbering: bool,
    #[serde(default)]
    pub has_failed: bool,
    /// Set to true while this series is being reorganized.
    /// Frontend uses this to show a loading spinner and grey out the row.
    #[serde(default)]
    pub processing: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenameQueueResponse {
    pub items: Vec<RenameQueueItem>,
    pub total_affected_episodes: usize,
    #[serde(default)]
    pub has_failed_renames: bool,
}

/// A single planned file rename shown in the detail modal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenameDetail {
    pub original: String, // current file path (src)
    pub expected: String, // target file path (dst)
    pub has_collision: bool,
    /// `Some(n)` when this entry is a part file (e.g. pt1, cd2).
    /// `None` for normal single-episode and multi-episode files.
    /// Use `part_number.is_some()` to check — no separate `is_part` field needed.
    #[serde(default)]
    pub part_number: Option<u32>,
    /// `Some(subtitle|nfo)` when this entry is an auxiliary sidecar; `None` for a
    /// playable video file.
    #[serde(default)]
    pub aux_kind: Option<crate::media_format::FileKind>,
}

fn default_queue_version() -> i32 {
    1
}

/// Detail response for a single rename-queue item (all files for one series).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenameQueueItemDetail {
    pub series_id: String,
    pub series_title: String,
    pub renames: Vec<RenameDetail>,
    pub collision_mode: String,
    /// Destination folders that do not yet exist and will be created.
    pub folder_creations: Vec<String>,
    /// Source folders that will become empty (all their files are being moved out) and will be removed.
    pub folder_deletions: Vec<String>,
}
