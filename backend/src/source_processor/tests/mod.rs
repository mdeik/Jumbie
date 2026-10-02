use std::collections::HashSet;

use std::sync::Arc;

use jumbie_shared::{
    config::SeasonPackStrategy,
    config::ui::ReleaseDateDisplayConfig,
    mapping::{EpisodeInfo, MappingRule, MonitorMode, SeasonOverride, SeriesSettings},
    parsing::{CustomParseResult, parse_title_with_custom_regex},
};

use crate::source_processor::*;

mod escape_tests;
mod gate_robustness_tests;
mod gate_tests;
mod manual_score_tests;
mod matcher_tests;
mod winner_tests;

/// Macro for simple `should_monitor_episode` tests that only vary
/// by (mode, season, ep, has_file) with all optional params as None.
/// Saves ~7 lines per test, keeping the signal (the params) without the noise.
macro_rules! monitor_test {
    ($name:ident, $expected:expr, $mode:expr, $season:expr, $ep:expr, $has_file:expr $(,)?) => {
        #[test]
        fn $name() {
            assert_eq!(
                ContentOrganizer::should_monitor_episode(
                    crate::source_processor::monitoring::MonitorParams {
                        mode: $mode,
                        season_str: $season,
                        ep_num: $ep,
                        has_file: $has_file,
                        currently_monitored: false,
                        effective_date: None,
                        season_override: None,
                    },
                ),
                $expected,
            );
        }
    };
}

// Helper: build a minimal EpisodeInfo with just the fields under test.
fn ep_info(ep: i32, ep_end: Option<i32>, is_pack: bool) -> EpisodeInfo {
    let episodes: Vec<i32> = if let Some(end) = ep_end {
        (ep..=end).collect()
    } else {
        vec![ep]
    };
    EpisodeInfo {
        is_season_pack: is_pack,
        series_key: "test".to_string(),
        raw_title: "Test Show S01E01".to_string(),
        file_ext: "mkv".to_string(),
        resolution: None,
        submitter: None,
        version: 1,
        part_number: None,
        is_complete_pack: false,
        seasons: vec![1],
        episodes,
        has_decimal_episode: false,
    }
}

fn no_downloads() -> HashSet<i32> {
    HashSet::new()
}

fn downloaded(eps: &[i32]) -> HashSet<i32> {
    eps.iter().copied().collect()
}

fn monitored(eps: &[i32]) -> HashSet<i32> {
    eps.iter().copied().collect()
}

fn all_monitored(start: i32, end: i32) -> HashSet<i32> {
    (start..=end).collect()
}

/// Build a minimal `ReleaseCandidate` for merge tests.
fn make_candidate(
    title: &str,
    download_url: Option<&str>,
    ep_num: i32,
    season: i32,
    series_id: &str,
    needed_episodes: Vec<i32>,
) -> crate::models::media::ReleaseCandidate {
    crate::models::media::ReleaseCandidate {
        title: title.to_string(),
        download_url: download_url.map(str::to_string),
        episode_info: EpisodeInfo {
            is_season_pack: false,
            series_key: "test".to_string(),
            raw_title: format!("{title}.mkv"),
            file_ext: "mkv".to_string(),
            resolution: None,
            submitter: None,
            version: 1,
            part_number: None,
            is_complete_pack: false,
            seasons: vec![season],
            episodes: vec![ep_num],
            has_decimal_episode: false,
        },
        mapping: Arc::new(MappingRule {
            series_id: series_id.to_string(),
            target_title: series_id.to_string(),
            name: series_id.to_string(),
            settings: SeriesSettings::default(),
            ..Default::default()
        }),
        meta_date: None,
        score: 100,
        score_breakdown: vec![],
        description: None,
        file_list: vec![],
        guid: None,
        needed_episodes,
        unneeded_count: 0,
        seeders: 0,
        leechers: 0,
        indexer: "test".to_string(),
        size_bytes: 0,
        download_id: None,
        multi_targets: vec![],
        submitter: None,
    }
}

// Single episode

#[test]
fn test_assess_pack_candidacy_single_needed() {
    // Single episode, not yet downloaded → needed, no penalty
    let info = ep_info(5, None, false);
    let (penalty, needed, unneeded) = ContentOrganizer::assess_pack_candidacy(
        &info,
        &no_downloads(),
        &monitored(&[5]),
        None,
        &SeasonPackStrategy::FavorEpisodes,
        70,
    );
    assert_eq!(penalty, 0);
    assert_eq!(needed, vec![5]);
    assert_eq!(unneeded, 0);
}

#[test]
fn test_assess_pack_candidacy_single_downloaded() {
    // Single episode, already downloaded → large penalty, not needed
    let info = ep_info(5, None, false);
    let (penalty, needed, unneeded) = ContentOrganizer::assess_pack_candidacy(
        &info,
        &downloaded(&[5]),
        &monitored(&[5]),
        None,
        &SeasonPackStrategy::FavorEpisodes,
        70,
    );
    assert_eq!(penalty, -1000);
    assert!(needed.is_empty());
    assert_eq!(unneeded, 1);
}

#[test]
fn test_assess_pack_candidacy_single_unmonitored() {
    // Single episode, NOT monitored → large penalty, not needed
    let info = ep_info(5, None, false);
    let (penalty, needed, unneeded) = ContentOrganizer::assess_pack_candidacy(
        &info,
        &no_downloads(),
        &no_downloads(), // Nothing monitored
        None,
        &SeasonPackStrategy::FavorEpisodes,
        70,
    );
    assert_eq!(penalty, -1000);
    assert!(needed.is_empty());
    assert_eq!(unneeded, 1);
}

// Season pack

#[test]
fn test_assess_pack_candidacy_pack_all_needed() {
    // Full pack (E1-E12), nothing downloaded → no penalty, all needed
    let info = ep_info(1, Some(12), true);
    let (penalty, needed, unneeded) = ContentOrganizer::assess_pack_candidacy(
        &info,
        &no_downloads(),
        &all_monitored(1, 12),
        Some(12),
        &SeasonPackStrategy::FavorEpisodes,
        70,
    );
    assert_eq!(penalty, 0);
    assert_eq!(needed.len(), 12);
    assert_eq!(unneeded, 0);
}

#[test]
fn test_assess_pack_candidacy_pack_above_threshold_triggers_full_replace() {
    // 10/12 needed (83%) → above 70% threshold → all 12 marked needed, no penalty
    let info = ep_info(1, Some(12), true);
    let (penalty, needed, unneeded) = ContentOrganizer::assess_pack_candidacy(
        &info,
        &downloaded(&[1, 2]), // 2 already have, 10 needed
        &all_monitored(1, 12),
        Some(12),
        &SeasonPackStrategy::FavorEpisodes,
        70,
    );
    assert_eq!(
        penalty, 0,
        "Above threshold — full pack replace, no penalty"
    );
    assert_eq!(
        needed.len(),
        12,
        "All eps should be needed after threshold met"
    );
    assert_eq!(unneeded, 0);
}

#[test]
fn test_assess_pack_candidacy_pack_below_threshold() {
    // Only 3/12 needed (25%) → below 70% threshold → penalty applies
    let info = ep_info(1, Some(12), true);
    let already: HashSet<i32> = (1..=9).collect(); // 9 downloaded, 3 needed
    let (penalty, needed, unneeded) = ContentOrganizer::assess_pack_candidacy(
        &info,
        &already,
        &all_monitored(1, 12),
        Some(12),
        &SeasonPackStrategy::FavorEpisodes,
        70,
    );
    assert!(
        penalty < 0,
        "Below threshold should incur a penalty: {}",
        penalty
    );
    assert_eq!(needed.len(), 3);
    assert_eq!(unneeded, 9);
}

#[test]
fn test_assess_pack_candidacy_favor_packs_smaller_penalty() {
    // Same scenario but FavorSeasonPacks → smaller per-ep penalty
    let info = ep_info(1, Some(12), true);
    let already: HashSet<i32> = (1..=6).collect(); // 6 downloaded → 50% needed, below 70%
    let (penalty_strict, _, _) = ContentOrganizer::assess_pack_candidacy(
        &info,
        &already,
        &all_monitored(1, 12),
        Some(12),
        &SeasonPackStrategy::FavorEpisodes,
        70,
    );
    let (penalty_favor, _, _) = ContentOrganizer::assess_pack_candidacy(
        &info,
        &already,
        &all_monitored(1, 12),
        Some(12),
        &SeasonPackStrategy::FavorSeasonPacks,
        70,
    );
    // FavorSeasonPacks uses a smaller base unneeded penalty per episode
    assert!(
        penalty_favor > penalty_strict,
        "FavorSeasonPacks should have smaller penalty: {} vs {}",
        penalty_favor,
        penalty_strict
    );
}

#[test]
fn test_assess_pack_candidacy_non_pack_range_penalties() {
    // Range file (E1-E3, is_season_pack=false) with one already downloaded
    let info = ep_info(1, Some(3), false);
    let (penalty, needed, unneeded) = ContentOrganizer::assess_pack_candidacy(
        &info,
        &downloaded(&[2]),
        &all_monitored(1, 3),
        Some(3),
        &SeasonPackStrategy::FavorEpisodes,
        70,
    );
    // Threshold logic only applies when is_season_pack=true, so normal penalty
    assert!(penalty < 0);
    assert_eq!(needed, vec![1, 3]);
    assert_eq!(unneeded, 1);
}

#[test]
fn test_assess_pack_candidacy_non_pack_range_all_needed() {
    // Multi-episode range (E2-E5), nothing downloaded → all needed
    // Verifies the matched episode_num (2) is the START of the range.
    let info = ep_info(2, Some(5), false);
    let (penalty, needed, unneeded) = ContentOrganizer::assess_pack_candidacy(
        &info,
        &no_downloads(),
        &all_monitored(2, 5),
        Some(5),
        &SeasonPackStrategy::FavorEpisodes,
        70,
    );
    assert_eq!(penalty, 0);
    assert_eq!(needed, vec![2, 3, 4, 5]);
    assert_eq!(unneeded, 0);
}

#[test]
fn test_assess_pack_candidacy_non_pack_range_start_already_downloaded() {
    // Multi-episode range (E2-E5), E02 already downloaded
    // → needed = [3, 4, 5] (start is skipped)
    // Verifies `needed_episodes` doesn't include already-downloaded start.
    let info = ep_info(2, Some(5), false);
    let (penalty, needed, unneeded) = ContentOrganizer::assess_pack_candidacy(
        &info,
        &downloaded(&[2]),
        &all_monitored(2, 5),
        Some(5),
        &SeasonPackStrategy::FavorEpisodes,
        70,
    );
    assert!(penalty < 0);
    assert_eq!(needed, vec![3, 4, 5]);
    assert_eq!(unneeded, 1);
}

#[test]
fn test_assess_pack_candidacy_non_pack_range_end_already_downloaded() {
    // Multi-episode range (E2-E5), E05 already downloaded
    // → needed = [2, 3, 4] (end is skipped)
    // Verifies `needed_episodes` doesn't include already-downloaded end.
    let info = ep_info(2, Some(5), false);
    let (penalty, needed, unneeded) = ContentOrganizer::assess_pack_candidacy(
        &info,
        &downloaded(&[5]),
        &all_monitored(2, 5),
        Some(5),
        &SeasonPackStrategy::FavorEpisodes,
        70,
    );
    assert!(penalty < 0);
    assert_eq!(needed, vec![2, 3, 4]);
    assert_eq!(unneeded, 1);
}

#[test]
fn test_assess_pack_candidacy_non_pack_range_middle_already_downloaded() {
    // Multi-episode range (E2-E5), E03 already downloaded
    // → needed = [2, 4, 5] (middle gap is fine)
    // Verifies `needed_episodes` correctly handles a gap.
    // download_winner uses min()/max() on this to derive the range.
    let info = ep_info(2, Some(5), false);
    let (penalty, needed, unneeded) = ContentOrganizer::assess_pack_candidacy(
        &info,
        &downloaded(&[3]),
        &all_monitored(2, 5),
        Some(5),
        &SeasonPackStrategy::FavorEpisodes,
        70,
    );
    assert!(penalty < 0);
    assert_eq!(needed, vec![2, 4, 5]);
    assert_eq!(unneeded, 1);
}

#[test]
fn test_assess_pack_candidacy_multi_episode_download_winner_derived_range() {
    // Simulates what download_winner does: take min()/max() of needed_episodes
    // to derive the insertion range. This must work regardless of which
    // episodes in the range are already downloaded.
    //
    // Scenario: Multi-ep release covering E2-E5, only E4-E5 needed (E2-E3 done).
    // download_winner should: start=4, end=5 → inserts E04 and E05.
    let info = ep_info(2, Some(5), false);
    let (_, needed, _) = ContentOrganizer::assess_pack_candidacy(
        &info,
        &downloaded(&[2, 3]),
        &all_monitored(2, 5),
        Some(5),
        &SeasonPackStrategy::FavorEpisodes,
        70,
    );

    let start_ep = needed
        .iter()
        .min()
        .copied()
        .unwrap_or(info.episodes.first().copied().unwrap_or(1));
    let end_ep = needed
        .iter()
        .max()
        .copied()
        .unwrap_or(info.episodes.last().copied().unwrap_or(start_ep));

    assert_eq!(start_ep, 4, "min of needed=[4,5] should be 4");
    assert_eq!(end_ep, 5, "max of needed=[4,5] should be 5");
    assert_eq!(needed, vec![4, 5]);
}

#[test]
fn test_assess_pack_candidacy_multi_episode_all_downloaded_penalty() {
    // Multi-episode range (E2-E5), ALL already downloaded → large penalty
    // Verifies the function correctly rejects fully-redundant ranges.
    let info = ep_info(2, Some(5), false);
    let (penalty, needed, unneeded) = ContentOrganizer::assess_pack_candidacy(
        &info,
        &all_monitored(2, 5), // everything downloaded
        &all_monitored(2, 5),
        Some(5),
        &SeasonPackStrategy::FavorEpisodes,
        70,
    );
    assert_eq!(
        penalty, -200,
        "All downloaded multi-ep range (4 unneeded * -50) should be -200"
    );
    assert!(needed.is_empty());
    assert_eq!(unneeded, 4);
}

#[test]
fn test_assess_pack_candidacy_non_pack_range_episode_start_equals_end() {
    // episode_num=episode_end=5 (single episode expressed as range)
    // Should behave like a single episode.
    let info = ep_info(5, Some(5), false);
    let (penalty, needed, unneeded) = ContentOrganizer::assess_pack_candidacy(
        &info,
        &no_downloads(),
        &monitored(&[5]),
        Some(5),
        &SeasonPackStrategy::FavorEpisodes,
        70,
    );
    assert_eq!(penalty, 0);
    assert_eq!(needed, vec![5]);
    assert_eq!(unneeded, 0);
}

#[test]
fn test_assess_pack_candidacy_zero_threshold_forces_full_replace() {
    // threshold=0 → any non-empty needed set triggers full replace
    let info = ep_info(1, Some(5), true);
    let (penalty, needed, unneeded) = ContentOrganizer::assess_pack_candidacy(
        &info,
        &downloaded(&[1, 2, 3, 4]), // 4/5 downloaded, only 1 needed
        &all_monitored(1, 5),
        Some(5),
        &SeasonPackStrategy::FavorEpisodes,
        0, // threshold = 0%: every pack qualifies
    );
    assert_eq!(penalty, 0);
    assert_eq!(
        needed.len(),
        5,
        "All episodes should be needed when threshold is 0"
    );
    assert_eq!(unneeded, 0);
}

#[test]
fn test_assess_pack_candidacy_season_pack_all_downloaded_empty_needed() {
    // Season pack (E1-E12), ALL episodes already downloaded
    // → needed = [], penalty = 12 * 50 = -600
    // This is the scenario that triggers the version-upgrade bypass in
    // select_winners: needed_episodes is empty, but the pack may be a
    // version upgrade (handled by the upgrade gate, not by this function).
    let info = ep_info(1, Some(12), true);
    let (penalty, needed, unneeded) = ContentOrganizer::assess_pack_candidacy(
        &info,
        &all_monitored(1, 12),
        &all_monitored(1, 12),
        Some(12),
        &SeasonPackStrategy::FavorEpisodes,
        70,
    );
    assert_eq!(penalty, -600);
    assert!(needed.is_empty());
    assert_eq!(unneeded, 12);
}

#[test]
fn test_assess_pack_candidacy_season_pack_all_downloaded_unmonitored() {
    // Season pack (E1-E12), ALL downloaded but NONE monitored
    // → needed = [] (unmonitored = unneeded), penalty applies
    // Even with version upgrade, unmonitored episodes are replaced
    // (matching threshold behavior) — but only if the version bump
    // is detected in the upgrade gate.
    let info = ep_info(1, Some(12), true);
    let (penalty, needed, unneeded) = ContentOrganizer::assess_pack_candidacy(
        &info,
        &downloaded(&(1..=12).collect::<Vec<_>>()),
        &HashSet::new(), // nothing monitored
        Some(12),
        &SeasonPackStrategy::FavorEpisodes,
        70,
    );
    assert_eq!(penalty, -600);
    assert!(needed.is_empty());
    assert_eq!(unneeded, 12);
}

#[test]
fn test_assess_pack_candidacy_season_0_pack() {
    // Season 0 (specials) pack — scoring works normally, but
    // select_winners blocks it from auto-download.
    // Verifies assess_pack_candidacy itself doesn't need to change.
    let mut info = ep_info(1, Some(12), true);
    info.seasons = vec![0];
    let (penalty, needed, unneeded) = ContentOrganizer::assess_pack_candidacy(
        &info,
        &HashSet::new(), // nothing downloaded
        &all_monitored(1, 12),
        Some(12),
        &SeasonPackStrategy::FavorEpisodes,
        70,
    );
    assert_eq!(penalty, 0, "All needed → no penalty");
    assert_eq!(needed.len(), 12);
    assert_eq!(unneeded, 0);
}

// should_monitor_episode tests
// The SSoT function `should_monitor_episode` lives on `ContentOrganizer`
// and is used by BOTH `apply_monitor_mode` (existing DB episodes) and
// `select_winners` (newly-discovered RSS episodes).  Every test below
// passes `has_file = false` to simulate a non-DB episode.
//
// `select_winners` (newly-discovered RSS episodes).  Every test below
// passes `has_file = false` to simulate a non-DB episode.
//
// Each test constructs parameters directly — no DB, no async.

// MonitorMode::All
// Logic: !is_special — all regular episodes monitored, specials excluded.
// test_all_with_file_upgrades_enabled since All mode ignores the setting.
monitor_test!(
    test_new_episode_all_normal,
    true,
    Some(MonitorMode::All),
    "1",
    5,
    false
);
monitor_test!(
    test_new_episode_all_special_0,
    false,
    Some(MonitorMode::All),
    "0",
    1,
    false
);
monitor_test!(
    test_new_episode_all_special_00,
    false,
    Some(MonitorMode::All),
    "00",
    1,
    false
);
monitor_test!(
    test_all_with_file_upgrades_enabled,
    true,
    Some(MonitorMode::All),
    "1",
    5,
    true
);

// MonitorMode::Future

#[test]
fn test_new_episode_future_after_max() {
    // Future mode: episode AFTER the latest known but no meta_date → NOT monitored
    // Future now only uses dates, so position alone is insufficient.
    assert!(!ContentOrganizer::should_monitor_episode(
        crate::source_processor::monitoring::MonitorParams {
            mode: Some(MonitorMode::Future),
            season_str: "1",
            ep_num: 8, // ep 8
            has_file: false,
            currently_monitored: false, // non-DB: not previously monitored
            effective_date: None,       // no meta_date
            season_override: None,
        },
    ));
}

#[test]
fn test_new_episode_future_before_max() {
    // Future mode: episode BEFORE the latest known → NOT monitored
    assert!(!ContentOrganizer::should_monitor_episode(
        crate::source_processor::monitoring::MonitorParams {
            mode: Some(MonitorMode::Future),
            season_str: "1",
            ep_num: 5, // ep 5
            has_file: false,
            currently_monitored: false, // non-DB: not previously monitored
            effective_date: None,
            season_override: None,
        },
    ));
}

#[test]
fn test_new_episode_future_at_max() {
    // Future mode: episode EQUAL to max → NOT monitored (already released)
    assert!(!ContentOrganizer::should_monitor_episode(
        crate::source_processor::monitoring::MonitorParams {
            mode: Some(MonitorMode::Future),
            season_str: "1",
            ep_num: 7,
            has_file: false,
            currently_monitored: false, // non-DB: not previously monitored
            effective_date: None,
            season_override: None,
        },
    ));
}

#[test]
fn test_new_episode_future_no_existing() {
    // Future mode: no episodes exist yet and no meta_date → NOT monitored
    // Future now only uses dates — without a date there's no way to know.
    assert!(!ContentOrganizer::should_monitor_episode(
        crate::source_processor::monitoring::MonitorParams {
            mode: Some(MonitorMode::Future),
            season_str: "1",
            ep_num: 1,
            has_file: false,
            currently_monitored: false, // non-DB: not previously monitored
            effective_date: None,
            season_override: None,
        },
    ));
}

#[test]
fn test_new_episode_future_future_date() {
    // Future mode: meta_date is in the future → monitored even if before max
    let future = chrono::Utc::now() + chrono::Duration::days(30);
    assert!(ContentOrganizer::should_monitor_episode(
        crate::source_processor::monitoring::MonitorParams {
            mode: Some(MonitorMode::Future),
            season_str: "1",
            ep_num: 3,
            has_file: false,
            currently_monitored: false, // non-DB: not previously monitored
            effective_date: Some(future.naive_utc()),
            season_override: None,
        },
    ));
}

#[test]
fn test_new_episode_future_no_date_not_monitored() {
    // Future mode: no meta_date → NOT monitored regardless of position
    // Since Future now only uses dates, an undated episode is never future.
    assert!(!ContentOrganizer::should_monitor_episode(
        crate::source_processor::monitoring::MonitorParams {
            mode: Some(MonitorMode::Future),
            season_str: "2", // season 2 (after max S01E07 — would have been "future" under old logic)
            ep_num: 1,
            has_file: false,
            currently_monitored: false, // non-DB: not previously monitored
            effective_date: None,
            season_override: None,
        },
    ));
}

#[test]
fn test_new_episode_future_different_season_before() {
    // Future mode: episode in a season BEFORE the max season → NOT monitored
    // (This is a past season, already aired)
    assert!(!ContentOrganizer::should_monitor_episode(
        crate::source_processor::monitoring::MonitorParams {
            mode: Some(MonitorMode::Future),
            season_str: "1",
            ep_num: 3,
            has_file: false,
            currently_monitored: false, // non-DB: not previously monitored
            effective_date: None,
            season_override: None,
        },
    ));
}

#[test]
fn test_future_special_not_monitored() {
    // Future mode: special episode (season "0") → NOT monitored
    // Specials are never "future" because season 0 < any real season in max_released.
    assert!(!ContentOrganizer::should_monitor_episode(
        crate::source_processor::monitoring::MonitorParams {
            mode: Some(MonitorMode::Future),
            season_str: "0",
            ep_num: 1,
            has_file: false,
            currently_monitored: false, // non-DB: not previously monitored
            effective_date: None,
            season_override: None,
        },
    ));
}

#[test]
fn test_future_has_file_upgrades_enabled_past() {
    // Future mode: has_file + upgrades enabled, but episode is past max
    // Past episodes are not monitored even when upgrades are enabled — the user
    // should use Existing mode to watch past episodes for upgrades.
    assert!(!ContentOrganizer::should_monitor_episode(
        crate::source_processor::monitoring::MonitorParams {
            mode: Some(MonitorMode::Future),
            season_str: "1",
            ep_num: 5,
            has_file: true,
            currently_monitored: false, // non-DB: not previously monitored
            effective_date: None,
            season_override: None,
        },
    ));
}

#[test]
fn test_future_has_file_upgrades_enabled_future_position() {
    // Future mode: has_file + upgrades enabled, episode is beyond max but no date
    // Without a future date, position alone no longer makes it "future".
    assert!(!ContentOrganizer::should_monitor_episode(
        crate::source_processor::monitoring::MonitorParams {
            mode: Some(MonitorMode::Future),
            season_str: "1",
            ep_num: 8,
            has_file: true,
            currently_monitored: false, // non-DB: not previously monitored
            effective_date: None,
            season_override: None,
        },
    ));
}

#[test]
fn test_future_has_file_upgrades_enabled_future_date() {
    // Future mode: has_file + upgrades enabled, meta_date is in the future
    // A future-dated episode should be monitored for upgrades even if a file exists.
    let future = chrono::Utc::now() + chrono::Duration::days(30);
    assert!(ContentOrganizer::should_monitor_episode(
        crate::source_processor::monitoring::MonitorParams {
            mode: Some(MonitorMode::Future),
            season_str: "1",
            ep_num: 3,
            has_file: true,
            currently_monitored: false, // non-DB: not previously monitored
            effective_date: Some(future.naive_utc()),
            season_override: None,
        },
    ));
}

#[test]
fn test_future_past_date_not_monitored() {
    // Future mode: past meta_date → NOT monitored
    // Without a future date, the episode is not considered future.
    let past = chrono::Utc::now() - chrono::Duration::days(30);
    assert!(!ContentOrganizer::should_monitor_episode(
        crate::source_processor::monitoring::MonitorParams {
            mode: Some(MonitorMode::Future),
            season_str: "1",
            ep_num: 1,
            has_file: false,
            currently_monitored: false, // non-DB: not previously monitored
            effective_date: Some(past.naive_utc()),
            season_override: None,
        },
    ));
}

// Future mode keep-monitoring tests
// These verify that episodes which were previously future-dated (currently_monitored=true)
// remain monitored after their date passes, but only until downloaded.

#[test]
fn test_future_past_date_previously_monitored_still_missing() {
    // Future mode: episode was previously monitored (was future), now past
    // but still missing a file → should remain monitored
    let past = chrono::Utc::now() - chrono::Duration::days(30);
    assert!(ContentOrganizer::should_monitor_episode(
        crate::source_processor::monitoring::MonitorParams {
            mode: Some(MonitorMode::Future),
            season_str: "1",
            ep_num: 1,
            has_file: false,           // no file yet
            currently_monitored: true, // currently_monitored = true (was future)
            effective_date: Some(past.naive_utc()),
            season_override: None,
        },
    ));
}

#[test]
fn test_future_past_date_previously_monitored_now_downloaded() {
    // Future mode: episode was previously monitored, now past AND downloaded
    // → should NOT be monitored (already have the file)
    let past = chrono::Utc::now() - chrono::Duration::days(30);
    assert!(!ContentOrganizer::should_monitor_episode(
        crate::source_processor::monitoring::MonitorParams {
            mode: Some(MonitorMode::Future),
            season_str: "1",
            ep_num: 1,
            has_file: true,            // has file (downloaded)
            currently_monitored: true, // currently_monitored = true (was future)
            effective_date: Some(past.naive_utc()),
            season_override: None,
        },
    ));
}

#[test]
fn test_future_past_date_never_monitored() {
    // Future mode: episode was NEVER monitored before (user just switched to Future),
    // date is past → should NOT be monitored (don't retroactively pick up old eps)
    let past = chrono::Utc::now() - chrono::Duration::days(30);
    assert!(!ContentOrganizer::should_monitor_episode(
        crate::source_processor::monitoring::MonitorParams {
            mode: Some(MonitorMode::Future),
            season_str: "1",
            ep_num: 1,
            has_file: false,            // no file
            currently_monitored: false, // currently_monitored = false (never was future)
            effective_date: Some(past.naive_utc()),
            season_override: None,
        },
    ));
}

#[test]
fn test_future_no_date_previously_monitored() {
    // Future mode: episode was previously monitored but has no effective date
    // → should NOT be monitored (can't determine if it's future)
    assert!(!ContentOrganizer::should_monitor_episode(
        crate::source_processor::monitoring::MonitorParams {
            mode: Some(MonitorMode::Future),
            season_str: "1",
            ep_num: 1,
            has_file: false,           // no file
            currently_monitored: true, // was monitored
            effective_date: None,      // no effective date
            season_override: None,
        },
    ));
}

#[test]
fn test_future_future_date_never_monitored() {
    // Future mode: episode with a future date, never monitored before
    // → should be monitored (this is the initial apply case when user switches to Future)
    let future = chrono::Utc::now() + chrono::Duration::days(30);
    assert!(ContentOrganizer::should_monitor_episode(
        crate::source_processor::monitoring::MonitorParams {
            mode: Some(MonitorMode::Future),
            season_str: "1",
            ep_num: 1,
            has_file: false,            // no file
            currently_monitored: false, // currently_monitored = false (fresh apply)
            effective_date: Some(future.naive_utc()),
            season_override: None,
        },
    ));
}

#[test]
fn test_future_past_date_previously_monitored_upgrades_disabled() {
    // Future mode: was future, now past, still missing
    // → should remain monitored
    let past = chrono::Utc::now() - chrono::Duration::days(30);
    assert!(ContentOrganizer::should_monitor_episode(
        crate::source_processor::monitoring::MonitorParams {
            mode: Some(MonitorMode::Future),
            season_str: "1",
            ep_num: 1,
            has_file: false,           // no file
            currently_monitored: true, // currently_monitored = true
            effective_date: Some(past.naive_utc()),
            season_override: None,
        },
    ));
}

#[test]
fn test_future_past_date_previously_monitored_downloaded_upgrades_enabled() {
    // Future mode: was future, now past, downloaded
    // → should NOT be monitored (Future mode doesn't seek upgrades for past eps)
    let past = chrono::Utc::now() - chrono::Duration::days(30);
    assert!(!ContentOrganizer::should_monitor_episode(
        crate::source_processor::monitoring::MonitorParams {
            mode: Some(MonitorMode::Future),
            season_str: "1",
            ep_num: 1,
            has_file: true,            // has file
            currently_monitored: true, // was monitored
            effective_date: Some(past.naive_utc()),
            season_override: None,
        },
    ));
}

// Future mode same-day / edge-case tests
// These verify that episodes releasing today (same calendar date as now) are
// properly handled, including the narrow edge case where a user switches to
// Future mode at the exact moment of release.

#[test]
fn test_future_same_day_date_fresh_apply_no_file() {
    // Same calendar day as now, never monitored, no file → should be monitored
    // (the release hasn't happened yet today; date-only metadata was set to midnight)
    let today_midnight = chrono::Utc::now()
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_utc()
        .naive_utc();
    assert!(ContentOrganizer::should_monitor_episode(
        crate::source_processor::monitoring::MonitorParams {
            mode: Some(MonitorMode::Future),
            season_str: "1",
            ep_num: 1,
            has_file: false,            // no file
            currently_monitored: false, // currently_monitored = false (fresh apply)
            effective_date: Some(today_midnight),
            season_override: None,
        },
    ));
}

#[test]
fn test_future_same_day_date_fresh_apply_has_file() {
    // Same day as now, never monitored, but already downloaded → not monitored
    let today_midnight = chrono::Utc::now()
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_utc()
        .naive_utc();
    assert!(!ContentOrganizer::should_monitor_episode(
        crate::source_processor::monitoring::MonitorParams {
            mode: Some(MonitorMode::Future),
            season_str: "1",
            ep_num: 1,
            has_file: true,             // has file
            currently_monitored: false, // currently_monitored = false
            effective_date: Some(today_midnight),
            season_override: None,
        },
    ));
}

#[test]
fn test_future_same_day_date_sweep_was_monitored() {
    // Same day, was previously monitored (was future yesterday), still missing
    // → should remain monitored
    let today_midnight = chrono::Utc::now()
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_utc()
        .naive_utc();
    assert!(ContentOrganizer::should_monitor_episode(
        crate::source_processor::monitoring::MonitorParams {
            mode: Some(MonitorMode::Future),
            season_str: "1",
            ep_num: 1,
            has_file: false,           // no file
            currently_monitored: true, // currently_monitored = true (was future)
            effective_date: Some(today_midnight),
            season_override: None,
        },
    ));
}

#[test]
fn test_future_same_day_date_sweep_was_monitored_has_file() {
    // Same day, was previously monitored, but already downloaded → not monitored
    let today_midnight = chrono::Utc::now()
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_utc()
        .naive_utc();
    assert!(!ContentOrganizer::should_monitor_episode(
        crate::source_processor::monitoring::MonitorParams {
            mode: Some(MonitorMode::Future),
            season_str: "1",
            ep_num: 1,
            has_file: true,            // has file
            currently_monitored: true, // currently_monitored = true
            effective_date: Some(today_midnight),
            season_override: None,
        },
    ));
}

#[test]
fn test_future_same_day_date_fresh_apply_never_monitored_upgrades_off() {
    // Same day, never monitored, no file → should be monitored
    let today_midnight = chrono::Utc::now()
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_utc()
        .naive_utc();
    assert!(ContentOrganizer::should_monitor_episode(
        crate::source_processor::monitoring::MonitorParams {
            mode: Some(MonitorMode::Future),
            season_str: "1",
            ep_num: 1,
            has_file: false,            // no file
            currently_monitored: false, // currently_monitored = false
            effective_date: Some(today_midnight),
            season_override: None,
        },
    ));
}

#[test]
fn test_future_same_day_fresh_apply_later_timestamp() {
    // Same calendar day but timestamp is later today (e.g. 8 PM airing),
    // never monitored, no file → should be monitored
    let later_today = chrono::Utc::now()
        .date_naive()
        .and_hms_opt(20, 0, 0)
        .unwrap()
        .and_utc()
        .naive_utc();
    // Only proceed if 8 PM hasn't passed yet
    if later_today > chrono::Utc::now().naive_utc() {
        assert!(ContentOrganizer::should_monitor_episode(
            crate::source_processor::monitoring::MonitorParams {
                mode: Some(MonitorMode::Future),
                season_str: "1",
                ep_num: 1,
                has_file: false,
                currently_monitored: false,
                effective_date: Some(later_today),
                season_override: None,
            },
        ));
    }
}

#[test]
fn test_future_yesterday_date_fresh_apply() {
    // Yesterday's date, never monitored (fresh switch to Future) → not monitored
    let yesterday = (chrono::Utc::now() - chrono::Duration::days(1))
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_utc()
        .naive_utc();
    assert!(!ContentOrganizer::should_monitor_episode(
        crate::source_processor::monitoring::MonitorParams {
            mode: Some(MonitorMode::Future),
            season_str: "1",
            ep_num: 1,
            has_file: false,            // no file
            currently_monitored: false, // currently_monitored = false (fresh apply)
            effective_date: Some(yesterday),
            season_override: None,
        },
    ));
}

#[test]
fn test_future_yesterday_date_sweep_was_monitored() {
    // Yesterday's date, was previously monitored, still missing → should remain
    let yesterday = (chrono::Utc::now() - chrono::Duration::days(1))
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_utc()
        .naive_utc();
    assert!(ContentOrganizer::should_monitor_episode(
        crate::source_processor::monitoring::MonitorParams {
            mode: Some(MonitorMode::Future),
            season_str: "1",
            ep_num: 1,
            has_file: false,           // no file
            currently_monitored: true, // currently_monitored = true (was future)
            effective_date: Some(yesterday),
            season_override: None,
        },
    ));
}

// MonitorMode::Missing
// Logic: !has_file — monitor only episodes without a file.
// upgrades_enabled vs upgrades_disabled produce the same result
// for Missing mode, so only the enabled variant is tested.
monitor_test!(
    test_missing_no_file,
    true,
    Some(MonitorMode::Missing),
    "1",
    5,
    false
);
monitor_test!(
    test_missing_special,
    true,
    Some(MonitorMode::Missing),
    "0",
    1,
    false
);
monitor_test!(
    test_missing_has_file_upgrades_enabled,
    false,
    Some(MonitorMode::Missing),
    "1",
    5,
    true
);

// MonitorMode::Existing
// Logic: has_file — monitor only episodes that already have a file.
// upgrades_enabled vs upgrades_disabled produce the same result
// for Existing mode, so only the enabled variant is tested.
monitor_test!(
    test_existing_no_file,
    false,
    Some(MonitorMode::Existing),
    "1",
    5,
    false
);
monitor_test!(
    test_existing_has_file_upgrades_enabled,
    true,
    Some(MonitorMode::Existing),
    "1",
    5,
    true
);

// MonitorMode::Pilot
// Logic: is_season_1 && ep_num == 1
// upgrades_enabled vs upgrades_disabled produce the same result;
// no_file_upgrades_disabled is covered by s01e01_no_file.
monitor_test!(
    test_pilot_s01e01_no_file,
    true,
    Some(MonitorMode::Pilot),
    "1",
    1,
    false
);
monitor_test!(
    test_pilot_s01e02,
    false,
    Some(MonitorMode::Pilot),
    "1",
    2,
    false
);
monitor_test!(
    test_pilot_s02e01,
    false,
    Some(MonitorMode::Pilot),
    "2",
    1,
    false
);
monitor_test!(
    test_pilot_s01e01_has_file_upgrades_enabled,
    true,
    Some(MonitorMode::Pilot),
    "1",
    1,
    true
);

// MonitorMode::FirstSeason -
// Logic: is_season_1 — monitor all season 1 episodes.
// upgrades_enabled vs upgrades_disabled produce the same result;
// no_file_upgrades_disabled is covered by s01_no_file.
monitor_test!(
    test_first_season_s01_no_file,
    true,
    Some(MonitorMode::FirstSeason),
    "1",
    5,
    false
);
monitor_test!(
    test_first_season_s02,
    false,
    Some(MonitorMode::FirstSeason),
    "2",
    1,
    false
);
monitor_test!(
    test_first_season_s01e01,
    true,
    Some(MonitorMode::FirstSeason),
    "1",
    1,
    false
);
monitor_test!(
    test_first_season_s01_has_file_upgrades_enabled,
    true,
    Some(MonitorMode::FirstSeason),
    "1",
    5,
    true
);
monitor_test!(
    test_first_season_special_not_monitored,
    false,
    Some(MonitorMode::FirstSeason),
    "0",
    1,
    false
);

// MonitorMode::Specials
// Logic: is_special — monitor only special episodes.
// upgrades_enabled vs upgrades_disabled produce the same result;
// no_file_upgrades_disabled is covered by s00_no_file.
monitor_test!(
    test_specials_s00_no_file,
    true,
    Some(MonitorMode::Specials),
    "0",
    1,
    false
);
monitor_test!(
    test_specials_s01_not_monitored,
    false,
    Some(MonitorMode::Specials),
    "1",
    1,
    false
);
monitor_test!(
    test_specials_has_file_upgrades_enabled,
    true,
    Some(MonitorMode::Specials),
    "0",
    1,
    true
);

// MonitorMode::None
// Logic: false — nothing is ever monitored.
// upgrades_enabled vs upgrades_disabled produce the same result.
monitor_test!(
    test_none_regular,
    false,
    Some(MonitorMode::None),
    "1",
    1,
    false
);
monitor_test!(
    test_none_special,
    false,
    Some(MonitorMode::None),
    "0",
    1,
    false
);
monitor_test!(
    test_none_has_file_upgrades_enabled,
    false,
    Some(MonitorMode::None),
    "1",
    1,
    true
);

// No mode set (None/Default)
// Logic: Option::None → nothing is monitored (same as MonitorMode::None).
// upgrades_enabled vs upgrades_disabled produce the same result.
monitor_test!(test_no_mode_regular, false, None, "1", 5, false);
monitor_test!(
    test_no_mode_has_file_upgrades_enabled,
    false,
    None,
    "1",
    5,
    true
);

// Season override episode range

#[test]
fn test_new_episode_season_override_inside_range() {
    // Season override: episode inside range → monitored (with All mode)
    let override_rule = SeasonOverride {
        season: "1".to_string(),
        episode_start: Some(1),
        episode_end: Some(7),
        cell_count: None,
        episode_offset: None,
        alias_season_number: None,
        search_format: None,
        aliases: vec![],
        reg_patterns: vec![],
    };
    assert!(ContentOrganizer::should_monitor_episode(
        crate::source_processor::monitoring::MonitorParams {
            mode: Some(MonitorMode::All),
            season_str: "1",
            ep_num: 5,
            has_file: false,
            currently_monitored: false, // non-DB: not previously monitored
            effective_date: None,
            season_override: Some(&override_rule),
        },
    ));
}

#[test]
fn test_new_episode_season_override_outside_range() {
    // Season override: episode outside range → NOT monitored regardless of mode
    let override_rule = SeasonOverride {
        season: "1".to_string(),
        episode_start: Some(1),
        episode_end: Some(7),
        cell_count: None,
        episode_offset: None,
        alias_season_number: None,
        search_format: None,
        aliases: vec![],
        reg_patterns: vec![],
    };
    // Future mode would normally monitor ep 8 (after max 7), but range
    // check blocks it first.
    assert!(!ContentOrganizer::should_monitor_episode(
        crate::source_processor::monitoring::MonitorParams {
            mode: Some(MonitorMode::Future),
            season_str: "1",
            ep_num: 8, // outside 1-7 range
            has_file: false,
            currently_monitored: false, // non-DB: not previously monitored
            effective_date: None,
            season_override: Some(&override_rule),
        },
    ));
}

#[test]
fn test_new_episode_season_override_no_start() {
    // Season override: only episode_end set → blocks everything after end
    let override_rule = SeasonOverride {
        season: "1".to_string(),
        episode_start: None,
        episode_end: Some(7),
        cell_count: None,
        episode_offset: None,
        alias_season_number: None,
        search_format: None,
        aliases: vec![],
        reg_patterns: vec![],
    };
    assert!(!ContentOrganizer::should_monitor_episode(
        crate::source_processor::monitoring::MonitorParams {
            mode: Some(MonitorMode::All),
            season_str: "1",
            ep_num: 8, // beyond end
            has_file: false,
            currently_monitored: false, // non-DB: not previously monitored
            effective_date: None,
            season_override: Some(&override_rule),
        },
    ));
}

#[test]
fn test_new_episode_season_override_no_end() {
    // Season override: only episode_start set → blocks everything before start
    let override_rule = SeasonOverride {
        season: "1".to_string(),
        episode_start: Some(3),
        episode_end: None,
        cell_count: None,
        episode_offset: None,
        alias_season_number: None,
        search_format: None,
        aliases: vec![],
        reg_patterns: vec![],
    };
    assert!(!ContentOrganizer::should_monitor_episode(
        crate::source_processor::monitoring::MonitorParams {
            mode: Some(MonitorMode::All),
            season_str: "1",
            ep_num: 1, // before start
            has_file: false,
            currently_monitored: false, // non-DB: not previously monitored
            effective_date: None,
            season_override: Some(&override_rule),
        },
    ));
}

#[test]
fn test_new_episode_season_override_at_boundary() {
    // Season override: episode exactly at range boundary → monitored
    let override_rule = SeasonOverride {
        season: "1".to_string(),
        episode_start: Some(1),
        episode_end: Some(7),
        cell_count: None,
        episode_offset: None,
        alias_season_number: None,
        search_format: None,
        aliases: vec![],
        reg_patterns: vec![],
    };
    assert!(ContentOrganizer::should_monitor_episode(
        crate::source_processor::monitoring::MonitorParams {
            mode: Some(MonitorMode::All),
            season_str: "1",
            ep_num: 1, // exactly at start
            has_file: false,
            currently_monitored: false, // non-DB: not previously monitored
            effective_date: None,
            season_override: Some(&override_rule),
        },
    ));
    assert!(ContentOrganizer::should_monitor_episode(
        crate::source_processor::monitoring::MonitorParams {
            mode: Some(MonitorMode::All),
            season_str: "1",
            ep_num: 7, // exactly at end
            has_file: false,
            currently_monitored: false, // non-DB: not previously monitored
            effective_date: None,
            season_override: Some(&override_rule),
        },
    ));
}

#[test]
fn test_assess_pack_candidacy_pack_with_unmonitored() {
    // Pack (E1-E5), only E3 monitored. E3 is NOT downloaded.
    // Needed: [3], Unneeded: 4 (1, 2, 4, 5)
    let info = ep_info(1, Some(5), true);
    let (penalty, needed, unneeded) = ContentOrganizer::assess_pack_candidacy(
        &info,
        &no_downloads(),
        &monitored(&[3]),
        Some(5),
        &SeasonPackStrategy::FavorEpisodes,
        70,
    );
    // 1/5 = 20%, below 70% threshold.
    assert!(penalty < 0);
    assert_eq!(needed, vec![3]);
    assert_eq!(unneeded, 4);
}

#[test]
fn test_merge_shared_downloads_combines_same_magnet() {
    // Two winners sharing the same magnet but targeting different
    // (series, season, episode) should merge into one entry.
    const MAGNET: &str = "magnet:?xt=urn:btih:test123";
    let base = make_candidate(
        "Test Show 1164",
        Some(MAGNET),
        1164,
        1,
        "series-a",
        vec![1164],
    );
    let second = make_candidate("Test Show 1164", Some(MAGNET), 10, 23, "series-a", vec![10]);

    let merged =
        crate::source_processor::merge_winners_by_source(vec![base, second], false).expect("merge");

    assert_eq!(merged.len(), 1, "Should merge 2 winners into 1");
    assert_eq!(
        merged[0].multi_targets.len(),
        1,
        "Primary should have 1 multi-target"
    );
    assert_eq!(
        merged[0].multi_targets[0].series_id, "series-a",
        "Multi-target series_id should match"
    );
    assert_eq!(
        merged[0].multi_targets[0].season, 23,
        "Multi-target season should be from second winner"
    );
    assert_eq!(
        merged[0].multi_targets[0].episode, 10,
        "Multi-target episode should be from second winner"
    );
}

#[test]
fn test_merge_shared_downloads_different_magnet_stays_separate() {
    // Winners with different magnets stay as separate entries.
    let first = make_candidate(
        "First Magnet",
        Some("magnet:?xt=urn:btih:aaa"),
        1,
        1,
        "series-x",
        vec![1],
    );
    let second = make_candidate(
        "Second Magnet",
        Some("magnet:?xt=urn:btih:bbb"),
        1,
        1,
        "series-y",
        vec![1],
    );

    let merged = crate::source_processor::merge_winners_by_source(vec![first, second], false)
        .expect("merge");

    assert_eq!(merged.len(), 2, "Different magnets should stay separate");
    assert!(
        merged[0].multi_targets.is_empty(),
        "No multi-targets for standalone winner"
    );
    assert!(
        merged[1].multi_targets.is_empty(),
        "No multi-targets for standalone winner"
    );
}

#[test]
fn test_merge_shared_downloads_no_magnet_unchanged() {
    // Winners with no magnet stay as-is.
    let winner = make_candidate("No Magnet", None, 1, 1, "series-z", vec![1]);

    let merged =
        crate::source_processor::merge_winners_by_source(vec![winner], false).expect("merge");

    assert_eq!(merged.len(), 1, "Single winner with no magnet stays as-is");
    assert!(
        merged[0].multi_targets.is_empty(),
        "No multi-targets for no-magnet winner"
    );
}

// Phase 1: Pattern-based identification
// Tests for the pattern matching logic that identify_by_pattern uses.
// These test parse_title_with_custom_regex with series-level patterns,
// which is the core of Phase 1's series identification.

/// Helper: build a minimal MappingRule with series-level reg_patterns.
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

#[test]
fn test_phase1_extraction_pattern_identifies_series() {
    // Pattern with (?P<episode>...) should both identify the series AND
    // extract the episode number — the ideal Phase 1 outcome.
    let mapping = series_with_patterns("My Show", vec![r"(?i)MyShow.*E(?P<episode>\d+)"]);

    let result = parse_title_with_custom_regex(
        "MyShow.S01E05",
        &mapping.settings.reg_patterns,
        None,
        false,
        None,
    );

    match result {
        CustomParseResult::Extracted(info) => {
            assert_eq!(info.episodes.first().copied().unwrap_or(0), 5);
            // series_key is clean_title of the title minus matched portions;
            // the dot is preserved since clean_title only strips brackets/parens.
            assert!(!info.series_key.is_empty());
        }
        _ => panic!("Expected Extracted result, got: {:?}", result),
    }
}

#[test]
fn test_phase1_extraction_pattern_with_season() {
    // Pattern with (?P<season>...) + (?P<episode>...) extracts both.
    let mapping = series_with_patterns(
        "My Show",
        vec![r"(?i)Show.*S(?P<season>\d+)E(?P<episode>\d+)"],
    );

    let result = parse_title_with_custom_regex(
        "Show.S02E10",
        &mapping.settings.reg_patterns,
        None,
        false,
        None,
    );

    match result {
        CustomParseResult::Extracted(info) => {
            assert_eq!(info.seasons.first().copied(), Some(2));
            assert_eq!(info.episodes.first().copied().unwrap_or(0), 10);
        }
        _ => panic!("Expected Extracted result, got: {:?}", result),
    }
}

#[test]
fn test_phase1_filter_only_pattern_identifies_series() {
    // Pattern with no named groups (filter-only) should identify the series
    // but NOT extract episode data — the caller must fall back to
    // parse_filename for values.
    let mapping = series_with_patterns("My Show", vec!["(?i)MyShowRelease"]);

    let result = parse_title_with_custom_regex(
        "MyShowRelease - S01E05",
        &mapping.settings.reg_patterns,
        None,
        false,
        None,
    );

    match result {
        CustomParseResult::MatchedFilter => {} // expected
        _ => panic!("Expected MatchedFilter result, got: {:?}", result),
    }
}

#[test]
fn test_phase1_pattern_does_not_match() {
    // When no pattern matches, returns NoMatch — caller falls to Phase 2.
    let mapping = series_with_patterns("My Show", vec!["(?i)MyShow"]);

    let result = parse_title_with_custom_regex(
        "Some.Other.Show.S01E01",
        &mapping.settings.reg_patterns,
        None,
        false,
        None,
    );

    assert!(matches!(result, CustomParseResult::NoMatch));
}

#[test]
fn test_phase1_patternless_series_matches_always() {
    // A series with no patterns can never match in Phase 1.
    // The caller's has_search_patterns() check should filter these out.
    let mapping = series_with_patterns("My Show", vec![]);

    assert!(!mapping.settings.has_search_patterns());
}

#[test]
fn test_phase1_source_scoped_pattern() {
    // Source-scoped patterns (@slug:pattern) should only match when the
    // correct source slug is provided.
    let mapping = series_with_patterns("My Show", vec![r"@nyaa:(?i)MyShow.*E(?P<episode>\d+)"]);

    // Match with correct source slug
    let result = parse_title_with_custom_regex(
        "MyShow E05",
        &mapping.settings.reg_patterns,
        None,
        false,
        Some("nyaa"),
    );
    assert!(
        matches!(result, CustomParseResult::Extracted(_)),
        "Should match when source slug is provided"
    );

    // No match with different source slug
    let result2 = parse_title_with_custom_regex(
        "MyShow E05",
        &mapping.settings.reg_patterns,
        None,
        false,
        Some("other"),
    );
    assert!(
        matches!(result2, CustomParseResult::NoMatch),
        "Should NOT match when source slug differs"
    );

    // No match with no source slug
    let result3 = parse_title_with_custom_regex(
        "MyShow E05",
        &mapping.settings.reg_patterns,
        None,
        false,
        None::<&str>,
    );
    assert!(
        matches!(result3, CustomParseResult::NoMatch),
        "Should NOT match when no source slug provided"
    );
}

#[test]
fn test_phase1_absolute_mode_pattern() {
    // In absolute mode, the pattern should not try to extract season.
    let mapping = series_with_patterns("My Show", vec![r"(?i)MyShow.*?(?P<episode>\d+)"]);
    let mut absolute_mapping = mapping;
    absolute_mapping.settings.absolute_numbering = Some(true);

    let result = parse_title_with_custom_regex(
        "MyShow.126",
        &absolute_mapping.settings.reg_patterns,
        None,
        true, // absolute numbering
        None,
    );

    match result {
        CustomParseResult::Extracted(info) => {
            assert_eq!(info.episodes.first().copied().unwrap_or(0), 126);
            assert!(
                info.seasons.is_empty(),
                "Absolute mode should not extract a season"
            );
        }
        _ => panic!("Expected Extracted result, got: {:?}", result),
    }
}

#[test]
fn test_phase1_extraction_no_season_normal_mode_bypasses_guard() {
    // Phase 1 extraction with no season in NORMAL mode should bypass the
    // mode guard's "no season" check, because the series-level pattern
    // already confirmed the entry belongs to this series.
    //
    // Regression guard: the mode guard checks SEASON-LEVEL patterns only,
    // but Phase 1 matched on SERIES-LEVEL patterns. Without the bypass,
    // this entry would be blocked even though the pattern matched.
    let result = parse_title_with_custom_regex(
        "MyShow.42",
        &series_with_patterns("My Show", vec![r"(?i)MyShow.*?(?P<episode>\d+)"])
            .settings
            .reg_patterns,
        None,
        false, // normal mode (no absolute numbering)
        None,
    );

    // Phase 1 would match and return Extracted. The mode guard must
    // NOT block this entry despite season=None in normal mode.
    match &result {
        CustomParseResult::Extracted(info) => {
            assert_eq!(info.episodes.first().copied().unwrap_or(0), 42);
            // Season defaults to Some(1) in normal mode when not captured
            // by the pattern — this is standard regex extraction behaviour.
            // The critical test is that the mode guard doesn't block this
            // entry despite having no explicit season in the title.
        }
        other => panic!("Expected Extracted, got: {:?}", other),
    }
}

// db_episode_effective_date tests
// Tests the SSoT helper that computes the effective date from DB episode row
// date fields (meta_date, upload_date, est_date) using the
// user's release-date display preferences.

/// Build a minimal `EpisodeDetailRow` with just the date fields under test.
fn ep_row(
    meta_date: Option<&str>,
    upload_date: Option<&str>,
    est_date: Option<&str>,
) -> crate::db::EpisodeDetailRow {
    let parse = |s: Option<&str>| {
        s.map(|d| chrono::NaiveDateTime::parse_from_str(d, "%Y-%m-%d %H:%M:%S").unwrap())
    };
    crate::db::EpisodeDetailRow {
        episode_id: "test-ep".to_string(),
        season: Some(1),
        episode: 1,
        status: None,
        file_path: None,
        release_title: None,
        size: 0,
        title: None,
        quality_profile_id: None,
        submitter: None,
        media_info: None,
        quick_hash: None,
        original_path: None,
        created_at: None,
        file_acquired_at: None,
        monitored: false,
        meta_date: parse(meta_date),
        upload_date: parse(upload_date),
        est_date: parse(est_date),
        metadata_ids: None,
        description: None,
        runtime: None,
        image_url: None,
        download_id: None,
        score: None,
        numbering_mode: 0,
        metadata_source: None,
        series_id: "test".to_string(),
        download_link: None,
        monitor_override: false,
    }
}

/// Default order: metadata, source, estimated.
fn default_rd_config() -> ReleaseDateDisplayConfig {
    ReleaseDateDisplayConfig {
        order: vec![
            "metadata".to_string(),
            "source".to_string(),
            "estimated".to_string(),
        ],
        metadata_enabled: true,
        source_enabled: true,
        estimated_enabled: true,
    }
}

#[test]
fn test_effective_date_estimated_fills_gap_when_pub_date_missing() {
    // Episode has no meta_date but has an estimated date in the future.
    // With default prefs (metadata > source > estimated), metadata has no
    // data so it falls through to source (also none), then to estimated.
    let row = ep_row(None, None, Some("2026-06-20 00:00:00"));
    let result = super::monitoring::db_episode_effective_date(&row, &default_rd_config());
    assert!(
        result.is_some(),
        "should fall through to estimated when metadata and source are None"
    );
    let expected =
        chrono::NaiveDateTime::parse_from_str("2026-06-20 00:00:00", "%Y-%m-%d %H:%M:%S").unwrap();
    assert_eq!(result.unwrap(), expected);
}

#[test]
fn test_effective_date_metadata_takes_priority_over_estimated() {
    // Both metadata and estimated are set. Default order puts metadata first.
    let row = ep_row(
        Some("2025-01-15 00:00:00"),
        None,
        Some("2026-06-20 00:00:00"),
    );
    let result = super::monitoring::db_episode_effective_date(&row, &default_rd_config());
    let expected =
        chrono::NaiveDateTime::parse_from_str("2025-01-15 00:00:00", "%Y-%m-%d %H:%M:%S").unwrap();
    assert_eq!(result.unwrap(), expected);
}

#[test]
fn test_effective_date_estimated_only_enabled() {
    // Only estimated is enabled, only estimated has data → returns it.
    let row = ep_row(None, None, Some("2026-06-20 00:00:00"));
    let config = ReleaseDateDisplayConfig {
        order: vec![
            "metadata".to_string(),
            "source".to_string(),
            "estimated".to_string(),
        ],
        metadata_enabled: false,
        source_enabled: false,
        estimated_enabled: true,
    };
    let result = super::monitoring::db_episode_effective_date(&row, &config);
    let expected =
        chrono::NaiveDateTime::parse_from_str("2026-06-20 00:00:00", "%Y-%m-%d %H:%M:%S").unwrap();
    assert_eq!(result.unwrap(), expected);
}

#[test]
fn test_effective_date_estimated_disabled_returns_none() {
    // Only estimated has data, but estimated is disabled → None.
    let row = ep_row(None, None, Some("2026-06-20 00:00:00"));
    let config = ReleaseDateDisplayConfig {
        order: vec![
            "metadata".to_string(),
            "source".to_string(),
            "estimated".to_string(),
        ],
        metadata_enabled: false,
        source_enabled: false,
        estimated_enabled: false,
    };
    let result = super::monitoring::db_episode_effective_date(&row, &config);
    assert_eq!(result, None);
}

#[test]
fn test_effective_date_no_dates_returns_none() {
    // No date sources have data → None.
    let row = ep_row(None, None, None);
    let result = super::monitoring::db_episode_effective_date(&row, &default_rd_config());
    assert_eq!(result, None);
}

#[test]
fn test_effective_date_source_used_when_metadata_missing() {
    // Metadata is None but source is set → falls through to source.
    let row = ep_row(None, Some("2025-03-01 12:00:00"), None);
    let result = super::monitoring::db_episode_effective_date(&row, &default_rd_config());
    let expected =
        chrono::NaiveDateTime::parse_from_str("2025-03-01 12:00:00", "%Y-%m-%d %H:%M:%S").unwrap();
    assert_eq!(result.unwrap(), expected);
}

// should_monitor_episode effective-date tests (Future mode)
// These verify that `Future` mode uses the effective date (which may come from
// est_date) rather than only the raw metadata meta_date.

#[test]
fn test_future_estimated_only_no_pub_date() {
    // Future mode: episode has no meta_date but estimated is in the future.
    // The effective date should be used so it's treated as "future."
    let future = chrono::Utc::now() + chrono::Duration::days(30);
    assert!(ContentOrganizer::should_monitor_episode(
        crate::source_processor::monitoring::MonitorParams {
            mode: Some(MonitorMode::Future),
            season_str: "1",
            ep_num: 5,
            has_file: false,
            currently_monitored: false, // non-DB: not previously monitored
            effective_date: Some(future.naive_utc()), // effective_date = estimated (in future)
            season_override: None,
        },
    ));
}

#[test]
fn test_future_past_estimated_date_not_future() {
    // Future mode: episode has estimated date in the past → NOT monitored.
    let past = chrono::Utc::now() - chrono::Duration::days(30);
    assert!(!ContentOrganizer::should_monitor_episode(
        crate::source_processor::monitoring::MonitorParams {
            mode: Some(MonitorMode::Future),
            season_str: "1",
            ep_num: 5,
            has_file: false,
            currently_monitored: false, // non-DB: not previously monitored
            effective_date: Some(past.naive_utc()), // effective_date = estimated (in past)
            season_override: None,
        },
    ));
}

// classify_db_episodes tests
// Uses the SSoT helper directly with minimal EpisodeDetailRow vectors.

/// Build a minimal mapping with default settings for testing.
fn test_mapping() -> MappingRule {
    MappingRule {
        series_id: "test-series".to_string(),
        target_title: "Test Series".to_string(),
        name: "test-series".to_string(),
        ..Default::default()
    }
}

#[test]
fn test_classify_future_episodes_estimated_date_used() {
    // Three episodes: E01 downloaded (past date), E02 missing (no dates),
    // E03 missing (estimated in future). Future mode with default prefs.
    // E03 should be classified as to_monitor because its effective date
    // (estimated in future) makes it "future."
    let now = chrono::Utc::now().naive_utc();
    let past = now - chrono::Duration::days(30);

    let episodes = vec![
        ep_row(Some("2024-01-15 00:00:00"), None, None), // E01: has meta_date
        ep_row(None, None, None),                        // E02: no dates
        ep_row(
            None,
            None,
            Some(
                &(now + chrono::Duration::days(1))
                    .format("%Y-%m-%d %H:%M:%S")
                    .to_string(),
            ),
        ), // E03: estimated in future
    ];
    // Manually set different episode numbers and statuses
    let episodes: Vec<crate::db::EpisodeDetailRow> = episodes
        .into_iter()
        .enumerate()
        .map(|(i, mut row)| {
            row.episode = (i + 1) as i32;
            row.episode_id = format!("test-ep-{}", i + 1);
            if i == 0 {
                row.status = Some("downloaded".to_string());
                row.meta_date = Some(past);
            }
            row
        })
        .collect();

    let mapping = test_mapping();
    let rd_config = default_rd_config();
    let (to_monitor, to_unmonitor, _to_clear) = classify_db_episodes(
        episodes,
        &mapping,
        MonitorMode::Future,
        &rd_config,
        false,
        &HashSet::new(),
        false,
    );

    // E01 is downloaded and past → not monitored (has_file + upgrades irrelevant
    // for Future mode — past date means not future)
    // E02 has no effective date → not monitored
    // E03 has estimated in future → monitored
    assert!(to_monitor.contains(&"test-ep-3".to_string()));
    assert!(!to_unmonitor.contains(&"test-ep-3".to_string()));
}

#[test]
fn test_classify_future_episodes_estimated_disabled() {
    // Same scenario but estimated is disabled in prefs. E03 has no meta_date
    // and no upload_date → effective date is None → not monitored.
    let now = chrono::Utc::now().naive_utc();
    let past = now - chrono::Duration::days(30);

    let episodes = vec![
        ep_row(Some("2024-01-15 00:00:00"), None, None), // E01: has meta_date
        ep_row(None, None, None),                        // E02: no dates
        ep_row(None, None, Some("2026-06-20 00:00:00")), // E03: estimated disabled
    ];
    let episodes: Vec<crate::db::EpisodeDetailRow> = episodes
        .into_iter()
        .enumerate()
        .map(|(i, mut row)| {
            row.episode = (i + 1) as i32;
            row.episode_id = format!("test-ep-{}", i + 1);
            if i == 0 {
                row.status = Some("downloaded".to_string());
                row.meta_date = Some(past);
            }
            row
        })
        .collect();

    let mapping = test_mapping();
    let rd_config = ReleaseDateDisplayConfig {
        order: vec![
            "metadata".to_string(),
            "source".to_string(),
            "estimated".to_string(),
        ],
        metadata_enabled: true,
        source_enabled: true,
        estimated_enabled: false, // disabled!
    };
    let (to_monitor, _to_unmonitor, _to_clear) = classify_db_episodes(
        episodes,
        &mapping,
        MonitorMode::Future,
        &rd_config,
        false,
        &HashSet::new(),
        false,
    );

    // E03 has no effective date (estimated disabled, no meta_date/source) → not monitored
    assert!(!to_monitor.contains(&"test-ep-3".to_string()));
}

#[test]
fn test_classify_future_episodes_override_prefs_order() {
    // User puts estimated first in priority. Episode has both metadata and
    // estimated dates. Since estimated is first, effective date = estimated.
    let future_meta = chrono::Utc::now() + chrono::Duration::days(10); // metadata 10d
    let future_est = chrono::Utc::now() + chrono::Duration::days(30); // estimated 30d

    let episodes = vec![crate::db::EpisodeDetailRow {
        episode_id: "test-ep-1".to_string(),
        episode: 1,
        season: Some(1),
        status: None,
        meta_date: Some(future_meta.naive_utc()),
        upload_date: None,
        est_date: Some(future_est.naive_utc()),
        ..ep_row(None, None, None)
    }];

    let mapping = test_mapping();
    // Estimated first in order
    let rd_config = ReleaseDateDisplayConfig {
        order: vec![
            "estimated".to_string(),
            "metadata".to_string(),
            "source".to_string(),
        ],
        metadata_enabled: true,
        source_enabled: true,
        estimated_enabled: true,
    };
    let (to_monitor, _to_unmonitor, _to_clear) = classify_db_episodes(
        episodes,
        &mapping,
        MonitorMode::Future,
        &rd_config,
        false,
        &HashSet::new(),
        false,
    );

    // E01 should be monitored because its effective date (estimated, 30d out) is future
    assert!(
        to_monitor.contains(&"test-ep-1".to_string()),
        "episode with estimated date should be monitored when estimated is first priority"
    );
}

// episode_is_effectively_monitored (non-DB episode)
// Tests the wrapper used by `select_winners` for episodes without a DB cell.
// The scenario: DB has S02E01-S02E03, RSS finds S02E04 (no DB cell yet).
// The system must decide: "would this episode be monitored IF it existed?"

/// Build a mapping targeting season 2 for testing non-DB episode logic.
fn s02_mapping() -> MappingRule {
    MappingRule {
        series_id: "test-series".to_string(),
        target_title: "Test Series S02".to_string(),
        name: "test-series-s02".to_string(),
        ..Default::default()
    }
}

#[test]
fn test_non_db_s02e04_pilot_is_unmonitored() {
    // Pilot mode → only S01E01. S02E04 is NOT the pilot.
    let mut mapping = s02_mapping();
    mapping.settings.monitor_mode = Some(MonitorMode::Pilot);
    assert!(!ContentOrganizer::episode_is_effectively_monitored(
        &mapping, "2", 4,
    ));
}

#[test]
fn test_non_db_s02e04_existing_is_unmonitored() {
    // Existing mode → only episodes that have a file. Non-DB has no file.
    let mut mapping = s02_mapping();
    mapping.settings.monitor_mode = Some(MonitorMode::Existing);
    assert!(!ContentOrganizer::episode_is_effectively_monitored(
        &mapping, "2", 4,
    ));
}

#[test]
fn test_non_db_s02e04_first_season_is_unmonitored() {
    // FirstSeason mode → only S01. S02 is not first season.
    let mut mapping = s02_mapping();
    mapping.settings.monitor_mode = Some(MonitorMode::FirstSeason);
    assert!(!ContentOrganizer::episode_is_effectively_monitored(
        &mapping, "2", 4,
    ));
}

#[test]
fn test_non_db_s02e04_specials_is_unmonitored() {
    // Specials mode → only season 0. S02 is not specials.
    let mut mapping = s02_mapping();
    mapping.settings.monitor_mode = Some(MonitorMode::Specials);
    assert!(!ContentOrganizer::episode_is_effectively_monitored(
        &mapping, "2", 4,
    ));
}

#[test]
fn test_non_db_s02e04_missing_is_unmonitored() {
    // Missing mode → normally monitored, but non-DB episodes are never
    // auto-downloaded.  The episode cell must exist first.
    let mut mapping = s02_mapping();
    mapping.settings.monitor_mode = Some(MonitorMode::Missing);
    assert!(!ContentOrganizer::episode_is_effectively_monitored(
        &mapping, "2", 4,
    ));
}

#[test]
fn test_non_db_s02e04_all_is_unmonitored() {
    // All mode → normally monitored, but non-DB episodes are never
    // auto-downloaded.  The episode cell must exist first.
    let mut mapping = s02_mapping();
    mapping.settings.monitor_mode = Some(MonitorMode::All);
    assert!(!ContentOrganizer::episode_is_effectively_monitored(
        &mapping, "2", 4,
    ));
}

// classify_db_episodes: conflicting file state vs mode
// DB episodes where the has_file status conflicts with what the mode expects.

#[test]
fn test_classify_existing_mode_without_file_goes_to_unmonitor() {
    // Existing mode: only monitor episodes that already have files.
    // An episode with status=None should be unmonitored.
    let episodes = vec![crate::db::EpisodeDetailRow {
        episode_id: "test-ep-1".to_string(),
        episode: 1,
        season: Some(2),
        status: None, // no file
        ..ep_row(None, None, None)
    }];
    let mapping = s02_mapping();
    let rd_config = default_rd_config();
    let (to_monitor, to_unmonitor, _to_clear) = classify_db_episodes(
        episodes,
        &mapping,
        MonitorMode::Existing,
        &rd_config,
        false,
        &HashSet::new(),
        false,
    );
    assert!(
        to_unmonitor.contains(&"test-ep-1".to_string()),
        "Existing mode + no file → should be unmonitored"
    );
    assert!(!to_monitor.contains(&"test-ep-1".to_string()));
}

#[test]
fn test_classify_missing_mode_with_file_goes_to_unmonitor() {
    // Missing mode: only monitor episodes without files.
    // An episode with status="downloaded" should be unmonitored.
    let episodes = vec![crate::db::EpisodeDetailRow {
        episode_id: "test-ep-1".to_string(),
        episode: 1,
        season: Some(2),
        status: Some("downloaded".to_string()),
        ..ep_row(None, None, None)
    }];
    let mapping = s02_mapping();
    let rd_config = default_rd_config();
    let (to_monitor, to_unmonitor, _to_clear) = classify_db_episodes(
        episodes,
        &mapping,
        MonitorMode::Missing,
        &rd_config,
        false,
        &HashSet::new(),
        false,
    );
    assert!(
        to_unmonitor.contains(&"test-ep-1".to_string()),
        "Missing mode + has file → should be unmonitored"
    );
    assert!(!to_monitor.contains(&"test-ep-1".to_string()));
}

#[test]
fn test_classify_pilot_s02e01_goes_to_unmonitor() {
    // Pilot mode: only S01E01. S02E01 should be unmonitored.
    let episodes = vec![crate::db::EpisodeDetailRow {
        episode_id: "test-ep-1".to_string(),
        episode: 1,
        season: Some(2),
        status: None,
        ..ep_row(None, None, None)
    }];
    let mapping = s02_mapping();
    let rd_config = default_rd_config();
    let (to_monitor, to_unmonitor, _to_clear) = classify_db_episodes(
        episodes,
        &mapping,
        MonitorMode::Pilot,
        &rd_config,
        false,
        &HashSet::new(),
        false,
    );
    assert!(
        to_unmonitor.contains(&"test-ep-1".to_string()),
        "Pilot mode + S02E01 → should be unmonitored"
    );
    assert!(!to_monitor.contains(&"test-ep-1".to_string()));
}

#[test]
fn test_classify_first_season_s02e01_goes_to_unmonitor() {
    // FirstSeason mode: only S01. S02E01 should be unmonitored.
    let episodes = vec![crate::db::EpisodeDetailRow {
        episode_id: "test-ep-1".to_string(),
        episode: 1,
        season: Some(2),
        status: None,
        ..ep_row(None, None, None)
    }];
    let mapping = s02_mapping();
    let rd_config = default_rd_config();
    let (to_monitor, to_unmonitor, _to_clear) = classify_db_episodes(
        episodes,
        &mapping,
        MonitorMode::FirstSeason,
        &rd_config,
        false,
        &HashSet::new(),
        false,
    );
    assert!(
        to_unmonitor.contains(&"test-ep-1".to_string()),
        "FirstSeason mode + S02E01 → should be unmonitored"
    );
    assert!(!to_monitor.contains(&"test-ep-1".to_string()));
}

// classify_db_episodes: Future mode fresh_apply vs sweep
// Verifies that fresh_apply=true (user mode switch) correctly ignores existing
// monitored state, while fresh_apply=false (sweep) preserves it.

#[test]
fn test_classify_future_same_day_fresh_apply() {
    // Future mode, same-day date, fresh_apply=true → should be monitored
    // (same-day date guard fires regardless of currently_monitored)
    let today_midnight = chrono::Utc::now()
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_utc()
        .naive_utc();
    let episodes = vec![crate::db::EpisodeDetailRow {
        episode_id: "test-ep-1".to_string(),
        episode: 1,
        season: Some(1),
        status: None,    // no file
        monitored: true, // was monitored from previous mode
        meta_date: Some(today_midnight),
        ..ep_row(None, None, None)
    }];
    let mapping = s02_mapping();
    let rd_config = default_rd_config();
    let (to_monitor, to_unmonitor, _to_clear) = classify_db_episodes(
        episodes,
        &mapping,
        MonitorMode::Future,
        &rd_config,
        true, // fresh_apply: user mode switch
        &HashSet::new(),
        false,
    );
    // Same-day date + no file = monitored, regardless of fresh_apply
    assert!(to_monitor.contains(&"test-ep-1".to_string()));
    assert!(!to_unmonitor.contains(&"test-ep-1".to_string()));
}

#[test]
fn test_classify_future_past_date_fresh_apply_not_monitored() {
    // Future mode, past date, fresh_apply=true → NOT monitored
    // (fresh apply means currently_monitored=false → past-date arm returns false)
    let past = chrono::Utc::now() - chrono::Duration::days(30);
    let episodes = vec![crate::db::EpisodeDetailRow {
        episode_id: "test-ep-1".to_string(),
        episode: 1,
        season: Some(1),
        status: None,    // no file
        monitored: true, // was monitored from previous mode!
        meta_date: Some(past.naive_utc()),
        ..ep_row(None, None, None)
    }];
    let mapping = s02_mapping();
    let rd_config = default_rd_config();
    let (to_monitor, to_unmonitor, _to_clear) = classify_db_episodes(
        episodes,
        &mapping,
        MonitorMode::Future,
        &rd_config,
        true, // fresh_apply: user mode switch back to Future
        &HashSet::new(),
        false,
    );
    // Past date + fresh_apply = not monitored (was monitored under another mode)
    let _ = to_unmonitor;
    assert!(!to_monitor.contains(&"test-ep-1".to_string()));
}

#[test]
fn test_classify_future_past_date_sweep_preserves_monitored() {
    // Future mode, past date, fresh_apply=false → remains monitored
    // (was monitored under Future, now past but still missing)
    let past = chrono::Utc::now() - chrono::Duration::days(30);
    let episodes = vec![crate::db::EpisodeDetailRow {
        episode_id: "test-ep-1".to_string(),
        episode: 1,
        season: Some(1),
        status: None,    // no file
        monitored: true, // was monitored as Future (previously future-dated)
        meta_date: Some(past.naive_utc()),
        ..ep_row(None, None, None)
    }];
    let mapping = s02_mapping();
    let rd_config = default_rd_config();
    let (to_monitor, to_unmonitor, _to_clear) = classify_db_episodes(
        episodes,
        &mapping,
        MonitorMode::Future,
        &rd_config,
        false, // sweep: preserve existing state
        &HashSet::new(),
        false,
    );
    // Past date + was monitored + no file = keep monitored
    assert!(to_monitor.contains(&"test-ep-1".to_string()));
    let _ = to_unmonitor;
}

#[test]
fn test_classify_future_past_date_sweep_never_monitored() {
    // Future mode, past date, fresh_apply=false, was NEVER monitored → not monitored
    // (differs from above: monitored=false in DB, so currently_monitored=false)
    let past = chrono::Utc::now() - chrono::Duration::days(30);
    let episodes = vec![crate::db::EpisodeDetailRow {
        episode_id: "test-ep-1".to_string(),
        episode: 1,
        season: Some(1),
        status: None,     // no file
        monitored: false, // never monitored
        meta_date: Some(past.naive_utc()),
        ..ep_row(None, None, None)
    }];
    let mapping = s02_mapping();
    let rd_config = default_rd_config();
    let (to_monitor, to_unmonitor, _to_clear) = classify_db_episodes(
        episodes,
        &mapping,
        MonitorMode::Future,
        &rd_config,
        false, // sweep: preserve existing state
        &HashSet::new(),
        false,
    );
    // Past date + never monitored + no file = NOT monitored (never was future)
    assert!(!to_monitor.contains(&"test-ep-1".to_string()));
    let _ = to_unmonitor;
}

#[test]
fn test_classify_future_past_date_sweep_downloaded_not_monitored() {
    // Future mode, past date, fresh_apply=false, was monitored but now has file → not monitored
    let past = chrono::Utc::now() - chrono::Duration::days(30);
    let episodes = vec![crate::db::EpisodeDetailRow {
        episode_id: "test-ep-1".to_string(),
        episode: 1,
        season: Some(1),
        status: Some("downloaded".to_string()), // has file
        monitored: true,                        // was monitored
        meta_date: Some(past.naive_utc()),
        ..ep_row(None, None, None)
    }];
    let mapping = s02_mapping();
    let rd_config = default_rd_config();
    let (to_monitor, to_unmonitor, _to_clear) = classify_db_episodes(
        episodes,
        &mapping,
        MonitorMode::Future,
        &rd_config,
        false, // sweep
        &HashSet::new(),
        false,
    );
    // Has file → not monitored
    assert!(to_unmonitor.contains(&"test-ep-1".to_string()));
    let _ = to_monitor;
}

#[test]
fn test_classify_future_same_day_sweep_was_never_monitored() {
    // Future mode, same-day date, fresh_apply=false, was NEVER monitored → monitored
    // (same-day date guard fires regardless of currently_monitored)
    let today_midnight = chrono::Utc::now()
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_utc()
        .naive_utc();
    let episodes = vec![crate::db::EpisodeDetailRow {
        episode_id: "test-ep-1".to_string(),
        episode: 1,
        season: Some(1),
        status: None,
        monitored: false, // never monitored
        meta_date: Some(today_midnight),
        ..ep_row(None, None, None)
    }];
    let mut mapping = s02_mapping();
    mapping.settings.monitor_mode = Some(MonitorMode::Future);
    let rd_config = default_rd_config();
    let (to_monitor, _to_unmonitor, _to_clear) = classify_db_episodes(
        episodes,
        &mapping,
        MonitorMode::Future,
        &rd_config,
        false, // sweep
        &HashSet::new(),
        false,
    );
    // Same-day + no file = monitored, regardless of history
    assert!(to_monitor.contains(&"test-ep-1".to_string()));
}

#[test]
fn test_phase1_filter_only_matched_for_series_identification() {
    // Filter-only Phase 1 match: pattern matched but no extraction.
    let result = parse_title_with_custom_regex(
        "MyShowRelease",
        &series_with_patterns("My Show", vec!["(?i)MyShowRelease"])
            .settings
            .reg_patterns,
        None,
        false,
        None,
    );

    // Phase 1 gets MatchedFilter, then parse_filename would be called
    // for values. If parse_filename also can't find a season, the mode
    // guard should block — this test verifies the MATCHING behaviour.
    assert!(
        matches!(result, CustomParseResult::MatchedFilter),
        "Filter-only pattern should return MatchedFilter"
    );
}

// Scanner regression tests
// These verify that fresh_apply=false (which is what the scanner now uses)
// correctly handles scanner-discovered episodes (which always have a file).
// The scanner bug was using fresh_apply=true, which reset previously-future
// monitored episodes to unmonitored on every scan cycle.

#[test]
fn test_scanner_fresh_apply_false_new_organized_episode_not_monitored() {
    // Scanner discovers a new file and inserts it with status="organized".
    // fresh_apply=false should NOT monitor it under Future mode (it already has a file).
    let past = chrono::Utc::now() - chrono::Duration::days(30);
    let episodes = vec![crate::db::EpisodeDetailRow {
        episode_id: "test-ep-scanner-1".to_string(),
        episode: 1,
        season: Some(1),
        status: Some("organized".to_string()), // scanner sets status="organized"
        monitored: true,                       // insert_episode defaults to true
        meta_date: Some(past.naive_utc()),
        ..ep_row(None, None, None)
    }];
    let mapping = s02_mapping();
    let rd_config = default_rd_config();
    let (to_monitor, to_unmonitor, _to_clear) = classify_db_episodes(
        episodes,
        &mapping,
        MonitorMode::Future,
        &rd_config,
        false, // scanner uses fresh_apply=false after the fix
        &HashSet::new(),
        false,
    );
    // Has file (organized) + past date → should be unmonitored
    assert!(
        to_unmonitor.contains(&"test-ep-scanner-1".to_string()),
        "Scanner-discovered episode with file should be unmonitored under Future mode"
    );
    let _ = to_monitor;
}

#[test]
fn test_scanner_fresh_apply_false_existing_future_ep_preserved() {
    // An episode that was previously future-dated (monitored=true) still has
    // no file. fresh_apply=false must preserve its monitored state even though
    // the date has passed. This was the exact scanner bug scenario.
    let past = chrono::Utc::now() - chrono::Duration::days(7);
    let episodes = vec![crate::db::EpisodeDetailRow {
        episode_id: "test-ep-future-scanner".to_string(),
        episode: 1,
        season: Some(1),
        status: None,    // no file
        monitored: true, // was future-dated, still missing
        meta_date: Some(past.naive_utc()),
        ..ep_row(None, None, None)
    }];
    let mapping = s02_mapping();
    let rd_config = default_rd_config();
    let (to_monitor, to_unmonitor, _to_clear) = classify_db_episodes(
        episodes,
        &mapping,
        MonitorMode::Future,
        &rd_config,
        false, // scanner uses fresh_apply=false after the fix
        &HashSet::new(),
        false,
    );
    // Past date + was monitored + no file = keep monitored
    assert!(
        to_monitor.contains(&"test-ep-future-scanner".to_string()),
        "Previously future-dated episode must stay monitored under fresh_apply=false"
    );
    let _ = to_unmonitor;
}

// pack_penalty tests (shared SSoT for penalty formula)
// `pack_penalty` is the single source of truth for the pack penalty formula.
// Both `calculate_pack_score` and `assess_pack_candidacy` delegate to it.
// These tests verify the function directly.

#[test]
fn test_pack_penalty_no_pack_no_penalty() {
    // Non-pack: no threshold check, penalty based on unneeded count
    let (penalty, replaced) = crate::source_processor::scoring::pack_penalty(
        5.0,
        5,
        0,
        false,
        &jumbie_shared::config::SeasonPackStrategy::FavorEpisodes,
        70,
    );
    assert_eq!(
        penalty, 0,
        "Non-pack with all needed should have no penalty"
    );
    assert!(!replaced, "Non-pack never triggers replacement");
}

#[test]
fn test_pack_penalty_pack_all_needed_triggers_threshold() {
    // Full pack, all needed (100%) → meets 70% threshold → replacement
    let (penalty, replaced) = crate::source_processor::scoring::pack_penalty(
        12.0,
        12,
        0,
        true,
        &jumbie_shared::config::SeasonPackStrategy::FavorEpisodes,
        70,
    );
    assert_eq!(penalty, 0, "Threshold met → penalty zeroed");
    assert!(replaced, "100% needed meets threshold");
}

#[test]
fn test_pack_penalty_above_threshold_triggers_replacement() {
    // 10/12 needed (83%) → above 70% threshold → replacement triggered
    let (penalty, replaced) = crate::source_processor::scoring::pack_penalty(
        12.0,
        10,
        2,
        true,
        &jumbie_shared::config::SeasonPackStrategy::FavorEpisodes,
        70,
    );
    assert_eq!(penalty, 0, "Replacement triggered → penalty zeroed");
    assert!(replaced);
}

#[test]
fn test_pack_penalty_below_threshold_penalty_applies() {
    // 3/12 needed (25%) → below 70% → penalty for 9 unneeded
    let (penalty, replaced) = crate::source_processor::scoring::pack_penalty(
        12.0,
        3,
        9,
        true,
        &jumbie_shared::config::SeasonPackStrategy::FavorEpisodes,
        70,
    );
    assert_eq!(penalty, -450, "9 unneeded * 50 strict penalty");
    assert!(!replaced, "Below threshold, no replacement");
}

#[test]
fn test_pack_penalty_favor_packs_smaller_penalty() {
    // Same scenario with FavorSeasonPacks → smaller per-ep penalty
    let (penalty, replaced) = crate::source_processor::scoring::pack_penalty(
        12.0,
        3,
        9,
        true,
        &jumbie_shared::config::SeasonPackStrategy::FavorSeasonPacks,
        70,
    );
    assert_eq!(penalty, -90, "9 unneeded * 10 favor-packs penalty");
    assert!(!replaced, "Below threshold, no replacement");
}

#[test]
fn test_pack_penalty_zero_threshold_triggers_replacement() {
    // threshold=0 → any needed count triggers replacement
    let (penalty, replaced) = crate::source_processor::scoring::pack_penalty(
        5.0,
        1,
        4,
        true,
        &jumbie_shared::config::SeasonPackStrategy::FavorEpisodes,
        0,
    );
    assert_eq!(penalty, 0, "Zero threshold → replacement, no penalty");
    assert!(replaced);
}

#[test]
fn test_pack_penalty_no_episodes_zero_total() {
    // Edge case: zero total episodes → no threshold check, full penalty
    let (penalty, replaced) = crate::source_processor::scoring::pack_penalty(
        0.0,
        0,
        1,
        true,
        &jumbie_shared::config::SeasonPackStrategy::FavorEpisodes,
        70,
    );
    assert_eq!(penalty, -50, "1 unneeded * 50");
    assert!(!replaced, "total=0 → threshold check skipped");
}

#[test]
fn test_pack_penalty_non_pack_ignores_threshold() {
    // Non-pack: threshold check is skipped even with high needed %
    let (penalty, replaced) = crate::source_processor::scoring::pack_penalty(
        5.0,
        5,
        0,
        false,
        &jumbie_shared::config::SeasonPackStrategy::FavorEpisodes,
        70,
    );
    assert_eq!(penalty, 0);
    assert!(!replaced, "Non-pack never triggers replacement");
}

// pack_meets_replacement_threshold: the SSoT predicate for "replace existing
// downloads", shared by pack_penalty and the auto-season search.

#[test]
fn test_pack_meets_replacement_threshold() {
    use crate::source_processor::scoring::pack_meets_replacement_threshold;

    // 13/21 = 61.9% >= 50 → replace.
    assert!(pack_meets_replacement_threshold(21, 13, 50));
    // 10/21 = 47.6% < 50 → do not replace.
    assert!(!pack_meets_replacement_threshold(21, 10, 50));
    // Exact boundary counts as meeting the threshold.
    assert!(pack_meets_replacement_threshold(4, 2, 50));
    // threshold 0 → any needed episode replaces.
    assert!(pack_meets_replacement_threshold(21, 1, 0));
    // No episodes → never replaces, even at threshold 0.
    assert!(!pack_meets_replacement_threshold(0, 0, 0));
}

// classify_db_episodes: monitor_override behavior
// Verifies that overridden episodes are skipped during sweeps (fresh_apply=false)
// unless the mode's evaluation has caught up (self-heal).

#[test]
fn test_classify_override_skips_divergent_episode() {
    // Missing mode, episode has no file → mode says monitored.
    // User toggled it OFF → override=1, monitored=false.
    // Sweep should skip it (doesn't appear in either list).
    let episodes = vec![crate::db::EpisodeDetailRow {
        episode_id: "test-override-1".to_string(),
        episode: 1,
        season: Some(1),
        status: None,     // no file → Missing mode says monitored
        monitored: false, // user toggled OFF
        ..ep_row(None, None, None)
    }];
    let mut overridden = HashSet::new();
    overridden.insert("test-override-1".to_string());
    let mapping = s02_mapping();
    let rd_config = default_rd_config();
    let (to_monitor, to_unmonitor, to_clear_override) = classify_db_episodes(
        episodes,
        &mapping,
        MonitorMode::Missing,
        &rd_config,
        false, // sweep: preserve state
        &overridden,
        false,
    );
    assert!(
        !to_monitor.contains(&"test-override-1".to_string()),
        "Overridden episode should NOT appear in to_monitor"
    );
    assert!(
        !to_unmonitor.contains(&"test-override-1".to_string()),
        "Overridden episode should NOT appear in to_unmonitor"
    );
    assert!(
        to_clear_override.is_empty(),
        "Override diverges from mode → should NOT be self-healed"
    );
}

#[test]
fn test_classify_override_self_heals_when_mode_agrees() {
    // Missing mode, episode has a file → mode says unmonitored.
    // User toggled it ON → override=1, monitored=true.
    // File arrives later (upgrade) → mode still says unmonitored (still has file).
    // Self-heal: mode_wants (unmonitored) != current (true) → still diverges, NOT healed.
    //
    // Instead test: Missing mode, episode has NO file → mode says monitored.
    // User toggled ON → override=1, monitored=true (same as mode).
    // Self-heal: mode_wants (monitored) == current (true) → override is stale, clear it.
    let episodes = vec![crate::db::EpisodeDetailRow {
        episode_id: "test-self-heal-1".to_string(),
        episode: 1,
        season: Some(1),
        status: None,    // no file → Missing mode says monitored
        monitored: true, // user toggled ON (same as mode!) → stale override
        ..ep_row(None, None, None)
    }];
    let mut overridden = HashSet::new();
    overridden.insert("test-self-heal-1".to_string());
    let mapping = s02_mapping();
    let rd_config = default_rd_config();
    let (_to_monitor, _to_unmonitor, to_clear_override) = classify_db_episodes(
        episodes,
        &mapping,
        MonitorMode::Missing,
        &rd_config,
        false, // sweep: preserve state
        &overridden,
        false,
    );
    assert!(
        to_clear_override.contains(&"test-self-heal-1".to_string()),
        "Override agrees with mode → should be self-healed (cleared)"
    );
}

#[test]
fn test_classify_override_does_not_self_heal_when_still_diverging() {
    // Missing mode, episode has no file → mode says monitored.
    // User toggled OFF → override=1, monitored=false.
    // Mode_wants (monitored) != current (false) → still diverging.
    // Self-heal does NOT fire.
    let episodes = vec![crate::db::EpisodeDetailRow {
        episode_id: "test-no-heal-1".to_string(),
        episode: 1,
        season: Some(1),
        status: None,     // no file → Missing mode says monitored
        monitored: false, // user toggled OFF → diverges
        ..ep_row(None, None, None)
    }];
    let mut overridden = HashSet::new();
    overridden.insert("test-no-heal-1".to_string());
    let mapping = s02_mapping();
    let rd_config = default_rd_config();
    let (_to_monitor, _to_unmonitor, to_clear_override) = classify_db_episodes(
        episodes,
        &mapping,
        MonitorMode::Missing,
        &rd_config,
        false, // sweep: preserve state
        &overridden,
        false,
    );
    assert!(
        to_clear_override.is_empty(),
        "Override still diverging from mode → should NOT be self-healed"
    );
}

#[test]
fn test_classify_override_self_heals_existing_mode_caught_up() {
    // Existing mode, episode has a file → mode says monitored.
    // User toggled ON → override=1, monitored=true (same as mode at the time).
    // File deleted → mode still says monitored (episode status still shows file).
    // Wait, that's not right. Let me use a different scenario:
    // Existing mode, episode has NO file → mode says unmonitored.
    // User toggled ON → override=1, monitored=true (diverges: override!)
    // File arrives → mode now says monitored (mode_wants=true == current=true).
    // Self-heal: mode caught up, clear override.
    let episodes = vec![crate::db::EpisodeDetailRow {
        episode_id: "test-caught-up-1".to_string(),
        episode: 1,
        season: Some(1),
        status: Some("downloaded".to_string()), // has file
        monitored: true,                        // user toggled ON (same as Existing mode now says)
        ..ep_row(None, None, None)
    }];
    let mut overridden = HashSet::new();
    overridden.insert("test-caught-up-1".to_string());
    let mapping = s02_mapping();
    let rd_config = default_rd_config();
    let (_to_monitor, _to_unmonitor, to_clear_override) = classify_db_episodes(
        episodes,
        &mapping,
        MonitorMode::Existing,
        &rd_config,
        false, // sweep: preserve state
        &overridden,
        false,
    );
    assert!(
        to_clear_override.contains(&"test-caught-up-1".to_string()),
        "Existing mode now agrees with user toggle → override should be cleared"
    );
}

#[test]
fn test_classify_override_skipped_for_future_mode_past_date_diverging() {
    // Future mode, past date + no file, default mode says NOT monitored
    // (since currently_monitored=false for non-DB). User toggled ON.
    // Sweep with fresh_apply=false: currently_monitored=true (DB value).
    // Past date + currently_monitored=true + no_file → mode says monitored.
    // So mode_wants (monitored) == current (true) → self-heal!
    //
    // Better test: Future mode, past date + has file → mode says unmonitored.
    // User toggled ON → override=1, monitored=true.
    // Sweep: past date + currently_monitored=true + has_file → mode says unmonitored.
    // mode_wants (unmonitored) != current (true) → skip.
    let past = chrono::Utc::now() - chrono::Duration::days(30);
    let episodes = vec![crate::db::EpisodeDetailRow {
        episode_id: "test-future-override-1".to_string(),
        episode: 1,
        season: Some(1),
        status: Some("downloaded".to_string()), // has file
        monitored: true,                        // user toggled ON (wants upgrades)
        meta_date: Some(past.naive_utc()),
        ..ep_row(None, None, None)
    }];
    let mut overridden = HashSet::new();
    overridden.insert("test-future-override-1".to_string());
    let mapping = s02_mapping();
    let rd_config = default_rd_config();
    let (to_monitor, to_unmonitor, to_clear_override) = classify_db_episodes(
        episodes,
        &mapping,
        MonitorMode::Future,
        &rd_config,
        false, // sweep: preserve state
        &overridden,
        false,
    );
    assert!(
        !to_monitor.contains(&"test-future-override-1".to_string()),
        "Overridden episode should not appear in to_monitor"
    );
    assert!(
        !to_unmonitor.contains(&"test-future-override-1".to_string()),
        "Overridden episode should not appear in to_unmonitor"
    );
    assert!(
        to_clear_override.is_empty(),
        "Override still diverges from mode → should not self-heal"
    );
}

#[test]
fn test_classify_override_clear_on_fresh_apply() {
    // fresh_apply=true (mode switch) passes empty overridden set.
    // Even if episodes have override=1 in the DB, classify_db_episodes
    // receives an empty set and treats them normally.
    // This simulates what happens after clear_monitor_overrides_batch.
    let episodes = vec![crate::db::EpisodeDetailRow {
        episode_id: "test-fresh-apply-1".to_string(),
        episode: 1,
        season: Some(1),
        status: None,     // no file
        monitored: false, // user toggled OFF on a Missing mode series
        ..ep_row(None, None, None)
    }];
    let overridden = HashSet::new(); // simulates fresh_apply=true (overrides already cleared)
    let mapping = s02_mapping();
    let rd_config = default_rd_config();
    let (to_monitor, to_unmonitor, _to_clear) = classify_db_episodes(
        episodes,
        &mapping,
        MonitorMode::Missing,
        &rd_config,
        true, // fresh_apply: mode switch
        &overridden,
        false,
    );
    // With fresh_apply=true and empty overrides, the user's toggle is wiped:
    // Missing mode + no file → monitored (regardless of previous state)
    assert!(
        to_monitor.contains(&"test-fresh-apply-1".to_string()),
        "Fresh apply should override user toggle: Missing+no file → monitored"
    );
    assert!(
        !to_unmonitor.contains(&"test-fresh-apply-1".to_string()),
        "Episode should not be unmonitored"
    );
}

#[test]
fn test_override_decision_proceed_no_override() {
    // No override → always proceed
    assert_eq!(
        crate::source_processor::monitoring::override_decision(false, true, false),
        crate::source_processor::monitoring::OverrideAction::Proceed,
    );
    assert_eq!(
        crate::source_processor::monitoring::override_decision(false, false, true),
        crate::source_processor::monitoring::OverrideAction::Proceed,
    );
}

#[test]
fn test_override_decision_skip_when_diverging() {
    // Override+diverging → skip
    assert_eq!(
        crate::source_processor::monitoring::override_decision(true, true, false),
        crate::source_processor::monitoring::OverrideAction::Skip,
        "mode wants true, user set false → skip"
    );
    assert_eq!(
        crate::source_processor::monitoring::override_decision(true, false, true),
        crate::source_processor::monitoring::OverrideAction::Skip,
        "mode wants false, user set true → skip"
    );
}

#[test]
fn test_override_decision_self_heal_when_mode_agrees() {
    // Override+agrees → self-heal
    assert_eq!(
        crate::source_processor::monitoring::override_decision(true, true, true),
        crate::source_processor::monitoring::OverrideAction::SelfHeal,
        "override set but mode agrees → self-heal"
    );
    assert_eq!(
        crate::source_processor::monitoring::override_decision(true, false, false),
        crate::source_processor::monitoring::OverrideAction::SelfHeal,
        "override set but mode agrees (both false) → self-heal"
    );
}

// classify_db_episodes: All mode
// Verifies that All mode correctly monitors all non-special episodes.

#[test]
fn test_classify_all_normal_monitored() {
    // All mode: regular episode should be monitored
    let episodes = vec![crate::db::EpisodeDetailRow {
        episode_id: "test-ep-1".to_string(),
        episode: 5,
        season: Some(2),
        status: None,
        monitored: false, // just inserted via metadata
        ..ep_row(None, None, None)
    }];
    let mapping = s02_mapping();
    let rd_config = default_rd_config();
    let (to_monitor, to_unmonitor, _to_clear) = classify_db_episodes(
        episodes,
        &mapping,
        MonitorMode::All,
        &rd_config,
        false,
        &HashSet::new(),
        false,
    );
    assert!(
        to_monitor.contains(&"test-ep-1".to_string()),
        "All mode + regular episode → should be monitored"
    );
    assert!(!to_unmonitor.contains(&"test-ep-1".to_string()));
}

#[test]
fn test_classify_all_special_not_monitored() {
    // All mode: special episode (season "0") should NOT be monitored
    let episodes = vec![crate::db::EpisodeDetailRow {
        episode_id: "test-ep-special".to_string(),
        episode: 1,
        season: Some(0),
        status: None,
        monitored: false, // just inserted via metadata
        ..ep_row(None, None, None)
    }];
    let mapping = s02_mapping();
    let rd_config = default_rd_config();
    let (to_monitor, to_unmonitor, _to_clear) = classify_db_episodes(
        episodes,
        &mapping,
        MonitorMode::All,
        &rd_config,
        false,
        &HashSet::new(),
        false,
    );
    assert!(
        to_unmonitor.contains(&"test-ep-special".to_string()),
        "All mode + special episode → should NOT be monitored"
    );
    assert!(!to_monitor.contains(&"test-ep-special".to_string()));
}

// classify_db_episodes: None mode
// Verifies that None mode unmonitors everything.

#[test]
fn test_classify_none_everything_unmonitored() {
    let episodes = vec![crate::db::EpisodeDetailRow {
        episode_id: "test-ep-1".to_string(),
        episode: 1,
        season: Some(1),
        status: Some("downloaded".to_string()),
        monitored: true, // was previously monitored
        ..ep_row(None, None, None)
    }];
    let mapping = s02_mapping();
    let rd_config = default_rd_config();
    let (to_monitor, to_unmonitor, _to_clear) = classify_db_episodes(
        episodes,
        &mapping,
        MonitorMode::None,
        &rd_config,
        false,
        &HashSet::new(),
        false,
    );
    assert!(
        to_unmonitor.contains(&"test-ep-1".to_string()),
        "None mode + any episode → should be unmonitored"
    );
    assert!(!to_monitor.contains(&"test-ep-1".to_string()));
}

// classify_db_episodes: Specials mode
// Verifies that Specials mode only monitors specials (season "0").

#[test]
fn test_classify_specials_s00_monitored() {
    let episodes = vec![crate::db::EpisodeDetailRow {
        episode_id: "test-ep-special".to_string(),
        episode: 1,
        season: Some(0),
        status: None,
        monitored: false, // just inserted via metadata
        ..ep_row(None, None, None)
    }];
    let mapping = s02_mapping();
    let rd_config = default_rd_config();
    let (to_monitor, to_unmonitor, _to_clear) = classify_db_episodes(
        episodes,
        &mapping,
        MonitorMode::Specials,
        &rd_config,
        false,
        &HashSet::new(),
        false,
    );
    assert!(
        to_monitor.contains(&"test-ep-special".to_string()),
        "Specials mode + special episode → should be monitored"
    );
    assert!(!to_unmonitor.contains(&"test-ep-special".to_string()));
}

#[test]
fn test_classify_specials_regular_not_affected() {
    // Specials mode only monitors specials — regular episodes are left as-is
    // (neither pushed to to_monitor nor to_unmonitor).
    let episodes = vec![crate::db::EpisodeDetailRow {
        episode_id: "test-ep-regular".to_string(),
        episode: 5,
        season: Some(1),
        status: None,
        monitored: false,
        ..ep_row(None, None, None)
    }];
    let mapping = s02_mapping();
    let rd_config = default_rd_config();
    let (to_monitor, to_unmonitor, _to_clear) = classify_db_episodes(
        episodes,
        &mapping,
        MonitorMode::Specials,
        &rd_config,
        false,
        &HashSet::new(),
        false,
    );
    assert!(
        !to_monitor.contains(&"test-ep-regular".to_string()),
        "Specials mode + regular episode → should NOT be in to_monitor"
    );
    assert!(
        !to_unmonitor.contains(&"test-ep-regular".to_string()),
        "Specials mode + regular episode → should NOT be in to_unmonitor (left as-is)"
    );
}

// classify_db_episodes: Metadata-sync flow (batch_insert + reapply)
// Simulates the exact scenario: batch_insert_metadata_episodes inserts
// episodes with monitored=0, then reapply_monitor_for_series runs.
// Each mode is verified for both "new metadata episode" (monitored=false)
// and "existing episode" (monitored=true) cases.

#[test]
fn test_metadata_flow_all_monitors_non_specials() {
    // Simulates: metadata inserts episodes, then Future/All/Missing/etc. apply.
    // All mode: regular episodes monitored, specials not.
    let episodes = vec![
        crate::db::EpisodeDetailRow {
            episode_id: "ep-regular".to_string(),
            episode: 3,
            season: Some(1),
            status: None,
            monitored: false, // batch_insert sets this to 0
            ..ep_row(None, None, None)
        },
        crate::db::EpisodeDetailRow {
            episode_id: "ep-special".to_string(),
            episode: 1,
            season: Some(0),
            status: None,
            monitored: false, // batch_insert sets this to 0
            ..ep_row(None, None, None)
        },
    ];
    let mapping = s02_mapping();
    let rd_config = default_rd_config();
    let (to_monitor, to_unmonitor, _to_clear) = classify_db_episodes(
        episodes,
        &mapping,
        MonitorMode::All,
        &rd_config,
        false,
        &HashSet::new(),
        false,
    );
    assert!(
        to_monitor.contains(&"ep-regular".to_string()),
        "Metadata-flow + All mode: regular episode should be monitored"
    );
    assert!(
        to_unmonitor.contains(&"ep-special".to_string()),
        "Metadata-flow + All mode: special should not be monitored"
    );
}

#[test]
fn test_metadata_flow_future_past_episodes_not_monitored() {
    // Simulates: metadata inserts old/past episodes with monitored=0.
    // Future mode: past-dated episodes should NOT be monitored.
    let past = chrono::Utc::now() - chrono::Duration::days(60);
    let episodes = vec![crate::db::EpisodeDetailRow {
        episode_id: "ep-old".to_string(),
        episode: 5,
        season: Some(1),
        status: None,
        monitored: false, // batch_insert sets this to 0
        meta_date: Some(past.naive_utc()),
        ..ep_row(None, None, None)
    }];
    let mapping = s02_mapping();
    let rd_config = default_rd_config();
    let (to_monitor, to_unmonitor, _to_clear) = classify_db_episodes(
        episodes,
        &mapping,
        MonitorMode::Future,
        &rd_config,
        false,
        &HashSet::new(),
        false,
    );
    assert!(
        !to_monitor.contains(&"ep-old".to_string()),
        "Metadata-flow + Future mode: old past-dated episode should NOT be monitored"
    );
    let _ = to_unmonitor;
}

#[test]
fn test_metadata_flow_missing_monitors_no_file() {
    // Missing mode: metadata-inserted episodes with no file should be monitored.
    let episodes = vec![
        crate::db::EpisodeDetailRow {
            episode_id: "ep-missing".to_string(),
            episode: 1,
            season: Some(1),
            status: None,
            monitored: false, // batch_insert sets this to 0
            ..ep_row(None, None, None)
        },
        crate::db::EpisodeDetailRow {
            episode_id: "ep-with-file".to_string(),
            episode: 2,
            season: Some(1),
            status: Some("downloaded".to_string()),
            monitored: true, // was previously monitored (already had file)
            ..ep_row(None, None, None)
        },
    ];
    let mapping = s02_mapping();
    let rd_config = default_rd_config();
    let (to_monitor, to_unmonitor, _to_clear) = classify_db_episodes(
        episodes,
        &mapping,
        MonitorMode::Missing,
        &rd_config,
        false,
        &HashSet::new(),
        false,
    );
    assert!(
        to_monitor.contains(&"ep-missing".to_string()),
        "Missing mode + no file → should be monitored"
    );
    assert!(
        to_unmonitor.contains(&"ep-with-file".to_string()),
        "Missing mode + has file → should be unmonitored"
    );
}

#[test]
fn test_metadata_flow_pilot_only_s01e01() {
    // Pilot mode: only S01E01 should be monitored.
    let episodes = vec![
        crate::db::EpisodeDetailRow {
            episode_id: "ep-pilot".to_string(),
            episode: 1,
            season: Some(1),
            status: None,
            monitored: false,
            ..ep_row(None, None, None)
        },
        crate::db::EpisodeDetailRow {
            episode_id: "ep-s01e02".to_string(),
            episode: 2,
            season: Some(1),
            status: None,
            monitored: false,
            ..ep_row(None, None, None)
        },
        crate::db::EpisodeDetailRow {
            episode_id: "ep-s02e01".to_string(),
            episode: 1,
            season: Some(2),
            status: None,
            monitored: false,
            ..ep_row(None, None, None)
        },
    ];
    let mapping = s02_mapping();
    let rd_config = default_rd_config();
    let (to_monitor, to_unmonitor, _to_clear) = classify_db_episodes(
        episodes,
        &mapping,
        MonitorMode::Pilot,
        &rd_config,
        false,
        &HashSet::new(),
        false,
    );
    assert!(
        to_monitor.contains(&"ep-pilot".to_string()),
        "Pilot mode + S01E01 → should be monitored"
    );
    assert!(
        to_unmonitor.contains(&"ep-s01e02".to_string()),
        "Pilot mode + S01E02 → should NOT be monitored"
    );
    assert!(
        to_unmonitor.contains(&"ep-s02e01".to_string()),
        "Pilot mode + S02E01 → should NOT be monitored"
    );
}

#[test]
fn test_metadata_flow_first_season_only_s01() {
    // FirstSeason mode: only season 1 episodes should be monitored.
    let episodes = vec![
        crate::db::EpisodeDetailRow {
            episode_id: "ep-s01".to_string(),
            episode: 5,
            season: Some(1),
            status: None,
            monitored: false,
            ..ep_row(None, None, None)
        },
        crate::db::EpisodeDetailRow {
            episode_id: "ep-s02".to_string(),
            episode: 1,
            season: Some(2),
            status: None,
            monitored: false,
            ..ep_row(None, None, None)
        },
    ];
    let mapping = s02_mapping();
    let rd_config = default_rd_config();
    let (to_monitor, to_unmonitor, _to_clear) = classify_db_episodes(
        episodes,
        &mapping,
        MonitorMode::FirstSeason,
        &rd_config,
        false,
        &HashSet::new(),
        false,
    );
    assert!(
        to_monitor.contains(&"ep-s01".to_string()),
        "FirstSeason mode + S01 → should be monitored"
    );
    assert!(
        to_unmonitor.contains(&"ep-s02".to_string()),
        "FirstSeason mode + S02 → should NOT be monitored"
    );
}

#[test]
fn test_metadata_flow_none_unmonitors_everything() {
    // None mode: even episodes that were previously monitored become unmonitored.
    let episodes = vec![crate::db::EpisodeDetailRow {
        episode_id: "ep-with-file".to_string(),
        episode: 1,
        season: Some(1),
        status: Some("downloaded".to_string()),
        monitored: true, // was monitored under previous mode
        ..ep_row(None, None, None)
    }];
    let mapping = s02_mapping();
    let rd_config = default_rd_config();
    let (to_monitor, to_unmonitor, _to_clear) = classify_db_episodes(
        episodes,
        &mapping,
        MonitorMode::None,
        &rd_config,
        false,
        &HashSet::new(),
        false,
    );
    assert!(
        to_unmonitor.contains(&"ep-with-file".to_string()),
        "None mode + any episode → should be unmonitored"
    );
    assert!(!to_monitor.contains(&"ep-with-file".to_string()));
}

#[test]
fn test_metadata_flow_existing_only_has_file() {
    // Existing mode: only episodes that already have a file should be monitored.
    let episodes = vec![
        crate::db::EpisodeDetailRow {
            episode_id: "ep-has-file".to_string(),
            episode: 1,
            season: Some(1),
            status: Some("organized".to_string()),
            monitored: false, // new metadata insert
            ..ep_row(None, None, None)
        },
        crate::db::EpisodeDetailRow {
            episode_id: "ep-missing".to_string(),
            episode: 2,
            season: Some(1),
            status: None,
            monitored: false, // new metadata insert
            ..ep_row(None, None, None)
        },
    ];
    let mapping = s02_mapping();
    let rd_config = default_rd_config();
    let (to_monitor, to_unmonitor, _to_clear) = classify_db_episodes(
        episodes,
        &mapping,
        MonitorMode::Existing,
        &rd_config,
        false,
        &HashSet::new(),
        false,
    );
    assert!(
        to_monitor.contains(&"ep-has-file".to_string()),
        "Existing mode + has file → should be monitored"
    );
    assert!(
        to_unmonitor.contains(&"ep-missing".to_string()),
        "Existing mode + no file → should NOT be monitored"
    );
}
