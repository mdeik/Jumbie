//! Episode offset behavior tests.
//!
//! These tests verify the episode offset matching semantics implemented in
//! `process_entry`:
//!
//! - `episode_start`/`episode_end` are in **local** episode space
//! - `episode_offset` is subtracted from the source episode number to get
//!   the local episode number: `local = source - offset`
//! - All three matching paths (alias, key, absolute-range) check the
//!   offset-adjusted episode against the range

use crate::source_processor::identification::{
    SeasonModeDecision, SeasonModeRejection, season_mode_guard,
};
use jumbie_shared::mapping::SeasonOverride;
use std::collections::HashMap;

/// Build a SeasonOverride with the full range+offset fields.
fn make_range_override(
    season: &str,
    alias_season_number: Option<u32>,
    episode_start: Option<i32>,
    episode_end: Option<i32>,
    episode_offset: Option<i32>,
) -> SeasonOverride {
    SeasonOverride {
        season: season.to_string(),
        episode_start,
        episode_end,
        cell_count: None,
        episode_offset,
        alias_season_number,
        search_format: None,
        aliases: vec![],
        reg_patterns: vec![],
    }
}

/// Match a release against the season overrides using the **production**
/// implementation (`identification::match_season_overrides`) — this helper only
/// adapts its output to the `(target_season, local_episode)` shape these tests
/// assert on. Deliberately not a reimplementation, so tests can't pass against a
/// drifted copy.
///
/// Returns `Some((target_season, local_episode))` if a match is found,
/// or `None` if no override matches.
fn match_season_override(
    season_overrides: &HashMap<String, SeasonOverride>,
    parsed_season_str: &str,
    source_season: Option<i32>,
    source_episode: i32,
) -> Option<(i32, i32)> {
    let info = jumbie_shared::types::EpisodeInfo {
        raw_title: String::new(),
        series_key: String::new(),
        file_ext: String::new(),
        submitter: None,
        resolution: None,
        version: 1,
        part_number: None,
        is_season_pack: false,
        is_complete_pack: false,
        seasons: source_season.into_iter().collect(),
        episodes: vec![source_episode],
        has_decimal_episode: false,
    };

    let matched = crate::source_processor::identification::match_season_overrides(
        season_overrides,
        parsed_season_str,
        "",
        &info,
        "",
        false,
    );

    let matched = matched.first()?;
    let target = jumbie_shared::mapping::parse_season_num(matched.season_key)?;
    let offset = matched
        .rule
        .episode_offset
        .unwrap_or(SeasonOverride::DEFAULT_EPISODE_OFFSET);
    Some((target, source_episode - offset))
}

#[test]
fn test_positive_offset_maps_source_episode_to_local() {
    // User's scenario: S01E150 → S02E50 via offset=100, range=1-100
    let mut season_overrides = HashMap::new();
    season_overrides.insert(
        "1".to_string(),
        make_range_override("1", Some(1), Some(1), Some(100), Some(0)),
    );
    season_overrides.insert(
        "2".to_string(),
        make_range_override("2", Some(1), Some(1), Some(100), Some(100)),
    );
    season_overrides.insert(
        "3".to_string(),
        make_range_override("3", Some(1), Some(1), Some(100), Some(200)),
    );

    // "Show S01E150" → season=Some(1), parsed="1", episode=150
    let result = match_season_override(&season_overrides, "1", Some(1), 150);
    assert_eq!(
        result,
        Some((2, 50)),
        "S01E150 should match S02 with local ep 50"
    );

    // "Show S01E250" → season=Some(1), parsed="1", episode=250
    let result = match_season_override(&season_overrides, "1", Some(1), 250);
    assert_eq!(
        result,
        Some((3, 50)),
        "S01E250 should match S03 with local ep 50"
    );

    // "Show S01E050" → season=Some(1), parsed="1", episode=50
    let result = match_season_override(&season_overrides, "1", Some(1), 50);
    assert_eq!(
        result,
        Some((1, 50)),
        "S01E050 should match S01 with local ep 50"
    );
}

#[test]
fn test_positive_offset_outside_range_no_match() {
    // Season 2 override: range 1-100, offset 100
    let mut season_overrides = HashMap::new();
    season_overrides.insert(
        "2".to_string(),
        make_range_override("2", Some(1), Some(1), Some(100), Some(100)),
    );

    // "Show S01E250" → adjusted = 250-100 = 150, not in [1,100]
    let result = match_season_override(&season_overrides, "1", Some(1), 250);
    assert_eq!(result, None, "S01E250 should NOT match range 1-100");
}

#[test]
fn test_positive_offset_zero_edge_case() {
    // Season 1 override: range 1-10, offset 0 (no shift)
    let mut season_overrides = HashMap::new();
    season_overrides.insert(
        "1".to_string(),
        make_range_override("1", Some(1), Some(1), Some(10), Some(0)),
    );

    // "Show S01E05" → adjusted = 5-0 = 5, in [1,10]
    let result = match_season_override(&season_overrides, "1", Some(1), 5);
    assert_eq!(result, Some((1, 5)), "Zero offset passes through unchanged");
}

#[test]
fn test_negative_offset_maps_source_episode_to_local() {
    // Source episodes are 50 behind local episodes.
    // Season 2 override: range 51-150, offset -50
    // Source E01 → local E51 (1 - (-50) = 51)
    let mut season_overrides = HashMap::new();
    season_overrides.insert(
        "2".to_string(),
        make_range_override("2", Some(1), Some(51), Some(150), Some(-50)),
    );

    // "Show S01E01" → adjusted = 1-(-50) = 51, in [51,150]
    let result = match_season_override(&season_overrides, "1", Some(1), 1);
    assert_eq!(
        result,
        Some((2, 51)),
        "Negative offset maps S01E01 to S02E51"
    );

    // "Show S01E100" → adjusted = 100-(-50) = 150, in [51,150]
    let result = match_season_override(&season_overrides, "1", Some(1), 100);
    assert_eq!(
        result,
        Some((2, 150)),
        "Negative offset maps S01E100 to S02E150"
    );
}

#[test]
fn test_negative_offset_outside_range_no_match() {
    // Season 2 override: range 51-150, offset -50
    let mut season_overrides = HashMap::new();
    season_overrides.insert(
        "2".to_string(),
        make_range_override("2", Some(1), Some(51), Some(150), Some(-50)),
    );

    // "Show S01E200" → adjusted = 200-(-50) = 250, NOT in [51,150]
    let result = match_season_override(&season_overrides, "1", Some(1), 200);
    assert_eq!(result, None, "S01E200 should NOT match range 51-150");
}

#[test]
fn test_absolute_release_matches_positive_offset() {
    // "Show - 150" (no season) matches season 2 with offset 100, range 1-100
    let mut season_overrides = HashMap::new();
    season_overrides.insert(
        "2".to_string(),
        make_range_override("2", None, Some(1), Some(100), Some(100)),
    );

    // No alias_season_number set — path 3 (absolute range) should still match
    let result = match_season_override(&season_overrides, "01", None, 150);
    assert_eq!(
        result,
        Some((2, 50)),
        "Absolute release 150 -> S02E50 via range override"
    );
}

#[test]
fn test_absolute_release_matches_negative_offset() {
    // "Show - 1" (no season) matches season 2 with offset -50, range 51-150
    let mut season_overrides = HashMap::new();
    season_overrides.insert(
        "2".to_string(),
        make_range_override("2", None, Some(51), Some(150), Some(-50)),
    );

    let result = match_season_override(&season_overrides, "01", None, 1);
    assert_eq!(
        result,
        Some((2, 51)),
        "Absolute release 1 -> S02E51 via negative offset"
    );
}

#[test]
fn test_absolute_release_no_match_without_range() {
    // No overrides — absolute release should not match anything
    let season_overrides = HashMap::new();
    let result = match_season_override(&season_overrides, "01", None, 150);
    assert_eq!(
        result, None,
        "No overrides -> no match for absolute release"
    );
}

#[test]
fn test_alias_match_respects_range_not_just_alias_number() {
    // Two overrides with alias_season_number=1 but different ranges + offsets
    let mut season_overrides = HashMap::new();
    season_overrides.insert(
        "2".to_string(),
        make_range_override("2", Some(1), Some(1), Some(100), Some(100)),
    );
    season_overrides.insert(
        "3".to_string(),
        make_range_override("3", Some(1), Some(101), Some(200), Some(200)),
    );

    // "Show S01E150" → adjusted for S2: 150-100=50 in [1,100] ✓
    //                 → adjusted for S3: 150-200=-50, NOT in [101,200] ✗
    let result = match_season_override(&season_overrides, "1", Some(1), 150);
    assert_eq!(
        result,
        Some((2, 50)),
        "Range disambiguates: only season 2 matches ep 150"
    );

    // "Show S01E250" → S2: 250-100=150, not in [1,100]
    //                 → S3: 250-200=50, not in [101,200]
    let result2 = match_season_override(&season_overrides, "1", Some(1), 250);
    assert_eq!(result2, None, "Neither override matches ep 250");
}

#[test]
fn test_alias_match_disambiguates_by_offset() {
    // Fix the ranges so both are reachable:
    // S2: offset 100, range 1-100  → source eps 101-200
    // S3: offset 200, range 1-100  → source eps 201-300
    let mut season_overrides = HashMap::new();
    season_overrides.insert(
        "2".to_string(),
        make_range_override("2", Some(1), Some(1), Some(100), Some(100)),
    );
    season_overrides.insert(
        "3".to_string(),
        make_range_override("3", Some(1), Some(1), Some(100), Some(200)),
    );

    // "Show S01E050" → S2: 50-100=-50, not in [1,100]
    //                 → S3: 50-200=-150, not in [1,100]
    let result = match_season_override(&season_overrides, "1", Some(1), 50);
    assert_eq!(result, None, "Ep 50 below both offset ranges");

    // "Show S01E150" → S2: 150-100=50, in [1,100] → match S02E50
    //                 → S3: 150-200=-50, not in [1,100]
    let result = match_season_override(&season_overrides, "1", Some(1), 150);
    assert_eq!(
        result,
        Some((2, 50)),
        "Ep 150 matches season 2 (local ep 50)"
    );

    // "Show S01E250" → S2: 250-100=150, not in [1,100]
    //                 → S3: 250-200=50, in [1,100] → match S03E50
    let result = match_season_override(&season_overrides, "1", Some(1), 250);
    assert_eq!(
        result,
        Some((3, 50)),
        "Ep 250 matches season 3 (local ep 50)"
    );
}

#[test]
fn test_absolute_mode_blocks_season_gt_1() {
    // Absolute numbering has a single season and expects season-less episode
    // numbers, so a release declaring season 2 is rejected — while season 1 and
    // season-less (absolute-numbered) releases pass.
    let overrides = HashMap::new();

    assert_eq!(
        season_mode_guard(true, &[2], &[1], &overrides, "Show", "", false),
        SeasonModeDecision::Reject(SeasonModeRejection::SeasonAboveOneInAbsoluteMode(2))
    );
    assert_eq!(
        season_mode_guard(true, &[1], &[1], &overrides, "Show", "", false),
        SeasonModeDecision::Accept
    );
    assert_eq!(
        season_mode_guard(true, &[], &[7], &overrides, "Show", "", false),
        SeasonModeDecision::Accept
    );
}

#[test]
fn test_normal_mode_blocks_unowned_seasonless_release() {
    // No overrides and no Phase 1 match: a season-less release has no episode
    // identity under normal numbering, so it is rejected rather than defaulted.
    let overrides = HashMap::new();
    assert_eq!(
        season_mode_guard(false, &[], &[150], &overrides, "Show", "", false),
        SeasonModeDecision::Reject(SeasonModeRejection::UnownedSeasonlessRelease)
    );
    // A release that declares its season is unaffected by the guard.
    assert_eq!(
        season_mode_guard(false, &[2], &[1], &overrides, "Show", "", false),
        SeasonModeDecision::Accept
    );
}

#[test]
fn test_normal_mode_admits_seasonless_release_claimed_by_range_override() {
    let mut overrides = HashMap::new();
    overrides.insert(
        "2".to_string(),
        make_range_override("2", None, Some(1), Some(100), Some(100)),
    );

    // offset 100 → source ep 150 is local 50, inside [1, 100].
    assert_eq!(
        season_mode_guard(false, &[], &[150], &overrides, "Show", "", false),
        SeasonModeDecision::Accept,
        "a range override claims this absolute release"
    );
    // Outside every range and no matching pattern → still rejected.
    assert_eq!(
        season_mode_guard(false, &[], &[999], &overrides, "Show", "", false),
        SeasonModeDecision::Reject(SeasonModeRejection::UnownedSeasonlessRelease)
    );
}

#[test]
fn test_normal_mode_admits_seasonless_release_matched_by_override_pattern() {
    let mut overrides = HashMap::new();
    let mut rule = make_range_override("2", None, None, None, None);
    rule.reg_patterns = vec![r"(?i)batch(?P<episode>\d+)".to_string()];
    overrides.insert("2".to_string(), rule);

    assert_eq!(
        season_mode_guard(false, &[], &[7], &overrides, "Show Batch07", "src", false),
        SeasonModeDecision::Accept,
        "the override's season-level pattern claims this release"
    );
}

#[test]
fn test_phase_one_match_admits_otherwise_unowned_seasonless_release() {
    // No override claims it, but Phase 1 already confirmed the series identity
    // by pattern — so the missing season is tolerated instead of blocking.
    let overrides = HashMap::new();
    assert_eq!(
        season_mode_guard(false, &[], &[7], &overrides, "Show", "", true),
        SeasonModeDecision::AcceptViaPhaseOneMatch
    );
}

#[test]
fn test_alias_matches_padded_parsed_season() {
    // When parse_filename produces parsed_season_str="S01" (padded format),
    // alias_season_number=1 should match via the "S01" format.
    let mut season_overrides = HashMap::new();
    season_overrides.insert(
        "2".to_string(),
        make_range_override("2", Some(1), Some(1), Some(100), Some(100)),
    );

    let result = match_season_override(&season_overrides, "S01", Some(1), 150);
    assert_eq!(
        result,
        Some((2, 50)),
        "Alias should match padded parsed season 'S01'"
    );
}
