//! Missing-files detection: `SeriesInfo.has_not_found_files` tracks
//! series-directory existence and `EpisodeViewModel.disk_present` tracks per-file
//! presence, both returning to the correct state when a directory is restored.

mod common;

use common::TestApp;
use jumbie_shared::types::{SeriesDetails, SeriesInfo};
use std::path::PathBuf;

/// Helper: create a minimal video file with a parseable episode name.
fn write_video_file(dir: &std::path::Path, filename: &str) -> PathBuf {
    let path = dir.join(filename);
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(&path, b"test video content").unwrap();
    path
}

#[tokio::test]
async fn test_has_not_found_files_reflects_directory_existence() {
    let (app, state, temp_dir) = common::setup_test_app().await;
    let org_root = temp_dir.path().join("organized");
    let series_dir = org_root.join("Not Found Show");
    std::fs::create_dir_all(&series_dir).unwrap();

    // The folder already exists on disk — claim it rather than suffixing.
    common::set_collision_handling(&state, "overwrite").await;

    let series_id = common::create_test_series(&app, "Not Found Show").await;

    write_video_file(&series_dir, "Not Found Show - S01E01.mkv");
    write_video_file(&series_dir, "Not Found Show - S01E02.mkv");

    let path = series_dir.clone();
    jumbie::scanner::scan_directory(&path, &state, false)
        .await
        .unwrap();

    let details: SeriesDetails = app.get_json(&format!("/api/series/{}", series_id)).await;
    assert_eq!(
        details.episodes.len(),
        2,
        "Should have 2 episodes after scan"
    );

    let series_list: Vec<SeriesInfo> = app.get_json("/api/series").await;
    let found = series_list.iter().find(|s| s.id == series_id).unwrap();
    assert!(
        !found.has_not_found_files,
        "has_not_found_files should be false when directory exists"
    );

    for ep in &details.episodes {
        if ep.path.is_some() {
            assert!(
                ep.disk_present,
                "Episode {} should have disk_present=true when file on disk",
                ep.header
            );
        }
    }

    // Simulate a manual move by deleting the directory.
    std::fs::remove_dir_all(&series_dir).unwrap();
    assert!(
        !series_dir.exists(),
        "Series directory should be gone after removal"
    );

    let series_list: Vec<SeriesInfo> = app.get_json("/api/series").await;
    let found = series_list.iter().find(|s| s.id == series_id).unwrap();
    assert!(
        found.has_not_found_files,
        "has_not_found_files should be true when directory no longer exists"
    );

    let details: SeriesDetails = app.get_json(&format!("/api/series/{}", series_id)).await;
    for ep in &details.episodes {
        if ep.path.is_some() {
            assert!(
                !ep.disk_present,
                "Episode {} should have disk_present=false after directory removed",
                ep.header
            );
        }
    }

    write_video_file(&series_dir, "Not Found Show - S01E01.mkv");
    write_video_file(&series_dir, "Not Found Show - S01E02.mkv");

    let path = series_dir.clone();
    jumbie::scanner::scan_directory(&path, &state, false)
        .await
        .unwrap();

    let series_list: Vec<SeriesInfo> = app.get_json("/api/series").await;
    let found = series_list.iter().find(|s| s.id == series_id).unwrap();
    assert!(
        !found.has_not_found_files,
        "has_not_found_files should be false after directory is restored and re-scanned"
    );
}

#[tokio::test]
async fn test_clear_not_found_files_only_clears_not_found_episodes() {
    let (app, state, temp_dir) = common::setup_test_app().await;
    let org_root = temp_dir.path().join("organized");
    let series_dir = org_root.join("Clear Test Show");
    std::fs::create_dir_all(&series_dir).unwrap();

    // The folder already exists on disk — claim it rather than suffixing.
    common::set_collision_handling(&state, "overwrite").await;

    let series_id = common::create_test_series(&app, "Clear Test Show").await;

    write_video_file(&series_dir, "Clear Test Show - S01E01.mkv");
    write_video_file(&series_dir, "Clear Test Show - S01E02.mkv");
    write_video_file(&series_dir, "Clear Test Show - S01E03.mkv");

    let path = series_dir.clone();
    jumbie::scanner::scan_directory(&path, &state, false)
        .await
        .unwrap();

    let details: SeriesDetails = app.get_json(&format!("/api/series/{}", series_id)).await;
    let with_path = details.episodes.iter().filter(|e| e.path.is_some()).count();
    assert_eq!(
        with_path, 3,
        "All 3 episodes should have file_path after scan"
    );

    std::fs::remove_file(series_dir.join("Clear Test Show - S01E01.mkv")).unwrap();
    std::fs::remove_file(series_dir.join("Clear Test Show - S01E03.mkv")).unwrap();

    use common::post_json_raw;
    let (status, body): (axum::http::StatusCode, serde_json::Value) = post_json_raw(
        &app,
        &format!("/api/series/{}/clear-not-found-files", series_id),
        &serde_json::json!({}),
    )
    .await;
    assert!(status.is_success(), "clear-not-found-files should succeed");
    let cleared = body.get("cleared").and_then(|c| c.as_u64()).unwrap_or(0);
    assert_eq!(
        cleared, 2,
        "Should have cleared 2 missing episode references"
    );

    let details: SeriesDetails = app.get_json(&format!("/api/series/{}", series_id)).await;
    let with_path_after = details.episodes.iter().filter(|e| e.path.is_some()).count();
    assert_eq!(
        with_path_after, 1,
        "Only 1 episode should still have file_path after clear"
    );
    let survivor = details.episodes.iter().find(|e| e.path.is_some()).unwrap();
    assert_eq!(survivor.episode, 2, "S01E02 should be the survivor");
    assert!(
        survivor.disk_present,
        "Survivor file should report disk_present=true"
    );

    let (status, body): (axum::http::StatusCode, serde_json::Value) = post_json_raw(
        &app,
        &format!("/api/series/{}/clear-not-found-files", series_id),
        &serde_json::json!({}),
    )
    .await;
    assert!(status.is_success());
    let cleared = body.get("cleared").and_then(|c| c.as_u64()).unwrap_or(0);
    assert_eq!(cleared, 0, "Second call should clear 0");
}
