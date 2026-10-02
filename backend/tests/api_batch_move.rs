mod common;

use axum::http::StatusCode;
use jumbie_shared::types::{BatchMoveOrganizedSeriesPayload, PathOperation};
use serde_json::Value;
use tower::ServiceExt;

// Helpers

/// Send a batch move request and poll `/status` until finished.
/// Returns the final status JSON body.
async fn execute_batch_move(
    app: &axum::Router,
    payload: &BatchMoveOrganizedSeriesPayload,
) -> Value {
    let (status, body) =
        common::post_json(app, "/api/system/organized_series/batch_move", payload).await;
    assert_eq!(status, StatusCode::OK, "POST failed: {}", body);

    let response: Value = body;
    let task_id = response["task_id"]
        .as_str()
        .expect("POST did not return task_id")
        .to_string();

    let max_polls: u32 = 100;
    for _ in 0..max_polls {
        let (status, body) = common::get_json(
            app,
            &format!("/api/system/organized_series/batch_move/{task_id}/status"),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        if body["finished"].as_bool().unwrap_or(false) {
            return body;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    panic!("Batch move task {task_id} did not finish in time");
}

/// Create a test series, then create a real directory for it under the organized root.
/// Returns (series_id, directory_path).
async fn create_test_series_with_dir(
    app: &axum::Router,
    tmp: &tempfile::TempDir,
    title: &str,
) -> (String, std::path::PathBuf) {
    let series_id = common::create_test_series(app, title).await;
    let dir = tmp.path().join("organized").join(title);
    tokio::fs::create_dir_all(&dir).await.unwrap();
    let ep_file = dir.join("episode.mkv");
    tokio::fs::write(&ep_file, b"fake video content")
        .await
        .unwrap();
    (series_id, dir)
}

// GET /api/system/organized_series — completion data

#[tokio::test]
async fn test_get_organized_series_completion_data() {
    let (app, _state, tmp) = common::setup_test_app().await;

    // Create a series with NO episodes → completion = NotStarted
    let (_, _dir) = create_test_series_with_dir(&app, &tmp, "Completion Test Show").await;

    let (status, body) = common::get_json(&app, "/api/system/organized_series").await;
    assert_eq!(status, StatusCode::OK);

    let items: Vec<serde_json::Value> = serde_json::from_value(body).unwrap();
    let item = items
        .iter()
        .find(|i| i["folder_name"].as_str() == Some("Completion Test Show"))
        .expect("Series should appear in organized series list");

    assert_eq!(item["completion_status"], "NotStarted");
    assert_eq!(item["total_episodes_organized"], 0);
    assert_eq!(item["total_episodes_expected"], 0);
}

// POST /api/system/organized_series/batch_move_preview

#[tokio::test]
async fn test_batch_move_preview_nonexistent_root() {
    let (app, _state, tmp) = common::setup_test_app().await;
    let (_sid, dir) = create_test_series_with_dir(&app, &tmp, "Preview No Root").await;

    let payload = BatchMoveOrganizedSeriesPayload {
        paths: vec![dir.to_string_lossy().to_string()],
        target_root: "/nonexistent/path/that/does/not/exist".to_string(),
        custom_paths: std::collections::HashMap::new(),
        file_operation: PathOperation::Move,
        stop_tracking: false,
    };

    let res = app
        .clone()
        .oneshot(common::post_json_request(
            "/api/system/organized_series/batch_move_preview",
            &payload,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_batch_move_preview_untracked_path() {
    let (app, _state, tmp) = common::setup_test_app().await;
    let untracked_dir = tmp.path().join("organized").join("Untracked Folder");
    tokio::fs::create_dir_all(&untracked_dir).await.unwrap();

    let payload = BatchMoveOrganizedSeriesPayload {
        paths: vec![untracked_dir.to_string_lossy().to_string()],
        target_root: tmp.path().join("organized").to_string_lossy().to_string(),
        custom_paths: std::collections::HashMap::new(),
        file_operation: PathOperation::Move,
        stop_tracking: false,
    };

    let (status, body) = common::post_json(
        &app,
        "/api/system/organized_series/batch_move_preview",
        &payload,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let response: serde_json::Value = body;
    let items = response["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert!(items[0]["series_id"].is_null());
    assert!(!items[0]["is_tracked"].as_bool().unwrap());
}

// POST /api/system/organized_series/batch_move — Move operation

#[tokio::test]
async fn test_batch_move_move_operation() {
    let (app, _state, tmp) = common::setup_test_app().await;
    let (_sid, dir) = create_test_series_with_dir(&app, &tmp, "Move Me Show").await;

    let new_root = tmp.path().join("archive");
    tokio::fs::create_dir_all(&new_root).await.unwrap();

    let payload = BatchMoveOrganizedSeriesPayload {
        paths: vec![dir.to_string_lossy().to_string()],
        target_root: new_root.to_string_lossy().to_string(),
        custom_paths: std::collections::HashMap::new(),
        file_operation: PathOperation::Move,
        stop_tracking: false,
    };

    let result = execute_batch_move(&app, &payload).await;
    assert_eq!(result["success_count"].as_u64().unwrap(), 1);
    assert_eq!(result["failed"].as_u64().unwrap(), 0);

    assert!(!dir.exists(), "Source directory should have been moved");

    let dest = new_root.join("Move Me Show");
    assert!(dest.exists(), "Destination directory should exist");
    assert!(
        dest.join("episode.mkv").exists(),
        "Episode file should exist at destination"
    );
}

// POST /api/system/organized_series/batch_move — Copy operation

#[tokio::test]
async fn test_batch_move_copy_operation() {
    let (app, _state, tmp) = common::setup_test_app().await;
    let (_sid, dir) = create_test_series_with_dir(&app, &tmp, "Copy Me Show").await;

    let new_root = tmp.path().join("copies");
    tokio::fs::create_dir_all(&new_root).await.unwrap();

    let payload = BatchMoveOrganizedSeriesPayload {
        paths: vec![dir.to_string_lossy().to_string()],
        target_root: new_root.to_string_lossy().to_string(),
        custom_paths: std::collections::HashMap::new(),
        file_operation: PathOperation::Copy,
        stop_tracking: false,
    };

    let result = execute_batch_move(&app, &payload).await;
    assert_eq!(result["success_count"].as_u64().unwrap(), 1);

    assert!(
        dir.exists(),
        "Source directory should still exist after copy"
    );

    let dest = new_root.join("Copy Me Show");
    assert!(
        dest.exists(),
        "Destination directory should exist after copy"
    );
    assert!(
        dest.join("episode.mkv").exists(),
        "Episode file should exist at destination"
    );
}

// POST /api/system/organized_series/batch_move — Delete operation

#[tokio::test]
async fn test_batch_move_delete_operation() {
    let (app, _state, tmp) = common::setup_test_app().await;
    let (_sid, dir) = create_test_series_with_dir(&app, &tmp, "Delete Me Show").await;

    // Delete with same root → just removes source
    let payload = BatchMoveOrganizedSeriesPayload {
        paths: vec![dir.to_string_lossy().to_string()],
        target_root: tmp.path().join("organized").to_string_lossy().to_string(),
        custom_paths: std::collections::HashMap::new(),
        file_operation: PathOperation::Delete,
        stop_tracking: false,
    };

    let result = execute_batch_move(&app, &payload).await;
    assert_eq!(result["success_count"].as_u64().unwrap(), 1);

    assert!(
        !dir.join("episode.mkv").exists(),
        "Episode file should have been deleted"
    );

    let dest_dir = tmp.path().join("organized").join("Delete Me Show");
    assert!(
        dest_dir.exists(),
        "Destination directory should exist after Delete"
    );
}

// POST /api/system/organized_series/batch_move — DoNothing operation

#[tokio::test]
async fn test_batch_move_do_nothing_operation() {
    let (app, _state, tmp) = common::setup_test_app().await;
    let (_sid, dir) = create_test_series_with_dir(&app, &tmp, "Do Nothing Show").await;

    let payload = BatchMoveOrganizedSeriesPayload {
        paths: vec![dir.to_string_lossy().to_string()],
        target_root: tmp.path().join("organized").to_string_lossy().to_string(),
        custom_paths: std::collections::HashMap::new(),
        file_operation: PathOperation::DoNothing,
        stop_tracking: false,
    };

    let result = execute_batch_move(&app, &payload).await;
    assert_eq!(result["success_count"].as_u64().unwrap(), 1);

    assert!(
        dir.exists(),
        "Source directory should still exist after DoNothing"
    );

    let dest_dir = tmp.path().join("organized").join("Do Nothing Show");
    assert!(
        dest_dir.exists(),
        "Destination directory should exist after DoNothing"
    );
}

// POST /api/system/organized_series/batch_move — Custom path

#[tokio::test]
async fn test_batch_move_custom_path() {
    let (app, _state, tmp) = common::setup_test_app().await;
    let (_sid, dir) = create_test_series_with_dir(&app, &tmp, "Custom Path Show").await;

    let custom_dest = tmp.path().join("custom_location").join("My Custom Name");
    let custom_root = tmp.path().join("custom_location");
    tokio::fs::create_dir_all(&custom_root).await.unwrap();

    let mut custom_paths = std::collections::HashMap::new();
    custom_paths.insert(
        dir.to_string_lossy().to_string(),
        custom_dest.to_string_lossy().to_string(),
    );

    let payload = BatchMoveOrganizedSeriesPayload {
        paths: vec![dir.to_string_lossy().to_string()],
        target_root: String::new(), // Irrelevant when custom_paths is used
        custom_paths,
        file_operation: PathOperation::Move,
        stop_tracking: false,
    };

    let result = execute_batch_move(&app, &payload).await;
    assert_eq!(result["success_count"].as_u64().unwrap(), 1);

    assert!(!dir.exists(), "Source directory should have been moved");

    assert!(custom_dest.exists(), "Custom destination should exist");
    assert!(
        custom_dest.join("episode.mkv").exists(),
        "Episode file should exist at custom destination"
    );
}

// POST /api/system/organized_series/batch_move — Stop tracking

#[tokio::test]
async fn test_batch_move_stop_tracking() {
    let (app, _state, tmp) = common::setup_test_app().await;
    let (series_id, dir) = create_test_series_with_dir(&app, &tmp, "Stop Tracking Show").await;

    let payload = BatchMoveOrganizedSeriesPayload {
        paths: vec![dir.to_string_lossy().to_string()],
        target_root: tmp.path().join("organized").to_string_lossy().to_string(),
        custom_paths: std::collections::HashMap::new(),
        file_operation: PathOperation::Move,
        stop_tracking: true,
    };

    let result = execute_batch_move(&app, &payload).await;
    assert_eq!(result["success_count"].as_u64().unwrap(), 1);

    let (list_status, list_body) = common::get_json(&app, "/api/series").await;
    assert_eq!(list_status, StatusCode::OK);
    let series_list: Vec<serde_json::Value> = serde_json::from_value(list_body).unwrap();
    let found = series_list
        .iter()
        .any(|s| s["id"].as_str() == Some(&series_id));
    assert!(
        !found,
        "Series should be removed from library after stop_tracking"
    );

    let (org_status, org_body) = common::get_json(&app, "/api/system/organized_series").await;
    assert_eq!(org_status, StatusCode::OK);
    let org_items: Vec<serde_json::Value> = serde_json::from_value(org_body).unwrap();
    let org_found = org_items
        .iter()
        .any(|i| i["series_id"].as_str() == Some(&series_id));
    assert!(
        !org_found,
        "Series should not appear in organized series after stop_tracking"
    );
}

// POST /api/system/organized_series/batch_move — Same path (no-op)

#[tokio::test]
async fn test_batch_move_same_path_noop() {
    let (app, _state, tmp) = common::setup_test_app().await;
    let (_sid, dir) = create_test_series_with_dir(&app, &tmp, "No-op Show").await;
    let dir_str = dir.to_string_lossy().to_string();

    // Move to the same path → should be a no-op success
    let payload = BatchMoveOrganizedSeriesPayload {
        paths: vec![dir_str.clone()],
        target_root: dir.parent().unwrap().to_string_lossy().to_string(),
        custom_paths: std::collections::HashMap::new(),
        file_operation: PathOperation::Move,
        stop_tracking: false,
    };

    let result = execute_batch_move(&app, &payload).await;
    assert_eq!(result["success_count"].as_u64().unwrap(), 1);
    assert!(dir.exists(), "Source should still exist after no-op");
}

// POST /api/system/organized_series/batch_move — Multiple series

#[tokio::test]
async fn test_batch_move_multiple_series() {
    let (app, _state, tmp) = common::setup_test_app().await;
    let (_sid1, dir1) = create_test_series_with_dir(&app, &tmp, "Multi Show A").await;
    let (_sid2, dir2) = create_test_series_with_dir(&app, &tmp, "Multi Show B").await;

    let new_root = tmp.path().join("multi_dest");
    tokio::fs::create_dir_all(&new_root).await.unwrap();

    let payload = BatchMoveOrganizedSeriesPayload {
        paths: vec![
            dir1.to_string_lossy().to_string(),
            dir2.to_string_lossy().to_string(),
        ],
        target_root: new_root.to_string_lossy().to_string(),
        custom_paths: std::collections::HashMap::new(),
        file_operation: PathOperation::Move,
        stop_tracking: false,
    };

    let result = execute_batch_move(&app, &payload).await;
    assert_eq!(result["success_count"].as_u64().unwrap(), 2);
    assert_eq!(result["failed"].as_u64().unwrap(), 0);

    assert!(new_root.join("Multi Show A").exists());
    assert!(new_root.join("Multi Show B").exists());
}

// POST /api/system/organized_series/batch_move — Partial failure

#[tokio::test]
async fn test_batch_move_partial_failure() {
    let (app, _state, tmp) = common::setup_test_app().await;
    let (_sid, valid_dir) = create_test_series_with_dir(&app, &tmp, "Partial Good").await;

    let new_root = tmp.path().join("partial_dest");
    tokio::fs::create_dir_all(&new_root).await.unwrap();

    // One valid path + one nonexistent path
    let payload = BatchMoveOrganizedSeriesPayload {
        paths: vec![
            valid_dir.to_string_lossy().to_string(),
            "/nonexistent/path/that/does/not/exist".to_string(),
        ],
        target_root: new_root.to_string_lossy().to_string(),
        custom_paths: std::collections::HashMap::new(),
        file_operation: PathOperation::Move,
        stop_tracking: false,
    };

    let result = execute_batch_move(&app, &payload).await;
    assert_eq!(result["success_count"].as_u64().unwrap(), 1);
    assert_eq!(result["failed"].as_u64().unwrap(), 1);

    assert!(new_root.join("Partial Good").exists());
}

// POST /api/system/organized_series/batch_move — Empty paths

#[tokio::test]
async fn test_batch_move_empty_paths() {
    let (app, _state, _tmp) = common::setup_test_app().await;

    let payload = BatchMoveOrganizedSeriesPayload {
        paths: vec![],
        target_root: "/media/tv".to_string(),
        custom_paths: std::collections::HashMap::new(),
        file_operation: PathOperation::Move,
        stop_tracking: false,
    };

    let result = execute_batch_move(&app, &payload).await;
    assert_eq!(result["success_count"].as_u64().unwrap(), 0);
    assert_eq!(result["failed"].as_u64().unwrap(), 0);
}

// POST /api/system/organized_series/batch_move — External collision blocked

#[tokio::test]
async fn test_batch_move_external_collision_blocked() {
    let (app, _state, tmp) = common::setup_test_app().await;
    let (_sid1, dir1) = create_test_series_with_dir(&app, &tmp, "Collision Series A").await;
    let (_sid2, dir2) = create_test_series_with_dir(&app, &tmp, "Collision Series B").await;

    // Use a custom path to make Series A's destination point to Series B's directory
    let mut custom_paths = std::collections::HashMap::new();
    custom_paths.insert(
        dir1.to_string_lossy().to_string(),
        dir2.to_string_lossy().to_string(),
    );

    let payload = BatchMoveOrganizedSeriesPayload {
        paths: vec![dir1.to_string_lossy().to_string()],
        target_root: String::new(),
        custom_paths,
        file_operation: PathOperation::Move,
        stop_tracking: false,
    };

    // Series A tries to move to Series B's directory → collision
    let result = execute_batch_move(&app, &payload).await;
    assert_eq!(
        result["failed"].as_u64().unwrap(),
        1,
        "Expected collision to cause a failure"
    );
    assert_eq!(result["success_count"].as_u64().unwrap(), 0);
}

// POST /api/system/organized_series/batch_move — Sanitized folder name (Unix only)
//
// Windows NTFS cannot host `?`/`:` directories, so this raw-Linux-source →
// sanitized-destination scenario is Unix-only; the policy engine itself is
// unit-tested on all platforms. The source folder + mapping are built directly
// because create_test_series sanitizes names, so an API-created folder can never
// contain illegal characters.

#[cfg(not(windows))]
#[tokio::test]
async fn test_batch_move_sanitized_folder_name() {
    let (app, state, tmp) = common::setup_test_app().await;

    // Build the source folder directly on disk (Linux allows `?` in names).
    let source_dir = tmp
        .path()
        .join("organized")
        .join("Sanitize:Test")
        .join("Show?");
    tokio::fs::create_dir_all(&source_dir).await.unwrap();
    tokio::fs::write(source_dir.join("episode.mkv"), b"fake video content")
        .await
        .unwrap();

    // Insert a mapping pointing at the raw path (simulating a legacy Linux folder).
    let uuid = jumbie_shared::config::generate_uuid();
    let mapping = jumbie_shared::types::MappingRule {
        target_title: "Show?".to_string(),
        name: "show".to_string(),
        series_id: uuid.clone(),
        settings: jumbie_shared::mapping::SeriesSettings {
            path: Some(source_dir.to_string_lossy().to_string()),
            ..Default::default()
        },
        ..Default::default()
    };
    state
        .db
        .upsert_series_mapping(&uuid, &mapping)
        .await
        .unwrap();

    let new_root = tmp.path().join("sanitized_dest");
    tokio::fs::create_dir_all(&new_root).await.unwrap();

    let payload = BatchMoveOrganizedSeriesPayload {
        paths: vec![source_dir.to_string_lossy().to_string()],
        target_root: new_root.to_string_lossy().to_string(),
        custom_paths: std::collections::HashMap::new(),
        file_operation: PathOperation::Move,
        stop_tracking: false,
    };

    let result = execute_batch_move(&app, &payload).await;
    assert_eq!(
        result["failed"].as_u64().unwrap(),
        0,
        "Expected no failures"
    );
    assert_eq!(result["success_count"].as_u64().unwrap(), 1);

    // The destination folder name is sanitized: leaf "Show?" gets `?` → `_`.
    let dest = new_root.join("Show_");
    assert!(
        dest.exists(),
        "Destination should exist with sanitized folder name"
    );
    assert!(
        dest.join("episode.mkv").exists(),
        "Episode file should exist at destination"
    );
}

// POST /api/system/organized_series/batch_move — orphan-folder collisions
// The org collision config governs a derived destination folder that already
// exists on disk but is not claimed by another series.

#[tokio::test]
async fn test_batch_move_orphan_destination_renames() {
    let (app, _state, tmp) = common::setup_test_app().await;
    let (_sid, dir) = create_test_series_with_dir(&app, &tmp, "Orphan Move Show").await;

    let new_root = tmp.path().join("orphan_dest");
    tokio::fs::create_dir_all(&new_root).await.unwrap();
    // Pre-create an orphan folder with the series' name at the destination.
    tokio::fs::create_dir_all(new_root.join("Orphan Move Show"))
        .await
        .unwrap();

    let payload = BatchMoveOrganizedSeriesPayload {
        paths: vec![dir.to_string_lossy().to_string()],
        target_root: new_root.to_string_lossy().to_string(),
        custom_paths: std::collections::HashMap::new(),
        file_operation: PathOperation::Move,
        stop_tracking: false,
    };

    let result = execute_batch_move(&app, &payload).await;
    assert_eq!(
        result["failed"].as_u64().unwrap(),
        0,
        "Expected no failures"
    );
    assert_eq!(result["success_count"].as_u64().unwrap(), 1);

    // Default "rename" handling → the series lands in the suffixed folder and
    // the orphan is left untouched.
    let dest = new_root.join("Orphan Move Show.001");
    assert!(
        dest.exists(),
        "Series should land in the suffixed destination folder"
    );
    assert!(
        dest.join("episode.mkv").exists(),
        "Episode file should be moved into the suffixed destination"
    );
    assert!(
        new_root.join("Orphan Move Show").exists(),
        "Orphan folder should remain untouched"
    );
}

#[tokio::test]
async fn test_batch_move_orphan_destination_skip_rejected() {
    let (app, state, tmp) = common::setup_test_app().await;
    common::set_collision_handling(&state, "skip").await;
    let (_sid, dir) = create_test_series_with_dir(&app, &tmp, "Skip Orphan Show").await;

    let new_root = tmp.path().join("skip_orphan_dest");
    tokio::fs::create_dir_all(&new_root).await.unwrap();
    tokio::fs::create_dir_all(new_root.join("Skip Orphan Show"))
        .await
        .unwrap();

    let payload = BatchMoveOrganizedSeriesPayload {
        paths: vec![dir.to_string_lossy().to_string()],
        target_root: new_root.to_string_lossy().to_string(),
        custom_paths: std::collections::HashMap::new(),
        file_operation: PathOperation::Move,
        stop_tracking: false,
    };

    let result = execute_batch_move(&app, &payload).await;
    assert_eq!(
        result["failed"].as_u64().unwrap(),
        1,
        "Skip handling should reject the move to an existing folder"
    );
    assert_eq!(result["success_count"].as_u64().unwrap(), 0);
    assert!(dir.exists());
}

// POST /api/system/organized_series/batch_move_preview — orphan resolution

#[tokio::test]
async fn test_batch_move_preview_resolves_orphan_destination() {
    let (app, _state, tmp) = common::setup_test_app().await;
    let (_sid, dir) = create_test_series_with_dir(&app, &tmp, "Preview Orphan Show").await;

    let new_root = tmp.path().join("preview_orphan_dest");
    tokio::fs::create_dir_all(&new_root).await.unwrap();
    tokio::fs::create_dir_all(new_root.join("Preview Orphan Show"))
        .await
        .unwrap();

    let payload = BatchMoveOrganizedSeriesPayload {
        paths: vec![dir.to_string_lossy().to_string()],
        target_root: new_root.to_string_lossy().to_string(),
        custom_paths: std::collections::HashMap::new(),
        file_operation: PathOperation::Move,
        stop_tracking: false,
    };

    let res = app
        .clone()
        .oneshot(common::post_json_request(
            "/api/system/organized_series/batch_move_preview",
            &payload,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["has_collisions"], false);
    let items = json["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    let dest = items[0]["destination_path"].as_str().unwrap();
    assert!(
        dest.ends_with("Preview Orphan Show.001"),
        "Preview should report the resolved destination, got: {}",
        dest
    );
}

// POST /api/system/organized_series/batch_move — Trailing slash in target_root

#[tokio::test]
async fn test_batch_move_trailing_slash_in_root() {
    let (app, _state, tmp) = common::setup_test_app().await;
    let (_sid, dir) = create_test_series_with_dir(&app, &tmp, "Trailing Slash Show").await;

    // Target root WITH trailing slash
    let new_root_dir = tmp.path().join("archive");
    let new_root = format!("{}/", new_root_dir.to_string_lossy());
    tokio::fs::create_dir_all(&new_root_dir).await.unwrap();

    let payload = BatchMoveOrganizedSeriesPayload {
        paths: vec![dir.to_string_lossy().to_string()],
        target_root: new_root,
        custom_paths: std::collections::HashMap::new(),
        file_operation: PathOperation::Move,
        stop_tracking: false,
    };

    let result = execute_batch_move(&app, &payload).await;
    assert_eq!(
        result["failed"].as_u64().unwrap(),
        0,
        "Expected no failures"
    );
    assert_eq!(result["success_count"].as_u64().unwrap(), 1);

    let dest = new_root_dir.join("Trailing Slash Show");
    assert!(
        dest.exists(),
        "Destination should exist without double slash"
    );
}

// POST /api/system/organized_series/batch_move — Empty custom path rejected

#[tokio::test]
async fn test_batch_move_empty_custom_path_rejected() {
    let (app, _state, tmp) = common::setup_test_app().await;
    let (_sid, dir) = create_test_series_with_dir(&app, &tmp, "Empty Custom").await;

    let mut custom_paths = std::collections::HashMap::new();
    custom_paths.insert(dir.to_string_lossy().to_string(), "".to_string());

    let payload = BatchMoveOrganizedSeriesPayload {
        paths: vec![dir.to_string_lossy().to_string()],
        target_root: String::new(),
        custom_paths,
        file_operation: PathOperation::Move,
        stop_tracking: false,
    };

    let (status, body) =
        common::post_json(&app, "/api/system/organized_series/batch_move", &payload).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "Body: {}", body);
}
