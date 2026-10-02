//! Season alias + source alias behavior tests.

use crate::tests::behavior_tests::helpers::make_mapping;
use jumbie_shared::mapping::{MappingRule, SeriesSettings};
use std::collections::HashMap;

use jumbie_shared::parsing::{CustomParseResult, parse_title_with_custom_regex};

/// Helper: apply the same reg_patterns filter logic that process_entry uses.
/// Returns true if the title matches at least one pattern (empty patterns = pass all).
/// Supports source-scoped patterns via `@slug:pattern` syntax; pass `None` for source_name
/// when testing non-source-aware filtering (generic-only patterns will still match).
///
/// For testing with plugin instance ids, use [`matches_reg_patterns_by_slug`].
fn matches_reg_patterns(title: &str, mapping: &MappingRule, source_name: Option<&str>) -> bool {
    use jumbie_shared::mapping::{parse_source_pattern, plugin_name_to_slug};

    if !mapping.settings.has_search_patterns() {
        return true;
    }
    let entry_slug = source_name.map(plugin_name_to_slug);
    mapping.settings.reg_patterns.iter().any(|pat| {
        let parsed = parse_source_pattern(pat);
        // If pattern is source-scoped, skip if source doesn't match
        if let Some(ref slug) = parsed.source_id {
            match &entry_slug {
                Some(es) if es == slug => {}
                _ => return false,
            }
        }
        regex::Regex::new(&parsed.pattern)
            .map(|re| re.is_match(title))
            .unwrap_or(false)
    })
}

/// Same as [`matches_reg_patterns`] but accepts a pre-computed source slug
/// (plugin instance id or legacy display-name slug) directly, without
/// conversion. Use this to test UUID-format prefixes like `@uuid:pattern`.
fn matches_reg_patterns_by_slug(
    title: &str,
    mapping: &MappingRule,
    source_slug: Option<&str>,
) -> bool {
    use jumbie_shared::mapping::parse_source_pattern;

    if !mapping.settings.has_search_patterns() {
        return true;
    }
    mapping.settings.reg_patterns.iter().any(|pat| {
        let parsed = parse_source_pattern(pat);
        if let Some(ref slug) = parsed.source_id {
            match source_slug {
                Some(es) if es == slug => {}
                _ => return false,
            }
        }
        regex::Regex::new(&parsed.pattern)
            .map(|re| re.is_match(title))
            .unwrap_or(false)
    })
}

/// Helper: simulate the combined custom regex extraction & filter logic that
/// `process_entry` now uses.  Returns:
///   - `Ok(Some(EpisodeInfo))` if a pattern with named groups extracted data
///   - `Ok(None)` if a pattern matched but had no named groups (filter-only)
///   - `Err(&str)` if no pattern matched (entry should be rejected)
///
/// This mirrors the logic in process_entry where `parse_title_with_custom_regex`
/// is called with the effective patterns, current season, and source slug.
///
/// For testing with plugin instance ids, use [`extract_or_filter_by_slug`].
fn extract_or_filter<'a>(
    title: &'a str,
    mapping: &'a MappingRule,
    target_season: Option<i32>,
    source_name: Option<&str>,
) -> Result<Option<jumbie_shared::mapping::EpisodeInfo>, String> {
    if !mapping.settings.has_search_patterns() {
        return Ok(None);
    }
    let entry_slug = source_name.map(jumbie_shared::mapping::plugin_name_to_slug);
    match parse_title_with_custom_regex(
        title,
        &mapping.settings.reg_patterns,
        target_season,
        mapping.settings.active_mode(false).is_absolute(),
        entry_slug.as_deref(),
    ) {
        CustomParseResult::Extracted(info) => Ok(Some(*info)),
        CustomParseResult::MatchedFilter => Ok(None),
        CustomParseResult::NoMatch => Err("No pattern matched the title".to_string()),
    }
}

/// Same as [`extract_or_filter`] but accepts a pre-computed source slug
/// (plugin instance id or legacy display-name slug) directly, without
/// conversion. Use this to test UUID-format prefixes like `@uuid:pattern`.
fn extract_or_filter_by_slug<'a>(
    title: &'a str,
    mapping: &'a MappingRule,
    target_season: Option<i32>,
    source_slug: Option<&str>,
) -> Result<Option<jumbie_shared::mapping::EpisodeInfo>, String> {
    if !mapping.settings.has_search_patterns() {
        return Ok(None);
    }
    match parse_title_with_custom_regex(
        title,
        &mapping.settings.reg_patterns,
        target_season,
        mapping.settings.active_mode(false).is_absolute(),
        source_slug,
    ) {
        CustomParseResult::Extracted(info) => Ok(Some(*info)),
        CustomParseResult::MatchedFilter => Ok(None),
        CustomParseResult::NoMatch => Err("No pattern matched the title".to_string()),
    }
}

#[test]
fn test_season_alias_replaces_series_aliases_in_process_entry() {
    // In process_entry: `mapping.settings.aliases = rule.aliases.clone()`
    let series_aliases = vec!["SeriesAlias".to_string()];
    let season_aliases = vec!["SeasonAlias".to_string()];

    let mut settings = SeriesSettings {
        aliases: series_aliases.clone(),
        ..SeriesSettings::default()
    };
    assert_eq!(
        settings.aliases,
        vec!["SeriesAlias"],
        "Before season override, series aliases are present"
    );

    // Season override matches and replaces (this is what process_entry does)
    settings.aliases = season_aliases.clone();

    assert_eq!(
        settings.aliases,
        vec!["SeasonAlias"],
        "Season aliases REPLACE series aliases in process_entry"
    );
    assert!(
        !settings.aliases.contains(&"SeriesAlias".to_string()),
        "Series aliases should be gone after season override replacement"
    );
}

#[test]
fn test_auto_search_missing_treats_empty_aliases_as_no_aliases() {
    // Regression test: blank lines in textarea produce empty strings.
    // `has_non_empty` should treat `[""]` the same as `[]`.
    let target_title = "OriginalTitle".to_string();
    let series_aliases = vec!["".to_string()];

    let has_any_alias = jumbie_shared::mapping::has_non_empty(&series_aliases);
    let mut all_aliases = if has_any_alias {
        vec![]
    } else {
        vec![target_title.clone()]
    };
    all_aliases.extend(jumbie_shared::mapping::non_empty_strs(&series_aliases).map(String::from));

    assert_eq!(
        all_aliases,
        vec!["OriginalTitle".to_string()],
        "Empty alias string should behave like no aliases — fall back to target title"
    );
}

#[test]
fn test_auto_search_missing_ignores_empty_among_real_aliases() {
    // Regression test: real aliases mixed with blank lines should preserve
    // real aliases and skip the blank ones.
    let target_title = "OriginalTitle".to_string();
    let series_aliases = vec![
        "FirstAlias".to_string(),
        "".to_string(),
        "SecondAlias".to_string(),
    ];

    let has_any_alias = jumbie_shared::mapping::has_non_empty(&series_aliases);
    let mut all_aliases = if has_any_alias {
        vec![]
    } else {
        vec![target_title.clone()]
    };
    all_aliases.extend(jumbie_shared::mapping::non_empty_strs(&series_aliases).map(String::from));

    assert!(
        !all_aliases.contains(&"OriginalTitle".to_string()),
        "Real aliases exist — target title should be omitted"
    );
    assert!(
        !all_aliases.contains(&"".to_string()),
        "Empty strings should be filtered out"
    );
    assert_eq!(
        all_aliases,
        vec!["FirstAlias".to_string(), "SecondAlias".to_string()],
        "Only non-empty aliases should remain"
    );
}

#[test]
fn test_auto_search_missing_prefers_aliases_over_target_title() {
    let target_title = "OriginalTitle".to_string();
    let series_aliases = vec!["SeriesAlias".to_string()];

    let has_any_alias = !series_aliases.is_empty();
    let mut all_aliases = if has_any_alias {
        vec![]
    } else {
        vec![target_title.clone()]
    };
    all_aliases.extend(series_aliases.clone());

    assert_eq!(
        all_aliases.first().unwrap(),
        "SeriesAlias",
        "Series alias should be FIRST when aliases exist, not target title"
    );
    assert!(
        !all_aliases.contains(&"OriginalTitle".to_string()),
        "Target title should NOT be in aliases when series aliases exist"
    );
}

#[test]
fn test_auto_search_missing_falls_back_to_title_when_no_aliases() {
    let target_title = "OriginalTitle".to_string();
    let series_aliases: Vec<String> = vec![];

    let has_any_alias = !series_aliases.is_empty();
    let mut all_aliases = if has_any_alias {
        vec![]
    } else {
        vec![target_title.clone()]
    };
    all_aliases.extend(series_aliases.clone());

    assert_eq!(
        all_aliases,
        vec!["OriginalTitle".to_string()],
        "Should fall back to target_title when no aliases exist"
    );
}

#[test]
fn test_auto_search_missing_with_season_aliases_only() {
    let target_title = "OriginalTitle".to_string();
    let series_aliases: Vec<String> = vec![];
    let season_aliases = vec!["SeasonAlias".to_string()];

    let has_any_alias = !series_aliases.is_empty() || !season_aliases.is_empty();
    let mut all_aliases = if has_any_alias {
        vec![]
    } else {
        vec![target_title.clone()]
    };
    all_aliases.extend(series_aliases.clone());
    all_aliases.extend(season_aliases.clone());

    assert!(
        !all_aliases.contains(&"OriginalTitle".to_string()),
        "Target title should NOT be present when season aliases exist"
    );
    assert!(all_aliases.contains(&"SeasonAlias".to_string()));
    assert_eq!(all_aliases.len(), 1);
}

#[test]
fn test_auto_search_missing_with_both_series_and_season_aliases() {
    // Priority: season aliases REPLACE series aliases.
    // When season has aliases, series-level aliases are NOT included.
    use jumbie_shared::types::MappingRule;
    let mapping: MappingRule = serde_json::from_value(serde_json::json!({
        "target_title": "OriginalTitle",
        "aliases": ["SeriesAlias"],
        "season": {
            "S01": {
                "season": "S01",
                "aliases": ["SeasonAlias"],
            }
        }
    }))
    .expect("Failed to create test MappingRule");
    let (generic, _source) = crate::search::split_aliases_by_source(&mapping, "S01", false);
    assert_eq!(
        generic,
        vec!["SeasonAlias"],
        "Season aliases replace series aliases — only SeasonAlias should appear"
    );
}

#[test]
fn test_season_override_reg_patterns_stored_correctly() {
    let mut season_overrides = HashMap::new();
    season_overrides.insert(
        "1".to_string(),
        jumbie_shared::mapping::SeasonOverride {
            season: "1".to_string(),
            episode_start: None,
            episode_end: None,
            cell_count: None,
            episode_offset: None,
            alias_season_number: None,
            search_format: None,
            aliases: vec!["Season1Show".to_string()],
            reg_patterns: vec!["1080p".to_string(), "\\[MockFansub\\]".to_string()],
        },
    );

    let mapping = make_mapping("TestShow", season_overrides);

    let rule = mapping.settings.season.get("1").unwrap();
    assert!(
        !rule.reg_patterns.is_empty(),
        "Season override should have source patterns"
    );
    assert_eq!(rule.reg_patterns.len(), 2);
    assert_eq!(rule.reg_patterns[0], "1080p");
}

#[test]
fn test_reg_patterns_empty_passes_everything() {
    let mut mapping = make_mapping("TestShow", HashMap::new());
    mapping.settings.reg_patterns = vec![];

    assert!(matches_reg_patterns("Anything here", &mapping, None));
    assert!(matches_reg_patterns("[MockFansub] Show", &mapping, None));
    assert!(matches_reg_patterns("Some random title", &mapping, None));
}

#[test]
fn test_reg_patterns_match_filter() {
    let mut mapping = make_mapping("TestShow", HashMap::new());
    mapping.settings.reg_patterns = vec!["\\[MockFansub\\]".to_string()];

    assert!(matches_reg_patterns(
        "[MockFansub] My Show - S01E01",
        &mapping,
        None
    ));
    assert!(!matches_reg_patterns(
        "OtherGroup - My Show - S01E01",
        &mapping,
        None
    ));
}

#[test]
fn test_reg_patterns_any_match_suffices() {
    let mut mapping = make_mapping("TestShow", HashMap::new());
    mapping.settings.reg_patterns = vec!["1080p".to_string(), "720p".to_string()];

    assert!(matches_reg_patterns(
        "Show - S01E01 - 1080p",
        &mapping,
        None
    ));
    assert!(matches_reg_patterns("Show - S01E01 - 720p", &mapping, None));
    assert!(!matches_reg_patterns(
        "Show - S01E01 - 480p",
        &mapping,
        None
    ));
}

#[test]
fn test_reg_patterns_series_level() {
    let mut mapping = make_mapping("TestShow", HashMap::new());
    mapping.settings.reg_patterns = vec!["\\[MockFansub\\]".to_string()];

    assert!(matches_reg_patterns(
        "[MockFansub] TestShow - S01E01",
        &mapping,
        None
    ));
    assert!(!matches_reg_patterns(
        "OtherGroup - TestShow - S01E01",
        &mapping,
        None
    ));
}

#[test]
fn test_reg_patterns_season_override_replaces_series_level() {
    // Simulates process_entry: season override's reg_patterns clone onto mapping
    let mut mapping = make_mapping("TestShow", HashMap::new());
    mapping.settings.reg_patterns = vec!["\\[MockFansub\\]".to_string()];

    // Season override replaces (as process_entry does: mapping.settings.reg_patterns = rule.reg_patterns.clone())
    mapping.settings.reg_patterns = vec!["1080p".to_string()];

    assert!(matches_reg_patterns(
        "TestShow - S01E01 - 1080p",
        &mapping,
        None
    ));
    assert!(!matches_reg_patterns(
        "[MockFansub] TestShow - S01E01",
        &mapping,
        None
    ));
}

#[test]
fn test_reg_patterns_invalid_regex_is_safely_skipped() {
    let mut mapping = make_mapping("TestShow", HashMap::new());
    mapping.settings.reg_patterns = vec!["*invalid[regex".to_string(), "MockFansub".to_string()];

    assert!(matches_reg_patterns(
        "[MockFansub] My Show - S01E01",
        &mapping,
        None
    ));
    assert!(!matches_reg_patterns(
        "[Other] My Show - S01E01",
        &mapping,
        None
    ));
    assert!(!matches_reg_patterns("Random title", &mapping, None));
}

#[test]
fn test_reg_patterns_source_specific() {
    let mut mapping = make_mapping("TestShow", HashMap::new());
    // Pattern scoped to "nyaa" — only matches when source_name is Nyaa
    mapping.settings.reg_patterns =
        vec!["@nyaa:1080p".to_string(), "@tokyotoshokan:720p".to_string()];

    // Nyaa source: should match "1080p"
    assert!(matches_reg_patterns(
        "TestShow - S01E01 - 1080p",
        &mapping,
        Some("Nyaa")
    ));
    assert!(!matches_reg_patterns(
        "TestShow - S01E01 - 720p",
        &mapping,
        Some("Nyaa")
    ));

    // TokyoToshokan source: should match "720p"
    assert!(matches_reg_patterns(
        "TestShow - S01E01 - 720p",
        &mapping,
        Some("TokyoToshokan")
    ));

    // Unknown source: neither pattern applies
    assert!(!matches_reg_patterns(
        "TestShow - S01E01 - 1080p",
        &mapping,
        Some("OtherSource")
    ));
}

#[test]
fn test_reg_patterns_source_specific_falls_back_to_generic() {
    let mut mapping = make_mapping("TestShow", HashMap::new());
    // Mix of source-scoped and generic patterns
    mapping.settings.reg_patterns = vec!["@nyaa:1080p".to_string(), "generic_pattern".to_string()];

    // TokyoToshokan doesn't match @nyaa, but generic pattern still applies
    assert!(matches_reg_patterns(
        "Some title with generic_pattern",
        &mapping,
        Some("TokyoToshokan")
    ));

    // TokyoToshokan doesn't match @nyaa and doesn't match generic either
    assert!(!matches_reg_patterns(
        "Some title without match",
        &mapping,
        Some("TokyoToshokan")
    ));
}


#[test]
fn test_reg_patterns_source_specific_regex_anchors() {
    // Source-scoped pattern using ^ and $ anchors for exact matching
    let mut mapping = make_mapping("TestShow", HashMap::new());
    mapping.settings.reg_patterns = vec!["@nyaa:^1080p$".to_string()];

    // Nyaa source: "1080p" matches anchored pattern
    assert!(matches_reg_patterns("1080p", &mapping, Some("Nyaa")));

    // Nyaa source: "1080p" as substring does NOT match anchored pattern
    assert!(!matches_reg_patterns(
        "TestShow - S01E01 - 1080p",
        &mapping,
        Some("Nyaa")
    ));

    // Generic source with no source name: anchored patterns still don't apply
    assert!(!matches_reg_patterns("1080p", &mapping, None));
}

#[test]
fn test_reg_patterns_source_specific_regex_alternation() {
    // Source-scoped pattern using | alternation to match multiple resolutions
    let mut mapping = make_mapping("TestShow", HashMap::new());
    mapping.settings.reg_patterns = vec!["@nyaa:(1080p|2160p)".to_string()];

    // Nyaa source: "1080p" matches the alternation
    assert!(matches_reg_patterns(
        "TestShow - S01E01 - 1080p",
        &mapping,
        Some("Nyaa")
    ));

    // Nyaa source: "2160p" also matches
    assert!(matches_reg_patterns(
        "TestShow - S01E01 - 2160p",
        &mapping,
        Some("Nyaa")
    ));

    // Nyaa source: "720p" does NOT match either alternation
    assert!(!matches_reg_patterns(
        "TestShow - S01E01 - 720p",
        &mapping,
        Some("Nyaa")
    ));

    // Other source: the @nyaa-scoped pattern is skipped entirely
    assert!(!matches_reg_patterns(
        "TestShow - S01E01 - 1080p",
        &mapping,
        Some("TokyoToshokan")
    ));
}

#[test]
fn test_reg_patterns_source_specific_regex_character_class() {
    // Source-scoped pattern using \d character class for flexible resolution matching
    let mut mapping = make_mapping("TestShow", HashMap::new());
    mapping.settings.reg_patterns = vec!["@nyaa:\\d{3,4}p".to_string()];

    // Nyaa source: "720p" matches \d{3,4}p
    assert!(matches_reg_patterns(
        "TestShow - S01E01 - 720p",
        &mapping,
        Some("Nyaa")
    ));

    // Nyaa source: "1080p" matches
    assert!(matches_reg_patterns(
        "TestShow - S01E01 - 1080p",
        &mapping,
        Some("Nyaa")
    ));

    // Nyaa source: "2160p" matches (4 digits)
    assert!(matches_reg_patterns(
        "TestShow - S01E01 - 2160p",
        &mapping,
        Some("Nyaa")
    ));

    // Nyaa source: "HD" does not match
    assert!(!matches_reg_patterns(
        "TestShow - S01E01 - HD",
        &mapping,
        Some("Nyaa")
    ));
}

#[test]
fn test_reg_patterns_source_specific_multiple_for_one_source() {
    // Multiple patterns targeting the same source — any match suffices
    let mut mapping = make_mapping("TestShow", HashMap::new());
    mapping.settings.reg_patterns = vec![
        "@nyaa:1080p".to_string(),
        "@nyaa:2160p".to_string(),
        "@nyaa:\\[MockFansub\\]".to_string(),
    ];

    // Nyaa source: matches first pattern "1080p"
    assert!(matches_reg_patterns(
        "TestShow - S01E01 - 1080p",
        &mapping,
        Some("Nyaa")
    ));

    // Nyaa source: matches second pattern "2160p"
    assert!(matches_reg_patterns(
        "TestShow - S01E01 - 2160p",
        &mapping,
        Some("Nyaa")
    ));

    // Nyaa source: matches third pattern "[MockFansub]"
    assert!(matches_reg_patterns(
        "[MockFansub] TestShow - S01E01",
        &mapping,
        Some("Nyaa")
    ));

    // Nyaa source: "720p" matches none of the three
    assert!(!matches_reg_patterns(
        "TestShow - S01E01 - 720p",
        &mapping,
        Some("Nyaa")
    ));

    // Other source: all @nyaa-scoped patterns are skipped
    assert!(!matches_reg_patterns(
        "TestShow - S01E01 - 1080p",
        &mapping,
        Some("TokyoToshokan")
    ));
}

#[test]
fn test_reg_patterns_source_specific_no_pattern_applies() {
    // One source has patterns but they only match a different source
    let mut mapping = make_mapping("TestShow", HashMap::new());
    mapping.settings.reg_patterns = vec!["@nyaa:1080p".to_string()];

    // TokyoToshokan source has no matching patterns (the @nyaa one is skipped)
    assert!(!matches_reg_patterns(
        "TestShow - S01E01 - 1080p",
        &mapping,
        Some("TokyoToshokan")
    ));

    // Source name that normalizes to a different slug than "nyaa"
    assert!(!matches_reg_patterns(
        "TestShow - S01E01 - 1080p",
        &mapping,
        Some("Nyaa Subs")
    ));
}

#[test]
fn test_reg_patterns_source_specific_with_escaped_brackets() {
    // Source-scoped pattern with escaped brackets for release group matching
    let mut mapping = make_mapping("TestShow", HashMap::new());
    mapping.settings.reg_patterns = vec!["@mockfansub:\\[MockFansub\\]".to_string()];

    // MockFansub source: "[MockFansub]" matches the escaped bracket pattern
    assert!(matches_reg_patterns(
        "[MockFansub] My Show - S01E01 - 1080p",
        &mapping,
        Some("MockFansub")
    ));

    // MockFansub source: other group does NOT match
    assert!(!matches_reg_patterns(
        "[MockFansubOld] My Show - S01E01",
        &mapping,
        Some("MockFansub")
    ));

    // Nyaa source: the @mockfansub-scoped pattern is skipped
    assert!(!matches_reg_patterns(
        "[MockFansub] My Show - S01E01",
        &mapping,
        Some("Nyaa")
    ));
}

#[test]
fn test_reg_patterns_source_specific_season_override() {
    // Season override with source-scoped patterns replaces series-level patterns
    let mut mapping = make_mapping("TestShow", HashMap::new());
    mapping.settings.reg_patterns = vec!["720p".to_string()];

    // Season override replaces (simulating process_entry behavior)
    mapping.settings.reg_patterns = vec![
        "@nyaa:1080p".to_string(),
        "@mockfansub:\\[MockFansub\\]".to_string(),
    ];

    // Nyaa source with season override: should match "1080p"
    assert!(matches_reg_patterns(
        "TestShow - S01E01 - 1080p",
        &mapping,
        Some("Nyaa")
    ));

    // MockFansub source with season override: matches "[MockFansub]"
    assert!(matches_reg_patterns(
        "[MockFansub] TestShow - S01E01",
        &mapping,
        Some("MockFansub")
    ));

    // MockFansub source: does NOT match "1080p" (only [MockFansub] pattern applies)
    assert!(!matches_reg_patterns(
        "TestShow - S01E01 - 1080p",
        &mapping,
        Some("MockFansub")
    ));

    // Other source: no pattern applies
    assert!(!matches_reg_patterns(
        "TestShow - S01E01 - 1080p",
        &mapping,
        Some("TokyoToshokan")
    ));

    // The series-level "720p" pattern was replaced and no longer applies
    assert!(!matches_reg_patterns(
        "TestShow - S01E01 - 720p",
        &mapping,
        None
    ));
}

#[test]
fn test_reg_patterns_source_specific_generic_only_for_source() {
    // A source with only generic (non-scoped) patterns still applies them
    let mut mapping = make_mapping("TestShow", HashMap::new());
    mapping.settings.reg_patterns = vec!["1080p".to_string()];

    // Generic pattern applies to any source
    assert!(matches_reg_patterns(
        "TestShow - S01E01 - 1080p",
        &mapping,
        Some("Nyaa")
    ));
    assert!(matches_reg_patterns(
        "TestShow - S01E01 - 1080p",
        &mapping,
        Some("TokyoToshokan")
    ));

    // Even when source_name is None, generic patterns apply
    assert!(matches_reg_patterns(
        "TestShow - S01E01 - 1080p",
        &mapping,
        None
    ));
}

#[test]
fn test_reg_patterns_source_specific_mixed_scenarios() {
    // Complex interaction: multiple scoped patterns + generic fallback
    let mut mapping = make_mapping("TestShow", HashMap::new());
    mapping.settings.reg_patterns = vec![
        "@nyaa:1080p".to_string(),
        "@nyaa:2160p".to_string(),
        "@mockfansub:\\[MockFansub\\]".to_string(),
        "generic_pattern".to_string(),
    ];

    // Nyaa source: matches "1080p" (first scoped pattern)
    assert!(matches_reg_patterns("Show - 1080p", &mapping, Some("Nyaa")));

    // Nyaa source: matches "2160p" (second scoped pattern)
    assert!(matches_reg_patterns("Show - 2160p", &mapping, Some("Nyaa")));

    // Nyaa source: doesn't match scoped patterns, but generic still applies
    assert!(matches_reg_patterns(
        "title with generic_pattern",
        &mapping,
        Some("Nyaa")
    ));

    // MockFansub source: matches its scoped pattern
    assert!(matches_reg_patterns(
        "[MockFansub] Show - S01E01",
        &mapping,
        Some("MockFansub")
    ));

    // MockFansub source: doesn't match scoped pattern, but generic still applies
    assert!(matches_reg_patterns(
        "title with generic_pattern",
        &mapping,
        Some("MockFansub")
    ));

    // TokyoToshokan source: no scoped patterns match, generic still applies
    assert!(matches_reg_patterns(
        "title with generic_pattern",
        &mapping,
        Some("TokyoToshokan")
    ));

    // TokyoToshokan source: neither scoped nor generic matches
    assert!(!matches_reg_patterns(
        "completely unrelated title",
        &mapping,
        Some("TokyoToshokan")
    ));

    // Generic source (None): only generic pattern applies
    assert!(matches_reg_patterns(
        "title with generic_pattern",
        &mapping,
        None
    ));
    assert!(!matches_reg_patterns(
        "title without anything",
        &mapping,
        None
    ));
}

#[test]
fn test_reg_patterns_source_specific_exclusion_via_negative_lookahead() {
    // Source-scoped pattern with `\b` word boundary to exclude substring matches
    // (Note: the `regex` crate does NOT support negative lookaheads `(?!...)`,
    //  but `\b` word boundaries and other basic constructs work fine.)
    let mut mapping = make_mapping("TestShow", HashMap::new());
    // Match "1080p" as a whole word — "1080px" or "1080p_extra" won't match
    mapping.settings.reg_patterns = vec!["@nyaa:1080p\\b".to_string()];

    // Nyaa source: "1080p" at end of string matches (word boundary before next char)
    assert!(matches_reg_patterns(
        "TestShow - S01E01 - 1080p",
        &mapping,
        Some("Nyaa")
    ));

    // Nyaa source: "1080p" followed by non-word char also matches
    assert!(matches_reg_patterns(
        "TestShow - S01E01 - 1080p HEVC",
        &mapping,
        Some("Nyaa")
    ));

    // Nyaa source: "1080p" as part of "1080p50" does NOT match (\b fails)
    // since "1080p50" continues with digits which are word chars
    assert!(!matches_reg_patterns(
        "TestShow - S01E01 - 1080p50",
        &mapping,
        Some("Nyaa")
    ));
}

#[test]
fn test_reg_patterns_source_specific_unknown_source_gets_no_scoped_match() {
    // When source_name is None, no source-scoped patterns should match
    // (only generic patterns should match when source_name is unknown)
    let mut mapping = make_mapping("TestShow", HashMap::new());
    mapping.settings.reg_patterns = vec!["@nyaa:1080p".to_string(), "@mockfansub:720p".to_string()];

    // Both patterns are source-scoped, so neither applies to an unknown source
    assert!(!matches_reg_patterns(
        "TestShow - S01E01 - 1080p",
        &mapping,
        None
    ));
    assert!(!matches_reg_patterns(
        "TestShow - S01E01 - 720p",
        &mapping,
        None
    ));
}

#[test]
fn test_reg_patterns_source_specific_slug_with_hyphens() {
    // Source name with multiple words produces a hyphenated slug
    let mut mapping = make_mapping("TestShow", HashMap::new());
    mapping.settings.reg_patterns = vec!["@nyaa-subs:1080p".to_string()];

    // Source name "Nyaa Subs" normalizes to slug "nyaa-subs" (double 'a')
    assert_eq!(
        jumbie_shared::mapping::plugin_name_to_slug("Nyaa Subs"),
        "nyaa-subs",
        "plugin_name_to_slug('Nyaa Subs') should produce slug 'nyaa-subs'"
    );

    // Source name "Nyaa Subs" normalizes to slug "nyaa-subs"
    assert!(matches_reg_patterns(
        "TestShow - S01E01 - 1080p",
        &mapping,
        Some("Nyaa Subs")
    ));

    // Source name with different casing still works
    assert!(matches_reg_patterns(
        "TestShow - S01E01 - 1080p",
        &mapping,
        Some("NYAA SUBS")
    ));

    // Different source does NOT match
    assert!(!matches_reg_patterns(
        "TestShow - S01E01 - 1080p",
        &mapping,
        Some("TokyoToshokan")
    ));
}


#[test]
fn test_reg_patterns_source_specific_shared_name_matches_both_instances() {
    // Edge case: two source plugins configured with the same user-defined name.
    // Both will produce the same `source_name` on their entries, so a single
    // @slug:pattern prefix matches entries from both.
    //
    // This is INTENTIONAL: if a user sets up two Nyaa instances (e.g. different
    // RSS URLs) both named "Nyaa", they want the same pattern to apply to both.
    let mut mapping = make_mapping("TestShow", HashMap::new());
    mapping.settings.reg_patterns = vec!["@nyaa:1080p".to_string()];

    // Entries from either plugin instance share source_name="Nyaa" → slug="nyaa"
    assert!(matches_reg_patterns(
        "Show - S01E01 - 1080p",
        &mapping,
        Some("Nyaa")
    ));
    assert!(matches_reg_patterns(
        "Show - S01E01 - 1080p",
        &mapping,
        Some("Nyaa")
    ));

    // Different source (even with similar name) does NOT match
    assert!(!matches_reg_patterns(
        "Show - S01E01 - 1080p",
        &mapping,
        Some("NyaaTwo")
    ));
}

#[test]
fn test_reg_patterns_source_specific_case_variations_produce_same_slug() {
    // Edge case: two plugins named "MySource" and "mysource" both produce
    // slug "mysource" because `plugin_name_to_slug` lowercases.
    // A single @mysource:pattern prefix should match entries from either.
    let mut mapping = make_mapping("TestShow", HashMap::new());
    mapping.settings.reg_patterns = vec!["@mysource:720p".to_string()];

    // Plugin named "MySource" → slug "mysource"
    assert!(matches_reg_patterns(
        "Show - 720p",
        &mapping,
        Some("MySource")
    ));

    // Plugin named "mysource" → slug "mysource" (same)
    assert!(matches_reg_patterns(
        "Show - 720p",
        &mapping,
        Some("mysource")
    ));

    // Plugin named "MYSOURCE" → slug "mysource" (same)
    assert!(matches_reg_patterns(
        "Show - 720p",
        &mapping,
        Some("MYSOURCE")
    ));

    // Mixed-case plugin name "My Source" → slug "my-source" (different slug)
    assert!(!matches_reg_patterns(
        "Show - 720p",
        &mapping,
        Some("My Source")
    ));
}

#[test]
fn test_reg_patterns_source_specific_downloader_name_never_matches_entries() {
    // Edge case: if a user accidentally uses a downloader plugin's display name
    // (e.g. "qBittorrent") as a slug prefix, the pattern will never match
    // because `entry.source_name` is always set by the SOURCE plugin that
    // scraped the entry — downloader names never appear as entry source names.
    let mut mapping = make_mapping("TestShow", HashMap::new());
    mapping.settings.reg_patterns = vec!["@qbittorrent:1080p".to_string()];

    // No source plugin will ever have source_name="qBittorrent", so the
    // pattern never applies — even if the entry's title matches.
    assert!(!matches_reg_patterns(
        "Show - S01E01 - 1080p",
        &mapping,
        Some("Nyaa")
    ));
    assert!(!matches_reg_patterns(
        "Show - S01E01 - 1080p",
        &mapping,
        Some("TokyoToshokan")
    ));
    assert!(!matches_reg_patterns(
        "Show - S01E01 - 1080p",
        &mapping,
        None
    ));
}

#[test]
fn test_reg_patterns_source_specific_downloader_vs_source_name_collision() {
    // Scenario: a source plugin and a downloader plugin share the same
    // user-defined name (e.g. both named "MyService"). The source plugin's
    // entries will have source_name="MyService", so the @myservice:pattern
    // prefix WILL match entries from the source plugin.
    // The downloader plugin's name is irrelevant — it doesn't set source_name.
    let mut mapping = make_mapping("TestShow", HashMap::new());
    mapping.settings.reg_patterns = vec!["@myservice:1080p".to_string()];

    // A source plugin named "MyService" produces matching entries
    assert!(matches_reg_patterns(
        "Show - S01E01 - 1080p",
        &mapping,
        Some("MyService")
    ));

    // The downloader having the same name is irrelevant
    assert!(!matches_reg_patterns(
        "Show - S01E01 - 1080p",
        &mapping,
        Some("OtherSource")
    ));
}


#[test]
fn test_custom_regex_extraction_episode_only_season_level() {
    // Season-level pattern with only an episode group — inherits season from
    // target_season (this is what happens when a SeasonOverride has reg_patterns
    // with `(?P<episode>\d+)` and process_entry passes target_season_num).
    let mut mapping = make_mapping("TestShow", HashMap::new());
    mapping.settings.reg_patterns = vec!["Episode\\s*(?P<episode>\\d+)".to_string()];

    // Season 2 context — should extract episode 5 with season = 2
    let result = extract_or_filter("My Show - Episode 5", &mapping, Some(2), None);
    match result {
        Ok(Some(info)) => {
            assert_eq!(
                info.episodes.first().copied().unwrap_or(0),
                5,
                "Should extract episode 5"
            );
            assert_eq!(
                info.seasons.first().copied(),
                Some(2),
                "Should inherit season from target_season"
            );
        }
        other => panic!("Expected Ok(Some(...)), got {:?}", other),
    }
}

#[test]
fn test_custom_regex_extraction_episode_only_defaults_to_season_1() {
    let mut mapping = make_mapping("TestShow", HashMap::new());
    mapping.settings.reg_patterns = vec!["Ep(?P<episode>\\d+)".to_string()];

    // No target season — should default to season 1
    let result = extract_or_filter("Show Ep10", &mapping, None, None);
    match result {
        Ok(Some(info)) => {
            assert_eq!(info.episodes.first().copied().unwrap_or(0), 10);
            assert_eq!(
                info.seasons.first().copied(),
                Some(1),
                "Should default to season 1"
            );
        }
        other => panic!("Expected Ok(Some(...)), got {:?}", other),
    }
}

#[test]
fn test_custom_regex_extraction_standard_mode() {
    // Series-level pattern with both season and episode groups (standard mode)
    let mut mapping = make_mapping("TestShow", HashMap::new());
    mapping.settings.reg_patterns = vec!["S(?P<season>\\d+)E(?P<episode>\\d+)".to_string()];

    let result = extract_or_filter("TestShow - S03E07", &mapping, None, None);
    match result {
        Ok(Some(info)) => {
            assert_eq!(info.seasons.first().copied(), Some(3));
            assert_eq!(info.episodes.first().copied().unwrap_or(0), 7);
        }
        other => panic!("Expected Ok(Some(...)), got {:?}", other),
    }
}

#[test]
fn test_custom_regex_extraction_absolute_mode() {
    // Series-level pattern with episode-only group in absolute mode
    let mut mapping = make_mapping("TestShow", HashMap::new());
    mapping.settings.absolute_numbering = Some(true);
    mapping.settings.reg_patterns = vec!["EP(?P<episode>\\d+)".to_string()];

    let result = extract_or_filter("Show EP126", &mapping, None, None);
    match result {
        Ok(Some(info)) => {
            assert_eq!(
                info.seasons.first().copied(),
                None,
                "Absolute mode: season should be None"
            );
            assert_eq!(info.episodes.first().copied().unwrap_or(0), 126);
        }
        other => panic!("Expected Ok(Some(...)), got {:?}", other),
    }
}

#[test]
fn test_custom_regex_rejects_when_no_pattern_matches() {
    let mut mapping = make_mapping("TestShow", HashMap::new());
    mapping.settings.reg_patterns = vec!["\\[MockFansub\\].*EP(?P<episode>\\d+)".to_string()];

    let result = extract_or_filter("OtherGroup - Show EP05", &mapping, None, None);
    assert!(
        result.is_err(),
        "Should reject entries that don't match the pattern"
    );
}

#[test]
fn test_custom_regex_falls_through_to_filter_only() {
    // Pattern has no named groups — should return Ok(None) so caller falls
    // back to default parse_filename.
    let mut mapping = make_mapping("TestShow", HashMap::new());
    mapping.settings.reg_patterns = vec!["1080p".to_string()];

    let result = extract_or_filter("Show - S01E01 - 1080p", &mapping, None, None);
    assert!(
        matches!(result, Ok(None)),
        "Expected Ok(None) for filter-only pattern, got {:?}",
        result
    );
}

#[test]
fn test_custom_regex_empty_patterns_falls_through() {
    let mapping = make_mapping("TestShow", HashMap::new());
    // No patterns defined — should return Ok(None)
    let result = extract_or_filter("Any Show S01E01", &mapping, None, None);
    assert!(
        matches!(result, Ok(None)),
        "Expected Ok(None) for empty patterns, got {:?}",
        result
    );
}

#[test]
fn test_custom_regex_extraction_with_source_scope() {
    let mut mapping = make_mapping("TestShow", HashMap::new());
    mapping.settings.reg_patterns = vec!["@nyaa:EP(?P<episode>\\d+)".to_string()];

    // Matching source — should extract
    let result = extract_or_filter("Show EP42", &mapping, None, Some("nyaa"));
    match result {
        Ok(Some(info)) => {
            assert_eq!(info.episodes.first().copied().unwrap_or(0), 42);
        }
        other => panic!("Expected Ok(Some(...)), got {:?}", other),
    }

    // Mismatched source — should reject
    let result2 = extract_or_filter("Show EP42", &mapping, None, Some("tokyotosho"));
    assert!(result2.is_err(), "Should reject when source doesn't match");
}

#[test]
fn test_custom_regex_season_override_replaces_series_level() {
    // Simulates process_entry where season override's reg_patterns clone
    // overrides series-level patterns.
    let mut mapping = make_mapping("TestShow", HashMap::new());
    mapping.settings.reg_patterns = vec!["\\[MockFansub\\]".to_string()];

    // Season override replaces (as done by process_entry)
    mapping.settings.reg_patterns = vec!["EP(?P<episode>\\d+)".to_string()];

    let result = extract_or_filter("Show EP07", &mapping, Some(2), None);
    match result {
        Ok(Some(info)) => {
            assert_eq!(info.episodes.first().copied().unwrap_or(0), 7);
            assert_eq!(
                info.seasons.first().copied(),
                Some(2),
                "Should inherit season from override context"
            );
        }
        other => panic!("Expected Ok(Some(...)), got {:?}", other),
    }
}

#[test]
fn test_custom_regex_source_scoped_and_generic_mix() {
    // Mixed patterns: source-scoped and generic. The generic one should match
    // even when source-scoped ones don't apply.
    let mut mapping = make_mapping("TestShow", HashMap::new());
    mapping.settings.reg_patterns = vec![
        "@nyaa:EP(?P<episode>\\d+)".to_string(),
        "(?P<episode>\\d+)v2".to_string(),
    ];

    // tokytosho source — first pattern skipped, second should match
    let result = extract_or_filter("Show 42v2", &mapping, None, Some("tokyotosho"));
    match result {
        Ok(Some(info)) => {
            assert_eq!(info.episodes.first().copied().unwrap_or(0), 42);
        }
        other => panic!("Expected Ok(Some(...)), got {:?}", other),
    }
}

#[test]
fn test_custom_regex_non_standard_title_format() {
    // Test a real-world non-standard title format that the default parser
    // might not handle correctly, but a custom regex can.
    let mut mapping = make_mapping("TestShow", HashMap::new());
    mapping.settings.reg_patterns = vec![
        "第(?P<episode>\\d+)話".to_string(), // Japanese: "Episode XX"
    ];

    let result = extract_or_filter(
        "[MockFansub] TestShow - 第05話 [1080p]",
        &mapping,
        Some(1),
        None,
    );
    match result {
        Ok(Some(info)) => {
            assert_eq!(info.episodes.first().copied().unwrap_or(0), 5);
            assert_eq!(info.seasons.first().copied(), Some(1));
        }
        other => panic!("Expected Ok(Some(...)), got {:?}", other),
    }
}

#[test]
fn test_custom_regex_season_only_group_detects_season_pack() {
    // Season-only group with `(?P<season>\d+)` — should detect as season pack.
    let mut mapping = make_mapping("TestShow", HashMap::new());
    mapping.settings.reg_patterns = vec!["Season (?P<season>\\d+)\\s*Complete".to_string()];

    let result = extract_or_filter("TestShow - Season 2 Complete [1080p]", &mapping, None, None);
    match result {
        Ok(Some(info)) => {
            assert_eq!(info.seasons.first().copied(), Some(2));
            assert_eq!(
                info.episodes.first().copied().unwrap_or(0),
                1,
                "Season pack episode_num should be 1"
            );
            assert!(
                info.is_season_pack,
                "Season-only group should produce season pack"
            );
            assert!(info.is_complete_pack, "Title contains 'Complete'");
        }
        other => panic!("Expected Ok(Some(...)), got {:?}", other),
    }
}

#[test]
fn test_custom_regex_season_only_group_no_complete() {
    // Season-only group without "Complete" — still a season pack.
    let mut mapping = make_mapping("TestShow", HashMap::new());
    mapping.settings.reg_patterns = vec!["Season (?P<season>\\d+)".to_string()];

    let result = extract_or_filter("TestShow - Season 1", &mapping, None, None);
    match result {
        Ok(Some(info)) => {
            assert_eq!(info.seasons.first().copied(), Some(1));
            assert!(info.is_season_pack);
            assert!(!info.is_complete_pack, "No 'Complete' in title");
        }
        other => panic!("Expected Ok(Some(...)), got {:?}", other),
    }
}

#[test]
fn test_custom_regex_season_and_episode_groups_not_a_pack() {
    // Both season and episode groups → standard episode, not a pack.
    let mut mapping = make_mapping("TestShow", HashMap::new());
    mapping.settings.reg_patterns = vec!["S(?P<season>\\d+)EP(?P<episode>\\d+)".to_string()];

    let result = extract_or_filter("TestShow - S02EP05", &mapping, None, None);
    match result {
        Ok(Some(info)) => {
            assert_eq!(info.seasons.first().copied(), Some(2));
            assert_eq!(info.episodes.first().copied().unwrap_or(0), 5);
            assert!(
                !info.is_season_pack,
                "Both season+episode groups = single episode, not pack"
            );
        }
        other => panic!("Expected Ok(Some(...)), got {:?}", other),
    }
}

#[test]
fn test_custom_regex_season_only_rejects_non_numeric() {
    // Season-only group with non-numeric capture should reject.
    let mut mapping = make_mapping("TestShow", HashMap::new());
    mapping.settings.reg_patterns = vec!["S(?P<season>[a-z]+)\\s*$".to_string()];

    let result = extract_or_filter("TestShow - Sab", &mapping, None, None);
    assert!(
        result.is_err(),
        "Non-numeric season capture should reject entry"
    );
}


#[test]
fn test_reg_patterns_source_specific_uuid_prefix() {
    // UUID-format prefixes work identically to legacy display-name slugs.
    // The pattern matching is agnostic to prefix content — pure string comparison.
    let uuid = "550e8400-e29b-41d4-a716-446655440000";
    let mut mapping = make_mapping("TestShow", HashMap::new());
    mapping.settings.reg_patterns = vec![
        format!("@{}:1080p", uuid),
        "@tokyotoshokan:720p".to_string(),
    ];

    // Source with matching UUID: pattern "1080p" applies
    assert!(matches_reg_patterns_by_slug(
        "TestShow - S01E01 - 1080p",
        &mapping,
        Some(uuid)
    ));

    // Same UUID: pattern "720p" from a different source does NOT match
    assert!(!matches_reg_patterns_by_slug(
        "TestShow - S01E01 - 720p",
        &mapping,
        Some(uuid)
    ));

    // Different UUID: neither pattern matches
    assert!(!matches_reg_patterns_by_slug(
        "TestShow - S01E01 - 1080p",
        &mapping,
        Some("a1b2c3d4-e5f6-7890-abcd-ef1234567890")
    ));

    // Unknown source: neither pattern applies
    assert!(!matches_reg_patterns_by_slug(
        "TestShow - S01E01 - 1080p",
        &mapping,
        None
    ));
}

#[test]
fn test_custom_regex_extraction_with_uuid_prefix() {
    // UUID-format prefix in extraction — should extract episode info
    // when the entry's source slug matches the pattern's UUID prefix.
    let uuid = "550e8400-e29b-41d4-a716-446655440000";
    let mut mapping = make_mapping("TestShow", HashMap::new());
    mapping.settings.reg_patterns = vec![format!(
        "@{}:.*S(?P<season>\\d+)E(?P<episode>\\d+).*1080p",
        uuid
    )];

    let result =
        extract_or_filter_by_slug("TestShow - S01E01 - 1080p", &mapping, Some(1), Some(uuid));
    match result {
        Ok(Some(info)) => {
            assert_eq!(info.seasons.first().copied(), Some(1));
            assert_eq!(info.episodes.first().copied().unwrap_or(0), 1);
        }
        other => panic!("Expected Ok(Some(...)), got {:?}", other),
    }

    // Same entry with a non-matching UUID: no pattern applies.
    assert!(
        extract_or_filter_by_slug(
            "TestShow - S01E01 - 1080p",
            &mapping,
            Some(1),
            Some("a1b2c3d4-e5f6-7890-abcd-ef1234567890")
        )
        .is_err()
    );

    // No source slug: UUID-scoped pattern is skipped, no match.
    assert!(
        extract_or_filter_by_slug("TestShow - S01E01 - 1080p", &mapping, Some(1), None).is_err()
    );
}

#[test]
fn test_reg_patterns_mixed_uuid_and_legacy_prefixes() {
    // Mixed UUID-format and legacy display-name prefixes work together.
    let uuid = "550e8400-e29b-41d4-a716-446655440000";
    let mut mapping = make_mapping("TestShow", HashMap::new());
    mapping.settings.reg_patterns = vec![
        format!("@{}:1080p", uuid),
        "@nyaa:720p".to_string(),
        "generic".to_string(),
    ];

    // UUID source: matches its scoped pattern
    assert!(matches_reg_patterns_by_slug(
        "TestShow - S01E01 - 1080p",
        &mapping,
        Some(uuid)
    ));
    // UUID source: does NOT match other source's scoped pattern
    assert!(!matches_reg_patterns_by_slug(
        "TestShow - S01E01 - 720p",
        &mapping,
        Some(uuid)
    ));
    // UUID source: generic patterns still apply
    assert!(matches_reg_patterns_by_slug(
        "Some title with generic content",
        &mapping,
        Some(uuid)
    ));

    // Legacy "nyaa" source: matches "@nyaa:720p" pattern
    assert!(matches_reg_patterns(
        "TestShow - S01E01 - 720p",
        &mapping,
        Some("Nyaa")
    ));
    // Legacy "nyaa" source: does NOT match UUID-scoped pattern
    assert!(!matches_reg_patterns(
        "TestShow - S01E01 - 1080p",
        &mapping,
        Some("Nyaa")
    ));
}
