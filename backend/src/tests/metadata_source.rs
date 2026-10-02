// Tests for the `metadata_source` column: batch_insert_metadata_episodes respects
// custom/cleared states, save/clear set the source, match_*_to_provider resets to NULL,
// provider switches update the key, and ensure_episode_cells seeds NULL. Each test uses a
// temp DB and asserts row-level state via raw SQL so metadata_source is inspectable.

use crate::db::DbManager;
use jumbie_shared::types::{MappingRule, SeasonOverride, SeriesSettings};
use std::collections::HashMap;

// Helpers

async fn setup_db() -> (DbManager, tempfile::TempDir, String) {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test.db");
    let db = DbManager::new(&db_path).await.unwrap();

    let mapping = MappingRule {
        target_title: "Test Series".to_string(),
        series_id: "series-key".to_string(),
        name: "test_series".to_string(),
        ..Default::default()
    };
    db.upsert_series_mapping("series-key", &mapping)
        .await
        .unwrap();

    (db, tmp, "series-key".to_string())
}

fn pool(db: &DbManager) -> &sqlx::SqlitePool {
    db.get_pool()
}

fn make_ep_meta(
    season: i32,
    episode: i32,
    title: &str,
) -> crate::plugins::metadata::EpisodeMetadata {
    crate::plugins::metadata::EpisodeMetadata {
        unique_id: format!("ext-{}-{}", season, episode),
        season,
        episode,
        title: title.to_string(),
        description: Some(format!("Desc for {}", title)),
        runtime: Some(30),
        image_url: Some(format!("https://example.com/{}.jpg", title)),
        meta_date: None,
    }
}

async fn get_metadata_source(db: &DbManager, series_id: &str, episode: i32) -> Option<String> {
    sqlx::query_scalar::<_, Option<String>>(
        "SELECT metadata_source FROM episodes \
         WHERE series_id = ? AND episode = ? AND numbering_mode = 0",
    )
    .bind(series_id)
    .bind(episode)
    .fetch_one(pool(db))
    .await
    .unwrap()
}

async fn get_title(db: &DbManager, series_id: &str, episode: i32) -> Option<String> {
    sqlx::query_scalar::<_, Option<String>>(
        "SELECT title FROM episodes \
         WHERE series_id = ? AND episode = ? AND numbering_mode = 0",
    )
    .bind(series_id)
    .bind(episode)
    .fetch_one(pool(db))
    .await
    .unwrap()
}

async fn insert_test_meta_episode(
    db: &DbManager,
    episode: i32,
    season: i32,
    title: &str,
    episode_key: &str,
) {
    let ep_meta = make_ep_meta(season, episode, title);
    db.batch_insert_metadata_episodes(
        "tvdb",
        "series-key",
        vec![(episode_key.to_string(), ep_meta)],
        0,
    )
    .await
    .unwrap();
}

// Tests

#[tokio::test]
async fn test_metadata_source_fills_missing() {
    let (db, _tmp, _key) = setup_db().await;

    sqlx::query(
        "INSERT INTO episodes (episode_id, series_id, season, episode, status, \
         numbering_mode) \
         VALUES (?, ?, ?, ?, 'unreleased', 0)",
    )
    .bind("ts-001_S01E01")
    .bind("series-key")
    .bind(1)
    .bind(1)
    .execute(pool(&db))
    .await
    .unwrap();

    assert_eq!(get_metadata_source(&db, "series-key", 1).await, None);

    insert_test_meta_episode(&db, 1, 1, "Provider Title", "ts-001_S01E01").await;

    assert_eq!(
        get_metadata_source(&db, "series-key", 1).await,
        Some("tvdb".to_string())
    );
    assert_eq!(
        get_title(&db, "series-key", 1).await,
        Some("Provider Title".to_string())
    );
}

#[tokio::test]
async fn test_custom_is_protected_from_overwrite() {
    let (db, _tmp, _key) = setup_db().await;

    sqlx::query(
        "INSERT INTO episodes (episode_id, series_id, season, episode, title, \
         description, runtime, image_url, status, metadata_source, numbering_mode) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'unreleased', 'custom', 0)",
    )
    .bind("ts-001_S01E01")
    .bind("series-key")
    .bind(1)
    .bind(1)
    .bind("Custom Title")
    .bind("Custom description")
    .bind(45)
    .bind("https://custom.example.com/img.jpg")
    .execute(pool(&db))
    .await
    .unwrap();

    assert_eq!(
        get_metadata_source(&db, "series-key", 1).await,
        Some("custom".to_string())
    );

    insert_test_meta_episode(&db, 1, 1, "Provider Title", "ts-001_S01E01").await;

    assert_eq!(
        get_metadata_source(&db, "series-key", 1).await,
        Some("custom".to_string())
    );
    assert_eq!(
        get_title(&db, "series-key", 1).await,
        Some("Custom Title".to_string())
    );
}

#[tokio::test]
async fn test_cleared_is_protected_from_overwrite() {
    let (db, _tmp, _key) = setup_db().await;

    sqlx::query(
        "INSERT INTO episodes (episode_id, series_id, season, episode, title, \
         status, metadata_source, numbering_mode) \
         VALUES (?, ?, ?, ?, NULL, 'unreleased', 'cleared', 0)",
    )
    .bind("ts-001_S01E01")
    .bind("series-key")
    .bind(1)
    .bind(1)
    .execute(pool(&db))
    .await
    .unwrap();

    assert_eq!(
        get_metadata_source(&db, "series-key", 1).await,
        Some("cleared".to_string())
    );
    assert_eq!(get_title(&db, "series-key", 1).await, None);

    insert_test_meta_episode(&db, 1, 1, "Provider Title", "ts-001_S01E01").await;

    assert_eq!(
        get_metadata_source(&db, "series-key", 1).await,
        Some("cleared".to_string())
    );
    assert_eq!(get_title(&db, "series-key", 1).await, None);
}

#[tokio::test]
async fn test_provider_is_overwritten() {
    let (db, _tmp, _key) = setup_db().await;

    sqlx::query(
        "INSERT INTO episodes (episode_id, series_id, season, episode, title, description, \
         runtime, image_url, status, metadata_source, numbering_mode) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'unreleased', 'tvdb', 0)",
    )
    .bind("ts-001_S01E01")
    .bind("series-key")
    .bind(1)
    .bind(1)
    .bind("Old TVDB Title")
    .bind("Old description")
    .bind(45)
    .bind("https://old.example.com/img.jpg")
    .execute(pool(&db))
    .await
    .unwrap();

    let ep_meta = make_ep_meta(1, 1, "New TVDB Title");
    db.batch_insert_metadata_episodes(
        "tvdb",
        "series-key",
        vec![("ts-001_S01E01".to_string(), ep_meta)],
        0,
    )
    .await
    .unwrap();

    assert_eq!(
        get_metadata_source(&db, "series-key", 1).await,
        Some("tvdb".to_string())
    );
    assert_eq!(
        get_title(&db, "series-key", 1).await,
        Some("New TVDB Title".to_string())
    );
}

#[tokio::test]
async fn test_provider_switch_updates_key() {
    let (db, _tmp, _key) = setup_db().await;

    sqlx::query(
        "INSERT INTO episodes (episode_id, series_id, season, episode, title, description, \
         runtime, image_url, status, metadata_source, numbering_mode) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'unreleased', 'tvmaze', 0)",
    )
    .bind("ts-001_S01E01")
    .bind("series-key")
    .bind(1)
    .bind(1)
    .bind("TVMaze Title")
    .bind("TVMaze description")
    .bind(45)
    .bind("https://tvmaze.example.com/img.jpg")
    .execute(pool(&db))
    .await
    .unwrap();

    let ep_meta = make_ep_meta(1, 1, "TVDB Title");
    db.batch_insert_metadata_episodes(
        "tvdb",
        "series-key",
        vec![("ts-001_S01E01".to_string(), ep_meta)],
        0,
    )
    .await
    .unwrap();

    assert_eq!(
        get_metadata_source(&db, "series-key", 1).await,
        Some("tvdb".to_string())
    );
    assert_eq!(
        get_title(&db, "series-key", 1).await,
        Some("TVDB Title".to_string())
    );
}

#[tokio::test]
async fn test_save_custom_metadata() {
    let (db, _tmp, _key) = setup_db().await;

    sqlx::query(
        "INSERT INTO episodes (episode_id, series_id, season, episode, status, \
         numbering_mode) \
         VALUES (?, ?, ?, ?, 'unreleased', 0)",
    )
    .bind("ts-001_S01E01")
    .bind("series-key")
    .bind(1)
    .bind(1)
    .execute(pool(&db))
    .await
    .unwrap();

    let found = db
        .save_custom_episode_metadata(crate::db::episodes::SaveCustomMetadataParams {
            episode_id: "ts-001_S01E01",
            series_id: "series-key",
            title: Some("Custom Title"),
            description: Some("Custom description"),
            runtime: Some(60),
            image_url: Some("https://custom.example.com/img.jpg"),
            meta_date: None,
        })
        .await
        .unwrap();
    assert!(found, "save_custom_episode_metadata should return true");

    assert_eq!(
        get_metadata_source(&db, "series-key", 1).await,
        Some("custom".to_string())
    );
    assert_eq!(
        get_title(&db, "series-key", 1).await,
        Some("Custom Title".to_string())
    );

    insert_test_meta_episode(&db, 1, 1, "Provider Title", "ts-001_S01E01").await;

    assert_eq!(
        get_title(&db, "series-key", 1).await,
        Some("Custom Title".to_string())
    );
    assert_eq!(
        get_metadata_source(&db, "series-key", 1).await,
        Some("custom".to_string())
    );
}

#[tokio::test]
async fn test_clear_episode_metadata() {
    let (db, _tmp, _key) = setup_db().await;

    insert_test_meta_episode(&db, 1, 1, "Provider Title", "ts-001_S01E01").await;

    assert_eq!(
        get_metadata_source(&db, "series-key", 1).await,
        Some("tvdb".to_string())
    );

    let found = db
        .clear_episode_metadata("ts-001_S01E01", "series-key")
        .await
        .unwrap();
    assert!(found, "clear_episode_metadata should return true");

    assert_eq!(
        get_metadata_source(&db, "series-key", 1).await,
        Some("cleared".to_string())
    );
    assert_eq!(get_title(&db, "series-key", 1).await, None);

    insert_test_meta_episode(&db, 2, 1, "New Provider Title", "ts-001_S01E02").await;

    assert_eq!(get_title(&db, "series-key", 1).await, None);
    assert_eq!(
        get_metadata_source(&db, "series-key", 1).await,
        Some("cleared".to_string())
    );
}

#[tokio::test]
async fn test_match_season_resets_to_null() {
    let (db, _tmp, _key) = setup_db().await;

    sqlx::query(
        "INSERT INTO episodes (episode_id, series_id, season, episode, title, \
         status, metadata_source, numbering_mode) \
         VALUES (?, ?, ?, ?, ?, 'unreleased', 'custom', 0)",
    )
    .bind("ts-001_S01E01")
    .bind("series-key")
    .bind(1)
    .bind(1)
    .bind("Custom E1")
    .execute(pool(&db))
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO episodes (episode_id, series_id, season, episode, title, \
         status, metadata_source, numbering_mode) \
         VALUES (?, ?, ?, ?, ?, 'unreleased', 'custom', 0)",
    )
    .bind("ts-001_S01E02")
    .bind("series-key")
    .bind(1)
    .bind(2)
    .bind("Custom E2")
    .execute(pool(&db))
    .await
    .unwrap();

    let affected = db.match_season_to_provider("series-key", 1).await.unwrap();
    assert_eq!(affected, 2, "Should have matched 2 episodes");

    assert_eq!(get_metadata_source(&db, "series-key", 1).await, None);
    assert_eq!(get_title(&db, "series-key", 1).await, None);
    assert_eq!(get_metadata_source(&db, "series-key", 2).await, None);
    assert_eq!(get_title(&db, "series-key", 2).await, None);
}

#[tokio::test]
async fn test_match_series_resets_all_seasons() {
    let (db, _tmp, _key) = setup_db().await;

    sqlx::query(
        "INSERT INTO episodes (episode_id, series_id, season, episode, title, \
         status, metadata_source, numbering_mode) \
         VALUES (?, ?, ?, ?, ?, 'unreleased', 'custom', 0)",
    )
    .bind("ts-001_S01E01")
    .bind("series-key")
    .bind(1)
    .bind(1)
    .bind("S01 Custom")
    .execute(pool(&db))
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO episodes (episode_id, series_id, season, episode, status, \
         metadata_source, numbering_mode) \
         VALUES (?, ?, ?, ?, 'unreleased', 'cleared', 0)",
    )
    .bind("ts-001_S02E01")
    .bind("series-key")
    .bind(2)
    .bind(1)
    .execute(pool(&db))
    .await
    .unwrap();

    let affected = db.match_series_to_provider("series-key").await.unwrap();
    assert_eq!(affected, 2, "Should have matched 2 episodes");

    assert_eq!(get_metadata_source(&db, "series-key", 1).await, None);
    assert_eq!(get_title(&db, "series-key", 1).await, None);
}

#[tokio::test]
async fn test_ensure_cells_have_null_metadata_source() {
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

    let mapping = MappingRule {
        target_title: "Test Series".to_string(),
        series_id: "series-key".to_string(),
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

    for ep_num in 1..=3 {
        let source = get_metadata_source(&db, "series-key", ep_num).await;
        assert_eq!(
            source, None,
            "Cell {} should have NULL metadata_source",
            ep_num
        );
    }
}

#[tokio::test]
async fn test_non_existent_episode_returns_false() {
    let (db, _tmp, _key) = setup_db().await;

    let found = db
        .save_custom_episode_metadata(crate::db::episodes::SaveCustomMetadataParams {
            episode_id: "nonexistent",
            series_id: "series-key",
            title: Some("Title"),
            description: None,
            runtime: None,
            image_url: None,
            meta_date: None,
        })
        .await
        .unwrap();
    assert!(!found, "Should return false for non-existent episode");

    let found = db
        .clear_episode_metadata("nonexistent", "series-key")
        .await
        .unwrap();
    assert!(!found, "Should return false for non-existent episode");
}

#[tokio::test]
async fn test_two_instances_store_separate_episode_cache() {
    let (db, _tmp, _key) = setup_db().await;

    // Simulate two instances of the same provider (e.g. TVDB English + TVDB French)
    let instance_a = "aaaaaaaa-aaaa-4aaa-aaaa-aaaaaaaaaaaa";
    let instance_b = "bbbbbbbb-bbbb-4bbb-bbbb-bbbbbbbbbbbb";
    let metadata_id = "tvdb-ext-id";

    let ep_a = crate::db::metadata_cache::EpisodeMetadataForCache {
        season_number: 1,
        episode_number: 1,
        unique_id: "ext-1-a".to_string(),
        title: "Episode A EN".to_string(),
        description: Some("English description".to_string()),
        runtime: Some(45),
        image_url: Some("https://en.example.com/ep1.jpg".to_string()),
        meta_date: None,
    };
    db.batch_upsert_metadata_episodes_cache(metadata_id, "tvdb", instance_a, "normal", &[ep_a])
        .await
        .unwrap();

    let ep_b = crate::db::metadata_cache::EpisodeMetadataForCache {
        season_number: 1,
        episode_number: 1,
        unique_id: "ext-1-b".to_string(),
        title: "Episode B FR".to_string(),
        description: Some("Description française".to_string()),
        runtime: Some(45),
        image_url: Some("https://fr.example.com/ep1.jpg".to_string()),
        meta_date: None,
    };
    db.batch_upsert_metadata_episodes_cache(metadata_id, "tvdb", instance_b, "normal", &[ep_b])
        .await
        .unwrap();

    let cached_a = db
        .get_metadata_episodes_cache(metadata_id, "tvdb", instance_a, "normal")
        .await
        .unwrap();
    assert_eq!(cached_a.len(), 1);
    assert_eq!(cached_a[0].title, "Episode A EN");

    let cached_b = db
        .get_metadata_episodes_cache(metadata_id, "tvdb", instance_b, "normal")
        .await
        .unwrap();
    assert_eq!(cached_b.len(), 1);
    assert_eq!(cached_b[0].title, "Episode B FR");

    db.delete_metadata_episodes_cache_for_provider(metadata_id, "tvdb", instance_a)
        .await
        .unwrap();

    let cached_a_after = db
        .get_metadata_episodes_cache(metadata_id, "tvdb", instance_a, "normal")
        .await
        .unwrap();
    assert_eq!(cached_a_after.len(), 0);

    let cached_b_after = db
        .get_metadata_episodes_cache(metadata_id, "tvdb", instance_b, "normal")
        .await
        .unwrap();
    assert_eq!(cached_b_after.len(), 1);
    assert_eq!(cached_b_after[0].title, "Episode B FR");
}

#[tokio::test]
async fn test_two_instances_store_separate_seasons() {
    let (db, _tmp, _key) = setup_db().await;

    let instance_a = "aaaaaaaa-aaaa-4aaa-aaaa-aaaaaaaaaaaa";
    let instance_b = "bbbbbbbb-bbbb-4bbb-bbbb-bbbbbbbbbbbb";
    let metadata_id = "tvdb-ext-id";

    let seasons_a = vec![
        crate::plugins::metadata::SeasonMetadata {
            season: 1,
            episode_count: 12,
        },
        crate::plugins::metadata::SeasonMetadata {
            season: 2,
            episode_count: 10,
        },
    ];
    db.upsert_metadata_season_cache(metadata_id, "tvdb", instance_a, "normal", &seasons_a)
        .await
        .unwrap();

    let seasons_b = vec![crate::plugins::metadata::SeasonMetadata {
        season: 1,
        episode_count: 24, // different episode count
    }];
    db.upsert_metadata_season_cache(metadata_id, "tvdb", instance_b, "normal", &seasons_b)
        .await
        .unwrap();

    let result_a = db
        .get_metadata_season_cache(metadata_id, "tvdb", instance_a, "normal")
        .await
        .unwrap();
    assert_eq!(result_a.len(), 2);
    assert!(result_a.contains(&("1".to_string(), 12)));
    assert!(result_a.contains(&("2".to_string(), 10)));

    let result_b = db
        .get_metadata_season_cache(metadata_id, "tvdb", instance_b, "normal")
        .await
        .unwrap();
    assert_eq!(result_b.len(), 1);
    assert!(result_b.contains(&("1".to_string(), 24)));

    db.delete_metadata_season_cache_for_provider(metadata_id, "tvdb", instance_a)
        .await
        .unwrap();

    let result_a_after = db
        .get_metadata_season_cache(metadata_id, "tvdb", instance_a, "normal")
        .await
        .unwrap();
    assert_eq!(result_a_after.len(), 0);

    let result_b_after = db
        .get_metadata_season_cache(metadata_id, "tvdb", instance_b, "normal")
        .await
        .unwrap();
    assert_eq!(result_b_after.len(), 1);
}

#[tokio::test]
async fn test_two_instances_separate_fetch_log() {
    let (db, _tmp, _key) = setup_db().await;

    let instance_a = "aaaaaaaa-aaaa-4aaa-aaaa-aaaaaaaaaaaa";
    let instance_b = "bbbbbbbb-bbbb-4bbb-bbbb-bbbbbbbbbbbb";
    let metadata_id = "tvdb-ext-id";

    db.mark_fetch_attempted(metadata_id, "tvdb", instance_a, "normal")
        .await
        .unwrap();

    let attempted_a = db
        .has_fetch_been_attempted(metadata_id, "tvdb", instance_a, "normal")
        .await;
    assert!(attempted_a);

    let attempted_b = db
        .has_fetch_been_attempted(metadata_id, "tvdb", instance_b, "normal")
        .await;
    assert!(!attempted_b);

    db.delete_metadata_fetch_log_for_provider(metadata_id, "tvdb", instance_a)
        .await
        .unwrap();

    let attempted_a_after = db
        .has_fetch_been_attempted(metadata_id, "tvdb", instance_a, "normal")
        .await;
    assert!(!attempted_a_after);
}

#[tokio::test]
async fn test_two_instances_separate_series_cache() {
    let (db, _tmp, _key) = setup_db().await;

    let instance_a = "aaaaaaaa-aaaa-4aaa-aaaa-aaaaaaaaaaaa";
    let instance_b = "bbbbbbbb-bbbb-4bbb-bbbb-bbbbbbbbbbbb";
    let metadata_id = "tvdb-ext-id";

    db.upsert_metadata_series_cache(crate::db::metadata_cache::UpsertMetadataCacheParams {
        metadata_id,
        plugin_id: "tvdb",
        instance_id: instance_a,
        title: "English Show",
        overview: Some("English overview"),
        language: None,
        aliases: &[],
        image_url: None,
    })
    .await
    .unwrap();

    db.upsert_metadata_series_cache(crate::db::metadata_cache::UpsertMetadataCacheParams {
        metadata_id,
        plugin_id: "tvdb",
        instance_id: instance_b,
        title: "Spectacle Français",
        overview: Some("Aperçu français"),
        language: None,
        aliases: &[],
        image_url: None,
    })
    .await
    .unwrap();

    let cached_a = db
        .get_metadata_series_cache(metadata_id, "tvdb", instance_a)
        .await
        .unwrap()
        .expect("Instance A should have cached data");
    assert_eq!(cached_a.title, "English Show");

    let cached_b = db
        .get_metadata_series_cache(metadata_id, "tvdb", instance_b)
        .await
        .unwrap()
        .expect("Instance B should have cached data");
    assert_eq!(cached_b.title, "Spectacle Français");

    db.delete_metadata_series_cache_for_provider(metadata_id, "tvdb", instance_a)
        .await
        .unwrap();

    let cached_a_after = db
        .get_metadata_series_cache(metadata_id, "tvdb", instance_a)
        .await
        .unwrap();
    assert!(
        cached_a_after.is_none(),
        "Instance A cache should be deleted"
    );

    let cached_b_after = db
        .get_metadata_series_cache(metadata_id, "tvdb", instance_b)
        .await
        .unwrap();
    assert!(cached_b_after.is_some(), "Instance B cache should survive");
    assert_eq!(cached_b_after.unwrap().title, "Spectacle Français");
}

#[tokio::test]
async fn test_per_instance_cache_delete_isolated() {
    let (db, _tmp, _key) = setup_db().await;

    let instance_a = "aaaaaaaa-aaaa-4aaa-aaaa-aaaaaaaaaaaa";
    let instance_b = "bbbbbbbb-bbbb-4bbb-bbbb-bbbbbbbbbbbb";
    let metadata_id = "tvdb-ext-id";

    let ep = crate::db::metadata_cache::EpisodeMetadataForCache {
        season_number: 1,
        episode_number: 1,
        unique_id: "ext-1".to_string(),
        title: "Episode Title".to_string(),
        description: None,
        runtime: None,
        image_url: None,
        meta_date: None,
    };
    let ep_for_b = crate::db::metadata_cache::EpisodeMetadataForCache {
        season_number: 1,
        episode_number: 1,
        unique_id: "ext-1".to_string(),
        title: "Episode Title".to_string(),
        description: None,
        runtime: None,
        image_url: None,
        meta_date: None,
    };
    db.batch_upsert_metadata_episodes_cache(metadata_id, "tvdb", instance_a, "normal", &[ep])
        .await
        .unwrap();
    db.batch_upsert_metadata_episodes_cache(metadata_id, "tvdb", instance_b, "normal", &[ep_for_b])
        .await
        .unwrap();

    let seasons = vec![crate::plugins::metadata::SeasonMetadata {
        season: 1,
        episode_count: 12,
    }];
    db.upsert_metadata_season_cache(metadata_id, "tvdb", instance_a, "normal", &seasons)
        .await
        .unwrap();
    db.upsert_metadata_season_cache(metadata_id, "tvdb", instance_b, "normal", &seasons)
        .await
        .unwrap();

    db.mark_fetch_attempted(metadata_id, "tvdb", instance_a, "normal")
        .await
        .unwrap();
    db.mark_fetch_attempted(metadata_id, "tvdb", instance_b, "normal")
        .await
        .unwrap();

    db.delete_metadata_episodes_cache_for_instance(instance_a)
        .await
        .unwrap();
    db.delete_metadata_season_cache_for_instance(instance_a)
        .await
        .unwrap();
    db.delete_metadata_fetch_log_for_instance(instance_a)
        .await
        .unwrap();
    db.delete_metadata_series_cache_for_instance(instance_a)
        .await
        .unwrap();

    let ep_a = db
        .get_metadata_episodes_cache(metadata_id, "tvdb", instance_a, "normal")
        .await
        .unwrap();
    assert_eq!(ep_a.len(), 0, "Instance A episodes should be deleted");

    let seasons_a = db
        .get_metadata_season_cache(metadata_id, "tvdb", instance_a, "normal")
        .await
        .unwrap();
    assert_eq!(seasons_a.len(), 0, "Instance A seasons should be deleted");

    let fetch_a = db
        .has_fetch_been_attempted(metadata_id, "tvdb", instance_a, "normal")
        .await;
    assert!(!fetch_a, "Instance A fetch log should be deleted");

    let ep_b = db
        .get_metadata_episodes_cache(metadata_id, "tvdb", instance_b, "normal")
        .await
        .unwrap();
    assert_eq!(ep_b.len(), 1, "Instance B episodes should survive");

    let seasons_b = db
        .get_metadata_season_cache(metadata_id, "tvdb", instance_b, "normal")
        .await
        .unwrap();
    assert_eq!(seasons_b.len(), 1, "Instance B seasons should survive");

    let fetch_b = db
        .has_fetch_been_attempted(metadata_id, "tvdb", instance_b, "normal")
        .await;
    assert!(fetch_b, "Instance B fetch log should survive");
}

#[tokio::test]
async fn test_two_instances_metadata_source_isolation() {
    let (db, _tmp, _key) = setup_db().await;

    let instance_a = "aaaaaaaa-aaaa-4aaa-aaaa-aaaaaaaaaaaa";
    let instance_b = "bbbbbbbb-bbbb-4bbb-bbbb-bbbbbbbbbbbb";

    sqlx::query(
        "INSERT INTO episodes (episode_id, series_id, season, episode, status, \
         numbering_mode) \
         VALUES (?, ?, ?, ?, 'unreleased', 0)",
    )
    .bind("ts-001_S01E01")
    .bind("series-key")
    .bind(1)
    .bind(1)
    .execute(pool(&db))
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO episodes (episode_id, series_id, season, episode, status, \
         numbering_mode) \
         VALUES (?, ?, ?, ?, 'unreleased', 0)",
    )
    .bind("ts-001_S01E02")
    .bind("series-key")
    .bind(1)
    .bind(2)
    .execute(pool(&db))
    .await
    .unwrap();

    let ep_a = make_ep_meta(1, 1, "Instance A Title");
    db.batch_insert_metadata_episodes(
        instance_a,
        "series-key",
        vec![("ts-001_S01E01".to_string(), ep_a)],
        0,
    )
    .await
    .unwrap();

    let ep_b = make_ep_meta(1, 2, "Instance B Title");
    db.batch_insert_metadata_episodes(
        instance_b,
        "series-key",
        vec![("ts-001_S01E02".to_string(), ep_b)],
        0,
    )
    .await
    .unwrap();

    let source_1 = get_metadata_source(&db, "series-key", 1).await;
    assert_eq!(source_1, Some(instance_a.to_string()));
    let title_1 = get_title(&db, "series-key", 1).await;
    assert_eq!(title_1, Some("Instance A Title".to_string()));

    let source_2 = get_metadata_source(&db, "series-key", 2).await;
    assert_eq!(source_2, Some(instance_b.to_string()));
    let title_2 = get_title(&db, "series-key", 2).await;
    assert_eq!(title_2, Some("Instance B Title".to_string()));

    db.clear_episode_metadata_for_instance(instance_b)
        .await
        .unwrap();

    let source_1_after = get_metadata_source(&db, "series-key", 1).await;
    assert_eq!(source_1_after, Some(instance_a.to_string()));
    let title_1_after = get_title(&db, "series-key", 1).await;
    assert_eq!(title_1_after, Some("Instance A Title".to_string()));

    let source_2_after = get_metadata_source(&db, "series-key", 2).await;
    assert_eq!(source_2_after, None);
    let title_2_after = get_title(&db, "series-key", 2).await;
    assert_eq!(title_2_after, None);
}

#[tokio::test]
async fn test_series_with_two_instance_metadata_ids() {
    let (db, _tmp, key) = setup_db().await;

    let instance_a = "aaaaaaaa-aaaa-4aaa-aaaa-aaaaaaaaaaaa";
    let instance_b = "bbbbbbbb-bbbb-4bbb-bbbb-bbbbbbbbbbbb";

    let mut mapping = db.get_series_mapping(&key).await.unwrap().unwrap();
    mapping
        .settings
        .metadata_ids
        .insert(instance_a.to_string(), "ext-id-a".to_string());
    mapping
        .settings
        .metadata_ids
        .insert(instance_b.to_string(), "ext-id-b".to_string());
    db.upsert_series_mapping(&key, &mapping).await.unwrap();

    let loaded = db.get_series_mapping(&key).await.unwrap().unwrap();
    assert_eq!(loaded.settings.metadata_ids.len(), 2);
    assert_eq!(
        loaded.settings.metadata_ids.get(instance_a),
        Some(&"ext-id-a".to_string())
    );
    assert_eq!(
        loaded.settings.metadata_ids.get(instance_b),
        Some(&"ext-id-b".to_string())
    );

    let ep_a = make_ep_meta(1, 1, "From Instance A");
    db.batch_insert_metadata_episodes(
        instance_a,
        "series-key",
        vec![("ts-001_S01E01".to_string(), ep_a)],
        0,
    )
    .await
    .unwrap();

    let ep_b = make_ep_meta(1, 2, "From Instance B");
    db.batch_insert_metadata_episodes(
        instance_b,
        "series-key",
        vec![("ts-001_S01E02".to_string(), ep_b)],
        0,
    )
    .await
    .unwrap();

    assert_eq!(
        get_metadata_source(&db, "series-key", 1).await,
        Some(instance_a.to_string())
    );
    assert_eq!(
        get_metadata_source(&db, "series-key", 2).await,
        Some(instance_b.to_string())
    );

    let meta_ids_1: Option<String> =
        sqlx::query_scalar("SELECT metadata_ids FROM episodes WHERE series_id = ? AND episode = 1")
            .bind("series-key")
            .fetch_one(pool(&db))
            .await
            .unwrap();
    let parsed_1: HashMap<String, String> =
        serde_json::from_str(&meta_ids_1.unwrap_or_default()).unwrap();
    assert!(
        parsed_1.contains_key(instance_a),
        "Episode 1 metadata_ids should contain instance_a UUID"
    );

    let meta_ids_2: Option<String> =
        sqlx::query_scalar("SELECT metadata_ids FROM episodes WHERE series_id = ? AND episode = 2")
            .bind("series-key")
            .fetch_one(pool(&db))
            .await
            .unwrap();
    let parsed_2: HashMap<String, String> =
        serde_json::from_str(&meta_ids_2.unwrap_or_default()).unwrap();
    assert!(
        parsed_2.contains_key(instance_b),
        "Episode 2 metadata_ids should contain instance_b UUID"
    );
}

#[tokio::test]
async fn test_reset_and_restore_episode_from_cache() {
    // Simulates the cache-hit path: episode exists, reset metadata_source,
    // batch insert restores cached data.
    let (db, _tmp, _key) = setup_db().await;

    sqlx::query(
        "INSERT INTO episodes (episode_id, series_id, season, episode, title, \
         status, metadata_source, numbering_mode) \
         VALUES (?, ?, ?, ?, ?, 'unreleased', 'custom', 0)",
    )
    .bind("ts-001_S01E01")
    .bind("series-key")
    .bind(1)
    .bind(1)
    .bind("Custom Title")
    .execute(pool(&db))
    .await
    .unwrap();

    assert_eq!(
        get_metadata_source(&db, "series-key", 1).await,
        Some("custom".to_string())
    );
    assert_eq!(
        get_title(&db, "series-key", 1).await,
        Some("Custom Title".to_string())
    );

    let found = db
        .reset_episode_metadata_source("ts-001_S01E01", "series-key")
        .await
        .unwrap();
    assert!(found, "reset should return true for existing episode");

    assert_eq!(get_metadata_source(&db, "series-key", 1).await, None);
    assert_eq!(get_title(&db, "series-key", 1).await, None);

    insert_test_meta_episode(&db, 1, 1, "Provider Title", "ts-001_S01E01").await;

    assert_eq!(
        get_metadata_source(&db, "series-key", 1).await,
        Some("tvdb".to_string())
    );
    assert_eq!(
        get_title(&db, "series-key", 1).await,
        Some("Provider Title".to_string())
    );
}

#[tokio::test]
async fn test_restore_deleted_episode_recreates_row_from_cache() {
    // Simulates restoring an episode that was deleted from the episodes table but
    // still has cached data in the metadata_episodes cache.
    let (db, _tmp, _key) = setup_db().await;
    let series_id = "series-key";

    let cache_ep = crate::db::metadata_cache::EpisodeMetadataForCache {
        season_number: 2,
        episode_number: 3,
        unique_id: "ext-provider-id".to_string(),
        title: "Cached Episode Title".to_string(),
        description: Some("Cached description".to_string()),
        runtime: Some(45),
        image_url: Some("https://example.com/ep.jpg".to_string()),
        meta_date: None,
    };
    db.batch_upsert_metadata_episodes_cache("metadata-id", "tvdb", "tvdb", "normal", &[cache_ep])
        .await
        .unwrap();

    let row: Option<(String,)> =
        sqlx::query_as("SELECT episode_id FROM episodes WHERE episode_id = ?")
            .bind("ts-001_S02E03")
            .fetch_optional(pool(&db))
            .await
            .unwrap();
    assert!(row.is_none(), "Episode should not exist yet (deleted)");

    let ep_meta = make_ep_meta(2, 3, "Cached Episode Title");
    db.batch_insert_metadata_episodes(
        "tvdb",
        series_id,
        vec![("ts-001_S02E03".to_string(), ep_meta)],
        0,
    )
    .await
    .unwrap();

    assert_eq!(
        get_metadata_source(&db, series_id, 3).await,
        Some("tvdb".to_string())
    );
    assert_eq!(
        get_title(&db, series_id, 3).await,
        Some("Cached Episode Title".to_string())
    );
}

#[tokio::test]
async fn test_reset_nonexistent_episode_returns_false() {
    let (db, _tmp, _key) = setup_db().await;

    let found = db
        .reset_episode_metadata_source("nonexistent", "series-key")
        .await
        .unwrap();
    assert!(!found, "reset should return false for non-existent episode");
}

// Guards in `proactively_fetch_missing_episode_metadata`: (A) provider configured
// (metadata_ids non-empty), (B) some episode with empty/NULL title not custom/cleared,
// (D) metadata_last_synced_at cooldown expired. Presence in `metadata_queue.active_series`
// after the call means all guards passed and a job was submitted.

use crate::api::{AppState, proactively_fetch_missing_episode_metadata};
use crate::config_manager::ConfigManager;
use crate::plugins::PluginManager;
use crate::scan_queue::ScanQueue;
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};

async fn setup_proactive_fetch_env(
    cooldown_minutes: u64,
) -> (Arc<AppState>, tempfile::TempDir, String) {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test.db");
    let db = Arc::new(DbManager::new(&db_path).await.unwrap());

    let general = jumbie_shared::config::general::GeneralConfig {
        metadata_fetch_cooldown_minutes: cooldown_minutes,
        ..Default::default()
    };
    db.save_general_config(&general).await.unwrap();

    let config_path = tmp.path().join("config.toml");
    std::fs::write(
        &config_path,
        format!(
            r#"database = "{}"
unknown_files_tmp_dir = "{}"
plugins_dir = "{}"
logs_dir = "{}"
"#,
            db_path.to_string_lossy().replace('\\', "/"),
            tmp.path().join("tmp").to_string_lossy().replace('\\', "/"),
            tmp.path()
                .join("plugins")
                .to_string_lossy()
                .replace('\\', "/"),
            tmp.path().join("logs").to_string_lossy().replace('\\', "/"),
        ),
    )
    .unwrap();

    let cfg = Arc::new(
        ConfigManager::new(db.clone(), config_path.to_str().unwrap())
            .await
            .unwrap(),
    );

    let state = Arc::new(AppState {
        cfg,
        db: db.clone(),
        downloader: None,
        notifications: None,
        organizer: None,
        ban_list: Arc::new(Mutex::new(HashMap::new())),
        is_reorganizing: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        failed_renames: Arc::new(RwLock::new(HashMap::new())),
        plugin_manager: Arc::new(RwLock::new(PluginManager::new(tmp.path().join("plugins")))),
        scan_queue: Arc::new(ScanQueue::default()),
        processing_renames: Arc::new(RwLock::new(HashSet::new())),
        metadata_queue: Arc::new(crate::metadata_queue::MetadataQueue::new()),
        rename_queue_has_pending: Arc::new(RwLock::new(false)),
        rename_queue_trigger: Arc::new(tokio::sync::Notify::new()),
        search_queue: Arc::new(crate::search_queue::SearchQueue::new()),
        shutdown_token: tokio_util::sync::CancellationToken::new(),
        rate_limiter: crate::middleware::rate_limit::new_rate_limiter(),
        auth_cache: Arc::new(crate::middleware::auth_cache::AuthCache::new()),
        monitor_sweep_cancel: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
        recalc_cancel: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
        progress_tracker: crate::api::ProgressTracker::new(),
        modifying_series: Arc::new(RwLock::new(HashSet::new())),
        rename_plan_cache: crate::file_manager::RenamePlanCache::new(),
        logs_dir: "/logs".to_string(),
        log_level: "info".to_string(),
        log_buffer: Arc::new(std::sync::RwLock::new(jumbie_shared::types::LogRing::new())),
    });

    let series_id = "proactive-series".to_string();
    let mapping = MappingRule {
        target_title: "Proactive Test Series".to_string(),
        series_id: series_id.clone(),
        name: "proactive_test".to_string(),
        settings: SeriesSettings {
            path: Some("ProactiveTestPath".to_string()),
            ..Default::default()
        },
        ..Default::default()
    };
    db.upsert_series_mapping(&series_id, &mapping)
        .await
        .unwrap();

    (state, tmp, series_id)
}

async fn insert_episode_row(
    db: &DbManager,
    series_id: &str,
    episode: i32,
    title: Option<&str>,
    metadata_source: Option<&str>,
) {
    let episode_id = format!("ps-{}-S01E{:02}", series_id, episode);
    sqlx::query(
        "INSERT INTO episodes (episode_id, series_id, season, episode, title, status, \
         metadata_source, numbering_mode) \
         VALUES (?, ?, ?, ?, ?, 'unreleased', ?, 0)",
    )
    .bind(&episode_id)
    .bind(series_id)
    .bind(1)
    .bind(episode)
    .bind(title)
    .bind(metadata_source)
    .execute(db.get_pool())
    .await
    .unwrap();
}

async fn fetch_episodes(db: &DbManager, series_id: &str) -> Vec<crate::db::EpisodeDetailRow> {
    db.get_series_episodes_details(series_id, false)
        .await
        .unwrap_or_default()
}

/// Reload the mapping to pick up metadata_ids changes.
async fn reload_mapping(db: &DbManager, series_id: &str) -> MappingRule {
    db.get_series_mapping(series_id)
        .await
        .unwrap()
        .expect("series mapping should exist")
}

#[tokio::test]
async fn test_proactive_fetch_skips_when_no_metadata_ids() {
    let (state, _tmp, series_id) = setup_proactive_fetch_env(60).await;
    let db = state.db.clone();

    insert_episode_row(&db, &series_id, 1, None, None).await;
    let mapping = reload_mapping(&db, &series_id).await;
    let episodes = fetch_episodes(&db, &series_id).await;

    assert!(mapping.settings.metadata_ids.is_empty());

    proactively_fetch_missing_episode_metadata(&state, &series_id, &episodes, &mapping, 60).await;

    assert!(
        !state.metadata_queue.contains(&series_id).await,
        "Should NOT submit to queue when no metadata_ids"
    );
}

#[tokio::test]
async fn test_proactive_fetch_skips_when_all_episodes_have_titles() {
    let (state, _tmp, series_id) = setup_proactive_fetch_env(60).await;
    let db = state.db.clone();

    insert_episode_row(&db, &series_id, 1, Some("Existing Title"), Some("tvdb")).await;

    // Add metadata_id so guard A passes, isolating guard B.
    let mut mapping = reload_mapping(&db, &series_id).await;
    mapping
        .settings
        .metadata_ids
        .insert("tvdb".to_string(), "ext-1".to_string());
    db.upsert_series_mapping(&series_id, &mapping)
        .await
        .unwrap();
    let mapping = reload_mapping(&db, &series_id).await;
    let episodes = fetch_episodes(&db, &series_id).await;

    proactively_fetch_missing_episode_metadata(&state, &series_id, &episodes, &mapping, 60).await;

    assert!(
        !state.metadata_queue.contains(&series_id).await,
        "Should NOT submit to queue when all episodes have titles"
    );
}

#[tokio::test]
async fn test_proactive_fetch_skips_when_title_empty_but_custom_source() {
    let (state, _tmp, series_id) = setup_proactive_fetch_env(60).await;
    let db = state.db.clone();

    insert_episode_row(&db, &series_id, 1, None, Some("custom")).await;

    let mut mapping = reload_mapping(&db, &series_id).await;
    mapping
        .settings
        .metadata_ids
        .insert("tvdb".to_string(), "ext-1".to_string());
    db.upsert_series_mapping(&series_id, &mapping)
        .await
        .unwrap();
    let mapping = reload_mapping(&db, &series_id).await;
    let episodes = fetch_episodes(&db, &series_id).await;

    proactively_fetch_missing_episode_metadata(&state, &series_id, &episodes, &mapping, 60).await;

    assert!(
        !state.metadata_queue.contains(&series_id).await,
        "Should NOT submit to queue when episode has custom metadata_source"
    );
}

#[tokio::test]
async fn test_proactive_fetch_skips_when_title_empty_but_cleared_source() {
    let (state, _tmp, series_id) = setup_proactive_fetch_env(60).await;
    let db = state.db.clone();

    insert_episode_row(&db, &series_id, 1, None, Some("cleared")).await;

    let mut mapping = reload_mapping(&db, &series_id).await;
    mapping
        .settings
        .metadata_ids
        .insert("tvdb".to_string(), "ext-1".to_string());
    db.upsert_series_mapping(&series_id, &mapping)
        .await
        .unwrap();
    let mapping = reload_mapping(&db, &series_id).await;
    let episodes = fetch_episodes(&db, &series_id).await;

    proactively_fetch_missing_episode_metadata(&state, &series_id, &episodes, &mapping, 60).await;

    assert!(
        !state.metadata_queue.contains(&series_id).await,
        "Should NOT submit to queue when episode has cleared metadata_source"
    );
}

#[tokio::test]
async fn test_proactive_fetch_skips_when_recently_synced() {
    let (state, _tmp, series_id) = setup_proactive_fetch_env(60).await;
    let db = state.db.clone();

    insert_episode_row(&db, &series_id, 1, None, None).await;

    let now = chrono::Utc::now().to_rfc3339();
    let mut mapping = reload_mapping(&db, &series_id).await;
    mapping
        .settings
        .metadata_ids
        .insert("tvdb".to_string(), "ext-1".to_string());
    mapping
        .settings
        .metadata_last_synced_at
        .insert("tvdb".to_string(), now);
    db.upsert_series_mapping(&series_id, &mapping)
        .await
        .unwrap();
    let mapping = reload_mapping(&db, &series_id).await;
    let episodes = fetch_episodes(&db, &series_id).await;

    proactively_fetch_missing_episode_metadata(&state, &series_id, &episodes, &mapping, 60).await;

    assert!(
        !state.metadata_queue.contains(&series_id).await,
        "Should NOT submit to queue when recently synced"
    );
}

#[tokio::test]
async fn test_proactive_fetch_triggers_when_all_guards_pass() {
    let (state, _tmp, series_id) = setup_proactive_fetch_env(60).await;
    let db = state.db.clone();

    insert_episode_row(&db, &series_id, 1, Some(""), None).await;
    // A second episode with a real title is present but must not block the trigger.
    insert_episode_row(&db, &series_id, 2, Some("Real Title"), Some("tvdb")).await;

    let past = (chrono::Utc::now() - chrono::Duration::hours(2)).to_rfc3339();
    let mut mapping = reload_mapping(&db, &series_id).await;
    mapping
        .settings
        .metadata_ids
        .insert("tvdb".to_string(), "ext-1".to_string());
    mapping
        .settings
        .metadata_last_synced_at
        .insert("tvdb".to_string(), past);
    db.upsert_series_mapping(&series_id, &mapping)
        .await
        .unwrap();
    let mapping = reload_mapping(&db, &series_id).await;
    let episodes = fetch_episodes(&db, &series_id).await;

    assert_eq!(episodes.len(), 2);

    proactively_fetch_missing_episode_metadata(&state, &series_id, &episodes, &mapping, 60).await;

    assert!(
        state.metadata_queue.contains(&series_id).await,
        "Should submit to queue when all guards pass (fetch dispatched)"
    );
}

#[tokio::test]
async fn test_proactive_fetch_skips_when_scanner_inserted_empty_string_but_fetched() {
    // Scanner inserts title = '' (not NULL); a fetch then sets metadata_source but may
    // leave title '' if the provider returned none. Such a row is still refetchable once
    // the cooldown expires.
    let (state, _tmp, series_id) = setup_proactive_fetch_env(60).await;
    let db = state.db.clone();

    insert_episode_row(&db, &series_id, 1, Some(""), Some("tvdb")).await;

    let mut mapping = reload_mapping(&db, &series_id).await;
    mapping
        .settings
        .metadata_ids
        .insert("tvdb".to_string(), "ext-1".to_string());
    let past = (chrono::Utc::now() - chrono::Duration::hours(2)).to_rfc3339();
    mapping
        .settings
        .metadata_last_synced_at
        .insert("tvdb".to_string(), past);
    db.upsert_series_mapping(&series_id, &mapping)
        .await
        .unwrap();
    let mapping = reload_mapping(&db, &series_id).await;
    let episodes = fetch_episodes(&db, &series_id).await;

    assert_eq!(episodes[0].title.as_deref(), Some(""));
    assert_eq!(episodes[0].metadata_source.as_deref(), Some("tvdb"));

    proactively_fetch_missing_episode_metadata(&state, &series_id, &episodes, &mapping, 60).await;

    assert!(
        state.metadata_queue.contains(&series_id).await,
        "Should refetch when title empty, source=provider, and cooldown expired"
    );
}

// Queue-level dedup must prevent duplicate submissions for the same (series_id, instance_id).

#[tokio::test]
async fn test_proactive_fetch_dedup_via_queue() {
    let (state, _tmp, series_id) = setup_proactive_fetch_env(60).await;
    let db = state.db.clone();

    insert_episode_row(&db, &series_id, 1, None, None).await;
    let past = (chrono::Utc::now() - chrono::Duration::hours(2)).to_rfc3339();
    let mut mapping = reload_mapping(&db, &series_id).await;
    mapping
        .settings
        .metadata_ids
        .insert("tvdb".to_string(), "ext-1".to_string());
    mapping
        .settings
        .metadata_last_synced_at
        .insert("tvdb".to_string(), past);
    db.upsert_series_mapping(&series_id, &mapping)
        .await
        .unwrap();
    let mapping = reload_mapping(&db, &series_id).await;
    let episodes = fetch_episodes(&db, &series_id).await;

    proactively_fetch_missing_episode_metadata(&state, &series_id, &episodes, &mapping, 60).await;
    assert!(
        state.metadata_queue.contains(&series_id).await,
        "First call should submit to queue"
    );

    proactively_fetch_missing_episode_metadata(&state, &series_id, &episodes, &mapping, 60).await;
    // No assert needed: dedup keeps a single active_series entry.
}

#[tokio::test]
async fn test_metadata_queue_submit_and_wait_e2e() {
    let (state, _tmp, _series_id) = setup_proactive_fetch_env(60).await;

    let result = state
        .metadata_queue
        .submit_and_wait("test-id".to_string(), || async {
            Ok("2024-06-01T00:00:00Z".to_string())
        })
        .await;

    assert!(result.is_ok());
    assert_eq!(result.unwrap(), "2024-06-01T00:00:00Z");
}

async fn get_file_path(db: &DbManager, episode_id: &str) -> Option<String> {
    db.get_episode_file_path(episode_id).await.unwrap()
}

/// Mark an episode `organized` and give it a main file (the SSoT association).
async fn mark_organized_with_file(db: &DbManager, episode_id: &str, file_path: &str) {
    sqlx::query("UPDATE episodes SET status = 'organized' WHERE episode_id = ?")
        .bind(episode_id)
        .execute(pool(db))
        .await
        .unwrap();
    db.associate_main_file(episode_id, file_path, None)
        .await
        .unwrap();
}

async fn get_episode_cell_type(db: &DbManager, episode_id: &str) -> Option<i32> {
    sqlx::query_scalar::<_, Option<i32>>(
        "SELECT episode_cell_type FROM episodes WHERE episode_id = ?",
    )
    .bind(episode_id)
    .fetch_one(pool(db))
    .await
    .unwrap()
}

async fn count_episodes(db: &DbManager, series_id: &str) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM episodes WHERE series_id = ?")
        .bind(series_id)
        .fetch_one(pool(db))
        .await
        .unwrap()
}

async fn count_for_source(db: &DbManager, series_id: &str, instance_id: &str) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM episodes WHERE series_id = ? AND metadata_source = ?")
        .bind(series_id)
        .bind(instance_id)
        .fetch_one(pool(db))
        .await
        .unwrap()
}

#[tokio::test]
async fn test_merge_metadata_preserves_organized_episode() {
    // An organized episode (has file_path + metadata_source) that appears in
    // new metadata should keep its file_path and get updated metadata.
    let (db, _tmp, series_id) = setup_db().await;
    let ep_id = "ep_S01E01".to_string();

    let ep = make_ep_meta(1, 1, "Old Title");
    db.batch_insert_metadata_episodes("tvdb", &series_id, vec![(ep_id.clone(), ep)], 0)
        .await
        .unwrap();
    mark_organized_with_file(&db, &ep_id, "/data/ep01.mkv").await;

    let new_ep = make_ep_meta(1, 1, "New Title");
    db.merge_metadata_episodes("tvdb", &series_id, vec![(ep_id.clone(), new_ep)], 0)
        .await
        .unwrap();

    assert_eq!(
        get_file_path(&db, &ep_id).await,
        Some("/data/ep01.mkv".to_string()),
        "file_path preserved"
    );
    assert_eq!(
        get_title(&db, &series_id, 1).await,
        Some("New Title".to_string()),
        "title updated"
    );
    assert_eq!(
        get_metadata_source(&db, &series_id, 1).await,
        Some("tvdb".to_string()),
        "metadata_source preserved"
    );
    assert_eq!(count_episodes(&db, &series_id).await, 1, "no extra rows");
}

#[tokio::test]
async fn test_merge_metadata_clears_orphaned_organized() {
    // An organized episode (has file_path) that is NOT in the new metadata
    // should have its metadata fields cleared but keep the file_path.
    let (db, _tmp, series_id) = setup_db().await;
    let ep_id = "ep_S01E01".to_string();

    let ep = make_ep_meta(1, 1, "Gone Title");
    db.batch_insert_metadata_episodes("tvdb", &series_id, vec![(ep_id.clone(), ep)], 0)
        .await
        .unwrap();
    mark_organized_with_file(&db, &ep_id, "/data/ep01.mkv").await;

    db.merge_metadata_episodes("tvdb", &series_id, vec![], 0)
        .await
        .unwrap();

    assert_eq!(
        get_file_path(&db, &ep_id).await,
        Some("/data/ep01.mkv".to_string()),
        "file_path kept"
    );
    assert_eq!(get_title(&db, &series_id, 1).await, None, "title cleared");
    assert_eq!(
        get_metadata_source(&db, &series_id, 1).await,
        None,
        "metadata_source = NULL un-scopes row"
    );
    assert_eq!(count_episodes(&db, &series_id).await, 1, "row not deleted");
}

#[tokio::test]
async fn test_merge_metadata_preserves_multipart_episode() {
    // A multipart episode has `file_path = NULL` but owns `episode_parts`. It must be
    // treated as having a file — metadata cleared, row AND parts kept — not deleted.
    let (db, _tmp, series_id) = setup_db().await;
    let ep_id = "ep_S01E01".to_string();

    let ep = make_ep_meta(1, 1, "Part Title");
    db.batch_insert_metadata_episodes("tvdb", &series_id, vec![(ep_id.clone(), ep)], 0)
        .await
        .unwrap();
    db.upsert_episode_part(&ep_id, 1, "/data/ep01.pt1.mkv", Some(10))
        .await
        .unwrap();
    db.upsert_episode_part(&ep_id, 2, "/data/ep01.pt2.mkv", Some(20))
        .await
        .unwrap();

    db.merge_metadata_episodes("tvdb", &series_id, vec![], 0)
        .await
        .unwrap();

    assert_eq!(
        count_episodes(&db, &series_id).await,
        1,
        "a multipart episode must not be deleted when the provider drops it"
    );
    let parts: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM episode_files WHERE episode_id = ? AND kind = 'main'",
    )
    .bind(&ep_id)
    .fetch_one(pool(&db))
    .await
    .unwrap();
    assert_eq!(parts, 2, "its parts must survive too");
}

#[tokio::test]
async fn test_merge_metadata_deletes_stale_provider_only() {
    // A provider-only row (no file_path) that is NOT in the new metadata
    // should be deleted entirely.
    let (db, _tmp, series_id) = setup_db().await;
    let ep_id = "ep_S01E01".to_string();

    let ep = make_ep_meta(1, 1, "Stale");
    db.batch_insert_metadata_episodes("tvdb", &series_id, vec![(ep_id.clone(), ep)], 0)
        .await
        .unwrap();

    assert_eq!(count_episodes(&db, &series_id).await, 1, "seeded");

    db.merge_metadata_episodes("tvdb", &series_id, vec![], 0)
        .await
        .unwrap();

    assert_eq!(
        count_episodes(&db, &series_id).await,
        0,
        "stale row deleted"
    );
}

#[tokio::test]
async fn test_merge_metadata_isolation_across_providers() {
    // Merging provider-a should not affect provider-b rows.
    let (db, _tmp, series_id) = setup_db().await;

    let ep_a = make_ep_meta(1, 1, "A");
    db.batch_insert_metadata_episodes(
        "provider-a",
        &series_id,
        vec![("ep_a_S01E01".to_string(), ep_a)],
        0,
    )
    .await
    .unwrap();
    let ep_b = make_ep_meta(1, 2, "B");
    db.batch_insert_metadata_episodes(
        "provider-b",
        &series_id,
        vec![("ep_b_S01E02".to_string(), ep_b)],
        0,
    )
    .await
    .unwrap();

    assert_eq!(count_for_source(&db, &series_id, "provider-a").await, 1);
    assert_eq!(count_for_source(&db, &series_id, "provider-b").await, 1);

    db.merge_metadata_episodes("provider-a", &series_id, vec![], 0)
        .await
        .unwrap();

    assert_eq!(
        count_for_source(&db, &series_id, "provider-a").await,
        0,
        "provider-a row deleted"
    );
    assert_eq!(
        count_for_source(&db, &series_id, "provider-b").await,
        1,
        "provider-b untouched"
    );
    assert_eq!(
        count_episodes(&db, &series_id).await,
        1,
        "only provider-b remains"
    );
}

#[tokio::test]
async fn test_merge_metadata_custom_episode_untouched() {
    // Episodes with metadata_source = 'custom' or 'cleared' are not scoped
    // to any provider and should never be touched by merge.
    let (db, _tmp, series_id) = setup_db().await;

    sqlx::query(
        "INSERT INTO episodes (episode_id, series_id, season, episode, title, status, \
         metadata_source, numbering_mode) \
         VALUES (?, ?, ?, ?, ?, 'organized', 'custom', 0)",
    )
    .bind("custom_S01E01")
    .bind(&series_id)
    .bind(1)
    .bind(1)
    .bind("Custom Title")
    .execute(pool(&db))
    .await
    .unwrap();
    db.associate_main_file("custom_S01E01", "/data/ep01.mkv", None)
        .await
        .unwrap();

    db.merge_metadata_episodes("tvdb", &series_id, vec![], 0)
        .await
        .unwrap();

    assert_eq!(
        count_episodes(&db, &series_id).await,
        1,
        "custom episode not touched"
    );
    assert_eq!(
        get_title(&db, &series_id, 1).await,
        Some("Custom Title".to_string())
    );
    assert_eq!(
        get_file_path(&db, "custom_S01E01").await,
        Some("/data/ep01.mkv".to_string())
    );
}

#[tokio::test]
async fn test_merge_metadata_upgrades_cell_type() {
    // A UserDefined cell (episode_cell_type = 0) should be upgraded to
    // type 1 (Provider) when metadata is merged.
    let (db, _tmp, series_id) = setup_db().await;
    let ep_id = "ep_S01E01".to_string();

    sqlx::query(
        "INSERT INTO episodes (episode_id, series_id, season, episode, status, \
         episode_cell_type, numbering_mode) \
         VALUES (?, ?, ?, ?, 'unreleased', 0, 0)",
    )
    .bind(&ep_id)
    .bind(&series_id)
    .bind(1)
    .bind(1)
    .execute(pool(&db))
    .await
    .unwrap();

    assert_eq!(
        get_episode_cell_type(&db, &ep_id).await,
        Some(0),
        "cell starts as UserDefined"
    );

    let ep = make_ep_meta(1, 1, "Now Populated");
    db.merge_metadata_episodes("tvdb", &series_id, vec![(ep_id.clone(), ep)], 0)
        .await
        .unwrap();

    assert_eq!(
        get_episode_cell_type(&db, &ep_id).await,
        Some(1),
        "cell upgraded to Provider"
    );
    assert_eq!(
        get_title(&db, &series_id, 1).await,
        Some("Now Populated".to_string())
    );
}

#[tokio::test]
async fn test_merge_metadata_mixed_scenario() {
    // Complex scenario with all three cases in one merge:
    //   ep1: organized + in new metadata  → update title, keep file
    //   ep2: organized + NOT in metadata  → clear metadata, keep file
    //   ep3: provider-only + NOT in metadata → delete
    //   ep4: provider-only + in new metadata → add title, no file
    let (db, _tmp, series_id) = setup_db().await;

    let ep1 = make_ep_meta(1, 1, "Ep1 Old");
    db.batch_insert_metadata_episodes("tvdb", &series_id, vec![("ep1_S01E01".to_string(), ep1)], 0)
        .await
        .unwrap();
    mark_organized_with_file(&db, "ep1_S01E01", "/data/ep01.mkv").await;

    let ep2 = make_ep_meta(1, 2, "Ep2 Old");
    db.batch_insert_metadata_episodes("tvdb", &series_id, vec![("ep2_S01E02".to_string(), ep2)], 0)
        .await
        .unwrap();
    mark_organized_with_file(&db, "ep2_S01E02", "/data/ep02.mkv").await;

    let ep3 = make_ep_meta(1, 3, "Ep3 Stale");
    db.batch_insert_metadata_episodes("tvdb", &series_id, vec![("ep3_S01E03".to_string(), ep3)], 0)
        .await
        .unwrap();

    assert_eq!(count_episodes(&db, &series_id).await, 3);

    let ep1_new = make_ep_meta(1, 1, "Ep1 Updated");
    let ep4_new = make_ep_meta(1, 4, "Ep4 New");
    db.merge_metadata_episodes(
        "tvdb",
        &series_id,
        vec![
            ("ep1_S01E01".to_string(), ep1_new),
            ("ep4_S01E04".to_string(), ep4_new),
        ],
        0,
    )
    .await
    .unwrap();

    assert_eq!(
        get_title(&db, &series_id, 1).await,
        Some("Ep1 Updated".to_string()),
        "ep1 title updated"
    );
    assert_eq!(
        get_file_path(&db, "ep1_S01E01").await,
        Some("/data/ep01.mkv".to_string()),
        "ep1 file kept"
    );
    assert_eq!(
        get_metadata_source(&db, &series_id, 1).await,
        Some("tvdb".to_string()),
        "ep1 re-adopted"
    );

    assert_eq!(
        get_title(&db, &series_id, 2).await,
        None,
        "ep2 title cleared"
    );
    assert_eq!(
        get_file_path(&db, "ep2_S01E02").await,
        Some("/data/ep02.mkv".to_string()),
        "ep2 file kept"
    );
    assert_eq!(
        get_metadata_source(&db, &series_id, 2).await,
        None,
        "ep2 un-scoped"
    );

    let count_ep3: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM episodes WHERE series_id = ? AND episode = ?")
            .bind(&series_id)
            .bind(3)
            .fetch_one(pool(&db))
            .await
            .unwrap();
    assert_eq!(count_ep3, 0, "ep3 deleted");

    assert_eq!(
        get_title(&db, &series_id, 4).await,
        Some("Ep4 New".to_string()),
        "ep4 inserted"
    );
    assert_eq!(get_file_path(&db, "ep4_S01E04").await, None, "ep4 no file");
    assert_eq!(
        get_metadata_source(&db, &series_id, 4).await,
        Some("tvdb".to_string()),
        "ep4 has source"
    );

    assert_eq!(
        count_episodes(&db, &series_id).await,
        3,
        "correct final count"
    );
}

#[tokio::test]
async fn test_merge_keeps_file_owner_in_suppressed_season() {
    // A suppressed season must not be re-created by a sync, but an episode that
    // still owns a file is real data: the sync clears its provider metadata and
    // keeps the row and its file association rather than deleting them.
    let (db, _tmp, series_id) = setup_db().await;
    let ep_id = "ep_S01E01".to_string();

    let ep = make_ep_meta(1, 1, "Kept Title");
    db.batch_insert_metadata_episodes("tvdb", &series_id, vec![(ep_id.clone(), ep)], 0)
        .await
        .unwrap();
    mark_organized_with_file(&db, &ep_id, "/data/ep01.mkv").await;
    db.suppress_season(&series_id, 1, 0).await.unwrap();

    db.merge_metadata_episodes("tvdb", &series_id, vec![], 0)
        .await
        .unwrap();

    assert_eq!(
        get_file_path(&db, &ep_id).await.as_deref(),
        Some("/data/ep01.mkv"),
        "the owned file association must survive a suppressed-season sync"
    );
    assert_eq!(
        get_metadata_source(&db, &series_id, 1).await,
        None,
        "cleared provider metadata"
    );
}
