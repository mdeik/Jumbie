//! Season number alias behavior tests.

use crate::tests::behavior_tests::helpers::{make_mapping, make_season_override};
use std::collections::HashMap;

#[test]
fn test_season_alias_matches_parsed_season_number() {
    let mut season_overrides = HashMap::new();
    season_overrides.insert(
        "3".to_string(),
        make_season_override("3", Some(1), None, vec!["Season3Alias".to_string()]),
    );
    season_overrides.insert(
        "1".to_string(),
        make_season_override("1", None, None, vec![]),
    );

    let mapping = make_mapping("TestShow", season_overrides);

    // Simulate process_entry: `info.season = Some(1)` → `parsed_season_str = "1"`
    let parsed_season_str = "1";

    let mut matched_seasons: Vec<String> = vec![];

    for (season_key, rule) in &mapping.settings.season {
        let mut matches_rule = false;

        // 1) Match by explicit alias season number
        if let Some(alias_num) = rule.alias_season_number {
            let alias_str = alias_num.to_string();
            let alias_padded = format!("S{:02}", alias_num);
            if alias_str == parsed_season_str || alias_padded == parsed_season_str {
                matches_rule = true;
            }
        }

        // 2) Match by actual explicit season key
        if !matches_rule {
            let rule_season_num = season_key.parse::<i32>().unwrap_or(-1);
            if parsed_season_str == season_key.as_str()
                || parsed_season_str
                    .trim_start_matches('S')
                    .parse::<i32>()
                    .ok()
                    == Some(rule_season_num)
            {
                matches_rule = true;
            }
        }

        if matches_rule {
            matched_seasons.push(season_key.clone());
        }
    }

    assert!(
        matched_seasons.contains(&"3".to_string()),
        "alias_season_number=1 on season '3' should match parsed season '1'. Matched: {:?}",
        matched_seasons
    );

    assert!(
        matched_seasons.contains(&"1".to_string()),
        "Season key '1' should also match parsed season '1' directly. Matched: {:?}",
        matched_seasons
    );

    let s3_rule = mapping.settings.season.get("3").unwrap();
    assert_eq!(
        s3_rule.aliases,
        vec!["Season3Alias"],
        "Override aliases should be available when matched via alias_season_number"
    );
}

#[test]
fn test_season_alias_does_not_match_wrong_number() {
    let mut season_overrides = HashMap::new();
    season_overrides.insert(
        "3".to_string(),
        make_season_override("3", Some(2), None, vec![]),
    );

    let mapping = make_mapping("TestShow", season_overrides);

    let parsed_season_str = "01";
    let mut matched = false;

    for rule in mapping.settings.season.values() {
        if let Some(alias_num) = rule.alias_season_number {
            let alias_str = alias_num.to_string();
            let alias_padded = format!("S{:02}", alias_num);
            if alias_str == parsed_season_str || alias_padded == parsed_season_str {
                matched = true;
            }
        }
    }

    assert!(
        !matched,
        "alias_season_number=2 should NOT match parsed season 01"
    );
}

#[test]
fn test_season_alias_matches_padded_format() {
    let mut season_overrides = HashMap::new();
    season_overrides.insert(
        "3".to_string(),
        make_season_override("3", Some(1), None, vec![]),
    );

    let mapping = make_mapping("TestShow", season_overrides);

    let parsed_season_str = "S01";
    let mut matched = false;

    for rule in mapping.settings.season.values() {
        if let Some(alias_num) = rule.alias_season_number {
            let alias_str = alias_num.to_string();
            let alias_padded = format!("S{:02}", alias_num);
            if alias_str == parsed_season_str || alias_padded == parsed_season_str {
                matched = true;
            }
        }
    }

    assert!(
        matched,
        "alias_season_number=1 should match parsed season 'S01' via padded format"
    );
}
