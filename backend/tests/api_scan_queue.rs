mod common;

use std::sync::Arc;
use std::time::Duration;

/// `scan_file_fingerprint` must create a fingerprint record and, when ffprobe
/// cannot extract media info, mark the file as permanently failed.
#[tokio::test]
async fn test_scan_file_fingerprint_creates_record() {
    let (_app, state, tmp) = common::setup_test_app().await;

    let file_path = tmp.path().join("test_video.mkv");
    tokio::fs::write(&file_path, b"fake video data")
        .await
        .unwrap();

    let (_hash_val, _hash, media_info) = state
        .db
        .scan_file_fingerprint(&file_path, "organized")
        .await;

    let path_str = file_path.to_string_lossy().to_string();
    let exists: bool =
        sqlx::query_scalar("SELECT COUNT(*) > 0 FROM file_paths WHERE file_path = ?")
            .bind(&path_str)
            .fetch_one(state.db.get_pool())
            .await
            .unwrap();
    assert!(
        exists,
        "scan_file_fingerprint should create a fingerprint record"
    );

    if media_info.is_none() {
        let failed = state
            .db
            .get_media_info_scan_failed_paths(std::slice::from_ref(&path_str))
            .await
            .unwrap();
        assert!(
            failed.contains(&path_str),
            "scan_file_fingerprint should mark failed files in DB"
        );

        let flag: Option<i64> = sqlx::query_scalar(
            "SELECT fc.media_info_scan_failed FROM file_paths fp \
             JOIN file_contents fc ON fc.fingerprint = fp.fingerprint \
             WHERE fp.file_path = ?",
        )
        .bind(&path_str)
        .fetch_optional(state.db.get_pool())
        .await
        .unwrap();
        assert_eq!(
            flag,
            Some(1),
            "media_info_scan_failed should be set to 1 when extraction fails"
        );
    } else {
        let failed = state
            .db
            .get_media_info_scan_failed_paths(std::slice::from_ref(&path_str))
            .await
            .unwrap();
        assert!(
            !failed.contains(&path_str),
            "Successful extract should not be marked as failed"
        );
    }
}

/// The scan queue must track submitted paths via `active_paths_snapshot`.
#[tokio::test]
async fn test_scan_queue_active_paths_tracking() {
    let (_app, state, tmp) = common::setup_test_app().await;

    let file_path = tmp.path().join("tracking_test.mkv");
    tokio::fs::write(&file_path, b"fake video content")
        .await
        .unwrap();

    let snapshot = state.scan_queue.active_paths_snapshot().await;
    assert!(snapshot.is_empty(), "Should have no active paths initially");

    // Block the job so it can be observed mid-scan.
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

    // Path should be visible in the snapshot immediately
    let snapshot = state.scan_queue.active_paths_snapshot().await;
    assert!(
        snapshot.contains(&file_path),
        "Submitted path should be in active_paths"
    );

    barrier.wait().await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    let snapshot = state.scan_queue.active_paths_snapshot().await;
    assert!(
        !snapshot.contains(&file_path),
        "Path should be removed after scan completes"
    );
}
