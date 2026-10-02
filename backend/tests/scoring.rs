// calculate_pack_score integration tests: the async, DB-dependent scoring path.
// Unit-test-only paths (pack_penalty, assess_pack_candidacy) live in
// source_processor/tests/mod.rs.

use std::collections::HashSet;
use std::sync::Arc;

use jumbie::db::DbManager;
use jumbie::organizer::ContentOrganizer;
use jumbie::plugins::PluginManager;
use jumbie_shared::mapping::{MappingRule, SeriesSettings};
use jumbie_shared::types::EpisodeInfo;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

const TEST_SERIES_ID: &str = "test-series-001";

/// Build a ContentOrganizer + DB with a fresh test database.
async fn setup() -> (ContentOrganizer, Arc<DbManager>, tempfile::TempDir) {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let db_path = temp_dir.path().join("test.db");

    jumbie::plugins::internal::register_all().await;

    let plugin_manager = Arc::new(RwLock::new(PluginManager::new(
        temp_dir.path().join("plugins"),
    )));

    let db = Arc::new(DbManager::new(&db_path).await.expect("DB init"));

    let modifying_series: Arc<RwLock<HashSet<String>>> = Arc::new(RwLock::new(HashSet::new()));
    let organizer = ContentOrganizer::new(
        db_path.to_str().unwrap(),
        db.clone(),
        plugin_manager,
        CancellationToken::new(),
        modifying_series,
    )
    .await
    .expect("ContentOrganizer init");

    (organizer, db, temp_dir)
}

/// Create a basic MappingRule with the test series_id.
fn make_mapping() -> MappingRule {
    MappingRule {
        series_id: TEST_SERIES_ID.to_string(),
        target_title: "Test Series".to_string(),
        name: "Test Series".to_string(),
        settings: SeriesSettings {
            season: std::collections::HashMap::new(),
            ..SeriesSettings::default()
        },
        ..MappingRule::default()
    }
}

/// Insert monitored episode rows for a range of episodes in a season.
async fn insert_monitored_episodes(
    db: &DbManager,
    series_id: &str,
    season: i32,
    start_ep: i32,
    end_ep: i32,
) {
    let meta_ids = std::collections::HashMap::new();
    for ep in start_ep..=end_ep {
        let s_str = format!("{:02}", season);
        let ep_id = make_episode_id(series_id, &s_str, ep);
        let _ = db
            .insert_episode(jumbie::db::episodes::crud::InsertEpisodeParams::dummy(
                &ep_id, series_id, season, ep, &meta_ids,
            ))
            .await;
        let _ = sqlx::query("UPDATE episodes SET monitored = 1 WHERE episode_id = ?")
            .bind(&ep_id)
            .execute(db.get_pool())
            .await;
    }
}

/// Build an episode_id matching the format used by MappingRule::get_episode_id.
fn make_episode_id(series_id: &str, season: &str, episode: i32) -> String {
    format!(
        "{}_S{:02}E{:02}",
        series_id,
        season.parse::<i32>().unwrap_or(1),
        episode
    )
}

// PATH A: Episode range (info.episode_end is Some, is_season_pack=false)
// E.g. S01E03-E05 — explicit start/end, not a pack.

#[tokio::test]
async fn test_cps_episode_range_all_needed() {
    let (organizer, db, _tmp) = setup().await;
    let mapping = make_mapping();
    insert_monitored_episodes(&db, TEST_SERIES_ID, 1, 1, 5).await;

    let info = EpisodeInfo {
        is_season_pack: false,
        is_complete_pack: false,
        series_key: "Test Series".to_string(),
        raw_title: "Test Series S01E03-E05".to_string(),
        file_ext: "mkv".to_string(),
        resolution: None,
        submitter: None,
        version: 1,
        part_number: None,
        seasons: vec![1],
        episodes: (3..=5).collect(),
        has_decimal_episode: false,
    };

    let (penalty, needed, unneeded) = organizer
        .calculate_pack_score(&info, &mapping, false)
        .await
        .unwrap();
    assert_eq!(penalty, 0, "Range all needed -> no penalty");
    assert_eq!(needed, vec![3, 4, 5], "Should need E03, E04, E05");
    assert_eq!(unneeded, 0);
}

// PATH B: Single episode (not a pack, no episode_end)

#[tokio::test]
async fn test_cps_single_episode_needed() {
    let (organizer, db, _tmp) = setup().await;
    let mapping = make_mapping();
    insert_monitored_episodes(&db, TEST_SERIES_ID, 1, 5, 5).await;

    let info = EpisodeInfo {
        is_season_pack: false,
        is_complete_pack: false,
        series_key: "Test Series".to_string(),
        raw_title: "Test Series S01E05".to_string(),
        file_ext: "mkv".to_string(),
        resolution: None,
        submitter: None,
        version: 1,
        part_number: None,
        seasons: vec![1],
        episodes: vec![5],
        has_decimal_episode: false,
    };

    let (penalty, needed, unneeded) = organizer
        .calculate_pack_score(&info, &mapping, false)
        .await
        .unwrap();
    assert_eq!(penalty, 0, "Single needed -> no penalty");
    assert_eq!(needed, vec![5]);
    assert_eq!(unneeded, 0);
}

#[tokio::test]
async fn test_cps_single_episode_unmonitored() {
    let (organizer, _db, _tmp) = setup().await;
    let mapping = make_mapping();

    let info = EpisodeInfo {
        is_season_pack: false,
        is_complete_pack: false,
        series_key: "Test Series".to_string(),
        raw_title: "Test Series S01E05".to_string(),
        file_ext: "mkv".to_string(),
        resolution: None,
        submitter: None,
        version: 1,
        part_number: None,
        seasons: vec![1],
        episodes: vec![5],
        has_decimal_episode: false,
    };

    let (penalty, needed, unneeded) = organizer
        .calculate_pack_score(&info, &mapping, false)
        .await
        .unwrap();
    assert_eq!(penalty, -50, "1 unneeded * 50 (non-pack)");
    assert!(needed.is_empty());
    assert_eq!(unneeded, 1);
}

// PATH D: Single-season pack (is_season_pack=true, season_end=None)

#[tokio::test]
async fn test_cps_single_season_pack_all_needed() {
    let (organizer, db, _tmp) = setup().await;
    let mapping = make_mapping();
    insert_monitored_episodes(&db, TEST_SERIES_ID, 1, 1, 12).await;

    let info = EpisodeInfo {
        is_season_pack: true,
        is_complete_pack: false,
        series_key: "Test Series".to_string(),
        raw_title: "Test Series S01 Complete".to_string(),
        file_ext: "mkv".to_string(),
        resolution: None,
        submitter: None,
        version: 1,
        part_number: None,
        seasons: vec![1],
        episodes: vec![],
        has_decimal_episode: false,
    };

    let (penalty, needed, unneeded) = organizer
        .calculate_pack_score(&info, &mapping, false)
        .await
        .unwrap();
    assert_eq!(penalty, 0, "100% needed -> threshold met -> no penalty");
    assert_eq!(needed.len(), 12);
    assert_eq!(unneeded, 0);
}

#[tokio::test]
async fn test_cps_single_season_pack_partial() {
    let (organizer, db, _tmp) = setup().await;
    let mapping = make_mapping();
    insert_monitored_episodes(&db, TEST_SERIES_ID, 1, 1, 12).await;
    for ep in 4..=12 {
        let ep_id = make_episode_id(TEST_SERIES_ID, "01", ep);
        let _ = sqlx::query("UPDATE episodes SET monitored = 0 WHERE episode_id = ?")
            .bind(&ep_id)
            .execute(db.get_pool())
            .await;
    }

    let info = EpisodeInfo {
        is_season_pack: true,
        is_complete_pack: false,
        series_key: "Test Series".to_string(),
        raw_title: "Test Series S01 Complete".to_string(),
        file_ext: "mkv".to_string(),
        resolution: None,
        submitter: None,
        version: 1,
        part_number: None,
        seasons: vec![1],
        episodes: vec![],
        has_decimal_episode: false,
    };

    let (penalty, needed, unneeded) = organizer
        .calculate_pack_score(&info, &mapping, false)
        .await
        .unwrap();
    assert_eq!(penalty, -450, "9 unneeded * 50");
    assert_eq!(needed, vec![1, 2, 3], "Only monitored episodes from S01");
    assert_eq!(unneeded, 9);
}

// PATH C1: Multi-season pack, all seasons known
// S01-S02, S01 has 10 eps, S02 has 8 eps, all monitored.

#[tokio::test]
async fn test_cps_multi_season_all_known() {
    let (organizer, db, _tmp) = setup().await;
    let mapping = make_mapping();
    insert_monitored_episodes(&db, TEST_SERIES_ID, 1, 1, 10).await;
    insert_monitored_episodes(&db, TEST_SERIES_ID, 2, 1, 8).await;

    let info = EpisodeInfo {
        is_season_pack: true,
        is_complete_pack: false,
        series_key: "Test Series".to_string(),
        raw_title: "Test Series S01-S02 Complete".to_string(),
        file_ext: "mkv".to_string(),
        resolution: None,
        submitter: None,
        version: 1,
        part_number: None,
        seasons: vec![1, 2],
        episodes: vec![],
        has_decimal_episode: false,
    };

    let (penalty, needed, unneeded) = organizer
        .calculate_pack_score(&info, &mapping, false)
        .await
        .unwrap();
    assert_eq!(penalty, 0, "18/18 needed, threshold met");
    assert_eq!(
        needed,
        (1..=10).collect::<Vec<i32>>(),
        "S01 only in needed return"
    );
    assert_eq!(unneeded, 0);
}

#[tokio::test]
async fn test_cps_multi_season_partial_monitored() {
    let (organizer, db, _tmp) = setup().await;
    let mapping = make_mapping();
    insert_monitored_episodes(&db, TEST_SERIES_ID, 1, 1, 10).await;
    insert_monitored_episodes(&db, TEST_SERIES_ID, 2, 1, 8).await;
    // Only monitor S01E01-E05 + S02E01-E03
    for ep in 6..=10 {
        let ep_id = make_episode_id(TEST_SERIES_ID, "01", ep);
        let _ = sqlx::query("UPDATE episodes SET monitored = 0 WHERE episode_id = ?")
            .bind(&ep_id)
            .execute(db.get_pool())
            .await;
    }
    for ep in 4..=8 {
        let ep_id = make_episode_id(TEST_SERIES_ID, "02", ep);
        let _ = sqlx::query("UPDATE episodes SET monitored = 0 WHERE episode_id = ?")
            .bind(&ep_id)
            .execute(db.get_pool())
            .await;
    }

    let info = EpisodeInfo {
        is_season_pack: true,
        is_complete_pack: false,
        series_key: "Test Series".to_string(),
        raw_title: "Test Series S01-S02 Complete".to_string(),
        file_ext: "mkv".to_string(),
        resolution: None,
        submitter: None,
        version: 1,
        part_number: None,
        seasons: vec![1, 2],
        episodes: vec![],
        has_decimal_episode: false,
    };

    let (penalty, needed, unneeded) = organizer
        .calculate_pack_score(&info, &mapping, false)
        .await
        .unwrap();
    assert_eq!(penalty, -500, "10 unneeded * 50");
    assert_eq!(needed, vec![1, 2, 3, 4, 5], "S01 monitored only");
    assert_eq!(unneeded, 10);
}

// PATH C2: Multi-season pack, some seasons known (average estimation)
// S01-S03, only S01 has data (10 eps). S02+S03 estimated at 10 each (avg).

#[tokio::test]
async fn test_cps_multi_season_average_estimation() {
    let (organizer, db, _tmp) = setup().await;
    let mapping = make_mapping();
    // Only S01 has data
    insert_monitored_episodes(&db, TEST_SERIES_ID, 1, 1, 10).await;

    let info = EpisodeInfo {
        is_season_pack: true,
        is_complete_pack: false,
        series_key: "Test Series".to_string(),
        raw_title: "Test Series Seasons 1-3".to_string(),
        file_ext: "mkv".to_string(),
        resolution: None,
        submitter: None,
        version: 1,
        part_number: None,
        seasons: vec![1, 2, 3],
        episodes: vec![],
        has_decimal_episode: false,
    };

    let (penalty, needed, unneeded) = organizer
        .calculate_pack_score(&info, &mapping, false)
        .await
        .unwrap();
    // S01 = 10 eps (known). S02+S03 = 10 each (estimated from avg of 10/1).
    // Total = 30. Only S01E01-E10 exist → 10 needed, 20 estimated unneeded.
    assert_eq!(penalty, -1000, "20 unneeded * 50");
    assert_eq!(needed, (1..=10).collect::<Vec<i32>>(), "S01 monitored");
    assert_eq!(unneeded, 20);
}

// PATH C3: Multi-season pack, no seasons known → falls through to single-season
// S01-S02, no episodes in DB → single-season default with 12 eps.

#[tokio::test]
async fn test_cps_multi_season_none_known() {
    let (organizer, _db, _tmp) = setup().await;
    let mapping = make_mapping();

    let info = EpisodeInfo {
        is_season_pack: true,
        is_complete_pack: false,
        series_key: "Test Series".to_string(),
        raw_title: "Test Series S01-S02 Complete".to_string(),
        file_ext: "mkv".to_string(),
        resolution: None,
        submitter: None,
        version: 1,
        part_number: None,
        seasons: vec![],
        episodes: vec![],
        has_decimal_episode: false,
    };

    let (penalty, needed, unneeded) = organizer
        .calculate_pack_score(&info, &mapping, false)
        .await
        .unwrap();
    // Fallback: no seasons, no episodes → single-season default 12, none in DB → all unneeded
    assert_eq!(penalty, -600, "12 unneeded * 50");
    assert!(needed.is_empty());
    assert_eq!(unneeded, 12);
}

// PATH C4: Non-contiguous seasons via `seasons` vec (e.g. S01, S08)
// When info.seasons is non-empty, it takes priority over season_end.

#[tokio::test]
async fn test_cps_non_contiguous_seasons() {
    let (organizer, db, _tmp) = setup().await;
    let mapping = make_mapping();
    insert_monitored_episodes(&db, TEST_SERIES_ID, 1, 1, 10).await;
    insert_monitored_episodes(&db, TEST_SERIES_ID, 8, 1, 6).await;

    let info = EpisodeInfo {
        is_season_pack: true,
        is_complete_pack: false,
        seasons: vec![1, 8],
        series_key: "Test Series".to_string(),
        raw_title: "Test Series S01, S08 Complete".to_string(),
        file_ext: "mkv".to_string(),
        resolution: None,
        submitter: None,
        version: 1,
        part_number: None,
        episodes: vec![],
        has_decimal_episode: false,
    };

    let (penalty, needed, unneeded) = organizer
        .calculate_pack_score(&info, &mapping, false)
        .await
        .unwrap();
    // 10 (S01) + 6 (S08) = 16 total, all monitored → 16/16 needed, threshold met
    assert_eq!(penalty, 0, "16/16 needed, threshold met");
    assert_eq!(needed.len(), 10, "S01 episodes only in needed return");
    assert_eq!(unneeded, 0);
}

#[tokio::test]
async fn test_cps_non_contiguous_seasons_average_estimation() {
    let (organizer, db, _tmp) = setup().await;
    let mapping = make_mapping();
    // Only S01 has data (10 eps)
    insert_monitored_episodes(&db, TEST_SERIES_ID, 1, 1, 10).await;

    let info = EpisodeInfo {
        is_season_pack: true,
        is_complete_pack: false,
        seasons: vec![1, 8],
        series_key: "Test Series".to_string(),
        raw_title: "Test Series S01, S08".to_string(),
        file_ext: "mkv".to_string(),
        resolution: None,
        submitter: None,
        version: 1,
        part_number: None,
        episodes: vec![],
        has_decimal_episode: false,
    };

    let (penalty, needed, unneeded) = organizer
        .calculate_pack_score(&info, &mapping, false)
        .await
        .unwrap();
    // S01 = 10 eps (known, all monitored). S08 estimated at 10 (avg of 10/1).
    // Total = 20 episode_ids. Needed = 10 (S01's 10). Unneeded = 10 (S08's 10, not in DB).
    // 10/20 = 50% needed. Default threshold = 50% → threshold IS met → replacement triggered.
    // When replacement triggers, ALL episodes are marked needed and unneeded=0.
    assert_eq!(penalty, 0, "50% meets 50% threshold -> replacement");
    assert_eq!(needed.len(), 10, "S01 episodes only");
    assert_eq!(unneeded, 0, "replaced -> unneeded=0");
}
