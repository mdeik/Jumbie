mod common;

use axum::http::StatusCode;
use jumbie_shared::mapping::MonitorMode;
use jumbie_shared::types::{
    BatchEditOrganizedSeriesPayload, SeasonOverride, ToggleMonitorPayload, ToggleVisibilityPayload,
};
use std::collections::HashMap;
use tower::ServiceExt;

// Helper: batch edit organized series

async fn batch_edit_organized_series(
    app: &axum::Router,
    paths: Vec<String>,
    operation: &str,
    delete_files: bool,
) -> serde_json::Value {
    let payload = BatchEditOrganizedSeriesPayload {
        paths,
        operation: operation.to_string(),
        delete_files,
    };
    let res = common::send_request(
        app,
        common::post_json_request("/api/system/organized_series/batch_edit", &payload),
    )
    .await;
    assert_eq!(
        res.status(),
        StatusCode::OK,
        "batch_edit operation '{}' should succeed",
        operation
    );
    common::response_json(res).await
}

// GET /api/system/organized_series

#[tokio::test]
async fn test_get_organized_series_empty() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let res = app
        .oneshot(common::get_request("/api/system/organized_series"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json.is_array());
}

#[tokio::test]
async fn test_get_organized_series_with_tracked() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    common::create_test_series(&app, "Organized Show").await;
    let res = app
        .clone()
        .oneshot(common::get_request("/api/system/organized_series"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json.is_array());
}

// PUT /api/system/organized_series/:series_id/toggle

#[tokio::test]
async fn test_toggle_series_monitor() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Toggle Show").await;
    let payload = ToggleMonitorPayload {
        monitor_mode: MonitorMode::None,
    };
    let res = app
        .clone()
        .oneshot(common::put_json_request(
            &format!("/api/system/organized_series/{}/toggle", series_id),
            &payload,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_toggle_series_monitor_not_found() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let payload = ToggleMonitorPayload {
        monitor_mode: MonitorMode::All,
    };
    let res = app
        .oneshot(common::put_json_request(
            "/api/system/organized_series/nonexistent_id_xyz/toggle",
            &payload,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

// PUT /api/system/organized_series/:series_id/visibility

#[tokio::test]
async fn test_toggle_library_visibility() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Visibility Show").await;
    let payload = ToggleVisibilityPayload {
        hidden_in_library: true,
    };
    let res = app
        .clone()
        .oneshot(common::put_json_request(
            &format!("/api/system/organized_series/{}/visibility", series_id),
            &payload,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_toggle_library_visibility_not_found() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let payload = ToggleVisibilityPayload {
        hidden_in_library: false,
    };
    let res = app
        .oneshot(common::put_json_request(
            "/api/system/organized_series/nonexistent_id_xyz/visibility",
            &payload,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

// POST /api/system/organized_series/batch_edit

#[tokio::test]
async fn test_batch_edit_organized_series_unknown_operation() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let payload = BatchEditOrganizedSeriesPayload {
        paths: vec!["/tmp/some_path".to_string()],
        operation: "unknown_operation".to_string(),
        delete_files: false,
    };
    let res = app
        .oneshot(common::post_json_request(
            "/api/system/organized_series/batch_edit",
            &payload,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_batch_edit_organized_series_remove_from_library() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Batch Vis Show").await;

    // Get the path from the organized series listing
    let (_, body_items) = common::get_json(&app, "/api/system/organized_series").await;

    let items: serde_json::Value = serde_json::from_value(body_items).unwrap();

    // Find our series entry
    let path = items
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["series_id"].as_str() == Some(&series_id))
        .and_then(|i| i["absolute_path"].as_str())
        .unwrap_or("/tmp/nonexistent")
        .to_string();

    let payload = BatchEditOrganizedSeriesPayload {
        paths: vec![path],
        operation: "remove_from_library".to_string(),
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
    assert_eq!(res.status(), StatusCode::OK);
}

// GET /api/system/rename_queue/:series_id

#[tokio::test]
async fn test_get_rename_queue_detail_not_found() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let res = app
        .oneshot(common::get_request(
            "/api/system/rename_queue/nonexistent_xyz",
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_get_rename_queue_detail() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Rename Detail Show").await;
    let res = app
        .clone()
        .oneshot(common::get_request(&format!(
            "/api/system/rename_queue/{}",
            series_id
        )))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json["series_id"].is_string());
    assert!(json["renames"].is_array());
}

// POST /api/system/organized_series/preview

#[tokio::test]
async fn test_preview_import_real_dir() {
    let (app, _state, tmp) = common::setup_test_app().await;
    // Use a real directory that exists — the temp dir itself
    let payload = jumbie_shared::types::PreviewSeriesRequest {
        path: tmp.path().to_string_lossy().to_string(),
        is_bulk: true,
    };
    let res = app
        .oneshot(common::post_json_request(
            "/api/system/organized_series/preview",
            &payload,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json.is_array());
}

#[tokio::test]
async fn test_preview_import_nonexistent_path() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let payload = jumbie_shared::types::PreviewSeriesRequest {
        path: "/nonexistent/path/that/does/not/exist".to_string(),
        is_bulk: false,
    };
    let res = app
        .oneshot(common::post_json_request(
            "/api/system/organized_series/preview",
            &payload,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

// Batch edit: multiple series across different directories

#[tokio::test]
async fn test_batch_edit_add_to_library_multiple_paths() {
    // Verifies that batch "add_to_library" works for multiple untracked paths
    // in different subdirectories of the same root.
    let (app, _state, tmp) = common::setup_test_app().await;
    let root = tmp.path().join("organized");

    // Create two sub-directories (untracked folders)
    let dir_a = root.join("Batch Add A");
    let dir_b = root.join("Batch Add B");
    std::fs::create_dir_all(&dir_a).unwrap();
    std::fs::create_dir_all(&dir_b).unwrap();

    let paths = vec![
        dir_a.to_string_lossy().to_string(),
        dir_b.to_string_lossy().to_string(),
    ];

    let payload = BatchEditOrganizedSeriesPayload {
        paths: paths.clone(),
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
    assert_eq!(
        res.status(),
        StatusCode::OK,
        "batch add_to_library should succeed"
    );

    let (_, body_items) = common::get_json(&app, "/api/system/organized_series").await;

    let items: serde_json::Value = serde_json::from_value(body_items).unwrap();
    let entries: Vec<&serde_json::Value> = items
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| {
            i["folder_name"].as_str() == Some("Batch Add A")
                || i["folder_name"].as_str() == Some("Batch Add B")
        })
        .collect();
    assert_eq!(
        entries.len(),
        2,
        "Both batch-added series should appear in the listing"
    );
    for entry in &entries {
        assert!(
            entry["is_tracked"].as_bool().unwrap_or(false),
            "Series '{}' should be tracked after batch add_to_library",
            entry["folder_name"].as_str().unwrap_or("")
        );
        assert!(
            !entry["hidden_in_library"].as_bool().unwrap_or(true),
            "Series '{}' should NOT be hidden after batch add_to_library",
            entry["folder_name"].as_str().unwrap_or("")
        );
    }

    // Now batch remove from library (hide them)
    let payload = BatchEditOrganizedSeriesPayload {
        paths: paths.clone(),
        operation: "remove_from_library".to_string(),
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
    assert_eq!(
        res.status(),
        StatusCode::OK,
        "batch remove_from_library should succeed"
    );

    let (_, body_items) = common::get_json(&app, "/api/system/organized_series").await;

    let items: serde_json::Value = serde_json::from_value(body_items).unwrap();
    for path_str in &paths {
        let found = items.as_array().unwrap().iter().any(|i| {
            i["absolute_path"].as_str() == Some(path_str)
                && i["is_tracked"].as_bool().unwrap_or(false)
                && i["hidden_in_library"].as_bool().unwrap_or(false)
        });
        assert!(
            found,
            "Series at '{}' should be tracked but hidden after batch remove_from_library",
            path_str
        );
    }
}

#[tokio::test]
async fn test_batch_edit_series_with_series_template_path() {
    // batch_edit must resolve a stored "${series}" path template to match the UI's
    // absolute_path (get_organized_series already does this).
    let (app, state, tmp) = common::setup_test_app().await;
    let root = tmp.path().join("organized");
    std::fs::create_dir_all(&root).unwrap();

    // Create a series directory on disk
    let series_dir = root.join("Template Show");
    std::fs::create_dir_all(&series_dir).unwrap();

    // Mapping created directly in DB with path = "${series}" (folder name = series title).
    let series_id = jumbie_shared::config::generate_uuid();
    let mapping = jumbie_shared::mapping::MappingRule {
        target_title: "Template Show".to_string(),
        name: "template_show".to_string(),
        series_id: series_id.clone(),
        quality_profile: Some("Any".to_string()),
        release_profile: Some("Any".to_string()),
        qb_category: Some("Template Show".to_string()),
        settings: jumbie_shared::mapping::SeriesSettings {
            path: Some("${series}".to_string()),
            ..Default::default()
        },
        ..Default::default()
    };
    state
        .db
        .upsert_series_mapping(&series_id, &mapping)
        .await
        .expect("Failed to insert template-path mapping");

    // get_organized_series resolves "${series}" to "Template Show", but the relative
    // path means the filesystem scan misses it; it appears in the "tracked but not on
    // disk" fallback with absolute_path = "Template Show" (relative).
    let (_, body_items) = common::get_json(&app, "/api/system/organized_series").await;

    let items: serde_json::Value = serde_json::from_value(body_items).unwrap();

    // Find our series — should be present even though path is relative
    let entry = items
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["series_id"].as_str() == Some(&series_id))
        .expect("Series with template path should appear in organized series listing");
    let ui_path = entry["absolute_path"]
        .as_str()
        .expect("Entry should have absolute_path")
        .to_string();

    // Now try a batch operation using the path from the UI
    batch_edit_organized_series(&app, vec![ui_path.clone()], "remove_from_library", false).await;

    let mapping = state
        .db
        .get_series_mapping(&series_id)
        .await
        .expect("Failed to fetch mapping")
        .expect("Mapping should still exist");
    assert!(
        mapping.hidden_in_library,
        "Series should be hidden after batch remove_from_library"
    );
}

#[tokio::test]
async fn test_batch_edit_series_created_via_api() {
    // Reproduces the user's scenario: a series created via the normal API flow
    // (like the frontend does) should be findable by batch operations using the
    // absolute_path returned by get_organized_series.
    let (app, state, tmp) = common::setup_test_app().await;

    // Create a series directory inside the destination root
    let root = tmp.path().join("organized");
    std::fs::create_dir_all(&root).unwrap();
    let series_dir = root.join("api_created_series");
    std::fs::create_dir_all(&series_dir).unwrap();

    // The folder already exists on disk — "overwrite" keeps claim semantics.
    common::set_collision_handling(&state, "overwrite").await;

    // Create via the normal series create API (same as frontend "add single" flow)
    let create_req = jumbie_shared::types::CreateSeriesRequest {
        path: series_dir.to_string_lossy().to_string(),
        series_name: Some("api_created_series".to_string()),
        scan_for_existing: Some(false),
        monitor_mode: None,
        quality_profile: None,
        release_profile: None,
        metadata_ids: Default::default(),
        search_missing_on_add: false,
        resolve_collisions: true,
        settings: Default::default(),
    };
    let res = app
        .clone()
        .oneshot(common::post_json_request("/api/series", &create_req))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let series_id: String = serde_json::from_slice(&body).unwrap();
    let series_id = series_id.trim_matches('"').to_string();

    // Fetch organized series listing
    let (_, body_items) = common::get_json(&app, "/api/system/organized_series").await;

    let items: serde_json::Value = serde_json::from_value(body_items).unwrap();

    // Find our series by series_id
    let entry = items
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["series_id"].as_str() == Some(&series_id))
        .unwrap_or_else(|| {
            panic!(
                "Series {} should appear in organized series listing",
                series_id
            )
        });
    let ui_path = entry["absolute_path"]
        .as_str()
        .expect("Entry must have absolute_path")
        .to_string();

    eprintln!(
        "DEBUG batch test: series_id={}, ui_path='{}', stored_path_matches={}",
        series_id,
        ui_path,
        {
            let mapping = state
                .db
                .get_series_mapping(&series_id)
                .await
                .unwrap()
                .unwrap();
            let stored = mapping.settings.path.unwrap_or_default();
            eprintln!("DEBUG batch test: stored_path='{}'", stored);
            stored == ui_path
        }
    );

    // Batch remove_from_library using the UI path
    batch_edit_organized_series(&app, vec![ui_path.clone()], "remove_from_library", false).await;

    let mapping = state
        .db
        .get_series_mapping(&series_id)
        .await
        .expect("Failed to fetch mapping")
        .expect("Mapping should still exist");
    assert!(
        mapping.hidden_in_library,
        "Series '{}' should be hidden after batch remove_from_library using path '{}'",
        series_id, ui_path
    );
}

#[tokio::test]
async fn test_batch_edit_path_with_trailing_slash_mismatch() {
    // Verifies that batch_edit works even when the stored path and the UI path
    // differ in trailing slashes. This can happen when a series is created
    // via the API with a trailing slash (e.g. user typed "/tmp/foo/") but the
    // filesystem scan in get_organized_series produces paths without trailing
    // slashes.
    let (app, state, tmp) = common::setup_test_app().await;
    let root = tmp.path().join("organized");
    std::fs::create_dir_all(&root).unwrap();
    let series_dir = root.join("trailing_slash_show");
    std::fs::create_dir_all(&series_dir).unwrap();

    // The folder already exists on disk — "overwrite" keeps claim semantics.
    common::set_collision_handling(&state, "overwrite").await;

    // Create series with trailing slash in the path
    let create_req = jumbie_shared::types::CreateSeriesRequest {
        path: series_dir.to_string_lossy().to_string() + "/", // WITH trailing slash
        series_name: Some("trailing_slash_show".to_string()),
        scan_for_existing: Some(false),
        monitor_mode: None,
        quality_profile: None,
        release_profile: None,
        metadata_ids: Default::default(),
        search_missing_on_add: false,
        resolve_collisions: true,
        settings: Default::default(),
    };
    let res = app
        .clone()
        .oneshot(common::post_json_request("/api/series", &create_req))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let series_id: String = serde_json::from_slice(&body).unwrap();
    let series_id = series_id.trim_matches('"').to_string();

    // validate_path canonicalizes (stripping trailing slashes), so the stored path
    // must be normalized; the batch_edit below accepts either form.
    let stored_path = state
        .db
        .get_series_mapping(&series_id)
        .await
        .unwrap()
        .unwrap()
        .settings
        .path
        .unwrap_or_default();
    eprintln!(
        "DEBUG: stored_path='{}' (ends_with_slash={})",
        stored_path,
        stored_path.ends_with('/')
    );
    assert!(
        !stored_path.ends_with('/'),
        "Stored path should be normalized (no trailing slash), got: {}",
        stored_path
    );

    // Fetch organized series listing - get_organized_series returns paths WITHOUT trailing slash
    // from entry.path() (read_dir never produces trailing slashes)
    let (_, body_items) = common::get_json(&app, "/api/system/organized_series").await;

    let items: serde_json::Value = serde_json::from_value(body_items).unwrap();

    // Find the series by ID
    let entry = items
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["series_id"].as_str() == Some(&series_id))
        .unwrap_or_else(|| panic!("Series {} should appear in listing", series_id));
    let ui_path = entry["absolute_path"]
        .as_str()
        .expect("Entry must have absolute_path")
        .to_string();

    eprintln!(
        "DEBUG: ui_path='{}' (ends_with_slash={})",
        ui_path,
        ui_path.ends_with('/')
    );
    // The UI path should NOT have a trailing slash because it comes from entry.path()
    assert!(
        !ui_path.ends_with('/'),
        "UI path should NOT have trailing slash"
    );

    // Now batch remove_from_library using the UI path (no trailing slash)
    batch_edit_organized_series(&app, vec![ui_path.clone()], "remove_from_library", false).await;

    let mapping = state
        .db
        .get_series_mapping(&series_id)
        .await
        .expect("Failed to fetch mapping")
        .expect("Mapping should still exist");
    assert!(
        mapping.hidden_in_library,
        "Series should be hidden despite trailing slash mismatch"
    );
}

// Completion counts must honour a season's configured `cell_count` (SSoT
// `SeriesSettings::expected_episode_count`), while `organized` still reflects the
// files on disk.

async fn set_cell_count(
    state: &jumbie::api::AppState,
    series_id: &str,
    season: &str,
    cell_count: i32,
) {
    let mut mapping = state
        .db
        .get_series_mapping(series_id)
        .await
        .unwrap()
        .expect("series mapping exists");
    mapping.settings.season.insert(
        season.to_string(),
        SeasonOverride {
            season: season.to_string(),
            episode_start: None,
            episode_end: None,
            cell_count: Some(cell_count),
            episode_offset: None,
            alias_season_number: None,
            search_format: None,
            aliases: vec![],
            reg_patterns: vec![],
        },
    );
    state
        .db
        .upsert_series_mapping(series_id, &mapping)
        .await
        .unwrap();
}

async fn seed_episode(state: &jumbie::api::AppState, series_id: &str, episode: i32) {
    let eid = format!("{}_S01E{:02}", series_id, episode);
    state
        .db
        .insert_episode(jumbie::db::episodes::InsertEpisodeParams {
            episode_id: &eid,
            series_id,
            season: 1,
            episode,
            file_path: Some("/media/cell.mkv"),
            title: None,
            quality_profile_id: None,
            status: "organized",
            meta_date: None,
            est_date: None,
            metadata_ids: &HashMap::new(),
            description: None,
            runtime: None,
            image_url: None,
            metadata_source: None,
            numbering_mode: None,
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn test_organized_series_expected_uses_season_cell_count() {
    let (app, state, _tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Cell Count Managed").await;
    set_cell_count(&state, &series_id, "1", 12).await;

    // Only 5 of the 12 configured cells have files.
    for ep in 1..=5 {
        seed_episode(&state, &series_id, ep).await;
    }

    let res = app
        .oneshot(common::get_request("/api/system/organized_series"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let item = json
        .as_array()
        .expect("array")
        .iter()
        .find(|e| e["series_id"].as_str() == Some(series_id.as_str()))
        .expect("tracked series appears in organized listing");

    assert_eq!(
        item["total_episodes_organized"].as_i64(),
        Some(5),
        "organized reflects the files on disk"
    );
    assert_eq!(
        item["total_episodes_expected"].as_i64(),
        Some(12),
        "expected must use the configured cell count"
    );
    let season = &item["season_counts"][0];
    assert_eq!(season["organized"].as_i64(), Some(5));
    assert_eq!(season["expected"].as_i64(), Some(12));
}
