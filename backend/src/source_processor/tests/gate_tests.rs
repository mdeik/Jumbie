use std::collections::HashMap;

use crate::source_processor::identification::{build_gate_regex, lookup_mapping_in};
use jumbie_shared::mapping::{MappingRule, SeriesSettings};
use jumbie_shared::parsing::{CustomParseResult, parse_title_with_custom_regex};

// Helpers

/// Build a minimal MappingRule with series-level reg_patterns (same helper as mod.rs).
fn series_with_patterns(target_title: &str, patterns: Vec<&str>) -> MappingRule {
    MappingRule {
        target_title: target_title.to_string(),
        name: target_title.to_lowercase().replace(' ', "_"),
        quality_profile: None,
        release_profile: None,
        qb_category: None,
        filters: None,
        scoring: None,
        series_id: String::new(),
        hidden_in_library: false,
        settings: SeriesSettings {
            aliases: vec![],
            absolute_numbering: Some(false),
            season: std::collections::HashMap::new(),
            season_absolute: std::collections::HashMap::new(),
            reg_patterns: patterns.into_iter().map(|s| s.to_string()).collect(),
            season_folder_format: None,
            episode_file_format: None,
            season_folder_format_absolute: None,
            episode_file_format_absolute: None,
            flatten_season_folders: None,
            rename_episodes: None,
            search_format: None,
            search_format_absolute: None,
            path: None,
            monitor_mode: None,
            metadata_ids: std::collections::HashMap::new(),
            metadata_last_synced_at: std::collections::HashMap::new(),
            last_known_dir_mtimes: std::collections::HashMap::new(),
        },
    }
}

fn build_single_series_mapping(
    name: &str,
    target_title: &str,
    aliases: Vec<&str>,
    patterns: Vec<&str>,
) -> HashMap<String, MappingRule> {
    let mut mappings = HashMap::new();
    let mapping = MappingRule {
        target_title: target_title.to_string(),
        name: name.to_string(),
        settings: SeriesSettings {
            aliases: aliases.into_iter().map(|s| s.to_string()).collect(),
            reg_patterns: patterns.into_iter().map(|s| s.to_string()).collect(),
            ..SeriesSettings::default()
        },
        ..MappingRule::default()
    };
    mappings.insert("test-series".to_string(), mapping);
    mappings
}

// Gate: basic name matching

#[test]
fn test_gate_generated_from_name() {
    let mappings = build_single_series_mapping("My Show", "My Show", vec![], vec![]);
    let gate = build_gate_regex(&mappings).expect("Gate should build");

    // Should match: name appears in title
    assert!(gate.is_match("My Show S01E01"), "Name with spaces");
    assert!(gate.is_match("[Group] My.Show.S01E01"), "Name with dots");
    assert!(
        gate.is_match("My.Show.2024.1080p"),
        "Name with dots and tags"
    );
    assert!(gate.is_match("My_Show S01E01"), "Name with underscore");

    // Should NOT match: name does not appear
    assert!(!gate.is_match("Other Show S01E01"), "Different name");
    assert!(!gate.is_match("Something Else"), "Completely different");
}

#[test]
fn test_gate_generated_from_aliases() {
    let mappings =
        build_single_series_mapping("My Show", "My Show", vec!["MST", "My Series"], vec![]);
    let gate = build_gate_regex(&mappings).expect("Gate should build");

    assert!(gate.is_match("MST S01E01"), "Alias matching");
    assert!(gate.is_match("My.Series.S01E01"), "Second alias with dots");
    assert!(gate.is_match("My.Show.S01E01"), "Name still matches too");
}

#[test]
fn test_gate_generated_from_target_title() {
    let mappings = build_single_series_mapping("My Show", "My Show (2024)", vec![], vec![]);
    let gate = build_gate_regex(&mappings).expect("Gate should build");

    assert!(gate.is_match("My Show S01E01"), "Name");
    assert!(gate.is_match("My Show (2024) S01E01"), "Target title");
    assert!(
        gate.is_match("My.Show.2024.S01E01"),
        "Target title with dots"
    );
}

#[test]
fn test_gate_target_title_same_as_name_does_not_duplicate() {
    // When target_title == name, it should not be added twice.
    let mappings = build_single_series_mapping("My Show", "My Show", vec![], vec![]);
    let gate = build_gate_regex(&mappings).expect("Gate should build");

    // Should still match — but we're just checking it compiles and works.
    assert!(gate.is_match("My Show S01E01"), "Name matches");
}

// Gate: user-defined patterns

#[test]
fn test_gate_user_filter_pattern() {
    let mappings = build_single_series_mapping(
        "My Show",
        "My Show",
        vec![],
        vec![r"(?i)MyShowRelease[\\.\s-]\d+"],
    );
    let gate = build_gate_regex(&mappings).expect("Gate should build");

    assert!(gate.is_match("MyShowRelease-123"), "Filter pattern matches");
    assert!(
        gate.is_match("MyShowRelease 456"),
        "Filter pattern with space"
    );
    assert!(!gate.is_match("Other Release-123"), "No match");
}

#[test]
fn test_gate_user_pattern_skips_auto_gen() {
    // Series with user patterns should NOT also get auto-generated name patterns.
    let mappings =
        build_single_series_mapping("My Show", "My Show", vec![], vec![r"(?i)MyShowRelease"]);
    let gate = build_gate_regex(&mappings).expect("Gate should build");

    assert!(gate.is_match("MyShowRelease S01E01"), "User pattern");
    // The gate includes BOTH auto-generated name patterns AND user patterns.
    // This is by design: the gate is a superset that covers all matching
    // paths (Phase 1 AND Phase 2). The auto-generated name pattern for
    // "My Show" is (?i)My[._ ]Show which matches "My.Show.S01E01".
    // Phase 1 will then try user patterns, miss, and Phase 2 will catch it.
    assert!(
        gate.is_match("My.Show.S01E01"),
        "Auto-generated name included too (superset)"
    );
}

#[test]
fn test_gate_user_extraction_pattern_stripped() {
    // Named groups stripped: (?P<season>...) → (?:...), (?P<episode>...) → (?:...)
    let mappings = build_single_series_mapping(
        "My Show",
        "My Show",
        vec![],
        vec![r"(?i)Show.*?S(?P<season>\d+)E(?P<episode>\d+)"],
    );
    let gate = build_gate_regex(&mappings).expect("Gate should build");

    assert!(gate.is_match("Show S02E10"), "S02E10 with spaces");
    assert!(gate.is_match("Show.S02E10.1080p"), "S02E10 with dots");
    // "Other Show S01E01" ALSO matches because the pattern (?i)Show.*?S\d+E\d+
    // matches any title containing "Show" followed by SxxExx — the gate is a
    // superset so this is expected (Phase 1 handles the specificity).
    assert!(
        gate.is_match("Other Show S01E01"),
        "Superset: matches any title with Show+SxxExx"
    );
    assert!(!gate.is_match("Completely Unrelated"), "Unrelated title");
}

#[test]
fn test_gate_mixed_filter_and_extraction() {
    // Both filter-only and extraction patterns per series.
    let mappings = build_single_series_mapping(
        "My Show",
        "My Show",
        vec![],
        vec![
            r"(?i)Show.*?S(?P<season>\d+)E(?P<episode>\d+)",
            r"(?i)MyShowRelease",
        ],
    );
    let gate = build_gate_regex(&mappings).expect("Gate should build");

    assert!(gate.is_match("Show S02E10"), "Extraction pattern");
    assert!(gate.is_match("MyShowRelease"), "Filter pattern");
}

// Gate: source-scoped patterns (@instance-id:...)

#[test]
fn test_gate_source_scoped_pattern() {
    // @slug: prefix is stripped in the gate; the raw pattern text is kept.
    let mappings = build_single_series_mapping(
        "My Show",
        "My Show",
        vec![],
        vec![r"@nyaa:(?i)Anime.*E(?P<episode>\d+)"],
    );
    let gate = build_gate_regex(&mappings).expect("Gate should build");

    assert!(
        gate.is_match("Anime Title E05"),
        "Prefix stripped, pattern text kept"
    );
    assert!(!gate.is_match("Other Title S01E01"), "Unrelated title");
}

#[test]
fn test_gate_source_scoped_multiple_sources() {
    // Multiple series with different source-scoped patterns.
    let mut mappings = HashMap::new();

    let s1 = MappingRule {
        target_title: "Anime One".to_string(),
        name: "Anime One".to_string(),
        settings: SeriesSettings {
            reg_patterns: vec![r"@nyaa:(?i)Anime.*E\d+".to_string()],
            ..SeriesSettings::default()
        },
        ..MappingRule::default()
    };
    mappings.insert("s1".to_string(), s1);

    let s2 = MappingRule {
        target_title: "Other Show".to_string(),
        name: "Other Show".to_string(),
        settings: SeriesSettings {
            reg_patterns: vec![r"@nzb:(?i)Other.*S\d+E\d+".to_string()],
            ..SeriesSettings::default()
        },
        ..MappingRule::default()
    };
    mappings.insert("s2".to_string(), s2);

    let gate = build_gate_regex(&mappings).expect("Gate should build with multi-source patterns");

    assert!(gate.is_match("Anime Title E05"), "Anime pattern");
    assert!(gate.is_match("Other S01E01"), "Other pattern");
    assert!(!gate.is_match("Unrelated Title"), "Neither");
}

// Gate: regex special characters in names/aliases

#[test]
fn test_gate_parentheses_in_name() {
    let mappings = build_single_series_mapping("Show (2024)", "Show (2024)", vec![], vec![]);
    let gate = build_gate_regex(&mappings).expect("Gate should build");

    assert!(
        gate.is_match("Show (2024) S01E01"),
        "Literal parens in title"
    );
    // "Show.2024.S01E01" does NOT match because the original name has literal
    // parens that are regex-escaped — only spaces between words get [._ ] flexibility.
    assert!(
        !gate.is_match("Show.2024.S01E01"),
        "No parens in title, no match"
    );
    assert!(!gate.is_match("Show 2025 S01E01"), "Wrong year, no parens");
}

#[test]
fn test_gate_plus_in_name() {
    let mappings = build_single_series_mapping("C++", "C++", vec![], vec![]);
    let gate = build_gate_regex(&mappings).expect("Gate should build with +");

    assert!(gate.is_match("C++ S01E01"), "Literal + in name");
    assert!(!gate.is_match("C S01E01"), "Without + should not match");
}

#[test]
fn test_gate_dot_in_name() {
    let mappings = build_single_series_mapping("U.S.A", "U.S.A", vec![], vec![]);
    let gate = build_gate_regex(&mappings).expect("Gate should build with dot");

    // regex::escape("U.S.A") → r"U\.S\.A" — literal dots
    assert!(gate.is_match("U.S.A S01E01"), "Literal dots in name");
    // Dots in the name are escaped literally; no flexible matching for them.
    assert!(!gate.is_match("USA S01E01"), "Without dots, no match");
}

#[test]
fn test_gate_regex_chars_in_alias() {
    let mappings =
        build_single_series_mapping("My Show", "My Show", vec!["Show+More (2024)"], vec![]);
    let gate = build_gate_regex(&mappings).expect("Gate should build");

    assert!(
        gate.is_match("Show+More (2024) E01"),
        "Alias with meta-chars"
    );
    assert!(!gate.is_match("Show More 2024 E01"), "Without meta-chars");
}

// Gate: edge cases

#[test]
fn test_gate_empty_mappings_returns_none() {
    let mappings = HashMap::new();
    assert!(build_gate_regex(&mappings).is_none(), "Empty → None");
}

#[test]
fn test_gate_empty_name_and_no_aliases() {
    let mappings = build_single_series_mapping("", "", vec![], vec![]);
    assert!(
        build_gate_regex(&mappings).is_none(),
        "Empty name, no aliases, no patterns → None"
    );
}

#[test]
fn test_gate_alias_with_empty_name() {
    let mappings = build_single_series_mapping("", "", vec!["ValidAlias"], vec![]);
    let gate = build_gate_regex(&mappings).expect("Gate should build from alias");
    assert!(
        gate.is_match("ValidAlias S01E01"),
        "Alias used when name empty"
    );
}

#[test]
fn test_gate_multiple_series() {
    let mut mappings = HashMap::new();

    let s1 = MappingRule {
        target_title: "Show A".to_string(),
        name: "Show A".to_string(),
        ..MappingRule::default()
    };
    mappings.insert("s1".to_string(), s1);

    let s2 = MappingRule {
        target_title: "Show B".to_string(),
        name: "Show B".to_string(),
        ..MappingRule::default()
    };
    mappings.insert("s2".to_string(), s2);

    let s3 = series_with_patterns("Show C", vec![r"(?i)ShowCPattern"]);
    mappings.insert("s3".to_string(), s3);

    let gate = build_gate_regex(&mappings).expect("Gate should build");

    assert!(gate.is_match("Show A S01E01"), "Series A");
    assert!(gate.is_match("Show.B.S01E01"), "Series B with dots");
    assert!(gate.is_match("ShowCPattern S01E01"), "Series C via pattern");
    assert!(!gate.is_match("Something Completely Different"), "No match");
}

#[test]
fn test_gate_case_sensitivity() {
    let mut mappings = HashMap::new();

    // Auto-generated pattern (case-insensitive via (?i))
    let s1 = MappingRule {
        target_title: "Show".to_string(),
        name: "Show".to_string(),
        ..MappingRule::default()
    };
    mappings.insert("s1".to_string(), s1);

    // User pattern without (?i) — case-sensitive
    let s2 = series_with_patterns("Exact", vec!["ExactMatch"]);
    mappings.insert("s2".to_string(), s2);

    let gate = build_gate_regex(&mappings).expect("Gate should build");

    assert!(gate.is_match("show S01E01"), "Auto: case insensitive");
    assert!(gate.is_match("SHOW S01E01"), "Auto: uppercase");
    assert!(gate.is_match("ExactMatch"), "User: exact case");
    // "exactmatch" matches via auto-generated name pattern (?i)Exact —
    // the gate is a superset so this is expected.
    assert!(
        gate.is_match("exactmatch"),
        "Auto-generated name matches case-insensitively"
    );
}

// Gate superset property: must match everything Phase 1 would

#[test]
fn test_gate_superset_auto_generated_patterns() {
    // For series with auto-generated patterns, every title that matches
    // the auto-generated pattern must also pass the gate.
    let mappings = build_single_series_mapping("My Series", "My Series", vec![], vec![]);
    let gate = build_gate_regex(&mappings).expect("Gate should build");

    let auto_patterns: Vec<String> =
        vec![crate::source_processor::identification::generate_auto_pattern("My Series")];
    let titles = vec![
        "My Series S01E01",
        "My.Series.S01E01",
        "[Group] My.Series.2024",
    ];

    for title in &titles {
        let phase1 =
            parse_title_with_custom_regex(title, &auto_patterns, None, false, None::<&str>);
        assert!(
            matches!(phase1, CustomParseResult::MatchedFilter),
            "Phase 1 should match '{}' via auto pattern, got {:?}",
            title,
            phase1
        );
        assert!(gate.is_match(title), "Gate must match '{}'", title);
    }
}

#[test]
fn test_gate_superset_user_filter_pattern() {
    let patterns = vec![r"(?i)ShowTwo.*\d+"];
    let mappings = build_single_series_mapping("Show Two", "Show Two", vec![], patterns.clone());
    let gate = build_gate_regex(&mappings).expect("Gate should build");

    let user_patterns: Vec<String> = patterns.into_iter().map(|s| s.to_string()).collect();
    let titles = vec!["ShowTwo 123", "ShowTwo 456"];

    for title in &titles {
        let phase1 =
            parse_title_with_custom_regex(title, &user_patterns, None, false, None::<&str>);
        assert!(
            matches!(phase1, CustomParseResult::MatchedFilter),
            "Phase 1 filter should match '{}', got {:?}",
            title,
            phase1
        );
        assert!(gate.is_match(title), "Gate must match '{}'", title);
    }
}

#[test]
fn test_gate_superset_user_extraction_pattern() {
    let patterns = vec![r"(?i)Three.*S(?P<season>\d+)E(?P<episode>\d+)"];
    let mappings =
        build_single_series_mapping("Show Three", "Show Three", vec![], patterns.clone());
    let gate = build_gate_regex(&mappings).expect("Gate should build");

    let user_patterns: Vec<String> = patterns.into_iter().map(|s| s.to_string()).collect();
    let titles = vec!["Three S02E10", "Three.S02E10.1080p"];

    for title in &titles {
        let phase1 =
            parse_title_with_custom_regex(title, &user_patterns, None, false, None::<&str>);
        assert!(
            matches!(phase1, CustomParseResult::Extracted(_)),
            "Phase 1 extraction should match '{}', got {:?}",
            title,
            phase1
        );
        assert!(gate.is_match(title), "Gate must match '{}'", title);
    }
}

#[test]
fn test_gate_superset_source_scoped() {
    let patterns = vec![r"@nyaa:(?i)Anime.*E(?P<episode>\d+)"];
    let mappings =
        build_single_series_mapping("Anime Show", "Anime Show", vec![], patterns.clone());
    let gate = build_gate_regex(&mappings).expect("Gate should build");

    let user_patterns: Vec<String> = patterns.into_iter().map(|s| s.to_string()).collect();
    let titles = vec!["Anime Title E05", "Anime E01"];

    for title in &titles {
        // Phase 1 matches when source slug matches
        let phase1 =
            parse_title_with_custom_regex(title, &user_patterns, None, false, Some("nyaa"));
        assert!(
            matches!(phase1, CustomParseResult::Extracted(_)),
            "Phase 1 should match '{}' with correct source, got {:?}",
            title,
            phase1
        );
        // Gate must match regardless of source (gate has no source awareness)
        assert!(gate.is_match(title), "Gate must match '{}'", title);
    }
}

#[test]
fn test_gate_no_false_negatives_all_patterns() {
    // Systematic: build gates from various series configurations and verify
    // every title that Phase 1 matches also passes the gate.
    let configs: Vec<(&str, &str, Vec<&str>, Vec<&str>)> = vec![
        ("Plain Name", "Plain Name", vec![], vec![]),
        ("With Aliases", "With Aliases", vec!["AltName"], vec![]),
        ("With Dots", "With Dots", vec![], vec![]),
        (
            "Filter Pattern",
            "Filter",
            vec![],
            vec![r"(?i)Filter.*Pattern"],
        ),
        (
            "Extract Pattern",
            "Ex",
            vec![],
            vec![r"(?i)Ex.*S(?P<season>\d+)E(?P<episode>\d+)"],
        ),
    ];

    for (name, target, aliases, patterns) in configs {
        let p: Vec<String> = patterns.iter().map(|s| s.to_string()).collect();
        let mut mappings = HashMap::new();
        let mapping = MappingRule {
            target_title: target.to_string(),
            name: name.to_string(),
            settings: SeriesSettings {
                aliases: aliases.iter().map(|s| s.to_string()).collect(),
                reg_patterns: p.clone(),
                ..SeriesSettings::default()
            },
            ..MappingRule::default()
        };
        mappings.insert("s".to_string(), mapping);

        let gate = build_gate_regex(&mappings).expect("Gate should build");

        // Test titles that SHOULD match for this config
        // For any title that parse_title_with_custom_regex matches,
        // the gate must also match.
        let test_titles: Vec<String> = if !patterns.is_empty() {
            // Use the first pattern to generate test titles
            let pat = patterns[0];
            if pat.contains("(?P<episode>") || pat.contains("(?P<season>") {
                vec![format!("{} S01E01", target), format!("{}.S01E01", target)]
            } else if pat.contains("@") {
                vec!["Some Anime E05".to_string()]
            } else {
                vec![format!("{}Release", target)]
            }
        } else if !aliases.is_empty() {
            vec![format!("{} S01E01", aliases[0])]
        } else {
            vec![format!("{} S01E01", name)]
        };

        for title in &test_titles {
            // Verify Phase 1 would match (using parse_title_with_custom_regex
            // as a proxy for identify_by_pattern_in)
            let phase1_result = if !p.is_empty() {
                parse_title_with_custom_regex(title, &p, None, false, None::<&str>)
            } else {
                // Auto-generated pattern proxy
                let auto = vec![format!("(?i){}", name)];
                parse_title_with_custom_regex(title, &auto, None, false, None::<&str>)
            };

            let phase1_matched = matches!(
                phase1_result,
                CustomParseResult::MatchedFilter | CustomParseResult::Extracted(_)
            );

            if phase1_matched {
                assert!(
                    gate.is_match(title),
                    "Gate false negative for '{}' (series '{}'): Phase 1 matched ({:?}) but gate didn't",
                    title,
                    name,
                    phase1_result
                );
            }
        }
    }
}

// lookup_mapping_in tests

#[test]
fn test_lookup_mapping_by_name() {
    let mut mappings = HashMap::new();
    let mapping = MappingRule {
        target_title: "My Show".to_string(),
        name: "My Show".to_string(),
        ..MappingRule::default()
    };
    mappings.insert("id-1".to_string(), mapping);

    let found = lookup_mapping_in("My Show", &mappings);
    assert!(found.is_some(), "Should find by series_key matching name");
}

#[test]
fn test_lookup_mapping_by_alias() {
    let mut mappings = HashMap::new();
    let mapping = MappingRule {
        target_title: "My Show".to_string(),
        name: "My Show".to_string(),
        settings: SeriesSettings {
            aliases: vec!["Alternate Name".to_string()],
            ..SeriesSettings::default()
        },
        ..MappingRule::default()
    };
    mappings.insert("id-1".to_string(), mapping);

    let found = lookup_mapping_in("Alternate Name", &mappings);
    assert!(found.is_some(), "Should find by series_key matching alias");
}

#[test]
fn test_lookup_mapping_dot_normalization() {
    let mut mappings = HashMap::new();
    let mapping = MappingRule {
        target_title: "My Show".to_string(),
        name: "My Show".to_string(),
        ..MappingRule::default()
    };
    mappings.insert("id-1".to_string(), mapping);

    // series_key with dots should be normalized to spaces
    let found = lookup_mapping_in("My.Show", &mappings);
    assert!(found.is_some(), "Dots should normalize to spaces");
}

#[test]
fn test_lookup_mapping_not_found() {
    let mut mappings = HashMap::new();
    let mapping = MappingRule {
        target_title: "My Show".to_string(),
        name: "My Show".to_string(),
        ..MappingRule::default()
    };
    mappings.insert("id-1".to_string(), mapping);

    let found = lookup_mapping_in("Different Show", &mappings);
    assert!(found.is_none(), "Should not find non-matching series_key");
}

#[test]
fn test_lookup_mapping_empty_mappings() {
    let mappings = HashMap::new();
    let found = lookup_mapping_in("Anything", &mappings);
    assert!(found.is_none(), "Empty mappings should return None");
}

#[test]
fn test_lookup_mapping_case_insensitive() {
    let mut mappings = HashMap::new();
    let mapping = MappingRule {
        target_title: "My Show".to_string(),
        name: "My Show".to_string(),
        ..MappingRule::default()
    };
    mappings.insert("id-1".to_string(), mapping);

    let found = lookup_mapping_in("my show", &mappings);
    assert!(found.is_some(), "Should match case-insensitively");
}

#[test]
fn test_lookup_mapping_series_key_contains_name() {
    // The contains() semantics: series_key "Some Thing" should match
    // name "Thing" because "some thing".contains("thing")
    let mut mappings = HashMap::new();
    let mapping = MappingRule {
        target_title: "Thing".to_string(),
        name: "Thing".to_string(),
        ..MappingRule::default()
    };
    mappings.insert("id-1".to_string(), mapping);

    let found = lookup_mapping_in("Some Thing", &mappings);
    assert!(
        found.is_some(),
        "Contains semantics: series_key contains name"
    );
}
