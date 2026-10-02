mod common;

use axum::http::StatusCode;
use std::sync::Arc;
use tower::ServiceExt;

/// Send a config update and assert that the server responds with `BAD_REQUEST`.
async fn assert_config_validation_fails(
    app: &axum::Router,
    config: &jumbie_shared::config::Config,
) {
    let payload = common::to_update_payload(config);
    let res = app
        .clone()
        .oneshot(common::put_json_request("/api/config", &payload))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST,);
}

use jumbie::api::AppState;
use jumbie_shared::types::{MappingRule, SeriesSettings};

/// Helper: insert a series mapping directly into the DB with a given `settings.path`.
/// This bypasses the API so we can create legacy series (path = None) or test edge cases.
async fn insert_series_mapping(
    state: &Arc<AppState>,
    title: &str,
    path_value: Option<&str>,
) -> String {
    let series_id = jumbie_shared::config::generate_uuid();
    let series_key = title.to_lowercase().replace(' ', "_");
    let mapping = MappingRule {
        target_title: title.to_string(),
        name: series_key,
        series_id: series_id.clone(),
        quality_profile: Some("Any".to_string()),
        release_profile: Some("Any".to_string()),
        qb_category: Some(title.to_string()),
        filters: None,
        scoring: None,
        hidden_in_library: false,
        settings: SeriesSettings {
            path: path_value.map(|p| p.to_string()),
            ..Default::default()
        },
    };
    state
        .db
        .upsert_series_mapping(&series_id, &mapping)
        .await
        .expect("Failed to insert test series mapping");
    series_id
}

// Empty destination roots

#[tokio::test]
async fn test_update_config_empty_destination_roots_rejected() {
    let (app, state, _tmp) = common::setup_test_app().await;

    let mut config = state.cfg.read().await.clone();
    config.organization.destination_roots.clear();
    let payload = common::to_update_payload(&config);

    let res = app
        .clone()
        .oneshot(common::put_json_request("/api/config", &payload))
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        StatusCode::OK,
        "Empty destination_roots should now be accepted"
    );
}

// Duplicate destination roots (same path added twice)

#[tokio::test]
async fn test_update_config_duplicate_exact_path_rejected() {
    let (app, state, _tmp) = common::setup_test_app().await;

    let mut config = state.cfg.read().await.clone();
    // Clone the existing root so the same path appears twice
    let root = config
        .organization
        .destination_roots
        .first()
        .cloned()
        .unwrap();
    config.organization.destination_roots.push(root);
    assert_config_validation_fails(&app, &config).await;
}

// Duplicate destination roots via symlink

#[cfg(unix)]
#[tokio::test]
async fn test_update_config_symlink_duplicate_rejected() {
    let (app, state, tmp) = common::setup_test_app().await;

    let organized_dir = tmp.path().join("organized");
    std::fs::create_dir_all(&organized_dir).expect("Failed to create organized dir");
    let symlink_dir = tmp.path().join("organized_symlink");
    if symlink_dir.exists() {
        std::fs::remove_dir(&symlink_dir).ok();
    }
    std::os::unix::fs::symlink(&organized_dir, &symlink_dir)
        .expect("Failed to create symlink for test");

    let mut config = state.cfg.read().await.clone();
    config
        .organization
        .destination_roots
        .push(symlink_dir.into());
    assert_config_validation_fails(&app, &config).await;
}

#[cfg(not(unix))]
#[tokio::test]
async fn test_update_config_symlink_duplicate_rejected() {
    eprintln!("Skipping symlink test on non-unix platform");
}

// Multiple distinct destination roots are accepted

#[tokio::test]
async fn test_update_config_distinct_roots_accepted() {
    let (app, state, tmp) = common::setup_test_app().await;

    // Create a second, genuinely distinct directory
    let second_root = tmp.path().join("organized_alt");
    std::fs::create_dir_all(&second_root).expect("Failed to create second root dir");

    let mut config = state.cfg.read().await.clone();
    config
        .organization
        .destination_roots
        .push(second_root.into());
    let payload = common::to_update_payload(&config);

    let res = app
        .clone()
        .oneshot(common::put_json_request("/api/config", &payload))
        .await
        .unwrap();
    assert!(
        res.status().is_success(),
        "Multiple distinct destination roots should be accepted, got {}",
        res.status()
    );
}

// Multi-root organized series scanning

#[tokio::test]
async fn test_organized_series_scans_all_roots() {
    let (app, state, tmp) = common::setup_test_app().await;

    // Create a second, distinct destination root directory
    let root2 = tmp.path().join("organized_alt");
    std::fs::create_dir_all(&root2).expect("Failed to create second root");

    // Create a directory in root2 (simulating an untracked series folder)
    let series_in_root2 = root2.join("Series In Root2");
    std::fs::create_dir_all(&series_in_root2).expect("Failed to create series dir in root2");

    // Add the second root to the config
    {
        let mut cfg = state.cfg.write().await;
        cfg.organization
            .destination_roots
            .push(root2.clone().into());
    }

    // Create a tracked series in root1 (the default root)
    common::create_test_series(&app, "Series In Root1").await;

    // Now fetch organized series — both should appear
    let res = app
        .clone()
        .oneshot(common::get_request("/api/system/organized_series"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let items: Vec<serde_json::Value> = serde_json::from_slice(&body).unwrap();

    // We should see at least 2 items: one tracked (Series In Root1) and one untracked (Series In Root2)
    let root1_series = items
        .iter()
        .find(|i| i["folder_name"].as_str() == Some("Series In Root1"));
    let root2_series = items
        .iter()
        .find(|i| i["folder_name"].as_str() == Some("Series In Root2"));

    assert!(
        root1_series.is_some(),
        "Series In Root1 should appear in organized series listing"
    );
    assert!(
        root2_series.is_some(),
        "Series In Root2 should appear in organized series listing even though it's in a different root"
    );

    if let Some(item) = root1_series {
        assert_eq!(
            item["is_tracked"], true,
            "Series In Root1 should be tracked"
        );
    }
    if let Some(item) = root2_series {
        assert_eq!(
            item["is_tracked"], false,
            "Series In Root2 should be untracked since it was only created as a directory"
        );
    }
}

// The toggle is persisted through the config API, then read back from the DB.

#[tokio::test]
async fn test_update_config_persists_include_subdirs_flag() {
    use jumbie_shared::config::DestinationRoot;

    let (app, state, tmp) = common::setup_test_app().await;
    let quiet_root = tmp.path().join("organized_quiet");
    std::fs::create_dir_all(&quiet_root).unwrap();

    let mut config = state.cfg.read().await.clone();
    config.organization.destination_roots.push(DestinationRoot {
        path: quiet_root.clone(),
        include_subdirs_in_managed: false,
    });
    let payload = common::to_update_payload(&config);
    let res = app
        .clone()
        .oneshot(common::put_json_request("/api/config", &payload))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // The DB is the source of truth for persisted config.
    let saved = state.db.get_organization_config().await.unwrap();
    let root = saved
        .destination_roots
        .iter()
        .find(|r| r.path == quiet_root)
        .expect("saved root must be present");
    assert!(
        !root.include_subdirs_in_managed,
        "the excluded flag must survive the save/load round trip"
    );
}

// Roots flagged to exclude their subdirs hide untracked folders but keep tracked ones.

#[tokio::test]
async fn test_organized_series_excluded_root_hides_untracked_but_keeps_tracked() {
    use jumbie_shared::config::DestinationRoot;

    let (app, state, tmp) = common::setup_test_app().await;

    let root2 = tmp.path().join("organized_quiet");
    std::fs::create_dir_all(&root2).expect("Failed to create quiet root");

    // An untracked folder the root would normally auto-list.
    std::fs::create_dir_all(root2.join("Untracked Folder")).expect("create untracked dir");

    // A tracked series that physically lives inside the quiet root. It must still
    // appear because it is registered in the library.
    let tracked_dir = root2.join("Tracked Series");
    std::fs::create_dir_all(&tracked_dir).expect("create tracked dir");
    insert_series_mapping(
        &state,
        "Tracked Series",
        Some(&tracked_dir.to_string_lossy()),
    )
    .await;

    {
        let mut cfg = state.cfg.write().await;
        cfg.organization.destination_roots.push(DestinationRoot {
            path: root2.clone(),
            include_subdirs_in_managed: false,
        });
    }

    let res = app
        .clone()
        .oneshot(common::get_request("/api/system/organized_series"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let items: Vec<serde_json::Value> = serde_json::from_slice(&body).unwrap();

    assert!(
        !items
            .iter()
            .any(|i| i["folder_name"].as_str() == Some("Untracked Folder")),
        "untracked folders under an excluded root must not be listed"
    );
    let tracked = items
        .iter()
        .find(|i| i["folder_name"].as_str() == Some("Tracked Series"));
    assert!(
        tracked.is_some(),
        "series already in the library must still list under an excluded root"
    );
    assert_eq!(
        tracked.unwrap()["is_tracked"],
        true,
        "the tracked series should be reported as tracked"
    );
}

// Config validation tests

#[tokio::test]
async fn test_update_config_invalid_unexpected_files_handling_rejected() {
    let (app, state, _tmp) = common::setup_test_app().await;

    let mut config = state.cfg.read().await.clone();
    config.general.unexpected_files_handling = "banana".to_string();
    assert_config_validation_fails(&app, &config).await;
}

#[tokio::test]
async fn test_update_config_negative_media_scan_interval_rejected() {
    let (app, state, _tmp) = common::setup_test_app().await;

    let mut config = state.cfg.read().await.clone();
    config.general.media_info_scan_interval = 0;
    assert_config_validation_fails(&app, &config).await;
}

#[tokio::test]
async fn test_update_config_negative_season_pack_threshold_rejected() {
    let (app, state, _tmp) = common::setup_test_app().await;

    let mut config = state.cfg.read().await.clone();
    config.general.season_pack_replace_threshold = 101;
    assert_config_validation_fails(&app, &config).await;
}

// Existing installs stored destination_roots as bare strings. The startup
// migration must rewrite them to the object form so the new config still loads.

#[tokio::test]
async fn test_migration_rewrites_legacy_string_destination_roots() {
    let (_app, state, _tmp) = common::setup_test_app().await;
    let db = &state.db;

    // Simulate a pre-upgrade install: bare-string roots plus an unset marker.
    sqlx::query(
        "INSERT INTO config_defaults (key, data) VALUES ('organization', ?) \
         ON CONFLICT(key) DO UPDATE SET data = excluded.data",
    )
    .bind(
        r#"{"destination_roots":["/media/legacy","/media/second"],"collision_handling":"rename"}"#,
    )
    .execute(db.get_pool())
    .await
    .unwrap();
    sqlx::query("DELETE FROM system_state WHERE key = ?")
        .bind("data_migration_destination_roots_objects_v1")
        .execute(db.get_pool())
        .await
        .unwrap();

    db.run_migrations().await.unwrap();

    let org = db.get_organization_config().await.unwrap();
    assert_eq!(org.destination_roots.len(), 2);
    assert_eq!(
        org.destination_roots[0].path,
        std::path::PathBuf::from("/media/legacy")
    );
    assert_eq!(
        org.destination_roots[1].path,
        std::path::PathBuf::from("/media/second")
    );
    assert!(
        org.destination_roots
            .iter()
            .all(|r| r.include_subdirs_in_managed),
        "migrated roots must default to including subdirs"
    );
}

#[tokio::test]
async fn test_migration_is_idempotent_for_object_roots() {
    let (_app, state, _tmp) = common::setup_test_app().await;
    let db = &state.db;

    // Object-form roots must survive migration untouched.
    sqlx::query(
        "INSERT INTO config_defaults (key, data) VALUES ('organization', ?) \
         ON CONFLICT(key) DO UPDATE SET data = excluded.data",
    )
    .bind(r#"{"destination_roots":[{"path":"/media/quiet","include_subdirs_in_managed":false}]}"#)
    .execute(db.get_pool())
    .await
    .unwrap();
    sqlx::query("DELETE FROM system_state WHERE key = ?")
        .bind("data_migration_destination_roots_objects_v1")
        .execute(db.get_pool())
        .await
        .unwrap();

    db.run_migrations().await.unwrap();

    let org = db.get_organization_config().await.unwrap();
    assert_eq!(org.destination_roots.len(), 1);
    assert_eq!(
        org.destination_roots[0].path,
        std::path::PathBuf::from("/media/quiet")
    );
    assert!(
        !org.destination_roots[0].include_subdirs_in_managed,
        "an already-object root keeps its flag through migration"
    );
}

// Search-key templates accept only `${season}` / `${episode}` + padding.

#[tokio::test]
async fn test_update_config_unknown_search_variable_rejected() {
    let (app, state, _tmp) = common::setup_test_app().await;

    let mut config = state.cfg.read().await.clone();
    config.organization.search_format = "${series} S${season:02}".to_string();
    assert_config_validation_fails(&app, &config).await;
}

#[tokio::test]
async fn test_update_config_non_padding_search_modifier_rejected() {
    let (app, state, _tmp) = common::setup_test_app().await;

    let mut config = state.cfg.read().await.clone();
    config.organization.search_format_absolute = "${episode:-x}".to_string();
    assert_config_validation_fails(&app, &config).await;
}

#[tokio::test]
async fn test_update_config_accepts_valid_search_formats() {
    let (app, state, _tmp) = common::setup_test_app().await;

    let mut config = state.cfg.read().await.clone();
    config.organization.search_format = "S${season:02}E${episode:02}".to_string();
    // Blank is allowed (search by title alone).
    config.organization.search_format_absolute = String::new();
    let payload = common::to_update_payload(&config);
    let res = app
        .clone()
        .oneshot(common::put_json_request("/api/config", &payload))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

// A series tracked in root2 via batch_edit

#[tokio::test]
async fn test_batch_edit_finds_series_in_any_root() {
    let (app, state, tmp) = common::setup_test_app().await;

    // Add a second root
    let root2 = tmp.path().join("organized_alt");
    std::fs::create_dir_all(&root2).expect("Failed to create second root");
    {
        let mut cfg = state.cfg.write().await;
        cfg.organization
            .destination_roots
            .push(root2.clone().into());
    }

    // Create a series directory in root2 (as if it was already on disk)
    let series_dir = root2.join("Batch Series Root2");
    std::fs::create_dir_all(&series_dir).expect("Failed to create series dir");

    // Use batch_edit add_to_library to track it
    let payload = jumbie_shared::types::BatchEditOrganizedSeriesPayload {
        paths: vec![series_dir.to_string_lossy().to_string()],
        operation: "add_to_library".to_string(),
        delete_files: false,
    };
    let res = app
        .clone()
        .oneshot(common::post_json_request(
            "/api/system/organized_series/batch_edit",
            &payload,
        ))
        .await
        .unwrap();
    assert!(
        res.status().is_success(),
        "Batch edit add_to_library should succeed for series in root2"
    );

    let res = app
        .clone()
        .oneshot(common::get_request("/api/system/organized_series"))
        .await
        .unwrap();
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let items: Vec<serde_json::Value> = serde_json::from_slice(&body).unwrap();

    let found = items
        .iter()
        .find(|i| i["folder_name"].as_str() == Some("Batch Series Root2"));
    assert!(
        found.is_some(),
        "Batch Series Root2 should appear after being added to library"
    );
    if let Some(item) = found {
        assert_eq!(
            item["is_tracked"], true,
            "Batch Series Root2 should be tracked after add_to_library"
        );
    }
}

#[tokio::test]
async fn test_series_remains_in_library_after_root_removed() {
    // After startup migration, removing a root should NOT affect series at all.
    // Series keep their explicit paths regardless of destination root config.
    let (app, state, _tmp) = common::setup_test_app().await;

    // Create a series via the normal API (which always sets path)
    let series_id = common::create_test_series(&app, "Persistent Show").await;

    let mapping = state
        .db
        .get_series_mapping(&series_id)
        .await
        .expect("DB error")
        .expect("Series should exist");
    let original_path = mapping.settings.path.clone();
    assert!(
        original_path.is_some(),
        "Series should have an explicit path"
    );

    // Remove all destination roots
    let mut config = state.cfg.read().await.clone();
    config.organization.destination_roots.clear();
    let payload = common::to_update_payload(&config);
    let res = app
        .clone()
        .oneshot(common::put_json_request("/api/config", &payload))
        .await
        .unwrap();
    assert!(res.status().is_success(), "Config update should succeed");

    let mapping = state
        .db
        .get_series_mapping(&series_id)
        .await
        .expect("DB error")
        .expect("Series should still exist in library");
    assert_eq!(
        mapping.settings.path, original_path,
        "Series path should be unchanged after root removal"
    );
    assert!(
        !mapping.hidden_in_library,
        "Series should not be hidden after root removal"
    );
}

#[tokio::test]
async fn test_explicit_path_series_unchanged_after_root_removal() {
    // A custom-path series (outside all roots) must be completely unaffected
    // by any destination root operation.
    let (app, state, tmp) = common::setup_test_app().await;

    let custom_path = tmp.path().join("external").join("Custom Series");
    tokio::fs::create_dir_all(&custom_path)
        .await
        .expect("Failed");

    let series_id = insert_series_mapping(
        &state,
        "Custom Series",
        Some(&custom_path.to_string_lossy()),
    )
    .await;

    // Remove all destination roots
    let mut config = state.cfg.read().await.clone();
    config.organization.destination_roots.clear();
    let payload = common::to_update_payload(&config);
    let res = app
        .clone()
        .oneshot(common::put_json_request("/api/config", &payload))
        .await
        .unwrap();
    assert!(res.status().is_success());

    let mapping = state
        .db
        .get_series_mapping(&series_id)
        .await
        .expect("DB error")
        .expect("Series should exist");
    assert_eq!(
        mapping.settings.path,
        Some(custom_path.to_string_lossy().to_string()),
        "Custom-path series should be untouched"
    );
}
