pub mod activity;
pub mod dates;
pub mod episode;
pub mod episode_status;
pub mod error;
pub mod log;
pub mod media;
pub mod payloads;
pub mod queue;
pub mod series;

pub use activity::{ActivityItem, ActivityType};
pub use dates::{DateSourceFlags, ReleaseDateSource, ReleaseDates, select_release_date_source};
pub use episode::{
    AuxiliaryFile, CalendarEpisode, CalendarResponse, CompletionStatus, EpisodeViewModel,
    MetadataEpisodeData, MetadataSyncStatus, RetryItem, SyncMetadataRequest,
};
pub use episode_status::EpisodeStatus;
pub use error::ApiErrorEnvelope;
pub use log::{LOG_BUFFER_BYTES, LOG_LEVELS, LogBuffer, LogEntry, LogRing, level_priority};
pub use media::{
    EpisodePartInfo, MediaEntry, MediaInfo, RELEASE_RANK_KEYS, ReleaseCheck, ReleaseCheckId,
    ReleaseRank, ReleaseRankKey, SearchResult, compare_multi_preference, compare_release_keys_desc,
    compare_release_rank_desc,
};
pub use payloads::{
    AddBanPayload, AutoSearchSeasonPayload, BanEntry, BootstrapData, DeleteTorrentPayload,
    DownloadMediaPayload, PluginStatusEntry, PublicThemeResponse, RemediatePayload,
    SavePluginsSectionResponse, SearchPayload, TestPluginPayload, UpdateConfigPayload,
    WantedEpisode,
};
pub use queue::{
    AddDownloadResponse, AddQueueResult, DownloadQueueItem, EpisodeIntention, MultiTarget,
    RenameDetail, RenameQueueItem, RenameQueueItemDetail, RenameQueueResponse,
};
pub use series::{
    AssignFilePayload, AutomaticProfile, AutomaticProfileRecord, BatchAssignPayload,
    BatchDeletePayload, BatchEditOrganizedSeriesPayload, BatchEditSeriesPayload,
    BatchMonitorEpisodesPayload, BatchMoveOrganizedSeriesPayload, BatchMovePreviewItem,
    BatchMovePreviewResponse, BatchMoveResponse, BatchMoveResult, BatchRemoveSeriesPayload,
    BatchUpsertSeasonsRequest, ConfirmSeriesImportRequest, CreateSeriesRequest,
    CreateSeriesSettings, EpisodeMonitorTogglePayload, MetadataSeasonInfo, MonitorEpisodesPayload,
    OrganizedSeriesItem, PathOperation, PreviewSeriesItem, PreviewSeriesRequest,
    RemoveSeriesPayload, ReorganizeAllPayload, ReorganizeSeriesPayload, SeasonEpisodeCount,
    SeriesDetails, SeriesFileViewModel, SeriesInfo, SuppressedSeasonInfo, ToggleMonitorPayload,
    ToggleVisibilityPayload, UpdateSeriesPayload, UpdateSeriesResponse, ValidatePathPayload,
    ValidatePathResponse,
};

// Re-exports from sibling crate modules so callers can use `jumbie_shared::types::*`.
pub use crate::filtering::{FilterRule, MatchMode};
pub use crate::mapping::{
    DEFAULT_MONITOR_MODE, EpisodeInfo, MappingRule, MonitorMode, NumberingMode, SeasonOverride,
    SeriesSettings,
};
pub use crate::scoring::{CustomFormat, Quality, QualityProfile, ReleaseProfile};

use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaginatedResponse<T> {
    pub items: Vec<T>,
    pub total: i64,
    pub page: i64,
    pub page_size: i64,
}

/// Query parameters shared by the paginated system list endpoints (logs,
/// activity, wanted).
///
/// `page`/`limit`/`sort`/`order` are handled uniformly by every endpoint (sort/order
/// fall back to the user's stored preference when omitted); the remaining fields are
/// endpoint-specific filters — an endpoint applies the ones it supports and ignores
/// the rest.
///
/// One struct rather than per-endpoint structs with `#[serde(flatten)]` because
/// axum's `Query` extractor uses `serde_urlencoded`, which cannot deserialize
/// flattened numeric fields (`page`/`limit`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaginationQuery {
    pub page: Option<i64>,
    pub limit: Option<i64>,
    /// Column to sort by. Supported values depend on the endpoint.
    pub sort: Option<String>,
    /// Sort direction: "asc" or "desc". Defaults to the stored preference,
    /// then the endpoint default.
    pub order: Option<String>,
    /// Free-text filter, matched case-insensitively. Supported by the logs
    /// (message), activity (title/details/status) and wanted (series title)
    /// endpoints; other endpoints ignore it.
    pub search: Option<String>,
    /// Endpoint-specific filter: comma-separated lowercase activity type names
    /// (activity endpoint only; other endpoints ignore it).
    pub types: Option<String>,
    /// Endpoint-specific filter: minimum log level, e.g. "INFO" (logs endpoint
    /// only; other endpoints ignore it).
    pub min_level: Option<String>,
}

pub struct EpisodeLogRef<'a, S: fmt::Display> {
    pub title: &'a str,
    pub season: S,
    pub episode: i32,
    pub episode_end: Option<i32>,
}

impl<'a, S: fmt::Display> EpisodeLogRef<'a, S> {
    pub fn new(title: &'a str, season: S, episode: i32, episode_end: Option<i32>) -> Self {
        Self {
            title,
            season,
            episode,
            episode_end,
        }
    }
}

impl<'a, S: fmt::Display> fmt::Display for EpisodeLogRef<'a, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(end) = self.episode_end
            && end > self.episode
        {
            let s: i32 = format!("{}", self.season).parse().unwrap_or(0);
            return write!(
                f,
                "{} ({}-{})",
                self.title,
                crate::formatting::fmt_season_episode(
                    s,
                    self.episode,
                    None,
                    crate::formatting::LabelStyle::Short
                ),
                crate::formatting::fmt_episode(end, crate::formatting::LabelStyle::Short)
            );
        }
        let s: i32 = format!("{}", self.season).parse().unwrap_or(0);
        write!(
            f,
            "{} ({})",
            self.title,
            crate::formatting::fmt_season_episode(
                s,
                self.episode,
                None,
                crate::formatting::LabelStyle::Short
            )
        )
    }
}

/// Canonical list of media-info field names used throughout the application.
///
/// SSoT: every reference to these field names (DB column mapping, file_manager
/// metadata collection, frontend display) uses this constant. Adding or renaming a
/// field is a one-line change; never hardcode the list inline.
pub const MEDIA_INFO_FIELDS: &[&str] = &[
    "codec",
    "resolution",
    "bitrate",
    "duration",
    "audio_codec",
    "width",
    "height",
    "audio_channels",
    "audio_languages",
    "subtitle_languages",
    "video_track_count",
    "audio_track_count",
    "subtitle_track_count",
    "has_chapters",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_to_template_vars_empty_default() {
        let info = MediaInfo::default();
        let vars = info.to_template_vars();

        assert_eq!(vars.len(), 14, "expected 14 template variable keys");

        // All values should be empty strings for a default MediaInfo
        for (key, value) in &vars {
            assert!(
                value.is_empty(),
                "expected empty string for key '{}', got '{}'",
                key,
                value
            );
        }

        // Verify all 14 expected keys exist — sourced from the SSoT constant.
        for key in MEDIA_INFO_FIELDS {
            assert!(vars.contains_key(*key), "missing key '{}'", key);
        }
    }

    #[test]
    fn test_to_template_vars_fully_populated() {
        let info = MediaInfo {
            codec: Some("h265".to_string()),
            resolution: Some("1080p".to_string()),
            bitrate: Some("4500kbps".to_string()),
            duration: Some("1h45m".to_string()),
            audio: Some("AAC".to_string()),
            subtitles: Some("VobSub".to_string()),
            has_chapters: true,
            audio_track_count: 2,
            subtitle_track_count: 3,
            video_track_count: 1,
            audio_languages: vec!["eng".to_string(), "jpn".to_string()],
            subtitle_languages: vec!["eng".to_string(), "spa".to_string(), "fre".to_string()],
            video_languages: vec!["eng".to_string()],
            audio_channels: vec![2, 6],
            width: 1920,
            height: 1080,
        };

        let vars = info.to_template_vars();

        assert_eq!(vars.len(), 14);
        assert_eq!(vars.get("codec").unwrap(), "h265");
        assert_eq!(vars.get("resolution").unwrap(), "1080p");
        assert_eq!(vars.get("bitrate").unwrap(), "4500kbps");
        assert_eq!(vars.get("duration").unwrap(), "1h45m");
        assert_eq!(vars.get("audio_codec").unwrap(), "AAC");
        assert_eq!(vars.get("width").unwrap(), "1920");
        assert_eq!(vars.get("height").unwrap(), "1080");
        assert_eq!(vars.get("audio_channels").unwrap(), "2,6");
        assert_eq!(vars.get("audio_languages").unwrap(), "eng,jpn");
        assert_eq!(vars.get("subtitle_languages").unwrap(), "eng,spa,fre");
        assert_eq!(vars.get("video_track_count").unwrap(), "1");
        assert_eq!(vars.get("audio_track_count").unwrap(), "2");
        assert_eq!(vars.get("subtitle_track_count").unwrap(), "3");
        assert_eq!(vars.get("has_chapters").unwrap(), "yes");
    }

    #[test]
    fn test_to_template_vars_numeric_fields_when_zero() {
        let info = MediaInfo {
            width: 0,
            height: 0,
            video_track_count: 0,
            audio_track_count: 0,
            subtitle_track_count: 0,
            ..MediaInfo::default()
        };

        let vars = info.to_template_vars();

        assert_eq!(vars.get("width").unwrap(), "");
        assert_eq!(vars.get("height").unwrap(), "");
        assert_eq!(vars.get("video_track_count").unwrap(), "");
        assert_eq!(vars.get("audio_track_count").unwrap(), "");
        assert_eq!(vars.get("subtitle_track_count").unwrap(), "");
    }

    #[test]
    fn test_to_template_vars_numeric_fields_when_nonzero() {
        let info = MediaInfo {
            width: 3840,
            height: 2160,
            video_track_count: 2,
            audio_track_count: 1,
            subtitle_track_count: 0,
            ..MediaInfo::default()
        };

        let vars = info.to_template_vars();

        assert_eq!(vars.get("width").unwrap(), "3840");
        assert_eq!(vars.get("height").unwrap(), "2160");
        assert_eq!(vars.get("video_track_count").unwrap(), "2");
        assert_eq!(vars.get("audio_track_count").unwrap(), "1");
        assert_eq!(vars.get("subtitle_track_count").unwrap(), "");
    }

    #[test]
    fn test_to_template_vars_has_chapters() {
        // true -> "yes"
        let info_true = MediaInfo {
            has_chapters: true,
            ..MediaInfo::default()
        };
        assert_eq!(
            info_true.to_template_vars().get("has_chapters").unwrap(),
            "yes"
        );

        // false -> ""
        let info_false = MediaInfo {
            has_chapters: false,
            ..MediaInfo::default()
        };
        assert_eq!(
            info_false.to_template_vars().get("has_chapters").unwrap(),
            ""
        );
    }

    #[test]
    fn test_to_template_vars_vec_fields_join() {
        let info = MediaInfo {
            audio_languages: vec!["eng".to_string(), "deu".to_string(), "kor".to_string()],
            subtitle_languages: vec!["eng".to_string()],
            audio_channels: vec![2],
            ..MediaInfo::default()
        };

        let vars = info.to_template_vars();

        assert_eq!(vars.get("audio_languages").unwrap(), "eng,deu,kor");
        assert_eq!(vars.get("subtitle_languages").unwrap(), "eng");
        assert_eq!(vars.get("audio_channels").unwrap(), "2");
    }

    #[test]
    fn test_to_template_vars_empty_vec_fields() {
        let info = MediaInfo {
            audio_languages: vec![],
            subtitle_languages: vec![],
            audio_channels: vec![],
            ..MediaInfo::default()
        };

        let vars = info.to_template_vars();

        assert_eq!(vars.get("audio_languages").unwrap(), "");
        assert_eq!(vars.get("subtitle_languages").unwrap(), "");
        assert_eq!(vars.get("audio_channels").unwrap(), "");
    }

    #[test]
    fn test_to_template_vars_option_fields_none() {
        let info = MediaInfo {
            codec: None,
            resolution: None,
            bitrate: None,
            duration: None,
            audio: None,
            subtitles: None,
            ..MediaInfo::default()
        };

        let vars = info.to_template_vars();

        assert_eq!(vars.get("codec").unwrap(), "");
        assert_eq!(vars.get("resolution").unwrap(), "");
        assert_eq!(vars.get("bitrate").unwrap(), "");
        assert_eq!(vars.get("duration").unwrap(), "");
        assert_eq!(vars.get("audio_codec").unwrap(), "");
    }

    #[test]
    fn test_to_template_vars_subtitles_not_mapped() {
        // The `subtitles` field on MediaInfo should NOT appear in template vars
        let info = MediaInfo {
            subtitles: Some("VobSub".to_string()),
            ..MediaInfo::default()
        };

        let vars = info.to_template_vars();

        assert!(!vars.contains_key("subtitles"));
        assert_eq!(vars.len(), 14);
    }

    // ── SearchResult / to_search_result ─────────────────────────────────

    #[test]
    fn test_to_search_result_carries_resolved_submitter() {
        // Plugins call resolve_submitter() in their parse_entries to derive
        // submitter from the title. This test verifies the resolved value
        // carries through to_search_result unchanged.
        let mut entry = MediaEntry {
            title: "[SubsGroup] My Show - 01 (1080p) [ABC123]".to_string(),
            size: Some(500_000_000),
            seeders: Some(10),
            leechers: Some(2),
            ..MediaEntry::default()
        };
        entry.resolve_submitter();
        let result = entry.to_search_result(100, false);
        assert_eq!(result.submitter.as_deref(), Some("SubsGroup"));
    }

    #[test]
    fn test_to_search_result_carries_resolved_submitter_from_suffix() {
        let mut entry = MediaEntry {
            title: "My.Show.S01E01.1080p.WEB-DL.x264-GRP".to_string(),
            size: Some(1_000_000_000),
            seeders: Some(5),
            leechers: Some(1),
            ..MediaEntry::default()
        };
        entry.resolve_submitter();
        let result = entry.to_search_result(80, false);
        assert_eq!(result.submitter.as_deref(), Some("GRP"));
    }

    #[test]
    fn test_to_search_result_submitter_none_when_no_match() {
        let entry = MediaEntry {
            title: "Some Show S01E01 NoGroup.mkv".to_string(),
            size: Some(300_000_000),
            seeders: Some(3),
            leechers: Some(0),
            ..MediaEntry::default()
        };
        let result = entry.to_search_result(50, false);
        assert!(result.submitter.is_none());
    }

    #[test]
    fn test_to_search_result_preserves_all_basic_fields() {
        let entry = MediaEntry {
            title: "My Show - 01.mkv".to_string(),
            size: Some(200_000_000),
            seeders: Some(7),
            leechers: Some(3),
            link: Some("https://example.com/download".to_string()),
            download_id: Some("abc123".to_string()),
            ..MediaEntry::default()
        };
        let result = entry.to_search_result(95, true);
        assert_eq!(result.title, "My Show - 01.mkv");
        assert_eq!(result.size, 200_000_000);
        assert_eq!(result.seeders, Some(7));
        assert_eq!(result.leechers, Some(3));
        assert_eq!(result.link.as_deref(), Some("https://example.com/download"));
        assert_eq!(result.source, "");
        assert_eq!(result.score, 95);
        assert!(result.is_season_pack);
        assert_eq!(result.download_id.as_deref(), Some("abc123"));
        assert!(result.release_checks.is_empty());
    }

    #[test]
    fn test_to_search_result_uses_submitter_from_plugin_when_provided() {
        let entry = MediaEntry {
            title: "Some.Show.S01E01.TitleWithoutGroup.mkv".to_string(),
            submitter: Some("KnownGroup".to_string()),
            ..MediaEntry::default()
        };
        let result = entry.to_search_result(100, false);
        // Plugin-provided submitter takes priority over parsing from title.
        // The title doesn't contain "KnownGroup" — it's supplied by the plugin.
        assert_eq!(result.submitter.as_deref(), Some("KnownGroup"));
    }

    #[test]
    fn test_to_search_result_passes_through_none_when_not_resolved() {
        // Without calling resolve_submitter(), submitter is None.
        // This is the expected contract: resolve at plugin boundary, carry through.
        let entry = MediaEntry {
            title: "[SubsGroup] My Show - 01 (1080p) [ABC123]".to_string(),
            submitter: None,
            ..MediaEntry::default()
        };
        let result = entry.to_search_result(100, false);
        assert_eq!(
            result.submitter.as_deref(),
            None,
            "to_search_result no longer falls back — plugin boundary must call resolve_submitter"
        );
    }

    // ── resolve_submitter ────────────────────────────────────────────

    #[test]
    fn test_resolve_submitter_keeps_plugin_value_when_provided() {
        // Scenario A: plugin provides a submitter that differs from the title.
        // resolve_submitter must NOT overwrite it.
        let mut entry = MediaEntry {
            title: "Some.Show.S01E01.TitleWithoutGroup.mkv".to_string(),
            submitter: Some("PluginGroup".to_string()),
            ..MediaEntry::default()
        };
        entry.resolve_submitter();
        assert_eq!(
            entry.submitter.as_deref(),
            Some("PluginGroup"),
            "resolve_submitter must NOT overwrite a plugin-provided submitter"
        );
    }

    #[test]
    fn test_resolve_submitter_parses_from_title_when_missing() {
        // Scenario B: plugin does NOT provide a submitter.
        // resolve_submitter should parse it from the title.
        let mut entry = MediaEntry {
            title: "[SubsGroup] My Show - 01 (1080p) [ABC123]".to_string(),
            submitter: None,
            ..MediaEntry::default()
        };
        entry.resolve_submitter();
        assert_eq!(
            entry.submitter.as_deref(),
            Some("SubsGroup"),
            "resolve_submitter must parse from title when submitter is None"
        );
    }

    #[test]
    fn test_resolve_submitter_leaves_none_when_unresolvable() {
        // Neither plugin provided nor parseable from title → stays None.
        let mut entry = MediaEntry {
            title: "NoGroupHere S01E01.mkv".to_string(),
            submitter: None,
            ..MediaEntry::default()
        };
        entry.resolve_submitter();
        assert!(
            entry.submitter.is_none(),
            "resolve_submitter must leave None when neither plugin nor title has a group"
        );
    }

    #[test]
    fn test_resolve_submitter_carries_through_to_search_result() {
        // After resolve_submitter at the plugin boundary, the resolved value
        // flows through to_search_result unchanged — no re-derivation.
        let mut entry = MediaEntry {
            title: "[SubsGroup] My Show - 01".to_string(),
            submitter: None,
            ..MediaEntry::default()
        };
        entry.resolve_submitter();
        assert_eq!(
            entry.submitter.as_deref(),
            Some("SubsGroup"),
            "resolve_submitter should have parsed SubsGroup"
        );
        let result = entry.to_search_result(100, false);
        assert_eq!(
            result.submitter.as_deref(),
            Some("SubsGroup"),
            "to_search_result carries the resolved value"
        );
    }
}
