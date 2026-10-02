use std::sync::Arc;

use crate::models::media::ReleaseCandidate;
use crate::source_processor::group_candidates_by_season;
use jumbie_shared::mapping::{EpisodeInfo, MappingRule};

fn candidate(
    series_id: &str,
    name: &str,
    seasons: Vec<i32>,
    needed_episodes: Vec<i32>,
    is_season_pack: bool,
    is_complete_pack: bool,
) -> ReleaseCandidate {
    ReleaseCandidate {
        title: format!("{name} test"),
        download_url: None,
        episode_info: EpisodeInfo {
            raw_title: String::new(),
            series_key: name.to_string(),
            file_ext: "mkv".to_string(),
            submitter: None,
            resolution: None,
            version: 1,
            part_number: None,
            is_season_pack,
            is_complete_pack,
            seasons,
            episodes: needed_episodes.clone(),
            has_decimal_episode: false,
        },
        mapping: Arc::new(MappingRule {
            series_id: series_id.to_string(),
            name: name.to_string(),
            target_title: name.to_string(),
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

fn single(series_id: &str, name: &str, season: i32, episode: i32) -> ReleaseCandidate {
    candidate(series_id, name, vec![season], vec![episode], false, false)
}

#[test]
fn same_named_series_resolve_in_separate_groups() {
    // Two distinct series sharing a display name must not land in one conflict group.
    let a = single("twins-a", "Twin Peaks", 1, 1);
    let b = single("twins-b", "Twin Peaks", 1, 1);

    let groups = group_candidates_by_season(vec![a, b]);

    assert_eq!(
        groups.len(),
        2,
        "same-named series must resolve independently"
    );
    assert!(groups.contains_key(&("twins-a".to_string(), "1".to_string())));
    assert!(groups.contains_key(&("twins-b".to_string(), "1".to_string())));
}

#[test]
fn episodes_of_one_series_and_season_share_a_group() {
    let first = single("show-a", "Show A", 1, 1);
    let second = single("show-a", "Show A", 1, 2);

    let groups = group_candidates_by_season(vec![first, second]);

    assert_eq!(groups.len(), 1);
    assert_eq!(groups[&("show-a".to_string(), "1".to_string())].len(), 2);
}

#[test]
fn different_seasons_are_separate_groups() {
    let first = single("show-a", "Show A", 1, 1);
    let second = single("show-a", "Show A", 2, 1);

    let groups = group_candidates_by_season(vec![first, second]);

    assert_eq!(groups.len(), 2);
}

#[test]
fn seasonless_candidate_defaults_to_season_one_group() {
    let candidate = candidate("show-a", "Show A", vec![], vec![5], false, false);

    let groups = group_candidates_by_season(vec![candidate]);

    assert!(groups.contains_key(&("show-a".to_string(), "01".to_string())));
}

#[test]
fn season_zero_packs_are_dropped() {
    let pack = candidate("show-a", "Show A", vec![0], vec![1], true, false);

    let groups = group_candidates_by_season(vec![pack]);

    assert!(
        groups.is_empty(),
        "season 0 packs are blocked from auto-download"
    );
}

#[test]
fn single_episode_with_nothing_needed_is_dropped() {
    let candidate = candidate("show-a", "Show A", vec![1], vec![], false, false);

    let groups = group_candidates_by_season(vec![candidate]);

    assert!(groups.is_empty());
}

#[test]
fn pack_with_empty_needed_episodes_is_kept_for_upgrade_check() {
    let pack = candidate("show-a", "Show A", vec![1], vec![], true, false);

    let groups = group_candidates_by_season(vec![pack]);

    assert_eq!(
        groups.len(),
        1,
        "version-upgrade packs must survive grouping"
    );
}
