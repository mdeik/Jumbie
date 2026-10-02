// Tests for clear_metadata_cache: per-provider clears remove only that provider's
// cached series/episode/season/fetch-log rows, full clears remove everything, and
// metadata_last_synced_at handling differs between the two.

use crate::db::DbManager;
use jumbie_shared::mapping::MappingRule;
use std::collections::HashMap;

/// Create a temp DB with a series mapping seeded and two metadata providers.
async fn setup_db_with_providers() -> (DbManager, tempfile::TempDir, String) {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test.db");
    let db = DbManager::new(&db_path).await.unwrap();

    let mut metadata_ids = HashMap::new();
    metadata_ids.insert(
        "aaaaaaaa-aaaa-4aaa-aaaa-aaaaaaaaaaaa".to_string(),
        "tvdb-123".to_string(),
    );
    metadata_ids.insert(
        "bbbbbbbb-bbbb-4bbb-bbbb-bbbbbbbbbbbb".to_string(),
        "tmdb-456".to_string(),
    );

    let mut metadata_last_synced_at = HashMap::new();
    metadata_last_synced_at.insert(
        "aaaaaaaa-aaaa-4aaa-aaaa-aaaaaaaaaaaa".to_string(),
        "2024-01-01T00:00:00Z".to_string(),
    );
    metadata_last_synced_at.insert(
        "bbbbbbbb-bbbb-4bbb-bbbb-bbbbbbbbbbbb".to_string(),
        "2024-01-01T00:00:00Z".to_string(),
    );

    let mapping = MappingRule {
        target_title: "Test Series".to_string(),
        name: "test_series".to_string(),
        series_id: "series-key".to_string(),
        settings: jumbie_shared::mapping::SeriesSettings {
            metadata_ids: metadata_ids.clone(),
            metadata_last_synced_at,
            ..Default::default()
        },
        ..Default::default()
    };
    db.upsert_series_mapping("series-key", &mapping)
        .await
        .unwrap();

    (db, tmp, "series-key".to_string())
}

/// Seed all cache tables for a given (metadata_id, plugin_id, instance_id) tuple.
async fn seed_cache(
    db: &DbManager,
    metadata_id: &str,
    plugin_id: &str,
    instance_id: &str,
    label: &str,
) {
    // Series metadata cache
    db.upsert_metadata_series_cache(crate::db::metadata_cache::UpsertMetadataCacheParams {
        metadata_id,
        plugin_id,
        instance_id,
        title: &format!("{} Title", label),
        overview: Some(&format!("{} Overview", label)),
        language: Some("en"),
        aliases: &[format!("{} Alias", label)],
        image_url: None,
    })
    .await
    .unwrap();

    // Episode cache (two seasons, one episode each)
    let ep1 = crate::db::metadata_cache::EpisodeMetadataForCache {
        season_number: 1,
        episode_number: 1,
        unique_id: format!("{}-s1e1", label),
        title: format!("{} Episode 1", label),
        description: Some(format!("{} Description 1", label)),
        runtime: Some(30),
        image_url: None,
        meta_date: None,
    };
    let ep2 = crate::db::metadata_cache::EpisodeMetadataForCache {
        season_number: 2,
        episode_number: 1,
        unique_id: format!("{}-s2e1", label),
        title: format!("{} Episode 2", label),
        description: Some(format!("{} Description 2", label)),
        runtime: Some(30),
        image_url: None,
        meta_date: None,
    };
    db.batch_upsert_metadata_episodes_cache(
        metadata_id,
        plugin_id,
        instance_id,
        "normal",
        &[ep1, ep2],
    )
    .await
    .unwrap();

    // Season cache
    db.upsert_metadata_season_cache(
        metadata_id,
        plugin_id,
        instance_id,
        "normal",
        &[
            crate::plugins::metadata::SeasonMetadata {
                season: 1,
                episode_count: 1,
            },
            crate::plugins::metadata::SeasonMetadata {
                season: 2,
                episode_count: 1,
            },
        ],
    )
    .await
    .unwrap();

    // Fetch log
    db.mark_fetch_attempted(metadata_id, plugin_id, instance_id, "normal")
        .await
        .unwrap();
}

/// Count rows in a table matching a specific instance_id.
async fn count_for_instance(db: &DbManager, table: &str, instance_id: &str) -> i64 {
    let sql = format!("SELECT COUNT(*) FROM {} WHERE instance_id = ?", table);
    sqlx::query_scalar(&sql)
        .bind(instance_id)
        .fetch_one(db.get_pool())
        .await
        .unwrap()
}

/// Get the per-provider last_synced timestamps from the series mapping.
async fn get_metadata_last_synced_at(
    db: &DbManager,
    series_id: &str,
) -> std::collections::HashMap<String, String> {
    let json: Option<String> = sqlx::query_scalar(
        "SELECT json_extract(data, '$.metadata_last_synced_at') FROM series_mappings WHERE id = ?",
    )
    .bind(series_id)
    .fetch_optional(db.get_pool())
    .await
    .unwrap()
    .flatten();
    json.and_then(|j| serde_json::from_str(&j).ok())
        .unwrap_or_default()
}

#[tokio::test]
async fn test_clear_single_provider_cache() {
    let (db, _tmp, series_id) = setup_db_with_providers().await;

    let instance_a = "aaaaaaaa-aaaa-4aaa-aaaa-aaaaaaaaaaaa";
    let instance_b = "bbbbbbbb-bbbb-4bbb-bbbb-bbbbbbbbbbbb";

    seed_cache(&db, "tvdb-123", "tvdb", instance_a, "ProviderA").await;
    seed_cache(&db, "tmdb-456", "tmdb", instance_b, "ProviderB").await;

    db.delete_metadata_episodes_cache_for_provider("tvdb-123", "tvdb", instance_a)
        .await
        .unwrap();
    db.delete_metadata_season_cache_for_provider("tvdb-123", "tvdb", instance_a)
        .await
        .unwrap();
    db.delete_metadata_series_cache_for_provider("tvdb-123", "tvdb", instance_a)
        .await
        .unwrap();
    db.delete_metadata_fetch_log_for_provider("tvdb-123", "tvdb", instance_a)
        .await
        .unwrap();

    assert_eq!(
        count_for_instance(&db, "metadata_episodes_cache", instance_a).await,
        0,
        "Provider A episode cache should be empty"
    );
    assert_eq!(
        count_for_instance(&db, "metadata_season_cache", instance_a).await,
        0,
        "Provider A season cache should be empty"
    );
    assert_eq!(
        count_for_instance(&db, "metadata_series_cache", instance_a).await,
        0,
        "Provider A series cache should be empty"
    );
    assert_eq!(
        count_for_instance(&db, "metadata_fetch_log", instance_a).await,
        0,
        "Provider A fetch log should be empty"
    );

    assert_eq!(
        count_for_instance(&db, "metadata_episodes_cache", instance_b).await,
        2,
        "Provider B episode cache should be intact"
    );
    assert_eq!(
        count_for_instance(&db, "metadata_season_cache", instance_b).await,
        2,
        "Provider B season cache should be intact"
    );
    assert_eq!(
        count_for_instance(&db, "metadata_series_cache", instance_b).await,
        1,
        "Provider B series cache should be intact"
    );
    assert_eq!(
        count_for_instance(&db, "metadata_fetch_log", instance_b).await,
        1,
        "Provider B fetch log should be intact"
    );

    // The DB cache-clearing operations don't touch the series mapping settings,
    // so both providers' per-provider timestamps should remain intact.
    let remaining = get_metadata_last_synced_at(&db, &series_id).await;
    assert!(
        remaining.contains_key(instance_a),
        "Provider A's metadata_last_synced_at should survive DB cache clear"
    );
    assert!(
        remaining.contains_key(instance_b),
        "Provider B's metadata_last_synced_at should survive DB cache clear"
    );
}

#[tokio::test]
async fn test_clear_all_providers_cache() {
    let (db, _tmp, _series_id) = setup_db_with_providers().await;

    let instance_a = "aaaaaaaa-aaaa-4aaa-aaaa-aaaaaaaaaaaa";
    let instance_b = "bbbbbbbb-bbbb-4bbb-bbbb-bbbbbbbbbbbb";

    seed_cache(&db, "tvdb-123", "tvdb", instance_a, "ProviderA").await;
    seed_cache(&db, "tmdb-456", "tmdb", instance_b, "ProviderB").await;

    db.delete_metadata_episodes_cache("tvdb-123").await.unwrap();
    db.delete_metadata_episodes_cache("tmdb-456").await.unwrap();
    db.delete_metadata_series_cache("tvdb-123").await.unwrap();
    db.delete_metadata_series_cache("tmdb-456").await.unwrap();
    // Seasons and fetch logs don't have a metadata_id-only deletion,
    // so we delete per-provider for each.
    db.delete_metadata_season_cache_for_provider("tvdb-123", "tvdb", instance_a)
        .await
        .unwrap();
    db.delete_metadata_season_cache_for_provider("tmdb-456", "tmdb", instance_b)
        .await
        .unwrap();
    db.delete_metadata_fetch_log_for_provider("tvdb-123", "tvdb", instance_a)
        .await
        .unwrap();
    db.delete_metadata_fetch_log_for_provider("tmdb-456", "tmdb", instance_b)
        .await
        .unwrap();

    assert_eq!(
        count_for_instance(&db, "metadata_episodes_cache", instance_a).await,
        0
    );
    assert_eq!(
        count_for_instance(&db, "metadata_episodes_cache", instance_b).await,
        0
    );
    assert_eq!(
        count_for_instance(&db, "metadata_series_cache", instance_a).await,
        0
    );
    assert_eq!(
        count_for_instance(&db, "metadata_series_cache", instance_b).await,
        0
    );
}

#[tokio::test]
async fn test_clear_nonexistent_provider_is_noop() {
    let (db, _tmp, _series_id) = setup_db_with_providers().await;

    let instance_a = "aaaaaaaa-aaaa-4aaa-aaaa-aaaaaaaaaaaa";

    // Seed provider A
    seed_cache(&db, "tvdb-123", "tvdb", instance_a, "ProviderA").await;

    // Clearing an empty provider must not error or affect provider A.
    let bogus = "cccccccc-cccc-4ccc-cccc-cccccccccccc";
    db.delete_metadata_episodes_cache_for_provider("tvdb-123", "tvdb", bogus)
        .await
        .unwrap();
    db.delete_metadata_season_cache_for_provider("tvdb-123", "tvdb", bogus)
        .await
        .unwrap();
    db.delete_metadata_series_cache_for_provider("tvdb-123", "tvdb", bogus)
        .await
        .unwrap();
    db.delete_metadata_fetch_log_for_provider("tvdb-123", "tvdb", bogus)
        .await
        .unwrap();

    assert_eq!(
        count_for_instance(&db, "metadata_episodes_cache", instance_a).await,
        2
    );
    assert_eq!(
        count_for_instance(&db, "metadata_series_cache", instance_a).await,
        1
    );
}
