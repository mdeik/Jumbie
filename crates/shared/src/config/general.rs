use serde::{Deserialize, Serialize};

use super::automatic::AutomaticProfilesConfig;
use super::plugins::SeasonPackStrategy;
use crate::filtering::FilterRule;

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct GeneralConfig {
    // Defaults true: media info enriches the library, so scanning is opt-out.
    #[serde(default = "crate::config::default_true")]
    pub media_info_scan_enabled: bool,
    // 15 minutes balances data freshness against I/O load.
    #[serde(default = "default_media_info_scan_interval")]
    pub media_info_scan_interval: u64, // Minutes
    #[serde(default)]
    pub default_score_for_manual_files: Option<i32>,
    // Separate sub-config: automatic profiles have their own validation logic and
    // a complex rules/categories model (see automatic.rs).
    #[serde(default)]
    pub automatic_profiles: AutomaticProfilesConfig,
    #[serde(default)]
    pub season_pack_strategy: SeasonPackStrategy,
    // Percentage (0-100) of a pack's episodes that must be needed (missing) for the
    // pack to replace already-downloaded episodes; below it the pack only fills gaps,
    // avoiding I/O for marginal gains. Default 50%.
    #[serde(default = "default_season_pack_replace_threshold")]
    pub season_pack_replace_threshold: u32,
    /// Flat score adjustment added to every season/complete pack during release
    /// scoring. Positive values favor packs over individual episodes, negative
    /// values discourage them. `None` (or 0) leaves scores unchanged.
    #[serde(default)]
    pub season_pack_score_modifier: Option<i32>,
    #[serde(default = "default_unexpected_files_handling")]
    pub unexpected_files_handling: String,
    #[serde(default = "default_unneeded_episodes_handling")]
    pub unneeded_episodes_handling: String,

    /// How many files to scan concurrently when gathering media info.
    /// Default 1 — safe everywhere; increase on multi-core machines with fast storage.
    #[serde(default = "default_media_info_scan_concurrency")]
    pub media_info_scan_concurrency: usize,
    // Series directory scanning: cheap stat() per tracked series per cycle, with a
    // full WalkDir only when mtime changes, so I/O on a quiet library is negligible.
    #[serde(default = "crate::config::default_true")]
    pub series_scan_enabled: bool,
    #[serde(default = "default_series_scan_interval")]
    pub series_scan_interval: u64, // Minutes
    #[serde(default)]
    pub flatten_season_folders: bool,
    #[serde(default)]
    pub absolute_numbering: bool,

    /// Minimum interval (minutes) between automatic metadata fetches for the same
    /// series from the rename queue. Default 60 — balances freshness against API
    /// rate limits; the per-provider `metadata_last_synced_at` is the SSoT.
    #[serde(default = "default_metadata_fetch_cooldown_minutes")]
    pub metadata_fetch_cooldown_minutes: u64,

    /// Auto-search for wanted episodes on a recurring schedule. When enabled,
    /// episodes wanted for at least `auto_search_wanted_min_wait` are searched every
    /// `auto_search_wanted_interval` minutes. Disabled by default — opt-in because
    /// it hits external indexers.
    #[serde(default)]
    pub auto_search_wanted_enabled: bool,
    /// How often (in minutes) to poll for wanted episodes to auto-search.
    #[serde(default = "default_auto_search_wanted_interval")]
    pub auto_search_wanted_interval: u64,
    /// How long (in minutes) an episode must have been wanted before auto-search
    /// will attempt it. 120 gives scene groups time to release after airing.
    #[serde(default = "default_auto_search_wanted_min_wait")]
    pub auto_search_wanted_min_wait: u64,
    /// Maximum age (in days) for wanted episodes to be auto-searched; 0 disables
    /// the limit. 2 days: most episodes appear within 48 hours of airing.
    #[serde(default = "default_auto_search_wanted_max_age_days")]
    pub auto_search_wanted_max_age_days: u64,
}

// Validation bounds (SSoT: shared between frontend and backend)
impl GeneralConfig {
    pub const AUTO_SEARCH_WANTED_INTERVAL_MIN: u64 = 5;
    pub const AUTO_SEARCH_WANTED_INTERVAL_MAX: u64 = 1440; // 24h
    pub const AUTO_SEARCH_WANTED_MIN_WAIT_MIN: u64 = 1;
    pub const AUTO_SEARCH_WANTED_MAX_AGE_DAYS_MAX: u64 = 365;

    // Default values, exposed so the UI can show them as placeholders and apply
    // them when a numeric field is left blank. These are the SSoT for the
    // corresponding `default_*` serde functions below.
    pub const MEDIA_INFO_SCAN_INTERVAL_DEFAULT: u64 = 15;
    pub const MEDIA_INFO_SCAN_CONCURRENCY_DEFAULT: usize = 1;
    /// `media_info_scan_concurrency` value meaning "no concurrency cap".
    pub const MEDIA_INFO_SCAN_CONCURRENCY_UNLIMITED: usize = 0;
    pub const SEASON_PACK_REPLACE_THRESHOLD_DEFAULT: u32 = 50;
    pub const AUTO_SEARCH_WANTED_INTERVAL_DEFAULT: u64 = 60;
    pub const AUTO_SEARCH_WANTED_MIN_WAIT_DEFAULT: u64 = 120;
    pub const AUTO_SEARCH_WANTED_MAX_AGE_DAYS_DEFAULT: u64 = 2;

    /// Effective season-pack score modifier (`None` behaves as 0).
    /// SSoT for how the configured modifier is applied to pack scores.
    pub fn effective_pack_score_modifier(&self) -> i32 {
        self.season_pack_score_modifier.unwrap_or(0)
    }
}

// Custom default functions (not `#[serde(default)]`) because serde's String default
// would be an invalid/incomplete `""`.
fn default_unexpected_files_handling() -> String {
    "delete".to_string()
}

fn default_unneeded_episodes_handling() -> String {
    "delete".to_string()
}

fn default_season_pack_replace_threshold() -> u32 {
    GeneralConfig::SEASON_PACK_REPLACE_THRESHOLD_DEFAULT
}

fn default_media_info_scan_concurrency() -> usize {
    GeneralConfig::MEDIA_INFO_SCAN_CONCURRENCY_DEFAULT
}

// 10 minutes: cheap stat() calls mean manually-added files appear without user
// action, without wasting I/O.
fn default_series_scan_interval() -> u64 {
    10
}

fn default_metadata_fetch_cooldown_minutes() -> u64 {
    60
}

fn default_auto_search_wanted_interval() -> u64 {
    GeneralConfig::AUTO_SEARCH_WANTED_INTERVAL_DEFAULT
}

fn default_auto_search_wanted_min_wait() -> u64 {
    GeneralConfig::AUTO_SEARCH_WANTED_MIN_WAIT_DEFAULT
}

fn default_auto_search_wanted_max_age_days() -> u64 {
    GeneralConfig::AUTO_SEARCH_WANTED_MAX_AGE_DAYS_DEFAULT
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            media_info_scan_enabled: true,
            media_info_scan_interval: default_media_info_scan_interval(),
            default_score_for_manual_files: None,
            automatic_profiles: AutomaticProfilesConfig::default(),
            season_pack_strategy: SeasonPackStrategy::default(),
            season_pack_replace_threshold: default_season_pack_replace_threshold(),
            season_pack_score_modifier: None,
            unexpected_files_handling: default_unexpected_files_handling(),
            unneeded_episodes_handling: default_unneeded_episodes_handling(),

            media_info_scan_concurrency: default_media_info_scan_concurrency(),
            series_scan_enabled: true,
            series_scan_interval: default_series_scan_interval(),
            flatten_season_folders: false,
            absolute_numbering: false,

            metadata_fetch_cooldown_minutes: default_metadata_fetch_cooldown_minutes(),

            auto_search_wanted_enabled: false,
            auto_search_wanted_interval: default_auto_search_wanted_interval(),
            auto_search_wanted_min_wait: default_auto_search_wanted_min_wait(),
            auto_search_wanted_max_age_days: default_auto_search_wanted_max_age_days(),
        }
    }
}

// Placeholder so the settings API can accept and validate source-level config.
// The actual polling interval is per-plugin (`refresh_interval`), the SSoT for
// source sync timing — hence this struct is intentionally empty.
#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct SourcesConfig {}

// 15 minutes: media info scans read actual files, so a slightly longer interval
// than source polling reduces I/O contention.
fn default_media_info_scan_interval() -> u64 {
    GeneralConfig::MEDIA_INFO_SCAN_INTERVAL_DEFAULT
}

// A separate struct because its escalation rules use `FilterRule` from the
// filtering crate: GeneralConfig is "what to do", DynamicProfilesConfig is "when
// to escalate".
#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct DynamicProfilesConfig {
    pub enabled: bool,
    pub escalation: Vec<EscalationRule>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct EscalationRule {
    pub after_days: u32,
    pub filters: FilterRule,
}

// A separate struct (not inlined into Config) because proxy settings are
// cross-cutting: they can be passed to HTTP client constructors without dragging
// the rest of Config along.
#[derive(Debug, Deserialize, Serialize, Clone, Default)]
#[serde(default)]
pub struct ProxyConfig {
    pub enabled: bool,
    pub http: String,
    pub https: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pack_score_modifier_none_behaves_as_zero() {
        let cfg = GeneralConfig::default();
        assert_eq!(cfg.season_pack_score_modifier, None);
        assert_eq!(cfg.effective_pack_score_modifier(), 0);
    }

    #[test]
    fn test_pack_score_modifier_returns_configured_value() {
        let cfg = GeneralConfig {
            season_pack_score_modifier: Some(-25),
            ..Default::default()
        };
        assert_eq!(cfg.effective_pack_score_modifier(), -25);
    }

    #[test]
    fn test_pack_score_modifier_defaults_to_none_when_missing() {
        let json = serde_json::json!({});
        let cfg: GeneralConfig = serde_json::from_value(json).unwrap();
        assert_eq!(cfg.season_pack_score_modifier, None);
    }

    #[test]
    fn test_exposed_defaults_match_constructed_defaults() {
        let cfg = GeneralConfig::default();
        assert_eq!(
            cfg.media_info_scan_interval,
            GeneralConfig::MEDIA_INFO_SCAN_INTERVAL_DEFAULT
        );
        assert_eq!(
            cfg.media_info_scan_concurrency,
            GeneralConfig::MEDIA_INFO_SCAN_CONCURRENCY_DEFAULT
        );
        assert_eq!(
            cfg.season_pack_replace_threshold,
            GeneralConfig::SEASON_PACK_REPLACE_THRESHOLD_DEFAULT
        );
        assert_eq!(
            cfg.auto_search_wanted_interval,
            GeneralConfig::AUTO_SEARCH_WANTED_INTERVAL_DEFAULT
        );
        assert_eq!(
            cfg.auto_search_wanted_min_wait,
            GeneralConfig::AUTO_SEARCH_WANTED_MIN_WAIT_DEFAULT
        );
        assert_eq!(
            cfg.auto_search_wanted_max_age_days,
            GeneralConfig::AUTO_SEARCH_WANTED_MAX_AGE_DAYS_DEFAULT
        );
    }

    #[test]
    fn test_serde_missing_fields_use_exposed_defaults() {
        // An empty object deserializes purely through the `#[serde(default = ...)]`
        // functions, which must agree with the exposed constants.
        let cfg: GeneralConfig = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(
            cfg.media_info_scan_interval,
            GeneralConfig::MEDIA_INFO_SCAN_INTERVAL_DEFAULT
        );
        assert_eq!(
            cfg.media_info_scan_concurrency,
            GeneralConfig::MEDIA_INFO_SCAN_CONCURRENCY_DEFAULT
        );
        assert_eq!(
            cfg.season_pack_replace_threshold,
            GeneralConfig::SEASON_PACK_REPLACE_THRESHOLD_DEFAULT
        );
        assert_eq!(
            cfg.auto_search_wanted_interval,
            GeneralConfig::AUTO_SEARCH_WANTED_INTERVAL_DEFAULT
        );
        assert_eq!(
            cfg.auto_search_wanted_min_wait,
            GeneralConfig::AUTO_SEARCH_WANTED_MIN_WAIT_DEFAULT
        );
        assert_eq!(
            cfg.auto_search_wanted_max_age_days,
            GeneralConfig::AUTO_SEARCH_WANTED_MAX_AGE_DAYS_DEFAULT
        );
    }
}
