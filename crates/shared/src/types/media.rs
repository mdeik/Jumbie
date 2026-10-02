use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Single source of truth for media info metadata attached to an episode file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct MediaInfo {
    pub codec: Option<String>,
    pub resolution: Option<String>,
    pub bitrate: Option<String>,
    pub duration: Option<String>,
    pub audio: Option<String>,
    pub subtitles: Option<String>,
    #[serde(default)]
    pub has_chapters: bool,
    #[serde(default)]
    pub audio_track_count: u32,
    #[serde(default)]
    pub subtitle_track_count: u32,
    #[serde(default)]
    pub video_track_count: u32,
    #[serde(default)]
    pub audio_languages: Vec<String>,
    #[serde(default)]
    pub subtitle_languages: Vec<String>,
    #[serde(default)]
    pub video_languages: Vec<String>,
    #[serde(default)]
    pub audio_channels: Vec<u32>,
    #[serde(default)]
    pub width: u32,
    #[serde(default)]
    pub height: u32,
}

impl MediaInfo {
    /// Convert media info into template variable key-value pairs.
    ///
    /// SSoT for which `MediaInfo` fields map to which template variable names;
    /// adding a `MediaInfo` field means adding its key here. Every key is inserted
    /// (even when empty) so conditional blocks (`?{ ... }`) suppress surrounding
    /// content when the value is absent.
    pub fn to_template_vars(&self) -> HashMap<String, String> {
        let mut vars = HashMap::new();

        vars.insert(
            "codec".to_string(),
            self.codec.as_deref().unwrap_or("").to_string(),
        );
        vars.insert(
            "resolution".to_string(),
            self.resolution.as_deref().unwrap_or("").to_string(),
        );
        vars.insert(
            "bitrate".to_string(),
            self.bitrate.as_deref().unwrap_or("").to_string(),
        );
        vars.insert(
            "duration".to_string(),
            self.duration.as_deref().unwrap_or("").to_string(),
        );
        vars.insert(
            "audio_codec".to_string(),
            self.audio.as_deref().unwrap_or("").to_string(),
        );
        vars.insert(
            "width".to_string(),
            if self.width > 0 {
                self.width.to_string()
            } else {
                String::new()
            },
        );
        vars.insert(
            "height".to_string(),
            if self.height > 0 {
                self.height.to_string()
            } else {
                String::new()
            },
        );
        vars.insert(
            "audio_channels".to_string(),
            if !self.audio_channels.is_empty() {
                self.audio_channels
                    .iter()
                    .map(|c| c.to_string())
                    .collect::<Vec<_>>()
                    .join(",")
            } else {
                String::new()
            },
        );
        vars.insert(
            "audio_languages".to_string(),
            self.audio_languages.join(","),
        );
        vars.insert(
            "subtitle_languages".to_string(),
            self.subtitle_languages.join(","),
        );
        vars.insert(
            "video_track_count".to_string(),
            if self.video_track_count > 0 {
                self.video_track_count.to_string()
            } else {
                String::new()
            },
        );
        vars.insert(
            "audio_track_count".to_string(),
            if self.audio_track_count > 0 {
                self.audio_track_count.to_string()
            } else {
                String::new()
            },
        );
        vars.insert(
            "subtitle_track_count".to_string(),
            if self.subtitle_track_count > 0 {
                self.subtitle_track_count.to_string()
            } else {
                String::new()
            },
        );
        vars.insert(
            "has_chapters".to_string(),
            if self.has_chapters {
                "yes".to_string()
            } else {
                String::new()
            },
        );

        vars
    }
}

/// Metadata for one part of a multi-part episode (e.g. -part-1.mkv, -cd2.mkv).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EpisodePartInfo {
    pub part_number: u32,
    pub file_path: String,
    pub size: u64,
    pub fingerprint: Option<String>,
    /// First path this part's own content was ever seen at. Each part is its own
    /// file with its own fingerprint, so each has its own origin.
    #[serde(default)]
    pub original_path: Option<String>,
    pub media_info: Option<MediaInfo>,
}

/// One raw entry from a media source (RSS feed, indexer, tracker, etc.).
///
/// # Date handling
/// `published` must be **UTC** — the backend stores UTC and converts only at
/// display. Plugin authors should output RFC 3339 strings or
/// `chrono::DateTime<Utc>`, never naive/local datetimes. Empty strings are not
/// valid; use `null` for unknown dates.
///
/// The source plugin instance id is not stored on this struct — it is passed through
/// the processing pipeline so source plugins stay deployment-agnostic.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MediaEntry {
    pub title: String,
    pub guid: Option<String>,
    pub link: Option<String>,
    pub published: Option<chrono::DateTime<chrono::Utc>>,
    pub download_url: Option<String>,
    /// Unique download identifier provided by the source plugin
    /// (e.g. Nyaa's infoHash, or a BTIH hash parsed from a magnet link).
    /// Required — without it the download can't be tracked in the client.
    #[serde(alias = "info_hash")]
    pub download_id: Option<String>,
    pub size: Option<u64>,
    pub description: Option<String>,
    #[serde(default)]
    pub file_list: Vec<String>,
    pub seeders: Option<u32>,
    pub leechers: Option<u32>,
    pub category: Option<String>,
    /// Human-readable source plugin name (e.g. "nyaa", "basic_rss").
    /// Required so the frontend can show a source badge.
    pub source: String,
    /// Optional submitter/release-group from the source plugin, set in
    /// `parse_entries` (from an uploader field or `resolve_submitter()`). When
    /// `None`, downstream treats it as "unknown" and never re-derives from the title.
    #[serde(default)]
    pub submitter: Option<String>,
}

impl MediaEntry {
    pub fn to_search_result(&self, score: i32, is_season_pack: bool) -> SearchResult {
        SearchResult {
            title: self.title.clone(),
            size: self.size.unwrap_or(0),
            seeders: self.seeders,
            leechers: self.leechers,
            link: self.link.clone(),
            source: self.source.clone(),
            score,
            is_season_pack,
            download_id: self.download_id.clone(),
            release_checks: Vec::new(),
            queue_action: String::new(),
            submitter: self.submitter.clone(),
            published: self.published,
        }
    }

    /// Deserialize a plugin JSON-RPC response into entries. Plugins provide
    /// `submitter` on each entry; use `resolve_submitter()` to derive it from the title.
    pub fn from_plugin_response(val: serde_json::Value) -> Result<Vec<Self>, serde_json::Error> {
        serde_json::from_value(val)
    }

    /// Helper for source plugins: derive the submitter from the title if not already
    /// set. Call this in `parse_entries` when the source has no uploader field.
    ///
    /// ```ignore
    /// let mut entry = MediaEntry { title: "...", .. };
    /// entry.resolve_submitter();
    /// ```
    pub fn resolve_submitter(&mut self) {
        if self.submitter.is_none() {
            self.submitter = crate::parsing::extract_submitter(&self.title);
        }
    }

    pub fn scoring_fields(&self) -> (&str, u64, u32, Option<chrono::DateTime<chrono::Utc>>) {
        (
            &self.title,
            self.size.unwrap_or(0),
            self.seeders.unwrap_or(0),
            self.published,
        )
    }
}

/// Stable identities for the gates in a [`ReleaseCheck`]. Serialized in
/// display order — `Rejected`, `Episode`, `Profile`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseCheckId {
    /// The release is on the reject list after a stalled download.
    Rejected,
    /// The release covers the episode the user searched for.
    Episode,
    /// The release satisfies the series' quality and release profiles.
    Profile,
}

impl ReleaseCheckId {
    /// Display heading for the check. Single source for the section titles.
    pub fn label(self) -> &'static str {
        match self {
            ReleaseCheckId::Rejected => "Rejected",
            ReleaseCheckId::Episode => "Episode",
            ReleaseCheckId::Profile => "Profile",
        }
    }
}

/// One acceptance gate auto-search applies to a release, surfaced on manual
/// search results so the user can see what auto-search would do. Manual search
/// never hides results — these are informational suggestions only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseCheck {
    pub id: ReleaseCheckId,
    /// Section heading, derived from `id` so it cannot drift.
    pub label: String,
    /// Short reason shown under the heading. Empty while the check passes.
    #[serde(default)]
    pub description: String,
    /// Whether the release passed. Only failures are shown; when every check
    /// passes the UI reports success in a single message.
    pub passed: bool,
}

impl ReleaseCheck {
    /// A passing check (no description is shown for these).
    pub fn passed(id: ReleaseCheckId) -> Self {
        Self {
            id,
            label: id.label().to_string(),
            description: String::new(),
            passed: true,
        }
    }

    /// A failing check carrying the reason shown under the heading.
    pub fn failed(id: ReleaseCheckId, description: impl Into<String>) -> Self {
        Self {
            id,
            label: id.label().to_string(),
            description: description.into(),
            passed: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub title: String,
    pub size: u64,
    pub seeders: Option<u32>,
    pub leechers: Option<u32>,
    pub link: Option<String>,
    pub source: String,
    #[serde(default)]
    pub score: i32,
    /// Source feed publish date. Carried to the download queue so the age-based
    /// scoring component (`age_score_per_day`) can be reconstructed on rescore.
    #[serde(default)]
    pub published: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    pub is_season_pack: bool,
    /// Unique download identifier from the source plugin (e.g. Nyaa's infoHash or a
    /// BTIH hash). Required to track the download in the client.
    #[serde(default, alias = "info_hash")]
    pub download_id: Option<String>,
    /// Submitter / release group (e.g. "SubsGroup", "iAHD"), populated by
    /// `to_search_result` from `MediaEntry.submitter` or extracted from the title.
    #[serde(default)]
    pub submitter: Option<String>,
    /// Auto-search acceptance gates evaluated for this result, populated by the
    /// backend during manual search (SSoT: auto-search applies the same gates).
    /// Empty = no checks applied (e.g. auto-search results).
    #[serde(default)]
    pub release_checks: Vec<ReleaseCheck>,
    /// Queue action taken for this result in auto-search modes: one of "", "added",
    /// "replaced", "skipped", "merged". Empty means no auto-queue action (manual
    /// search). Frontend uses this to choose the toast notification.
    #[serde(default)]
    pub queue_action: String,
}

/// Canonical ranking key for a release. Ordered ascending by `score`, then
/// `published`, then `seeders`; callers sort descending (see
/// [`compare_release_rank_desc`]). A missing date sorts below any real date, so an
/// undated release never outranks a dated one of equal score.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ReleaseRank {
    pub score: i32,
    pub published: Option<chrono::DateTime<chrono::Utc>>,
    pub seeders: u32,
}

/// Canonical ranking keys, in priority order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseRankKey {
    Score,
    Date,
    Seeders,
}

/// The full canonical tie-break order: score, then date, then seeders.
pub const RELEASE_RANK_KEYS: [ReleaseRankKey; 3] = [
    ReleaseRankKey::Score,
    ReleaseRankKey::Date,
    ReleaseRankKey::Seeders,
];

/// Descending comparison over `keys`, in the given order. Each key is consulted
/// only while the previous ones tie, so callers pass exactly the keys they still
/// need — e.g. the keys a primary sort did not already apply (the frontend column
/// sort) or the keys below a score-level preference (the season-pack strategy).
pub fn compare_release_keys_desc(
    a: ReleaseRank,
    b: ReleaseRank,
    keys: &[ReleaseRankKey],
) -> std::cmp::Ordering {
    let mut ord = std::cmp::Ordering::Equal;
    for key in keys {
        if ord != std::cmp::Ordering::Equal {
            break;
        }
        ord = match key {
            ReleaseRankKey::Score => b.score.cmp(&a.score),
            ReleaseRankKey::Date => b.published.cmp(&a.published),
            ReleaseRankKey::Seeders => b.seeders.cmp(&a.seeders),
        };
    }
    ord
}

/// Single source of truth for release ordering: **score ↓ → date ↓ → seeders ↓**.
/// Used by the manual search list, episode auto-search, and scheduler winner
/// selection so ties resolve identically and deterministically.
pub fn compare_release_rank_desc(a: ReleaseRank, b: ReleaseRank) -> std::cmp::Ordering {
    compare_release_keys_desc(a, b, &RELEASE_RANK_KEYS)
}

/// Tiebreak that favors multi-releases (season packs / episode ranges) over individual
/// episodes when `strategy` is `FavorSeasonPacks`, and the reverse for `FavorEpisodes`.
/// Equal when both releases are multi or both are individual. Inserted after score so
/// the season-pack strategy resolves equal-score ties identically in search and in the
/// source queue.
pub fn compare_multi_preference(
    a_is_multi: bool,
    b_is_multi: bool,
    strategy: &crate::config::SeasonPackStrategy,
) -> std::cmp::Ordering {
    let ord = match (a_is_multi, b_is_multi) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => std::cmp::Ordering::Equal,
    };
    match strategy {
        crate::config::SeasonPackStrategy::FavorSeasonPacks => ord,
        crate::config::SeasonPackStrategy::FavorEpisodes => ord.reverse(),
    }
}

impl SearchResult {
    /// Canonical ranking key for this result.
    pub fn rank(&self) -> ReleaseRank {
        ReleaseRank {
            score: self.score,
            published: self.published,
            seeders: self.seeders.unwrap_or(0),
        }
    }
}

#[cfg(test)]
mod rank_tests {
    use super::*;

    fn rank(score: i32, date: Option<&str>, seeders: u32) -> ReleaseRank {
        ReleaseRank {
            score,
            published: date.map(|d| d.parse::<chrono::DateTime<chrono::Utc>>().unwrap()),
            seeders,
        }
    }

    fn sorted(mut v: Vec<ReleaseRank>) -> Vec<ReleaseRank> {
        v.sort_by(|a, b| compare_release_rank_desc(*a, *b));
        v
    }

    #[test]
    fn orders_by_score_then_date_then_seeders() {
        let top = rank(200, Some("2020-01-01T00:00:00Z"), 0);
        let new_more = rank(100, Some("2024-01-01T00:00:00Z"), 50);
        let new_less = rank(100, Some("2024-01-01T00:00:00Z"), 10);
        let old_many = rank(100, Some("2023-01-01T00:00:00Z"), 900);
        let low = rank(50, Some("2030-01-01T00:00:00Z"), 9999);

        let order = sorted(vec![low, old_many, new_less, top, new_more]);
        assert_eq!(order, vec![top, new_more, new_less, old_many, low]);
    }

    #[test]
    fn undated_release_ranks_below_dated_of_equal_score() {
        let dated = rank(100, Some("2000-01-01T00:00:00Z"), 0);
        let undated = rank(100, None, 999);
        assert_eq!(sorted(vec![undated, dated]), vec![dated, undated]);
    }

    #[test]
    fn equal_ranks_compare_equal() {
        let a = rank(10, Some("2024-01-01T00:00:00Z"), 5);
        assert_eq!(compare_release_rank_desc(a, a), std::cmp::Ordering::Equal);
    }

    /// A key subset ignores earlier keys entirely: date+seeders only, so a much
    /// lower score must not influence the result.
    #[test]
    fn key_subset_skips_earlier_keys() {
        let low_score_newer = rank(1, Some("2024-01-01T00:00:00Z"), 0);
        let high_score_older = rank(999, Some("2020-01-01T00:00:00Z"), 0);
        assert_eq!(
            compare_release_keys_desc(low_score_newer, high_score_older, &RELEASE_RANK_KEYS[1..]),
            std::cmp::Ordering::Less,
        );
    }

    #[test]
    fn multi_preference_follows_strategy() {
        use crate::config::SeasonPackStrategy;
        use std::cmp::Ordering;

        // FavorSeasonPacks: the multi-release sorts first.
        assert_eq!(
            compare_multi_preference(true, false, &SeasonPackStrategy::FavorSeasonPacks),
            Ordering::Less
        );
        // FavorEpisodes: the individual episode sorts first.
        assert_eq!(
            compare_multi_preference(true, false, &SeasonPackStrategy::FavorEpisodes),
            Ordering::Greater
        );
        // Same kind on both sides → no preference either way.
        assert_eq!(
            compare_multi_preference(true, true, &SeasonPackStrategy::FavorEpisodes),
            Ordering::Equal
        );
        assert_eq!(
            compare_multi_preference(false, false, &SeasonPackStrategy::FavorSeasonPacks),
            Ordering::Equal
        );
    }
}
