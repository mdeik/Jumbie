//! Series mapping types — the core data model for tracked series.
//!
//! [`MappingRule`] lives in this root module because it composes the submodules:
//! [`alias`] (alias/pattern parsing), [`matching`] (release/scan normalization),
//! [`season_alias`] (season-alias resolution), [`types`] (`SeriesSettings`,
//! `EpisodeInfo`, season-number helpers), and [`validation`].

pub mod alias;
pub mod matching;
pub mod season_alias;
pub mod season_resolve;
pub mod types;
pub mod validation;

pub use alias::{
    ParsedAlias, ParsedPattern, parse_alias, parse_source_pattern, plugin_name_to_slug,
    strip_named_groups,
};
pub use matching::{matches_normalized, normalize_cleaned_for_match, normalize_for_match};
pub use season_alias::{SeasonAliasDecision, resolve_season_from_aliases};
pub use season_resolve::{
    ResolvedSeason, infer_season_from_path, resolve_season_for_series,
    resolve_season_for_series_with_fallback, resolve_season_raw,
};
pub use types::{
    ABSOLUTE_SEASON_NUM, CompiledPatterns, DEFAULT_MONITOR_MODE, DEFAULT_SEASON_NUM, EpisodeInfo,
    MonitorMode, NumberingMode, SeasonOverride, SeasonParseError, SeriesSettings,
    SeriesSettingsForm, local_to_source_episode, parse_season_num, resolve_search_format,
    resolve_season_num, resolve_season_opt, source_to_local_episode,
};
pub use validation::{has_non_empty, non_empty_strs};

use crate::filtering::FilterRule;
use crate::scoring::ReleaseProfile;
use serde::{Deserialize, Serialize};

/// The full mapping rule for a tracked series.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MappingRule {
    pub target_title: String,

    pub release_profile: Option<String>,
    pub qb_category: Option<String>,
    pub filters: Option<FilterRule>,
    pub scoring: Option<ReleaseProfile>,

    #[serde(default)]
    pub name: String,
    /// Unique UUID identifying this series, used to disambiguate episode IDs when
    /// two series share a title. Populated on creation; MUST never be empty — call
    /// [`Self::ensure_series_id`] on any path loading a mapping from storage to
    /// backfill legacy mappings.
    #[serde(default)]
    pub series_id: String,
    pub quality_profile: Option<String>,
    #[serde(default)]
    pub hidden_in_library: bool,

    #[serde(flatten)]
    pub settings: SeriesSettings,
}

impl MappingRule {
    pub fn validate(&self) -> Result<(), String> {
        crate::validation::validate_title(&self.target_title).map_err(|e| e.to_string())?;
        self.settings.validate()?;

        if let Some(ref scoring) = self.scoring {
            scoring.validate()?;
        }

        Ok(())
    }

    /// Episode offset that releases for `season` use relative to local (DB)
    /// numbering: `source = local + offset`. Zero when the season has no override
    /// or no offset configured. SSoT for the per-season offset lookup.
    pub fn effective_episode_offset(&self, season: &str, global_absolute: bool) -> i32 {
        self.settings
            .find_season_override(season, global_absolute)
            .map(|override_rule| override_rule.offset())
            .unwrap_or(0)
    }

    /// Build the stable episode ID for `season`/`episode_num` under the series'
    /// **effective** numbering mode.
    ///
    /// `global_absolute_default` is `config.general.absolute_numbering`; it is
    /// required because `settings.absolute_numbering` is a tristate where `None`
    /// follows the global default. Emitting `S..E..` IDs for a global-absolute
    /// series would collide with the `ABS...` rows from `ensure_episode_cells`.
    ///
    /// Fails (rather than substituting a season) when a normal-mode `season` label
    /// is not a number. Absolute mode has no season, so the label is not consulted.
    pub fn get_episode_id(
        &self,
        season: &str,
        episode_num: i32,
        global_absolute_default: bool,
    ) -> Result<String, SeasonParseError> {
        // The ID encodes its numbering mode via the prefix (`ABS` vs `S##E`) to
        // prevent collisions between mode-switched data.
        let absolute = self
            .settings
            .active_mode(global_absolute_default)
            .is_absolute();
        crate::formatting::fmt_episode_id(season, episode_num, &self.series_id, absolute)
    }

    pub fn get_merged_filters(&self, global: &FilterRule) -> FilterRule {
        if let Some(local) = &self.filters {
            global.merge(local)
        } else {
            global.clone()
        }
    }

    pub fn get_merged_scoring(&self, global: &ReleaseProfile) -> ReleaseProfile {
        if let Some(local) = &self.scoring {
            global.merge(local)
        } else {
            global.clone()
        }
    }

    pub fn get_rename_cause(&self) -> String {
        let mut cause = if self.settings.episode_file_format.is_some()
            || self.settings.episode_file_format_absolute.is_some()
        {
            "Series Episode File Format"
        } else {
            "Global Episode File Format"
        }
        .to_string();

        if self.settings.absolute_numbering.unwrap_or(false) {
            cause.push_str(" (Absolute)");
        }

        cause
    }

    /// Ensure `series_id` is non-empty by generating a UUID if it is empty; returns
    /// `true` if one was generated.
    ///
    /// Call on every path loading a `MappingRule` from persistent storage to
    /// backfill legacy mappings created before the field existed.
    pub fn ensure_series_id(&mut self) -> bool {
        if self.series_id.is_empty() {
            self.series_id = crate::config::generate_uuid();
            true
        } else {
            false
        }
    }

    /// Bulk version for collections. Returns the number of backfilled mappings.
    pub fn ensure_series_ids<'a>(mappings: impl IntoIterator<Item = &'a mut Self>) -> usize {
        let mut count = 0;
        for m in mappings {
            if m.ensure_series_id() {
                count += 1;
            }
        }
        count
    }

    pub fn get_folder_rename_cause(&self) -> String {
        let mut cause = if self.settings.season_folder_format.is_some()
            || self.settings.season_folder_format_absolute.is_some()
        {
            "Series Season Folder Format"
        } else {
            "Global Season Folder Format"
        }
        .to_string();

        if self.settings.absolute_numbering.unwrap_or(false) {
            cause.push_str(" (Absolute)");
        }

        cause
    }
}

crate::test_module! {
    use std::collections::HashMap;
    use super::types::SeasonOverride;

    fn make_test_rule(season_overrides: HashMap<String, SeasonOverride>) -> MappingRule {
        MappingRule {
            target_title: "TestShow".to_string(),
            release_profile: None,
            qb_category: None,
            filters: None,
            scoring: None,
            name: "TestShow".to_string(),
            quality_profile: None,
            hidden_in_library: false,
            series_id: "test-series-1234".to_string(),
            settings: SeriesSettings {
                absolute_numbering: Some(false),
                season: season_overrides,
                ..Default::default()
            },
        }
    }

    #[test]
    fn test_contains_source_episode_with_offset() {
            // Season 1 with offset 100, range 1-100
            let so = SeasonOverride {
                season: "1".to_string(),
                episode_start: Some(1),
                episode_end: Some(100),
                cell_count: None,
                episode_offset: Some(100),
                alias_season_number: None,
                search_format: None,
                aliases: vec![],
                reg_patterns: vec![],
            };

        // Source ep 101 → local ep 1, in range [1,100] ✓
        assert!(so.contains_source_episode(101));
        // Source ep 200 → local ep 100, in range [1,100] ✓
        assert!(so.contains_source_episode(200));
        // Source ep 100 → local ep 0, NOT in range [1,100]
        assert!(!so.contains_source_episode(100));
        // Source ep 201 → local ep 101, NOT in range [1,100]
        assert!(!so.contains_source_episode(201));
    }

    #[test]
    fn test_contains_source_episode_default_start() {
            // No episode_start configured — should default to 1.
            // All optional fields must be provided since SeasonOverride doesn't derive Default.
            let so = SeasonOverride {
                season: "1".to_string(),
                episode_start: None,
                episode_end: None,
                cell_count: None,
                episode_offset: None,
                alias_season_number: None,
                search_format: None,
                aliases: vec![],
                reg_patterns: vec![],
            };

        // Any positive ep is in unbounded range starting at 1
        assert!(so.contains_source_episode(1));
        assert!(so.contains_source_episode(100));
        // Episode 0 should NOT be in range (default start is 1)
        assert!(!so.contains_source_episode(0));
    }

    #[test]
    fn test_contains_source_episode_negative_offset() {
            // Season with offset -5 (source ep 1 → local ep 6)
            let so = SeasonOverride {
                season: "2".to_string(),
                episode_start: Some(6),
                episode_end: Some(10),
                cell_count: None,
                episode_offset: Some(-5),
                alias_season_number: None,
                search_format: None,
                aliases: vec![],
                reg_patterns: vec![],
            };

        // Source ep 1 → local ep 6, in range [6,10] ✓
        assert!(so.contains_source_episode(1));
        // Source ep 5 → local ep 10, in range [6,10] ✓
        assert!(so.contains_source_episode(5));
        // Source ep 6 → local ep 11, NOT in range [6,10]
        assert!(!so.contains_source_episode(6));
    }

    #[test]
    fn test_contains_local_episode() {
            // Season with range 1-100, no offset
            let so = SeasonOverride {
                season: "1".to_string(),
                episode_start: Some(1),
                episode_end: Some(100),
                cell_count: None,
                episode_offset: None,
                alias_season_number: None,
                search_format: None,
                aliases: vec![],
                reg_patterns: vec![],
            };

        // Local ep in range
        assert!(so.contains_local_episode(1));
        assert!(so.contains_local_episode(50));
        assert!(so.contains_local_episode(100));
        // Local ep outside range
        assert!(!so.contains_local_episode(0));
        assert!(!so.contains_local_episode(101));
    }

    #[test]
    fn test_contains_local_episode_default_start() {
            // No range configured — unbounded starting at 1
            let so = SeasonOverride {
                season: "1".to_string(),
                episode_start: None,
                episode_end: None,
                cell_count: None,
                episode_offset: None,
                alias_season_number: None,
                search_format: None,
                aliases: vec![],
                reg_patterns: vec![],
            };

        assert!(so.contains_local_episode(1));
        // Episode 0 should be rejected (default start is 1)
        assert!(!so.contains_local_episode(0));
    }

    #[test]
    fn test_get_episode_id() {
        // Standard mode — uses S{season}E{episode}
        let rule = make_test_rule(HashMap::new());
        let id = rule.get_episode_id("1", 5, false).unwrap();
        assert_eq!(id, "test-series-1234_S01E05");

        let id2 = rule.get_episode_id("2", 100, false).unwrap();
        assert_eq!(id2, "test-series-1234_S02E100");

        // Absolute mode — uses ABS{episode}
        let mut abs_rule = make_test_rule(HashMap::new());
        abs_rule.settings.absolute_numbering = Some(true);
        let abs_id = abs_rule.get_episode_id("1", 5, false).unwrap();
        assert_eq!(abs_id, "test-series-1234_ABS0005");

        // Verify no collision: same series_id, same episode number,
        // different modes produce different IDs.
        assert_ne!(
            id, abs_id,
            "Standard and absolute modes must produce different episode IDs"
        );
    }

    #[test]
    fn test_get_episode_id_rejects_non_numeric_season() {
        // A normal-mode season label that isn't a number must fail rather than
        // be coerced to season 0/1 (which wrote rows under the wrong season).
        let rule = make_test_rule(HashMap::new());
        assert!(rule.get_episode_id("SP", 5, false).is_err());

        // Absolute mode has no season, so the same label is fine there.
        let mut abs_rule = make_test_rule(HashMap::new());
        abs_rule.settings.absolute_numbering = Some(true);
        assert_eq!(
            abs_rule.get_episode_id("SP", 5, false).unwrap(),
            "test-series-1234_ABS0005"
        );
    }

    #[test]
    fn test_get_episode_id_none_tristate_follows_global_default() {
        // Regression: `absolute_numbering: None` means "follow the global
        // default". It must NOT be treated as normal mode — otherwise a
        // global-absolute series generates S..E.. IDs that collide with the
        // ABS... rows created by ensure_episode_cells.
        let mut rule = make_test_rule(HashMap::new());
        rule.settings.absolute_numbering = None;
        assert_eq!(
            rule.get_episode_id("1", 5, true).unwrap(),
            "test-series-1234_ABS0005",
            "None tristate + global absolute must produce an ABS id"
        );
        assert_eq!(
            rule.get_episode_id("1", 5, false).unwrap(),
            "test-series-1234_S01E05",
            "None tristate + global normal must produce a standard id"
        );
    }

    #[test]
    fn test_get_merged_filters() {
        let mut rule = make_test_rule(HashMap::new());
        let global_filter = FilterRule {
            required: vec!["global_req".to_string()],
            ..Default::default()
        };

        // Case 1: No local filters
        let merged1 = rule.get_merged_filters(&global_filter);
        assert_eq!(merged1.required, vec!["global_req".to_string()]);

        // Case 2: Local filters exist
        rule.filters = Some(FilterRule {
            required: vec!["local_req".to_string()],
            ..Default::default()
        });

        let merged2 = rule.get_merged_filters(&global_filter);
        assert!(merged2.required.contains(&"global_req".to_string()));
        assert!(merged2.required.contains(&"local_req".to_string()));
        assert_eq!(merged2.required.len(), 2);
    }

    #[test]
    fn test_ensure_series_id_generates_uuid_for_empty() {
        let mut rule = make_test_rule(HashMap::new());
        rule.series_id = String::new(); // explicitly empty (legacy scenario)
        assert!(
            rule.ensure_series_id(),
            "ensure_series_id should return true when backfilling"
        );
        assert!(
            !rule.series_id.is_empty(),
            "series_id should be populated after ensure_series_id"
        );
        // Verify it looks like a UUID
        assert!(
            rule.series_id.len() == 36,
            "series_id should be a UUID (36 chars), got {}: '{}'",
            rule.series_id.len(),
            rule.series_id
        );
    }

    #[test]
    fn test_ensure_series_id_does_not_change_existing() {
        let mut rule = make_test_rule(HashMap::new());
        // rule already has series_id = "test-series-1234" from make_test_rule
        assert!(
            !rule.ensure_series_id(),
            "ensure_series_id should return false when already populated"
        );
        assert_eq!(
            rule.series_id, "test-series-1234",
            "existing series_id must not be changed"
        );
    }

    #[test]
    fn test_ensure_series_ids_bulk() {
        let mut mappings = vec![
            MappingRule {
                target_title: "Empty 1".to_string(),
                series_id: String::new(),
                ..Default::default()
            },
            MappingRule {
                target_title: "Populated".to_string(),
                series_id: "existing-uuid".to_string(),
                ..Default::default()
            },
            MappingRule {
                target_title: "Empty 2".to_string(),
                series_id: String::new(),
                ..Default::default()
            },
        ];
        let count = MappingRule::ensure_series_ids(mappings.iter_mut());
        assert_eq!(
            count, 2,
            "should have backfilled 2 empty series_ids"
        );
        for m in &mappings {
            assert!(
                !m.series_id.is_empty(),
                "all mappings should have non-empty series_id after bulk ensure, got empty for '{}'",
                m.target_title
            );
        }
    }

    #[test]
    fn test_get_merged_scoring() {
        let mut rule = make_test_rule(HashMap::new());
        let mut global_terms = HashMap::new();
        global_terms.insert("1080p".to_string(), 10);

        let global_scoring = ReleaseProfile {
            terms: global_terms,
            size_score_per_gb: 5,
            ..Default::default()
        };

        // Case 1: No local scoring
        let merged1 = rule.get_merged_scoring(&global_scoring);
        assert!(merged1.terms.contains_key("1080p"));
        assert_eq!(merged1.size_score_per_gb, 5);

        // Case 2: Local scoring exists
        let mut local_terms = HashMap::new();
        local_terms.insert("hevc".to_string(), 20);

        rule.scoring = Some(ReleaseProfile {
            terms: local_terms,
            size_score_per_gb: 15,
            ..Default::default()
        });

        let merged2 = rule.get_merged_scoring(&global_scoring);
        assert!(merged2.terms.contains_key("1080p"), "Should contain global term");
        assert!(merged2.terms.contains_key("hevc"), "Should contain local term");
        assert_eq!(merged2.size_score_per_gb, 15, "Local property should override global");
    }

    // ── Episode numbering-space conversions (SSoT) ─────────────────────

    fn make_override(offset: i32) -> SeasonOverride {
        SeasonOverride {
            season: "1".to_string(),
            episode_start: Some(1),
            episode_end: Some(100),
            cell_count: None,
            episode_offset: Some(offset),
            alias_season_number: None,
            search_format: None,
            aliases: vec![],
            reg_patterns: vec![],
        }
    }

    #[test]
    fn test_source_local_episode_round_trip() {
        for offset in [-100, -1, 0, 1, 5, 1169] {
            for local in [1, 5, 100, 1170] {
                let source = local_to_source_episode(local, offset);
                assert_eq!(
                    source_to_local_episode(source, offset),
                    local,
                    "round-trip failed for local={local} offset={offset}"
                );
            }
        }
    }

    #[test]
    fn test_contains_source_episode_uses_local_episode_helper() {
        let so = make_override(100);
        // Source 101 → local 1; source 201 → local 101 (out of [1,100]).
        assert_eq!(so.local_episode(101), 1);
        assert_eq!(so.source_episode(1), 101);
        assert!(so.contains_source_episode(101));
        assert!(!so.contains_source_episode(201));
    }

    // ── search format resolution (season → series → global) ───────────────

    #[test]
    fn test_resolve_search_format_layers_season_over_series_over_global() {
        let mut series = SeriesSettings {
            search_format: Some("SERIES ${episode}".to_string()),
            ..Default::default()
        };
        let season = make_override(0);

        assert_eq!(
            resolve_search_format(&series, Some(&season), "GLOBAL", false),
            "SERIES ${episode}"
        );

        let mut season_override = make_override(0);
        season_override.search_format = Some("SEASON ${episode}".to_string());
        assert_eq!(
            resolve_search_format(&series, Some(&season_override), "GLOBAL", false),
            "SEASON ${episode}"
        );

        // A blank season override is a deliberate blank, not an inherit.
        season_override.search_format = Some(String::new());
        assert_eq!(
            resolve_search_format(&series, Some(&season_override), "GLOBAL", false),
            ""
        );

        // No series override → global.
        series.search_format = None;
        assert_eq!(
            resolve_search_format(&series, None, "GLOBAL", false),
            "GLOBAL"
        );
    }

    #[test]
    fn test_resolve_search_format_is_mode_aware() {
        let series = SeriesSettings {
            search_format: Some("NORMAL".to_string()),
            search_format_absolute: Some("ABS".to_string()),
            ..Default::default()
        };
        assert_eq!(
            resolve_search_format(&series, None, "GLOBAL", false),
            "NORMAL"
        );
        assert_eq!(resolve_search_format(&series, None, "GLOBAL", true), "ABS");
    }

    #[test]
    fn test_resolve_search_format_distinguishes_inherit_from_blank() {
        // Series-level blank beats global, and is NOT the same as inherit.
        let series = SeriesSettings {
            search_format: Some(String::new()),
            ..Default::default()
        };
        assert_eq!(resolve_search_format(&series, None, "GLOBAL", false), "");

        // `None` inherits global; `Some("")` is a deliberate blank that wins.
        let inherit = SeriesSettings {
            search_format: None,
            ..Default::default()
        };
        assert_eq!(
            resolve_search_format(&inherit, None, "GLOBAL", false),
            "GLOBAL"
        );
    }

    #[test]
    fn test_resolve_search_format_cross_mode_isolation() {
        // A series override for one mode must not leak into the other: the mode
        // without an override falls through to global, not to its sibling.
        let mut series = SeriesSettings {
            search_format: Some("NORMAL".to_string()),
            ..Default::default()
        };
        assert_eq!(resolve_search_format(&series, None, "GLOBAL", false), "NORMAL");
        assert_eq!(resolve_search_format(&series, None, "GLOBAL", true), "GLOBAL");

        series.search_format = None;
        series.search_format_absolute = Some("ABS".to_string());
        assert_eq!(resolve_search_format(&series, None, "GLOBAL", false), "GLOBAL");
        assert_eq!(resolve_search_format(&series, None, "GLOBAL", true), "ABS");
    }

    #[test]
    fn test_resolve_search_format_absolute_blank_wins() {
        // A deliberate blank on the absolute chain beats global, exactly like
        // the normal chain; blank is not "inherit".
        let series = SeriesSettings {
            search_format: Some("NORMAL".to_string()),
            search_format_absolute: Some(String::new()),
            ..Default::default()
        };
        assert_eq!(resolve_search_format(&series, None, "GLOBAL", true), "");
    }

    #[test]
    fn test_resolve_search_format_season_override_wins_in_both_modes() {
        // `SeasonOverride::search_format` is a single field, so it wins in
        // normal and absolute mode alike (absolute still uses the series/global
        // chain underneath).
        let series = SeriesSettings {
            search_format: Some("NORMAL".to_string()),
            search_format_absolute: Some("ABS".to_string()),
            ..Default::default()
        };
        let mut season = make_override(0);
        season.search_format = Some("SEASON".to_string());
        assert_eq!(
            resolve_search_format(&series, Some(&season), "GLOBAL", false),
            "SEASON"
        );
        assert_eq!(
            resolve_search_format(&series, Some(&season), "GLOBAL", true),
            "SEASON"
        );

        // A blank season override also wins in absolute mode.
        season.search_format = Some(String::new());
        assert_eq!(resolve_search_format(&series, Some(&season), "GLOBAL", true), "");
    }

    #[test]
    fn test_resolve_search_format_season_override_without_format_falls_through() {
        // A season override can exist for other reasons (aliases, offset) while
        // leaving `search_format` unset — that inherits, it is not a blank.
        let series = SeriesSettings {
            search_format: Some("SERIES".to_string()),
            ..Default::default()
        };
        let season = make_override(5);
        assert_eq!(season.search_format, None);
        assert_eq!(
            resolve_search_format(&series, Some(&season), "GLOBAL", false),
            "SERIES"
        );

        let bare = SeriesSettings::default();
        assert_eq!(
            resolve_search_format(&bare, Some(&season), "GLOBAL", false),
            "GLOBAL"
        );
    }

    #[test]
    fn test_search_format_absolute_blank_survives_serde() {
        let s: SeriesSettings = serde_json::from_value(serde_json::json!({
            "search_format_absolute": "",
        }))
        .expect("blank absolute search_format deserializes");
        assert_eq!(s.search_format_absolute, Some(String::new()));
        // The sibling chain stays inherit.
        assert_eq!(s.search_format, None);

        let missing: SeriesSettings =
            serde_json::from_value(serde_json::json!({})).expect("empty object deserializes");
        assert_eq!(missing.search_format_absolute, None);
    }

    #[test]
    fn test_search_format_blank_survives_serde() {
        let so: SeasonOverride = serde_json::from_value(serde_json::json!({
            "season": "1",
            "search_format": "",
        }))
        .expect("blank search_format deserializes");
        assert_eq!(so.search_format, Some(String::new()));

        // Missing field is inherit, not blank.
        let inherit: SeasonOverride = serde_json::from_value(serde_json::json!({ "season": "1" }))
            .expect("missing search_format deserializes");
        assert_eq!(inherit.search_format, None);

        // Round-trip keeps the blank distinct from absent.
        let value = serde_json::to_value(&so).unwrap();
        assert_eq!(value["search_format"], serde_json::json!(""));
    }

    #[test]
    fn test_legacy_omit_fields_are_ignored_on_deserialize() {
        // Storage: the removed omit fields are dropped, not carried forward.
        let s: SeriesSettings = serde_json::from_value(serde_json::json!({
            "omit_season": true,
            "omit_prefix": true,
        }))
        .unwrap();
        assert_eq!(s.search_format, None);
        assert_eq!(s.search_format_absolute, None);

        let so: SeasonOverride = serde_json::from_value(serde_json::json!({
            "season": "1",
            "omit_season": true,
        }))
        .unwrap();
        assert_eq!(so.search_format, None);
    }
}
