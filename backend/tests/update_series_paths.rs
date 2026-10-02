// update_series_paths tests
//
// After a series directory is moved on disk, `DbManager::update_series_paths`
// rewrites all DB paths that pointed inside the old directory so they point to
// the new location. Files whose paths were OUTSIDE the old directory must be
// left untouched.
//
// These tests inject known data into all affected tables and verify correct
// prefix replacement in each one.

use jumbie::db::DbManager;
use std::path::Path;

/// Helper: query all (file_path, expected_path) from file_paths.
async fn get_fingerprint_paths(db: &DbManager) -> Vec<(String, Option<String>)> {
    sqlx::query_as::<_, (String, Option<String>)>(
        "SELECT file_path, expected_path FROM file_paths ORDER BY file_path",
    )
    .fetch_all(db.get_pool())
    .await
    .unwrap()
}

// Insert helpers

/// Insert only the episode row, with no file association.
async fn insert_episode_row(db: &DbManager, episode_id: &str, episode_num: i32) {
    sqlx::query(
        "INSERT INTO episodes (episode_id, season, episode, status)
         VALUES (?, '1', ?, 'organized')
         ON CONFLICT(episode_id) DO NOTHING",
    )
    .bind(episode_id)
    .bind(episode_num)
    .execute(db.get_pool())
    .await
    .unwrap();
}

async fn insert_episode(db: &DbManager, episode_id: &str, episode_num: i32, file_path: &str) {
    insert_episode_row(db, episode_id, episode_num).await;
    db.associate_main_file(episode_id, file_path, None)
        .await
        .unwrap();
}

async fn insert_episode_part(db: &DbManager, episode_id: &str, part_number: u32, file_path: &str) {
    db.upsert_episode_part(episode_id, part_number, file_path, None)
        .await
        .unwrap();
}

/// All main-file paths (single + parts), ordered by episode then part.
async fn episode_main_paths(db: &DbManager) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT fp.file_path FROM episode_files ef
         JOIN file_paths fp ON fp.id = ef.file_path_id
         WHERE ef.kind = 'main'
         ORDER BY ef.episode_id, (ef.part_number IS NULL) DESC, ef.part_number",
    )
    .fetch_all(db.get_pool())
    .await
    .unwrap()
}

async fn insert_fingerprint(db: &DbManager, file_path: &str, expected_path: Option<&str>) {
    sqlx::query(
        "INSERT INTO file_paths (file_path, fingerprint, inode, device, size, mtime, state, expected_path)
         VALUES (?, 'fp', 0, 0, 0, 0.0, 'complete', ?)
         ON CONFLICT(file_path) DO UPDATE SET expected_path = COALESCE(excluded.expected_path, expected_path)",
    )
    .bind(file_path)
    .bind(expected_path)
    .execute(db.get_pool())
    .await
    .unwrap();
}

/// Helper: insert a fingerprint with quick_hash and media_info (for INSERT OR REPLACE tests).
/// Includes all columns that the migration copies so we can verify data is preserved.
async fn insert_fingerprint_full(
    db: &DbManager,
    file_path: &str,
    expected_path: Option<&str>,
    quick_hash: &str,
    media_info: &str,
) {
    sqlx::query(
        "INSERT INTO file_contents (fingerprint, media_info) VALUES (?, ?)
         ON CONFLICT(fingerprint) DO UPDATE SET media_info = COALESCE(excluded.media_info, media_info)",
    )
    .bind(quick_hash)
    .bind(media_info)
    .execute(db.get_pool())
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO file_paths
           (file_path, inode, device, size, mtime, fingerprint, state, expected_path)
         VALUES (?, 0, 0, 0, 0.0, ?, 'complete', ?)
         ON CONFLICT(file_path) DO UPDATE SET
           fingerprint   = excluded.fingerprint,
           expected_path = COALESCE(excluded.expected_path, expected_path)",
    )
    .bind(file_path)
    .bind(quick_hash)
    .bind(expected_path)
    .execute(db.get_pool())
    .await
    .unwrap();
}

async fn insert_retry(db: &DbManager, operation: &str, source_path: &str, dest_path: Option<&str>) {
    sqlx::query(
        "INSERT INTO retry_queue (operation, source_path, destination_path, next_retry_at, status)
         VALUES (?, ?, ?, datetime('now', '+1 hour'), 'pending')",
    )
    .bind(operation)
    .bind(source_path)
    .bind(dest_path)
    .execute(db.get_pool())
    .await
    .unwrap();
}

// Scenarios

/// The old prefix contains a substring that could match a different directory.
/// e.g. old="/data/My Show" should NOT match "/data/My Showcase/file.mkv".
#[tokio::test]
async fn test_does_not_match_similar_directory_names() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db_path = tmp.path().join("test.db");
    let db = DbManager::new(&db_path).await.unwrap();

    // Use distinct episode_ids and episode numbers for each row
    insert_episode(&db, "in1", 1, "/data/My Show/S01/ep01.mkv").await;
    // Insert outside with ep02 so it doesn't collide
    insert_episode(&db, "out1", 2, "/data/My Showcase/S01/ep01.mkv").await;

    db.update_series_paths(Path::new("/data/My Show"), Path::new("/data/New Show"))
        .await
        .unwrap();

    let paths = episode_main_paths(&db).await;
    assert_eq!(paths.len(), 2, "both episodes should still exist");

    // Files INSIDE old dir are updated
    assert!(
        paths.contains(&"/data/New Show/S01/ep01.mkv".to_string()),
        "inside file should be updated: got {:?}",
        paths
    );
    // Files OUTSIDE old dir are preserved
    assert!(
        paths.contains(&"/data/My Showcase/S01/ep01.mkv".to_string()),
        "outside file should be preserved: got {:?}",
        paths
    );
}

/// All tables (episodes, episode_parts, file_paths, retry_queue)
/// have their paths updated correctly.
#[tokio::test]
async fn test_all_tables_updated_in_transaction() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db_path = tmp.path().join("test.db");
    let db = DbManager::new(&db_path).await.unwrap();

    // Set up: old series at /data/tv/My Series
    let old_root = "/data/tv/My Series";

    // Files INSIDE the old directory
    // Episode 1: multipart, both parts inside the old directory.
    insert_episode_row(&db, "ep1", 1).await;
    insert_episode_part(&db, "ep1", 1, &format!("{}/S01/ep01.mkv", old_root)).await;
    insert_episode_part(&db, "ep1", 2, &format!("{}/S01/ep01-pt2.mkv", old_root)).await;

    // Episode 2: single file inside the old directory.
    insert_episode(&db, "ep2", 2, &format!("{}/S01/ep02.mkv", old_root)).await;

    insert_fingerprint(
        &db,
        &format!("{}/S01/ep01.mkv", old_root),
        Some(&format!("{}/S01/ep01.mkv", old_root)),
    )
    .await;
    insert_fingerprint(&db, &format!("{}/S01/ep02.mkv", old_root), None).await;

    insert_retry(
        &db,
        "rename",
        &format!("{}/S01/ep01.mkv", old_root),
        Some(&format!("{}/S01/ep01-fixed.mkv", old_root)),
    )
    .await;

    // Files OUTSIDE the old directory (should be preserved)
    insert_episode(&db, "outside1", 3, "/data/downloads/My Series S01E01.mkv").await;
    insert_fingerprint(&db, "/data/downloads/My Series S01E01.mkv", None).await;
    insert_retry(&db, "rename", "/data/downloads/My Series S01E01.mkv", None).await;

    // Act
    db.update_series_paths(
        Path::new("/data/tv/My Series"),
        Path::new("/data/new/My Series"),
    )
    .await
    .unwrap();

    let mains = episode_main_paths(&db).await;
    assert_eq!(mains.len(), 4, "2 parts + inside single + outside single");
    // Inside files: updated
    assert!(mains.contains(&"/data/new/My Series/S01/ep01.mkv".to_string()));
    assert!(mains.contains(&"/data/new/My Series/S01/ep01-pt2.mkv".to_string()));
    assert!(mains.contains(&"/data/new/My Series/S01/ep02.mkv".to_string()));
    // Outside file: preserved
    assert!(mains.contains(&"/data/downloads/My Series S01E01.mkv".to_string()));

    let fps = get_fingerprint_paths(&db).await;
    assert_eq!(fps.len(), 4, "2 parts + inside single + outside single");

    // Inside fingerprints: updated
    assert!(fps.contains(&(
        "/data/new/My Series/S01/ep01.mkv".to_string(),
        Some("/data/new/My Series/S01/ep01.mkv".to_string())
    )));
    assert!(fps.contains(&("/data/new/My Series/S01/ep02.mkv".to_string(), None)));
    // Outside fingerprint: preserved
    assert!(fps.contains(&("/data/downloads/My Series S01E01.mkv".to_string(), None)));

    let retry_sources: Vec<String> =
        sqlx::query_scalar::<_, String>("SELECT source_path FROM retry_queue ORDER BY source_path")
            .fetch_all(db.get_pool())
            .await
            .unwrap();
    assert_eq!(retry_sources.len(), 2);
    // Inside retry: updated
    assert!(retry_sources.contains(&"/data/new/My Series/S01/ep01.mkv".to_string()));
    // Outside retry: preserved
    assert!(retry_sources.contains(&"/data/downloads/My Series S01E01.mkv".to_string()));

    // Also verify destination_path was updated for the inside retry
    let dest_paths: Vec<Option<String>> = sqlx::query_scalar::<_, Option<String>>(
        "SELECT destination_path FROM retry_queue ORDER BY source_path",
    )
    .fetch_all(db.get_pool())
    .await
    .unwrap();
    assert!(dest_paths.contains(&Some("/data/new/My Series/S01/ep01-fixed.mkv".to_string())));
}

/// Moving to the same prefix is a no-op.
#[tokio::test]
async fn test_noop_when_old_prefix_equals_new_prefix() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db_path = tmp.path().join("test.db");
    let db = DbManager::new(&db_path).await.unwrap();

    insert_episode(&db, "ep1", 1, "/data/tv/Series/S01/ep01.mkv").await;

    db.update_series_paths(Path::new("/data/tv/Series"), Path::new("/data/tv/Series"))
        .await
        .unwrap();

    let paths = episode_main_paths(&db).await;
    assert_eq!(paths, vec!["/data/tv/Series/S01/ep01.mkv"]);
}

/// Empty old directory (no matching files) should not break anything.
#[tokio::test]
async fn test_no_matching_files_is_ok() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db_path = tmp.path().join("test.db");
    let db = DbManager::new(&db_path).await.unwrap();

    insert_episode(&db, "ep1", 1, "/somewhere/else/file.mkv").await;

    db.update_series_paths(
        Path::new("/data/tv/Empty Series"),
        Path::new("/data/new/Empty Series"),
    )
    .await
    .unwrap();

    let paths = episode_main_paths(&db).await;
    assert_eq!(paths, vec!["/somewhere/else/file.mkv"]);
}

/// Symbolic link in the old prefix (the DB stores the resolved path).
#[tokio::test]
async fn test_path_with_deep_nesting() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db_path = tmp.path().join("test.db");
    let db = DbManager::new(&db_path).await.unwrap();

    insert_episode(
        &db,
        "ep1",
        1,
        "/deeply/nested/tv/storage/Series Name/Season 1/Episode 01.mkv",
    )
    .await;

    db.update_series_paths(
        Path::new("/deeply/nested/tv/storage/Series Name"),
        Path::new("/new/location/Series Name"),
    )
    .await
    .unwrap();

    let paths = episode_main_paths(&db).await;
    assert_eq!(
        paths,
        vec!["/new/location/Series Name/Season 1/Episode 01.mkv"]
    );
}

/// INSERT OR REPLACE ensures migrated fingerprint data replaces stale entries at
/// the destination path. Before the fix, INSERT OR IGNORE would silently drop the
/// migrated row if a fingerprint already existed at the new path, causing data loss.
#[tokio::test]
async fn test_insert_or_replace_migrates_existing_destination() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db_path = tmp.path().join("test.db");
    let db = DbManager::new(&db_path).await.unwrap();

    let old_path = "/data/tv/Show/S01/ep01.mkv";
    let new_path = "/data/tv/Show-New/S01/ep01.mkv";

    // Insert the real fingerprint at the old path (data we want to migrate).
    insert_fingerprint_full(
        &db,
        old_path,
        Some(old_path),
        "abc123",
        r#"{"codec":"h264"}"#,
    )
    .await;

    // Insert a stale/different fingerprint at the destination path (simulates a
    // prior scan that left an outdated entry).
    insert_fingerprint_full(
        &db,
        new_path,
        Some(new_path),
        "def456",
        r#"{"codec":"hevc"}"#,
    )
    .await;

    // Act: migrate from old prefix to new prefix.
    db.update_series_paths(Path::new("/data/tv/Show"), Path::new("/data/tv/Show-New"))
        .await
        .unwrap();

    // replaced by INSERT OR REPLACE with the migrated data).
    let fps = get_fingerprint_paths(&db).await;
    assert_eq!(fps.len(), 1, "only the migrated fingerprint should remain");
    assert_eq!(fps[0].0, new_path, "file_path should be the new location");
    assert_eq!(
        fps[0].1.as_deref(),
        Some(new_path),
        "expected_path should be the new location"
    );

    let quick_hashes: Vec<String> =
        sqlx::query_scalar::<_, String>("SELECT fingerprint FROM file_paths ORDER BY file_path")
            .fetch_all(db.get_pool())
            .await
            .unwrap();
    assert_eq!(
        quick_hashes,
        vec!["abc123"],
        "quick_hash should be the migrated one"
    );

    let media_infos: Vec<String> = sqlx::query_scalar::<_, String>(
        "SELECT fc.media_info FROM file_paths fp \
         JOIN file_contents fc ON fc.fingerprint = fp.fingerprint ORDER BY fp.file_path",
    )
    .fetch_all(db.get_pool())
    .await
    .unwrap();
    assert_eq!(
        media_infos,
        vec![r#"{"codec":"h264"}"#],
        "media_info should be the migrated one"
    );
}

/// download_id is unaffected by path moves — it tracks the btih (torrent) hash,
/// not the file content hash. This test verifies it survives the move unchanged.
#[tokio::test]
async fn test_download_id_survives_path_move() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db_path = tmp.path().join("test.db");
    let db = DbManager::new(&db_path).await.unwrap();

    let old_path = "/data/tv/Show/S01/ep01.mkv";

    insert_episode(&db, "ep1", 1, old_path).await;

    db.update_series_paths(Path::new("/data/tv/Show"), Path::new("/data/tv/Show-New"))
        .await
        .unwrap();

    let file_path = db.get_episode_file_path("ep1").await.unwrap();
    assert_eq!(
        file_path.as_deref(),
        Some("/data/tv/Show-New/S01/ep01.mkv"),
        "file_path must survive path moves unchanged"
    );
}

// clear_series_paths tests

/// Inside files are cleared, outside files are preserved.
#[tokio::test]
async fn test_clear_series_paths_inside_files_only() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db_path = tmp.path().join("test.db");
    let db = DbManager::new(&db_path).await.unwrap();

    let old_root = "/data/tv/My Series";

    // Files INSIDE the old directory
    insert_episode(&db, "ep1", 1, &format!("{}/S01/ep01.mkv", old_root)).await;
    insert_episode(&db, "ep2", 2, &format!("{}/S01/ep02.mkv", old_root)).await;

    insert_fingerprint(
        &db,
        &format!("{}/S01/ep01.mkv", old_root),
        Some(&format!("{}/S01/ep01.mkv", old_root)),
    )
    .await;

    insert_retry(
        &db,
        "rename",
        &format!("{}/S01/ep01.mkv", old_root),
        Some(&format!("{}/S01/ep01-renamed.mkv", old_root)),
    )
    .await;

    // Files OUTSIDE the old directory
    insert_episode(&db, "outside1", 3, "/data/downloads/My Series S01E01.mkv").await;
    insert_fingerprint(&db, "/data/downloads/My Series S01E01.mkv", None).await;
    insert_retry(&db, "rename", "/data/downloads/My Series S01E01.mkv", None).await;

    // Act
    db.clear_series_paths(Path::new("/data/tv/My Series"))
        .await
        .unwrap();

    // Inside episodes lost their file; the outside one is preserved.
    let mains = episode_main_paths(&db).await;
    assert_eq!(mains.len(), 1, "only the outside file should remain");
    assert_eq!(mains[0], "/data/downloads/My Series S01E01.mkv");

    let fp_paths: Vec<String> =
        sqlx::query_scalar::<_, String>("SELECT file_path FROM file_paths ORDER BY file_path")
            .fetch_all(db.get_pool())
            .await
            .unwrap();
    assert_eq!(fp_paths.len(), 1, "only outside fingerprint should remain");
    assert_eq!(fp_paths[0], "/data/downloads/My Series S01E01.mkv");

    let retry_sources: Vec<String> =
        sqlx::query_scalar::<_, String>("SELECT source_path FROM retry_queue ORDER BY source_path")
            .fetch_all(db.get_pool())
            .await
            .unwrap();
    assert_eq!(
        retry_sources.len(),
        1,
        "inside retry should be deleted, outside preserved"
    );
    assert_eq!(retry_sources[0], "/data/downloads/My Series S01E01.mkv");
}

/// No matching files ŌĆö should not affect anything.
#[tokio::test]
async fn test_clear_series_paths_no_matching_files() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db_path = tmp.path().join("test.db");
    let db = DbManager::new(&db_path).await.unwrap();

    insert_episode(&db, "ep1", 1, "/somewhere/else/file.mkv").await;

    db.clear_series_paths(Path::new("/data/tv/Empty Series"))
        .await
        .unwrap();

    let paths = episode_main_paths(&db).await;
    assert_eq!(paths, vec!["/somewhere/else/file.mkv"]);
}

/// NULL paths should not be affected (they're skipped by the IS NOT NULL filter).
#[tokio::test]
async fn test_clear_series_paths_with_null_paths() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db_path = tmp.path().join("test.db");
    let db = DbManager::new(&db_path).await.unwrap();

    // Insert an episode with no file association.
    insert_episode_row(&db, "nullpath", 1).await;

    // Insert one with a real path (matching old prefix to be cleared)
    insert_episode(&db, "realpath", 2, "/data/tv/Series/S01/ep02.mkv").await;

    db.clear_series_paths(Path::new("/data/tv/Series"))
        .await
        .unwrap();

    // No main file remains; both episode rows survive.
    assert!(episode_main_paths(&db).await.is_empty());
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM episodes")
        .fetch_one(db.get_pool())
        .await
        .unwrap();
    assert_eq!(count, 2);
}

#[tokio::test]
async fn test_update_series_paths_windows_backslashes() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db_path = tmp.path().join("test.db");
    let db = DbManager::new(&db_path).await.unwrap();

    let inside_old = r"C:\media\tv\Old Series\Season 01\ep01.mkv";
    let inside_new = r"C:\media\tv\New Series\Season 01\ep01.mkv";
    let outside = r"C:\media\tv\Old Series Extra\ep02.mkv";

    insert_episode(&db, "in_win", 1, inside_old).await;
    insert_episode(&db, "out_win", 2, outside).await;
    insert_fingerprint(&db, inside_old, Some(inside_old)).await;
    insert_retry(&db, "rename", inside_old, Some(inside_old)).await;

    db.update_series_paths(
        Path::new(r"C:\media\tv\Old Series"),
        Path::new(r"C:\media\tv\New Series"),
    )
    .await
    .unwrap();

    let paths = episode_main_paths(&db).await;
    assert_eq!(paths.len(), 2);
    assert!(paths.contains(&inside_new.to_string()));
    assert!(paths.contains(&outside.to_string()));

    let fp_paths: Vec<String> =
        sqlx::query_scalar::<_, String>("SELECT file_path FROM file_paths ORDER BY file_path")
            .fetch_all(db.get_pool())
            .await
            .unwrap();
    assert!(fp_paths.contains(&inside_new.to_string()));
    assert!(!fp_paths.contains(&inside_old.to_string()));

    let retry_sources: Vec<String> =
        sqlx::query_scalar::<_, String>("SELECT source_path FROM retry_queue")
            .fetch_all(db.get_pool())
            .await
            .unwrap();
    assert!(retry_sources.contains(&inside_new.to_string()));
}

#[tokio::test]
async fn test_clear_series_paths_windows_backslashes() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db_path = tmp.path().join("test.db");
    let db = DbManager::new(&db_path).await.unwrap();

    let inside = r"C:\media\tv\My Series\Season 01\ep01.mkv";
    let outside = r"C:\downloads\file.mkv";

    insert_episode(&db, "in_win", 1, inside).await;
    insert_episode(&db, "out_win", 2, outside).await;
    insert_fingerprint(&db, inside, None).await;
    insert_fingerprint(&db, outside, None).await;
    insert_retry(&db, "rename", inside, None).await;
    insert_retry(&db, "rename", outside, None).await;

    db.clear_series_paths(Path::new(r"C:\media\tv\My Series"))
        .await
        .unwrap();

    let paths = episode_main_paths(&db).await;
    assert_eq!(paths, vec![outside.to_string()]);

    let fp_paths: Vec<String> =
        sqlx::query_scalar::<_, String>("SELECT file_path FROM file_paths ORDER BY file_path")
            .fetch_all(db.get_pool())
            .await
            .unwrap();
    assert_eq!(fp_paths, vec![outside.to_string()]);

    let retry_sources: Vec<String> =
        sqlx::query_scalar::<_, String>("SELECT source_path FROM retry_queue")
            .fetch_all(db.get_pool())
            .await
            .unwrap();
    assert_eq!(retry_sources, vec![outside.to_string()]);
}

