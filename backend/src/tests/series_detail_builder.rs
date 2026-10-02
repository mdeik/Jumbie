use std::collections::HashMap;

use jumbie_shared::formatting::{LabelStyle, fmt_episode};
use jumbie_shared::mapping::{SeasonOverride, SeriesSettings};
use jumbie_shared::types::{EpisodeViewModel, MappingRule};

use crate::api_routes::series_detail_builder::fill_missing_episodes;

/// Build a minimal `MappingRule` with the given season overrides.
fn make_mapping(season_overrides: HashMap<String, SeasonOverride>) -> MappingRule {
    MappingRule {
        target_title: "TestShow".to_string(),
        name: "TestShow".to_string(),
        series_id: "test-series-1234".to_string(),
        settings: SeriesSettings {
            absolute_numbering: Some(false),
            season: season_overrides,
            ..Default::default()
        },
        ..Default::default()
    }
}

/// Build a minimal `EpisodeViewModel` for a given season/episode.
///
/// - `have_file`: when `true` sets `path` to a non‑empty string and `status` to
///   `"downloaded"`.  When `false`, `path` is `None` and `status` is `"missing"`
///   (caller should override `status` if needed).
fn make_ep(season: &str, episode: i32, have_file: bool) -> EpisodeViewModel {
    let season_fmt = format!("S{}", season);
    let status = if have_file {
        "downloaded".to_string()
    } else {
        "missing".to_string()
    };
    EpisodeViewModel {
        unique_id: format!("{}_S{}E{:02}", "TestShow", season, episode),
        season: season_fmt.clone(),
        episode,
        release_title: None,
        header: fmt_episode(episode, LabelStyle::Human),
        title: Some(format!("Episode {}", episode)),
        status,
        quality_profile_id: None,
        size: if have_file { 500_000_000 } else { 0 },
        submitter: if have_file { Some("GRP".into()) } else { None },
        path: if have_file {
            Some(format!("/path/S{}E{:02}.mkv", season, episode))
        } else {
            None
        },
        original_path: None,
        media_info: None,
        fingerprint: if have_file {
            Some("abc123".into())
        } else {
            None
        },
        created_at: if have_file {
            Some("2024-01-01 12:00:00".into())
        } else {
            None
        },
        file_acquired_at: if have_file {
            Some("2024-01-01 12:00:00".into())
        } else {
            None
        },
        monitored: true,
        dates: jumbie_shared::types::ReleaseDates {
            meta_date: None,
            upload_date: None,
            est_date: None,
        },
        metadata_ids: HashMap::new(),
        metadata_source: None,
        description: None,
        runtime: None,
        image_url: None,
        parts: vec![],
        auxiliary_files: Vec::new(),
        show_only_downloaded: false,
        assigned: false,
        disk_present: false,
    }
}

/// Assert that a list of episodes has the expected statuses.
fn assert_statuses(episodes: &[EpisodeViewModel], expected: &[(&str, i32, &str)]) {
    assert_eq!(
        episodes.len(),
        expected.len(),
        "expected {} episodes, got {}:\n{:#?}",
        expected.len(),
        episodes.len(),
        episodes
    );
    for (i, (exp_season, exp_ep, exp_status)) in expected.iter().enumerate() {
        assert_eq!(
            episodes[i].season, *exp_season,
            "episode {}: expected season {} got {}",
            i, exp_season, episodes[i].season
        );
        assert_eq!(
            episodes[i].episode, *exp_ep,
            "episode {}: expected episode {} got {}",
            i, exp_ep, episodes[i].episode
        );
        assert_eq!(
            episodes[i].status, *exp_status,
            "episode {} (S{}E{}): expected status '{}' got '{}'",
            i, exp_season, exp_ep, exp_status, episodes[i].status
        );
    }
}

#[test]
fn test_fill_no_cell_count_no_db_episodes() {
    // Scenario 1: No cell_count, no DB episodes → empty result
    let mapping = make_mapping(HashMap::new());
    let season = "01".to_string();
    let normalized_seasons = vec![season.clone()];
    let episode_map = HashMap::new();
    let _max_episodes: HashMap<String, i32> = HashMap::new();

    let result = fill_missing_episodes(&mapping, &normalized_seasons, &episode_map, false);

    assert!(
        result.is_empty(),
        "expected empty result, got {} episodes",
        result.len()
    );
}

#[test]
fn test_fill_no_cell_count_all_downloaded() {
    // Scenario 2: No cell_count, 3 DB episodes (all downloaded) → 3 downloaded
    let mapping = make_mapping(HashMap::new());
    let season = "01".to_string();
    let normalized_seasons = vec![season.clone()];
    let mut episode_map = HashMap::new();
    episode_map.insert(("S01".to_string(), 1), make_ep("01", 1, true));
    episode_map.insert(("S01".to_string(), 2), make_ep("01", 2, true));
    episode_map.insert(("S01".to_string(), 3), make_ep("01", 3, true));
    let _max_episodes: HashMap<String, i32> = HashMap::new();

    let result = fill_missing_episodes(&mapping, &normalized_seasons, &episode_map, false);

    assert_eq!(result.len(), 3);
    for ep in &result {
        assert_eq!(ep.status, "downloaded");
    }
}

#[test]
fn test_fill_no_cell_count_mixed_status() {
    // Scenario 3: No cell_count, 2 DB episodes (1 downloaded, 1 missing) → 2 episodes
    let mapping = make_mapping(HashMap::new());
    let season = "01".to_string();
    let normalized_seasons = vec![season.clone()];
    let mut episode_map = HashMap::new();
    episode_map.insert(("S01".to_string(), 1), make_ep("01", 1, true));
    let mut ep2 = make_ep("01", 2, false);
    ep2.status = "missing".to_string();
    episode_map.insert(("S01".to_string(), 2), ep2);
    let _max_episodes: HashMap<String, i32> = HashMap::new();

    let result = fill_missing_episodes(&mapping, &normalized_seasons, &episode_map, false);

    assert_eq!(result.len(), 2);
    assert_eq!(result[0].status, "downloaded");
    assert_eq!(result[1].status, "missing");
}

#[test]
fn test_fill_cell_count_no_db_episodes() {
    // Scenario 4: cell_count=5, no DB episodes → 5 placeholders, all missing
    let mut overrides = HashMap::new();
    overrides.insert(
        "1".to_string(),
        SeasonOverride {
            season: "1".to_string(),
            cell_count: Some(5),
            episode_start: None,
            episode_end: None,
            episode_offset: None,
            alias_season_number: None,
            search_format: None,
            aliases: vec![],
            reg_patterns: vec![],
        },
    );
    let mapping = make_mapping(overrides);
    let normalized_seasons = vec!["01".to_string()];
    let episode_map = HashMap::new();
    let _max_episodes: HashMap<String, i32> = HashMap::new();

    let result = fill_missing_episodes(&mapping, &normalized_seasons, &episode_map, false);

    assert_eq!(result.len(), 5);
    for (i, ep) in result.iter().enumerate() {
        assert_eq!(
            ep.episode,
            (i + 1) as i32,
            "E{:02} has wrong episode number",
            i + 1
        );
        assert_eq!(ep.season, "S01");
        assert_eq!(ep.status, "missing", "E{:02} should be missing", i + 1);
        assert!(ep.monitored, "E{:02} should be monitored", i + 1);
        assert!(ep.path.is_none(), "E{:02} should have no path", i + 1);
    }
}

#[test]
fn test_fill_cell_count_interleaved_real_and_placeholder() {
    // Scenario 5: cell_count=5, DB episodes (downloaded) in slots 1,3,5
    // Expected: 5 items: E01 (real), E02 (placeholder), E03 (real), E04 (placeholder), E05 (real)
    let mut overrides = HashMap::new();
    overrides.insert(
        "1".to_string(),
        SeasonOverride {
            season: "1".to_string(),
            cell_count: Some(5),
            episode_start: None,
            episode_end: None,
            episode_offset: None,
            alias_season_number: None,
            search_format: None,
            aliases: vec![],
            reg_patterns: vec![],
        },
    );
    let mapping = make_mapping(overrides);
    let normalized_seasons = vec!["01".to_string()];
    let mut episode_map = HashMap::new();
    episode_map.insert(("S01".to_string(), 1), make_ep("01", 1, true));
    episode_map.insert(("S01".to_string(), 3), make_ep("01", 3, true));
    episode_map.insert(("S01".to_string(), 5), make_ep("01", 5, true));
    let _max_episodes: HashMap<String, i32> = HashMap::new();

    let result = fill_missing_episodes(&mapping, &normalized_seasons, &episode_map, false);

    assert_eq!(result.len(), 5);
    assert_statuses(
        &result,
        &[
            ("S01", 1, "downloaded"),
            ("S01", 2, "missing"),
            ("S01", 3, "downloaded"),
            ("S01", 4, "missing"),
            ("S01", 5, "downloaded"),
        ],
    );
}

#[test]
fn test_fill_cell_count_with_beyond_range_episode_having_file() {
    // Scenario 6: cell_count=5, DB has E01(have) + E12(have)
    // Expected: E01 (real) + E02-E05 (placeholders, missing) + E12 (appended, downloaded)
    let mut overrides = HashMap::new();
    overrides.insert(
        "1".to_string(),
        SeasonOverride {
            season: "1".to_string(),
            cell_count: Some(5),
            episode_start: None,
            episode_end: None,
            episode_offset: None,
            alias_season_number: None,
            search_format: None,
            aliases: vec![],
            reg_patterns: vec![],
        },
    );
    let mapping = make_mapping(overrides);
    let normalized_seasons = vec!["01".to_string()];
    let mut episode_map = HashMap::new();
    episode_map.insert(("S01".to_string(), 1), make_ep("01", 1, true));
    let mut ep12 = make_ep("01", 12, true);
    ep12.status = "downloaded".to_string();
    episode_map.insert(("S01".to_string(), 12), ep12);
    let _max_episodes: HashMap<String, i32> = HashMap::new();

    let result = fill_missing_episodes(&mapping, &normalized_seasons, &episode_map, false);

    assert_eq!(result.len(), 6, "expected 6 episodes (5 cell + 1 appended)");
    assert_statuses(
        &result,
        &[
            ("S01", 1, "downloaded"),
            ("S01", 2, "missing"),
            ("S01", 3, "missing"),
            ("S01", 4, "missing"),
            ("S01", 5, "missing"),
            ("S01", 12, "downloaded"),
        ],
    );
}

#[test]
fn test_fill_cell_count_beyond_range_episode_no_file_hidden() {
    // Scenario 7: cell_count=5, DB has E01(missing, no file) + E12(missing, no file)
    // Expected: E01 (from map, status "missing") + E02-E05 (placeholders, missing).
    // E12 is NOT shown because it has no file and is beyond cell_count.
    let mut overrides = HashMap::new();
    overrides.insert(
        "1".to_string(),
        SeasonOverride {
            season: "1".to_string(),
            cell_count: Some(5),
            episode_start: None,
            episode_end: None,
            episode_offset: None,
            alias_season_number: None,
            search_format: None,
            aliases: vec![],
            reg_patterns: vec![],
        },
    );
    let mapping = make_mapping(overrides);
    let normalized_seasons = vec!["01".to_string()];
    let mut episode_map = HashMap::new();
    episode_map.insert(("S01".to_string(), 1), make_ep("01", 1, false));
    let mut ep12 = make_ep("01", 12, false);
    ep12.status = "missing".to_string();
    episode_map.insert(("S01".to_string(), 12), ep12);
    let _max_episodes: HashMap<String, i32> = HashMap::new();

    let result = fill_missing_episodes(&mapping, &normalized_seasons, &episode_map, false);

    assert_eq!(
        result.len(),
        5,
        "expected 5 episodes (E12 without file should be hidden)"
    );
    assert_statuses(
        &result,
        &[
            ("S01", 1, "missing"),
            ("S01", 2, "missing"),
            ("S01", 3, "missing"),
            ("S01", 4, "missing"),
            ("S01", 5, "missing"),
        ],
    );
}

#[test]
fn test_fill_cell_count_with_episode_start_configured() {
    // Scenario 8: cell_count=5, episode_start=2
    // DB has E01 (downloaded) + E03 (downloaded)
    // Expected: E01 (out_of_range), E02 (placeholder, missing), E03 (real, downloaded),
    //           E04 (placeholder, missing), E05 (placeholder, missing)
    let mut overrides = HashMap::new();
    overrides.insert(
        "1".to_string(),
        SeasonOverride {
            season: "1".to_string(),
            cell_count: Some(5),
            episode_start: Some(2),
            episode_end: None,
            episode_offset: None,
            alias_season_number: None,
            search_format: None,
            aliases: vec![],
            reg_patterns: vec![],
        },
    );
    let mapping = make_mapping(overrides);
    let normalized_seasons = vec!["01".to_string()];
    let mut episode_map = HashMap::new();
    episode_map.insert(("S01".to_string(), 1), make_ep("01", 1, true));
    episode_map.insert(("S01".to_string(), 3), make_ep("01", 3, true));
    let _max_episodes: HashMap<String, i32> = HashMap::new();

    let result = fill_missing_episodes(&mapping, &normalized_seasons, &episode_map, false);

    assert_eq!(result.len(), 5);
    assert_statuses(
        &result,
        &[
            ("S01", 1, "out_of_range"),
            ("S01", 2, "missing"),
            ("S01", 3, "downloaded"),
            ("S01", 4, "missing"),
            ("S01", 5, "missing"),
        ],
    );
    // E01 should NOT be monitored since it's out_of_range
    assert!(
        !result[0].monitored,
        "E01 should not be monitored (out_of_range)"
    );
}

#[test]
fn test_fill_cell_count_with_episode_end_configured() {
    // Scenario 9: cell_count=5, episode_end=3
    // DB has E01 (downloaded) + E03 (downloaded)
    // Expected: E01 (downloaded), E02 (placeholder, missing), E03 (downloaded),
    //           E04 (placeholder, out_of_range), E05 (placeholder, out_of_range)
    let mut overrides = HashMap::new();
    overrides.insert(
        "1".to_string(),
        SeasonOverride {
            season: "1".to_string(),
            cell_count: Some(5),
            episode_end: Some(3),
            episode_start: None,
            episode_offset: None,
            alias_season_number: None,
            search_format: None,
            aliases: vec![],
            reg_patterns: vec![],
        },
    );
    let mapping = make_mapping(overrides);
    let normalized_seasons = vec!["01".to_string()];
    let mut episode_map = HashMap::new();
    episode_map.insert(("S01".to_string(), 1), make_ep("01", 1, true));
    episode_map.insert(("S01".to_string(), 3), make_ep("01", 3, true));
    let _max_episodes: HashMap<String, i32> = HashMap::new();

    let result = fill_missing_episodes(&mapping, &normalized_seasons, &episode_map, false);

    assert_eq!(result.len(), 5);
    assert_statuses(
        &result,
        &[
            ("S01", 1, "downloaded"),
            ("S01", 2, "missing"),
            ("S01", 3, "downloaded"),
            ("S01", 4, "out_of_range"),
            ("S01", 5, "out_of_range"),
        ],
    );
    // E04 and E05 should NOT be monitored
    assert!(
        !result[3].monitored,
        "E04 should not be monitored (out_of_range)"
    );
    assert!(
        !result[4].monitored,
        "E05 should not be monitored (out_of_range)"
    );
}

#[test]
fn test_fill_no_cell_count_shows_all_db_episodes() {
    // Scenario 10: No cell_count, DB has E01(have) + E03(missing)
    // Expected: both shown — E01 (downloaded), E03 (missing)
    let mapping = make_mapping(HashMap::new());
    let normalized_seasons = vec!["01".to_string()];
    let mut episode_map = HashMap::new();
    episode_map.insert(("S01".to_string(), 1), make_ep("01", 1, true));
    let mut ep3 = make_ep("01", 3, false);
    ep3.status = "missing".to_string();
    episode_map.insert(("S01".to_string(), 3), ep3);
    let _max_episodes: HashMap<String, i32> = HashMap::new();

    let result = fill_missing_episodes(&mapping, &normalized_seasons, &episode_map, false);

    assert_eq!(result.len(), 2);
    assert_eq!(result[0].episode, 1);
    assert_eq!(result[0].status, "downloaded");
    assert_eq!(result[1].episode, 3);
    assert_eq!(result[1].status, "missing");
}

#[test]
fn test_fill_multiple_seasons_with_cell_count() {
    // Scenario 11: Multiple seasons — S01 cell_count=3, S02 cell_count=2
    // DB: S01E01 (downloaded), S02E01 (downloaded)
    // Expected: S01: E01 (downloaded), E02 (missing), E03 (missing)
    //           S02: E01 (downloaded), E02 (missing)
    let mut overrides = HashMap::new();
    overrides.insert(
        "1".to_string(),
        SeasonOverride {
            season: "1".to_string(),
            cell_count: Some(3),
            episode_start: None,
            episode_end: None,
            episode_offset: None,
            alias_season_number: None,
            search_format: None,
            aliases: vec![],
            reg_patterns: vec![],
        },
    );
    overrides.insert(
        "2".to_string(),
        SeasonOverride {
            season: "2".to_string(),
            cell_count: Some(2),
            episode_start: None,
            episode_end: None,
            episode_offset: None,
            alias_season_number: None,
            search_format: None,
            aliases: vec![],
            reg_patterns: vec![],
        },
    );
    let mapping = make_mapping(overrides);
    let normalized_seasons = vec!["01".to_string(), "02".to_string()];
    let mut episode_map = HashMap::new();
    episode_map.insert(("S01".to_string(), 1), make_ep("01", 1, true));
    episode_map.insert(("S02".to_string(), 1), make_ep("02", 1, true));
    let _max_episodes: HashMap<String, i32> = HashMap::new();

    let result = fill_missing_episodes(&mapping, &normalized_seasons, &episode_map, false);

    assert_eq!(
        result.len(),
        5,
        "expected 3 from S01 + 2 from S02 = 5 total"
    );
    assert_statuses(
        &result,
        &[
            ("S01", 1, "downloaded"),
            ("S01", 2, "missing"),
            ("S01", 3, "missing"),
            ("S02", 1, "downloaded"),
            ("S02", 2, "missing"),
        ],
    );
}

#[test]
fn test_fill_cell_count_zero_treated_as_none() {
    // cell_count=Some(0) should be treated like None — show only DB episodes.
    // The match arm `Some(n) if n > 0` captures this.
    let mut overrides = HashMap::new();
    overrides.insert(
        "1".to_string(),
        SeasonOverride {
            season: "1".to_string(),
            cell_count: Some(0),
            episode_start: None,
            episode_end: None,
            episode_offset: None,
            alias_season_number: None,
            search_format: None,
            aliases: vec![],
            reg_patterns: vec![],
        },
    );
    let mapping = make_mapping(overrides);
    let normalized_seasons = vec!["01".to_string()];
    let mut episode_map = HashMap::new();
    episode_map.insert(("S01".to_string(), 1), make_ep("01", 1, true));
    episode_map.insert(("S01".to_string(), 3), make_ep("01", 3, false));
    let _max_episodes: HashMap<String, i32> = HashMap::new();

    let result = fill_missing_episodes(&mapping, &normalized_seasons, &episode_map, false);

    assert_eq!(
        result.len(),
        2,
        "should show only DB episodes when cell_count=0"
    );
    assert_eq!(result[0].episode, 1);
    assert_eq!(result[1].episode, 3);
}

#[test]
fn test_fill_cell_count_multiple_beyond_range_with_files() {
    // cell_count=3, DB has E01 (have), E05 (have), E07 (have) → should show
    // E01 (in-range), E02-E03 (placeholders), E05 + E07 (appended with files)
    let mut overrides = HashMap::new();
    overrides.insert(
        "1".to_string(),
        SeasonOverride {
            season: "1".to_string(),
            cell_count: Some(3),
            episode_start: None,
            episode_end: None,
            episode_offset: None,
            alias_season_number: None,
            search_format: None,
            aliases: vec![],
            reg_patterns: vec![],
        },
    );
    let mapping = make_mapping(overrides);
    let normalized_seasons = vec!["01".to_string()];
    let mut episode_map = HashMap::new();
    episode_map.insert(("S01".to_string(), 1), make_ep("01", 1, true)); // in-range
    episode_map.insert(("S01".to_string(), 5), make_ep("01", 5, true)); // beyond, with file
    episode_map.insert(("S01".to_string(), 7), make_ep("01", 7, true)); // beyond, with file
    let _max_episodes: HashMap<String, i32> = HashMap::new();

    let result = fill_missing_episodes(&mapping, &normalized_seasons, &episode_map, false);

    // Expected: E01 (real), E02 (placeholder), E03 (placeholder), E05 (appended), E07 (appended)
    assert_eq!(result.len(), 5);
    assert_statuses(
        &result,
        &[
            ("S01", 1, "downloaded"),
            ("S01", 2, "missing"),
            ("S01", 3, "missing"),
            ("S01", 5, "downloaded"),
            ("S01", 7, "downloaded"),
        ],
    );
}
