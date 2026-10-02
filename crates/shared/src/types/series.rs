use crate::mapping::{DEFAULT_MONITOR_MODE, MappingRule, MonitorMode, SeriesSettings};
use crate::types::{CompletionStatus, EpisodeViewModel};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeriesInfo {
    pub id: String,
    pub title: String,
    pub seasons: Vec<String>,
    /// Number of distinct seasons — sent directly from the backend so the frontend
    /// can sort by it without reverse-engineering the formatted `seasons` display strings.
    #[serde(default)]
    pub season_count: i32,
    pub release_profile: String,
    pub quality_profile: String,
    pub episodes_counts: (i32, i32), // (downloaded, total)
    #[serde(default)]
    pub monitored_missing_count: i32,
    /// Episodes currently in the download queue (in-flight: Queued/Downloading),
    /// regardless of the `monitored` flag. The library shows these as yellow
    /// (in-progress) instead of red (missing). Unlike `monitored_missing_count`,
    /// this is sourced from the download queue, so an unmonitored episode that is
    /// downloading still lights it up.
    #[serde(default)]
    pub queued_count: i32,
    pub size: u64, // in bytes
    /// The stored/organized path of the series, if known.
    /// Empty string means the path is derived from `destination_root / title`.
    #[serde(default)]
    pub path: String,
    /// How many entries are currently queued or actively scanning in the media
    /// info scan queue for this series.
    #[serde(default)]
    pub scan_queue_count: usize,
    /// Whether this series uses absolute numbering mode
    #[serde(default)]
    pub absolute_numbering: bool,
    /// Search aliases for this series. When set, the frontend search also matches
    /// against these, making the series findable by alternate names.
    #[serde(default)]
    pub aliases: Vec<String>,
    /// Whether any episodes in this series have a `file_path` pointing to a
    /// file that no longer exists on disk. The frontend shows a warning icon
    /// in the library when this is `true`.
    #[serde(default)]
    pub has_not_found_files: bool,
}

/// A season record persisted from a metadata sync — survives episode/season deletion.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct MetadataSeasonInfo {
    pub season_number: i32,
    pub title: Option<String>,
    pub episode_count: i32,
    pub premiere_date: Option<String>,
    pub end_date: Option<String>,
    pub image_url: Option<String>,
    pub summary: Option<String>,
    /// The provider INSTANCE id that produced this season's metadata (a UUID, the
    /// key of the series mapping's `metadata_ids`). Used so the frontend can label
    /// the "Match" button with the correct provider.
    #[serde(default)]
    pub provider_instance_id: String,
    /// True when the episode count was sourced from the *opposite* ordering mode
    /// (e.g. absolute series fell back to normal-mode metadata because no absolute
    /// metadata has been fetched yet). The frontend shows a visual indicator.
    #[serde(default)]
    pub is_fallback_mode: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeriesDetails {
    pub info: SeriesInfo,
    pub config: MappingRule,
    pub episodes: Vec<EpisodeViewModel>,
    /// Metadata season data stored independently in the DB — present when the show
    /// has ever been synced with a metadata provider. Empty when no sync has happened.
    #[serde(default)]
    pub metadata_seasons: Vec<MetadataSeasonInfo>,
    /// Seasons the user deleted (durable suppression). They are hidden from
    /// `episodes` / `metadata_seasons`, and listed here so the UI can offer a
    /// restore when cached metadata is available.
    #[serde(default)]
    pub suppressed_seasons: Vec<SuppressedSeasonInfo>,
}

/// A season the user deleted (durable suppression).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuppressedSeasonInfo {
    pub season: i32,
    /// Whether the provider's episode cache can restore this season without an
    /// API call. Drives the Restore button's visibility.
    pub cache_available: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "backend", derive(sqlx::FromRow))]
pub struct AutomaticProfile {
    pub submitter: String,
    pub score: i32,
    #[serde(default)]
    pub media_scan_count: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "backend", derive(sqlx::FromRow))]
pub struct AutomaticProfileRecord {
    pub id: String,
    pub submitter: String,
    pub category: String,
    #[serde(alias = "penalty")]
    pub score: i32,
    pub description: String,
    pub date_added: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extension: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub enum PathOperation {
    #[default]
    Move,
    Copy,
    Delete,
    DoNothing,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreviewSeriesItem {
    pub path: String,
    pub original_folder_name: String,
    pub final_title: String,
    pub season_count: usize,
    pub episode_count: usize,
    pub selected: bool,
    pub already_exists: bool,
    /// True when all video files reside directly in the series root folder
    /// (no season sub-directories).  The UI can display this as a hint
    /// and the import flow auto-enables `flatten_season_folders` for such items.
    #[serde(default)]
    pub all_files_in_root: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreviewSeriesRequest {
    pub path: String,
    pub is_bulk: bool,
}

/// Series-level settings that can be supplied when adding a series.
///
/// This is the create-time counterpart of [`SeriesSettings`]: it mirrors every
/// per-series option the edit page can configure (aliases, absolute numbering,
/// search flags, format overrides, rename/flatten toggles) while deliberately
/// excluding **season overrides** (`season` / `season_absolute`) — those are
/// managed through the season endpoints once the series exists — and internal
/// bookkeeping (`path`, `metadata_last_synced_at`, `last_known_dir_mtimes`).
/// The `path`, `monitor_mode`, and `metadata_ids` inputs stay on
/// [`CreateSeriesRequest`] itself.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CreateSeriesSettings {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reg_patterns: Vec<String>,
    /// Tri-state numbering mode: `Some(true)` = absolute, `Some(false)` = normal,
    /// `None` = inherit the global `general.absolute_numbering` default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub absolute_numbering: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub season_folder_format: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub episode_file_format: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub season_folder_format_absolute: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub episode_file_format_absolute: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flatten_season_folders: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rename_episodes: Option<bool>,
    /// Per-series search-key template for normal numbering; omitted = global default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search_format: Option<String>,
    /// Per-series search-key template for absolute numbering; omitted = global default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search_format_absolute: Option<String>,
}

impl From<CreateSeriesSettings> for SeriesSettings {
    /// Seed a full [`SeriesSettings`] from the create-time subset. Season maps,
    /// `path`, `monitor_mode`, `metadata_ids`, and internal bookkeeping are left at
    /// their defaults for the caller to fill in.
    fn from(s: CreateSeriesSettings) -> Self {
        SeriesSettings {
            aliases: s.aliases,
            reg_patterns: s.reg_patterns,
            absolute_numbering: s.absolute_numbering,
            season_folder_format: s.season_folder_format,
            episode_file_format: s.episode_file_format,
            season_folder_format_absolute: s.season_folder_format_absolute,
            episode_file_format_absolute: s.episode_file_format_absolute,
            flatten_season_folders: s.flatten_season_folders,
            rename_episodes: s.rename_episodes,
            search_format: s.search_format,
            search_format_absolute: s.search_format_absolute,
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateSeriesRequest {
    pub path: String,
    pub series_name: Option<String>,
    pub scan_for_existing: Option<bool>,
    pub monitor_mode: Option<MonitorMode>,
    pub quality_profile: Option<String>,
    pub release_profile: Option<String>,
    #[serde(default)]
    pub metadata_ids: HashMap<String, String>,
    /// When true, after the series is created and metadata synced, the server
    /// will auto-search for all monitored+missing episodes and queue them for
    /// download.  Only meaningful when at least one metadata provider is active
    /// and has an ID entered.
    #[serde(default)]
    pub search_missing_on_add: bool,
    /// When true (Add Series form with a destination-root-derived path), the
    /// backend sanitizes the folder name per the illegal-char policy and resolves
    /// folder collisions per the collision config (rename → suffixed folder,
    /// skip → rejected). Explicit custom paths send false so they are honored
    /// verbatim, matching the validate-path preview.
    #[serde(default = "crate::types::payloads::default_true")]
    pub resolve_collisions: bool,
    /// Series-level settings (aliases, absolute numbering, search flags, format
    /// overrides, rename/flatten toggles). Season overrides are intentionally not
    /// accepted at creation — add the series first, then configure seasons via the
    /// season endpoints.
    #[serde(flatten)]
    pub settings: CreateSeriesSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfirmSeriesImportRequest {
    pub items: Vec<PreviewSeriesItem>,
    pub scan_for_existing: bool,
    pub monitor_mode: Option<MonitorMode>,
    pub quality_profile: Option<String>,
    pub release_profile: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateSeriesResponse {
    #[serde(default)]
    pub mode_switch_warning: bool,
    #[serde(default)]
    pub mode_switch_message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UpdateSeriesPayload {
    pub quality_profile: String,
    pub release_profile: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub path_operation: Option<PathOperation>,

    #[serde(flatten)]
    pub settings: SeriesSettings,
}

impl From<&SeriesDetails> for UpdateSeriesPayload {
    fn from(details: &SeriesDetails) -> Self {
        let mut settings = details.config.settings.clone();
        if settings.monitor_mode.is_none() {
            settings.monitor_mode = Some(DEFAULT_MONITOR_MODE);
        }
        if settings.path.is_none() {
            settings.path = Some(String::new());
        }
        settings.metadata_last_synced_at = std::collections::HashMap::new();
        Self {
            quality_profile: details.info.quality_profile.clone(),
            release_profile: details.info.release_profile.clone(),
            title: Some(details.info.title.clone()),
            path_operation: None,
            settings,
        }
    }
}

/// Decisions for the remove-series endpoint.
/// When **neither** flag is set, the series is simply hidden (`hidden_in_library = true`)
/// and all data is preserved — unhiding later restores everything.
///
/// - `delete_configurations` removes series settings, season settings, and episode mappings
///   from the DB (but keeps episode metadata and files).
/// - `delete_episode_data` removes episode data (metadata, media scan fingerprints) from
///   the DB but keeps the files on disk.
/// - `delete_episodes` removes episode data **and** deletes the episode files from disk.
///   When this flag is set, `delete_episode_data` is implicitly true.
/// - Setting `delete_configurations` + `delete_episodes` is equivalent to the legacy
///   `delete_files = true` full-removal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoveSeriesPayload {
    #[serde(default)]
    pub delete_configurations: bool,
    #[serde(default)]
    pub delete_episode_data: bool,
    #[serde(default)]
    pub delete_episodes: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchRemoveSeriesPayload {
    pub series_ids: Vec<String>,
    #[serde(default)]
    pub delete_configurations: bool,
    #[serde(default)]
    pub delete_episode_data: bool,
    #[serde(default)]
    pub delete_episodes: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchEditSeriesPayload {
    pub series_ids: Vec<String>,
    pub quality_profile: Option<String>,
    pub release_profile: Option<String>,
    pub monitor_mode: Option<MonitorMode>,
}

/// Request to batch-upsert seasons for a series.
///
/// Deliberately minimal — only the fields needed to add/update seasons.
/// Uses the existing `update_series` DB write path internally, avoiding
/// a separate dual-write concern.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchUpsertSeasonsRequest {
    pub seasons: Vec<i32>,
    pub episode_count: i32,
    pub absolute_numbering: bool,
}

/// Organized/expected episode counts for a single season of a series. Used to
/// break a series' completion down by season in the Manage Folders UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeasonEpisodeCount {
    pub season: i32,
    /// Episodes in the season that own a file.
    pub organized: i32,
    /// Episodes the season is expected to contain.
    pub expected: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrganizedSeriesItem {
    pub folder_name: String,
    pub absolute_path: String,
    pub is_tracked: bool,
    pub is_monitored: bool,
    pub monitor_mode: Option<MonitorMode>,
    pub series_id: Option<String>,
    pub hidden_in_library: bool,
    #[serde(default)]
    pub selected: bool,
    /// Completion status computed from episode counts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_status: Option<CompletionStatus>,
    /// Total episodes expected for this series (from season config + metadata).
    #[serde(default)]
    pub total_episodes_expected: i32,
    /// Total episodes that have a file_path on disk.
    #[serde(default)]
    pub total_episodes_organized: i32,
    /// Per-season episode counts, season-ascending. Empty for untracked folders.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub season_counts: Vec<SeasonEpisodeCount>,
    /// Whether this series is currently locked by an in-progress modification
    /// (batch move, reorganize, path update, delete, etc). Frontend should
    /// disable edit/delete/reorganize actions when this is true.
    #[serde(default)]
    pub locked: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchEditOrganizedSeriesPayload {
    pub paths: Vec<String>,
    pub operation: String,
    /// When `true`, the "delete" operation removes DB data AND deletes files
    /// from disk. When `false`, only DB data is removed. Ignored for non-delete
    /// operations.
    #[serde(default)]
    pub delete_files: bool,
}

/// Payload for bulk episode monitor-mode changes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonitorEpisodesPayload {
    pub mode: MonitorMode,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToggleMonitorPayload {
    pub monitor_mode: MonitorMode,
}

/// Payload for toggling a single episode's monitored flag (PUT
/// /api/episodes/{id}/monitor). A direct toggle, not a mode re-evaluation: the flag is
/// preserved through periodic sweeps (fresh_apply=false) but reset when the user
/// changes the series' monitor mode.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpisodeMonitorTogglePayload {
    pub monitored: bool,
}

/// Payload for batch-setting the monitored flag (POST /api/episodes/batch-monitor).
/// Generic batch write — the frontend decides which IDs to target.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchMonitorEpisodesPayload {
    pub ids: Vec<String>,
    pub monitored: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToggleVisibilityPayload {
    pub hidden_in_library: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ValidatePathPayload {
    pub path: String,
    /// Optional series ID to exclude from path collision checks.
    /// When set, the validation will allow this series' own path without
    /// flagging it as a collision. This is used by the Edit Series Path
    /// modal so that saving the same path back does not trigger a false
    /// collision error.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub series_id: Option<String>,
    /// When true (Add Series form with a destination-root-derived path), the
    /// backend resolves folder-level collisions per the org config
    /// (rename → suffixed path, skip → invalid) and returns the effective
    /// path in `ValidatePathResponse.resolved_path`.
    #[serde(default)]
    pub resolve_collisions: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidatePathResponse {
    pub is_valid: bool,
    pub message: String,
    /// The effective path the series will use after collision resolution
    /// (create flow only). `None` when no resolution was performed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReorganizeSeriesPayload {
    pub target_absolute: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReorganizeAllPayload {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssignFilePayload {
    pub path: String,
    pub season: String,
    /// Episode number or range, e.g. "5" or "1-3"
    pub episode: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SeriesFileViewModel {
    /// Relative to the series root when organized, else the download filename.
    pub path: String,
    pub filename: String,
    pub size: u64,
    pub assigned_id: Option<String>,     // If mapped to an episode
    pub assigned_header: Option<String>, // e.g. "S01E01"
    /// `video` for an episode's playable file, `subtitle`/`nfo` for a sidecar.
    #[serde(default)]
    pub kind: crate::media_format::FileKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchDeletePayload {
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchAssignPayload {
    pub paths: Vec<String>,
    pub start_season: String,
    pub start_episode: i32,
    #[serde(default)]
    pub is_multipart: bool,
}

/// Payload for the batch-move organized series endpoint.
///
/// The server **always** appends the series folder name to target_root
/// (e.g. target_root=/media/tv + series "My Show" → /media/tv/My Show).
/// This prevents API callers from accidentally dumping all series into
/// a single flat directory. Use `custom_paths` for per-series overrides
/// that ignore both the root and the folder-name append.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchMoveOrganizedSeriesPayload {
    /// Source paths of the series to move (as displayed in OrganizedSeriesItem).
    pub paths: Vec<String>,
    /// Destination root directory. The series folder name is appended to this.
    pub target_root: String,
    /// Per-series custom path overrides. Keyed by source path.
    /// When a path is in this map, target_root + folder-name append is ignored.
    #[serde(default)]
    pub custom_paths: std::collections::HashMap<String, String>,
    /// How to handle existing files at the destination.
    /// - Move: rename directory contents to new location, delete source
    /// - Copy: copy directory contents to new location, keep source
    /// - Delete: update DB path, delete source directory
    /// - DoNothing: only update DB path, leave files in place
    #[serde(default)]
    pub file_operation: PathOperation,
    /// When true, after the file operation completes, all DB data for the series
    /// (configurations, episode data, metadata cache) is deleted — effectively
    /// removing the series from tracking. The database deletion uses the same
    /// `delete_series_data` + `delete_series_mapping` + `cleanup_orphaned_metadata`
    /// sequence as `batch_edit_organized_series` with the "delete" operation.
    #[serde(default)]
    pub stop_tracking: bool,
}

/// Preview response for a batch-move request.
/// Returns a list of (source → destination) mappings so the user can
/// confirm before executing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchMovePreviewItem {
    pub source_path: String,
    pub destination_path: String,
    pub folder_name: String,
    pub series_id: Option<String>,
    pub is_tracked: bool,
    /// Set when the destination path is already claimed by a different series.
    pub collision: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchMovePreviewResponse {
    pub items: Vec<BatchMovePreviewItem>,
    pub has_collisions: bool,
}

/// Result of executing a batch move.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchMoveResult {
    pub source_path: String,
    pub destination_path: String,
    pub success: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchMoveResponse {
    pub results: Vec<BatchMoveResult>,
    pub success_count: usize,
    pub failure_count: usize,
    /// When the batch move runs asynchronously, this task_id can be polled
    /// via GET /api/system/organized_series/batch_move/{task_id}/status
    /// to track progress. `None` for synchronous responses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::CreateSeriesSettings;
    use crate::mapping::{MonitorMode, SeasonOverride, SeriesSettings};
    use std::collections::{BTreeSet, HashMap};

    /// `CreateSeriesSettings` must accept every series-level setting except the
    /// deliberately excluded ones.
    ///
    /// This serializes a fully-populated [`SeriesSettings`], subtracts the
    /// excluded/relocated/internal fields, and requires the create-time DTO to
    /// expose exactly the remainder. Adding a new series setting therefore fails
    /// this test until it is either wired into the create payload or consciously
    /// added to the exclusion list below — keeping `SeriesSettings` the single
    /// source of truth without a second hard-coded field list silently drifting.
    #[test]
    fn create_settings_covers_all_series_settings() {
        let mut season = HashMap::new();
        season.insert(
            "1".to_string(),
            SeasonOverride {
                season: "1".to_string(),
                search_format: Some("S${episode}".to_string()),
                ..Default::default()
            },
        );

        let full = SeriesSettings {
            aliases: vec!["alias".to_string()],
            absolute_numbering: Some(true),
            season: season.clone(),
            season_absolute: season,
            reg_patterns: vec!["pattern".to_string()],
            season_folder_format: Some("Season {season}".to_string()),
            episode_file_format: Some("{series} - S{season}E{episode}".to_string()),
            season_folder_format_absolute: Some("Season {season}".to_string()),
            episode_file_format_absolute: Some("{series} - E{episode}".to_string()),
            flatten_season_folders: Some(true),
            path: Some("/media/tv/Show".to_string()),
            monitor_mode: Some(MonitorMode::All),
            metadata_ids: HashMap::from([("provider".to_string(), "id".to_string())]),
            metadata_last_synced_at: HashMap::from([("provider".to_string(), "t".to_string())]),
            rename_episodes: Some(true),
            search_format: Some("S${season:02}E${episode:02}".to_string()),
            search_format_absolute: Some("E${episode:02}".to_string()),
            last_known_dir_mtimes: HashMap::from([(".".to_string(), 1.0)]),
        };

        let keys = |value: serde_json::Value| -> BTreeSet<String> {
            value.as_object().unwrap().keys().cloned().collect()
        };
        let full_keys = keys(serde_json::to_value(&full).unwrap());

        // Season overrides (managed via the season endpoints), the add-specific
        // top-level inputs, and internal bookkeeping are not part of the create DTO.
        let excluded: BTreeSet<String> = [
            "season",
            "season_absolute",
            "path",
            "monitor_mode",
            "metadata_ids",
            "metadata_last_synced_at",
            "last_known_dir_mtimes",
        ]
        .into_iter()
        .map(str::to_string)
        .collect();
        let expected: BTreeSet<String> = full_keys.difference(&excluded).cloned().collect();

        // Populate every field so `skip_serializing_if` doesn't hide any key.
        let create = CreateSeriesSettings {
            aliases: vec!["alias".to_string()],
            reg_patterns: vec!["pattern".to_string()],
            absolute_numbering: Some(true),
            season_folder_format: Some("Season {season}".to_string()),
            episode_file_format: Some("{series} - S{season}E{episode}".to_string()),
            season_folder_format_absolute: Some("Season {season}".to_string()),
            episode_file_format_absolute: Some("{series} - E{episode}".to_string()),
            flatten_season_folders: Some(true),
            rename_episodes: Some(true),
            search_format: Some("S${season:02}E${episode:02}".to_string()),
            search_format_absolute: Some("E${episode:02}".to_string()),
        };
        let create_keys = keys(serde_json::to_value(&create).unwrap());

        assert_eq!(
            create_keys, expected,
            "CreateSeriesSettings is out of sync with SeriesSettings — wire the new field \
             into the create payload or add it to the exclusion list"
        );
    }
}
