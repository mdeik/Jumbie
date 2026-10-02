mod common;

use std::collections::HashMap;
use std::sync::Arc;

use crate::common::TestApp;
use axum::http::StatusCode;
use jumbie_shared::{
    config::{ReleaseDateDisplayConfig, UIConfig},
    mapping::MonitorMode,
    types::{
        BatchEditSeriesPayload, BatchRemoveSeriesPayload, CreateSeriesRequest,
        CreateSeriesSettings, MonitorEpisodesPayload, RemoveSeriesPayload, ReorganizeSeriesPayload,
        SeasonOverride, SeriesInfo, SyncMetadataRequest, UpdateSeriesPayload,
    },
};
use tower::ServiceExt;

// GET /api/series

#[tokio::test]
async fn test_get_series_list_empty() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let res = app
        .oneshot(common::get_request("/api/series"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json.is_array());
    assert_eq!(json.as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn test_get_series_list_populated() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    common::create_test_series(&app, "List Show").await;
    let res = app
        .clone()
        .oneshot(common::get_request("/api/series"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(!json.as_array().unwrap().is_empty());
}

// GET /api/series/:id

#[tokio::test]
async fn test_get_series_details() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Detail Show").await;
    let res = app
        .clone()
        .oneshot(common::get_request(&format!("/api/series/{}", series_id)))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    // Returns Some<SeriesDetails> so the root is an object
    assert!(json.is_object() || json.is_null());
}

#[tokio::test]
async fn test_get_series_details_not_found() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let res = app
        .oneshot(common::get_request("/api/series/nonexistent_id_xyz"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK); // Returns Json(None) which is 200 null
}

#[tokio::test]
async fn test_series_details_shows_in_queue_when_download_queued() {
    let (app, state, _tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "In Queue Series").await;
    let ep_id = format!("{}_S01E01", series_id);

    // Seed a monitored episode with a past pub_date and no file (→ "missing")
    sqlx::query(
        "INSERT INTO episodes (episode_id, series_id, season, episode, status, monitored, meta_date)
         VALUES (?, ?, 1, 1, 'unreleased', 1, ?)",
    )
    .bind(&ep_id)
    .bind(&series_id)
    .bind(chrono::NaiveDateTime::new(
        chrono::NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
        chrono::NaiveTime::from_hms_opt(0, 0, 0).unwrap(),
    ))
    .execute(state.db.get_pool())
    .await
    .unwrap();

    // Fetch series details BEFORE queue entry — episode should be "missing"
    let res = app
        .clone()
        .oneshot(common::get_request(&format!("/api/series/{}", series_id)))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let json: serde_json::Value = common::response_json(res).await;
    let eps = json["episodes"].as_array().unwrap();
    let ep = eps
        .iter()
        .find(|e| e["unique_id"] == ep_id)
        .expect("Episode should exist");
    assert_eq!(
        ep["status"], "missing",
        "Before queue: expected 'missing', got '{}'",
        ep["status"]
    );

    // Add a download queue entry for this episode
    sqlx::query(
        "INSERT INTO download_queue (media_name, media_link, series_title, season, episode, episode_id, score, is_user_requested, is_season_pack, category, multi_targets, status)
         VALUES (?, ?, ?, ?, ?, ?, 0, 0, 0, '', '[]', 'Queued')",
    )
    .bind("Test Release")
    .bind("magnet:?xt=urn:btih:test789")
    .bind("In Queue Series")
    .bind("1")
    .bind(1)
    .bind(&ep_id)
    .execute(state.db.get_pool())
    .await
    .unwrap();

    // Fetch series details AFTER queue entry — episode should be "in_queue"
    let res = app
        .clone()
        .oneshot(common::get_request(&format!("/api/series/{}", series_id)))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let json: serde_json::Value = common::response_json(res).await;
    let eps = json["episodes"].as_array().unwrap();
    let ep = eps
        .iter()
        .find(|e| e["unique_id"] == ep_id)
        .expect("Episode should exist");
    assert_eq!(
        ep["status"], "in_queue",
        "After queue: expected 'in_queue', got '{}'",
        ep["status"]
    );
}

#[tokio::test]
async fn test_series_details_in_queue_absolute_mode() {
    let (app, state, _tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Abs Queue Show").await;

    // Update series to absolute mode
    let update_payload = UpdateSeriesPayload {
        quality_profile: "Any".to_string(),
        release_profile: "Any".to_string(),
        title: Some("Abs Queue Show".to_string()),
        path_operation: None,
        settings: jumbie_shared::mapping::SeriesSettings {
            aliases: vec![],
            reg_patterns: vec![],
            season: HashMap::new(),
            season_absolute: HashMap::new(),
            season_folder_format: None,
            episode_file_format: None,
            season_folder_format_absolute: None,
            episode_file_format_absolute: None,
            flatten_season_folders: None,
            absolute_numbering: Some(true),
            rename_episodes: None,
            search_format: None,
            search_format_absolute: None,
            path: Some("Abs Queue Show".to_string()),
            monitor_mode: Some(MonitorMode::All),
            metadata_ids: HashMap::new(),
            metadata_last_synced_at: HashMap::new(),
            last_known_dir_mtimes: HashMap::new(),
        },
    };
    let req = common::put_json_request(&format!("/api/series/{}", series_id), &update_payload);
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // In absolute mode, episode_id format is {series_id}_ABS{episode:04}
    let abs_ep = 42;
    let ep_id = format!("{}_ABS{:04}", series_id, abs_ep);

    // Seed an absolute-mode episode
    sqlx::query(
        "INSERT INTO episodes (episode_id, series_id, season, episode, status, monitored, meta_date, numbering_mode)
         VALUES (?, ?, 1, ?, 'unreleased', 1, ?, 1)",
    )
    .bind(&ep_id)
    .bind(&series_id)
    .bind(abs_ep)
    .bind(chrono::NaiveDateTime::new(
        chrono::NaiveDate::from_ymd_opt(2024, 6, 1).unwrap(),
        chrono::NaiveTime::from_hms_opt(0, 0, 0).unwrap(),
    ))
    .execute(state.db.get_pool())
    .await
    .unwrap();

    // Before queue: should be "missing"
    let res = app
        .clone()
        .oneshot(common::get_request(&format!("/api/series/{}", series_id)))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let json: serde_json::Value = common::response_json(res).await;
    let eps = json["episodes"].as_array().unwrap();
    let ep = eps
        .iter()
        .find(|e| e["unique_id"] == ep_id)
        .expect("Absolute episode should exist in response");
    assert_eq!(
        ep["status"], "missing",
        "Absolute before queue: expected 'missing', got '{}'",
        ep["status"]
    );

    // Add queue entry
    sqlx::query(
        "INSERT INTO download_queue (media_name, media_link, series_title, season, episode, episode_id, score, is_user_requested, is_season_pack, category, multi_targets, status)
         VALUES (?, ?, ?, ?, ?, ?, 0, 0, 0, '', '[]', 'Queued')",
    )
    .bind("Abs Release")
    .bind("magnet:?xt=urn:btih:absqueue")
    .bind("Abs Queue Show")
    .bind("1")
    .bind(abs_ep)
    .bind(&ep_id)
    .execute(state.db.get_pool())
    .await
    .unwrap();

    // After queue: should be "in_queue"
    let res = app
        .clone()
        .oneshot(common::get_request(&format!("/api/series/{}", series_id)))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let json: serde_json::Value = common::response_json(res).await;
    let eps = json["episodes"].as_array().unwrap();
    let ep = eps
        .iter()
        .find(|e| e["unique_id"] == ep_id)
        .expect("Absolute episode should exist");
    assert_eq!(
        ep["status"], "in_queue",
        "Absolute after queue: expected 'in_queue', got '{}'",
        ep["status"]
    );
}

// DELETE /api/series/:id

#[tokio::test]
async fn test_remove_series() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Remove Show").await;
    let payload = RemoveSeriesPayload {
        delete_configurations: false,
        delete_episode_data: false,
        delete_episodes: false,
    };
    let res = app
        .clone()
        .oneshot(common::delete_json_request(
            &format!("/api/series/{}", series_id),
            &payload,
        ))
        .await
        .unwrap();
    assert!(res.status().is_success());
}

#[tokio::test]
async fn test_remove_series_neither_flag_hides_series() {
    // When neither flag is set, the series should be hidden (hidden_in_library = true)
    // but all data preserved so unhiding later restores everything.
    let (app, state, _tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Hide Preserve").await;

    // Remove with neither flag → hide
    let payload = RemoveSeriesPayload {
        delete_configurations: false,
        delete_episode_data: false,
        delete_episodes: false,
    };
    let res = app
        .clone()
        .oneshot(common::delete_json_request(
            &format!("/api/series/{}", series_id),
            &payload,
        ))
        .await
        .unwrap();
    assert!(res.status().is_success());

    let mapping = state.db.get_series_mapping(&series_id).await.unwrap();
    assert!(mapping.is_some(), "Series mapping should still exist");
    assert!(
        mapping.unwrap().hidden_in_library,
        "Series should be marked as hidden"
    );
}

#[tokio::test]
async fn test_remove_series_config_only_resets_and_hides() {
    // With delete_configurations=true and delete_episodes=false, the mapping
    // should have its settings reset to defaults and be marked as hidden.
    // The series identity (title, id) and all episode data are preserved.
    let (app, state, _tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Config Only Reset").await;

    // First verify the series mapping exists
    let mapping_before = state
        .db
        .get_series_mapping(&series_id)
        .await
        .unwrap()
        .expect("Series mapping should exist before deletion");

    // Set some custom settings to verify they get reset.
    let payload = RemoveSeriesPayload {
        delete_configurations: true,
        delete_episode_data: false,
        delete_episodes: false,
    };
    let res = app
        .clone()
        .oneshot(common::delete_json_request(
            &format!("/api/series/{}", series_id),
            &payload,
        ))
        .await
        .unwrap();
    assert!(res.status().is_success());

    let mapping = state
        .db
        .get_series_mapping(&series_id)
        .await
        .unwrap()
        .expect("Series mapping should still exist");

    assert!(
        mapping.hidden_in_library,
        "Series should be marked as hidden"
    );
    assert_eq!(
        mapping.target_title, mapping_before.target_title,
        "target_title should be preserved"
    );
    assert_eq!(
        mapping.series_id, mapping_before.series_id,
        "series_id should be preserved"
    );

    assert!(
        mapping.release_profile.is_none(),
        "release_profile should be cleared"
    );
    assert!(
        mapping.qb_category.is_none(),
        "qb_category should be cleared"
    );
    assert!(mapping.filters.is_none(), "filters should be cleared");
    assert!(mapping.scoring.is_none(), "scoring should be cleared");
    assert!(
        mapping.quality_profile.is_none(),
        "quality_profile should be cleared"
    );
    assert!(
        mapping.settings.aliases.is_empty(),
        "aliases should be cleared"
    );
    assert!(
        !mapping.settings.absolute_numbering.unwrap_or(false),
        "absolute_numbering should be false"
    );
    assert!(
        mapping.settings.season.is_empty(),
        "season overrides should be cleared"
    );
    assert!(
        mapping.settings.reg_patterns.is_empty(),
        "reg_patterns should be cleared"
    );
    assert!(mapping.settings.path.is_none(), "path should be cleared");
    assert!(
        mapping.settings.monitor_mode.is_none(),
        "monitor_mode should be cleared"
    );
    assert!(
        mapping.settings.metadata_ids.is_empty(),
        "metadata_ids should be cleared"
    );

    // The series was just created with minimal_test_request which doesn't
    // create episode rows, but the point is that delete_series_data wasn't called.
}

#[tokio::test]
async fn test_remove_series_episodes_only_deletes_data_and_hides() {
    // With delete_episodes=true and delete_configurations=false, episode data
    // and files should be deleted, but the mapping kept. The series should be
    // marked as hidden so its identity is preserved for future re-scans.
    let (app, state, _tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Eps Only Delete").await;

    let payload = RemoveSeriesPayload {
        delete_configurations: false,
        delete_episode_data: false,
        delete_episodes: true,
    };
    let res = app
        .clone()
        .oneshot(common::delete_json_request(
            &format!("/api/series/{}", series_id),
            &payload,
        ))
        .await
        .unwrap();
    assert!(res.status().is_success());

    let mapping = state.db.get_series_mapping(&series_id).await.unwrap();
    assert!(mapping.is_some(), "Series mapping should still exist");
    assert!(
        mapping.unwrap().hidden_in_library,
        "Series should be marked as hidden after episode deletion"
    );
}

#[tokio::test]
async fn test_remove_series_both_flags_full_removal() {
    // Both flags set → full removal (equivalent to legacy delete_files=true)
    let (app, state, _tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Full Removal").await;

    let payload = RemoveSeriesPayload {
        delete_configurations: true,
        delete_episode_data: false,
        delete_episodes: true,
    };
    let res = app
        .clone()
        .oneshot(common::delete_json_request(
            &format!("/api/series/{}", series_id),
            &payload,
        ))
        .await
        .unwrap();
    assert!(res.status().is_success());

    assert!(
        state
            .db
            .get_series_mapping(&series_id)
            .await
            .unwrap()
            .is_none(),
        "Series mapping should be deleted"
    );
    let episodes = state
        .db
        .get_series_episodes_details("Full Removal", false)
        .await
        .unwrap();
    assert!(episodes.is_empty(), "Episode data should be deleted");
}

#[tokio::test]
async fn test_create_series_at_path_of_hidden_series_clears_old_data() {
    // Creating a new series at a path claimed by a hidden series should:
    let (app, state, _tmp) = common::setup_test_app().await;

    // Create a series
    let series_id = common::create_test_series(&app, "Old Hidden").await;

    // Hide it via the API (neither flag → just hide)
    let hide_payload = RemoveSeriesPayload {
        delete_configurations: false,
        delete_episode_data: false,
        delete_episodes: false,
    };
    let res = app
        .clone()
        .oneshot(common::delete_json_request(
            &format!("/api/series/{}", series_id),
            &hide_payload,
        ))
        .await
        .unwrap();
    assert!(res.status().is_success());

    let mapping = state.db.get_series_mapping(&series_id).await.unwrap();
    assert!(mapping.unwrap().hidden_in_library);

    // Now create a NEW series at the same path via the standard create endpoint
    let new_id = common::create_test_series(&app, "Old Hidden").await;

    // The new series should have a DIFFERENT UUID
    assert_ne!(
        new_id, series_id,
        "New series should get a fresh UUID, not reuse the old one"
    );

    // The old series mapping should be gone
    assert!(
        state
            .db
            .get_series_mapping(&series_id)
            .await
            .unwrap()
            .is_none(),
        "Old hidden series mapping should be deleted"
    );

    // The new series mapping should exist and NOT be hidden
    let new_mapping = state.db.get_series_mapping(&new_id).await.unwrap();
    assert!(new_mapping.is_some(), "New series mapping should exist");
    assert!(
        !new_mapping.unwrap().hidden_in_library,
        "New series should not be hidden"
    );
}

#[tokio::test]
async fn test_create_series_at_path_of_visible_series_rejected() {
    // Creating a new series at a path claimed by a visible (non-hidden) series
    // should be rejected with a path collision error.
    let (app, state, _tmp) = common::setup_test_app().await;

    // Create a series
    let series_id = common::create_test_series(&app, "Visible Existing").await;

    let mapping = state.db.get_series_mapping(&series_id).await.unwrap();
    assert!(!mapping.unwrap().hidden_in_library);

    // Attempt to create another series at the same path
    let payload = common::test_fixtures::minimal_test_request("Visible Existing");
    let res = app
        .clone()
        .oneshot(common::post_json_request("/api/series", &payload))
        .await
        .unwrap();

    assert_eq!(
        res.status(),
        StatusCode::BAD_REQUEST,
        "Creating at same path as visible series should be rejected"
    );

    // The error body carries the claiming series id so the frontend can link
    // the user to the existing series (timeout-then-re-add flow).
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        json["series_id"].as_str(),
        Some(series_id.as_str()),
        "Collision error should identify the claiming series"
    );
    assert!(
        json["error"].as_str().unwrap().contains("Visible Existing"),
        "Collision error should name the conflicting series"
    );
}

// POST /api/series — folder-level collision handling
// The org config (collision_handling + collision_rename_suffix) governs what
// happens when the destination folder already exists on disk but is NOT claimed
// by another series: rename → suffixed folder, skip → rejected, overwrite → the
// existing folder is claimed. The same SSoT helper drives the validate-path
// preview, so these tests pin down what the Add Series form actually creates.

#[tokio::test]
async fn test_create_series_at_orphan_folder_renames_with_default_config() {
    // Default collision_handling is "rename" with the DotNumeric suffix: creating
    // a series whose folder already exists on disk should create the series in
    // the next free suffixed folder (e.g. `.001`) instead of claiming the
    // existing folder.
    let (app, state, tmp) = common::setup_test_app().await;
    let org_root = tmp.path().join("organized");
    let series_dir = org_root.join("Orphan Folder");
    std::fs::create_dir_all(&series_dir).unwrap();

    let series_id = common::create_test_series(&app, "Orphan Folder").await;
    let mapping = state
        .db
        .get_series_mapping(&series_id)
        .await
        .unwrap()
        .unwrap();
    let stored = mapping.settings.path.unwrap_or_default();
    assert!(
        stored.ends_with("Orphan Folder.001"),
        "Rename handling should suffix the folder, got: {}",
        stored
    );
    // The original orphan folder must remain untouched, and the suffixed folder
    // must actually exist on disk (matches the stored mapping).
    assert!(
        series_dir.exists(),
        "Original orphan folder should be left alone"
    );
    assert!(
        std::path::Path::new(&stored).exists(),
        "Suffixed folder should exist on disk"
    );
}

#[tokio::test]
async fn test_create_series_rename_picks_next_free_suffix() {
    // When both the base folder AND the first suffixed name are taken, rename
    // handling must walk to the next free name.
    let (app, state, tmp) = common::setup_test_app().await;
    let org_root = tmp.path().join("organized");
    let base_dir = org_root.join("Suffixed Show");
    std::fs::create_dir_all(&base_dir).unwrap();
    std::fs::create_dir_all(org_root.join("Suffixed Show.001")).unwrap();

    let series_id = common::create_test_series(&app, "Suffixed Show").await;
    let mapping = state
        .db
        .get_series_mapping(&series_id)
        .await
        .unwrap()
        .unwrap();
    let stored = mapping.settings.path.unwrap_or_default();
    assert!(
        stored.ends_with("Suffixed Show.002"),
        "Rename should walk past taken suffixes, got: {}",
        stored
    );
}

#[tokio::test]
async fn test_create_series_at_orphan_folder_skip_rejected() {
    // "skip" collision handling: adding a series whose folder already exists
    // should be rejected outright.
    let (app, state, tmp) = common::setup_test_app().await;
    let org_root = tmp.path().join("organized");
    let series_dir = org_root.join("Skip Folder");
    std::fs::create_dir_all(&series_dir).unwrap();

    common::set_collision_handling(&state, "skip").await;

    let payload = common::test_fixtures::minimal_test_request("Skip Folder");
    let res = app
        .clone()
        .oneshot(common::post_json_request("/api/series", &payload))
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        StatusCode::BAD_REQUEST,
        "Skip handling should reject creation at an existing folder"
    );
}

#[tokio::test]
async fn test_create_series_at_orphan_folder_overwrite_claims() {
    // "overwrite" collision handling: the existing folder is claimed as-is
    // (this is the historical claim-existing-content behavior).
    let (app, state, tmp) = common::setup_test_app().await;
    let org_root = tmp.path().join("organized");
    let series_dir = org_root.join("Claimed Folder");
    std::fs::create_dir_all(&series_dir).unwrap();

    common::set_collision_handling(&state, "overwrite").await;

    let series_id = common::create_test_series(&app, "Claimed Folder").await;
    let mapping = state
        .db
        .get_series_mapping(&series_id)
        .await
        .unwrap()
        .unwrap();
    let stored = mapping.settings.path.unwrap_or_default();
    // Normalize both sides: the backend stores the path as resolved by
    // validate_path (dunce::canonicalize), which resolves symlinks such as
    // macOS /var → /private/var in the tmp prefix.
    assert_eq!(
        std::path::Path::new(&stored),
        jumbie::validation::normalize_path(&series_dir),
        "Overwrite should claim the existing folder"
    );
}

// POST /api/series — illegal-char policy for folder names
// The org illegal_char_policy + allow_platform_specific_chars config governs how
// the series FOLDER name is sanitized (same engine as file renames).

#[tokio::test]
async fn test_create_series_sanitizes_folder_name_per_policy() {
    // Default policy is Underscore: `:` and `?` become `_`.
    let (app, state, tmp) = common::setup_test_app().await;
    let org_root = tmp.path().join("organized");

    let series_id = common::create_test_series(&app, "Colon: Question?").await;
    let mapping = state
        .db
        .get_series_mapping(&series_id)
        .await
        .unwrap()
        .unwrap();
    let stored = mapping.settings.path.unwrap_or_default();
    assert!(
        stored.ends_with("Colon_ Question_"),
        "Folder name should be sanitized per the underscore policy, got: {}",
        stored
    );
    assert!(
        std::path::Path::new(&stored).exists(),
        "Sanitized folder should exist on disk"
    );
    assert!(!org_root.join("Colon: Question?").exists());
}

// Host-legal chars differ per OS: allow_platform_specific_chars preserves chars
// valid on the CURRENT OS. On Linux/macOS `:` and `?` are legal and kept; on
// Windows they are ALWAYS illegal (NTFS), so they are filtered to `_` even with
// the flag enabled. The gating is per-OS (`PLATFORM_SAFE_CHARS`), asserted at
// RUNTIME so this test runs on every platform instead of being cfg-skipped.
#[tokio::test]
async fn test_create_series_sanitization_respects_platform_specific_chars() {
    let on_windows = cfg!(target_os = "windows");
    let (app, state, _tmp) = common::setup_test_app().await;
    {
        let mut cfg = state.cfg.write().await;
        cfg.organization.allow_platform_specific_chars = true;
    }

    let series_id = common::create_test_series(&app, "Keep: Question?").await;
    let mapping = state
        .db
        .get_series_mapping(&series_id)
        .await
        .unwrap()
        .unwrap();
    let stored = mapping.settings.path.unwrap_or_default();
    if on_windows {
        assert!(
            stored.ends_with("Keep_ Question_"),
            "Host-illegal chars must be filtered on Windows even with the flag on, got: {}",
            stored
        );
    } else {
        assert!(
            stored.ends_with("Keep: Question?"),
            "Host-legal chars should be preserved with allow_platform_specific_chars, got: {}",
            stored
        );
    }
}

#[tokio::test]
async fn test_create_series_custom_path_is_not_sanitized_or_resolved() {
    // Explicit custom paths (resolve_collisions = false) are honored verbatim —
    // no sanitization, no folder-collision suffixing.
    let (app, state, tmp) = common::setup_test_app().await;
    let org_root = tmp.path().join("organized");
    let custom_dir = org_root.join("Custom: Path");

    let payload = jumbie_shared::types::CreateSeriesRequest {
        path: custom_dir.to_string_lossy().to_string(),
        series_name: Some("Custom Show".to_string()),
        scan_for_existing: Some(false),
        monitor_mode: None,
        quality_profile: None,
        release_profile: None,
        metadata_ids: HashMap::new(),
        search_missing_on_add: false,
        resolve_collisions: false,
        settings: Default::default(),
    };
    let res = app
        .clone()
        .oneshot(common::post_json_request("/api/series", &payload))
        .await
        .unwrap();

    // `:` is legal on Unix (stored verbatim) but ALWAYS illegal on Windows
    // (NTFS). Custom paths are honored verbatim — never sanitized — so on
    // Windows the request must be rejected cleanly (400) instead of crashing
    // with a mkdir failure (500).
    if cfg!(target_os = "windows") {
        assert_eq!(
            res.status(),
            StatusCode::BAD_REQUEST,
            "Custom paths with host-illegal chars must be rejected on Windows"
        );
        let body = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        let msg = String::from_utf8_lossy(&body).to_string();
        assert!(
            msg.contains(':'),
            "Rejection should name the illegal character, got: {}",
            msg
        );
        return;
    }

    assert_eq!(res.status(), StatusCode::CREATED);

    let series_id: String = common::response_json(res).await;
    let mapping = state
        .db
        .get_series_mapping(series_id.trim_matches('"'))
        .await
        .unwrap()
        .unwrap();
    // Normalize both sides: the backend stores the path as resolved by
    // validate_path (dunce::canonicalize), which resolves symlinks such as
    // macOS /var → /private/var in the tmp prefix.
    assert_eq!(
        mapping.settings.path.unwrap_or_default(),
        jumbie::validation::normalize_path(&custom_dir).to_string_lossy(),
        "Custom path should be stored verbatim"
    );
}

// PUT /api/series — folder collision handling on moves (edit-path modal)

#[tokio::test]
async fn test_update_series_move_to_orphan_folder_renames() {
    // Moving a series to a destination whose folder already exists (orphan) uses
    // the org collision config: default "rename" → the series lands in the next
    // free suffixed folder and the orphan stays untouched.
    let (app, state, tmp) = common::setup_test_app().await;
    let org_root = tmp.path().join("organized");
    let series_id = common::create_test_series(&app, "Move Target Show").await;
    let orphan = org_root.join("New Home");
    std::fs::create_dir_all(&orphan).unwrap();

    let update_payload = jumbie_shared::types::UpdateSeriesPayload {
        quality_profile: "Any".to_string(),
        release_profile: "Any".to_string(),
        title: Some("Move Target Show".to_string()),
        path_operation: Some(jumbie_shared::types::PathOperation::Move),
        settings: jumbie_shared::mapping::SeriesSettings {
            path: Some(orphan.to_string_lossy().to_string()),
            monitor_mode: Some(MonitorMode::All),
            absolute_numbering: Some(false),
            ..Default::default()
        },
    };
    let res = app
        .clone()
        .oneshot(common::put_json_request(
            &format!("/api/series/{}", series_id),
            &update_payload,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let mapping = state
        .db
        .get_series_mapping(&series_id)
        .await
        .unwrap()
        .unwrap();
    let stored = mapping.settings.path.unwrap_or_default();
    assert!(
        stored.ends_with("New Home.001"),
        "Move should resolve the orphan collision with a suffix, got: {}",
        stored
    );
    // The orphan is untouched and the suffixed folder holds the moved content.
    assert!(orphan.exists(), "Orphan folder should remain untouched");
    assert!(
        std::path::Path::new(&stored).exists(),
        "Suffixed destination should exist after the move"
    );
}

#[tokio::test]
async fn test_update_series_move_resolves_old_template_with_old_title() {
    // Regression: when a `${series}`-templated path and the title change in the same
    // update, the OLD path must resolve with the PRE-update title (the on-disk folder
    // was built from it) and the NEW path with the post-update title; both sanitize per
    // the org policy via `paths::resolve_template`.
    let (app, state, tmp) = common::setup_test_app().await;
    let org_root = tmp.path().join("organized");
    let series_id = common::create_test_series(&app, "Old: Title").await;

    // Simulate a stored `${series}` template (as saved by update_series when the
    // resolved folder matches the template). The on-disk folder was built from
    // the OLD title, sanitized per the default underscore policy.
    let mut mapping = state
        .db
        .get_series_mapping(&series_id)
        .await
        .unwrap()
        .unwrap();
    mapping.settings.path = Some(format!("{}/${{series}}", org_root.display()));
    state
        .db
        .upsert_series_mapping(&series_id, &mapping)
        .await
        .unwrap();

    let old_dir = org_root.join("Old_ Title");
    std::fs::create_dir_all(&old_dir).unwrap();
    let file = old_dir.join("episode.mkv");
    std::fs::write(&file, b"data").unwrap();

    let new_home = org_root.join("New Home");
    let update_payload = jumbie_shared::types::UpdateSeriesPayload {
        quality_profile: "Any".to_string(),
        release_profile: "Any".to_string(),
        title: Some("New: Title".to_string()),
        path_operation: Some(jumbie_shared::types::PathOperation::Move),
        settings: jumbie_shared::mapping::SeriesSettings {
            path: Some(new_home.to_string_lossy().to_string()),
            monitor_mode: Some(MonitorMode::All),
            absolute_numbering: Some(false),
            ..Default::default()
        },
    };
    let res = app
        .clone()
        .oneshot(common::put_json_request(
            &format!("/api/series/{}", series_id),
            &update_payload,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // The file must have been moved FROM the old-titled folder (resolved with
    // the pre-update title) TO the new destination.
    assert!(
        new_home.join("episode.mkv").exists(),
        "file should be moved to the new destination"
    );
    assert!(
        !old_dir.join("episode.mkv").exists(),
        "file should no longer be in the old folder"
    );

    let mapping = state
        .db
        .get_series_mapping(&series_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(mapping.target_title, "New: Title");
    assert_eq!(
        mapping.settings.path.unwrap_or_default(),
        new_home.to_string_lossy().to_string()
    );
}

#[tokio::test]
async fn test_update_series_move_to_existing_folder_skip_rejected() {
    // "skip" collision handling: moving to a destination whose folder already
    // exists is rejected outright.
    let (app, state, tmp) = common::setup_test_app().await;
    common::set_collision_handling(&state, "skip").await;

    let org_root = tmp.path().join("organized");
    let series_id = common::create_test_series(&app, "Skip Move Show").await;
    let orphan = org_root.join("Occupied Home");
    std::fs::create_dir_all(&orphan).unwrap();

    let update_payload = jumbie_shared::types::UpdateSeriesPayload {
        quality_profile: "Any".to_string(),
        release_profile: "Any".to_string(),
        title: Some("Skip Move Show".to_string()),
        path_operation: Some(jumbie_shared::types::PathOperation::Move),
        settings: jumbie_shared::mapping::SeriesSettings {
            path: Some(orphan.to_string_lossy().to_string()),
            monitor_mode: Some(MonitorMode::All),
            absolute_numbering: Some(false),
            ..Default::default()
        },
    };
    let res = app
        .clone()
        .oneshot(common::put_json_request(
            &format!("/api/series/{}", series_id),
            &update_payload,
        ))
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        StatusCode::BAD_REQUEST,
        "Skip handling should reject the move"
    );
}

// ${series} template paths + illegal-char policy.
// A mapping stored as a `${series}` template with an illegal-char title resolves
// (via `paths::mapping_path`) to the SANITIZED folder name; filesystem operations
// must use that sanitized folder, not the raw title.

async fn insert_template_mapping_with_sanitized_folder(
    state: &std::sync::Arc<jumbie::api::AppState>,
    tmp: &tempfile::TempDir,
    title: &str,
) -> String {
    let org_root = tmp.path().join("organized");
    let uuid = jumbie_shared::config::generate_uuid();
    let mapping = jumbie_shared::types::MappingRule {
        target_title: title.to_string(),
        name: title.to_lowercase().replace(' ', "_"),
        series_id: uuid.clone(),
        settings: jumbie_shared::mapping::SeriesSettings {
            path: Some(format!("{}/${{series}}", org_root.display())),
            ..Default::default()
        },
        ..Default::default()
    };
    state
        .db
        .upsert_series_mapping(&uuid, &mapping)
        .await
        .unwrap();

    // Create the SANITIZED folder (as the organizer would) with an episode file.
    let org_config = state.org_config().await;
    let sanitized_dir = org_root.join(jumbie::paths::sanitize_title(title, &org_config));
    std::fs::create_dir_all(&sanitized_dir).unwrap();
    std::fs::write(sanitized_dir.join("episode.mkv"), b"fake video content").unwrap();
    uuid
}

#[tokio::test]
async fn test_batch_remove_deletes_sanitized_template_folder() {
    let (app, state, tmp) = common::setup_test_app().await;
    let uuid = insert_template_mapping_with_sanitized_folder(&state, &tmp, "Show: Part").await;
    let sanitized_dir = tmp.path().join("organized").join("Show_ Part");
    assert!(sanitized_dir.exists());

    let payload = jumbie_shared::types::BatchRemoveSeriesPayload {
        series_ids: vec![uuid.clone()],
        delete_configurations: true,
        delete_episode_data: true,
        delete_episodes: true,
    };
    let res = app
        .clone()
        .oneshot(common::post_json_request(
            "/api/series/batch-delete",
            &payload,
        ))
        .await
        .unwrap();
    assert!(res.status().is_success());

    assert!(
        !sanitized_dir.exists(),
        "Batch remove should delete the SANITIZED folder (template + policy)"
    );
}

#[tokio::test]
async fn test_get_series_files_finds_sanitized_template_folder() {
    let (app, state, tmp) = common::setup_test_app().await;
    let uuid = insert_template_mapping_with_sanitized_folder(&state, &tmp, "Show: Part").await;

    let files: Vec<jumbie_shared::types::SeriesFileViewModel> = app
        .get_json(&format!("/api/series/{}/files", uuid.trim_matches('"')))
        .await;
    assert!(
        files.iter().any(|f| f.filename == "episode.mkv"),
        "Series files should be found in the sanitized template folder, got: {:?}",
        files.iter().map(|f| &f.filename).collect::<Vec<_>>()
    );
}

// POST /api/series — metadata_ids persistence

#[tokio::test]
async fn test_create_series_with_metadata_ids() {
    // Creating a series with metadata_ids in the request should persist
    // those IDs to the series settings so the edit page sees them.
    let (app, _state, _tmp) = common::setup_test_app().await;

    let mut metadata_ids = HashMap::new();
    metadata_ids.insert(
        "aaaaaaaa-aaaa-4aaa-aaaa-aaaaaaaaaaaa".to_string(),
        "tvdb-12345".to_string(),
    );
    metadata_ids.insert(
        "bbbbbbbb-bbbb-4bbb-bbbb-bbbbbbbbbbbb".to_string(),
        "tmdb-6789".to_string(),
    );

    let payload = jumbie_shared::types::CreateSeriesRequest {
        path: "Metadata IDs Show".to_string(),
        series_name: Some("Metadata IDs Show".to_string()),
        scan_for_existing: Some(false),
        monitor_mode: Some(MonitorMode::All),
        quality_profile: None,
        release_profile: None,
        metadata_ids: metadata_ids.clone(),
        search_missing_on_add: false,
        resolve_collisions: true,
        settings: Default::default(),
    };

    let res = app
        .clone()
        .oneshot(common::post_json_request("/api/series", &payload))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);

    let series_id: String = common::response_json(res).await;

    // Fetch series details and verify metadata_ids are stored
    let details: jumbie_shared::types::SeriesDetails = app
        .get_json(&format!("/api/series/{}", series_id.trim_matches('"')))
        .await;

    let stored_ids = &details.config.settings.metadata_ids;
    assert_eq!(stored_ids.len(), 2, "Both metadata_ids should be persisted");
    assert_eq!(
        stored_ids.get("aaaaaaaa-aaaa-4aaa-aaaa-aaaaaaaaaaaa"),
        Some(&"tvdb-12345".to_string()),
    );
    assert_eq!(
        stored_ids.get("bbbbbbbb-bbbb-4bbb-bbbb-bbbbbbbbbbbb"),
        Some(&"tmdb-6789".to_string()),
    );
}

// SERIES SETTINGS AT CREATION

#[tokio::test]
async fn test_create_series_with_series_settings() {
    let (app, _state, _tmp) = common::setup_test_app().await;

    // The add-series call accepts the same per-series settings as the edit page
    // (aliases, absolute numbering, search flags, rename/flatten toggles).
    let payload = CreateSeriesRequest {
        path: "Configured Show".to_string(),
        series_name: Some("Configured Show".to_string()),
        scan_for_existing: Some(false),
        monitor_mode: Some(MonitorMode::All),
        quality_profile: None,
        release_profile: None,
        metadata_ids: HashMap::new(),
        search_missing_on_add: false,
        resolve_collisions: true,
        settings: CreateSeriesSettings {
            // A duplicate and a blank entry must be dropped on save.
            aliases: vec![
                "Alt Name".to_string(),
                "Alt Name".to_string(),
                String::new(),
            ],
            absolute_numbering: Some(true),
            search_format: Some("S${season:02}E${episode:02}".to_string()),
            search_format_absolute: Some("E${episode:02}".to_string()),
            rename_episodes: Some(false),
            flatten_season_folders: Some(true),
            ..Default::default()
        },
    };

    let res = app
        .clone()
        .oneshot(common::post_json_request("/api/series", &payload))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
    let series_id: String = common::response_json(res).await;
    let series_id = series_id.trim_matches('"');

    let details: jumbie_shared::types::SeriesDetails =
        app.get_json(&format!("/api/series/{}", series_id)).await;
    let settings = &details.config.settings;

    assert_eq!(settings.aliases, vec!["Alt Name".to_string()]);
    assert_eq!(settings.absolute_numbering, Some(true));
    assert_eq!(
        settings.search_format.as_deref(),
        Some("S${season:02}E${episode:02}")
    );
    assert_eq!(
        settings.search_format_absolute.as_deref(),
        Some("E${episode:02}")
    );
    assert_eq!(settings.rename_episodes, Some(false));
    assert_eq!(settings.flatten_season_folders, Some(true));
}

// POST /api/series/batch-edit

#[tokio::test]
async fn test_batch_edit_series() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Batch Edit Show").await;

    let payload = BatchEditSeriesPayload {
        series_ids: vec![series_id],
        quality_profile: Some("Any".to_string()),
        release_profile: None,
        monitor_mode: None,
    };
    let res = app
        .clone()
        .oneshot(common::post_json_request(
            "/api/series/batch-edit",
            &payload,
        ))
        .await
        .unwrap();
    assert!(res.status().is_success());
}

// POST /api/series/batch-delete

#[tokio::test]
async fn test_batch_remove_series() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Batch Remove Show").await;

    let payload = BatchRemoveSeriesPayload {
        series_ids: vec![series_id],
        delete_configurations: false,
        delete_episode_data: false,
        delete_episodes: false,
    };
    let res = app
        .clone()
        .oneshot(common::post_json_request(
            "/api/series/batch-delete",
            &payload,
        ))
        .await
        .unwrap();
    assert!(res.status().is_success());
}

// DELETE /api/series/:id/season/:season

#[tokio::test]
async fn test_delete_season() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Season Delete Show").await;
    // Season may not exist but the endpoint should respond gracefully
    let res = app
        .oneshot(common::delete_request(&format!(
            "/api/series/{}/season/01",
            series_id
        )))
        .await
        .unwrap();
    assert!(res.status().is_success() || res.status() == StatusCode::NOT_FOUND);
}

// POST /api/series/:id/refresh

#[tokio::test]
async fn test_refresh_series() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Refresh Show").await;
    let res = app
        .clone()
        .oneshot(common::post_empty_request(&format!(
            "/api/series/{}/refresh",
            series_id
        )))
        .await
        .unwrap();
    assert!(res.status().is_success());
}

#[tokio::test]
async fn test_refresh_series_not_found() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let res = app
        .oneshot(common::post_empty_request(
            "/api/series/nonexistent_xyz/refresh",
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

// GET /api/series/:id/files

#[tokio::test]
async fn test_get_series_files() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Files Show").await;
    let res = app
        .clone()
        .oneshot(common::get_request(&format!(
            "/api/series/{}/files",
            series_id
        )))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json.is_array());
}

// PUT /api/series/:id/actions/monitor_episodes

#[tokio::test]
async fn test_monitor_episodes() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Monitor Show").await;
    let payload = MonitorEpisodesPayload {
        mode: MonitorMode::All,
    };
    let res = app
        .clone()
        .oneshot(common::put_json_request(
            &format!("/api/series/{}/actions/monitor_episodes", series_id),
            &payload,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

// POST /api/series/:id/actions/reorganize

#[tokio::test]
async fn test_reorganize_series() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Reorg Show").await;
    let payload = ReorganizeSeriesPayload {
        target_absolute: false,
    };
    let res = app
        .clone()
        .oneshot(common::post_json_request(
            &format!("/api/series/{}/actions/reorganize", series_id),
            &payload,
        ))
        .await
        .unwrap();
    // No files to reorganize so it returns 200 immediately
    assert_eq!(res.status(), StatusCode::OK);
}

// POST /api/series/:id/sync_metadata

#[tokio::test]
async fn test_sync_metadata_without_provider_rejected() {
    // No metadata provider configured → 400, consistent with /fetch_metadata.
    let (app, _state, _tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Meta Show").await;
    let payload = SyncMetadataRequest { episodes: vec![] };
    let res = app
        .clone()
        .oneshot(common::post_json_request(
            &format!("/api/series/{}/sync_metadata", series_id),
            &payload,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

// POST /api/series/:id/fetch_metadata

#[tokio::test]
async fn test_fetch_metadata_no_id_set() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "No Meta Show").await;
    // No metadata_id configured → should return 400
    let res = app
        .clone()
        .oneshot(common::post_empty_request(&format!(
            "/api/series/{}/fetch_metadata",
            series_id
        )))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

// POST /api/episodes/:id/scan_media

#[tokio::test]
async fn test_scan_episode_media_not_found() {
    // The route checks ffprobe availability before looking up the episode,
    // returning 400 Bad Request when ffprobe is absent. Skip the test when
    // ffmpeg/ffprobe aren't on PATH.
    if !common::ffprobe_available() {
        eprintln!("skipping: ffmpeg/ffprobe not found on PATH");
        return;
    }

    let (app, _state, _tmp) = common::setup_test_app().await;
    // Non-existent episode → 404
    let res = app
        .oneshot(common::post_empty_request(
            "/api/episodes/nonexistent_ep_id/scan_media",
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

// POST /api/series/details/batch

#[tokio::test]
async fn test_batch_details_empty_ids() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let req =
        common::post_json_request("/api/series/details/batch", &serde_json::json!({"ids": []}));
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let map: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(map.is_object());
    assert_eq!(map.as_object().unwrap().len(), 0);
}

#[tokio::test]
async fn test_batch_details_single_series() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let id = common::create_test_series(&app, "Batch Single").await;

    let req = common::post_json_request(
        "/api/series/details/batch",
        &serde_json::json!({"ids": [id]}),
    );
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let map: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(map.is_object());
    let obj = map.as_object().unwrap();
    assert_eq!(obj.len(), 1);
    assert!(obj.contains_key(&id));
    let details = obj.get(&id).unwrap();
    assert_eq!(details["info"]["title"], "Batch Single");
    assert!(details["episodes"].is_array());
}

#[tokio::test]
async fn test_batch_details_multiple_series() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let id1 = common::create_test_series(&app, "Batch Multi A").await;
    let id2 = common::create_test_series(&app, "Batch Multi B").await;
    let id3 = common::create_test_series(&app, "Batch Multi C").await;

    let req = common::post_json_request(
        "/api/series/details/batch",
        &serde_json::json!({"ids": [id1, id2, id3]}),
    );
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let map: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let obj = map.as_object().unwrap();
    assert_eq!(obj.len(), 3);
    assert!(obj.contains_key(&id1));
    assert!(obj.contains_key(&id2));
    assert!(obj.contains_key(&id3));
}

#[tokio::test]
async fn test_batch_details_partial_nonexistent() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let real_id = common::create_test_series(&app, "Batch Partial").await;
    let fake_id = "nonexistent-id-12345".to_string();

    let req = common::post_json_request(
        "/api/series/details/batch",
        &serde_json::json!({"ids": [real_id, fake_id]}),
    );
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let map: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let obj = map.as_object().unwrap();
    // Only the real series should be returned
    assert_eq!(obj.len(), 1);
    assert!(obj.contains_key(&real_id));
}

#[tokio::test]
async fn test_batch_details_pagination_limit() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let id1 = common::create_test_series(&app, "Batch Limit A").await;
    let id2 = common::create_test_series(&app, "Batch Limit B").await;
    let id3 = common::create_test_series(&app, "Batch Limit C").await;

    let req = common::post_json_request(
        "/api/series/details/batch",
        &serde_json::json!({"ids": [id1, id2, id3], "limit": 2}),
    );
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let map: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let obj = map.as_object().unwrap();
    assert_eq!(obj.len(), 2, "limit=2 should return only 2 series");
}

#[tokio::test]
async fn test_batch_details_pagination_offset() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let id1 = common::create_test_series(&app, "Batch Offset A").await;
    let id2 = common::create_test_series(&app, "Batch Offset B").await;
    let id3 = common::create_test_series(&app, "Batch Offset C").await;

    // offset=2 should skip the first 2 alphabetically
    let req = common::post_json_request(
        "/api/series/details/batch",
        &serde_json::json!({"ids": [id1, id2, id3], "offset": 2}),
    );
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let map: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let obj = map.as_object().unwrap();
    assert_eq!(obj.len(), 1, "offset=2 should return only the last series");
}

#[tokio::test]
async fn test_batch_details_offset_beyond_count() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let id = common::create_test_series(&app, "Batch Beyond").await;

    // offset=10 when there's only 1 series → empty result
    let req = common::post_json_request(
        "/api/series/details/batch",
        &serde_json::json!({"ids": [id], "offset": 10}),
    );
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let map: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let obj = map.as_object().unwrap();
    assert_eq!(obj.len(), 0, "offset beyond count should return empty map");
}

// POST /api/series/:id/actions/delete_episode_data

#[tokio::test]
async fn test_delete_episode_data() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "DelData Show").await;

    let res = app
        .clone()
        .oneshot(common::get_request(&format!("/api/series/{}", series_id)))
        .await
        .unwrap();
    assert!(res.status().is_success());

    // Delete episode data
    let res = app
        .clone()
        .oneshot(common::post_empty_request(&format!(
            "/api/series/{}/actions/delete_episode_data",
            series_id
        )))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);

    // Series should still exist (not hidden, not deleted)
    let res = app
        .clone()
        .oneshot(common::get_request(&format!("/api/series/{}", series_id)))
        .await
        .unwrap();
    assert!(res.status().is_success());
}

#[tokio::test]
async fn test_delete_episode_data_not_found() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let res = app
        .clone()
        .oneshot(common::post_empty_request(
            "/api/series/nonexistent/actions/delete_episode_data",
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_delete_episode_data_resets_last_synced_at() {
    let (app, state, _tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Metadata Reset Test").await;

    let mapping = state
        .db
        .get_series_mapping(&series_id)
        .await
        .unwrap()
        .unwrap();
    assert!(mapping.settings.metadata_last_synced_at.is_empty());

    // Set a metadata_last_synced_at entry so we can verify it gets cleared
    {
        let mut mapping = state
            .db
            .get_series_mapping(&series_id)
            .await
            .unwrap()
            .unwrap();
        mapping.settings.metadata_last_synced_at.insert(
            "test-instance".to_string(),
            "2024-01-01T00:00:00+00:00".to_string(),
        );
        state
            .db
            .upsert_series_mapping(&series_id, &mapping)
            .await
            .unwrap();
    }

    let mapping = state
        .db
        .get_series_mapping(&series_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        mapping
            .settings
            .metadata_last_synced_at
            .get("test-instance")
            .map(|s| s.as_str()),
        Some("2024-01-01T00:00:00+00:00")
    );

    // Call delete_episode_data endpoint
    let req = common::post_empty_request(&format!(
        "/api/series/{}/actions/delete_episode_data",
        series_id
    ));
    use tower::ServiceExt;
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);

    let mapping = state
        .db
        .get_series_mapping(&series_id)
        .await
        .unwrap()
        .unwrap();
    assert!(mapping.settings.metadata_last_synced_at.is_empty());
}

// POST /api/series/:id/actions/reset_configuration

#[tokio::test]
async fn test_reset_configuration() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "ResetCfg Show").await;

    // Reset configuration
    let res = app
        .clone()
        .oneshot(common::post_empty_request(&format!(
            "/api/series/{}/actions/reset_configuration",
            series_id
        )))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);

    // Series should still exist
    let res = app
        .clone()
        .oneshot(common::get_request(&format!("/api/series/{}", series_id)))
        .await
        .unwrap();
    assert!(res.status().is_success());

    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    // Series details should still be accessible (not null)
    assert!(json.is_object());
}

// GET /api/series scan_queue_count field

#[tokio::test]
async fn test_series_list_has_scan_queue_count() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    common::create_test_series(&app, "ScanQ Show").await;

    let series_list: Vec<SeriesInfo> = app.get_json("/api/series").await;

    assert!(!series_list.is_empty());
    for series in &series_list {
        // scan_queue_count should be present (defaults to 0 when not scanning)
        assert_eq!(
            series.scan_queue_count, 0,
            "Series '{}' should have scan_queue_count=0 when idle",
            series.title
        );
    }
}

// POST /api/series/:id/season/:season/actions/delete_episode_data

#[tokio::test]
async fn test_delete_season_episode_data() {
    let (app, state, _tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "SeasonDel Show").await;

    // Insert test episodes for season 1 and season 2
    // Seasons are stored as numeric strings (e.g. "1"), matching the format
    // used by batch_insert_metadata_episodes (ep.season.to_string()).
    state
        .db
        .insert_episode(jumbie::db::episodes::InsertEpisodeParams {
            episode_id: "ep_s1e1",
            series_id: &series_id,
            season: 1,
            episode: 1,
            file_path: Some("/tmp/ep1.mkv"),
            title: Some("Episode 1"),
            quality_profile_id: None,
            status: "downloaded",
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
    state
        .db
        .insert_episode(jumbie::db::episodes::InsertEpisodeParams {
            episode_id: "ep_s1e2",
            series_id: &series_id,
            season: 1,
            episode: 2,
            file_path: Some("/tmp/ep2.mkv"),
            title: Some("Episode 2"),
            quality_profile_id: None,
            status: "downloaded",
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
    state
        .db
        .insert_episode(jumbie::db::episodes::InsertEpisodeParams {
            episode_id: "ep_s2e1",
            series_id: &series_id,
            season: 2,
            episode: 1,
            file_path: Some("/tmp/ep3.mkv"),
            title: Some("Episode 3"),
            quality_profile_id: None,
            status: "downloaded",
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

    // Confirm 3 episodes exist
    let count = state.db.get_series_episode_count(&series_id).await.unwrap();
    assert_eq!(count, 3, "Should have 3 episodes before deletion");

    // Delete episode data for season 1 only
    let res = app
        .clone()
        .oneshot(common::post_empty_request(&format!(
            "/api/series/{}/season/1/actions/delete_episode_data",
            series_id
        )))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);

    // Only season 2 episode should remain
    let count = state.db.get_series_episode_count(&series_id).await.unwrap();
    assert_eq!(count, 1, "Should have 1 episode after deleting season 1");

    // Series should still exist
    let res = app
        .clone()
        .oneshot(common::get_request(&format!("/api/series/{}", series_id)))
        .await
        .unwrap();
    assert!(res.status().is_success());
}

#[tokio::test]
async fn test_delete_season_episode_data_not_found() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let res = app
        .clone()
        .oneshot(common::post_empty_request(
            "/api/series/nonexistent/season/1/actions/delete_episode_data",
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_delete_season_episode_data_invalid_season() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "InvalidSeasonShow").await;

    let res = app
        .clone()
        .oneshot(common::post_empty_request(&format!(
            "/api/series/{}/season/invalid/actions/delete_episode_data",
            series_id
        )))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_delete_season_episode_data_resets_last_synced_at() {
    let (app, state, _tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "SeasonSync Show").await;

    // Insert an episode so there's data to delete
    state
        .db
        .insert_episode(jumbie::db::episodes::InsertEpisodeParams {
            episode_id: "ep_s1e1",
            series_id: &series_id,
            season: 1,
            episode: 1,
            file_path: Some("/tmp/ep.mkv"),
            title: Some("Ep 1"),
            quality_profile_id: None,
            status: "downloaded",
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

    // Set metadata_last_synced_at so we can verify it gets cleared
    {
        let mut mapping = state
            .db
            .get_series_mapping(&series_id)
            .await
            .unwrap()
            .unwrap();
        mapping.settings.metadata_last_synced_at.insert(
            "test-instance".to_string(),
            "2024-06-15T12:00:00+00:00".to_string(),
        );
        state
            .db
            .upsert_series_mapping(&series_id, &mapping)
            .await
            .unwrap();
    }

    let mapping = state
        .db
        .get_series_mapping(&series_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        mapping
            .settings
            .metadata_last_synced_at
            .get("test-instance")
            .map(|s| s.as_str()),
        Some("2024-06-15T12:00:00+00:00")
    );

    // Delete season episode data
    let res = app
        .clone()
        .oneshot(common::post_empty_request(&format!(
            "/api/series/{}/season/1/actions/delete_episode_data",
            series_id
        )))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);

    let mapping = state
        .db
        .get_series_mapping(&series_id)
        .await
        .unwrap()
        .unwrap();
    assert!(mapping.settings.metadata_last_synced_at.is_empty());
}

// POST /api/series/:id/season/:season/actions/reset_configuration

#[tokio::test]
async fn test_reset_season_configuration() {
    let (app, state, _tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "SeasonCfg Show").await;

    // Add a season override for season 1
    {
        let mut mapping = state
            .db
            .get_series_mapping(&series_id)
            .await
            .unwrap()
            .unwrap();
        mapping.settings.season.insert(
            "1".to_string(),
            SeasonOverride {
                season: "1".to_string(),
                episode_start: Some(1),
                episode_end: Some(10),
                cell_count: Some(10),
                episode_offset: Some(5),
                alias_season_number: None,
                search_format: None,
                aliases: vec!["Season1Alias".to_string()],
                reg_patterns: vec![],
            },
        );
        // Also add season 2 override to verify only season 1 gets reset
        mapping.settings.season.insert(
            "2".to_string(),
            SeasonOverride {
                season: "2".to_string(),
                episode_start: None,
                episode_end: None,
                cell_count: Some(8),
                episode_offset: None,
                alias_season_number: None,
                search_format: None,
                aliases: vec![],
                reg_patterns: vec![],
            },
        );
        // Absolute-mode season 1 override — a normal-mode reset must not touch it.
        mapping.settings.season_absolute.insert(
            "1".to_string(),
            SeasonOverride {
                season: "1".to_string(),
                episode_start: None,
                episode_end: None,
                cell_count: Some(12),
                episode_offset: Some(4),
                alias_season_number: None,
                search_format: None,
                aliases: vec!["AbsoluteAlias".to_string()],
                reg_patterns: vec![],
            },
        );
        state
            .db
            .upsert_series_mapping(&series_id, &mapping)
            .await
            .unwrap();
    }

    let mapping = state
        .db
        .get_series_mapping(&series_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(mapping.settings.season.len(), 2);
    assert!(mapping.settings.season.contains_key("1"));
    assert!(mapping.settings.season.contains_key("2"));
    assert!(mapping.settings.season_absolute.contains_key("1"));

    // Reset configuration for season 1
    let res = app
        .clone()
        .oneshot(common::post_empty_request(&format!(
            "/api/series/{}/season/1/actions/reset_configuration",
            series_id
        )))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);

    // Season 1 override should be removed, season 2 should remain
    let mapping = state
        .db
        .get_series_mapping(&series_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        mapping.settings.season.len(),
        1,
        "Only season 2 override should remain"
    );
    assert!(
        !mapping.settings.season.contains_key("1"),
        "Season 1 override should be removed"
    );
    assert!(
        mapping.settings.season.contains_key("2"),
        "Season 2 override should still exist"
    );

    // The absolute-mode override for the same season key must be untouched by a
    // normal-mode reset — the two maps are independent.
    let abs = mapping
        .settings
        .season_absolute
        .get("1")
        .expect("absolute-mode override must survive a normal-mode reset");
    assert_eq!(abs.cell_count, Some(12));
    assert_eq!(abs.episode_offset, Some(4));
    assert_eq!(abs.aliases, vec!["AbsoluteAlias".to_string()]);

    // Series should still be accessible
    let res = app
        .clone()
        .oneshot(common::get_request(&format!("/api/series/{}", series_id)))
        .await
        .unwrap();
    assert!(res.status().is_success());
}

#[tokio::test]
async fn test_reset_season_configuration_absolute_keeps_the_season() {
    let (app, state, _tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "AbsSeasonCfg Show").await;

    {
        let mut mapping = state
            .db
            .get_series_mapping(&series_id)
            .await
            .unwrap()
            .unwrap();
        mapping.settings.absolute_numbering = Some(true);
        mapping.settings.season_absolute.insert(
            "1".to_string(),
            SeasonOverride {
                season: "1".to_string(),
                episode_start: Some(1),
                episode_end: Some(10),
                cell_count: Some(10),
                episode_offset: Some(5),
                alias_season_number: None,
                search_format: None,
                aliases: vec!["AbsAlias".to_string()],
                reg_patterns: vec![],
            },
        );
        // Normal-mode season 1 override — an absolute-mode reset must not touch it.
        mapping.settings.season.insert(
            "1".to_string(),
            SeasonOverride {
                season: "1".to_string(),
                episode_start: None,
                episode_end: None,
                cell_count: Some(7),
                episode_offset: Some(2),
                alias_season_number: None,
                search_format: None,
                aliases: vec!["NormalAlias".to_string()],
                reg_patterns: vec![],
            },
        );
        state
            .db
            .upsert_series_mapping(&series_id, &mapping)
            .await
            .unwrap();
    }

    let res = app
        .clone()
        .oneshot(common::post_empty_request(&format!(
            "/api/series/{}/season/1/actions/reset_configuration",
            series_id
        )))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);

    // Absolute numbering has one season: its override is reset in place, not
    // removed, so the series keeps its season.
    let mapping = state
        .db
        .get_series_mapping(&series_id)
        .await
        .unwrap()
        .unwrap();
    let kept = mapping
        .settings
        .season_absolute
        .get("1")
        .expect("absolute season override must be kept");
    assert_eq!(kept.cell_count, None);
    assert_eq!(kept.episode_offset, None);
    assert!(kept.aliases.is_empty());

    // The normal-mode override for the same season key must be untouched by an
    // absolute-mode reset — the two maps are independent.
    let normal = mapping
        .settings
        .season
        .get("1")
        .expect("normal-mode override must survive an absolute-mode reset");
    assert_eq!(normal.cell_count, Some(7));
    assert_eq!(normal.episode_offset, Some(2));
    assert_eq!(normal.aliases, vec!["NormalAlias".to_string()]);
}

#[tokio::test]
async fn test_reset_season_configuration_not_found() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let res = app
        .clone()
        .oneshot(common::post_empty_request(
            "/api/series/nonexistent/season/1/actions/reset_configuration",
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_reset_season_configuration_invalid_season() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "InvalidCfg Show").await;

    let res = app
        .clone()
        .oneshot(common::post_empty_request(&format!(
            "/api/series/{}/season/abc/actions/reset_configuration",
            series_id
        )))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_reset_season_configuration_no_override() {
    // Resetting config for a season without overrides should be a no-op (still succeed)
    let (app, state, _tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "NoOverride Show").await;

    let mapping = state
        .db
        .get_series_mapping(&series_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(mapping.settings.season.len(), 0);

    let res = app
        .clone()
        .oneshot(common::post_empty_request(&format!(
            "/api/series/{}/season/1/actions/reset_configuration",
            series_id
        )))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);

    // Still no overrides
    let mapping = state
        .db
        .get_series_mapping(&series_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(mapping.settings.season.len(), 0);
}

// metadata_last_synced_at prune behavior on update_series

#[tokio::test]
async fn test_update_series_prunes_timestamp_on_id_change() {
    let (app, state, _tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Prune Test").await;
    let instance_id = "tvdb-instance-uuid";

    // Set a known metadata_id and sync timestamp
    {
        let mut mapping = state
            .db
            .get_series_mapping(&series_id)
            .await
            .unwrap()
            .unwrap();
        mapping
            .settings
            .metadata_ids
            .insert(instance_id.to_string(), "old-id-123".to_string());
        mapping.settings.metadata_last_synced_at.insert(
            instance_id.to_string(),
            "2024-01-01T00:00:00+00:00".to_string(),
        );
        state
            .db
            .upsert_series_mapping(&series_id, &mapping)
            .await
            .unwrap();
    }

    {
        let mapping = state
            .db
            .get_series_mapping(&series_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            mapping
                .settings
                .metadata_ids
                .get(instance_id)
                .map(|s| s.as_str()),
            Some("old-id-123")
        );
        assert!(
            mapping
                .settings
                .metadata_last_synced_at
                .contains_key(instance_id)
        );
    }

    // Update the metadata ID (same instance id, different external ID)
    let update_payload = UpdateSeriesPayload {
        quality_profile: "Any".to_string(),
        release_profile: "Any".to_string(),
        title: Some("Prune Test".to_string()),
        path_operation: None,
        settings: jumbie_shared::mapping::SeriesSettings {
            path: Some("Prune Test".to_string()),
            monitor_mode: Some(MonitorMode::All),
            absolute_numbering: Some(false),
            metadata_ids: HashMap::from([(instance_id.to_string(), "new-id-456".to_string())]),
            ..Default::default()
        },
    };

    let req = common::put_json_request(&format!("/api/series/{}", series_id), &update_payload);
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let mapping = state
        .db
        .get_series_mapping(&series_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        mapping
            .settings
            .metadata_ids
            .get(instance_id)
            .map(|s| s.as_str()),
        Some("new-id-456"),
        "metadata_id should be updated"
    );
    assert!(
        !mapping
            .settings
            .metadata_last_synced_at
            .contains_key(instance_id),
        "timestamp should be pruned when metadata ID changes"
    );
}

#[tokio::test]
async fn test_update_series_preserves_timestamp_on_noop() {
    let (app, state, _tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Noop Test").await;
    let instance_id = "tvdb-instance-uuid";

    // Set a known metadata_id and sync timestamp
    {
        let mut mapping = state
            .db
            .get_series_mapping(&series_id)
            .await
            .unwrap()
            .unwrap();
        mapping
            .settings
            .metadata_ids
            .insert(instance_id.to_string(), "same-id".to_string());
        mapping.settings.metadata_last_synced_at.insert(
            instance_id.to_string(),
            "2024-06-15T12:00:00+00:00".to_string(),
        );
        state
            .db
            .upsert_series_mapping(&series_id, &mapping)
            .await
            .unwrap();
    }

    // Update with the EXACT same metadata_ids (no-op for IDs)
    let update_payload = UpdateSeriesPayload {
        quality_profile: "Any".to_string(),
        release_profile: "Any".to_string(),
        title: Some("Noop Test".to_string()),
        path_operation: None,
        settings: jumbie_shared::mapping::SeriesSettings {
            path: Some("Noop Test".to_string()),
            monitor_mode: Some(MonitorMode::All),
            absolute_numbering: Some(false),
            metadata_ids: HashMap::from([(instance_id.to_string(), "same-id".to_string())]),
            ..Default::default()
        },
    };

    let req = common::put_json_request(&format!("/api/series/{}", series_id), &update_payload);
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let mapping = state
        .db
        .get_series_mapping(&series_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        mapping
            .settings
            .metadata_last_synced_at
            .get(instance_id)
            .map(|s| s.as_str()),
        Some("2024-06-15T12:00:00+00:00"),
        "timestamp should be preserved when metadata ID doesn't change"
    );
}

#[tokio::test]
async fn test_update_series_removes_provider_cleans_up_timestamp() {
    let (app, state, _tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "RemoveProv Test").await;
    let instance_a = "instance-a-uuid";
    let instance_b = "instance-b-uuid";

    // Set two providers with timestamps
    {
        let mut mapping = state
            .db
            .get_series_mapping(&series_id)
            .await
            .unwrap()
            .unwrap();
        mapping
            .settings
            .metadata_ids
            .insert(instance_a.to_string(), "id-a".to_string());
        mapping
            .settings
            .metadata_ids
            .insert(instance_b.to_string(), "id-b".to_string());
        mapping.settings.metadata_last_synced_at.insert(
            instance_a.to_string(),
            "2024-01-01T00:00:00+00:00".to_string(),
        );
        mapping.settings.metadata_last_synced_at.insert(
            instance_b.to_string(),
            "2024-06-15T00:00:00+00:00".to_string(),
        );
        state
            .db
            .upsert_series_mapping(&series_id, &mapping)
            .await
            .unwrap();
    }

    // Remove instance_b entirely
    let update_payload = UpdateSeriesPayload {
        quality_profile: "Any".to_string(),
        release_profile: "Any".to_string(),
        title: Some("RemoveProv Test".to_string()),
        path_operation: None,
        settings: jumbie_shared::mapping::SeriesSettings {
            path: Some("RemoveProv Test".to_string()),
            monitor_mode: Some(MonitorMode::All),
            absolute_numbering: Some(false),
            metadata_ids: HashMap::from([(instance_a.to_string(), "id-a".to_string())]),
            ..Default::default()
        },
    };

    let req = common::put_json_request(&format!("/api/series/{}", series_id), &update_payload);
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let mapping = state
        .db
        .get_series_mapping(&series_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(mapping.settings.metadata_ids.len(), 1);
    assert!(
        mapping
            .settings
            .metadata_last_synced_at
            .contains_key(instance_a),
        "instance_a timestamp should survive"
    );
    assert!(
        !mapping
            .settings
            .metadata_last_synced_at
            .contains_key(instance_b),
        "instance_b timestamp should be cleaned up"
    );
}

// POST /api/series/batch-fetch-metadata

/// Register the mock metadata plugin in the given app state.
async fn register_batch_mock_plugin(state: &Arc<jumbie::api::AppState>, plugin_instance_id: &str) {
    state
        .plugin_manager
        .write()
        .await
        .add_internal_plugin(Arc::new(common::MockMetadataPlugin {
            instance_id: plugin_instance_id.to_string(),
            display_name: "Batch Mock Metadata".to_string(),
            capabilities: vec![jumbie_shared::plugin::Capability::MetadataProviderNormal],
            series_identifier_label: Some("Batch Mock ID".to_string()),
            series_name: "Batch Provider Title".to_string(),
            overview: "Batch test overview.".to_string(),
            aliases: vec![],
            episodes: vec![],
        }));
}

#[tokio::test]
async fn test_batch_fetch_metadata_no_provider() {
    let (app, _state, _tmp) = common::setup_test_app().await;

    let payload = serde_json::json!({
        "series_ids": ["nonexistent-series"]
    });

    let res = app
        .clone()
        .oneshot(common::post_json_request(
            "/api/series/batch-fetch-metadata",
            &payload,
        ))
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_batch_fetch_metadata_no_metadata_id() {
    let (app, state, _tmp) = common::setup_test_app().await;

    // Register a mock metadata provider so the check passes.
    register_batch_mock_plugin(&state, "mock-instance").await;

    // Create a series — it will have no metadata_id by default.
    let series_id = common::create_test_series(&app, "No Meta Batch").await;

    let payload = serde_json::json!({
        "series_ids": [series_id]
    });

    let res = app
        .clone()
        .oneshot(common::post_json_request(
            "/api/series/batch-fetch-metadata",
            &payload,
        ))
        .await
        .unwrap();

    // Provider is active → 200, series without metadata_id are silently skipped.
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_batch_fetch_metadata_with_ids() {
    let (app, state, _tmp) = common::setup_test_app().await;

    // Register a mock metadata provider.
    register_batch_mock_plugin(&state, "mock-instance").await;

    // Create a series and give it a metadata_id so it gets picked up.
    let series_id = common::create_test_series(&app, "Meta Batch").await;

    // Set a metadata_id on the series.
    {
        let mut mapping = state
            .db
            .get_series_mapping(&series_id)
            .await
            .unwrap()
            .unwrap();
        mapping
            .settings
            .metadata_ids
            .insert("mock-instance".to_string(), "test_mock_456".to_string());
        state
            .db
            .upsert_series_mapping(&series_id, &mapping)
            .await
            .unwrap();
    }

    let payload = serde_json::json!({
        "series_ids": [series_id]
    });

    let res = app
        .clone()
        .oneshot(common::post_json_request(
            "/api/series/batch-fetch-metadata",
            &payload,
        ))
        .await
        .unwrap();

    // Provider active + series with metadata_id → 200, background fetch initiated.
    assert_eq!(res.status(), StatusCode::OK);
}

// Missing status uses time-of-day, not just date

#[tokio::test]
async fn test_series_details_missing_status_uses_time_not_just_date() {
    let (app, state, _tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Time Test").await;
    let ep_id = format!("{}_S01E01", series_id);

    // Seed an episode with a pub_date set to "today but a few hours from now".
    // The status should be "unreleased" because the precise release time has
    // NOT passed yet (the old bug: it would be "missing" because the date matches).
    let now = chrono::Utc::now().naive_utc();
    // "Future release" = now + 6 hours (still today, but time hasn't come yet)
    let future_release = now + chrono::Duration::hours(6);

    sqlx::query(
        "INSERT INTO episodes (episode_id, series_id, season, episode, status, monitored, meta_date)
         VALUES (?, ?, 1, 1, 'unreleased', 1, ?)",
    )
    .bind(&ep_id)
    .bind(&series_id)
    .bind(future_release)
    .execute(state.db.get_pool())
    .await
    .unwrap();

    // Fetch series details
    let res = app
        .clone()
        .oneshot(common::get_request(&format!("/api/series/{}", series_id)))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let json: serde_json::Value = common::response_json(res).await;
    let eps = json["episodes"].as_array().unwrap();
    let ep = eps
        .iter()
        .find(|e| e["unique_id"] == ep_id)
        .expect("Episode should exist");

    assert_eq!(
        ep["status"], "unreleased",
        "Episode with future time-of-day should be 'unreleased', got '{}'",
        ep["status"]
    );
}

#[tokio::test]
async fn test_series_details_missing_status_when_time_passed() {
    let (app, state, _tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Time Passed Test").await;
    let ep_id = format!("{}_S01E01", series_id);

    // Seed an episode with a pub_date set to "today but a few hours ago".
    // The status should be "missing" because the release time has passed.
    let now = chrono::Utc::now().naive_utc();
    let past_release = now - chrono::Duration::hours(2);

    sqlx::query(
        "INSERT INTO episodes (episode_id, series_id, season, episode, status, monitored, meta_date)
         VALUES (?, ?, 1, 1, 'unreleased', 1, ?)",
    )
    .bind(&ep_id)
    .bind(&series_id)
    .bind(past_release)
    .execute(state.db.get_pool())
    .await
    .unwrap();

    // Fetch series details
    let res = app
        .clone()
        .oneshot(common::get_request(&format!("/api/series/{}", series_id)))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let json: serde_json::Value = common::response_json(res).await;
    let eps = json["episodes"].as_array().unwrap();
    let ep = eps
        .iter()
        .find(|e| e["unique_id"] == ep_id)
        .expect("Episode should exist");

    assert_eq!(
        ep["status"], "missing",
        "Episode with past time-of-day should be 'missing', got '{}'",
        ep["status"]
    );
}

// monitored_missing_count respects release date display preferences

fn past_dt() -> chrono::NaiveDateTime {
    chrono::NaiveDateTime::new(
        chrono::NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
        chrono::NaiveTime::from_hms_opt(0, 0, 0).unwrap(),
    )
}

fn future_dt() -> chrono::NaiveDateTime {
    chrono::NaiveDateTime::new(
        chrono::NaiveDate::from_ymd_opt(2099, 1, 1).unwrap(),
        chrono::NaiveTime::from_hms_opt(0, 0, 0).unwrap(),
    )
}

async fn set_release_date_prefs(
    app: &axum::Router,
    order: Vec<&str>,
    metadata_enabled: bool,
    source_enabled: bool,
    estimated_enabled: bool,
) {
    let prefs = UIConfig {
        release_date_display: ReleaseDateDisplayConfig {
            order: order.into_iter().map(|s| s.to_string()).collect(),
            metadata_enabled,
            source_enabled,
            estimated_enabled,
        },
        ..Default::default()
    };
    let req = common::put_json_request("/api/config/ui_preferences", &prefs);
    let res = app.clone().oneshot(req).await.unwrap();
    assert!(res.status().is_success(), "Failed to set UI preferences");
}

async fn seed_ep_for_missing_count_test(
    state: &Arc<jumbie::api::AppState>,
    series_id: &str,
    ep_id: &str,
    season: i32,
    ep_num: i32,
    status: &str,
    dates: common::EpisodeDates,
) {
    state
        .db
        .insert_episode(jumbie::db::episodes::InsertEpisodeParams {
            episode_id: ep_id,
            series_id,
            season,
            episode: ep_num,
            file_path: None,
            title: Some("Test Ep"),
            quality_profile_id: None,
            status,
            meta_date: dates.meta_date,
            est_date: dates.est_date,
            metadata_ids: &std::collections::HashMap::new(),
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
async fn test_monitored_missing_count_preferred_date_past() {
    // User prefers estimated (enabled, priority 1) over metadata (enabled, priority 2).
    // Episode has estimated = past, metadata = future → should count.
    let (app, state, _tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "MissingPrefPast").await;
    let ep_id = format!("{}_S01E01", series_id);

    set_release_date_prefs(&app, vec!["estimated", "metadata"], true, false, true).await;

    seed_ep_for_missing_count_test(
        &state,
        &series_id,
        &ep_id,
        1,
        1,
        "unreleased",
        common::EpisodeDates {
            meta_date: Some(future_dt()),
            source_date: None,
            est_date: Some(past_dt()),
        },
    )
    .await;

    let list: Vec<SeriesInfo> = app.get_json("/api/series").await;
    let our = list
        .iter()
        .find(|s| s.id == series_id)
        .expect("Series in list");
    assert_eq!(
        our.monitored_missing_count, 1,
        "Preferred estimated (past) should count, got {}",
        our.monitored_missing_count
    );
}

#[tokio::test]
async fn test_monitored_missing_count_preferred_date_future() {
    // User prefers estimated (enabled, priority 1) over metadata (enabled, priority 2).
    // Episode has estimated = future, metadata = past → should NOT count
    // because the resolved effective date is future.
    let (app, state, _tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "MissingPrefFuture").await;
    let ep_id = format!("{}_S01E01", series_id);

    set_release_date_prefs(&app, vec!["estimated", "metadata"], true, false, true).await;

    seed_ep_for_missing_count_test(
        &state,
        &series_id,
        &ep_id,
        1,
        1,
        "unreleased",
        common::EpisodeDates {
            meta_date: Some(past_dt()),
            source_date: None,
            est_date: Some(future_dt()),
        },
    )
    .await;

    let list: Vec<SeriesInfo> = app.get_json("/api/series").await;
    let our = list
        .iter()
        .find(|s| s.id == series_id)
        .expect("Series in list");
    assert_eq!(
        our.monitored_missing_count, 0,
        "Preferred estimated (future) should NOT count, got {}",
        our.monitored_missing_count
    );
}

#[tokio::test]
async fn test_monitored_missing_count_preferred_none_fallback() {
    // User prefers estimated (enabled, priority 1), but episode has no estimated date.
    // Metadata (enabled, priority 2) has past date → should count via fallback.
    let (app, state, _tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "MissingPrefFallback").await;
    let ep_id = format!("{}_S01E01", series_id);

    set_release_date_prefs(&app, vec!["estimated", "metadata"], true, false, true).await;

    seed_ep_for_missing_count_test(
        &state,
        &series_id,
        &ep_id,
        1,
        1,
        "unreleased",
        common::EpisodeDates {
            meta_date: Some(past_dt()),
            source_date: None,
            est_date: None,
        },
    )
    .await;

    let list: Vec<SeriesInfo> = app.get_json("/api/series").await;
    let our = list
        .iter()
        .find(|s| s.id == series_id)
        .expect("Series in list");
    assert_eq!(
        our.monitored_missing_count, 1,
        "Fallback to metadata (past) should count, got {}",
        our.monitored_missing_count
    );
}

#[tokio::test]
async fn test_monitored_missing_count_disabled_source_ignored() {
    // User has metadata enabled but estimated disabled.
    // Episode has estimated = past, metadata = future → should NOT count
    // because estimated is disabled.
    let (app, state, _tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "MissingDisabled").await;
    let ep_id = format!("{}_S01E01", series_id);

    set_release_date_prefs(&app, vec!["metadata", "estimated"], true, false, false).await;

    seed_ep_for_missing_count_test(
        &state,
        &series_id,
        &ep_id,
        1,
        1,
        "unreleased",
        common::EpisodeDates {
            meta_date: Some(future_dt()),
            source_date: None,
            est_date: Some(past_dt()),
        },
    )
    .await;

    let list: Vec<SeriesInfo> = app.get_json("/api/series").await;
    let our = list
        .iter()
        .find(|s| s.id == series_id)
        .expect("Series in list");
    assert_eq!(
        our.monitored_missing_count, 0,
        "Disabled estimated should NOT count even if past, got {}",
        our.monitored_missing_count
    );
}

#[tokio::test]
async fn test_monitored_missing_count_all_sources_disabled() {
    // All sources disabled → effective date is None → nothing counts as missing.
    let (app, state, _tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "MissingAllOff").await;
    let ep_id = format!("{}_S01E01", series_id);

    set_release_date_prefs(&app, vec!["metadata"], false, false, false).await;

    seed_ep_for_missing_count_test(
        &state,
        &series_id,
        &ep_id,
        1,
        1,
        "unreleased",
        common::EpisodeDates {
            meta_date: Some(past_dt()),
            source_date: None,
            est_date: None,
        },
    )
    .await;

    let list: Vec<SeriesInfo> = app.get_json("/api/series").await;
    let our = list
        .iter()
        .find(|s| s.id == series_id)
        .expect("Series in list");
    assert_eq!(
        our.monitored_missing_count, 0,
        "All sources disabled should count 0, got {}",
        our.monitored_missing_count
    );
}

#[tokio::test]
async fn test_monitored_missing_count_in_queue_not_counted() {
    // Episode in download queue should NOT be counted as missing.
    let (app, state, _tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "MissingInQueue").await;
    let ep_id = format!("{}_S01E01", series_id);

    set_release_date_prefs(&app, vec!["metadata"], true, false, false).await;

    seed_ep_for_missing_count_test(
        &state,
        &series_id,
        &ep_id,
        1,
        1,
        "unreleased",
        common::EpisodeDates {
            meta_date: Some(past_dt()),
            source_date: None,
            est_date: None,
        },
    )
    .await;

    // Put the episode in the download queue
    sqlx::query(
        "INSERT INTO download_queue (media_name, media_link, series_title, season, episode, episode_id, score, is_user_requested, is_season_pack, category, multi_targets, status)
         VALUES (?, ?, ?, ?, ?, ?, 0, 0, 0, '', '[]', 'Queued')",
    )
    .bind("Test Release")
    .bind("magnet:?xt=urn:btih:test")
    .bind("MissingInQueue")
    .bind("1")
    .bind(1)
    .bind(&ep_id)
    .execute(state.db.get_pool())
    .await
    .unwrap();

    let list: Vec<SeriesInfo> = app.get_json("/api/series").await;
    let our = list
        .iter()
        .find(|s| s.id == series_id)
        .expect("Series in list");
    assert_eq!(
        our.monitored_missing_count, 0,
        "In-queue episode should NOT count as missing, got {}",
        our.monitored_missing_count
    );
    assert_eq!(
        our.queued_count, 1,
        "In-queue episode should count as queued (yellow), got {}",
        our.queued_count
    );
}

#[tokio::test]
async fn test_queued_count_includes_unmonitored_episodes() {
    // An UNMONITORED episode in the download queue must still mark the series as
    // queued (yellow) — monitoring only governs the red "missing" count.
    let (app, state, _tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Unmonitored Queue").await;
    let ep_id = format!("{}_S01E01", series_id);
    let past = past_dt();

    sqlx::query(
        "INSERT INTO episodes (episode_id, series_id, season, episode, status, monitored, meta_date)
         VALUES (?, ?, 1, 1, 'unreleased', 0, ?)",
    )
    .bind(&ep_id)
    .bind(&series_id)
    .bind(past)
    .execute(state.db.get_pool())
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO download_queue (media_name, media_link, series_title, season, episode, episode_id, score, is_user_requested, is_season_pack, category, multi_targets, status)
         VALUES (?, ?, ?, ?, ?, ?, 0, 1, 0, '', '[]', 'Queued')",
    )
    .bind("Test Release")
    .bind("magnet:?xt=urn:btih:unmonitored")
    .bind("Unmonitored Queue")
    .bind("1")
    .bind(1)
    .bind(&ep_id)
    .execute(state.db.get_pool())
    .await
    .unwrap();

    let list: Vec<SeriesInfo> = app.get_json("/api/series").await;
    let our = list
        .iter()
        .find(|s| s.id == series_id)
        .expect("Series in list");
    assert_eq!(
        our.queued_count, 1,
        "unmonitored in-queue episode must count as queued, got {}",
        our.queued_count
    );
    assert_eq!(
        our.monitored_missing_count, 0,
        "unmonitored episode must not count as missing, got {}",
        our.monitored_missing_count
    );
}

// POST /api/series/:id/fetch_metadata (empty response)

#[tokio::test]
async fn test_fetch_metadata_empty_episodes() {
    // Provider returns 200 OK with 0 episodes → should get a 400 error with
    // a clear message instead of silently succeeding.
    let (app, state, _tmp) = common::setup_test_app().await;

    // Register a mock provider that returns empty episodes.
    register_batch_mock_plugin(&state, "mock-instance-empty").await;

    // Create a series and give it a metadata_id.
    let series_id = common::create_test_series(&app, "Empty Meta").await;
    {
        let mut mapping = state
            .db
            .get_series_mapping(&series_id)
            .await
            .unwrap()
            .unwrap();
        mapping.settings.metadata_ids.insert(
            "mock-instance-empty".to_string(),
            "nonexistent-999".to_string(),
        );
        state
            .db
            .upsert_series_mapping(&series_id, &mapping)
            .await
            .unwrap();
    }

    // Fetch metadata — the mock returns empty episodes, so expect 400.
    let res = app
        .clone()
        .oneshot(common::post_empty_request(&format!(
            "/api/series/{}/fetch_metadata",
            series_id
        )))
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::BAD_REQUEST);

    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let error_msg = json["error"].as_str().unwrap_or("");
    assert!(
        error_msg.contains("No episodes found"),
        "Expected 'No episodes found' error, got: {}",
        error_msg
    );
}

// Series list episode totals must honour a season's configured `cell_count`
// (SSoT `SeriesSettings::expected_episode_count`), while the downloaded count
// always reflects real downloads — even outside the configured range.

fn cell_count_override(season: &str, cell_count: i32) -> SeasonOverride {
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
    }
}

async fn set_season_override(
    state: &Arc<jumbie::api::AppState>,
    series_id: &str,
    override_rule: SeasonOverride,
) {
    let mut mapping = state
        .db
        .get_series_mapping(series_id)
        .await
        .unwrap()
        .expect("series mapping exists");
    mapping
        .settings
        .season
        .insert(override_rule.season.clone(), override_rule);
    state
        .db
        .upsert_series_mapping(series_id, &mapping)
        .await
        .unwrap();
}

async fn seed_season_episode(
    state: &Arc<jumbie::api::AppState>,
    series_id: &str,
    episode: i32,
    status: &str,
    file_path: Option<&str>,
) {
    let eid = format!("{}_S01E{:02}", series_id, episode);
    state
        .db
        .insert_episode(jumbie::db::episodes::InsertEpisodeParams {
            episode_id: &eid,
            series_id,
            season: 1,
            episode,
            file_path,
            title: None,
            quality_profile_id: None,
            status,
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

async fn series_counts(app: &axum::Router, series_id: &str) -> (i32, i32) {
    let list: Vec<SeriesInfo> = app.get_json("/api/series").await;
    let our = list
        .iter()
        .find(|s| s.id == series_id)
        .expect("Series in list");
    our.episodes_counts
}

#[tokio::test]
async fn test_series_list_total_uses_season_cell_count() {
    let (app, state, _tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Cell Count Show").await;
    set_season_override(&state, &series_id, cell_count_override("1", 12)).await;

    // Only 5 of the 12 configured cells exist yet.
    for ep in 1..=5 {
        seed_season_episode(&state, &series_id, ep, "organized", Some("/media/s1.mkv")).await;
    }

    assert_eq!(
        series_counts(&app, &series_id).await,
        (5, 12),
        "total must use the configured cell count, not the episodes present"
    );
}

#[tokio::test]
async fn test_series_list_total_excludes_out_of_range_cells() {
    let (app, state, _tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Out Of Range Show").await;

    // 100 cells, but the season is bounded to episodes 1..=12 → 12 in-range cells.
    set_season_override(
        &state,
        &series_id,
        SeasonOverride {
            episode_start: Some(1),
            episode_end: Some(12),
            ..cell_count_override("1", 100)
        },
    )
    .await;

    for ep in 1..=3 {
        seed_season_episode(&state, &series_id, ep, "organized", Some("/media/s1.mkv")).await;
    }

    assert_eq!(
        series_counts(&app, &series_id).await,
        (3, 12),
        "cells outside [episode_start, episode_end] must not count toward the total"
    );
}

#[tokio::test]
async fn test_series_list_downloaded_counts_episodes_outside_cell_count() {
    let (app, state, _tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Beyond Range Show").await;
    set_season_override(&state, &series_id, cell_count_override("1", 3)).await;

    // 5 downloaded episodes (2 beyond the 3-cell range) plus one queued episode.
    for ep in 1..=5 {
        seed_season_episode(&state, &series_id, ep, "organized", Some("/media/s1.mkv")).await;
    }
    seed_season_episode(&state, &series_id, 6, "in_queue", None).await;

    assert_eq!(
        series_counts(&app, &series_id).await,
        (5, 3),
        "downloaded counts every real download (even beyond cell_count); in-queue is excluded"
    );
}

#[tokio::test]
async fn test_series_list_total_uses_absolute_cell_count() {
    let (app, state, _tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Absolute Cell Show").await;

    // Switch to absolute numbering and configure 24 cells for the absolute season.
    let mut mapping = state
        .db
        .get_series_mapping(&series_id)
        .await
        .unwrap()
        .expect("series mapping exists");
    mapping.settings.absolute_numbering = Some(true);
    mapping
        .settings
        .season_absolute
        .insert("1".to_string(), cell_count_override("1", 24));
    state
        .db
        .upsert_series_mapping(&series_id, &mapping)
        .await
        .unwrap();

    // 3 absolute episodes, well below the 24 configured cells.
    for ep in 1..=3 {
        let eid = format!("{}_ABS{:04}", series_id, ep);
        state
            .db
            .insert_episode(jumbie::db::episodes::InsertEpisodeParams {
                episode_id: &eid,
                series_id: &series_id,
                season: 1,
                episode: ep,
                file_path: Some("/media/abs.mkv"),
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
                numbering_mode: Some(1),
            })
            .await
            .unwrap();
    }

    assert_eq!(
        series_counts(&app, &series_id).await,
        (3, 24),
        "absolute mode must use the absolute season's cell count"
    );
}

#[tokio::test]
async fn test_series_details_total_uses_season_cell_count() {
    let (app, state, _tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Details Cell Show").await;
    set_season_override(&state, &series_id, cell_count_override("1", 12)).await;
    for ep in 1..=5 {
        seed_season_episode(&state, &series_id, ep, "organized", Some("/media/s1.mkv")).await;
    }

    let res = app
        .clone()
        .oneshot(common::get_request(&format!("/api/series/{}", series_id)))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        json["info"]["episodes_counts"],
        serde_json::json!([5, 12]),
        "details total must use the same SSoT helper as the library list"
    );
}

#[tokio::test]
async fn test_series_details_batch_total_uses_season_cell_count() {
    let (app, state, _tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Batch Details Show").await;
    set_season_override(&state, &series_id, cell_count_override("1", 12)).await;
    for ep in 1..=5 {
        seed_season_episode(&state, &series_id, ep, "organized", Some("/media/s1.mkv")).await;
    }

    let payload = serde_json::json!({ "ids": [series_id] });
    let res = app
        .clone()
        .oneshot(common::post_json_request(
            "/api/series/details/batch",
            &payload,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        json[&series_id]["info"]["episodes_counts"],
        serde_json::json!([5, 12]),
        "batch details total must use the same SSoT helper as the library list"
    );
}
