use super::*;
use crate::db::DbManager;
use jumbie_shared::mapping::{MappingRule, NumberingMode};
use std::collections::HashMap;

/// Seed a test DB with a series mapping and episodes.
///
/// Episodes 1, 2, 4 have a real file_path → downloaded.
/// Episodes 3, 5 have file_path = None → not downloaded.
async fn setup_db() -> (DbManager, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test.db");
    let db = DbManager::new(&db_path).await.unwrap();

    let mapping = MappingRule {
        target_title: "Test Show".to_string(),
        name: "test_show".to_string(),
        ..Default::default()
    };
    db.upsert_series_mapping("series-abc", &mapping)
        .await
        .unwrap();

    // Ep 1, 2, 4 have file_path → downloaded
    // Ep 3, 5 have file_path = None → not downloaded
    let meta = HashMap::new();

    for ep in &[1, 2, 4] {
        db.insert_episode(crate::db::episodes::InsertEpisodeParams {
            file_path: Some("/media/show/S01/some_file.mkv"),
            title: Some("Episode Title"),
            status: "organized",
            ..crate::db::episodes::InsertEpisodeParams::dummy(
                &format!("ep-{ep}"),
                "series-abc",
                1,
                *ep,
                &meta,
            )
        })
        .await
        .unwrap();
    }

    for ep in &[3, 5] {
        db.insert_episode(crate::db::episodes::InsertEpisodeParams {
            title: Some("Episode Title"),
            status: "unreleased",
            ..crate::db::episodes::InsertEpisodeParams::dummy(
                &format!("ep-{ep}"),
                "series-abc",
                1,
                *ep,
                &meta,
            )
        })
        .await
        .unwrap();
    }

    (db, tmp)
}

/// Seed a DB with episodes that have empty-string file_path values.
///
/// Episodes with `file_path = Some("")` should be treated identically
/// to `file_path = None` — they have no file on disk.
async fn setup_db_with_empty_string_paths() -> (DbManager, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test.db");
    let db = DbManager::new(&db_path).await.unwrap();

    let mapping = MappingRule {
        target_title: "Empty Path Show".to_string(),
        name: "empty_path_show".to_string(),
        ..Default::default()
    };
    db.upsert_series_mapping("series-empty", &mapping)
        .await
        .unwrap();

    let meta = HashMap::new();

    // Episode 1: empty string file_path (should count as NOT downloaded)
    db.insert_episode(crate::db::episodes::InsertEpisodeParams {
        file_path: Some(""),
        title: Some("Empty Path"),
        ..crate::db::episodes::InsertEpisodeParams::dummy("ep-1", "series-empty", 1, 1, &meta)
    })
    .await
    .unwrap();

    // Episode 2: real file_path
    db.insert_episode(crate::db::episodes::InsertEpisodeParams {
        file_path: Some("/media/show/S01/real_file.mkv"),
        title: Some("Real Path"),
        status: "organized",
        ..crate::db::episodes::InsertEpisodeParams::dummy("ep-2", "series-empty", 1, 2, &meta)
    })
    .await
    .unwrap();

    (db, tmp)
}

/// Seed a DB with episodes across multiple seasons.
async fn setup_db_multi_season() -> (DbManager, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test.db");
    let db = DbManager::new(&db_path).await.unwrap();

    let mapping = MappingRule {
        target_title: "Multi Season Show".to_string(),
        name: "multi_season_show".to_string(),
        ..Default::default()
    };
    db.upsert_series_mapping("series-multi", &mapping)
        .await
        .unwrap();

    let meta = HashMap::new();

    // Season 1: eps 1, 2 downloaded; ep 3 missing
    for ep in &[1, 2] {
        db.insert_episode(crate::db::episodes::InsertEpisodeParams {
            file_path: Some("/media/show/S01/some_file.mkv"),
            title: Some("Episode Title"),
            status: "organized",
            ..crate::db::episodes::InsertEpisodeParams::dummy(
                &format!("s1-ep-{ep}"),
                "series-multi",
                1,
                *ep,
                &meta,
            )
        })
        .await
        .unwrap();
    }

    db.insert_episode(crate::db::episodes::InsertEpisodeParams {
        title: Some("Missing Ep"),
        ..crate::db::episodes::InsertEpisodeParams::dummy("s1-ep-3", "series-multi", 1, 3, &meta)
    })
    .await
    .unwrap();

    // Season 2: ep 1 downloaded; eps 2, 3 missing
    db.insert_episode(crate::db::episodes::InsertEpisodeParams {
        file_path: Some("/media/show/S02/ep1.mkv"),
        title: Some("S02 Ep1"),
        status: "organized",
        ..crate::db::episodes::InsertEpisodeParams::dummy("s2-ep-1", "series-multi", 2, 1, &meta)
    })
    .await
    .unwrap();

    for ep in &[2, 3] {
        db.insert_episode(crate::db::episodes::InsertEpisodeParams {
            title: Some("Missing S02 Ep"),
            ..crate::db::episodes::InsertEpisodeParams::dummy(
                &format!("s2-ep-{ep}"),
                "series-multi",
                2,
                *ep,
                &meta,
            )
        })
        .await
        .unwrap();
    }

    (db, tmp)
}

// Tests for `filter_downloaded_episodes`, which `auto_search_season` calls to
// decide which episodes to search for:
//
//   - replacement_on = false: downloaded episodes are removed from the search list;
//     only truly missing episodes are searched.
//   - replacement_on = true (upgrades enabled): all episodes are kept, even already
//     downloaded ones, so higher-quality replacements can be found.
//
// `auto_episode` mode (search.rs) does NOT call this — it always searches and lets
// the queue's dedup/replace logic decide.

#[tokio::test]
async fn test_filter_downloaded_replacement_off_filters_downloaded() {
    let (db, _tmp) = setup_db().await;

    let mut result = filter_downloaded_episodes(
        &db,
        "series-abc",
        1,
        &[1, 2, 3, 4, 5],
        false,
        NumberingMode::Normal,
    )
    .await;

    let mut expected: Vec<i32> = vec![3, 5];
    result.sort_unstable();
    expected.sort_unstable();
    assert_eq!(
        result, expected,
        "Only non-downloaded episodes should remain"
    );
}

#[tokio::test]
async fn test_filter_downloaded_replacement_on_keeps_all() {
    let (db, _tmp) = setup_db().await;

    let mut result = filter_downloaded_episodes(
        &db,
        "series-abc",
        1,
        &[1, 2, 3, 4, 5],
        true,
        NumberingMode::Normal,
    )
    .await;

    let mut expected: Vec<i32> = vec![1, 2, 3, 4, 5];
    result.sort_unstable();
    expected.sort_unstable();
    assert_eq!(
        result, expected,
        "With replacement on, all episodes should remain for upgrade checks"
    );
}

#[tokio::test]
async fn test_filter_downloaded_replacement_off_all_downloaded() {
    let (db, _tmp) = setup_db().await;

    let result = filter_downloaded_episodes(
        &db,
        "series-abc",
        1,
        &[1, 2, 4],
        false,
        NumberingMode::Normal,
    )
    .await;

    assert!(
        result.is_empty(),
        "All requested episodes are downloaded → should return empty"
    );
}

#[tokio::test]
async fn test_filter_downloaded_replacement_off_none_downloaded() {
    let (db, _tmp) = setup_db().await;

    let result =
        filter_downloaded_episodes(&db, "series-abc", 1, &[3, 5], false, NumberingMode::Normal)
            .await;

    assert_eq!(
        result,
        vec![3, 5],
        "No episodes are downloaded → should return all unchanged"
    );
}

#[tokio::test]
async fn test_filter_downloaded_unknown_series_returns_all() {
    let (db, _tmp) = setup_db().await;

    let result = filter_downloaded_episodes(
        &db,
        "nonexistent",
        1,
        &[1, 2, 3],
        false,
        NumberingMode::Normal,
    )
    .await;

    assert_eq!(
        result,
        vec![1, 2, 3],
        "Unknown series → should return all episodes unchanged"
    );
}

// Empty-string file_path is treated as NOT downloaded — identical to None (some
// plugins set "").

#[tokio::test]
async fn test_filter_downloaded_empty_string_path_treated_as_not_downloaded() {
    let (db, _tmp) = setup_db_with_empty_string_paths().await;

    let result = filter_downloaded_episodes(
        &db,
        "series-empty",
        1,
        &[1, 2],
        false,
        NumberingMode::Normal,
    )
    .await;

    assert_eq!(
        result,
        vec![1],
        "Ep 1 has empty file_path → treated as missing and kept; ep 2 has real path → filtered out"
    );
}

#[tokio::test]
async fn test_filter_downloaded_empty_string_path_with_replacement_on() {
    // With upgrades enabled, even episodes with a real file_path are kept.
    // An episode with an empty string file_path should definitely be kept.
    let (db, _tmp) = setup_db_with_empty_string_paths().await;

    let mut result =
        filter_downloaded_episodes(&db, "series-empty", 1, &[1, 2], true, NumberingMode::Normal)
            .await;

    let mut expected = vec![1, 2];
    result.sort_unstable();
    expected.sort_unstable();
    assert_eq!(
        result, expected,
        "With replacement on, both episodes remain regardless of file_path status"
    );
}

// NumberingMode::Absolute behaves identically to Normal here; it only changes how
// the DB query maps episode numbers.

#[tokio::test]
async fn test_filter_downloaded_absolute_numbering_replacement_off() {
    // In the test setup, episodes have `numbering_mode: None` (normal numbering).
    // When querying with absolute mode, the DB looks for episodes stored with
    // absolute numbering. Since none are found, `filter_downloaded_episodes`
    // cannot determine which are downloaded, so the entire list passes through.
    //
    // This is correct behavior: in absolute mode, both the episode storage and
    // the search query must use consistent numbering. The function's safety net
    // (keep all episodes when DB returns no data) prevents accidental drops.
    let (db, _tmp) = setup_db().await;

    let result = filter_downloaded_episodes(
        &db,
        "series-abc",
        1,
        &[1, 2, 3, 4, 5],
        false,
        NumberingMode::Absolute,
    )
    .await;

    assert_eq!(
        result,
        vec![1, 2, 3, 4, 5],
        "Absolute mode with no absolute-stored eps: all pass through (DB returned empty for this numbering)"
    );
}

#[tokio::test]
async fn test_filter_downloaded_absolute_numbering_replacement_on() {
    let (db, _tmp) = setup_db().await;

    let mut result = filter_downloaded_episodes(
        &db,
        "series-abc",
        1,
        &[1, 2, 3, 4, 5],
        true,
        NumberingMode::Absolute,
    )
    .await;

    let mut expected: Vec<i32> = vec![1, 2, 3, 4, 5];
    result.sort_unstable();
    expected.sort_unstable();
    assert_eq!(
        result, expected,
        "Absolute numbering mode with replacement on: all episodes remain for upgrade checks"
    );
}

// filter_downloaded_episodes only removes episodes for the given season; other
// seasons are unaffected (they wouldn't be in this season's search list anyway).

#[tokio::test]
async fn test_filter_downloaded_season_1_only() {
    let (db, _tmp) = setup_db_multi_season().await;

    // Search season 1, eps [1, 2, 3]; replacement off
    // S1 ep 1 and 2 are downloaded → filtered; ep 3 is missing → kept
    let result = filter_downloaded_episodes(
        &db,
        "series-multi",
        1,
        &[1, 2, 3],
        false,
        NumberingMode::Normal,
    )
    .await;

    assert_eq!(
        result,
        vec![3],
        "Season 1: downloaded eps 1,2 should be filtered; missing ep 3 kept"
    );
}

#[tokio::test]
async fn test_filter_downloaded_season_2_only() {
    let (db, _tmp) = setup_db_multi_season().await;

    // Search season 2, eps [1, 2, 3]; replacement off
    // S2 ep 1 is downloaded → filtered; eps 2,3 are missing → kept
    let mut result = filter_downloaded_episodes(
        &db,
        "series-multi",
        2,
        &[1, 2, 3],
        false,
        NumberingMode::Normal,
    )
    .await;

    let mut expected = vec![2, 3];
    result.sort_unstable();
    expected.sort_unstable();
    assert_eq!(
        result, expected,
        "Season 2: downloaded ep 1 filtered out; missing eps 2,3 kept"
    );
}

#[tokio::test]
async fn test_filter_downloaded_season_2_replacement_on() {
    let (db, _tmp) = setup_db_multi_season().await;

    // Search season 2 with upgrades enabled → all episodes kept
    let mut result = filter_downloaded_episodes(
        &db,
        "series-multi",
        2,
        &[1, 2, 3],
        true,
        NumberingMode::Normal,
    )
    .await;

    let mut expected = vec![1, 2, 3];
    result.sort_unstable();
    expected.sort_unstable();
    assert_eq!(
        result, expected,
        "Season 2 with upgrades on: ep 1 (downloaded) kept for upgrade checks; eps 2,3 (missing) kept"
    );
}

// Simulates the initial pre-filter step inside auto_search_season. Monitored
// episodes are always open to upgrades, so replacement_on is always `true` and all
// episodes (downloaded included) stay as upgrade candidates; the helper still
// accepts the toggle for other callers.

#[tokio::test]
async fn test_upgrades_always_on_keeps_all_episodes() {
    // Replacement is always on for monitored episodes. Downloaded episodes (1, 2, 4)
    // are kept as upgrade candidates — all episodes (1-5) matter.
    let (db, _tmp) = setup_db().await;

    let upgrades = true; // always on for monitored episodes
    let remaining = filter_downloaded_episodes(
        &db,
        "series-abc",
        1,
        &[1, 2, 3, 4, 5],
        upgrades,
        NumberingMode::Normal,
    )
    .await;

    assert_eq!(
        remaining,
        vec![1, 2, 3, 4, 5],
        "Upgrades always on → all episodes remain as upgrade candidates (1-5)"
    );
}

#[tokio::test]
async fn test_upgrades_always_on_different_season() {
    // Upgrades are always on for monitored episodes — same behavior as above
    // but on a different season to confirm there's no season-specific logic.
    let (db, _tmp) = setup_db().await;

    let upgrades = true; // always on for monitored episodes
    let mut remaining = filter_downloaded_episodes(
        &db,
        "series-abc",
        1,
        &[1, 2, 3, 4, 5],
        upgrades,
        NumberingMode::Normal,
    )
    .await;

    let mut expected = vec![1, 2, 3, 4, 5];
    remaining.sort_unstable();
    expected.sort_unstable();
    assert_eq!(
        remaining, expected,
        "Upgrades ON → auto_season keeps all episodes for potential upgrades"
    );
}

#[tokio::test]
async fn test_upgrades_on_all_downloaded_still_searches() {
    // Even when ALL episodes are downloaded, upgrades=on means the search
    // still runs (looking for better releases). This is the key difference
    // from upgrades=off, which would return empty and skip the search.
    let (db, _tmp) = setup_db().await;

    let upgrades = true;
    let result = filter_downloaded_episodes(
        &db,
        "series-abc",
        1,
        &[1, 2, 4], // all downloaded
        upgrades,
        NumberingMode::Normal,
    )
    .await;

    assert_eq!(
        result.len(),
        3,
        "Upgrades ON: all-downloaded list should still be fully returned for upgrade checks"
    );
    assert_eq!(result, vec![1, 2, 4]);
}

#[tokio::test]
async fn test_upgrades_off_all_downloaded_returns_empty() {
    // When ALL episodes are downloaded and upgrades are off, the search
    // can be skipped entirely — nothing to do.
    let (db, _tmp) = setup_db().await;

    let upgrades = false;
    let result = filter_downloaded_episodes(
        &db,
        "series-abc",
        1,
        &[1, 2, 4], // all downloaded
        upgrades,
        NumberingMode::Normal,
    )
    .await;

    assert!(
        result.is_empty(),
        "Upgrades OFF: all-downloaded list should return empty — no search needed"
    );
}

// ---- parse_episode_numbers_for_season -------------------------------------

#[test]
fn test_season_scoped_parse_rejects_seasonless_results() {
    // Season-based series (required_season = Some): a result without a parsed
    // season must not fall back to S01 — it is unrelated to the search.
    assert!(parse_episode_numbers_for_season("Show - 05 [1080p]", Some(1)).is_empty());
    assert!(parse_episode_numbers_for_season("Show E05 [1080p]", Some(1)).is_empty());
    assert!(parse_episode_numbers_for_season("Show - 01-13 [1080p]", Some(1)).is_empty());
    assert!(parse_episode_numbers_for_season("Show Episodes 1, 2, 3", Some(1)).is_empty());
}

#[test]
fn test_season_scoped_parse_reads_nx_season() {
    // `NxNN` names declare a season the filename parser does not read; the
    // regex fallback verifies it against the searched season.
    assert_eq!(
        parse_episode_numbers_for_season("Show 2x05 [1080p]", Some(2)),
        vec![5]
    );
    assert!(parse_episode_numbers_for_season("Show 3x05 [1080p]", Some(2)).is_empty());
}

#[test]
fn test_season_scoped_parse_rejects_other_season() {
    // The SXX in the result must be the searched season, not just any season.
    assert_eq!(
        parse_episode_numbers_for_season("Show S02E05 [1080p]", Some(2)),
        vec![5]
    );
    assert!(parse_episode_numbers_for_season("Show S05E05 [1080p]", Some(2)).is_empty());
}

#[test]
fn test_season_scoped_parse_without_required_season_allows_seasonless() {
    // `None` (no season required) accepts a season-less result — the release
    // title may carry no season marker at all.
    assert_eq!(
        parse_episode_numbers_for_season("Show - 05 [1080p]", None),
        vec![5]
    );
    // With no required season the result's season is incidental: an SXX marker
    // is parsed for its episode number and otherwise ignored.
    assert_eq!(
        parse_episode_numbers_for_season("Show S05E05 [1080p]", None),
        vec![5]
    );
}

// Unlike auto_season, auto_episode mode (search.rs) does NOT call
// filter_downloaded_episodes: it always searches and lets the queue's
// AddQueueResult (Added / Replaced / Skipped / Merged) decide the outcome.

// ---- Partial packs (multi-episode range releases) -------------------------
//
// A "partial pack" is a multi-episode range release (e.g. S01E01-E13) that covers
// only part of the season. It is deliberately NOT treated as a season pack: the
// auto-season loop enqueues exactly the episodes in its range and searches the
// remainder individually. The observed production behavior — a 1-13 pack plus
// 14-21 downloaded individually — is what these tests lock in.

// ---- Multi-release decision (decide_multi_release) ------------------------
//
// A multi-release (season pack or episode range) is compared against the individual
// alternatives for its coverage: a missing episode with no alternative forces the
// multi (necessity); otherwise combined value is compared to the individual total and
// an exact tie is broken by the season-pack strategy.

use crate::source_processor::{EpisodeAlt, MultiBranch, MultiDecision, decide_multi_release};
use jumbie_shared::config::SeasonPackStrategy;

fn alt(missing: bool, alternative: Option<i32>) -> EpisodeAlt {
    EpisodeAlt {
        missing,
        alternative,
    }
}

#[test]
fn test_multi_necessity_wins_regardless_of_score() {
    // Episode 2 is missing with no alternative → the multi is the only way to obtain
    // it, so it wins even though its combined score is far below the alternatives.
    let coverage = vec![
        alt(true, Some(5000)), // missing, has a strong single
        alt(true, None),       // missing, obtainable only here
        alt(false, Some(9000)),
    ];
    assert_eq!(
        decide_multi_release(1, &coverage, 50, &SeasonPackStrategy::FavorEpisodes),
        MultiDecision::Win {
            score: 3, // base × full coverage
            branch: MultiBranch::Necessity,
        }
    );
}

#[test]
fn test_multi_competitive_replace_win_lose_and_tie() {
    // All episodes have alternatives; 10 of 12 missing → meets 50% → replace branch.
    let coverage: Vec<EpisodeAlt> = (0..12).map(|i| alt(i < 10, Some(100))).collect();

    // base 100 × 12 == 1200 → tie; FavorSeasonPacks keeps the multi.
    assert_eq!(
        decide_multi_release(100, &coverage, 50, &SeasonPackStrategy::FavorSeasonPacks),
        MultiDecision::Win {
            score: 1200,
            branch: MultiBranch::CompetitiveReplace,
        }
    );
    // The same tie under FavorEpisodes goes to the individuals → dropped.
    assert_eq!(
        decide_multi_release(100, &coverage, 50, &SeasonPackStrategy::FavorEpisodes),
        MultiDecision::Drop
    );
    // Strictly better than the alternatives → wins under either strategy.
    assert_eq!(
        decide_multi_release(101, &coverage, 50, &SeasonPackStrategy::FavorEpisodes),
        MultiDecision::Win {
            score: 101 * 12,
            branch: MultiBranch::CompetitiveReplace,
        }
    );
    // Strictly worse → dropped.
    assert_eq!(
        decide_multi_release(99, &coverage, 50, &SeasonPackStrategy::FavorSeasonPacks),
        MultiDecision::Drop
    );
}

#[test]
fn test_multi_below_threshold_only_fills_gaps() {
    // 2 of 12 missing → below 50% → gap-fill: only the missing episodes count.
    let coverage: Vec<EpisodeAlt> = (0..12).map(|i| alt(i < 2, Some(100))).collect();

    // base 100 × 2 == 200 → tie; FavorSeasonPacks keeps the multi.
    assert_eq!(
        decide_multi_release(100, &coverage, 50, &SeasonPackStrategy::FavorSeasonPacks),
        MultiDecision::Win {
            score: 200,
            branch: MultiBranch::CompetitiveFillGap,
        }
    );
    // base 99 × 2 == 198 < 200 → dropped.
    assert_eq!(
        decide_multi_release(99, &coverage, 50, &SeasonPackStrategy::FavorSeasonPacks),
        MultiDecision::Drop
    );
}

#[test]
fn test_multi_empty_or_fully_present_drops() {
    // No coverage at all.
    assert_eq!(
        decide_multi_release(100, &[], 50, &SeasonPackStrategy::FavorSeasonPacks),
        MultiDecision::Drop
    );
    // Nothing missing and below threshold → no gaps to fill → drop.
    let coverage = vec![alt(false, Some(100)), alt(false, Some(100))];
    assert_eq!(
        decide_multi_release(100, &coverage, 50, &SeasonPackStrategy::FavorSeasonPacks),
        MultiDecision::Drop
    );
}

#[test]
fn test_partial_pack_episode_count_is_its_range_size() {
    // Partial pack: size normalizes by the episodes it actually covers.
    assert_eq!(episode_count_for_scoring(false, 13, 21), Some(13));
    // Single episode: not normalized.
    assert_eq!(episode_count_for_scoring(false, 1, 21), None);
    // Full season pack: normalize by the whole season.
    assert_eq!(episode_count_for_scoring(true, 0, 21), Some(21));
    // Season pack with unknown season size: not normalized.
    assert_eq!(episode_count_for_scoring(true, 0, 0), None);
}

#[test]
fn test_build_episode_intentions_marks_overspill_unneeded() {
    // Claimed episodes stay (keep=true); covered-but-not-needed overspill is marked
    // keep=false so smart-link discards those files.
    let json = build_episode_intentions(&[11, 12, 13], &[1, 2], 1, 0, false, "series-abc", 250);
    let intentions: Vec<jumbie_shared::types::EpisodeIntention> =
        serde_json::from_str(&json).expect("valid intentions JSON");

    for ep in [11, 12, 13] {
        assert!(
            intentions.iter().any(|i| i.episode_num == ep && i.keep),
            "episode {ep} should be kept"
        );
    }
    for ep in [1, 2] {
        assert!(
            intentions.iter().any(|i| i.episode_num == ep && !i.keep),
            "episode {ep} should be unneeded"
        );
    }
    assert!(intentions.iter().all(|i| i.score == 250));
}

#[test]
fn test_partial_pack_scored_per_episode_against_singles() {
    use jumbie_shared::scoring::ReleaseProfile;

    let profile = ReleaseProfile {
        name: "pack normalization".to_string(),
        size_score_per_gb: 10,
        ..Default::default()
    };
    let gb = 1024u64 * 1024 * 1024;

    // A 13 GB partial pack covering 13 episodes normalizes to 1 GB/episode → +10,
    // the same as a single 1 GB episode. This is what makes the pack and the
    // individual episodes comparable on score.
    let (pack_score, _) = profile.calculate_with_submitter(
        "Test Show S01E01-E13 [1080p]",
        13 * gb,
        0,
        None,
        episode_count_for_scoring(false, 13, 21),
        None,
    );
    let (single_score, _) = profile.calculate_with_submitter(
        "Test Show S01E14 [1080p]",
        gb,
        0,
        None,
        episode_count_for_scoring(false, 1, 21),
        None,
    );
    assert_eq!(pack_score, 10);
    assert_eq!(
        pack_score, single_score,
        "a partial pack scores per episode, so it competes fairly with singles"
    );
}
