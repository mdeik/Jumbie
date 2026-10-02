//! Series Directory Scanner integration tests: placing files triggers episode
//! discovery, `last_known_dir_mtime` is updated per scan, and unchanged
//! directories are not re-scanned into duplicates (plus path/config edge cases).

mod common;

use common::TestApp;
use jumbie_shared::types::SeriesDetails;
use std::path::PathBuf;

/// Create a minimal video file with a parseable episode name; return its path.
fn write_video_file(dir: &std::path::Path, filename: &str) -> PathBuf {
    let path = dir.join(filename);
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(&path, b"test video content").unwrap();
    path
}

#[tokio::test]
async fn test_scanner_discovers_new_episodes() {
    let (app, state, temp_dir) = common::setup_test_app().await;
    let org_root = temp_dir.path().join("organized");
    let series_dir = org_root.join("Scanner Test Show");
    std::fs::create_dir_all(&series_dir).unwrap();

    let series_id = common::create_test_series(&app, "Scanner Test Show").await;

    let details: SeriesDetails = app.get_json(&format!("/api/series/{}", series_id)).await;
    assert_eq!(
        details.episodes.len(),
        0,
        "Should have no episodes initially"
    );

    write_video_file(&series_dir, "Scanner Test Show - S01E01.mkv");
    write_video_file(&series_dir, "Scanner Test Show - S01E02.mkv");

    let path = series_dir.clone();
    let result = jumbie::scanner::scan_directory(&path, &state, false).await;
    assert!(result.is_ok(), "scan_directory should succeed");
    let found = result.unwrap();
    assert!(
        found.contains(&"Scanner Test Show".to_string()),
        "Should have found the series: {:?}",
        found
    );

    let details: SeriesDetails = app.get_json(&format!("/api/series/{}", series_id)).await;
    assert_eq!(
        details.episodes.len(),
        2,
        "Should have discovered 2 episodes, got {}",
        details.episodes.len()
    );

    let mapping = state.db.get_series_mapping(&series_id).await.unwrap();
    assert!(
        mapping.is_some(),
        "Series mapping should still exist after scan"
    );
    let mtime = mapping.unwrap().settings.last_known_dir_mtimes;
    assert!(
        !mtime.is_empty(),
        "last_known_dir_mtimes should have been set after scan"
    );
    let mtime_val = *mtime.values().next().unwrap();
    assert!(
        mtime_val > 0.0,
        "last_known_dir_mtimes should be positive, got {}",
        mtime_val
    );
}

#[tokio::test]
async fn test_scanner_discovers_auxiliary_files() {
    let (app, state, temp_dir) = common::setup_test_app().await;
    let org_root = temp_dir.path().join("organized");
    let series_dir = org_root.join("Aux Test Show");
    std::fs::create_dir_all(&series_dir).unwrap();

    let series_id = common::create_test_series(&app, "Aux Test Show").await;

    let write = |name: &str| std::fs::write(series_dir.join(name), b"x").unwrap();
    write("Aux Test Show - S01E01.mkv");
    write("Aux Test Show - S01E01.en.srt");
    write("Aux Test Show - S01E01.nfo");

    jumbie::scanner::scan_directory(&series_dir, &state, false)
        .await
        .unwrap();

    let details: SeriesDetails = app.get_json(&format!("/api/series/{}", series_id)).await;
    let ep = details
        .episodes
        .iter()
        .find(|e| e.episode == 1)
        .expect("episode 1 discovered");

    let kinds: Vec<_> = ep.auxiliary_files.iter().map(|a| a.kind).collect();
    assert!(
        kinds.contains(&jumbie_shared::media_format::FileKind::Subtitle),
        "subtitle should be discovered: {:?}",
        ep.auxiliary_files
    );
    assert!(
        kinds.contains(&jumbie_shared::media_format::FileKind::Nfo),
        "nfo should be discovered: {:?}",
        ep.auxiliary_files
    );
    assert!(
        ep.path.as_deref().is_some_and(|p| p.ends_with(".mkv")),
        "the canonical path stays the video: {:?}",
        ep.path
    );
}

// The per-series scanner must re-hash a file whose identity changed since the last
// scan (the path→fingerprint row is only valid while inode/device/size/mtime match),
// so the episode's joined metadata (size/release) reflects the new content.
#[tokio::test]
async fn test_series_scan_rehashes_a_changed_file() {
    let (app, state, temp_dir) = common::setup_test_app().await;
    let org_root = temp_dir.path().join("organized");
    let series_dir = org_root.join("Rescan Show");
    std::fs::create_dir_all(&series_dir).unwrap();

    let series_id = common::create_test_series(&app, "Rescan Show").await;

    let file = series_dir.join("Rescan Show - S01E01.mkv");
    std::fs::write(&file, vec![b'a'; 100]).unwrap();

    let mapping = state
        .db
        .get_series_mapping(&series_id)
        .await
        .unwrap()
        .expect("mapping");
    jumbie::scanner::scan_series_directory(&series_dir, &mapping, &state)
        .await
        .unwrap();

    let details: SeriesDetails = app.get_json(&format!("/api/series/{}", series_id)).await;
    let size = details
        .episodes
        .iter()
        .find(|e| e.episode == 1)
        .expect("episode 1")
        .size;
    assert_eq!(size, 100, "initial size must reflect the scanned file");

    // Let the background fingerprint/media-info tasks from scan #1 drain; while the
    // path is still queued the scan queue dedups and a later hash-first is skipped.
    for _ in 0..100 {
        if !state.scan_queue.contains(&file).await {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    // Replace the file in place with a different (larger) payload.
    std::fs::write(&file, vec![b'b'; 500]).unwrap();

    let mapping = state
        .db
        .get_series_mapping(&series_id)
        .await
        .unwrap()
        .expect("mapping");
    jumbie::scanner::scan_series_directory(&series_dir, &mapping, &state)
        .await
        .unwrap();

    let details: SeriesDetails = app.get_json(&format!("/api/series/{}", series_id)).await;
    let size = details
        .episodes
        .iter()
        .find(|e| e.episode == 1)
        .expect("episode 1")
        .size;
    assert_eq!(
        size, 500,
        "a changed file must be re-hashed on the next series scan"
    );
}

// Recreates the reported bug: a video is organized into the series directory
// (via Jumbie), and its sidecar (nfo/srt) is added LATER (e.g. by Jellyfin).
// The per-series scanner (`scan_series_directory` -> `import_scan_for_series`)
// is what runs on a known series directory (create/refresh/update/periodic),
// so it must pick up and link the newly added sidecar.
#[tokio::test]
async fn test_series_scan_links_later_added_auxiliary() {
    let (app, state, temp_dir) = common::setup_test_app().await;
    let org_root = temp_dir.path().join("organized");
    let series_dir = org_root.join("Late Sidecar Show");
    std::fs::create_dir_all(&series_dir).unwrap();

    let series_id = common::create_test_series(&app, "Late Sidecar Show").await;

    // Phase 1: the video arrives via Jumbie and is organized.
    std::fs::write(series_dir.join("Late Sidecar Show - S01E01.mkv"), b"x").unwrap();
    let mapping = state
        .db
        .get_series_mapping(&series_id)
        .await
        .unwrap()
        .expect("mapping");
    jumbie::scanner::scan_series_directory(&series_dir, &mapping, &state)
        .await
        .unwrap();

    // Phase 2: Jellyfin drops in the nfo + subtitle after the fact.
    std::fs::write(series_dir.join("Late Sidecar Show - S01E01.nfo"), b"x").unwrap();
    std::fs::write(series_dir.join("Late Sidecar Show - S01E01.en.srt"), b"x").unwrap();

    let mapping = state
        .db
        .get_series_mapping(&series_id)
        .await
        .unwrap()
        .expect("mapping");
    jumbie::scanner::scan_series_directory(&series_dir, &mapping, &state)
        .await
        .unwrap();

    let details: SeriesDetails = app.get_json(&format!("/api/series/{}", series_id)).await;
    let ep = details
        .episodes
        .iter()
        .find(|e| e.episode == 1)
        .expect("episode 1 discovered");
    eprintln!("AUX AFTER LATE ADD: {:?}", ep.auxiliary_files);
    let kinds: Vec<_> = ep.auxiliary_files.iter().map(|a| a.kind).collect();
    assert!(
        kinds.contains(&jumbie_shared::media_format::FileKind::Nfo),
        "later-added nfo should have been linked: {:?}",
        ep.auxiliary_files
    );
    assert!(
        kinds.contains(&jumbie_shared::media_format::FileKind::Subtitle),
        "later-added subtitle should have been linked: {:?}",
        ep.auxiliary_files
    );
}

#[tokio::test]
async fn test_scanner_idempotent_no_duplicates() {
    let (app, state, temp_dir) = common::setup_test_app().await;
    let org_root = temp_dir.path().join("organized");
    let series_dir = org_root.join("Idempotent Show");
    std::fs::create_dir_all(&series_dir).unwrap();

    let series_id = common::create_test_series(&app, "Idempotent Show").await;

    write_video_file(&series_dir, "Idempotent Show - S01E01.mkv");
    write_video_file(&series_dir, "Idempotent Show - S01E02.mkv");

    let path = series_dir.clone();
    jumbie::scanner::scan_directory(&path, &state, false)
        .await
        .unwrap();

    let details: SeriesDetails = app.get_json(&format!("/api/series/{}", series_id)).await;
    let first_count = details.episodes.len();
    assert_eq!(first_count, 2, "First scan should create 2 episodes");

    let result = jumbie::scanner::scan_directory(&path, &state, false).await;
    assert!(result.is_ok(), "Second scan should succeed");

    let details: SeriesDetails = app.get_json(&format!("/api/series/{}", series_id)).await;
    let second_count = details.episodes.len();
    assert_eq!(
        second_count, first_count,
        "Second scan without changes should NOT create duplicates: {} vs {}",
        second_count, first_count
    );
}

#[tokio::test]
async fn test_scanner_discovers_only_new_files() {
    let (app, state, temp_dir) = common::setup_test_app().await;
    let org_root = temp_dir.path().join("organized");
    let series_dir = org_root.join("Incremental Show");
    std::fs::create_dir_all(&series_dir).unwrap();

    let series_id = common::create_test_series(&app, "Incremental Show").await;

    write_video_file(&series_dir, "Incremental Show - S01E01.mkv");
    let path = series_dir.clone();
    jumbie::scanner::scan_directory(&path, &state, false)
        .await
        .unwrap();

    let details: SeriesDetails = app.get_json(&format!("/api/series/{}", series_id)).await;
    assert_eq!(
        details.episodes.len(),
        1,
        "Should have 1 episode after first scan"
    );

    write_video_file(&series_dir, "Incremental Show - S01E02.mkv");
    jumbie::scanner::scan_directory(&path, &state, false)
        .await
        .unwrap();

    let details: SeriesDetails = app.get_json(&format!("/api/series/{}", series_id)).await;
    assert_eq!(
        details.episodes.len(),
        2,
        "Should now have 2 episodes, not {}",
        details.episodes.len()
    );
}

#[tokio::test]
async fn test_scanner_skips_series_without_path() {
    let (app, state, _temp_dir) = common::setup_test_app().await;

    let series_id = common::create_test_series(&app, "No Path Show").await;

    if let Some(mut mapping) = state.db.get_series_mapping(&series_id).await.unwrap() {
        mapping.settings.path = None;
        state
            .db
            .upsert_series_mapping(&series_id, &mapping)
            .await
            .unwrap();
    }

    // Running scan_directory on a non-existent path should not panic
    let bad_path = std::path::PathBuf::from("/tmp/nonexistent_path_for_test_12345");
    let result = jumbie::scanner::scan_directory(&bad_path, &state, false).await;
    assert!(
        result.is_ok(),
        "Scanning a non-existent path should not fail"
    );
    let found = result.unwrap();
    assert!(
        found.is_empty(),
        "Should not find any series in a non-existent directory"
    );
}

#[tokio::test]
async fn test_create_series_with_scan_discovers_files() {
    let (app, state, temp_dir) = common::setup_test_app().await;
    let org_root = temp_dir.path().join("organized");
    let series_dir = org_root.join("Pre Existing Show");
    std::fs::create_dir_all(&series_dir).unwrap();

    // The folder already exists on disk — "overwrite" collision handling keeps
    // the claim-existing-content semantics this test exercises.
    common::set_collision_handling(&state, "overwrite").await;

    // Place files BEFORE creating the series.
    write_video_file(&series_dir, "Pre Existing Show - S01E01.mkv");
    write_video_file(&series_dir, "Pre Existing Show - S01E02.mkv");

    let series_id = common::create_test_series(&app, "Pre Existing Show").await;

    let path = series_dir.clone();
    jumbie::scanner::scan_directory(&path, &state, false)
        .await
        .unwrap();

    let details: SeriesDetails = app.get_json(&format!("/api/series/{}", series_id)).await;
    assert_eq!(
        details.episodes.len(),
        2,
        "Should discover pre-existing files"
    );
    assert!(details.info.path.contains("Pre Existing Show"));

    let mapping = state.db.get_series_mapping(&series_id).await.unwrap();
    assert!(
        !mapping.unwrap().settings.last_known_dir_mtimes.is_empty(),
        "Series created with scan should have last_known_dir_mtimes set"
    );
}

// trust_existing=true skips files with existing fingerprints but still
// discovers truly new files (files without a fingerprint entry).

#[tokio::test]
async fn test_scan_trust_existing_skips_known_files() {
    let (app, state, temp_dir) = common::setup_test_app().await;
    let org_root = temp_dir.path().join("organized");
    let series_dir = org_root.join("Trust Existing Show");
    std::fs::create_dir_all(&series_dir).unwrap();

    // The folder already exists on disk — claim it rather than suffixing.
    common::set_collision_handling(&state, "overwrite").await;

    let series_id = common::create_test_series(&app, "Trust Existing Show").await;

    // Normal scan (trust_existing=false) so file 1 gets a fingerprint.
    write_video_file(&series_dir, "Trust Existing Show - S01E01.mkv");
    let path = series_dir.clone();
    jumbie::scanner::scan_directory(&path, &state, false)
        .await
        .unwrap();

    // Wait for the scan queue to finish fingerprinting
    while !state.scan_queue.is_idle().await {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    let details: SeriesDetails = app.get_json(&format!("/api/series/{}", series_id)).await;
    assert_eq!(
        details.episodes.len(),
        1,
        "Should have 1 episode after initial scan"
    );

    let fp_path = series_dir.join("Trust Existing Show - S01E01.mkv");
    let fp = state
        .db
        .get_fingerprint_meta(fp_path.to_str().unwrap())
        .await
        .unwrap();
    assert!(
        fp.is_some(),
        "Fingerprint should exist for file after initial scan"
    );

    // trust_existing=true skips the known file, but the series must still be
    // "found" so mtime tracking continues.
    let result = jumbie::scanner::scan_directory(&path, &state, true)
        .await
        .unwrap();
    assert!(
        result.contains(&"Trust Existing Show".to_string()),
        "Series should still be found for mtime tracking"
    );

    let details: SeriesDetails = app.get_json(&format!("/api/series/{}", series_id)).await;
    assert_eq!(
        details.episodes.len(),
        1,
        "trust_existing=true should NOT create duplicate episodes"
    );

    let mapping = state
        .db
        .get_series_mapping(&series_id)
        .await
        .unwrap()
        .unwrap();
    assert!(
        !mapping.settings.last_known_dir_mtimes.is_empty(),
        "last_known_dir_mtimes should be updated even with trust_existing=true"
    );
}

// trust_existing=true still discovers NEW files (no existing fingerprint).

#[tokio::test]
async fn test_trust_existing_still_discovers_new_files() {
    let (app, state, temp_dir) = common::setup_test_app().await;
    let org_root = temp_dir.path().join("organized");
    let series_dir = org_root.join("Trust New Show");
    std::fs::create_dir_all(&series_dir).unwrap();

    let series_id = common::create_test_series(&app, "Trust New Show").await;

    write_video_file(&series_dir, "Trust New Show - S01E01.mkv");
    let path = series_dir.clone();
    jumbie::scanner::scan_directory(&path, &state, false)
        .await
        .unwrap();

    // Wait for the scan queue to finish fingerprinting
    while !state.scan_queue.is_idle().await {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    write_video_file(&series_dir, "Trust New Show - S01E02.mkv");

    // trust_existing=true: file 1 is skipped (fingerprinted), file 2 is discovered.
    jumbie::scanner::scan_directory(&path, &state, true)
        .await
        .unwrap();

    while !state.scan_queue.is_idle().await {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    let details: SeriesDetails = app.get_json(&format!("/api/series/{}", series_id)).await;
    assert_eq!(
        details.episodes.len(),
        2,
        "trust_existing=true should still discover new files without fingerprints"
    );

    let fp2_path = series_dir.join("Trust New Show - S01E02.mkv");
    let fp2 = state
        .db
        .get_fingerprint_meta(fp2_path.to_str().unwrap())
        .await
        .unwrap();
    assert!(
        fp2.is_some(),
        "New file should have been fingerprinted even with trust_existing=true"
    );
}
