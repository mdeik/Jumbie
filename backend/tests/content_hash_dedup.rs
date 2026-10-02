// Content-hash dedup: copies of already-fingerprinted files reuse media_info
// instead of re-running ffprobe. The dedup works via the quick_hash lookup in
// update_file_fingerprint: when a file with a new inode has the same xxhash as
// an already-known file, the stored media_info is reused.

use jumbie::db::DbManager;
use std::path::Path;

/// Create a temp file with deterministic content.
async fn create_temp_file(dir: &Path, name: &str, content: &[u8]) -> std::path::PathBuf {
    let path = dir.join(name);
    tokio::fs::write(&path, content).await.unwrap();
    path
}

/// Create a DbManager backed by a temp file.
async fn create_db() -> (DbManager, tempfile::TempDir) {
    let tmp = tempfile::TempDir::new().unwrap();
    let db_path = tmp.path().join("test.db");
    let db = DbManager::new(&db_path).await.unwrap();
    (db, tmp)
}

/// A copy of a file produces a different inode, so has_existing_fingerprint
/// returns false.  But the xxhash is identical, so get_media_info_by_quick_hash
/// should return the original's media_info, and update_file_fingerprint should
/// skip ffprobe and reuse it.
#[tokio::test]
async fn test_content_hash_dedup_reuses_media_info() {
    let (db, tmp) = create_db().await;
    let content = b"fake video content for dedup test";
    let dir = tmp.path();

    let original = create_temp_file(dir, "original.mkv", content).await;
    let (_hash_val_1, hash_1, media_info_1) =
        db.update_file_fingerprint(&original, "complete").await;

    assert!(!hash_1.is_empty(), "hash should be computed");
    assert!(
        !hash_1.contains('-'),
        "hash should be a real xxhash (no dashes)"
    );

    // Same content, different path → different inode.
    let copy = create_temp_file(dir, "copy.mkv", content).await;

    let has_existing = db
        .has_existing_fingerprint(copy.to_str().unwrap())
        .await
        .unwrap();
    assert!(
        !has_existing,
        "copy has a different inode, so has_existing_fingerprint should be false"
    );

    let (_hash_val_2, hash_2, media_info_2) = db.update_file_fingerprint(&copy, "complete").await;

    assert_eq!(
        hash_1, hash_2,
        "identical content must produce the same quick_hash"
    );

    // The copy must reuse the original's media_info, not re-run ffprobe. Both
    // sides are None when ffprobe is unavailable, or Some and identical when it
    // ran on the original and the copy reused it.
    assert_eq!(
        media_info_1, media_info_2,
        "copy should reuse media_info from original"
    );

    let stored_hash: Option<String> =
        sqlx::query_scalar("SELECT fingerprint FROM file_paths WHERE file_path = ?")
            .bind(copy.to_str().unwrap())
            .fetch_optional(db.get_pool())
            .await
            .unwrap();
    assert_eq!(
        stored_hash.as_deref(),
        Some(hash_2.as_str()),
        "fingerprint row should exist for the copy"
    );
}

/// get_media_info_by_quick_hash should return media_info for a matching hash,
/// and None for hashes that don't exist or are identity-fallbacks.
#[tokio::test]
async fn test_get_media_info_by_quick_hash() {
    let (db, _tmp) = create_db().await;

    let known_hash = "a1b2c3d4e5f6a7b8";
    sqlx::query(
        "INSERT INTO file_contents (fingerprint, media_info) \
         VALUES (?, ?)",
    )
    .bind(known_hash)
    .bind(r#"{"codec":"h264","resolution":"1920x1080"}"#)
    .execute(db.get_pool())
    .await
    .unwrap();

    let result = db.get_media_info_by_quick_hash(known_hash).await.unwrap();
    assert_eq!(
        result.as_deref(),
        Some(r#"{"codec":"h264","resolution":"1920x1080"}"#),
        "should return media_info for matching quick_hash"
    );

    let result = db
        .get_media_info_by_quick_hash("0000000000000000")
        .await
        .unwrap();
    assert!(result.is_none(), "nonexistent hash should return None");

    let result = db.get_media_info_by_quick_hash("").await.unwrap();
    assert!(result.is_none(), "empty string should return None");

    let result = db
        .get_media_info_by_quick_hash("12345-67890-1234567890.123")
        .await
        .unwrap();
    assert!(
        result.is_none(),
        "identity-fallback hashes (with dashes) should return None"
    );
}

/// A file with the same content but a DIFFERENT name/extension is still a
/// content-duplicate — the dedup should work regardless of filename.
#[tokio::test]
async fn test_content_hash_dedup_different_filename() {
    let (db, tmp) = create_db().await;
    let content = b"same content, different name";
    let dir = tmp.path();

    let file_a = create_temp_file(dir, "Episode S01E01.mkv", content).await;
    let file_b = create_temp_file(dir, "Episode Backup.mkv", content).await;

    db.update_file_fingerprint(&file_a, "complete").await;
    let (_hash_val, hash, _media_info) = db.update_file_fingerprint(&file_b, "complete").await;

    assert!(!hash.is_empty(), "hash should be computed");
    assert!(!hash.contains('-'), "hash should be a real xxhash");

    // The copy's media_info was reused from file_a: if ffprobe was available
    // both are Some and identical, otherwise both are None. They must match.
    let stored_a: Option<String> = sqlx::query_scalar(
        "SELECT fc.media_info FROM file_paths fp JOIN file_contents fc ON fc.fingerprint = fp.fingerprint WHERE fp.file_path = ?",
    )
            .bind(file_a.to_str().unwrap())
            .fetch_optional(db.get_pool())
            .await
            .unwrap();

    let stored_b: Option<String> = sqlx::query_scalar(
        "SELECT fc.media_info FROM file_paths fp JOIN file_contents fc ON fc.fingerprint = fp.fingerprint WHERE fp.file_path = ?",
    )
            .bind(file_b.to_str().unwrap())
            .fetch_optional(db.get_pool())
            .await
            .unwrap();

    assert_eq!(stored_a, stored_b, "media_info must be identical");
}

/// The dedup query does NOT return media_info for rows where media_info_scan_failed = 1.
/// This prevents reusing media_info from a file whose extraction permanently failed.
#[tokio::test]
async fn test_get_media_info_by_quick_hash_ignores_failed_rows() {
    let (db, _tmp) = create_db().await;

    let failed_hash = "deadbeefcafebabe";
    let ok_hash = "feedfacecafebeef";

    // A content row whose media_info extraction permanently failed.
    sqlx::query(
        "INSERT INTO file_contents (fingerprint, media_info, media_info_scan_failed) \
         VALUES (?, NULL, 1)",
    )
    .bind(failed_hash)
    .execute(db.get_pool())
    .await
    .unwrap();

    // A content row that succeeded.
    sqlx::query(
        "INSERT INTO file_contents (fingerprint, media_info) \
         VALUES (?, '{\"codec\":\"h265\"}')",
    )
    .bind(ok_hash)
    .execute(db.get_pool())
    .await
    .unwrap();

    let result = db.get_media_info_by_quick_hash(ok_hash).await.unwrap();
    assert_eq!(
        result.as_deref(),
        Some(r#"{"codec":"h265"}"#),
        "should return media_info from the non-failed row"
    );
    assert!(
        db.get_media_info_by_quick_hash(failed_hash)
            .await
            .unwrap()
            .is_none(),
        "a failed content row has no media_info to return"
    );
}
