use std::collections::HashMap;

use crate::source_processor::SeriesMatcher;
use jumbie_shared::mapping::{MappingRule, SeriesSettings};
use jumbie_shared::parsing::CustomParseResult;

fn mapping(name: &str, aliases: &[&str], patterns: &[&str]) -> MappingRule {
    MappingRule {
        target_title: name.to_string(),
        name: name.to_string(),
        settings: SeriesSettings {
            aliases: aliases.iter().map(|s| s.to_string()).collect(),
            reg_patterns: patterns.iter().map(|s| s.to_string()).collect(),
            ..SeriesSettings::default()
        },
        ..MappingRule::default()
    }
}

fn mappings(entries: Vec<(&str, MappingRule)>) -> HashMap<String, MappingRule> {
    entries
        .into_iter()
        .map(|(id, rule)| (id.to_string(), rule))
        .collect()
}

#[test]
fn identify_selects_the_matching_series() {
    let all = mappings(vec![
        ("a", mapping("Alpha Show", &[], &[])),
        ("b", mapping("Bravo Show", &[], &[])),
        ("c", mapping("Charlie Show", &[], &[])),
    ]);
    let matcher = SeriesMatcher::build(&all);

    let (found, _) = matcher
        .identify(&all, "Bravo.Show.S02E05.1080p", "src", false)
        .expect("Bravo should match");
    assert_eq!(found.target_title, "Bravo Show");
}

#[test]
fn identify_returns_none_when_the_gate_misses() {
    let all = mappings(vec![("a", mapping("Alpha Show", &[], &[]))]);
    let matcher = SeriesMatcher::build(&all);

    assert!(!matcher.is_match("Completely Unrelated 1080p"));
    assert!(
        matcher
            .identify(&all, "Completely Unrelated 1080p", "src", false)
            .is_none()
    );
}

#[test]
fn identify_matches_via_alias() {
    let all = mappings(vec![(
        "a",
        mapping("A Very Long Series Name", &["AVLSN"], &[]),
    )]);
    let matcher = SeriesMatcher::build(&all);

    let (found, _) = matcher
        .identify(&all, "AVLSN S01E03", "src", false)
        .expect("alias should match");
    assert_eq!(found.target_title, "A Very Long Series Name");
}

#[test]
fn identify_uses_extraction_pattern() {
    let all = mappings(vec![(
        "a",
        mapping(
            "Anime",
            &[],
            &[r"(?i)Anime.*S(?P<season>\d+)E(?P<episode>\d+)"],
        ),
    )]);
    let matcher = SeriesMatcher::build(&all);

    let (_, result) = matcher
        .identify(&all, "Anime S02E07 1080p", "src", false)
        .expect("extraction pattern should match");
    match result {
        Some(CustomParseResult::Extracted(info)) => {
            assert_eq!(info.seasons, vec![2]);
            assert_eq!(info.episodes, vec![7]);
        }
        other => panic!("expected extraction, got {other:?}"),
    }
}

#[test]
fn identify_honors_absolute_numbering() {
    let mut rule = mapping("Abs", &[], &[r"(?i)Abs.*S(?P<season>\d+)E(?P<episode>\d+)"]);
    rule.settings.absolute_numbering = Some(true);
    let all = mappings(vec![("a", rule)]);
    let matcher = SeriesMatcher::build(&all);

    let (_, result) = matcher
        .identify(&all, "Abs S02E07", "src", false)
        .expect("pattern should match");
    match result {
        Some(CustomParseResult::Extracted(info)) => {
            assert!(
                info.seasons.is_empty(),
                "the season group is ignored in absolute mode"
            );
        }
        other => panic!("expected extraction, got {other:?}"),
    }
}

#[test]
fn user_patterns_replace_auto_patterns_in_phase_one() {
    // The series name matches the title, but its user pattern does not. Phase 1
    // uses the user patterns only, so identify falls through (None here).
    let all = mappings(vec![(
        "a",
        mapping("My Show", &[], &[r"(?i)CompletelyDifferent"]),
    )]);
    let matcher = SeriesMatcher::build(&all);

    // The gate still admits it: the name is part of the gate's superset.
    assert!(matcher.is_match("My.Show.S01E01"));
    // ...but Phase 1 has no matching pattern.
    assert!(
        matcher
            .identify(&all, "My.Show.S01E01", "src", false)
            .is_none()
    );
}

#[test]
fn identify_skips_gate_candidates_that_fail_phase_one() {
    // Both series' names appear in the title, so both pass the gate. Only the
    // second one has a matching Phase 1 pattern.
    let all = mappings(vec![
        ("a", mapping("Shared Show", &[], &[r"(?i)NeverMatches"])),
        (
            "b",
            mapping("Shared Show", &[], &[r"(?i)Shared.*E(?P<episode>\d+)"]),
        ),
    ]);
    let matcher = SeriesMatcher::build(&all);

    let (found, _) = matcher
        .identify(&all, "Shared Show E05", "src", false)
        .expect("the second series should match");
    assert_eq!(
        found.settings.reg_patterns,
        vec![r"(?i)Shared.*E(?P<episode>\d+)"]
    );
}

#[test]
fn identify_respects_source_scoped_patterns() {
    let all = mappings(vec![(
        "a",
        mapping("Anime", &[], &[r"@nyaa:(?i)Anime.*E(?P<episode>\d+)"]),
    )]);
    let matcher = SeriesMatcher::build(&all);

    assert!(matches!(
        matcher.identify(&all, "Anime E05", "nyaa", false),
        Some((_, Some(CustomParseResult::Extracted(_))))
    ));
    // A different source sees no pattern: the generic list is empty.
    assert!(
        matcher
            .identify(&all, "Anime E05", "other", false)
            .is_none()
    );
}

#[test]
fn gate_admits_every_phase_one_match() {
    let all = mappings(vec![
        ("a", mapping("Alpha Show", &["AS"], &[])),
        (
            "b",
            mapping("Bravo", &[], &[r"(?i)Bravo.*E(?P<episode>\d+)"]),
        ),
        ("c", mapping("Charlie", &["Chuck"], &[])),
    ]);
    let matcher = SeriesMatcher::build(&all);

    for title in [
        "Alpha Show S01E01",
        "AS S01E02",
        "Bravo E05",
        "Chuck S02E03",
        "charlie s01e01",
    ] {
        assert!(
            matcher.identify(&all, title, "src", false).is_some(),
            "expected a series to match {title}"
        );
        assert!(matcher.is_match(title), "gate must admit {title}");
    }
}

#[test]
fn empty_mappings_have_no_gate_and_no_match() {
    let all = HashMap::new();
    let matcher = SeriesMatcher::build(&all);

    assert!(matcher.is_match("Anything S01E01"));
    assert!(
        matcher
            .identify(&all, "Anything S01E01", "src", false)
            .is_none()
    );
}

#[test]
fn series_without_identifiers_never_matches() {
    let rule = MappingRule {
        target_title: String::new(),
        name: String::new(),
        ..MappingRule::default()
    };
    let all = mappings(vec![("empty", rule)]);
    let matcher = SeriesMatcher::build(&all);

    // No gate patterns exist, so everything is admitted — but Phase 1 has no
    // patterns for the series, so nothing matches.
    assert!(matcher.is_match("Some Show S01E01"));
    assert!(
        matcher
            .identify(&all, "Some Show S01E01", "src", false)
            .is_none()
    );
}
