mod common;

use axum::http::StatusCode;
use std::sync::Arc;
use tower::ServiceExt;

/// Helper: inject the mock metadata plugin into the plugin manager and
/// set the metadata_id on the series so the handler can find it.
async fn setup_mock_metadata_plugin(
    state: &Arc<jumbie::api::AppState>,
    series_id: &str,
    plugin_instance_id: &str,
) {
    state
        .plugin_manager
        .write()
        .await
        .add_internal_plugin(Arc::new(common::MockMetadataPlugin {
            instance_id: plugin_instance_id.to_string(),
            display_name: "Mock Metadata".to_string(),
            capabilities: vec![
                jumbie_shared::plugin::Capability::MetadataProviderNormal,
                jumbie_shared::plugin::Capability::FetchSeriesTitle,
                jumbie_shared::plugin::Capability::FetchSeriesAliases,
            ],
            series_identifier_label: Some("Mock ID".to_string()),
            series_name: "Provider Title".to_string(),
            overview: "A test series overview.".to_string(),
            aliases: vec!["Alt Name".to_string(), "Another Alt".to_string()],
            episodes: vec![],
        }));

    // Set the metadata_id on the series mapping so the handler can look it up
    if let Ok(Some(mut mapping)) = state.db.get_series_mapping(series_id).await {
        mapping
            .settings
            .metadata_ids
            .insert(plugin_instance_id.to_string(), "test_123".to_string());
        state
            .db
            .upsert_series_mapping(series_id, &mapping)
            .await
            .unwrap();
    }
}

/// Helper: inject the empty-aliases mock metadata plugin.
async fn setup_mock_metadata_plugin_empty_aliases(
    state: &Arc<jumbie::api::AppState>,
    series_id: &str,
    plugin_instance_id: &str,
) {
    state
        .plugin_manager
        .write()
        .await
        .add_internal_plugin(Arc::new(common::MockMetadataPlugin {
            instance_id: plugin_instance_id.to_string(),
            display_name: "Mock Metadata (No Aliases)".to_string(),
            capabilities: vec![
                jumbie_shared::plugin::Capability::MetadataProviderNormal,
                // Supports the capability but returns an empty alias list — the
                // fixture models "provider has no aliases", NOT "provider
                // doesn't support aliases" (the endpoint now guards the latter).
                jumbie_shared::plugin::Capability::FetchSeriesAliases,
            ],
            series_identifier_label: Some("Mock ID".to_string()),
            series_name: "Provider Title".to_string(),
            overview: "A test series overview.".to_string(),
            aliases: vec![],
            episodes: vec![],
        }));

    if let Ok(Some(mut mapping)) = state.db.get_series_mapping(series_id).await {
        mapping
            .settings
            .metadata_ids
            .insert(plugin_instance_id.to_string(), "test_123".to_string());
        state
            .db
            .upsert_series_mapping(series_id, &mapping)
            .await
            .unwrap();
    }
}

// Tests

#[tokio::test]
async fn test_fetch_series_aliases_injects_provider_title() {
    let (app, state, _temp_dir) = common::setup_test_app().await;

    // Create a series
    let series_id = common::create_test_series(&app, "My Series").await;

    // Inject mock metadata plugin and set metadata_id
    let plugin_instance_id = uuid::Uuid::new_v4().to_string();
    setup_mock_metadata_plugin(&state, &series_id, &plugin_instance_id).await;

    // Call fetch_series_aliases
    let res = app
        .clone()
        .oneshot(common::post_empty_request(&format!(
            "/api/series/{}/fetch_series_aliases",
            series_id
        )))
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        StatusCode::OK,
        "fetch_series_aliases should return 200 OK"
    );

    // The response body returns the merged list (with provider title injected first).
    let body: serde_json::Value = common::response_json(res).await;
    let response_aliases = body["aliases"]
        .as_array()
        .expect("response should contain aliases array");
    assert_eq!(
        response_aliases.as_slice(),
        &["Provider Title", "Alt Name", "Another Alt"],
        "Response contains aliases with provider title injected first"
    );

    let mapping = state
        .db
        .get_series_mapping(&series_id)
        .await
        .unwrap()
        .expect("series mapping should exist");
    assert_eq!(
        mapping.settings.aliases,
        vec![
            "Provider Title".to_string(),
            "Alt Name".to_string(),
            "Another Alt".to_string(),
        ],
        "Persisted aliases: provider_title → provider_aliases → existing_aliases"
    );
}

#[tokio::test]
async fn test_fetch_series_aliases_deduplicates_existing() {
    let (app, state, _temp_dir) = common::setup_test_app().await;

    let series_id = common::create_test_series(&app, "Dedup Series").await;

    // Pre-set some aliases on the series (including one that overlaps with provider data)
    if let Ok(Some(mut mapping)) = state.db.get_series_mapping(&series_id).await {
        mapping.settings.aliases = vec![
            "Alt Name".to_string(), // overlaps with provider alias
            "Custom Alias".to_string(),
        ];
        state
            .db
            .upsert_series_mapping(&series_id, &mapping)
            .await
            .unwrap();
    }

    let plugin_instance_id = uuid::Uuid::new_v4().to_string();
    setup_mock_metadata_plugin(&state, &series_id, &plugin_instance_id).await;

    let res = app
        .clone()
        .oneshot(common::post_empty_request(&format!(
            "/api/series/{}/fetch_series_aliases",
            series_id
        )))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let mapping = state
        .db
        .get_series_mapping(&series_id)
        .await
        .unwrap()
        .expect("series mapping should exist");
    assert_eq!(
        mapping.settings.aliases,
        vec![
            "Alt Name".to_string(),       // existing alias (deduped from new)
            "Custom Alias".to_string(),   // existing alias
            "Provider Title".to_string(), // provider title injected after existing
            "Another Alt".to_string(),    // provider alias (Alt Name deduped against existing)
        ],
        "Order: existing_aliases → provider_title → provider_aliases (deduped)"
    );
}

#[tokio::test]
async fn test_fetch_series_aliases_cache_uses_provider_title_not_mapping_title() {
    // Regression: the cache write in fetch_series_aliases used `mapping.target_title`
    // (the user's title) instead of the provider's canonical title. Because the cache
    // merge does `title = excluded.title` (no COALESCE), the next call resolved the
    // polluted title and injected the wrong first alias.
    let (app, state, _temp_dir) = common::setup_test_app().await;

    let series_id = common::create_test_series(&app, "My Custom Series").await;

    let plugin_instance_id = uuid::Uuid::new_v4().to_string();
    let metadata_id = "test_123";
    setup_mock_metadata_plugin(&state, &series_id, &plugin_instance_id).await;

    let mapping = state
        .db
        .get_series_mapping(&series_id)
        .await
        .unwrap()
        .expect("series mapping should exist");
    assert_eq!(
        mapping.target_title, "My Custom Series",
        "Pre-condition: mapping.target_title should be the user's custom title"
    );

    // fetch_series_aliases must cache the provider title, not 'My Custom Series'.
    let res = app
        .clone()
        .oneshot(common::post_empty_request(&format!(
            "/api/series/{}/fetch_series_aliases",
            series_id
        )))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let cached = state
        .db
        .get_metadata_series_cache(metadata_id, &plugin_instance_id, &plugin_instance_id)
        .await
        .unwrap()
        .expect("series metadata cache should exist after fetch_series_aliases");
    assert_eq!(
        cached.title, "Provider Title",
        "Cache should store the provider's canonical title, not the mapping's target_title"
    );

    // Call fetch_series_aliases again — this will hit the cache.  Verify the
    // injected first alias is the provider's title, not the user's custom title.
    let res2 = app
        .clone()
        .oneshot(common::post_empty_request(&format!(
            "/api/series/{}/fetch_series_aliases",
            series_id
        )))
        .await
        .unwrap();
    assert_eq!(res2.status(), StatusCode::OK);
    let body2: serde_json::Value = common::response_json(res2).await;
    let response_aliases = body2["aliases"]
        .as_array()
        .expect("response should contain aliases array");
    assert!(
        !response_aliases.is_empty(),
        "Second call should return aliases from cache"
    );
    assert_eq!(
        response_aliases[0].as_str().unwrap(),
        "Provider Title",
        "First alias should be the provider's canonical title, not the user's custom title"
    );
    assert!(
        !response_aliases
            .iter()
            .any(|a| a.as_str() == Some("My Custom Series")),
        "The user's custom title 'My Custom Series' should NOT appear as an alias"
    );

    // bug caused the cache-hit to return aliases without persisting them to
    // the mapping, so they'd disappear on refresh.
    let mapping2 = state
        .db
        .get_series_mapping(&series_id)
        .await
        .unwrap()
        .expect("series mapping should exist");
    assert!(
        !mapping2.settings.aliases.is_empty(),
        "Mapping aliases should be persisted even on cache-hit path"
    );
    assert_eq!(
        mapping2.settings.aliases.first().unwrap(),
        "Provider Title",
        "First alias in mapping should be the provider's canonical title"
    );
}

#[tokio::test]
async fn test_fetch_series_aliases_persists_after_fetch_series_info() {
    // Regression: "Fetch Title" populates the metadata cache, so a following "Fetch
    // Aliases" hit the cache and returned early WITHOUT persisting to the mapping —
    // aliases showed in the UI but vanished on refresh.
    let (app, state, _temp_dir) = common::setup_test_app().await;

    let series_id = common::create_test_series(&app, "Test Series").await;

    let plugin_instance_id = uuid::Uuid::new_v4().to_string();
    setup_mock_metadata_plugin(&state, &series_id, &plugin_instance_id).await;

    let payload = serde_json::json!({
        "update_title": true,
        "merge_aliases": false,
    });
    let res = app
        .clone()
        .oneshot(common::post_json_request(
            &format!("/api/series/{}/fetch_series_info", series_id),
            &payload,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // "Fetch Aliases" now hits the populated cache path; it must still persist to
    // the mapping (before the fix it only returned aliases to the frontend).
    let res2 = app
        .clone()
        .oneshot(common::post_empty_request(&format!(
            "/api/series/{}/fetch_series_aliases",
            series_id
        )))
        .await
        .unwrap();
    assert_eq!(res2.status(), StatusCode::OK);
    let body2: serde_json::Value = common::response_json(res2).await;
    assert!(
        body2["aliases"]
            .as_array()
            .map(|a| !a.is_empty())
            .unwrap_or(false),
        "fetch_series_aliases should return aliases"
    );

    // caught the bug.
    let mapping = state
        .db
        .get_series_mapping(&series_id)
        .await
        .unwrap()
        .expect("series mapping should exist");
    assert!(
        !mapping.settings.aliases.is_empty(),
        "Mapping aliases must be persisted after fetch_series_aliases, even when cache-hit"
    );
}

#[tokio::test]
async fn test_fetch_series_aliases_skips_title_when_no_aliases() {
    // Regression: with no provider aliases and none stored, the provider title must
    // NOT be injected as a lone alias (previously it always was, polluting the list).
    let (app, state, _temp_dir) = common::setup_test_app().await;

    let series_id = common::create_test_series(&app, "No Alias Series").await;

    let plugin_instance_id = uuid::Uuid::new_v4().to_string();
    setup_mock_metadata_plugin_empty_aliases(&state, &series_id, &plugin_instance_id).await;

    let res = app
        .clone()
        .oneshot(common::post_empty_request(&format!(
            "/api/series/{}/fetch_series_aliases",
            series_id
        )))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // Response should contain no aliases
    let body: serde_json::Value = common::response_json(res).await;
    let response_aliases = body["aliases"]
        .as_array()
        .expect("response should contain aliases array");
    assert!(
        response_aliases.is_empty(),
        "Response aliases should be empty when no aliases exist"
    );

    let mapping = state
        .db
        .get_series_mapping(&series_id)
        .await
        .unwrap()
        .expect("series mapping should exist");
    assert!(
        mapping.settings.aliases.is_empty(),
        "Mapping aliases should remain empty when no aliases exist"
    );
}

#[tokio::test]
async fn test_fetch_series_info_with_merge_aliases_injects_provider_title() {
    let (app, state, _temp_dir) = common::setup_test_app().await;

    let series_id = common::create_test_series(&app, "Info Test").await;

    let plugin_instance_id = uuid::Uuid::new_v4().to_string();
    setup_mock_metadata_plugin(&state, &series_id, &plugin_instance_id).await;

    // Call fetch_series_info with merge_aliases=true and update_title=true
    let payload = serde_json::json!({
        "update_title": true,
        "merge_aliases": true,
    });
    let res = app
        .clone()
        .oneshot(common::post_json_request(
            &format!("/api/series/{}/fetch_series_info", series_id),
            &payload,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let mapping = state
        .db
        .get_series_mapping(&series_id)
        .await
        .unwrap()
        .expect("series mapping should exist");
    assert_eq!(
        mapping.target_title, "Provider Title",
        "Series title should be updated to provider title"
    );

    // Our mock fetch_series_info returns no aliases in info.aliases map,
    // so only the provider title should be present.
    assert_eq!(
        mapping.settings.aliases,
        vec!["Provider Title".to_string()],
        "With merge_aliases, aliases should start with the provider title"
    );
}

#[tokio::test]
async fn test_fetch_series_aliases_without_metadata_id_returns_error() {
    let (app, _state, _temp_dir) = common::setup_test_app().await;

    let series_id = common::create_test_series(&app, "No Metadata").await;

    // Don't set any metadata_id — the handler should reject the request
    let res = app
        .clone()
        .oneshot(common::post_empty_request(&format!(
            "/api/series/{}/fetch_series_aliases",
            series_id
        )))
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        StatusCode::BAD_REQUEST,
        "Should fail when no metadata ID is set"
    );
}

/// Inject a metadata plugin that declares ONLY the base metadata capability
/// (no `FetchSeriesTitle` / `FetchSeriesAliases`) so the endpoint guards reject it.
async fn setup_mock_metadata_plugin_without_fetch_caps(
    state: &Arc<jumbie::api::AppState>,
    series_id: &str,
    plugin_instance_id: &str,
) {
    state
        .plugin_manager
        .write()
        .await
        .add_internal_plugin(Arc::new(common::MockMetadataPlugin {
            instance_id: plugin_instance_id.to_string(),
            display_name: "Mock Metadata (No Fetch Caps)".to_string(),
            capabilities: vec![jumbie_shared::plugin::Capability::MetadataProviderNormal],
            series_identifier_label: Some("Mock ID".to_string()),
            series_name: "Provider Title".to_string(),
            overview: String::new(),
            aliases: vec![],
            episodes: vec![],
        }));

    if let Ok(Some(mut mapping)) = state.db.get_series_mapping(series_id).await {
        mapping
            .settings
            .metadata_ids
            .insert(plugin_instance_id.to_string(), "test_123".to_string());
        state
            .db
            .upsert_series_mapping(series_id, &mapping)
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn test_fetch_series_aliases_rejected_when_provider_lacks_capability() {
    let (app, state, _temp_dir) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "No Alias Cap Series").await;

    let plugin_instance_id = uuid::Uuid::new_v4().to_string();
    setup_mock_metadata_plugin_without_fetch_caps(&state, &series_id, &plugin_instance_id).await;

    let res = app
        .clone()
        .oneshot(common::post_empty_request(&format!(
            "/api/series/{}/fetch_series_aliases",
            series_id
        )))
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        StatusCode::BAD_REQUEST,
        "provider without FetchSeriesAliases must be rejected, not called"
    );
}

#[tokio::test]
async fn test_fetch_series_info_rejected_when_provider_lacks_capability() {
    let (app, state, _temp_dir) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "No Title Cap Series").await;

    let plugin_instance_id = uuid::Uuid::new_v4().to_string();
    setup_mock_metadata_plugin_without_fetch_caps(&state, &series_id, &plugin_instance_id).await;

    let payload = serde_json::json!({ "update_title": true, "merge_aliases": false });
    let res = app
        .clone()
        .oneshot(common::post_json_request(
            &format!("/api/series/{}/fetch_series_info", series_id),
            &payload,
        ))
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        StatusCode::BAD_REQUEST,
        "provider without FetchSeriesTitle must be rejected, not called"
    );
}

/// The Fetch action targets the provider instance given in `provider_instance_id`,
/// so a multi-provider series uses that provider's config — not an arbitrary one.
#[tokio::test]
async fn test_fetch_series_aliases_targets_requested_provider() {
    use jumbie_shared::plugin::Capability;

    let (app, state, _temp_dir) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Per Provider Aliases").await;

    let pid_a = uuid::Uuid::new_v4().to_string();
    let pid_b = uuid::Uuid::new_v4().to_string();

    let caps = vec![
        Capability::MetadataProviderNormal,
        Capability::FetchSeriesTitle,
        Capability::FetchSeriesAliases,
    ];
    state
        .plugin_manager
        .write()
        .await
        .add_internal_plugin(Arc::new(common::MockMetadataPlugin {
            instance_id: pid_a.clone(),
            display_name: "Provider A".to_string(),
            capabilities: caps.clone(),
            series_identifier_label: Some("A".to_string()),
            series_name: "A Title".to_string(),
            overview: String::new(),
            aliases: vec!["A Alt".to_string()],
            episodes: vec![],
        }));
    state
        .plugin_manager
        .write()
        .await
        .add_internal_plugin(Arc::new(common::MockMetadataPlugin {
            instance_id: pid_b.clone(),
            display_name: "Provider B".to_string(),
            capabilities: caps,
            series_identifier_label: Some("B".to_string()),
            series_name: "B Title".to_string(),
            overview: String::new(),
            aliases: vec!["B Alt".to_string()],
            episodes: vec![],
        }));

    if let Ok(Some(mut mapping)) = state.db.get_series_mapping(&series_id).await {
        mapping
            .settings
            .metadata_ids
            .insert(pid_a.clone(), "id_a".to_string());
        mapping
            .settings
            .metadata_ids
            .insert(pid_b.clone(), "id_b".to_string());
        state
            .db
            .upsert_series_mapping(&series_id, &mapping)
            .await
            .unwrap();
    }

    // Target provider B explicitly.
    let res = app
        .clone()
        .oneshot(common::post_empty_request(&format!(
            "/api/series/{series_id}/fetch_series_aliases?provider_instance_id={pid_b}"
        )))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let body: serde_json::Value = common::response_json(res).await;
    let aliases: Vec<String> = body["aliases"]
        .as_array()
        .expect("aliases array")
        .iter()
        .filter_map(|v| v.as_str().map(String::from))
        .collect();
    assert!(
        aliases.contains(&"B Alt".to_string()),
        "targeted provider B's aliases should be returned"
    );
    assert!(
        !aliases.contains(&"A Alt".to_string()),
        "provider A's aliases must not leak in when B is targeted"
    );
}

/// An unknown `provider_instance_id` is rejected instead of silently falling
/// back to another provider.
#[tokio::test]
async fn test_fetch_series_aliases_unknown_provider_rejected() {
    let (app, state, _temp_dir) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Unknown Provider").await;

    let plugin_instance_id = uuid::Uuid::new_v4().to_string();
    setup_mock_metadata_plugin(&state, &series_id, &plugin_instance_id).await;

    let res = app
        .clone()
        .oneshot(common::post_empty_request(&format!(
            "/api/series/{series_id}/fetch_series_aliases?provider_instance_id=does-not-exist"
        )))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_restore_episode_metadata_existing_episode() {
    // Tests the cache-hit path for an episode that currently exists in the
    // episodes table. The handler should null out its metadata, then batch-
    // insert the provider data from the metadata_episodes_cache.
    let (app, state, _temp_dir) = common::setup_test_app().await;

    let series_id = common::create_test_series(&app, "Restore Existing").await;

    // Create an episode with custom metadata
    sqlx::query(
        "INSERT INTO episodes (episode_id, series_id, season, episode, title, \
         status, metadata_source, numbering_mode) \
         VALUES (?, ?, ?, ?, ?, 'unreleased', 'custom', 0)",
    )
    .bind(format!("{}_S01E01", series_id))
    .bind(&series_id)
    .bind(1)
    .bind(1)
    .bind("Custom Title")
    .execute(state.db.get_pool())
    .await
    .unwrap();

    // Inject mock metadata plugin and set metadata_id on the series
    let plugin_instance_id = uuid::Uuid::new_v4().to_string();
    setup_mock_metadata_plugin(&state, &series_id, &plugin_instance_id).await;

    let metadata_id = "test_123";

    // Seed the cache with metadata for this episode
    let cache_ep = jumbie::db::metadata_cache::EpisodeMetadataForCache {
        season_number: 1,
        episode_number: 1,
        unique_id: format!("{}-ext-1", plugin_instance_id),
        title: "Provider Title".to_string(),
        description: Some("Provider description".to_string()),
        runtime: Some(30),
        image_url: Some("https://example.com/ep1.jpg".to_string()),
        meta_date: None,
    };
    state
        .db
        .batch_upsert_metadata_episodes_cache(
            metadata_id,
            &plugin_instance_id,
            &plugin_instance_id,
            "normal",
            &[cache_ep],
        )
        .await
        .unwrap();

    // Call restore_episode_metadata
    let episode_id = format!("{}_S01E01", series_id);
    let res = app
        .clone()
        .oneshot(common::post_empty_request(&format!(
            "/api/series/{}/episodes/{}/restore_metadata",
            series_id, episode_id
        )))
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        StatusCode::OK,
        "restore_episode_metadata should return 200 OK"
    );

    let row: Option<(Option<String>, Option<String>)> =
        sqlx::query_as("SELECT title, metadata_source FROM episodes WHERE episode_id = ?")
            .bind(&episode_id)
            .fetch_optional(state.db.get_pool())
            .await
            .unwrap();
    let (title, source) = row.expect("Episode should still exist");
    assert_eq!(
        title,
        Some("Provider Title".to_string()),
        "Title should be restored from cache"
    );
    assert_eq!(
        source,
        Some(plugin_instance_id.clone()),
        "metadata_source should be set to the provider instance id"
    );
}

#[tokio::test]
async fn test_restore_episode_metadata_deleted_episode() {
    // Tests restoring an episode that was deleted from the episodes table,
    // but still has cached data in the metadata_episodes cache table.
    // The handler should parse the season from the episode_id and recreate
    // the row via batch_insert_metadata_episodes.
    let (app, state, _temp_dir) = common::setup_test_app().await;

    let series_id = common::create_test_series(&app, "Restore Deleted").await;

    // Create an episode and then delete it to simulate a deleted state
    let episode_id = format!("{}_S02E03", series_id);
    sqlx::query(
        "INSERT INTO episodes (episode_id, series_id, season, episode, title, \
         status, metadata_source, numbering_mode) \
         VALUES (?, ?, ?, ?, ?, 'unreleased', 'custom', 0)",
    )
    .bind(&episode_id)
    .bind(&series_id)
    .bind(2)
    .bind(3)
    .bind("Old Title")
    .execute(state.db.get_pool())
    .await
    .unwrap();

    // Delete the episode
    sqlx::query("DELETE FROM episodes WHERE episode_id = ?")
        .bind(&episode_id)
        .execute(state.db.get_pool())
        .await
        .unwrap();

    let check: Option<(String,)> =
        sqlx::query_as("SELECT episode_id FROM episodes WHERE episode_id = ?")
            .bind(&episode_id)
            .fetch_optional(state.db.get_pool())
            .await
            .unwrap();
    assert!(check.is_none(), "Episode should be deleted");

    // Inject mock metadata plugin and set metadata_id
    let plugin_instance_id = uuid::Uuid::new_v4().to_string();
    setup_mock_metadata_plugin(&state, &series_id, &plugin_instance_id).await;

    let metadata_id = "test_123";

    // Seed the cache with metadata for the deleted episode
    let cache_ep = jumbie::db::metadata_cache::EpisodeMetadataForCache {
        season_number: 2,
        episode_number: 3,
        unique_id: format!("{}-ext-23", plugin_instance_id),
        title: "Provider Title Restored".to_string(),
        description: Some("Provider description".to_string()),
        runtime: Some(45),
        image_url: Some("https://example.com/ep23.jpg".to_string()),
        meta_date: None,
    };
    state
        .db
        .batch_upsert_metadata_episodes_cache(
            metadata_id,
            &plugin_instance_id,
            &plugin_instance_id,
            "normal",
            &[cache_ep],
        )
        .await
        .unwrap();

    // Call restore_episode_metadata — this should succeed even though the
    // episode row was deleted, by parsing season from the episode_id
    let res = app
        .clone()
        .oneshot(common::post_empty_request(&format!(
            "/api/series/{}/episodes/{}/restore_metadata",
            series_id, episode_id
        )))
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        StatusCode::OK,
        "restore_episode_metadata should return 200 OK for deleted episode"
    );

    let row: Option<(Option<String>, Option<String>, i32, i32)> = sqlx::query_as(
        "SELECT title, metadata_source, season, episode FROM episodes WHERE episode_id = ?",
    )
    .bind(&episode_id)
    .fetch_optional(state.db.get_pool())
    .await
    .unwrap();
    let (title, source, season, episode) = row.expect("Episode should be recreated from cache");
    assert_eq!(
        title,
        Some("Provider Title Restored".to_string()),
        "Title should be restored from cache"
    );
    assert_eq!(
        source,
        Some(plugin_instance_id.clone()),
        "metadata_source should be set to the provider instance id"
    );
    assert_eq!(
        season, 2,
        "Season should match parsed value from episode_id"
    );
    assert_eq!(episode, 3, "Episode number should match parsed value");
}

#[tokio::test]
async fn test_custom_metadata_zone_less_is_rejected() {
    // The API boundary is strict: `meta_date` must be RFC 3339 with an explicit
    // offset. Zone-less and date-only inputs are rejected with 400 rather than
    // being silently anchored to midnight UTC.
    let (app, state, _temp_dir) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Strict Dates").await;
    let episode_id = format!("{}_S01E01", series_id);

    sqlx::query(
        "INSERT INTO episodes (episode_id, series_id, season, episode, title, \
         status, numbering_mode) \
         VALUES (?, ?, ?, ?, ?, 'unreleased', 0)",
    )
    .bind(&episode_id)
    .bind(&series_id)
    .bind(1)
    .bind(1)
    .bind("Episode")
    .execute(state.db.get_pool())
    .await
    .unwrap();

    for bad in ["2026-07-07", "2026-07-07T00:00:00", "2026-07-07 00:00:00"] {
        let (status, _) = common::put_json(
            &app,
            &format!("/api/series/{}/episodes/{}/metadata", series_id, episode_id),
            &serde_json::json!({ "meta_date": bad }),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad} should be rejected");
    }

    // Nothing was written — rejection happens before persistence.
    let stored: Option<String> =
        sqlx::query_scalar("SELECT meta_date FROM episodes WHERE episode_id = ?")
            .bind(&episode_id)
            .fetch_one(state.db.get_pool())
            .await
            .unwrap();
    assert_eq!(stored, None);
}

#[tokio::test]
async fn test_custom_metadata_offset_is_converted_to_utc() {
    // API consumers may send any timezone offset; the backend normalizes to UTC
    // so they never have to pre-convert.
    let (app, state, _temp_dir) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Offset Dates").await;
    let episode_id = format!("{}_S01E01", series_id);

    sqlx::query(
        "INSERT INTO episodes (episode_id, series_id, season, episode, title, \
         status, numbering_mode) \
         VALUES (?, ?, ?, ?, ?, 'unreleased', 0)",
    )
    .bind(&episode_id)
    .bind(&series_id)
    .bind(1)
    .bind(1)
    .bind("Episode")
    .execute(state.db.get_pool())
    .await
    .unwrap();

    let (status, _) = common::put_json(
        &app,
        &format!("/api/series/{}/episodes/{}/metadata", series_id, episode_id),
        &serde_json::json!({ "meta_date": "2026-07-07T20:30:00+09:00" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let stored: Option<String> =
        sqlx::query_scalar("SELECT meta_date FROM episodes WHERE episode_id = ?")
            .bind(&episode_id)
            .fetch_one(state.db.get_pool())
            .await
            .unwrap();
    assert_eq!(
        stored.as_deref(),
        Some("2026-07-07 11:30:00"),
        "+09:00 20:30 must be stored as 11:30 UTC"
    );
}

// Fetch-metadata response timestamp contract

/// Helper: mock metadata plugin that returns one episode, so
/// `fetch_metadata_for_series` takes its success path.
async fn setup_mock_metadata_plugin_with_episode(
    state: &Arc<jumbie::api::AppState>,
    series_id: &str,
    plugin_instance_id: &str,
) {
    state
        .plugin_manager
        .write()
        .await
        .add_internal_plugin(Arc::new(common::MockMetadataPlugin {
            instance_id: plugin_instance_id.to_string(),
            display_name: "Mock Metadata".to_string(),
            capabilities: vec![jumbie_shared::plugin::Capability::MetadataProviderNormal],
            series_identifier_label: Some("Mock ID".to_string()),
            series_name: "Provider Title".to_string(),
            overview: "A test series overview.".to_string(),
            aliases: vec![],
            episodes: vec![serde_json::json!({
                "unique_id": "e1",
                "season": 1,
                "episode": 1,
                "title": "Pilot",
                "description": null,
                "runtime": null,
                "image_url": null,
                "meta_date": "2026-06-18T20:00:00+00:00"
            })],
        }));

    if let Ok(Some(mut mapping)) = state.db.get_series_mapping(series_id).await {
        mapping
            .settings
            .metadata_ids
            .insert(plugin_instance_id.to_string(), "test_123".to_string());
        state
            .db
            .upsert_series_mapping(series_id, &mapping)
            .await
            .unwrap();
    }
}

/// `POST /api/series/{id}/fetch_metadata` must return `synced_at` as RFC 3339 with
/// an explicit offset, not the DB-canonical naive shape (`2026-06-18 20:00:00`).
#[tokio::test]
async fn test_fetch_metadata_response_synced_at_is_rfc3339() {
    let (app, state, _temp_dir) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Sync Format").await;
    let plugin_instance_id = uuid::Uuid::new_v4().to_string();
    setup_mock_metadata_plugin_with_episode(&state, &series_id, &plugin_instance_id).await;

    let res = app
        .clone()
        .oneshot(common::post_empty_request(&format!(
            "/api/series/{}/fetch_metadata",
            series_id
        )))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let synced_at = json["synced_at"]
        .as_str()
        .expect("synced_at should be a string");

    assert!(
        synced_at.ends_with("+00:00") || synced_at.ends_with('Z'),
        "synced_at must carry an explicit offset, got {synced_at}"
    );
    jumbie_shared::datetime::parse_rfc3339(synced_at)
        .unwrap_or_else(|e| panic!("synced_at must be RFC 3339: {synced_at}: {e}"));
}

// Season match: unassign episodes the provider does not have

#[tokio::test]
async fn test_match_season_unassigns_extra_episodes() {
    let (app, state, _temp_dir) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Match Unassign").await;

    let plugin_instance_id = uuid::Uuid::new_v4().to_string();
    setup_mock_metadata_plugin(&state, &series_id, &plugin_instance_id).await;
    let metadata_id = "test_123";

    // The provider has only episodes 1 and 2 in season 1.
    let cache: Vec<jumbie::db::metadata_cache::EpisodeMetadataForCache> = [1, 2]
        .into_iter()
        .map(|ep| jumbie::db::metadata_cache::EpisodeMetadataForCache {
            season_number: 1,
            episode_number: ep,
            unique_id: format!("{}-ext-{}", plugin_instance_id, ep),
            title: format!("Provider E{ep}"),
            description: None,
            runtime: None,
            image_url: None,
            meta_date: None,
        })
        .collect();
    state
        .db
        .batch_upsert_metadata_episodes_cache(
            metadata_id,
            &plugin_instance_id,
            &plugin_instance_id,
            "normal",
            &cache,
        )
        .await
        .unwrap();

    // On disk: E01 and E02 (provider episodes) plus E03 (an extra download).
    let dir = tempfile::TempDir::new().unwrap();
    let mut files = std::collections::HashMap::new();
    for ep in 1..=3 {
        let p = dir.path().join(format!("Show.S01E0{ep}.mkv"));
        std::fs::write(&p, vec![0u8; ep as usize * 10]).unwrap();
        files.insert(ep, p.to_string_lossy().to_string());
        let episode_id = format!("{series_id}_S01E0{ep}");
        sqlx::query(
            "INSERT INTO episodes (episode_id, series_id, season, episode, status, numbering_mode) \
             VALUES (?, ?, 1, ?, 'organized', 0)",
        )
        .bind(&episode_id)
        .bind(&series_id)
        .bind(ep)
        .execute(state.db.get_pool())
        .await
        .unwrap();
        state
            .db
            .associate_main_file(&episode_id, &files[&ep], None)
            .await
            .unwrap();
    }

    let res = app
        .clone()
        .oneshot(common::post_empty_request(&format!(
            "/api/series/{series_id}/season/1/match"
        )))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // E03 is removed; the provider's episodes stay.
    let remaining: Vec<i32> = sqlx::query_scalar(
        "SELECT episode FROM episodes WHERE series_id = ? AND season = 1 ORDER BY episode",
    )
    .bind(&series_id)
    .fetch_all(state.db.get_pool())
    .await
    .unwrap();
    assert_eq!(
        remaining,
        vec![1, 2],
        "an episode the provider does not have must be unassigned and removed"
    );

    // …and its file is durably blocked so a rescan cannot re-adopt it.
    let blocks = state.db.get_blocked_files(&series_id).await.unwrap();
    assert_eq!(blocks.len(), 1, "the extra episode's file must be blocked");
    assert_eq!(blocks[0].file_name, "show.s01e03.mkv");
    assert_eq!(blocks[0].size, 30);
}

/// Regression: the per-provider cache clear passed a lowercased plugin TYPE short
/// name ("tvdb") as the cache key's provider column, while writers stored the
/// backend plugin TYPE id ("jumbie.tvdb") — so a per-provider clear deleted
/// nothing. This drives the HTTP handler end-to-end and asserts the rows for the
/// targeted instance are removed while a same-type sibling instance is untouched.
#[tokio::test]
async fn test_clear_metadata_cache_deletes_target_provider_rows() {
    use jumbie::db::metadata_cache::{EpisodeMetadataForCache, UpsertMetadataCacheParams};

    let (app, state, _temp_dir) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Clear Cache").await;

    // Two instances of the SAME plugin type, so a type/instance mix-up cannot
    // pass by coincidence (instance ids differ from the type id and from each other).
    let instance_a = "aaaaaaaa-aaaa-4aaa-aaaa-aaaaaaaaaaaa";
    let instance_b = "bbbbbbbb-bbbb-4bbb-bbbb-bbbbbbbbbbbb";
    let metadata_a = "ext-a";
    let metadata_b = "ext-b";

    let mut instances = std::collections::HashMap::new();
    instances.insert(
        instance_a.to_string(),
        serde_json::json!({ "enabled": true, "name": "TVDB A" }),
    );
    instances.insert(
        instance_b.to_string(),
        serde_json::json!({ "enabled": true, "name": "TVDB B" }),
    );
    let mut section = std::collections::HashMap::new();
    section.insert("jumbie.tvdb".to_string(), instances);
    state
        .db
        .save_plugins_section("metadata", &section)
        .await
        .unwrap();

    // Point the series at both provider instances.
    let mut mapping = state
        .db
        .get_series_mapping(&series_id)
        .await
        .unwrap()
        .unwrap();
    mapping
        .settings
        .metadata_ids
        .insert(instance_a.to_string(), metadata_a.to_string());
    mapping
        .settings
        .metadata_ids
        .insert(instance_b.to_string(), metadata_b.to_string());
    state
        .db
        .upsert_series_mapping(&series_id, &mapping)
        .await
        .unwrap();

    // Seed both providers' caches exactly as `fetch_metadata_for_series` writes
    // them: plugin_id = TYPE id, instance_id = the instance.
    for (metadata_id, instance_id) in [(metadata_a, instance_a), (metadata_b, instance_b)] {
        state
            .db
            .upsert_metadata_series_cache(UpsertMetadataCacheParams {
                metadata_id,
                plugin_id: "jumbie.tvdb",
                instance_id,
                title: "T",
                overview: None,
                language: None,
                aliases: &[],
                image_url: None,
            })
            .await
            .unwrap();
        let ep = EpisodeMetadataForCache {
            season_number: 1,
            episode_number: 1,
            unique_id: "u1".to_string(),
            title: "E1".to_string(),
            description: None,
            runtime: None,
            image_url: None,
            meta_date: None,
        };
        state
            .db
            .batch_upsert_metadata_episodes_cache(
                metadata_id,
                "jumbie.tvdb",
                instance_id,
                "normal",
                &[ep],
            )
            .await
            .unwrap();
        state
            .db
            .upsert_metadata_season_cache(
                metadata_id,
                "jumbie.tvdb",
                instance_id,
                "normal",
                &[jumbie::plugins::metadata::SeasonMetadata {
                    season: 1,
                    episode_count: 1,
                }],
            )
            .await
            .unwrap();
        state
            .db
            .mark_fetch_attempted(metadata_id, "jumbie.tvdb", instance_id, "normal")
            .await
            .unwrap();
    }

    // Clear ONLY provider instance A via the HTTP handler.
    let res = common::send_request(
        &app,
        common::delete_request(&format!(
            "/api/series/{series_id}/metadata-cache?provider={instance_a}"
        )),
    )
    .await;
    assert!(res.status().is_success(), "clear should succeed");

    // A's rows are gone across every table.
    assert!(
        state
            .db
            .get_metadata_series_cache(metadata_a, "jumbie.tvdb", instance_a)
            .await
            .unwrap()
            .is_none(),
        "targeted provider series cache must be deleted"
    );
    assert!(
        state
            .db
            .get_metadata_episodes_cache(metadata_a, "jumbie.tvdb", instance_a, "normal")
            .await
            .unwrap()
            .is_empty(),
        "targeted provider episode cache must be deleted"
    );
    assert!(
        state
            .db
            .get_metadata_season_cache(metadata_a, "jumbie.tvdb", instance_a, "normal")
            .await
            .unwrap()
            .is_empty(),
        "targeted provider season cache must be deleted"
    );
    assert!(
        !state
            .db
            .has_fetch_been_attempted(metadata_a, "jumbie.tvdb", instance_a, "normal")
            .await,
        "targeted provider fetch log must be deleted"
    );

    // B's rows survive untouched.
    assert!(
        state
            .db
            .get_metadata_series_cache(metadata_b, "jumbie.tvdb", instance_b)
            .await
            .unwrap()
            .is_some(),
        "sibling provider series cache must survive"
    );
    assert_eq!(
        state
            .db
            .get_metadata_episodes_cache(metadata_b, "jumbie.tvdb", instance_b, "normal")
            .await
            .unwrap()
            .len(),
        1,
        "sibling provider episode cache must survive"
    );
}
