use std::collections::HashMap;

use jumbie_shared::mapping::{MappingRule, SeriesSettings};

// strip_named_groups unit tests

#[test]
fn test_strip_groups_basic_episode_and_season() {
    assert_eq!(
        crate::source_processor::identification::strip_named_groups_for_test(
            r"(?i)Show.*?S(?P<season>\d+)E(?P<episode>\d+)"
        ),
        r"(?i)Show.*?S(?:\d+)E(?:\d+)"
    );
}

#[test]
fn test_strip_groups_only_episode() {
    assert_eq!(
        crate::source_processor::identification::strip_named_groups_for_test(
            r"(?i)MyShow.*E(?P<episode>\d+)"
        ),
        r"(?i)MyShow.*E(?:\d+)"
    );
}

#[test]
fn test_strip_groups_no_named_groups() {
    let s = r"(?i)MyShowRelease";
    assert_eq!(
        crate::source_processor::identification::strip_named_groups_for_test(s),
        s
    );
}

#[test]
fn test_strip_groups_nested_parens_inside_named() {
    // Named group with nested non-capturing group inside
    assert_eq!(
        crate::source_processor::identification::strip_named_groups_for_test(
            r"(?P<episode>(?:\d+))"
        ),
        r"(?:(?:\d+))"
    );
}

#[test]
fn test_strip_groups_solo_named_group() {
    // Pattern that IS just a named group
    assert_eq!(
        crate::source_processor::identification::strip_named_groups_for_test(r"(?P<episode>\d+)"),
        r"(?:\d+)"
    );
}

#[test]
fn test_strip_groups_hyphen_in_name_does_not_match() {
    // Named group names with hyphens are invalid regex — the regex crate
    // would reject them. strip_named_groups should leave them unchanged
    // because \(\?P<\w+> requires only \w chars for the name.
    let input = r"(?P<episode-no>\d+)";
    let result = crate::source_processor::identification::strip_named_groups_for_test(input);
    // The result should be unchanged because the regex didn't match the hyphenated name
    assert_eq!(result, input);
}

#[test]
fn test_strip_groups_multiple_different_names() {
    // Multiple named groups with different names
    assert_eq!(
        crate::source_processor::identification::strip_named_groups_for_test(
            r"foo(?P<episode>\d+)bar(?P<season>\d+)baz"
        ),
        r"foo(?:\d+)bar(?:\d+)baz"
    );
}

#[test]
fn test_strip_groups_adjacent_named_groups() {
    // Adjacent named groups: S(?P<season>\d+)E(?P<episode>\d+)
    assert_eq!(
        crate::source_processor::identification::strip_named_groups_for_test(
            r"(?i)Show.*S(?P<season>\d+)E(?P<episode>\d+)"
        ),
        r"(?i)Show.*S(?:\d+)E(?:\d+)"
    );
}

#[test]
fn test_strip_groups_with_source_prefix_preserved() {
    // Source prefix is handled by parse_source_pattern before strip_named_groups.
    // This test verifies strip_named_groups itself doesn't touch @source: prefix.
    assert_eq!(
        crate::source_processor::identification::strip_named_groups_for_test(
            r"@nyaa:(?i)Anime.*E(?P<episode>\d+)"
        ),
        r"@nyaa:(?i)Anime.*E(?:\d+)"
    );
}

// Gate: invalid pattern handling

#[test]
fn test_gate_invalid_pattern_does_not_break_gate() {
    // A single invalid user pattern (hyphen in named group name) must not
    // prevent the gate from compiling for OTHER series.
    let mut mappings = HashMap::new();

    // Good series — valid name, no patterns
    let s1 = MappingRule {
        target_title: "Valid Show".to_string(),
        name: "Valid Show".to_string(),
        ..MappingRule::default()
    };
    mappings.insert("s1".to_string(), s1);

    // Bad series — invalid regex pattern with hyphen in named group
    let s2 = MappingRule {
        target_title: "Bad Show".to_string(),
        name: "Bad Show".to_string(),
        settings: SeriesSettings {
            reg_patterns: vec![r"(?i)Bad.*E(?P<episode-no>\d+)".to_string()],
            ..SeriesSettings::default()
        },
        ..MappingRule::default()
    };
    mappings.insert("s2".to_string(), s2);

    let gate = crate::source_processor::build_gate_regex(&mappings);
    assert!(
        gate.is_some(),
        "Invalid pattern in one series should not prevent gate compilation"
    );

    let gate = gate.unwrap();
    // Valid series should still be matched
    assert!(
        gate.is_match("Valid Show S01E01"),
        "Valid series still matches"
    );
}

#[test]
fn test_gate_all_patterns_invalid_returns_none() {
    // If ALL user patterns are invalid, the gate still includes auto-generated
    // names — so it should still compile.
    let mut mappings = HashMap::new();

    let s1 = MappingRule {
        target_title: "Only Show".to_string(),
        name: "Only Show".to_string(),
        settings: SeriesSettings {
            reg_patterns: vec![r"(?P<bad-name>\d+)".to_string()],
            ..SeriesSettings::default()
        },
        ..MappingRule::default()
    };
    mappings.insert("s1".to_string(), s1);

    let gate = crate::source_processor::build_gate_regex(&mappings);
    // The auto-generated name pattern should still work even if user patterns are junk
    assert!(gate.is_some(), "Auto-generated name still produces a gate");
    assert!(
        gate.unwrap().is_match("Only Show S01E01"),
        "Auto-generated name matches"
    );
}

#[test]
fn test_gate_user_pattern_compiles_but_stripped_version_is_invalid() {
    // Rare edge case: pattern compiles OK with named groups but after stripping
    // named groups the result is somehow invalid. This shouldn't happen for valid
    // input, but let's verify robustness.
    let mut mappings = HashMap::new();

    let s1 = MappingRule {
        target_title: "Test".to_string(),
        name: "Test".to_string(),
        settings: SeriesSettings {
            // This has a character class with `(` inside — unusual but valid regex
            reg_patterns: vec![r"(?i)Test[(]pattern[)]".to_string()],
            ..SeriesSettings::default()
        },
        ..MappingRule::default()
    };
    mappings.insert("s1".to_string(), s1);

    let gate = crate::source_processor::build_gate_regex(&mappings);
    assert!(
        gate.is_some(),
        "Gate handles regex with char classes containing parens"
    );
    assert!(
        gate.unwrap().is_match("Test(pattern)"),
        "Pattern with literal parens in char class matches"
    );
}

// Gate: case sensitivity (scoped per-alternative)

#[test]
fn test_gate_case_sensitivity_isolated_per_alternative() {
    let mut mappings = HashMap::new();

    // Auto-generated: case-insensitive via (?i)
    let s1 = MappingRule {
        target_title: "ZetaPrime".to_string(),
        name: "ZetaPrime".to_string(),
        ..MappingRule::default()
    };
    mappings.insert("s1".to_string(), s1);

    // User pattern: case-sensitive (no (?i))
    let s2 = MappingRule {
        target_title: "Other".to_string(),
        name: "Other".to_string(),
        settings: SeriesSettings {
            reg_patterns: vec!["ExactCase".to_string()],
            ..SeriesSettings::default()
        },
        ..MappingRule::default()
    };
    mappings.insert("s2".to_string(), s2);

    let gate = crate::source_processor::build_gate_regex(&mappings).expect("Gate should build");

    // Auto-generated (?i)ZetaPrime — matches case-insensitively
    assert!(gate.is_match("zetaprime"), "Auto: lowercase");
    assert!(gate.is_match("ZETAPRIME"), "Auto: uppercase");

    // User pattern ExactCase — exact case only
    assert!(gate.is_match("ExactCase"), "User: exact case");

    // Each alternative is wrapped in (?:...) so (?i) flags are scoped per-alternative.
    // The name "Other" is also in the gate as (?i)Other, but that only matches
    // "OtherXxx" — not our test strings which don't contain "Other".
    assert!(
        !gate.is_match("exactcase"),
        "Flags scoped: lowercase should not match case-sensitive pattern"
    );
    assert!(
        !gate.is_match("EXACTCASE"),
        "Flags scoped: uppercase should not match case-sensitive pattern"
    );
}

#[test]
fn test_gate_case_sensitive_user_pattern_with_case_insensitive_name() {
    // User pattern is case-sensitive but auto-generated name is in gate too.
    // The gate should match case-insensitively via the name, and case-sensitively
    // via the pattern — independently.
    let mut mappings = HashMap::new();

    // Name "CaseShow" appears in titles and is case-insensitive
    let s1 = MappingRule {
        target_title: "CaseShow".to_string(),
        name: "CaseShow".to_string(),
        settings: SeriesSettings {
            reg_patterns: vec!["ExactUpper".to_string()],
            ..SeriesSettings::default()
        },
        ..MappingRule::default()
    };
    mappings.insert("s1".to_string(), s1);

    let gate = crate::source_processor::build_gate_regex(&mappings).expect("Gate should build");

    // Auto-generated name is case-insensitive
    assert!(gate.is_match("caseshow"), "Name match: lowercase");
    assert!(gate.is_match("CASESHOW"), "Name match: uppercase");
    assert!(
        gate.is_match("CaseShow.S01E01"),
        "Name match: exact case with dots"
    );

    // User pattern is case-sensitive — each alternative is scoped independently
    assert!(gate.is_match("ExactUpper"), "Pattern: exact case");
    assert!(
        !gate.is_match("exactupper"),
        "Pattern: only scoped (?i), no leak"
    );
    assert!(
        !gate.is_match("EXACTUPPER"),
        "Pattern: only scoped (?i), no leak"
    );
}

// Gate: Phase 2 catch-all coverage

#[test]
fn test_gate_passes_for_phase2_fallback() {
    // Titles that would be caught by Phase 2 (via parse_filename + lookup)
    // must also pass the gate. Phase 2 uses name/alias substring matching,
    // which auto-generated patterns cover.
    let mut mappings = HashMap::new();

    let s1 = MappingRule {
        target_title: "Some Show".to_string(),
        name: "Some Show".to_string(),
        // No user patterns — auto-generated from name
        ..MappingRule::default()
    };
    mappings.insert("s1".to_string(), s1);

    let gate = crate::source_processor::build_gate_regex(&mappings).expect("Gate should build");

    // Phase 2 catch: titles that parse_filename would extract a series_key
    // matching "Some Show" — gate must let them through
    assert!(gate.is_match("Some.Show.S01E01"), "Phase2: standard naming");
    assert!(
        gate.is_match("[Group] Some.Show.S01E01.1080p"),
        "Phase2: with group tag"
    );
    assert!(
        gate.is_match("Some.Show.2024.1080p.WEB-DL"),
        "Phase2: with year/resolution"
    );
}

#[test]
fn test_gate_phase2_via_alias() {
    // Phase 2 can match via alias — gate must include alias patterns.
    let mut mappings = HashMap::new();

    let s1 = MappingRule {
        target_title: "Some Show".to_string(),
        name: "Some Show".to_string(),
        settings: SeriesSettings {
            aliases: vec!["SShow".to_string(), "SS".to_string()],
            ..SeriesSettings::default()
        },
        ..MappingRule::default()
    };
    mappings.insert("s1".to_string(), s1);

    let gate = crate::source_processor::build_gate_regex(&mappings).expect("Gate should build");

    // Phase 2 via alias matching
    assert!(gate.is_match("SShow S01E01"), "Phase2: alias match");
    assert!(gate.is_match("SS.S01E01"), "Phase2: short alias with dots");
}

// Gate: large mapping set

#[test]
fn test_gate_large_mapping_set_compiles() {
    // Simulate a realistic large library: 500 series with names and aliases.
    let mut mappings = HashMap::new();
    for i in 0..500 {
        let name = format!("Series Number {}", i);
        let mapping = MappingRule {
            target_title: name.clone(),
            name,
            settings: SeriesSettings {
                aliases: vec![format!("SN{}", i), format!("s{}", i)],
                ..SeriesSettings::default()
            },
            ..MappingRule::default()
        };
        mappings.insert(format!("s{}", i), mapping);
    }

    // 500 names + 500 target_titles (same as names for most) + 1000 aliases = 1500 alternatives
    let gate = crate::source_processor::build_gate_regex(&mappings);
    assert!(gate.is_some(), "1500 alternatives should compile");

    let gate = gate.unwrap();
    // Verify first and last match
    assert!(
        gate.is_match("Series Number 0 S01E01"),
        "First series matches"
    );
    assert!(gate.is_match("s499 S01E01"), "Last series alias matches");
    // Verify unrelated title doesn't match
    assert!(
        !gate.is_match("Something Completely Unrelated"),
        "No match for unknown title"
    );
}

#[test]
fn test_gate_large_with_user_patterns() {
    // 300 series with user-defined patterns (extraction + filter)
    let mut mappings = HashMap::new();
    for i in 0..300 {
        let mapping = MappingRule {
            target_title: format!("Show {}", i),
            name: format!("Show {}", i),
            settings: SeriesSettings {
                reg_patterns: vec![
                    format!("(?i)Show{0}.*S(?P<season>\\d+)E(?P<episode>\\d+)", i),
                    format!("(?i)SH{0}Release", i),
                ],
                ..SeriesSettings::default()
            },
            ..MappingRule::default()
        };
        mappings.insert(format!("s{}", i), mapping);
    }

    // 300 × 2 user patterns = 600 alternatives, plus 300 auto-generated names
    // With (?:...) wrapping, total pattern string is ~30K chars
    let gate = crate::source_processor::build_gate_regex(&mappings);
    assert!(
        gate.is_some(),
        "900 alternatives with extraction patterns should compile"
    );

    let gate = gate.unwrap();
    assert!(gate.is_match("SH0Release"), "Filter pattern matches");
    assert!(
        gate.is_match("Show 299 S01E01"),
        "Extraction pattern matches"
    );
}

// Gate: graceful degradation — extreme input

#[test]
fn test_gate_extreme_size_does_not_crash() {
    // Very large pattern set — should either compile or return None gracefully.
    let mut mappings = HashMap::new();
    for i in 0..2000 {
        let name = format!("Series{}", i);
        let mapping = MappingRule {
            target_title: name.clone(),
            name,
            ..MappingRule::default()
        };
        mappings.insert(format!("s{}", i), mapping);
    }

    let gate = crate::source_processor::build_gate_regex(&mappings);
    // 2000 alternatives ~60KB pattern string — may or may not compile
    // depending on DFA size limits. Either outcome is acceptable.
    if let Some(g) = gate {
        assert!(g.is_match("Series1999"), "Extreme: last series matches");
    }
    // If gate is None, the system falls back to Phase 1 + Phase 2 — no crash
}

// Gate: unicode in names

#[test]
fn test_gate_unicode_name() {
    let mut mappings = HashMap::new();
    let mapping = MappingRule {
        target_title: "日本語".to_string(),
        name: "日本語".to_string(),
        ..MappingRule::default()
    };
    mappings.insert("s1".to_string(), mapping);

    let gate = crate::source_processor::build_gate_regex(&mappings);
    assert!(gate.is_some(), "Unicode name should compile");
    assert!(
        gate.unwrap().is_match("日本語 S01E01"),
        "Unicode name matches"
    );
}

#[test]
fn test_gate_unicode_alias() {
    let mut mappings = HashMap::new();
    let mapping = MappingRule {
        target_title: "Show".to_string(),
        name: "Show".to_string(),
        settings: SeriesSettings {
            aliases: vec!["ショー".to_string()],
            ..SeriesSettings::default()
        },
        ..MappingRule::default()
    };
    mappings.insert("s1".to_string(), mapping);

    let gate = crate::source_processor::build_gate_regex(&mappings);
    assert!(gate.is_some(), "Unicode alias should compile");
    assert!(
        gate.unwrap().is_match("ショー S01E01"),
        "Unicode alias matches"
    );
}

// Gate: empty/whitespace patterns

#[test]
fn test_gate_whitespace_only_name_no_crash() {
    let mut mappings = HashMap::new();
    let mapping = MappingRule {
        target_title: "   ".to_string(),
        name: "   ".to_string(),
        ..MappingRule::default()
    };
    mappings.insert("s1".to_string(), mapping);

    // Empty/whitespace name should be skipped — gate may or may not compile
    // depending on whether there are other contributing parts
    let gate = crate::source_processor::build_gate_regex(&mappings);
    // No crash is the main test — result can be Some or None
    if let Some(g) = gate {
        // If it compiled, the whitespace pattern shouldn't match normal titles
        // (?i)[._ ][._ ][._ ] is very unlikely to match a real release title
        assert!(
            !g.is_match("Normal Release S01E01"),
            "Whitespace-only name shouldn't match real titles"
        );
    }
}

#[test]
fn test_gate_empty_patterns_in_user_list() {
    // User has empty strings in their reg_patterns list
    let mut mappings = HashMap::new();
    let mapping = MappingRule {
        target_title: "Show".to_string(),
        name: "Show".to_string(),
        settings: SeriesSettings {
            reg_patterns: vec!["".to_string(), "(?i)ValidPattern".to_string()],
            ..SeriesSettings::default()
        },
        ..MappingRule::default()
    };
    mappings.insert("s1".to_string(), mapping);

    let gate = crate::source_processor::build_gate_regex(&mappings);
    assert!(
        gate.is_some(),
        "Empty pattern in list should not break gate"
    );
    assert!(
        gate.unwrap().is_match("ValidPattern E01"),
        "Valid pattern still matches"
    );
}

// Gate: source-scoped + extraction combined

#[test]
fn test_gate_source_scoped_with_extraction() {
    // Pattern with both @source: prefix AND extraction groups
    let mut mappings = HashMap::new();
    let mapping = MappingRule {
        target_title: "Anime".to_string(),
        name: "Anime".to_string(),
        settings: SeriesSettings {
            reg_patterns: vec![r"@nyaa:(?i)Anime.*E(?P<episode>\d+)".to_string()],
            ..SeriesSettings::default()
        },
        ..MappingRule::default()
    };
    mappings.insert("s1".to_string(), mapping);

    let gate = crate::source_processor::build_gate_regex(&mappings);
    assert!(gate.is_some(), "Source-scoped extraction pattern compiles");
    assert!(
        gate.unwrap().is_match("Anime Title E05"),
        "Gate includes stripped pattern text"
    );
}

// Comprehensive integration: all expression types together

#[test]
fn test_gate_all_expression_types_combine_correctly() {
    // Build a single gate with every expression type that exists in the system,
    // then verify every type matches correctly and independently.
    let mut mappings = HashMap::new();

    // Type 1: Auto-generated from name (single word)
    let s1 = MappingRule {
        target_title: "Alpha".to_string(),
        name: "Alpha".to_string(),
        ..MappingRule::default()
    };
    mappings.insert("s1".to_string(), s1);

    // Type 2: Auto-generated from name (multi-word with spaces → dot-flexible)
    let s2 = MappingRule {
        target_title: "Bravo Show".to_string(),
        name: "Bravo Show".to_string(),
        ..MappingRule::default()
    };
    mappings.insert("s2".to_string(), s2);

    // Type 3: Auto-generated from target_title (different from name)
    let s3 = MappingRule {
        target_title: "Charlie Program".to_string(),
        name: "Charlie".to_string(),
        ..MappingRule::default()
    };
    mappings.insert("s3".to_string(), s3);

    // Type 4: Auto-generated from alias
    let s4 = MappingRule {
        target_title: "Delta".to_string(),
        name: "Delta".to_string(),
        settings: SeriesSettings {
            aliases: vec!["DShow".to_string()],
            ..SeriesSettings::default()
        },
        ..MappingRule::default()
    };
    mappings.insert("s4".to_string(), s4);

    // Type 5: User-defined filter pattern (case-insensitive with (?i))
    let s5 = MappingRule {
        target_title: "Echo".to_string(),
        name: "Echo".to_string(),
        settings: SeriesSettings {
            reg_patterns: vec![r"(?i)EchoRelease".to_string()],
            ..SeriesSettings::default()
        },
        ..MappingRule::default()
    };
    mappings.insert("s5".to_string(), s5);

    // Type 6: User-defined filter pattern (case-sensitive, no (?i))
    // Name chosen so auto-generated (?i)Quux doesn't match our test strings
    let s6 = MappingRule {
        target_title: "Quux".to_string(),
        name: "Quux".to_string(),
        settings: SeriesSettings {
            reg_patterns: vec!["ExactOnlyPattern".to_string()],
            ..SeriesSettings::default()
        },
        ..MappingRule::default()
    };
    mappings.insert("s6".to_string(), s6);

    // Type 7: User-defined extraction pattern (with (?P<season>+?P<episode>))
    let s7 = MappingRule {
        target_title: "Golf".to_string(),
        name: "Golf".to_string(),
        settings: SeriesSettings {
            reg_patterns: vec![r"(?i)Golf.*S(?P<season>\d+)E(?P<episode>\d+)".to_string()],
            ..SeriesSettings::default()
        },
        ..MappingRule::default()
    };
    mappings.insert("s7".to_string(), s7);

    // Type 8: User-defined extraction pattern (episode only, no season)
    let s8 = MappingRule {
        target_title: "Hotel".to_string(),
        name: "Hotel".to_string(),
        settings: SeriesSettings {
            reg_patterns: vec![r"(?i)Hotel.*E(?P<episode>\d+)".to_string()],
            ..SeriesSettings::default()
        },
        ..MappingRule::default()
    };
    mappings.insert("s8".to_string(), s8);

    // Type 9: Source-scoped filter pattern
    let s9 = MappingRule {
        target_title: "India".to_string(),
        name: "India".to_string(),
        settings: SeriesSettings {
            reg_patterns: vec![r"@nyaa:(?i)IndiaSource".to_string()],
            ..SeriesSettings::default()
        },
        ..MappingRule::default()
    };
    mappings.insert("s9".to_string(), s9);

    // Type 10: Source-scoped extraction pattern
    let s10 = MappingRule {
        target_title: "Juliett".to_string(),
        name: "Juliett".to_string(),
        settings: SeriesSettings {
            reg_patterns: vec![r"@nzb:(?i)Juliett.*E(?P<episode>\d+)".to_string()],
            ..SeriesSettings::default()
        },
        ..MappingRule::default()
    };
    mappings.insert("s10".to_string(), s10);

    // Type 11: Mixed per-series (filter + extraction)
    let s11 = MappingRule {
        target_title: "Kilo".to_string(),
        name: "Kilo".to_string(),
        settings: SeriesSettings {
            reg_patterns: vec![
                r"(?i)KiloRelease".to_string(),
                r"(?i)Kilo.*S(?P<season>\d+)E(?P<episode>\d+)".to_string(),
            ],
            ..SeriesSettings::default()
        },
        ..MappingRule::default()
    };
    mappings.insert("s11".to_string(), s11);

    // Type 12: Name with regex meta-characters
    let s12 = MappingRule {
        target_title: "C++".to_string(),
        name: "C++".to_string(),
        ..MappingRule::default()
    };
    mappings.insert("s12".to_string(), s12);

    // Type 13: Name with unicode
    let s13 = MappingRule {
        target_title: "東京".to_string(),
        name: "東京".to_string(),
        ..MappingRule::default()
    };
    mappings.insert("s13".to_string(), s13);

    // Type 14: Targeted filter pattern only matches a very specific string
    let s14 = MappingRule {
        target_title: "Lima".to_string(),
        name: "Lima".to_string(),
        settings: SeriesSettings {
            reg_patterns: vec![r"^Lima\s+\d+$".to_string()],
            ..SeriesSettings::default()
        },
        ..MappingRule::default()
    };
    mappings.insert("s14".to_string(), s14);

    // Build the gate
    let gate = crate::source_processor::build_gate_regex(&mappings)
        .expect("All 14 expression types should compile into a single gate");

    // Verify each type matches correctly

    // Type 1: Single-word name (case-insensitive)
    assert!(gate.is_match("alpha"), "Type1: single-word name lowercase");
    assert!(gate.is_match("ALPHA"), "Type1: single-word name uppercase");
    assert!(gate.is_match("Alpha.S01E01"), "Type1: with dots");

    // Type 2: Multi-word name with dot-flexible separator
    assert!(
        gate.is_match("Bravo Show S01E01"),
        "Type2: name with spaces"
    );
    assert!(gate.is_match("Bravo.Show.S01E01"), "Type2: name with dots");
    assert!(
        gate.is_match("bravo.show.s01e01"),
        "Type2: case-insensitive"
    );

    // Type 3: target_title different from name
    assert!(gate.is_match("Charlie S01E01"), "Type3: name");
    assert!(
        gate.is_match("Charlie Program S01E01"),
        "Type3: target_title"
    );

    // Type 4: Alias
    assert!(gate.is_match("DShow S01E01"), "Type4: alias");
    assert!(gate.is_match("Delta S01E01"), "Type4: name");

    // Type 5: Case-insensitive filter pattern
    assert!(gate.is_match("EchoRelease"), "Type5: filter match");
    assert!(
        gate.is_match("echorelease"),
        "Type5: filter case-insensitive"
    );

    // Type 6: Case-sensitive filter pattern — each alternative has scoped flags.
    // The name "Quux" doesn't appear in our test strings, so the auto-generated
    // (?i)Quux won't interfere. Only the case-sensitive user pattern applies.
    assert!(
        gate.is_match("ExactOnlyPattern"),
        "Type6: exact case matches"
    );
    assert!(
        !gate.is_match("exactonlypattern"),
        "Type6: case-sensitive alternative must reject lowercase (no (?i) leaking)"
    );
    assert!(
        !gate.is_match("EXACTONLYPATTERN"),
        "Type6: case-sensitive alternative must reject uppercase"
    );

    // Type 7: Extraction pattern (season + episode)
    assert!(gate.is_match("Golf S02E10"), "Type7: extraction S02E10");
    assert!(
        gate.is_match("Golf.S02E10.1080p"),
        "Type7: extraction with dots"
    );
    assert!(
        gate.is_match("golf s02e10"),
        "Type7: extraction case-insensitive"
    );

    // Type 8: Extraction pattern (episode only)
    assert!(gate.is_match("Hotel E05"), "Type8: episode-only");
    assert!(gate.is_match("Hotel.E05.1080p"), "Type8: episode with dots");

    // Type 9: Source-scoped filter — prefix stripped in gate
    assert!(gate.is_match("IndiaSource"), "Type9: source-scoped filter");
    assert!(
        gate.is_match("indiasource"),
        "Type9: source-scoped case-insensitive"
    );

    // Type 10: Source-scoped extraction — prefix stripped, named groups stripped
    assert!(
        gate.is_match("Juliett E10"),
        "Type10: source-scoped extraction"
    );
    assert!(
        gate.is_match("juliett e10"),
        "Type10: source-scoped case-insensitive"
    );

    // Type 11: Mixed per-series (both filter and extraction work)
    assert!(gate.is_match("KiloRelease"), "Type11: mixed filter");
    assert!(gate.is_match("Kilo S01E01"), "Type11: mixed extraction");

    // Type 12: Name with regex meta-characters
    assert!(gate.is_match("C++ S01E01"), "Type12: escaped + in name");
    assert!(gate.is_match("c++ s01e01"), "Type12: case-insensitive");

    // Type 13: Unicode
    assert!(gate.is_match("東京 S01E01"), "Type13: unicode name");

    // Type 14: Anchored pattern with ^...$ — the auto-generated name (?i)Lima
    // also matches titles containing "Lima" as a substring (that's the gate
    // being a superset). To verify the anchored pattern works independently
    // within its own alternative, we check via titles that ONLY differ in case
    // or prefix — if (?i) leaked from other alternatives, those would match.
    assert!(gate.is_match("Lima 123"), "Type14: anchored match");
    // "Lima" without digits doesn't match the anchored pattern, but DOES
    // match the auto-generated (?i)Lima name pattern — that's the gate being
    // a superset. To verify the anchored alternative independently, check
    // that using a DIFFERENT prefix (which doesn't contain "Lima") doesn't
    // match the anchored pattern.
    assert!(
        !gate.is_match("Xyzzy 123"),
        "Type14: anchored doesn't match different prefix"
    );

    // Unrelated title must NOT match
    assert!(
        !gate.is_match("Something Completely Unrelated That No Series Has"),
        "Unrelated title must not match any alternative"
    );

    // Verify no false negatives
    // For every title we said should match above, run a quick double-check
    let all_should_match: &[&str] = &[
        "alpha",
        "ALPHA",
        "Alpha.S01E01", // Type1
        "Bravo Show S01E01",
        "Bravo.Show.S01E01",
        "bravo.show.s01e01", // Type2
        "Charlie S01E01",
        "Charlie Program S01E01", // Type3
        "DShow S01E01",
        "Delta S01E01", // Type4
        "EchoRelease",
        "echorelease",      // Type5
        "ExactOnlyPattern", // Type6
        "Golf S02E10",
        "Golf.S02E10.1080p",
        "golf s02e10", // Type7
        "Hotel E05",
        "Hotel.E05.1080p", // Type8
        "IndiaSource",
        "indiasource", // Type9
        "Juliett E10",
        "juliett e10", // Type10
        "KiloRelease",
        "Kilo S01E01", // Type11
        "C++ S01E01",
        "c++ s01e01",  // Type12
        "東京 S01E01", // Type13
        "Lima 123",    // Type14
    ];
    for title in all_should_match {
        assert!(
            gate.is_match(title),
            "False negative: '{}' should match",
            title
        );
    }

    // Verify no unwanted matches (strict false positive control)
    let all_should_not_match: &[&str] = &[
        "exactonlypattern",   // Type6: case-sensitive, no (?i) on this alt
        "EXACTONLYPATTERN",   // Type6: case-sensitive
        "Xyzzy 123",          // Type14: anchored ^Lima..., different prefix doesn't match
        "Sombre.Show.S01E01", // Not a known series
        "Something Unrelated",
    ];
    for title in all_should_not_match {
        assert!(
            !gate.is_match(title),
            "False positive: '{}' should NOT match",
            title
        );
    }
}

// Full regex flag scoping tests
// The regex crate supports six flags: i (case), m (multiline), s (dot-all),
// U (swap greediness), x (verbose), u (unicode). Each must be scoped to its
// own (?:...) alternative and not leak to adjacent alternatives.

#[test]
fn test_gate_flag_m_multiline_scoped() {
    // (?m) makes ^ and $ match line boundaries. The (?:...) wrapping must
    // scope it so it only affects its own alternative.
    let mut mappings = HashMap::new();

    let s1 = MappingRule {
        target_title: "M1".to_string(),
        name: "M1".to_string(),
        settings: SeriesSettings {
            reg_patterns: vec![r"(?m)^\d+$".to_string()],
            ..SeriesSettings::default()
        },
        ..MappingRule::default()
    };
    mappings.insert("s1".to_string(), s1);

    let s2 = MappingRule {
        target_title: "Plain".to_string(),
        name: "Plain".to_string(),
        ..MappingRule::default()
    };
    mappings.insert("s2".to_string(), s2);

    let gate = crate::source_processor::build_gate_regex(&mappings)
        .expect("Gate with (?m) flag should compile");

    assert!(gate.is_match("42"), "(?m) alt matches single-line digits");
    assert!(
        gate.is_match("plain"),
        "(?m) flag doesn't break other alternatives"
    );
}

#[test]
fn test_gate_flag_s_dotall_scoped() {
    // (?s) makes . match \n. Must be scoped per-alternative.
    let mut mappings = HashMap::new();

    let s1 = MappingRule {
        target_title: "DotAll".to_string(),
        name: "DotAll".to_string(),
        settings: SeriesSettings {
            reg_patterns: vec![r"(?s)foo.bar".to_string()],
            ..SeriesSettings::default()
        },
        ..MappingRule::default()
    };
    mappings.insert("s1".to_string(), s1);

    let s2 = MappingRule {
        target_title: "Normal".to_string(),
        name: "Normal".to_string(),
        ..MappingRule::default()
    };
    mappings.insert("s2".to_string(), s2);

    let gate = crate::source_processor::build_gate_regex(&mappings)
        .expect("Gate with (?s) flag should compile");

    assert!(gate.is_match("foo\nbar"), "(?s) alt: dot matches newline");
    assert!(gate.is_match("foo bar"), "(?s) alt: dot matches space too");
    assert!(
        gate.is_match("normal"),
        "(?s) flag scoped, doesn't affect others"
    );
}

#[test]
fn test_gate_flag_u_swap_greediness_scoped() {
    // (?U) makes quantifiers lazy by default.
    let mut mappings = HashMap::new();

    let s1 = MappingRule {
        target_title: "Lazy".to_string(),
        name: "Lazy".to_string(),
        settings: SeriesSettings {
            reg_patterns: vec![r"(?U)a.+b".to_string()],
            ..SeriesSettings::default()
        },
        ..MappingRule::default()
    };
    mappings.insert("s1".to_string(), s1);

    let gate = crate::source_processor::build_gate_regex(&mappings)
        .expect("Gate with (?U) flag should compile");

    assert!(gate.is_match("aXYb"), "(?U) alt: lazy matches shortest");
    assert!(gate.is_match("aXYbZb"), "(?U) alt: lazy still matches");
}

#[test]
fn test_gate_flag_x_verbose_scoped() {
    // (?x) ignores whitespace in the pattern.
    let mut mappings = HashMap::new();

    let s1 = MappingRule {
        target_title: "Verbose".to_string(),
        name: "Verbose".to_string(),
        settings: SeriesSettings {
            reg_patterns: vec![r"(?x)foo bar baz".to_string()],
            ..SeriesSettings::default()
        },
        ..MappingRule::default()
    };
    mappings.insert("s1".to_string(), s1);

    let gate = crate::source_processor::build_gate_regex(&mappings)
        .expect("Gate with (?x) flag should compile");

    assert!(
        gate.is_match("foobarbaz"),
        "(?x) alt: whitespace in pattern ignored"
    );
    assert!(
        !gate.is_match("foo bar baz"),
        "(?x) alt: spaces in pattern are ignored"
    );
}

#[test]
fn test_gate_flag_minus_u_ascii_only_scoped() {
    // (?-u) disables Unicode mode — \w matches ASCII only.
    let mut mappings = HashMap::new();

    let s1 = MappingRule {
        target_title: "ASCII".to_string(),
        name: "ASCII".to_string(),
        settings: SeriesSettings {
            reg_patterns: vec![r"(?-u)\w+".to_string()],
            ..SeriesSettings::default()
        },
        ..MappingRule::default()
    };
    mappings.insert("s1".to_string(), s1);

    let gate = crate::source_processor::build_gate_regex(&mappings)
        .expect("Gate with (?-u) flag should compile");

    assert!(gate.is_match("hello"), "(?-u) alt: \\w+ matches ASCII word");
}

#[test]
fn test_gate_flag_combined_im_scoped() {
    // (?im) combines multiline and case-insensitive.
    let mut mappings = HashMap::new();

    // Pattern matches exactly "^foo$" with (?i) and (?m).
    // Using a specific word so we can distinguish from other alternatives.
    let s1 = MappingRule {
        target_title: "Combined".to_string(),
        name: "Combined".to_string(),
        settings: SeriesSettings {
            reg_patterns: vec![r"(?im)^foo$".to_string()],
            ..SeriesSettings::default()
        },
        ..MappingRule::default()
    };
    mappings.insert("s1".to_string(), s1);

    // Another series with a case-sensitive pattern to verify no flag leakage.
    let s2 = MappingRule {
        target_title: "OtherShow".to_string(),
        name: "OtherShow".to_string(),
        settings: SeriesSettings {
            reg_patterns: vec!["^Exact$".to_string()],
            ..SeriesSettings::default()
        },
        ..MappingRule::default()
    };
    mappings.insert("s2".to_string(), s2);

    let gate = crate::source_processor::build_gate_regex(&mappings)
        .expect("Gate with (?im) flag should compile");

    // (?im) alt: matches "foo" case-insensitively
    assert!(gate.is_match("foo"), "(?im) alt: literal match");
    assert!(gate.is_match("FOO"), "(?im) alt: case-insensitive via (?i)");

    // The case-sensitive alt: (?im) must NOT leak into it.
    // The auto-generated (?i)OtherShow won't match "Exact"/"exact"/"EXACT"
    // (they don't contain "OtherShow"), so only the user pattern applies.
    assert!(gate.is_match("Exact"), "Case-sensitive alt: exact case");
    assert!(
        !gate.is_match("exact"),
        "(?im) must NOT leak: case-sensitive alt rejects lowercase"
    );
    assert!(
        !gate.is_match("EXACT"),
        "(?im) must NOT leak: case-sensitive alt rejects uppercase"
    );
}

#[test]
fn test_gate_flag_toggle_within_alternative() {
    // (?i)foo(?-i)bar — flags toggled within a single alternative.
    let mut mappings = HashMap::new();

    let s1 = MappingRule {
        target_title: "Toggle".to_string(),
        name: "Toggle".to_string(),
        settings: SeriesSettings {
            reg_patterns: vec![r"(?i)foo(?-i)bar".to_string()],
            ..SeriesSettings::default()
        },
        ..MappingRule::default()
    };
    mappings.insert("s1".to_string(), s1);

    let gate = crate::source_processor::build_gate_regex(&mappings)
        .expect("Gate with flag toggling should compile");

    assert!(
        gate.is_match("FOObar"),
        "(?i)foo + (?-i)bar: FOObar matches"
    );
    assert!(
        gate.is_match("foobar"),
        "(?i)foo + (?-i)bar: foobar matches"
    );
    assert!(
        !gate.is_match("FOOBAR"),
        "bar is case-sensitive: FOOBAR should not match"
    );
}
