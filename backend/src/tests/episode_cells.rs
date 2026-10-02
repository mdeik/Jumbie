// Tests for ensure_episode_cells, mode isolation, pruning, and provider upgrades,
// run against a real migrated SQLite DB. Each test seeds a series mapping + episodes
// and asserts the resulting episode rows.

use crate::db::DbManager;
use jumbie_shared::types::{MappingRule, SeasonOverride, SeriesSettings};
use std::collections::HashMap;

/// Create a temp DB with a series mapping seeded.
async fn setup_db() -> (DbManager, tempfile::TempDir, String) {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test.db");
    let db = DbManager::new(&db_path).await.unwrap();

    let mapping = MappingRule {
        target_title: "Test Series".to_string(),
        series_id: "ts-001".to_string(),
        name: "test_series".to_string(),
        ..Default::default()
    };
    db.upsert_series_mapping("series-key", &mapping)
        .await
        .unwrap();

    (db, tmp, "series-key".to_string())
}

/// Insert 5 test episodes with "organized" status for the given series.
async fn seed_episodes(db: &DbManager, series_id: &str) {
    for ep in 1..=5 {
        let eid = format!("{series_id}_S01E{ep:02}");
        let meta_ids = HashMap::new();

        db.insert_episode(crate::db::episodes::crud::InsertEpisodeParams {
            file_path: Some("/media/file.mkv"),

            status: "organized",

            ..crate::db::episodes::crud::InsertEpisodeParams::dummy(
                &eid, series_id, 1, ep, &meta_ids,
            )
        })
        .await
        .unwrap();
    }
}

/// Fetch episode rows (normal mode) through the view layer.
/// Returns (episode_id, season, episode, numbering_mode, monitored, status).
/// Does NOT expose episode_cell_type — use raw SQL queries for that.
async fn fetch_episodes(
    db: &DbManager,
    series_id: &str,
) -> Vec<(String, i32, i32, i32, bool, String)> {
    let rows = db
        .get_series_episodes_details(series_id, false)
        .await
        .unwrap();
    rows.into_iter()
        .map(|r| {
            (
                r.episode_id,
                r.season.unwrap_or_default(),
                r.episode,
                r.numbering_mode,
                r.monitored,
                r.status.unwrap_or_default(),
            )
        })
        .collect()
}

/// Get the connection pool from DbManager for raw SQL queries.
fn pool(db: &DbManager) -> &sqlx::SqlitePool {
    db.get_pool()
}

/// Fetch all episodes for ALL modes (no filter) using a raw query.
/// Returns (episode_id, season, episode, numbering_mode).
async fn fetch_all_episodes(db: &DbManager, series_id: &str) -> Vec<(String, i32, i32, i32)> {
    sqlx::query_as::<_, (String, i32, i32, i32)>(
        "SELECT episode_id, COALESCE(season, 1), episode, numbering_mode \
         FROM episodes WHERE series_id = ?",
    )
    .bind(series_id)
    .fetch_all(pool(db))
    .await
    .unwrap()
}

/// Build a MappingRule with a single season override.
/// When `absolute` is true, the override is placed in `season_absolute`.
/// When `absolute` is false, the override is placed in `season`.
fn make_mapping_with_season(
    series_id: &str,
    season: &str,
    cell_count: Option<i32>,
    absolute: bool,
) -> MappingRule {
    let mut overrides = HashMap::new();
    let so = SeasonOverride {
        season: season.to_string(),
        cell_count,
        episode_start: None,
        episode_end: None,
        episode_offset: None,
        alias_season_number: None,
        search_format: None,
        aliases: vec![],
        reg_patterns: vec![],
    };
    overrides.insert(season.to_string(), so);

    let (season_map, season_absolute_map) = if absolute {
        (HashMap::new(), overrides)
    } else {
        (overrides, HashMap::new())
    };

    MappingRule {
        target_title: "Test Series".to_string(),
        series_id: series_id.to_string(),
        settings: SeriesSettings {
            season: season_map,
            season_absolute: season_absolute_map,
            absolute_numbering: Some(absolute),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// Build a MappingRule whose single season override defines an explicit episode
/// range (`episode_start`..=`episode_end`, no `cell_count`). This is the standard
/// per-season range mechanism that replaced the removed series-level range.
fn make_mapping_with_range(
    series_id: &str,
    season: &str,
    start: Option<i32>,
    end: Option<i32>,
) -> MappingRule {
    let mut overrides = HashMap::new();
    overrides.insert(
        season.to_string(),
        SeasonOverride {
            season: season.to_string(),
            episode_start: start,
            episode_end: end,
            cell_count: None,
            episode_offset: None,
            alias_season_number: None,
            search_format: None,
            aliases: vec![],
            reg_patterns: vec![],
        },
    );
    MappingRule {
        target_title: "Test Series".to_string(),
        series_id: series_id.to_string(),
        settings: SeriesSettings {
            season: overrides,
            ..Default::default()
        },
        ..Default::default()
    }
}

#[tokio::test]
async fn test_create_cells_empty_series() {
    let (db, _tmp, _key) = setup_db().await;
    let mapping = make_mapping_with_season("ts-001", "01", Some(5), false);
    let normalized = vec!["01".to_string()];

    db.ensure_episode_cells(&mapping, &normalized, false)
        .await
        .unwrap();

    let episodes = fetch_episodes(&db, "ts-001").await;
    assert_eq!(episodes.len(), 5, "Should create 5 cells");

    for (i, (eid, season, ep, mode, monitored, status)) in episodes.iter().enumerate() {
        let ep_num = i + 1;
        assert_eq!(*eid, format!("ts-001_S01E{ep_num:02}"));
        assert_eq!(*season, 1);
        assert_eq!(*ep, ep_num as i32);
        assert_eq!(*mode, 0, "Normal mode");
        assert!(*monitored, "In-range cells should be monitored");
        assert_eq!(status, "unreleased");
    }
}

#[tokio::test]
async fn test_create_cells_non_numeric_season_is_skipped() {
    // A season label that isn't a number has no cell identity: building IDs for it
    // would coerce the label into some other season and write rows under the wrong
    // season. `ensure_episode_cells` therefore skips it and writes nothing.
    let (db, _tmp, _key) = setup_db().await;
    let mapping = make_mapping_with_season("ts-001", "SP", Some(2), false);
    let normalized = vec!["SP".to_string()];

    db.ensure_episode_cells(&mapping, &normalized, false)
        .await
        .unwrap();

    let all = fetch_all_episodes(&db, "ts-001").await;
    assert!(
        all.is_empty(),
        "a non-numeric season must not create cells, got {all:?}"
    );
}

#[tokio::test]
async fn test_create_cells_absolute_mode() {
    let (db, _tmp, _key) = setup_db().await;
    let mapping = make_mapping_with_season("ts-001", "01", Some(3), true);
    let normalized = vec!["01".to_string()];

    db.ensure_episode_cells(&mapping, &normalized, false)
        .await
        .unwrap();

    // Use fetch_all_episodes (no mode filter) since fetch_episodes filters by normal mode
    let all = fetch_all_episodes(&db, "ts-001").await;
    assert_eq!(all.len(), 3, "Should create 3 cells in absolute mode");

    for (i, (eid, season, _ep, mode)) in all.iter().enumerate() {
        let ep_num = i + 1;
        assert_eq!(*eid, format!("ts-001_ABS{ep_num:04}"));
        assert_eq!(*mode, 1, "Absolute mode");
        // The ID carries no season, but the absolute space is canonically season
        // 1 (see ABSOLUTE_SEASON_NUM) — the stored column must agree.
        assert_eq!(*season, jumbie_shared::mapping::ABSOLUTE_SEASON_NUM);
    }
}

#[tokio::test]
async fn test_create_cells_skips_existing_metadata() {
    let (db, _tmp, _key) = setup_db().await;

    let meta_ids = std::collections::HashMap::new();
    for ep in &[1i32, 2, 3] {
        let eid = format!("ts-001_S01E{ep:02}");
        db.insert_episode(crate::db::episodes::crud::InsertEpisodeParams {
            status: "unreleased",
            ..crate::db::episodes::crud::InsertEpisodeParams::dummy(
                &eid, "ts-001", 1, *ep, &meta_ids,
            )
        })
        .await
        .unwrap();
    }

    // cell_count=5 — only cells 4 and 5 should be created.
    let mapping = make_mapping_with_season("ts-001", "01", Some(5), false);
    let normalized = vec!["01".to_string()];
    db.ensure_episode_cells(&mapping, &normalized, false)
        .await
        .unwrap();

    let episodes = fetch_episodes(&db, "ts-001").await;
    assert_eq!(episodes.len(), 5, "3 existing + 2 new cells");

    // New cells are UserDefined; metadata-created cells keep a NULL cell type.
    let cell_4_type: Option<i32> = sqlx::query_scalar(
        "SELECT episode_cell_type FROM episodes WHERE series_id = ? AND episode = 4",
    )
    .bind("ts-001")
    .fetch_one(pool(&db))
    .await
    .unwrap();
    assert_eq!(cell_4_type, Some(0), "New cell should be UserDefined");

    // Cell 1 was created by metadata, not the cell system, so its type stays NULL.
    let cell_1_type: Option<i32> = sqlx::query_scalar(
        "SELECT episode_cell_type FROM episodes WHERE series_id = ? AND episode = 1",
    )
    .bind("ts-001")
    .fetch_one(pool(&db))
    .await
    .unwrap();
    assert_eq!(
        cell_1_type, None,
        "Existing metadata should have NULL cell type"
    );
}

#[tokio::test]
async fn test_prune_on_reduce() {
    let (db, _tmp, _key) = setup_db().await;

    let mapping = make_mapping_with_season("ts-001", "01", Some(5), false);
    let normalized = vec!["01".to_string()];
    db.ensure_episode_cells(&mapping, &normalized, false)
        .await
        .unwrap();
    assert_eq!(
        fetch_episodes(&db, "ts-001").await.len(),
        5,
        "5 cells initially"
    );

    let mapping2 = make_mapping_with_season("ts-001", "01", Some(3), false);
    db.ensure_episode_cells(&mapping2, &normalized, false)
        .await
        .unwrap();

    let episodes = fetch_episodes(&db, "ts-001").await;
    assert_eq!(episodes.len(), 3, "Should prune to 3 cells");
    assert!(
        episodes.iter().all(|(_, _, ep, _, _, _)| *ep <= 3),
        "Only episodes 1-3 should remain"
    );
}

#[tokio::test]
async fn test_prune_on_remove() {
    let (db, _tmp, _key) = setup_db().await;

    let mapping = make_mapping_with_season("ts-001", "01", Some(5), false);
    let normalized = vec!["01".to_string()];
    db.ensure_episode_cells(&mapping, &normalized, false)
        .await
        .unwrap();
    assert_eq!(
        fetch_episodes(&db, "ts-001").await.len(),
        5,
        "5 cells initially"
    );

    // Removing cell_count calls ensure with has_cells=false.
    let mapping2 = MappingRule {
        target_title: "Test Series".to_string(),
        series_id: "ts-001".to_string(),
        ..Default::default()
    };
    db.ensure_episode_cells(&mapping2, &normalized, false)
        .await
        .unwrap();

    let episodes = fetch_episodes(&db, "ts-001").await;
    assert_eq!(episodes.len(), 0, "All UserDefined cells should be pruned");
}

#[tokio::test]
async fn test_mode_isolation_normal_to_absolute() {
    let (db, _tmp, _key) = setup_db().await;

    let norm = make_mapping_with_season("ts-001", "01", Some(3), false);
    let normalized = vec!["01".to_string()];
    db.ensure_episode_cells(&norm, &normalized, false)
        .await
        .unwrap();

    let abs = make_mapping_with_season("ts-001", "01", Some(2), true);
    db.ensure_episode_cells(&abs, &normalized, false)
        .await
        .unwrap();

    let all = fetch_all_episodes(&db, "ts-001").await;
    assert_eq!(all.len(), 5, "3 normal + 2 absolute = 5 total rows");

    let norm_eps = fetch_episodes(&db, "ts-001").await;
    assert_eq!(norm_eps.len(), 3, "Normal query sees 3 normal cells");
    assert!(
        norm_eps.iter().all(|(_, _, _, mode, _, _)| *mode == 0),
        "All returned rows should be normal mode"
    );

    let abs_rows = db
        .get_series_episodes_details("ts-001", true)
        .await
        .unwrap();
    assert_eq!(abs_rows.len(), 2, "Absolute query sees 2 absolute cells");
    assert!(
        abs_rows.iter().all(|r| r.numbering_mode == 1),
        "All returned rows should be absolute mode"
    );
}

#[tokio::test]
async fn test_mode_switch_does_not_delete_other_mode_cells() {
    let (db, _tmp, _key) = setup_db().await;

    let norm = make_mapping_with_season("ts-001", "01", Some(3), false);
    let normalized = vec!["01".to_string()];
    db.ensure_episode_cells(&norm, &normalized, false)
        .await
        .unwrap();

    let abs = make_mapping_with_season("ts-001", "01", Some(5), true);
    db.ensure_episode_cells(&abs, &normalized, false)
        .await
        .unwrap();

    let all = fetch_all_episodes(&db, "ts-001").await;
    let norm_ids: Vec<&str> = all
        .iter()
        .filter(|(_, _, _, mode)| *mode == 0)
        .map(|(eid, _, _, _)| eid.as_str())
        .collect();
    assert_eq!(norm_ids.len(), 3, "Normal cells survived");
    assert!(
        norm_ids.iter().all(|id| id.contains("S01E")),
        "Normal cells have SXXEXX IDs"
    );
}

#[tokio::test]
async fn test_out_of_range_cells_not_monitored() {
    let (db, _tmp, _key) = setup_db().await;

    let mut overrides = HashMap::new();
    overrides.insert(
        "01".to_string(),
        SeasonOverride {
            season: "01".to_string(),
            cell_count: Some(5),
            episode_start: Some(2),
            episode_end: Some(4),
            episode_offset: None,
            alias_season_number: None,
            search_format: None,
            aliases: vec![],
            reg_patterns: vec![],
        },
    );

    let mapping = MappingRule {
        target_title: "Test Series".to_string(),
        series_id: "ts-001".to_string(),
        settings: SeriesSettings {
            season: overrides,
            ..Default::default()
        },
        ..Default::default()
    };
    let normalized = vec!["01".to_string()];

    db.ensure_episode_cells(&mapping, &normalized, false)
        .await
        .unwrap();

    let episodes = fetch_episodes(&db, "ts-001").await;

    // Ep 1: out of range (below start) → not monitored
    let ep1 = episodes
        .iter()
        .find(|(_, _, ep, _, _, _)| *ep == 1)
        .unwrap();
    assert!(!ep1.4, "Episode 1 (below start) should NOT be monitored");

    // Ep 2-4: in range → monitored
    for ep_num in 2..=4 {
        let ep = episodes
            .iter()
            .find(|(_, _, e, _, _, _)| *e == ep_num)
            .unwrap();
        assert!(ep.4, "Episode {} (in range) should be monitored", ep_num);
    }

    // Ep 5: out of range (above end) → not monitored
    let ep5 = episodes
        .iter()
        .find(|(_, _, ep, _, _, _)| *ep == 5)
        .unwrap();
    assert!(!ep5.4, "Episode 5 (above end) should NOT be monitored");
}

#[tokio::test]
async fn test_provider_upgrade_user_defined() {
    let (db, _tmp, _key) = setup_db().await;

    let mapping = make_mapping_with_season("ts-001", "01", Some(3), false);
    let normalized = vec!["01".to_string()];
    db.ensure_episode_cells(&mapping, &normalized, false)
        .await
        .unwrap();

    // Ensure cell is UserDefined via raw SQL
    let before_type: Option<i32> = sqlx::query_scalar(
        "SELECT episode_cell_type FROM episodes WHERE series_id = ? AND episode = 1",
    )
    .bind("ts-001")
    .fetch_one(pool(&db))
    .await
    .unwrap();
    assert_eq!(
        before_type,
        Some(0),
        "Cell should be UserDefined before provider fill"
    );

    let ep_meta = crate::plugins::metadata::EpisodeMetadata {
        unique_id: "tvdb-123".to_string(),
        season: 1,
        episode: 1,
        title: "Real Title".to_string(),
        description: Some("Real description".to_string()),
        runtime: Some(45),
        image_url: None,
        meta_date: None,
    };
    db.batch_insert_metadata_episodes(
        "tvdb",
        "ts-001",
        vec![(format!("ts-001_S01E{:02}", 1), ep_meta)],
        0, // normal mode
    )
    .await
    .unwrap();

    let after: Vec<(String, Option<String>, Option<i32>)> =
        sqlx::query_as::<_, (String, Option<String>, Option<i32>)>(
            "SELECT episode_id, title, episode_cell_type FROM episodes \
         WHERE series_id = ? AND episode = 1 AND numbering_mode = 0",
        )
        .bind("ts-001")
        .fetch_all(pool(&db))
        .await
        .unwrap();

    assert_eq!(after.len(), 1);
    assert_eq!(
        after[0].2,
        Some(1),
        "Cell type should be Provider (1) after metadata fill"
    );
    assert_eq!(
        after[0].1.as_deref(),
        Some("Real Title"),
        "Title should be populated by provider"
    );
}

#[tokio::test]
async fn test_scanner_sets_numbering_mode() {
    let (db, _tmp, _key) = setup_db().await;

    let _mapping = MappingRule {
        target_title: "Test Series".to_string(),
        series_id: "ts-001".to_string(),
        settings: SeriesSettings {
            absolute_numbering: Some(true),
            ..Default::default()
        },
        ..Default::default()
    };

    db.insert_episode(crate::db::episodes::InsertEpisodeParams {
        episode_id: "ts-001_ABS0001",
        series_id: "ts-001",
        season: 1,
        episode: 1,
        file_path: Some("/media/test.mkv"),
        title: Some("Episode Title"),
        quality_profile_id: None,
        status: "organized",
        meta_date: None,
        est_date: None,
        metadata_ids: &std::collections::HashMap::new(),
        description: None,
        runtime: None,
        image_url: None,
        metadata_source: None,
        numbering_mode: Some(1), // absolute mode
    })
    .await
    .unwrap();

    let rows = db
        .get_series_episodes_details("ts-001", true)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].numbering_mode, 1, "Scanner-created ABS episode");
    assert_eq!(
        rows[0].episode_id, "ts-001_ABS0001",
        "ABS format episode_id"
    );
}

#[tokio::test]
async fn test_no_cell_count_does_not_delete_metadata() {
    let (db, _tmp, _key) = setup_db().await;

    let meta_ids = std::collections::HashMap::new();
    db.insert_episode(crate::db::episodes::crud::InsertEpisodeParams {
        status: "unreleased",
        ..crate::db::episodes::crud::InsertEpisodeParams::dummy(
            "ts-001_S01E01",
            "ts-001",
            1,
            1,
            &meta_ids,
        )
    })
    .await
    .unwrap();

    let mapping = MappingRule {
        target_title: "Test Series".to_string(),
        series_id: "ts-001".to_string(),
        ..Default::default()
    };
    let normalized = vec!["01".to_string()];
    db.ensure_episode_cells(&mapping, &normalized, false)
        .await
        .unwrap();

    // A metadata row (NULL cell type, not UserDefined) must survive the prune.
    let episodes = fetch_episodes(&db, "ts-001").await;
    assert_eq!(episodes.len(), 1, "Metadata row should survive prune");
    let cell_type: Option<i32> = sqlx::query_scalar(
        "SELECT episode_cell_type FROM episodes WHERE series_id = ? AND episode = 1",
    )
    .bind("ts-001")
    .fetch_one(pool(&db))
    .await
    .unwrap();
    assert_eq!(
        cell_type, None,
        "Cell type should still be NULL for metadata rows"
    );
}

#[tokio::test]
async fn test_multiple_seasons_independent_cells() {
    let (db, _tmp, _key) = setup_db().await;

    let mut overrides = HashMap::new();
    overrides.insert(
        "01".to_string(),
        SeasonOverride {
            season: "01".to_string(),
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
        "02".to_string(),
        SeasonOverride {
            season: "02".to_string(),
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

    let mapping = MappingRule {
        target_title: "Test Series".to_string(),
        series_id: "ts-001".to_string(),
        settings: SeriesSettings {
            season: overrides,
            ..Default::default()
        },
        ..Default::default()
    };
    let normalized = vec!["01".to_string(), "02".to_string()];

    db.ensure_episode_cells(&mapping, &normalized, false)
        .await
        .unwrap();

    let episodes = fetch_episodes(&db, "ts-001").await;
    assert_eq!(episodes.len(), 5, "3 + 2 = 5 total cells");

    let s1_count = episodes.iter().filter(|(_, s, _, _, _, _)| *s == 1).count();
    let s2_count = episodes.iter().filter(|(_, s, _, _, _, _)| *s == 2).count();
    assert_eq!(s1_count, 3, "Season 1: 3 cells");
    assert_eq!(s2_count, 2, "Season 2: 2 cells");
}

#[tokio::test]
async fn test_identical_calls_are_idempotent() {
    let (db, _tmp, _key) = setup_db().await;

    let mapping = make_mapping_with_season("ts-001", "01", Some(3), false);
    let normalized = vec!["01".to_string()];

    db.ensure_episode_cells(&mapping, &normalized, false)
        .await
        .unwrap();
    db.ensure_episode_cells(&mapping, &normalized, false)
        .await
        .unwrap();

    let episodes = fetch_episodes(&db, "ts-001").await;
    assert_eq!(episodes.len(), 3, "Still 3 cells after second call");
}

// Cache entries are isolated by ordering_mode.

#[tokio::test]
async fn test_cache_mode_isolation() {
    let (db, _tmp, _key) = setup_db().await;

    let normal_entry = crate::db::metadata_cache::EpisodeMetadataForCache {
        season_number: 1,
        episode_number: 1,
        unique_id: "tvdb-normal-1".to_string(),
        title: "Normal Pilot".to_string(),
        description: None,
        runtime: None,
        image_url: None,
        meta_date: None,
    };
    db.batch_upsert_metadata_episodes_cache("meta-001", "tvdb", "", "normal", &[normal_entry])
        .await
        .unwrap();

    let absolute_entry = crate::db::metadata_cache::EpisodeMetadataForCache {
        season_number: 1,
        episode_number: 1,
        unique_id: "tvdb-abs-1".to_string(),
        title: "Absolute Pilot".to_string(),
        description: None,
        runtime: None,
        image_url: None,
        meta_date: None,
    };
    db.batch_upsert_metadata_episodes_cache("meta-001", "tvdb", "", "absolute", &[absolute_entry])
        .await
        .unwrap();

    let normal_results = db
        .get_metadata_episodes_cache("meta-001", "tvdb", "", "normal")
        .await
        .unwrap();
    assert_eq!(
        normal_results.len(),
        1,
        "Normal mode should have 1 cache entry"
    );
    assert_eq!(normal_results[0].title, "Normal Pilot");
    assert_eq!(normal_results[0].unique_id, "tvdb-normal-1");

    let absolute_results = db
        .get_metadata_episodes_cache("meta-001", "tvdb", "", "absolute")
        .await
        .unwrap();
    assert_eq!(
        absolute_results.len(),
        1,
        "Absolute mode should have 1 cache entry"
    );
    assert_eq!(absolute_results[0].title, "Absolute Pilot");
    assert_eq!(absolute_results[0].unique_id, "tvdb-abs-1");

    // Verify the two entries have different unique_ids (they're separate rows)
    assert_ne!(
        normal_results[0].unique_id, absolute_results[0].unique_id,
        "Normal and absolute cache entries must have different unique_ids"
    );
}

#[tokio::test]
async fn test_cache_season_mode_isolation() {
    let (db, _tmp, _key) = setup_db().await;

    // Insert normal-mode entries for season 1
    let norm_s1 = crate::db::metadata_cache::EpisodeMetadataForCache {
        season_number: 1,
        episode_number: 1,
        unique_id: "n-s1e1".to_string(),
        title: "Normal S1E1".to_string(),
        description: None,
        runtime: None,
        image_url: None,
        meta_date: None,
    };
    let norm_s1e2 = crate::db::metadata_cache::EpisodeMetadataForCache {
        season_number: 1,
        episode_number: 2,
        unique_id: "n-s1e2".to_string(),
        title: "Normal S1E2".to_string(),
        description: None,
        runtime: None,
        image_url: None,
        meta_date: None,
    };
    db.batch_upsert_metadata_episodes_cache(
        "meta-001",
        "tvdb",
        "",
        "normal",
        &[norm_s1, norm_s1e2],
    )
    .await
    .unwrap();

    // Insert absolute-mode entries for season 1 (different titles)
    let abs_s1 = crate::db::metadata_cache::EpisodeMetadataForCache {
        season_number: 1,
        episode_number: 1,
        unique_id: "a-s1e1".to_string(),
        title: "Absolute S1E1".to_string(),
        description: None,
        runtime: None,
        image_url: None,
        meta_date: None,
    };
    db.batch_upsert_metadata_episodes_cache("meta-001", "tvdb", "", "absolute", &[abs_s1])
        .await
        .unwrap();

    // Query cache for normal mode, season 1
    let norm_results = db
        .get_metadata_episodes_cache_for_season("meta-001", "tvdb", "", "normal", 1)
        .await
        .unwrap();
    assert_eq!(
        norm_results.len(),
        2,
        "Normal mode season 1 should have 2 entries"
    );
    assert!(norm_results.iter().all(|r| r.title.starts_with("Normal")));

    // Query cache for absolute mode, season 1
    let abs_results = db
        .get_metadata_episodes_cache_for_season("meta-001", "tvdb", "", "absolute", 1)
        .await
        .unwrap();
    assert_eq!(
        abs_results.len(),
        1,
        "Absolute mode season 1 should have 1 entry"
    );
    assert!(abs_results[0].title.starts_with("Absolute"));
}

/// Test that normal and absolute mode overrides with the same season number
/// are completely isolated — cell counts, episode ranges, and everything else
/// should be independent per mode.
#[tokio::test]
async fn test_normal_and_absolute_overrides_are_independent() {
    let (db, _tmp, _key) = setup_db().await;

    // Create a mapping with BOTH normal and absolute overrides for season "01".
    // Normal mode: cell_count=24, Absolute mode: cell_count=50.
    let mut normal_overrides = HashMap::new();
    normal_overrides.insert(
        "01".to_string(),
        SeasonOverride {
            season: "01".to_string(),
            cell_count: Some(24),
            episode_start: None,
            episode_end: None,
            episode_offset: None,
            alias_season_number: None,
            search_format: None,
            aliases: vec![],
            reg_patterns: vec![],
        },
    );

    let mut absolute_overrides = HashMap::new();
    absolute_overrides.insert(
        "01".to_string(),
        SeasonOverride {
            season: "01".to_string(),
            cell_count: Some(50),
            episode_start: None,
            episode_end: None,
            episode_offset: None,
            alias_season_number: None,
            search_format: None,
            aliases: vec![],
            reg_patterns: vec![],
        },
    );

    let mapping = MappingRule {
        target_title: "Test Series".to_string(),
        series_id: "ts-001".to_string(),
        settings: SeriesSettings {
            season: normal_overrides,
            season_absolute: absolute_overrides,
            ..Default::default()
        },
        ..Default::default()
    };

    let normalized = vec!["01".to_string()];

    // Step 1: Ensure cells in NORMAL mode — should create 24 normal-mode cells
    let mut norm = mapping.clone();
    norm.settings.absolute_numbering = Some(false);
    db.ensure_episode_cells(&norm, &normalized, false)
        .await
        .unwrap();

    let norm_eps = fetch_episodes(&db, "ts-001").await;
    assert_eq!(
        norm_eps.len(),
        24,
        "Normal mode should have 24 cells, got {}",
        norm_eps.len()
    );
    assert!(
        norm_eps.iter().all(|(_, _, _, mode, _, _)| *mode == 0),
        "All cells must be normal mode (numbering_mode=0)"
    );

    // Step 2: Ensure cells in ABSOLUTE mode — should create 50 absolute-mode cells
    let mut abs = mapping.clone();
    abs.settings.absolute_numbering = Some(true);
    db.ensure_episode_cells(&abs, &normalized, false)
        .await
        .unwrap();

    let all = fetch_all_episodes(&db, "ts-001").await;
    assert_eq!(
        all.len(),
        74,
        "24 normal + 50 absolute = 74 total, got {}",
        all.len()
    );

    let abs_count = all.iter().filter(|(_, _, _, m)| *m == 1).count();
    let norm_count = all.iter().filter(|(_, _, _, m)| *m == 0).count();
    assert_eq!(
        abs_count, 50,
        "Should have 50 absolute-mode cells, got {}",
        abs_count
    );
    assert_eq!(
        norm_count, 24,
        "Should have 24 normal-mode cells, got {}",
        norm_count
    );

    // Step 3: Verify IDs are in the correct format
    for (eid, _, _, mode) in &all {
        if *mode == 0 {
            assert!(
                eid.contains("S01E"),
                "Normal cell should have SXXEXX format: {}",
                eid
            );
        } else {
            assert!(
                eid.contains("ABS"),
                "Absolute cell should have ABS format: {}",
                eid
            );
        }
    }

    // Step 4: Re-run ensure_episode_cells in normal mode — should NOT delete absolute cells
    db.ensure_episode_cells(&norm, &normalized, false)
        .await
        .unwrap();
    let after = fetch_all_episodes(&db, "ts-001").await;
    assert_eq!(
        after.len(),
        74,
        "Re-running normal mode should not delete absolute cells, got {}",
        after.len()
    );
}

#[tokio::test]
async fn test_cache_upsert_does_not_collide_across_modes() {
    let (db, _tmp, _key) = setup_db().await;

    // Insert entries for both modes — same (season, episode) keys but different ordering_mode
    let make_entry =
        |season, ep, mode: &str, title: &str| crate::db::metadata_cache::EpisodeMetadataForCache {
            season_number: season,
            episode_number: ep,
            unique_id: format!("{}-s{}e{}", mode, season, ep),
            title: title.to_string(),
            description: None,
            runtime: None,
            image_url: None,
            meta_date: None,
        };
    db.batch_upsert_metadata_episodes_cache(
        "meta-001",
        "tvdb",
        "",
        "normal",
        &[
            make_entry(1, 1, "normal", "Normal S1E1"),
            make_entry(2, 1, "normal", "Normal S2E1"),
        ],
    )
    .await
    .unwrap();
    db.batch_upsert_metadata_episodes_cache(
        "meta-001",
        "tvdb",
        "",
        "absolute",
        &[
            make_entry(1, 1, "absolute", "Absolute S1E1"),
            make_entry(1, 2, "absolute", "Absolute S1E2"),
        ],
    )
    .await
    .unwrap();

    // Total rows in cache should be 4 (no collisions despite shared (season, episode) keys)
    let total: i32 =
        sqlx::query_scalar("SELECT COUNT(*) FROM metadata_episodes_cache WHERE metadata_id = ?")
            .bind("meta-001")
            .fetch_one(pool(&db))
            .await
            .unwrap();
    assert_eq!(total, 4, "4 total cache rows — no collisions between modes");

    // Normal mode query: 2 entries
    let normal = db
        .get_metadata_episodes_cache("meta-001", "tvdb", "", "normal")
        .await
        .unwrap();
    assert_eq!(normal.len(), 2);
    assert!(normal.iter().all(|r| r.unique_id.starts_with("normal-")));

    // Absolute mode query: 2 entries
    let absolute = db
        .get_metadata_episodes_cache("meta-001", "tvdb", "", "absolute")
        .await
        .unwrap();
    assert_eq!(absolute.len(), 2);
    assert!(
        absolute
            .iter()
            .all(|r| r.unique_id.starts_with("absolute-"))
    );
}

#[tokio::test]
async fn test_stats_counts_only_in_range_episodes() {
    let (db, _tmp, _key) = setup_db().await;

    // Season range [1, 11] via the season override (the SSoT range mechanism).
    let mut mapping = make_mapping_with_range("ts-001", "01", Some(1), Some(11));
    mapping.ensure_series_id();
    db.upsert_series_mapping("ts-001", &mapping).await.unwrap();

    // Insert 9 in-range episodes (1-9) WITH files — these SHOULD count as downloaded
    for ep in 1..=9 {
        let eid = format!("ts-001_S01E{:02}", ep);
        let meta_ids = std::collections::HashMap::new();

        db.insert_episode(crate::db::episodes::crud::InsertEpisodeParams {
            file_path: Some("/media/file.mkv"),

            status: "organized",

            ..crate::db::episodes::crud::InsertEpisodeParams::dummy(
                &eid, "ts-001", 1, ep, &meta_ids,
            )
        })
        .await
        .unwrap();
    }

    // Insert 2 in-range episodes (10-11) WITHOUT files — these should count in total but NOT downloaded
    let meta_ids = std::collections::HashMap::new();
    for ep in 10..=11 {
        let eid = format!("ts-001_S01E{:02}", ep);
        db.insert_episode(crate::db::episodes::crud::InsertEpisodeParams {
            status: "missing",
            ..crate::db::episodes::crud::InsertEpisodeParams::dummy(
                &eid, "ts-001", 1, ep, &meta_ids,
            )
        })
        .await
        .unwrap();
    }

    // Insert 13 out-of-range episodes (12-24) WITH files — these should NOT count at all
    for ep in 12..=24 {
        let eid = format!("ts-001_S01E{:02}", ep);
        let meta_ids = std::collections::HashMap::new();

        db.insert_episode(crate::db::episodes::crud::InsertEpisodeParams {
            file_path: Some("/media/extra.mkv"),

            status: "organized",

            ..crate::db::episodes::crud::InsertEpisodeParams::dummy(
                &eid, "ts-001", 1, ep, &meta_ids,
            )
        })
        .await
        .unwrap();
    }

    // Insert 3 unassigned cells (episode_cell_type = 0) with distinct episode_ids — these should be excluded
    // NOTE: Raw SQL is intentional here — episode_cell_type is a DB-internal field not exposed via
    // insert_episode(). This test specifically validates that the stats query excludes UserDefined cells.
    for ep in 1..=3 {
        let eid = format!("ts-001_S01E{:02}_cell", ep);
        sqlx::query(
            "INSERT INTO episodes (episode_id, series_id, season, episode, \
             status, monitored, episode_cell_type, numbering_mode) \
             VALUES (?, ?, ?, ?, 'unreleased', 1, 0, 0)",
        )
        .bind(&eid)
        .bind("ts-001")
        .bind(1)
        .bind(ep)
        .execute(db.get_pool())
        .await
        .unwrap();
    }

    // Test get_series_stats_with_seasons (used by library list view)
    let stats = db.get_series_stats_with_seasons().await.unwrap();
    assert_eq!(stats.len(), 1, "Should return stats for 1 series");
    assert_eq!(stats[0].series_title, "Test Series");
    assert_eq!(
        stats[0].downloaded_count, 9,
        "downloaded_count = 9 (9 in-range episodes with files, not 22 total files)"
    );
    assert_eq!(
        stats[0].total_count, 11,
        "total_count = 11 (11 in-range episodes, cells and out-of-range excluded)"
    );

    // Test get_series_stats (used by detail endpoints)
    let stats2 = db.get_series_stats().await.unwrap();
    assert_eq!(stats2.len(), 1, "Should return stats for 1 series");
    let (_, _, count, _size, _mmc) = &stats2[0];
    assert_eq!(*count, 9, "downloaded_count = 9 (same filter applied)");
}

#[tokio::test]
async fn test_stats_counts_episode_start_gt_one() {
    let (db, _tmp, _key) = setup_db().await;

    // Season range [3, 7].
    let mut mapping = make_mapping_with_range("ts-001", "01", Some(3), Some(7));
    mapping.ensure_series_id();
    db.upsert_series_mapping("ts-001", &mapping).await.unwrap();

    // Insert episodes 1-9 with files
    for ep in 1..=9 {
        let eid = format!("ts-001_S01E{:02}", ep);
        let meta_ids = std::collections::HashMap::new();

        db.insert_episode(crate::db::episodes::crud::InsertEpisodeParams {
            file_path: Some("/media/file.mkv"),

            status: "organized",

            ..crate::db::episodes::crud::InsertEpisodeParams::dummy(
                &eid, "ts-001", 1, ep, &meta_ids,
            )
        })
        .await
        .unwrap();
    }

    let stats = db.get_series_stats_with_seasons().await.unwrap();
    assert_eq!(stats.len(), 1);
    // Range [3,7]: episodes 3-7 have files → 5 downloaded, 5 total
    assert_eq!(stats[0].downloaded_count, 5);
    assert_eq!(stats[0].total_count, 5);

    // Verify episodes 1-2 and 8-9 are excluded despite having files
    let stats2 = db.get_series_stats().await.unwrap();
    assert_eq!(stats2.len(), 1);
    let (_, _, count, _size, _mmc) = &stats2[0];
    assert_eq!(*count, 5, "Only episodes 3-7 count");
}

#[tokio::test]
async fn test_stats_counts_per_season_overrides() {
    let (db, _tmp, _key) = setup_db().await;

    // Series with per-season overrides: season 1 has 24 eps, season 2 has 12 eps.
    let so2 = SeasonOverride {
        season: "02".to_string(),
        episode_start: Some(1),
        episode_end: Some(12),
        cell_count: None,
        episode_offset: None,
        alias_season_number: None,
        search_format: None,
        aliases: vec![],
        reg_patterns: vec![],
    };

    let mut seasons = std::collections::HashMap::new();
    seasons.insert("02".to_string(), so2);

    let mut mapping = MappingRule {
        target_title: "Per-Season Series".to_string(),
        series_id: "ts-001".to_string(),
        name: "per_season_series".to_string(),
        settings: SeriesSettings {
            season: seasons,
            ..Default::default()
        },
        ..Default::default()
    };
    mapping.ensure_series_id();
    db.upsert_series_mapping("ts-001", &mapping).await.unwrap();

    // Season 1: insert episodes 1-24, all WITH files
    for ep in 1..=24 {
        let eid = format!("ts-001_S01E{:02}", ep);
        let meta_ids = std::collections::HashMap::new();

        db.insert_episode(crate::db::episodes::crud::InsertEpisodeParams {
            file_path: Some("/media/s1_file.mkv"),

            status: "organized",

            ..crate::db::episodes::crud::InsertEpisodeParams::dummy(
                &eid, "ts-001", 1, ep, &meta_ids,
            )
        })
        .await
        .unwrap();
    }

    // Season 2: insert episodes 1-20, all WITH files (8 of which are out of range per override)
    for ep in 1..=20 {
        let eid = format!("ts-001_S02E{:02}", ep);
        let meta_ids = std::collections::HashMap::new();

        db.insert_episode(crate::db::episodes::crud::InsertEpisodeParams {
            file_path: Some("/media/s2_file.mkv"),

            status: "organized",

            ..crate::db::episodes::crud::InsertEpisodeParams::dummy(
                &eid, "ts-001", 2, ep, &meta_ids,
            )
        })
        .await
        .unwrap();
    }

    // Expected:
    //   Season 1: episodes 1-24 have files -> 24 downloaded, 24 total
    //   Season 2: episodes 1-12 have files (in range), 13-20 excluded -> 12 downloaded, 12 total
    //   Total downloaded: 24 + 12 = 36
    //   Total episodes:   24 + 12 = 36
    let stats = db.get_series_stats_with_seasons().await.unwrap();
    assert_eq!(stats.len(), 1);
    assert_eq!(
        stats[0].downloaded_count, 36,
        "36 total files: 24 (S1) + 12 (S2 in-range), not 44 (all files)"
    );
    assert_eq!(
        stats[0].total_count, 36,
        "36 total episodes: 24 (S1) + 12 (S2 in-range), S2 episodes 13-20 excluded"
    );

    // Verify get_series_stats has the same filter applied
    let stats2 = db.get_series_stats().await.unwrap();
    assert_eq!(stats2.len(), 1);
    let (_, _, count, _size, _mmc) = &stats2[0];
    assert_eq!(*count, 36);
}

#[tokio::test]
async fn test_stats_counts_no_bounds_same_as_all() {
    let (db, _tmp, _key) = setup_db().await;

    // No episode_start or episode_end set — all episodes should be counted
    let mut mapping = MappingRule {
        target_title: "No Bounds".to_string(),
        series_id: "ts-001".to_string(),
        name: "no_bounds".to_string(),
        ..Default::default()
    };
    mapping.ensure_series_id();
    db.upsert_series_mapping("ts-001", &mapping).await.unwrap();
    seed_episodes(&db, "ts-001").await;

    let stats = db.get_series_stats_with_seasons().await.unwrap();
    assert_eq!(stats.len(), 1);
    assert_eq!(stats[0].downloaded_count, 5);
    assert_eq!(stats[0].total_count, 5);
}

#[tokio::test]
async fn test_stats_counts_only_end_bound() {
    let (db, _tmp, _key) = setup_db().await;

    // Season override with only the end bound — episodes above end are excluded.
    let mut mapping = make_mapping_with_range("ts-001", "01", None, Some(3));
    mapping.target_title = "End Only".to_string();
    mapping.ensure_series_id();
    db.upsert_series_mapping("ts-001", &mapping).await.unwrap();
    seed_episodes(&db, "ts-001").await;

    let stats = db.get_series_stats_with_seasons().await.unwrap();
    assert_eq!(stats[0].downloaded_count, 3, "Only eps 1-3 count");
    assert_eq!(stats[0].total_count, 3);
}

#[tokio::test]
async fn test_stats_counts_only_start_bound() {
    let (db, _tmp, _key) = setup_db().await;

    // Season override with only the start bound — episodes below start are excluded.
    let mut mapping = make_mapping_with_range("ts-001", "01", Some(3), None);
    mapping.target_title = "Start Only".to_string();
    mapping.ensure_series_id();
    db.upsert_series_mapping("ts-001", &mapping).await.unwrap();
    seed_episodes(&db, "ts-001").await;

    let stats = db.get_series_stats_with_seasons().await.unwrap();
    assert_eq!(stats[0].downloaded_count, 3, "Only eps 3-5 count");
    assert_eq!(stats[0].total_count, 3);
}

#[tokio::test]
async fn test_stats_counts_multiple_series() {
    let (db, _tmp, _key) = setup_db().await;

    // Series A: season range [2, 4]
    let mut mapping_a = make_mapping_with_range("ts-001", "01", Some(2), Some(4));
    mapping_a.target_title = "Series A".to_string();
    mapping_a.ensure_series_id();
    db.upsert_series_mapping("ts-001", &mapping_a)
        .await
        .unwrap();

    // Series B: season range [1, 2]
    let mut mapping_b = make_mapping_with_range("ts-002", "01", Some(1), Some(2));
    mapping_b.target_title = "Series B".to_string();
    mapping_b.ensure_series_id();
    db.upsert_series_mapping("ts-002", &mapping_b)
        .await
        .unwrap();

    // Insert episodes for Series A: season 1, episodes 1-5 with files
    for ep in 1..=5 {
        let eid = format!("ts-001_S01E{:02}", ep);
        let meta_ids = std::collections::HashMap::new();

        db.insert_episode(crate::db::episodes::crud::InsertEpisodeParams {
            file_path: Some("/media/a.mkv"),

            status: "organized",

            ..crate::db::episodes::crud::InsertEpisodeParams::dummy(
                &eid, "ts-001", 1, ep, &meta_ids,
            )
        })
        .await
        .unwrap();
    }

    // Insert episodes for Series B: season 1, episodes 1-5 with files
    for ep in 1..=5 {
        let eid = format!("ts-002_S01E{:02}", ep);
        let meta_ids = std::collections::HashMap::new();

        db.insert_episode(crate::db::episodes::crud::InsertEpisodeParams {
            file_path: Some("/media/b.mkv"),

            status: "organized",

            ..crate::db::episodes::crud::InsertEpisodeParams::dummy(
                &eid, "ts-002", 1, ep, &meta_ids,
            )
        })
        .await
        .unwrap();
    }

    let stats = db.get_series_stats_with_seasons().await.unwrap();
    assert_eq!(stats.len(), 2);

    // Series A: only eps 2-4 count
    let a = stats.iter().find(|s| s.series_title == "Series A").unwrap();
    assert_eq!(a.downloaded_count, 3);
    assert_eq!(a.total_count, 3);

    // Series B: only eps 1-2 count
    let b = stats.iter().find(|s| s.series_title == "Series B").unwrap();
    assert_eq!(b.downloaded_count, 2);
    assert_eq!(b.total_count, 2);
}

#[tokio::test]
async fn test_stats_counts_per_season_start_gt_one() {
    let (db, _tmp, _key) = setup_db().await;

    // Per-season override: season 2 starts at episode 5, ends at 10
    let so2 = SeasonOverride {
        season: "02".to_string(),
        episode_start: Some(5),
        episode_end: Some(10),
        cell_count: None,
        episode_offset: None,
        alias_season_number: None,
        search_format: None,
        aliases: vec![],
        reg_patterns: vec![],
    };

    let mut seasons = std::collections::HashMap::new();
    seasons.insert("02".to_string(), so2);

    let mut mapping = MappingRule {
        target_title: "Per-Season Start".to_string(),
        series_id: "ts-001".to_string(),
        name: "per_season_start".to_string(),
        settings: SeriesSettings {
            season: seasons,
            ..Default::default()
        },
        ..Default::default()
    };
    mapping.ensure_series_id();
    db.upsert_series_mapping("ts-001", &mapping).await.unwrap();

    // Season 2: insert episodes 1-12, all WITH files (only 5-10 should count)
    for ep in 1..=12 {
        let eid = format!("ts-001_S02E{:02}", ep);
        let meta_ids = std::collections::HashMap::new();

        db.insert_episode(crate::db::episodes::crud::InsertEpisodeParams {
            file_path: Some("/media/s2_file.mkv"),

            status: "organized",

            ..crate::db::episodes::crud::InsertEpisodeParams::dummy(
                &eid, "ts-001", 2, ep, &meta_ids,
            )
        })
        .await
        .unwrap();
    }

    let stats = db.get_series_stats_with_seasons().await.unwrap();
    assert_eq!(stats.len(), 1);
    assert_eq!(
        stats[0].downloaded_count, 6,
        "6 files: S2 episodes 5-10 only, not 12"
    );
    assert_eq!(stats[0].total_count, 6, "6 episodes: S2 episodes 5-10 only");
}

#[tokio::test]
async fn test_prune_spares_a_cell_that_owns_a_file() {
    let (db, _tmp, _key) = setup_db().await;
    let normalized = vec!["01".to_string()];

    db.ensure_episode_cells(
        &make_mapping_with_season("ts-001", "01", Some(3), false),
        &normalized,
        false,
    )
    .await
    .unwrap();

    // Cell 3 stops being a placeholder once it owns a file.
    let ep3 = "ts-001_S01E03";
    db.assign_file_to_episode(crate::db::episodes::assign::AssignFileToEpisodeParams {
        episode_id: ep3,
        series_id: "ts-001",
        series_title: "Test Series",
        season: 1,
        episode: 3,
        file_path: "/media/ts-001/S01E03.mkv",
        episode_title: "Episode 3",
        all_target_episode_ids: None,
        numbering_mode: 0,
        only_unassigned: false,
    })
    .await
    .unwrap();

    // Shrinking cell_count prunes the empty placeholders, never the owned file.
    db.ensure_episode_cells(
        &make_mapping_with_season("ts-001", "01", Some(2), false),
        &normalized,
        false,
    )
    .await
    .unwrap();

    let episodes = fetch_episodes(&db, "ts-001").await;
    assert!(
        episodes.iter().any(|(id, ..)| id.as_str() == ep3),
        "a cell that owns a file must survive pruning, got {episodes:?}"
    );
    let assoc: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM episode_files WHERE episode_id = ?")
        .bind(ep3)
        .fetch_one(pool(&db))
        .await
        .unwrap();
    assert_eq!(assoc, 1, "the file association must survive pruning");
}

#[tokio::test]
async fn test_completion_counts_use_file_ownership() {
    let (db, _tmp, _key) = setup_db().await;
    seed_episodes(&db, "ts-001").await;

    let counts = db.get_series_season_completion_counts().await.unwrap();
    let seasons = counts.get("ts-001").expect("ts-001 has episodes");
    assert_eq!(
        seasons
            .iter()
            .map(|c| (c.season, c.organized, c.expected))
            .collect::<Vec<_>>(),
        vec![(1, 5, 5)],
        "every seeded episode owns a file, all in season 1"
    );
}
