use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use jumbie::db::episodes::InsertEpisodeParams;
use jumbie_shared::types::RenameQueueResponse;
use tower::ServiceExt;

mod common;

use common::TestApp;

/// Basic smoke test: the rename queue endpoints return valid JSON structures.
#[tokio::test]
async fn test_get_rename_queue_endpoints() {
    let (app, _state, _temp_dir) = common::setup_test_app().await;

    // Create a series
    let series_id = common::create_test_series(&app, "Test Series Rename").await;

    // Test GET /api/system/rename_queue
    let res: RenameQueueResponse = app.get_json("/api/system/rename_queue").await;
    // No files yet, so the queue is likely empty.
    assert!(res.items.is_empty() || res.items.iter().any(|i| i.series_id == series_id));

    // Test GET /api/system/rename_queue/:series_id
    let req_detail = common::get_request(&format!("/api/system/rename_queue/{}", series_id));
    let res_detail = app.clone().oneshot(req_detail).await.unwrap();
    assert!(res_detail.status().is_success());
}

/// Permanently-failed media-scan files should appear in the rename queue.
#[tokio::test]
async fn test_rename_queue_includes_permanently_failed_files() {
    let (app, state, tmp) = common::setup_test_app().await;

    let series_id = common::create_test_series(&app, "Failed Scan Show").await;

    // Create a source file at a path different from the expected format output
    // so the rename plan is non-empty.
    let src_dir = tmp.path().join("source");
    tokio::fs::create_dir_all(&src_dir).await.unwrap();
    let file_path = src_dir.join("S01E01.mkv");
    tokio::fs::write(&file_path, b"not a real video")
        .await
        .unwrap();

    let episode_id = format!("{}_1_1", series_id);
    let path_str = file_path.to_string_lossy().to_string();
    state
        .db
        .insert_episode(InsertEpisodeParams {
            file_path: Some(&path_str),
            title: Some("Episode 1"),
            status: "organized",
            ..InsertEpisodeParams::dummy(&episode_id, &series_id, 1, 1, &HashMap::new())
        })
        .await
        .unwrap();

    // Scan the file and wait for the scan queue to finish before forcing
    // the permanently-failed media scan state. Without the wait, the
    // organizer's background callback may overwrite our SQL update below.
    state
        .db
        .scan_file_fingerprint(&file_path, "organized")
        .await;
    while !state.scan_queue.is_idle().await {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    sqlx::query(
        "UPDATE file_contents SET media_info = NULL, media_info_scan_failed = 1 WHERE fingerprint IN (SELECT fingerprint FROM file_paths WHERE file_path = ?)",
    )
    .bind(&path_str)
    .execute(state.db.get_pool())
    .await
    .unwrap();

    let failed = state
        .db
        .get_media_info_scan_failed_paths(std::slice::from_ref(&path_str))
        .await
        .unwrap();
    assert!(
        failed.contains(&path_str),
        "File should be marked as scan-failed"
    );

    // The file needs a rename: its current location (source/S01E01.mkv) differs
    // from where the naming format puts it (organized/Failed Scan Show/...).
    let res: RenameQueueResponse = app.get_json("/api/system/rename_queue").await;

    let series_item = res
        .items
        .iter()
        .find(|i| i.series_id == series_id)
        .expect("Series with permanently-failed file should appear in rename queue items");
    assert!(
        series_item.affected_episodes > 0,
        "Permanently-failed file should still be queued for rename"
    );
    assert!(
        !series_item.has_failed,
        "Permanently-failed media scan should NOT mark the rename as failed"
    );
}

/// Files currently being scanned should be excluded from the rename plan
/// because their media info may not be ready yet.
#[tokio::test]
async fn test_active_scan_excluded_from_rename_queue() {
    let (app, state, tmp) = common::setup_test_app().await;

    let series_id = common::create_test_series(&app, "Active Scan Show").await;

    // Create a source file at a path different from the expected format output.
    let src_dir = tmp.path().join("source");
    tokio::fs::create_dir_all(&src_dir).await.unwrap();
    let file_path = src_dir.join("S01E01.mkv");
    tokio::fs::write(&file_path, b"not a real video")
        .await
        .unwrap();

    let episode_id = format!("{}_1_1", series_id);
    let path_str = file_path.to_string_lossy().to_string();
    state
        .db
        .insert_episode(InsertEpisodeParams {
            file_path: Some(&path_str),
            title: Some("Episode 1"),
            status: "organized",
            ..InsertEpisodeParams::dummy(&episode_id, &series_id, 1, 1, &HashMap::new())
        })
        .await
        .unwrap();

    // Submit the file to the scan queue with a barrier so it stays "active"
    let barrier = Arc::new(tokio::sync::Barrier::new(2));
    let b = barrier.clone();
    state
        .scan_queue
        .submit(file_path.clone(), move || {
            let b = b.clone();
            async move {
                b.wait().await;
            }
        })
        .await;

    let snapshot = state.scan_queue.active_paths_snapshot().await;
    assert!(
        snapshot.contains(&file_path),
        "File should be in active scan paths"
    );

    // The file is mid-scan → rename queue should exclude this series
    let res: RenameQueueResponse = app.get_json("/api/system/rename_queue").await;

    assert!(
        !res.items.iter().any(|i| i.series_id == series_id),
        "Series with actively-scanned file should be excluded from rename queue"
    );

    // Release the barrier so the test cleanup proceeds
    barrier.wait().await;
    tokio::time::sleep(Duration::from_millis(100)).await;
}

/// Files pending organization by the download orchestrator (fingerprint state = 'complete',
/// episode status != 'organized') should be excluded from the rename queue.
///
/// This tests our fix: `get_rename_queue` adds paths from `get_completed_files()` to the
/// `skip_paths` set, preventing `compute_batch_plan` from seeing them. Without this fix, a
/// file with status='downloaded' and a fingerprint state='complete' would briefly appear
/// in the rename queue between `finalize_download` (which sets `file_path` to the download
/// dir) and `organize_completed` (which moves it to the library).
#[tokio::test]
async fn test_pending_organization_excluded_from_rename_queue() {
    let (app, state, tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Pending Org Show").await;

    // Create a file at a "download location" (simulating a completed download).
    let dl_dir = tmp.path().join("downloads");
    tokio::fs::create_dir_all(&dl_dir).await.unwrap();
    let file_path = dl_dir.join("S01E01.mkv");
    tokio::fs::write(&file_path, b"fake video content")
        .await
        .unwrap();
    let path_str = file_path.to_string_lossy().to_string();

    let episode_id = format!("{}_1_1", series_id);

    // Phase 1: status='downloaded' with file_path in the download directory (state
    // right after finalize_download).
    state
        .db
        .insert_episode(InsertEpisodeParams {
            file_path: Some(&path_str),
            title: Some("Episode 1"),
            status: "downloaded",
            ..InsertEpisodeParams::dummy(&episode_id, &series_id, 1, 1, &HashMap::new())
        })
        .await
        .unwrap();

    // Phase 2: fingerprint the file with state='complete' and link it to the episode
    // (what finalize_download + smart_link_downloaded_files do).
    let meta = tokio::fs::metadata(&file_path).await.unwrap();
    let (inode, dev, mtime) = jumbie::platform::file_identity(&meta);
    state
        .db
        .save_fingerprint(jumbie::db::files::SaveFingerprintParams {
            path: &path_str,
            inode,
            dev,
            size: meta.len(),
            mtime,
            quick_hash: "test_hash_pending",
            state: "complete",
            media_info: None,
        })
        .await
        .unwrap();
    state
        .db
        .link_file_episode(&path_str, &episode_id)
        .await
        .unwrap();

    // Give the episode non-null media_info, or proactively_scan_missing_media_info
    // would add the path to skip_paths and keep the file out of the rename queue.
    sqlx::query("UPDATE file_contents SET media_info = '{\"container\":\"matroska\"}' WHERE fingerprint IN (SELECT fingerprint FROM file_paths WHERE file_path = ?)")
        .bind(&path_str)
        .execute(state.db.get_pool())
        .await
        .unwrap();

    let completed = state.db.get_completed_files().await.unwrap();
    assert!(
        completed
            .iter()
            .any(|f| f.file_path == path_str && f.episode_status == "downloaded"),
        "get_completed_files should include our pending-organization file"
    );

    // Pending-organization files must be EXCLUDED from the rename queue.
    let res: RenameQueueResponse = app.get_json("/api/system/rename_queue").await;
    assert!(
        !res.items.iter().any(|i| i.series_id == series_id),
        "Series with pending-organization file should be excluded from rename queue"
    );

    // Phase 3: simulate organize_completed by flipping the fingerprint state to
    // 'organized'. media_info survives, so the proactive scan won't re-add it to
    // skip_paths; get_completed_files no longer returns it, so the exclusion ends.
    sqlx::query("UPDATE file_paths SET state = 'organized' WHERE file_path = ?")
        .bind(&path_str)
        .execute(state.db.get_pool())
        .await
        .unwrap();

    // The now-non-pending file must appear in the rename queue (src=download_path
    // != dst=library_path). Retry to absorb a race: between the UPDATE above and the
    // queue recompute, the scan worker can run update_file_fingerprint and (if the
    // scan of this synthetic file fails) clear media_info, pushing the path onto
    // skip_paths. save_fingerprint's COALESCE prevents permanent data loss; retrying
    // lets a racing scan settle so the next get_rename_queue sees the right state.
    for _attempt in 0..5 {
        let res: RenameQueueResponse = app.get_json("/api/system/rename_queue").await;
        if res.items.iter().any(|i| i.series_id == series_id) {
            return; // success
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!(
        "Series should appear in rename queue after fingerprint state is changed to 'organized' (file needs organizing)"
    );
}

/// Files pending organization should be excluded regardless of whether rename_episodes
/// is enabled or disabled — the exclusion happens before the rename decision.
#[tokio::test]
async fn test_pending_organization_excluded_with_rename_off() {
    let (app, state, tmp) = common::setup_test_app().await;

    // Set rename_episodes = false globally in the in-memory config
    {
        let mut cfg = state.cfg.write().await;
        cfg.organization.rename_episodes = false;
    }

    let series_id = common::create_test_series(&app, "Pending Org Rename Off").await;

    let dl_dir = tmp.path().join("downloads");
    tokio::fs::create_dir_all(&dl_dir).await.unwrap();
    let file_path = dl_dir.join("My.Release-Group.mkv");
    tokio::fs::write(&file_path, b"fake video content")
        .await
        .unwrap();
    let path_str = file_path.to_string_lossy().to_string();
    let episode_id = format!("{}_1_1", series_id);

    state
        .db
        .insert_episode(InsertEpisodeParams {
            file_path: Some(&path_str),
            title: Some("Episode 1"),
            status: "downloaded",
            ..InsertEpisodeParams::dummy(&episode_id, &series_id, 1, 1, &HashMap::new())
        })
        .await
        .unwrap();

    // Create fingerprint with state='complete'
    let meta = tokio::fs::metadata(&file_path).await.unwrap();
    let (inode, dev, mtime) = jumbie::platform::file_identity(&meta);
    state
        .db
        .save_fingerprint(jumbie::db::files::SaveFingerprintParams {
            path: &path_str,
            inode,
            dev,
            size: meta.len(),
            mtime,
            quick_hash: "test_hash_off",
            state: "complete",
            media_info: None,
        })
        .await
        .unwrap();
    state
        .db
        .link_file_episode(&path_str, &episode_id)
        .await
        .unwrap();

    // Minimal media_info so the path isn't added to skip_paths (same reason as test 1).
    sqlx::query("UPDATE file_contents SET media_info = '{\"container\":\"matroska\"}' WHERE fingerprint IN (SELECT fingerprint FROM file_paths WHERE file_path = ?)")
        .bind(&path_str)
        .execute(state.db.get_pool())
        .await
        .unwrap();

    let res: RenameQueueResponse = app.get_json("/api/system/rename_queue").await;
    assert!(
        !res.items.iter().any(|i| i.series_id == series_id),
        "Series with pending-organization file should be excluded even when rename_episodes=false"
    );

    // Flipping the fingerprint to 'organized' must surface the file (folder
    // organization is still needed even when rename is off).
    sqlx::query("UPDATE file_paths SET state = 'organized' WHERE file_path = ?")
        .bind(&path_str)
        .execute(state.db.get_pool())
        .await
        .unwrap();

    let res: RenameQueueResponse = app.get_json("/api/system/rename_queue").await;
    let item = res.items.iter().find(|i| i.series_id == series_id);
    assert!(
        item.is_some(),
        "Series should appear in rename queue after fingerprint removed (needs folder organize)"
    );
    if let Some(item) = item {
        assert!(
            item.causes
                .iter()
                .any(|c| c.contains("folder") || c.contains("Folder")),
            "With rename off, the cause should be a folder move, not a file rename. Causes: {:?}",
            item.causes
        );
    }
}

/// Regression test: when the episode is already in its final location (file_path matches
/// the computed destination), it should NOT appear in the rename queue even if a stale
/// 'complete' fingerprint exists at the OLD download path.
///
/// This scenario occurs after organize_completed has moved the file: the episode's
/// file_path is updated to the library path, but the old download fingerprint may still
/// linger with state='complete'. get_completed_files would return the old fingerprint
/// (because e.status='organized' AND e.file_path != f.file_path), but adding the OLD
/// download path to skip_paths must NOT cause the episode to be incorrectly skipped —
/// the organize step has already been done.
#[tokio::test]
async fn test_stale_complete_fingerprint_does_not_block_already_organized_episode() {
    let (app, state, tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Clean Stale Show").await;

    // Create the final (library) destination directory to simulate an already-organized file.
    let org_root = tmp
        .path()
        .join("organized")
        .join("Clean Stale Show")
        .join("S01");
    tokio::fs::create_dir_all(&org_root).await.unwrap();

    // The file at its final location (matches the template with rename on).
    let final_file = org_root.join("Clean Stale Show - S01E01 - Episode 1.mkv");
    tokio::fs::write(&final_file, b"organized content")
        .await
        .unwrap();
    let final_path_str = final_file.to_string_lossy().to_string();

    let episode_id = format!("{}_1_1", series_id);

    // Episode is 'organized' at the final path.
    state
        .db
        .insert_episode(InsertEpisodeParams {
            episode_id: &episode_id,
            series_id: &series_id,
            season: 1,
            episode: 1,
            file_path: Some(&final_path_str),
            title: Some("Episode 1"),
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

    // Create a STALE fingerprint at a different (old download) path with state='complete'.
    // This simulates the old download fingerprint that organize_completed didn't clean up.
    let stale_path = tmp.path().join("downloads_stale").join("S01E01.mkv");
    tokio::fs::create_dir_all(stale_path.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(&stale_path, b"stale download")
        .await
        .unwrap();
    let stale_path_str = stale_path.to_string_lossy().to_string();

    let meta = tokio::fs::metadata(&stale_path).await.unwrap();
    let (inode, dev, mtime) = jumbie::platform::file_identity(&meta);
    state
        .db
        .save_fingerprint(jumbie::db::files::SaveFingerprintParams {
            path: &stale_path_str,
            inode,
            dev,
            size: meta.len(),
            mtime,
            quick_hash: "stale_hash",
            state: "complete",
            media_info: None,
        })
        .await
        .unwrap();
    // Link the stale fingerprint to the same episode (simulating smart_link's work)
    state
        .db
        .link_file_episode(&stale_path_str, &episode_id)
        .await
        .unwrap();

    // Library-path fingerprint WITH media_info (post-organize state). Without it,
    // proactively_scan_missing_media_info would add the library path to skip_paths.
    let lib_meta = tokio::fs::metadata(&final_file).await.unwrap();
    let (lib_inode, lib_dev, lib_mtime) = jumbie::platform::file_identity(&lib_meta);
    state
        .db
        .save_fingerprint(jumbie::db::files::SaveFingerprintParams {
            path: &final_path_str,
            inode: lib_inode,
            dev: lib_dev,
            size: lib_meta.len(),
            mtime: lib_mtime,
            quick_hash: "lib_hash",
            state: "organized",
            media_info: None,
        })
        .await
        .unwrap();
    state
        .db
        .link_file_episode(&final_path_str, &episode_id)
        .await
        .unwrap();
    sqlx::query("UPDATE file_contents SET media_info = '{\"container\":\"matroska\"}' WHERE fingerprint IN (SELECT fingerprint FROM file_paths WHERE file_path = ?)")
        .bind(&final_path_str)
        .execute(state.db.get_pool())
        .await
        .unwrap();

    // get_completed_files returns the stale fingerprint, not the library one
    // (f.state='organized' != 'complete').
    let completed = state.db.get_completed_files().await.unwrap();
    assert!(
        completed.iter().any(|f| f.file_path == stale_path_str),
        "Stale download fingerprint should still be returned by get_completed_files"
    );

    let res: RenameQueueResponse = app.get_json("/api/system/rename_queue").await;
    assert!(
        !res.items.iter().any(|i| i.series_id == series_id),
        "Already-organized episode should NOT appear in rename queue despite stale fingerprint"
    );
}

/// Regression: a plan cached while an episode is unassigned must be recomputed once
/// the episode's inputs change, even though nothing publishes an invalidation event.
#[tokio::test]
async fn test_rename_queue_recomputes_when_episode_changes() {
    let (app, state, tmp) = common::setup_test_app().await;

    let series_id = common::create_test_series(&app, "Cache Bump Show").await;

    let src_dir = tmp.path().join("source");
    tokio::fs::create_dir_all(&src_dir).await.unwrap();
    let file_path = src_dir.join("S01E01.mkv");
    tokio::fs::write(&file_path, b"not a real video")
        .await
        .unwrap();

    let episode_id = format!("{}_1_1", series_id);
    let path_str = file_path.to_string_lossy().to_string();
    state
        .db
        .insert_episode(InsertEpisodeParams {
            file_path: Some(&path_str),
            title: Some("Episode 1"),
            status: "organized",
            ..InsertEpisodeParams::dummy(&episode_id, &series_id, 1, 1, &HashMap::new())
        })
        .await
        .unwrap();

    // Settle the media scan so the file is not held out of the plan by skip_paths.
    state
        .db
        .scan_file_fingerprint(&file_path, "organized")
        .await;
    while !state.scan_queue.is_idle().await {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    // Prime the cache and capture the current expected name.
    let req = common::get_request(&format!("/api/system/rename_queue/{}", series_id));
    let res = app.clone().oneshot(req).await.unwrap();
    let before: serde_json::Value = common::response_json(res).await;
    let before_expected = before["renames"]
        .as_array()
        .and_then(|a| a.first())
        .and_then(|r| r["expected"].as_str())
        .unwrap_or_default()
        .to_string();
    assert!(
        !before_expected.is_empty(),
        "plan should be non-empty before the change"
    );

    // Change an input directly (no invalidation call): renumber the episode.
    sqlx::query("UPDATE episodes SET episode = 2 WHERE episode_id = ?")
        .bind(&episode_id)
        .execute(state.db.get_pool())
        .await
        .unwrap();

    let req = common::get_request(&format!("/api/system/rename_queue/{}", series_id));
    let res = app.clone().oneshot(req).await.unwrap();
    let after: serde_json::Value = common::response_json(res).await;
    let after_expected = after["renames"]
        .as_array()
        .and_then(|a| a.first())
        .and_then(|r| r["expected"].as_str())
        .unwrap_or_default()
        .to_string();

    assert_ne!(
        before_expected, after_expected,
        "rename queue must reflect the changed episode without an invalidation event"
    );
}

async fn rename_detail(app: &axum::Router, series_id: &str) -> serde_json::Value {
    let req = common::get_request(&format!("/api/system/rename_queue/{}", series_id));
    let res = app.clone().oneshot(req).await.unwrap();
    common::response_json(res).await
}

fn first_expected(detail: &serde_json::Value) -> String {
    detail["renames"]
        .as_array()
        .and_then(|a| a.first())
        .and_then(|r| r["expected"].as_str())
        .unwrap_or_default()
        .to_string()
}

/// A config save must be reflected in the rename plan without any explicit cache
/// invalidation.
#[tokio::test]
async fn test_rename_queue_recomputes_when_config_changes() {
    let (app, state, tmp) = common::setup_test_app().await;

    let series_id = common::create_test_series(&app, "Config Bump Show").await;

    let src_dir = tmp.path().join("source");
    tokio::fs::create_dir_all(&src_dir).await.unwrap();
    let file_path = src_dir.join("S01E01.mkv");
    tokio::fs::write(&file_path, b"not a real video")
        .await
        .unwrap();

    let episode_id = format!("{}_1_1", series_id);
    let path_str = file_path.to_string_lossy().to_string();
    state
        .db
        .insert_episode(InsertEpisodeParams {
            file_path: Some(&path_str),
            title: Some("Episode 1"),
            status: "organized",
            ..InsertEpisodeParams::dummy(&episode_id, &series_id, 1, 1, &HashMap::new())
        })
        .await
        .unwrap();

    state
        .db
        .scan_file_fingerprint(&file_path, "organized")
        .await;
    while !state.scan_queue.is_idle().await {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let before_expected = first_expected(&rename_detail(&app, &series_id).await);
    assert!(
        !before_expected.is_empty(),
        "plan should be non-empty before the change"
    );

    // Change the global episode filename format; the plan fingerprints the whole
    // config, so this is picked up on the next poll without an invalidation call.
    let mut config = state.cfg.read().await.clone();
    config.organization.episode_file_format = "CONFIGCHANGED ${series} ${episode:02}".to_string();
    let payload = jumbie_shared::types::UpdateConfigPayload {
        organization: Some(config.organization.clone()),
        sources: None,
        general: None,
        proxy: None,
        auth: None,
        security: None,
    };
    let _: serde_json::Value = app.put_json("/api/config", &payload).await;

    let after_expected = first_expected(&rename_detail(&app, &series_id).await);
    assert!(
        after_expected.contains("CONFIGCHANGED"),
        "config change must be reflected in the plan, got {after_expected}"
    );
}
