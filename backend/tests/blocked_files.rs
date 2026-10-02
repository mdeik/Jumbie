// Blocked files (durable "do not auto-adopt") tests.
//
// A file the user unassigned as wrong must not be re-adopted by a filesystem
// rescan; renaming it (or replacing its content) unblocks it; mismatching blocks
// for the same name are all consulted; explicit assignments clear blocks and a
// manual unassign creates one; suppressed/blocked files still count toward disk
// usage.

mod common;

use axum::http::StatusCode;
use tower::ServiceExt;

async fn adopted_count(state: &jumbie::api::AppState, series_id: &str) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM episodes e WHERE e.series_id = ? \
         AND EXISTS (SELECT 1 FROM episode_files ef \
                     WHERE ef.episode_id = e.episode_id AND ef.kind = 'main')",
    )
    .bind(series_id)
    .fetch_one(state.db.get_pool())
    .await
    .unwrap()
}

async fn scan(
    state: &std::sync::Arc<jumbie::api::AppState>,
    series_id: &str,
    dir: &std::path::Path,
) -> usize {
    let mapping = state
        .db
        .get_series_mapping(series_id)
        .await
        .unwrap()
        .expect("mapping");
    jumbie::scanner::scan_series_directory(&dir.to_path_buf(), &mapping, state)
        .await
        .expect("scan should succeed")
}

async fn assign(app: &axum::Router, series_id: &str, path: &str, season: &str, episode: &str) {
    let res = app
        .clone()
        .oneshot(common::post_json_request(
            &format!("/api/series/{series_id}/files/assign"),
            &serde_json::json!({ "path": path, "season": season, "episode": episode }),
        ))
        .await
        .unwrap();
    assert!(res.status().is_success(), "assign failed: {}", res.status());
}

#[tokio::test]
async fn rescan_does_not_readopt_a_blocked_file() {
    let (app, state, _tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Blocked File Test").await;

    let dir = tempfile::TempDir::new().unwrap();
    let f1 = dir.path().join("Show.S01E01.mkv");
    let f2 = dir.path().join("Show.S01E02.mkv");
    std::fs::write(&f1, b"one").unwrap();
    std::fs::write(&f2, b"two").unwrap();

    // The user unassigned S01E02's file as wrong → a durable block.
    let size = std::fs::metadata(&f2).unwrap().len() as i64;
    state
        .db
        .block_file(&series_id, "Show.S01E02.mkv", size, None)
        .await
        .unwrap();

    let inserted = scan(&state, &series_id, dir.path()).await;

    assert_eq!(inserted, 1, "only the unblocked file should be adopted");
    assert_eq!(adopted_count(&state, &series_id).await, 1);
    let files: Vec<String> = sqlx::query_scalar(
        "SELECT fp.file_path FROM episodes e \
         JOIN episode_files ef ON ef.episode_id = e.episode_id AND ef.kind = 'main' \
         JOIN file_paths fp ON fp.id = ef.file_path_id \
         WHERE e.series_id = ?",
    )
    .bind(&series_id)
    .fetch_all(state.db.get_pool())
    .await
    .unwrap();
    assert_eq!(
        files,
        vec![f1.to_string_lossy().to_string()],
        "the blocked file must not have been re-adopted"
    );
}

#[tokio::test]
async fn renaming_a_blocked_file_unblocks_it() {
    let (app, state, _tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Rename Unblocks Test").await;

    let dir = tempfile::TempDir::new().unwrap();
    let original = dir.path().join("Show.S01E02.mkv");
    std::fs::write(&original, b"two").unwrap();
    let size = std::fs::metadata(&original).unwrap().len() as i64;
    state
        .db
        .block_file(&series_id, "Show.S01E02.mkv", size, None)
        .await
        .unwrap();

    // Rename to claim a different episode — a deliberate act that unblocks it.
    std::fs::rename(&original, dir.path().join("Show.S01E03.mkv")).unwrap();

    let inserted = scan(&state, &series_id, dir.path()).await;
    assert_eq!(inserted, 1, "the renamed file is a new name → adopted");
    assert_eq!(adopted_count(&state, &series_id).await, 1);
}

#[tokio::test]
async fn same_name_different_content_is_adopted() {
    let (app, state, _tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "New Content Test").await;

    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("Show.S01E02.mkv");
    std::fs::write(&path, b"http").unwrap();

    // Block with a size that no longer matches the (re-downloaded) file.
    state
        .db
        .block_file(&series_id, "Show.S01E02.mkv", 999, None)
        .await
        .unwrap();

    let inserted = scan(&state, &series_id, dir.path()).await;
    assert_eq!(inserted, 1, "a different size means a new file → adopted");
}

#[tokio::test]
async fn multiple_blocks_for_the_same_name_are_all_consulted() {
    let (app, state, _tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Multi Block Test").await;

    let dir = tempfile::TempDir::new().unwrap();
    let f = dir.path().join("Show.S01E01.mkv");
    std::fs::write(&f, b"abcdef").unwrap();

    // Two blocks share the name; only the second matches this file's size.
    state
        .db
        .block_file(&series_id, "Show.S01E01.mkv", 111, None)
        .await
        .unwrap();
    state
        .db
        .block_file(&series_id, "Show.S01E01.mkv", 6, None)
        .await
        .unwrap();

    let inserted = scan(&state, &series_id, dir.path()).await;
    assert_eq!(
        inserted, 0,
        "the matching block must be found among same-name blocks"
    );
    assert_eq!(adopted_count(&state, &series_id).await, 0);
}

#[tokio::test]
async fn suppressed_and_blocked_files_still_count_toward_disk_usage() {
    let (app, state, _tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Disk Usage Test").await;

    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("Show.S01E01.mkv"), vec![0u8; 5]).unwrap();
    std::fs::write(dir.path().join("Show.S02E01.mkv"), vec![0u8; 10]).unwrap();
    std::fs::write(dir.path().join("Show.S03E01.mkv"), vec![0u8; 20]).unwrap();

    state.db.suppress_season(&series_id, 2, 0).await.unwrap();
    state
        .db
        .block_file(&series_id, "Show.S03E01.mkv", 20, None)
        .await
        .unwrap();

    let inserted = scan(&state, &series_id, dir.path()).await;
    assert_eq!(inserted, 1, "only the season-1 file is adopted");
    assert_eq!(adopted_count(&state, &series_id).await, 1);

    let total: i64 = sqlx::query_scalar("SELECT total_size FROM series_mappings WHERE id = ?")
        .bind(&series_id)
        .fetch_one(state.db.get_pool())
        .await
        .unwrap();
    assert_eq!(
        total, 35,
        "suppressed and blocked files still occupy disk and must be counted"
    );
}

#[tokio::test]
async fn explicit_assignment_clears_the_block() {
    let (app, state, _tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Assign Clears Test").await;

    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("Show.S01E01.mkv");
    std::fs::write(&path, b"content").unwrap();
    let path_str = path.to_string_lossy().to_string();

    // A block exists (e.g. the file was previously unassigned).
    state
        .db
        .block_file(&series_id, "Show.S01E01.mkv", 7, None)
        .await
        .unwrap();

    assign(&app, &series_id, &path_str, "1", "1").await;

    assert!(
        state
            .db
            .get_blocked_files(&series_id)
            .await
            .unwrap()
            .is_empty(),
        "explicit assignment must clear the matching block"
    );
}

#[tokio::test]
async fn unassign_creates_a_block() {
    let (app, state, _tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Unassign Blocks Test").await;

    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("Show.S01E01.mkv");
    std::fs::write(&path, b"content").unwrap();
    let path_str = path.to_string_lossy().to_string();

    assign(&app, &series_id, &path_str, "1", "1").await;

    // Assignment may have organized (moved) the file; unassign the path the DB
    // now holds so the block is created for the real on-disk file.
    let current_path: String = sqlx::query_scalar(
        "SELECT fp.file_path FROM episodes e \
         JOIN episode_files ef ON ef.episode_id = e.episode_id AND ef.kind = 'main' \
         JOIN file_paths fp ON fp.id = ef.file_path_id \
         WHERE e.series_id = ? LIMIT 1",
    )
    .bind(&series_id)
    .fetch_one(state.db.get_pool())
    .await
    .unwrap();
    assert!(
        std::path::Path::new(&current_path).exists(),
        "assigned file should exist at {current_path}"
    );

    let res = app
        .clone()
        .oneshot(common::post_json_request(
            &format!("/api/series/{series_id}/files/unassign"),
            &serde_json::json!({ "paths": [current_path.clone()] }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let blocks = state.db.get_blocked_files(&series_id).await.unwrap();
    assert_eq!(
        blocks.len(),
        1,
        "unassigning a file must record a durable block"
    );
    let expected_name = jumbie_shared::formatting::normalize_file_name(
        std::path::Path::new(&current_path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap(),
    );
    assert_eq!(blocks[0].file_name, expected_name);
}

// Multi-file / multi-part shapes

#[tokio::test]
async fn blocked_multi_episode_file_is_not_adopted() {
    let (app, state, _tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Range Show").await;

    let dir = tempfile::TempDir::new().unwrap();
    // One file covering two episodes, plus a normal single-episode file (control).
    let range = dir.path().join("Range Show - S01E01E02.mkv");
    let single = dir.path().join("Range Show - S01E03.mkv");
    std::fs::write(&range, b"range-file-content").unwrap();
    std::fs::write(&single, b"single").unwrap();

    let range_size = std::fs::metadata(&range).unwrap().len() as i64;
    state
        .db
        .block_file(&series_id, "Range Show - S01E01E02.mkv", range_size, None)
        .await
        .unwrap();

    scan(&state, &series_id, dir.path()).await;

    // E01/E02 come only from the blocked range file → absent. E03 proves the
    // scan itself worked (non-vacuous control).
    let rows: Vec<(i32, Option<String>)> = sqlx::query_as(
        "SELECT e.episode, fp.file_path FROM episodes e \
         LEFT JOIN episode_files ef ON ef.episode_id = e.episode_id AND ef.kind = 'main' \
         LEFT JOIN file_paths fp ON fp.id = ef.file_path_id \
         WHERE e.series_id = ? ORDER BY e.episode",
    )
    .bind(&series_id)
    .fetch_all(state.db.get_pool())
    .await
    .unwrap();
    assert_eq!(
        rows,
        vec![(3, Some(single.to_string_lossy().to_string()))],
        "a blocked multi-episode file must not be adopted"
    );
}

#[tokio::test]
async fn blocked_part_file_is_not_registered() {
    let (app, state, temp_dir) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Multipart Show").await;
    let series_dir = temp_dir.path().join("organized").join("Multipart Show");
    std::fs::create_dir_all(&series_dir).unwrap();

    // A two-part episode lives under a series directory; scan_directory resolves
    // the series from the filename key (like the background scanner does).
    let p1 = series_dir.join("Multipart Show - S01E01 - pt1.mkv");
    let p2 = series_dir.join("Multipart Show - S01E01 - pt2.mkv");
    std::fs::write(&p1, b"part-one").unwrap();
    std::fs::write(&p2, b"part-two").unwrap();

    let p2_size = std::fs::metadata(&p2).unwrap().len() as i64;
    state
        .db
        .block_file(
            &series_id,
            "Multipart Show - S01E01 - pt2.mkv",
            p2_size,
            None,
        )
        .await
        .unwrap();

    jumbie::scanner::scan_directory(&series_dir, &state, false)
        .await
        .expect("scan_directory");

    // Only the unblocked part is registered (and the assertion also proves the
    // filename resolved to this series — otherwise the list would be empty).
    let parts: Vec<String> = state
        .db
        .get_episode_parts(&format!("{series_id}_S01E01"))
        .await
        .unwrap()
        .into_iter()
        .map(|p| p.file_path)
        .collect();
    assert_eq!(
        parts,
        vec![p1.to_string_lossy().to_string()],
        "a blocked part file must not be registered"
    );
}
