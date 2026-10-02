mod common;

use axum::http::StatusCode;
use common::TestApp;
use jumbie_shared::mapping::MonitorMode;
use jumbie_shared::types::{
    AssignFilePayload, BatchAssignPayload, BatchDeletePayload, ConfirmSeriesImportRequest,
    PreviewSeriesItem, SeriesDetails,
};
use tower::ServiceExt;

// POST /api/series/:id/files/assign
//
// The endpoint returns 404 when the file does not exist on disk, so we can
// exercise several code-paths without creating real media files.

#[tokio::test]
async fn test_assign_file_series_not_found() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let payload = AssignFilePayload {
        path: "/nonexistent/file.mkv".to_string(),
        season: "01".to_string(),
        episode: "1".to_string(),
    };
    let res = app
        .oneshot(common::post_json_request(
            "/api/series/nonexistent_series_id/files/assign",
            &payload,
        ))
        .await
        .unwrap();
    // File doesn't exist → 404 before even reaching the series look-up
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_assign_file_missing_file() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Assign Show").await;
    let payload = AssignFilePayload {
        path: "/nonexistent/missing.mkv".to_string(),
        season: "01".to_string(),
        episode: "1".to_string(),
    };
    let res = app
        .clone()
        .oneshot(common::post_json_request(
            &format!("/api/series/{}/files/assign", series_id),
            &payload,
        ))
        .await
        .unwrap();
    // The source file does not exist → 404
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_assign_file_invalid_episode_format() {
    let (app, _state, tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Assign Bad Ep Show").await;

    // Create a real file so the path check passes
    let file_path = tmp.path().join("episode.mkv");
    tokio::fs::write(&file_path, b"fake video").await.unwrap();

    let payload = AssignFilePayload {
        path: file_path.to_string_lossy().to_string(),
        season: "01".to_string(),
        episode: "not_a_number".to_string(), // invalid
    };
    let res = app
        .clone()
        .oneshot(common::post_json_request(
            &format!("/api/series/{}/files/assign", series_id),
            &payload,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_assign_file_inverted_range() {
    let (app, _state, tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Assign Range Show").await;

    let file_path = tmp.path().join("episode_range.mkv");
    tokio::fs::write(&file_path, b"fake video").await.unwrap();

    let payload = AssignFilePayload {
        path: file_path.to_string_lossy().to_string(),
        season: "01".to_string(),
        episode: "5-2".to_string(), // end < start → invalid
    };
    let res = app
        .clone()
        .oneshot(common::post_json_request(
            &format!("/api/series/{}/files/assign", series_id),
            &payload,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_assign_file_success() {
    let (app, _state, tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Assign OK Show").await;

    let file_path = tmp
        .path()
        .join("organized")
        .join("Assign OK Show")
        .join("S01E01.mkv");
    tokio::fs::create_dir_all(file_path.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(&file_path, b"fake video").await.unwrap();

    let payload = AssignFilePayload {
        path: file_path.to_string_lossy().to_string(),
        season: "01".to_string(),
        episode: "1".to_string(),
    };
    let res = app
        .clone()
        .oneshot(common::post_json_request(
            &format!("/api/series/{}/files/assign", series_id),
            &payload,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_assign_nfo_does_not_unlink_episode_video() {
    // Reproduces the bug where linking an nfo to an episode that already has a
    // video file silently unlinked (overwrote) the video: assigning an auxiliary
    // sidecar must only create an `auxiliary` association, never replace the
    // episode's playable main file. It used to "work" only when the nfo was linked first.
    let (app, _state, tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Aux Assign Show").await;

    let series_dir = tmp.path().join("organized").join("Aux Assign Show");
    tokio::fs::create_dir_all(&series_dir).await.unwrap();

    // 1. Assign the video.
    let video = series_dir.join("Aux Assign Show - S01E01.mkv");
    tokio::fs::write(&video, b"fake video").await.unwrap();
    let res = app
        .clone()
        .oneshot(common::post_json_request(
            &format!("/api/series/{}/files/assign", series_id),
            &AssignFilePayload {
                path: video.to_string_lossy().to_string(),
                season: "01".to_string(),
                episode: "1".to_string(),
            },
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // 2. Assign the nfo to the SAME episode (which now has a video).
    let nfo = series_dir.join("Aux Assign Show - S01E01.nfo");
    tokio::fs::write(&nfo, b"fake nfo").await.unwrap();
    let res = app
        .clone()
        .oneshot(common::post_json_request(
            &format!("/api/series/{}/files/assign", series_id),
            &AssignFilePayload {
                path: nfo.to_string_lossy().to_string(),
                season: "01".to_string(),
                episode: "1".to_string(),
            },
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let details: jumbie_shared::types::SeriesDetails =
        app.get_json(&format!("/api/series/{}", series_id)).await;
    let ep = details
        .episodes
        .iter()
        .find(|e| e.episode == 1)
        .expect("episode 1");
    assert!(
        ep.path.as_deref().is_some_and(|p| p.ends_with(".mkv")),
        "the episode's playable file must stay the video, got {:?}",
        ep.path
    );
    assert!(
        ep.auxiliary_files
            .iter()
            .any(|a| a.kind == jumbie_shared::media_format::FileKind::Nfo),
        "the nfo should be linked as auxiliary: {:?}",
        ep.auxiliary_files
    );
}

#[tokio::test]
async fn test_batch_assign_shift_up_preserves_all_episodes() {
    // Reproduces the batch "shift by one" bug: with automatic renames enabled,
    // reassigning E17→E18, E18→E19, E19→E20, E20→E21 used to leave only the last
    // one assigned. Interleaving each rename with its DB assignment recycled the
    // paths the later items still used as sources, so the "clear previous holder"
    // step wiped the assignments that had just been written.
    let (app, state, tmp) = common::setup_test_app().await;

    // Renaming on (organize is what recycles the on-disk paths).
    {
        let mut org = state.db.get_organization_config().await.unwrap_or_default();
        org.auto_apply_renames = true;
        let _ = state.db.save_organization_config(&org).await;
        let mut cfg = state.cfg.write().await;
        cfg.organization.auto_apply_renames = true;
    }

    let series_id = common::create_test_series(&app, "Shift Show").await;
    let series_dir = tmp.path().join("organized").join("Shift Show");
    tokio::fs::create_dir_all(&series_dir).await.unwrap();

    // Assign 4 files to S00E17..S00E20. Each assign organizes the file to its
    // canonical episode name, so the on-disk paths become S00E17..S00E20.
    let mut current_paths: Vec<String> = Vec::new();
    for ep in 17..=20 {
        let incoming = series_dir.join(format!("incoming_{}.mkv", ep));
        tokio::fs::write(&incoming, format!("content-{}", ep))
            .await
            .unwrap();
        let res = app
            .clone()
            .oneshot(common::post_json_request(
                &format!("/api/series/{}/files/assign", series_id),
                &AssignFilePayload {
                    path: incoming.to_string_lossy().to_string(),
                    season: "00".to_string(),
                    episode: ep.to_string(),
                },
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        let details: SeriesDetails = app.get_json(&format!("/api/series/{}", series_id)).await;
        let row = details
            .episodes
            .iter()
            .find(|e| e.episode == ep)
            .unwrap_or_else(|| panic!("episode {} not assigned", ep));
        current_paths.push(row.path.clone().expect("assigned path"));
    }

    // Wait for the background fingerprint tasks kicked off by the initial
    // assigns to finish; while a file is mid-scan the batch organize is skipped,
    // which would mask the path-recycling bug this test targets.
    for _ in 0..100 {
        let mut busy = false;
        for p in &current_paths {
            if state.scan_queue.contains(std::path::Path::new(p)).await {
                busy = true;
                break;
            }
        }
        if !busy {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    // Seed per-content release metadata so the shift can be checked against the
    // joined provenance fields (Release Name / size / Original Path).
    for (idx, path) in current_paths.iter().enumerate() {
        let ep = 17 + idx as i32;
        let _ = state
            .db
            .record_hash_only(std::path::Path::new(path), "organized")
            .await;
        state
            .db
            .set_release_info_by_path(path, Some(&format!("Release {}", ep)), None, None)
            .await
            .unwrap();
    }

    // Shift every episode up by one.
    let payload = BatchAssignPayload {
        paths: current_paths.clone(),
        start_season: "00".to_string(),
        start_episode: 18,
        is_multipart: false,
    };
    let res = app
        .clone()
        .oneshot(common::post_json_request(
            &format!("/api/series/{}/files/batch-assign", series_id),
            &payload,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let details: SeriesDetails = app.get_json(&format!("/api/series/{}", series_id)).await;
    for ep in 18..=21 {
        let row = details
            .episodes
            .iter()
            .find(|e| e.episode == ep)
            .unwrap_or_else(|| panic!("episode {} missing after shift", ep));
        let path = row
            .path
            .as_deref()
            .unwrap_or_else(|| panic!("episode {} lost its file after shift", ep));
        assert!(
            std::path::Path::new(path).exists(),
            "episode {} points at a missing file: {}",
            ep,
            path
        );
        // The shift must not leave spurious collision suffixes behind.
        let expected = format!("Shift Show - S00E{:02} - .mkv", ep);
        assert!(
            path.ends_with(&expected),
            "episode {} should be at its canonical name '{}', got '{}'",
            ep,
            expected,
            path
        );
        // Content must have followed its episode (E17's file is now E18, …).
        let content = tokio::fs::read_to_string(path).await.unwrap();
        assert_eq!(
            content,
            format!("content-{}", ep - 1),
            "episode {} has the wrong file contents",
            ep
        );

        // The joined provenance fields must still resolve for the content now on
        // the episode (they used to go stale/blank after a batch rename).
        let release = format!("Release {}", ep - 1);
        assert_eq!(
            row.release_title.as_deref(),
            Some(release.as_str()),
            "episode {} release name must follow its content",
            ep
        );
        assert!(row.size > 0, "episode {} must report its file size", ep);
        let origin = format!("Shift Show - S00E{:02} - .mkv", ep - 1);
        assert!(
            row.original_path
                .as_deref()
                .is_some_and(|p| p.ends_with(&origin)),
            "episode {} original path should be where its content was first seen ('{}'), got {:?}",
            ep,
            origin,
            row.original_path
        );
    }
}

#[tokio::test]
async fn test_reassign_refreshes_release_metadata() {
    // After Manage Series Files assigns a DIFFERENT file to an episode, the
    // episode's origin/provenance fields (Original Path, Release Name) must
    // follow the file that is now on the episode.
    let (app, state, tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Meta Show").await;
    let series_dir = tmp.path().join("organized").join("Meta Show");
    tokio::fs::create_dir_all(&series_dir).await.unwrap();

    let assign = |path: String| {
        let app = app.clone();
        let series_id = series_id.clone();
        async move {
            let res = app
                .oneshot(common::post_json_request(
                    &format!("/api/series/{}/files/assign", series_id),
                    &AssignFilePayload {
                        path,
                        season: "01".to_string(),
                        episode: "1".to_string(),
                    },
                ))
                .await
                .unwrap();
            assert_eq!(res.status(), StatusCode::OK);
        }
    };

    let drain_scan_queue = |paths: Vec<String>| {
        let state = state.clone();
        async move {
            for _ in 0..100 {
                let mut busy = false;
                for p in &paths {
                    if state.scan_queue.contains(std::path::Path::new(p)).await {
                        busy = true;
                        break;
                    }
                }
                if !busy {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        }
    };

    // --- File A assigned to E01 ---
    let a = series_dir.join("A.mkv");
    tokio::fs::write(&a, b"content A AAAAAAAA").await.unwrap();
    assign(a.to_string_lossy().to_string()).await;
    drain_scan_queue(vec![a.to_string_lossy().to_string()]).await;

    let details: SeriesDetails = app.get_json(&format!("/api/series/{}", series_id)).await;
    let a_final = details
        .episodes
        .iter()
        .find(|e| e.episode == 1)
        .and_then(|e| e.path.clone())
        .expect("A assigned");
    let _ = state
        .db
        .record_hash_only(std::path::Path::new(&a_final), "organized")
        .await;
    state
        .db
        .set_release_info_by_path(&a_final, Some("Release A"), None, Some("GroupA"))
        .await
        .unwrap();

    let details: SeriesDetails = app.get_json(&format!("/api/series/{}", series_id)).await;
    let ep1 = details.episodes.iter().find(|e| e.episode == 1).unwrap();
    assert_eq!(ep1.release_title.as_deref(), Some("Release A"));
    assert_eq!(ep1.original_path.as_deref(), Some(a_final.as_str()));

    // --- Replace with File B on the same episode ---
    let b = series_dir.join("B.mkv");
    tokio::fs::write(&b, b"content B BBBBBBBB").await.unwrap();
    assign(b.to_string_lossy().to_string()).await;
    drain_scan_queue(vec![b.to_string_lossy().to_string()]).await;

    let details: SeriesDetails = app.get_json(&format!("/api/series/{}", series_id)).await;
    let b_final = details
        .episodes
        .iter()
        .find(|e| e.episode == 1)
        .and_then(|e| e.path.clone())
        .expect("B assigned");
    let _ = state
        .db
        .record_hash_only(std::path::Path::new(&b_final), "organized")
        .await;
    state
        .db
        .set_release_info_by_path(&b_final, Some("Release B"), None, Some("GroupB"))
        .await
        .unwrap();

    let details: SeriesDetails = app.get_json(&format!("/api/series/{}", series_id)).await;
    let ep1 = details.episodes.iter().find(|e| e.episode == 1).unwrap();
    assert_eq!(
        ep1.release_title.as_deref(),
        Some("Release B"),
        "release name must follow the episode's current file"
    );
    assert_eq!(
        ep1.original_path.as_deref(),
        Some(b_final.as_str()),
        "original path must follow the episode's current content"
    );
}

#[tokio::test]
async fn test_assign_file_non_numeric_season_is_rejected() {
    // A season that isn't a number has no episode identity. The assign handler
    // must reject it (400) rather than coercing it into some season and writing
    // the file under the wrong season.
    let (app, _state, tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Assign Bad Season Show").await;

    let file_path = tmp
        .path()
        .join("organized")
        .join("Assign Bad Season Show")
        .join("SP.mkv");
    tokio::fs::create_dir_all(file_path.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(&file_path, b"fake video").await.unwrap();

    let payload = AssignFilePayload {
        path: file_path.to_string_lossy().to_string(),
        season: "SP".to_string(),
        episode: "1".to_string(),
    };
    let res = app
        .clone()
        .oneshot(common::post_json_request(
            &format!("/api/series/{}/files/assign", series_id),
            &payload,
        ))
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        StatusCode::BAD_REQUEST,
        "a non-numeric season must be rejected, not coerced"
    );
}

#[tokio::test]
async fn test_assign_file_multi_episode_range() {
    let (app, _state, tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Assign Multi Show").await;

    let file_path = tmp
        .path()
        .join("organized")
        .join("Assign Multi Show")
        .join("S01E01-E03.mkv");
    tokio::fs::create_dir_all(file_path.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(&file_path, b"fake multi video")
        .await
        .unwrap();

    let payload = AssignFilePayload {
        path: file_path.to_string_lossy().to_string(),
        season: "01".to_string(),
        episode: "1-3".to_string(), // valid range
    };
    let res = app
        .clone()
        .oneshot(common::post_json_request(
            &format!("/api/series/{}/files/assign", series_id),
            &payload,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

// POST /api/series/:id/files/unassign

#[tokio::test]
async fn test_unassign_file_empty_paths() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Unassign Show").await;
    let payload = BatchDeletePayload { paths: vec![] };
    let res = app
        .clone()
        .oneshot(common::post_json_request(
            &format!("/api/series/{}/files/unassign", series_id),
            &payload,
        ))
        .await
        .unwrap();
    // Empty paths → silently succeeds
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_unassign_file_missing_path() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Unassign Missing Show").await;
    // Paths that don't exist on disk are simply skipped (logged as a warning)
    let payload = BatchDeletePayload {
        paths: vec!["/nonexistent/path/some_ep.mkv".to_string()],
    };
    let res = app
        .clone()
        .oneshot(common::post_json_request(
            &format!("/api/series/{}/files/unassign", series_id),
            &payload,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_unassign_file_existing_path() {
    let (app, _state, tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Unassign Existing Show").await;

    // Create a real file so the path.exists() branch is entered
    let file_path = tmp.path().join("some_video.mkv");
    tokio::fs::write(&file_path, b"fake video").await.unwrap();

    let payload = BatchDeletePayload {
        paths: vec![file_path.to_string_lossy().to_string()],
    };
    let res = app
        .clone()
        .oneshot(common::post_json_request(
            &format!("/api/series/{}/files/unassign", series_id),
            &payload,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

// POST /api/series/:id/files/batch-assign

#[tokio::test]
async fn test_unassign_severs_file_paths_episode_link() {
    // Unassign must drop the episode's association, not just its status —
    // otherwise the episode-details join can still resolve the file.
    let (app, state, tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Unassign Link Show").await;

    let file_path = tmp.path().join("linked.mkv");
    tokio::fs::write(&file_path, b"fake video").await.unwrap();
    let path_str = file_path.to_string_lossy().to_string();

    let _ = state.db.record_hash_only(&file_path, "organized").await;
    sqlx::query(
        "INSERT INTO episodes (episode_id, series_id, season, episode) VALUES ('ep-x', ?, 1, 1)",
    )
    .bind(&series_id)
    .execute(state.db.get_pool())
    .await
    .unwrap();
    state
        .db
        .associate_main_file("ep-x", &path_str, None)
        .await
        .unwrap();

    let payload = BatchDeletePayload {
        paths: vec![path_str.clone()],
    };
    let res = app
        .clone()
        .oneshot(common::post_json_request(
            &format!("/api/series/{}/files/unassign", series_id),
            &payload,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let linked: Option<String> = sqlx::query_scalar(
        "SELECT ef.episode_id FROM episode_files ef \
         JOIN file_paths fp ON fp.id = ef.file_path_id WHERE fp.file_path = ?",
    )
    .bind(&path_str)
    .fetch_optional(state.db.get_pool())
    .await
    .unwrap();
    assert!(
        linked.is_none(),
        "unassign must sever the path→episode association"
    );
}

#[tokio::test]
async fn test_unassign_auxiliary_file_severs_link() {
    // An auxiliary sidecar is never the episode's main file, so it takes the
    // "no episode claims this path" branch — the association must still be cleared.
    let (app, state, tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Unassign Aux Show").await;

    let nfo = tmp.path().join("sidecar.nfo");
    tokio::fs::write(&nfo, b"meta").await.unwrap();
    let path_str = nfo.to_string_lossy().to_string();

    let _ = state.db.record_hash_only(&nfo, "organized").await;
    sqlx::query(
        "INSERT INTO episodes (episode_id, series_id, season, episode) VALUES ('ep-aux', ?, 1, 1)",
    )
    .bind(&series_id)
    .execute(state.db.get_pool())
    .await
    .unwrap();
    state
        .db
        .associate_auxiliary_file("ep-aux", &path_str)
        .await
        .unwrap();

    let payload = BatchDeletePayload {
        paths: vec![path_str.clone()],
    };
    let res = app
        .clone()
        .oneshot(common::post_json_request(
            &format!("/api/series/{}/files/unassign", series_id),
            &payload,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let linked: Option<String> = sqlx::query_scalar(
        "SELECT ef.episode_id FROM episode_files ef \
         JOIN file_paths fp ON fp.id = ef.file_path_id WHERE fp.file_path = ?",
    )
    .bind(&path_str)
    .fetch_optional(state.db.get_pool())
    .await
    .unwrap();
    assert!(
        linked.is_none(),
        "unassigning an auxiliary file must sever its link"
    );
}

#[tokio::test]
async fn test_batch_assign_files_series_not_found() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let payload = BatchAssignPayload {
        paths: vec!["/nonexistent/ep.mkv".to_string()],
        start_season: "01".to_string(),
        start_episode: 1,
        is_multipart: false,
    };
    let res = app
        .oneshot(common::post_json_request(
            "/api/series/nonexistent_xyz/files/batch-assign",
            &payload,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_batch_assign_files_empty_paths() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Batch Assign Empty Show").await;
    let payload = BatchAssignPayload {
        paths: vec![],
        start_season: "01".to_string(),
        start_episode: 1,
        is_multipart: false,
    };
    let res = app
        .clone()
        .oneshot(common::post_json_request(
            &format!("/api/series/{}/files/batch-assign", series_id),
            &payload,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_batch_assign_files_missing_paths_skipped() {
    // Files that don't exist are silently skipped; the endpoint still returns 200.
    let (app, _state, _tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Batch Assign Skip Show").await;
    let payload = BatchAssignPayload {
        paths: vec![
            "/nonexistent/ep01.mkv".to_string(),
            "/nonexistent/ep02.mkv".to_string(),
        ],
        start_season: "01".to_string(),
        start_episode: 1,
        is_multipart: false,
    };
    let res = app
        .clone()
        .oneshot(common::post_json_request(
            &format!("/api/series/{}/files/batch-assign", series_id),
            &payload,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_batch_assign_files_sequential() {
    let (app, _state, tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Batch Assign Sequential Show").await;

    // Create a couple of real video files that can be processed
    let ep1 = tmp
        .path()
        .join("organized")
        .join("Batch Assign Sequential Show")
        .join("ep01.mkv");
    let ep2 = tmp
        .path()
        .join("organized")
        .join("Batch Assign Sequential Show")
        .join("ep02.mkv");
    tokio::fs::create_dir_all(ep1.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(&ep1, b"video data 1").await.unwrap();
    tokio::fs::write(&ep2, b"video data 2").await.unwrap();

    let payload = BatchAssignPayload {
        paths: vec![
            ep1.to_string_lossy().to_string(),
            ep2.to_string_lossy().to_string(),
        ],
        start_season: "01".to_string(),
        start_episode: 1,
        is_multipart: false,
    };
    let res = app
        .clone()
        .oneshot(common::post_json_request(
            &format!("/api/series/{}/files/batch-assign", series_id),
            &payload,
        ))
        .await
        .unwrap();
    let status = res.status();
    if status != StatusCode::OK {
        // The handler returns the DB error string as the 500 body — surface it
        // so a transient failure (e.g. SQLite write-lock contention) is
        // diagnosable instead of a bare status mismatch.
        let body = axum::body::to_bytes(res.into_body(), 16 * 1024)
            .await
            .unwrap();
        panic!(
            "batch-assign returned {status}: {}",
            String::from_utf8_lossy(&body)
        );
    }
}

#[tokio::test]
async fn test_batch_assign_files_multipart() {
    let (app, state, tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Batch Assign Multipart Show").await;

    let part1 = tmp
        .path()
        .join("organized")
        .join("Batch Assign Multipart Show")
        .join("ep01-part1.mkv");
    let part2 = tmp
        .path()
        .join("organized")
        .join("Batch Assign Multipart Show")
        .join("ep01-part2.mkv");
    tokio::fs::create_dir_all(part1.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(&part1, b"part data 1").await.unwrap();
    tokio::fs::write(&part2, b"part data 2").await.unwrap();

    let payload = BatchAssignPayload {
        paths: vec![
            part1.to_string_lossy().to_string(),
            part2.to_string_lossy().to_string(),
        ],
        start_season: "01".to_string(),
        start_episode: 1,
        is_multipart: true, // both files are parts of one episode
    };
    let ep_id = format!("{series_id}_S01E01");
    // Create the target episode up front (no `file_path`; ownership is recorded via
    // `episode_files`). The batch-assign handler registers parts against it.
    sqlx::query(
        "INSERT INTO episodes (episode_id, series_id, season, episode) VALUES (?, ?, 1, 1)",
    )
    .bind(&ep_id)
    .bind(&series_id)
    .execute(state.db.get_pool())
    .await
    .unwrap();

    let res = app
        .clone()
        .oneshot(common::post_json_request(
            &format!("/api/series/{}/files/batch-assign", series_id),
            &payload,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let parts: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM episode_files WHERE episode_id = ? AND kind = 'main'",
    )
    .bind(&ep_id)
    .fetch_one(state.db.get_pool())
    .await
    .unwrap();
    assert_eq!(parts, 2, "both parts must be registered");

    // The multipart association lives in `episode_files` (the SSoT), and both part
    // files must be fingerprinted so the details join can resolve their content.
    let p1s = part1.to_string_lossy().to_string();
    let p2s = part2.to_string_lossy().to_string();
    let fingerprinted: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM file_paths WHERE file_path IN (?, ?) AND fingerprint IS NOT NULL",
    )
    .bind(&p1s)
    .bind(&p2s)
    .fetch_one(state.db.get_pool())
    .await
    .unwrap();
    assert_eq!(
        fingerprinted, 2,
        "both parts must be fingerprinted so their file fields resolve"
    );
}

// DELETE /api/series/:id/files

#[tokio::test]
async fn test_delete_series_files_empty_paths() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Delete Files Empty Show").await;
    let payload = BatchDeletePayload { paths: vec![] };
    let res = app
        .clone()
        .oneshot(common::delete_json_request(
            &format!("/api/series/{}/files", series_id),
            &payload,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_delete_series_files_nonexistent_path() {
    // Paths that don't exist on disk are skipped (just a warning log)
    let (app, _state, _tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Delete Files Missing Show").await;
    let payload = BatchDeletePayload {
        paths: vec!["/nonexistent/episode.mkv".to_string()],
    };
    let res = app
        .clone()
        .oneshot(common::delete_json_request(
            &format!("/api/series/{}/files", series_id),
            &payload,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_delete_series_files_existing_file() {
    let (app, _state, tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Delete Files Real Show").await;

    let file_path = tmp.path().join("to_delete.mkv");
    tokio::fs::write(&file_path, b"delete me").await.unwrap();

    let payload = BatchDeletePayload {
        paths: vec![file_path.to_string_lossy().to_string()],
    };
    let res = app
        .clone()
        .oneshot(common::delete_json_request(
            &format!("/api/series/{}/files", series_id),
            &payload,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    assert!(!file_path.exists(), "File should have been deleted");
}

#[tokio::test]
async fn test_delete_series_files_takes_episode_sidecars_with_it() {
    // Deleting an episode's video must also remove the auxiliary sidecars
    // (subtitles/nfo) attached to that episode, not just the video's own link.
    let (app, state, tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Delete Sidecars Show").await;

    let video = tmp.path().join("show/ep.mkv");
    let sub = tmp.path().join("show/ep.en.srt");
    let nfo = tmp.path().join("show/ep.nfo");
    tokio::fs::create_dir_all(video.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(&video, b"video").await.unwrap();
    tokio::fs::write(&sub, b"subs").await.unwrap();
    tokio::fs::write(&nfo, b"meta").await.unwrap();
    let video_s = video.to_string_lossy().to_string();
    let sub_s = sub.to_string_lossy().to_string();
    let nfo_s = nfo.to_string_lossy().to_string();

    sqlx::query(
        "INSERT INTO episodes (episode_id, series_id, season, episode) VALUES ('ep-side', ?, 1, 1)",
    )
    .bind(&series_id)
    .execute(state.db.get_pool())
    .await
    .unwrap();
    state
        .db
        .associate_main_file("ep-side", &video_s, None)
        .await
        .unwrap();
    state
        .db
        .associate_auxiliary_file("ep-side", &sub_s)
        .await
        .unwrap();
    state
        .db
        .associate_auxiliary_file("ep-side", &nfo_s)
        .await
        .unwrap();

    // Only the video path is requested; the sidecars must follow it.
    let payload = BatchDeletePayload {
        paths: vec![video_s.clone()],
    };
    let res = app
        .clone()
        .oneshot(common::delete_json_request(
            &format!("/api/series/{}/files", series_id),
            &payload,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    assert!(!video.exists(), "video must be deleted");
    assert!(
        !sub.exists(),
        "subtitle sidecar must be deleted with the video"
    );
    assert!(!nfo.exists(), "nfo sidecar must be deleted with the video");
}

#[tokio::test]
async fn test_delete_series_files_sidecar_only_leaves_the_video() {
    // Requesting only a sidecar must not pull the episode's video into the delete —
    // the expansion is triggered by main files only.
    let (app, state, tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Delete Sidecar Only Show").await;

    let video = tmp.path().join("only/ep.mkv");
    let sub = tmp.path().join("only/ep.en.srt");
    tokio::fs::create_dir_all(video.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(&video, b"video").await.unwrap();
    tokio::fs::write(&sub, b"subs").await.unwrap();
    let video_s = video.to_string_lossy().to_string();
    let sub_s = sub.to_string_lossy().to_string();

    sqlx::query(
        "INSERT INTO episodes (episode_id, series_id, season, episode) VALUES ('ep-sc', ?, 1, 1)",
    )
    .bind(&series_id)
    .execute(state.db.get_pool())
    .await
    .unwrap();
    state
        .db
        .associate_main_file("ep-sc", &video_s, None)
        .await
        .unwrap();
    state
        .db
        .associate_auxiliary_file("ep-sc", &sub_s)
        .await
        .unwrap();

    let payload = BatchDeletePayload {
        paths: vec![sub_s.clone()],
    };
    let res = app
        .clone()
        .oneshot(common::delete_json_request(
            &format!("/api/series/{}/files", series_id),
            &payload,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    assert!(!sub.exists(), "requested sidecar must be deleted");
    assert!(video.exists(), "the episode's video must be left intact");
}

// POST /api/series/actions/reorganize_all

#[tokio::test]
async fn test_reorganize_all_series_empty_library() {
    // No tracked series → reorganization completes with 200
    let (app, _state, _tmp) = common::setup_test_app().await;
    let res = app
        .oneshot(common::post_empty_request(
            "/api/series/actions/reorganize_all",
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_reorganize_all_async_returns_task_id() {
    // The async reorganize endpoint returns a task_id immediately; the background
    // task then completes with finished=true.
    let (app, _state, _tmp) = common::setup_test_app().await;

    let res = app
        .clone()
        .oneshot(common::post_empty_request(
            "/api/series/actions/reorganize_all_async",
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let body: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    let task_id = body["task_id"]
        .as_str()
        .expect("Response should contain task_id")
        .to_string();
    assert!(!task_id.is_empty(), "task_id should not be empty");

    // Poll the status endpoint until finished or timeout
    let max_polls: u32 = 50;
    for i in 0..max_polls {
        let status_res = app
            .clone()
            .oneshot(common::get_request(&format!(
                "/api/series/actions/reorganize_all/{}/status",
                task_id
            )))
            .await
            .unwrap();
        assert_eq!(status_res.status(), StatusCode::OK);

        let progress: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(status_res.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();

        if progress["finished"].as_bool().unwrap_or(false) {
            assert_eq!(
                progress["completed"].as_u64().unwrap_or(0),
                progress["total"].as_u64().unwrap_or(0),
                "SSoT: completed must equal total when finished"
            );
            assert_eq!(
                progress["operation_type"], "reorganize",
                "operation_type should match OperationType serde rename_all"
            );
            // finished_at is skipped in serialization, so it won't appear
            return;
        }

        if i == max_polls - 1 {
            panic!("Task did not complete within polling timeout");
        }

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

#[tokio::test]
async fn test_reorganize_all_async_with_series_polls_to_completion() {
    // With tracked series but no actual files, async reorganize still completes,
    // reporting 0 success / 0 failed via the ProgressTracker lifecycle.
    let (app, _state, _tmp) = common::setup_test_app().await;
    common::create_test_series(&app, "Async Reorg Show A").await;
    common::create_test_series(&app, "Async Reorg Show B").await;

    let res = app
        .clone()
        .oneshot(common::post_empty_request(
            "/api/series/actions/reorganize_all_async",
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let body: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    let task_id = body["task_id"]
        .as_str()
        .expect("Response should contain task_id")
        .to_string();

    // Poll until finished
    let max_polls: u32 = 50;
    for i in 0..max_polls {
        let status_res = app
            .clone()
            .oneshot(common::get_request(&format!(
                "/api/series/actions/reorganize_all/{}/status",
                task_id
            )))
            .await
            .unwrap();
        assert_eq!(status_res.status(), StatusCode::OK);

        let progress: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(status_res.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();

        if progress["finished"].as_bool().unwrap_or(false) {
            assert_eq!(
                progress["completed"].as_u64().unwrap_or(0),
                progress["total"].as_u64().unwrap_or(0)
            );
            assert_eq!(progress["total"].as_u64().unwrap_or(0), 2);
            assert_eq!(progress["success_count"].as_u64().unwrap_or(0), 0);
            assert_eq!(progress["failed"].as_u64().unwrap_or(0), 0);
            return;
        }

        if i == max_polls - 1 {
            panic!("Task did not complete within polling timeout");
        }

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

#[tokio::test]
async fn test_system_status_includes_active_operations() {
    // /api/system/status includes active operations from ProgressTracker::all_active().
    let (app, _state, _tmp) = common::setup_test_app().await;

    let res = app
        .clone()
        .oneshot(common::get_request("/api/status"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let body: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();

    // active_operations should be present (empty array is fine)
    assert!(
        body["active_operations"].is_array(),
        "active_operations should be an array"
    );
}

#[tokio::test]
async fn test_reorganize_all_series_with_tracked() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    common::create_test_series(&app, "Reorg All Show A").await;
    common::create_test_series(&app, "Reorg All Show B").await;

    let res = app
        .clone()
        .oneshot(common::post_empty_request(
            "/api/series/actions/reorganize_all",
        ))
        .await
        .unwrap();
    // No files to actually move, but the endpoint should still return 200
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_reorganize_preserves_file_contents_through_naming_chain() {
    // Coverage for a previously-untested reorganization shape: episode files whose
    // current names collide with other episodes' target names (a rename chain).
    // The executor must relocate every file without losing or duplicating content.
    let (app, state, tmp) = common::setup_test_app().await;

    // Renaming on — reorganization applies the episode template.
    {
        let mut org = state.db.get_organization_config().await.unwrap_or_default();
        org.auto_apply_renames = true;
        let _ = state.db.save_organization_config(&org).await;
        let mut cfg = state.cfg.write().await;
        cfg.organization.auto_apply_renames = true;
    }

    let series_id = common::create_test_series(&app, "Chain Show").await;
    let series_dir = tmp.path().join("organized").join("Chain Show");
    tokio::fs::create_dir_all(&series_dir).await.unwrap();

    let assign = |path: String, episode: &str| {
        let app = app.clone();
        let series_id = series_id.clone();
        let episode = episode.to_string();
        async move {
            let res = app
                .oneshot(common::post_json_request(
                    &format!("/api/series/{}/files/assign", series_id),
                    &AssignFilePayload {
                        path,
                        season: "01".to_string(),
                        episode,
                    },
                ))
                .await
                .unwrap();
            assert_eq!(res.status(), StatusCode::OK);
        }
    };

    // Assign normally to learn the canonical paths P1/P2 for S01E01/S01E02.
    let a = series_dir.join("A.mkv");
    let b = series_dir.join("B.mkv");
    tokio::fs::write(&a, b"AAAA content").await.unwrap();
    tokio::fs::write(&b, b"BBBB content").await.unwrap();
    assign(a.to_string_lossy().to_string(), "1").await;
    assign(b.to_string_lossy().to_string(), "2").await;

    // Let the background fingerprint tasks drain — while a path is still queued it
    // is excluded from the rename plan (skip_paths), which would make it empty.
    for _ in 0..100 {
        let active = state.scan_queue.active_paths_snapshot().await;
        if !active.iter().any(|p| p.starts_with(&series_dir)) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    let details: SeriesDetails = app.get_json(&format!("/api/series/{}", series_id)).await;
    let ep1 = details.episodes.iter().find(|e| e.episode == 1).unwrap();
    let ep2 = details.episodes.iter().find(|e| e.episode == 2).unwrap();
    let p1 = ep1.path.clone().expect("ep1 path");
    let p2 = ep2.path.clone().expect("ep2 path");
    let ep1_id = ep1.unique_id.clone();
    let ep2_id = ep2.unique_id.clone();

    // Rearrange so ep1's file is "loose" and ep2's file occupies P1 (the canonical
    // name ep1 wants). Moving ep1 → P1 would clobber ep2's file unless it is
    // temp-renamed first.
    let loose = std::path::Path::new(&p1).with_file_name("loose.mkv");
    let loose_str = loose.to_string_lossy().to_string();
    tokio::fs::rename(&p1, &loose).await.unwrap();
    state
        .db
        .move_fingerprint_path(&p1, &loose_str)
        .await
        .unwrap();
    tokio::fs::rename(&p2, &p1).await.unwrap();
    state.db.move_fingerprint_path(&p2, &p1).await.unwrap();

    for (id, path) in [(&ep1_id, &loose_str), (&ep2_id, &p1)] {
        state.db.associate_main_file(id, path, None).await.unwrap();
    }

    let res = app
        .clone()
        .oneshot(common::post_json_request(
            &format!("/api/series/{}/actions/reorganize", series_id),
            &jumbie_shared::types::ReorganizeSeriesPayload {
                target_absolute: false,
            },
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let details: SeriesDetails = app.get_json(&format!("/api/series/{}", series_id)).await;
    let mut contents = Vec::new();
    for episode in 1..=2 {
        let path = details
            .episodes
            .iter()
            .find(|e| e.episode == episode)
            .and_then(|e| e.path.clone())
            .unwrap_or_else(|| panic!("episode {} lost its file", episode));
        assert!(
            std::path::Path::new(&path).exists(),
            "episode {} points at a missing file: {}",
            episode,
            path
        );
        contents.push(tokio::fs::read_to_string(&path).await.unwrap());
    }
    contents.sort();
    assert_eq!(
        contents,
        vec!["AAAA content".to_string(), "BBBB content".to_string()],
        "both files' contents must survive the chain reorganization"
    );
}

// POST /api/system/organized_series/bulk

#[tokio::test]
async fn test_bulk_create_series_empty_items() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let payload = ConfirmSeriesImportRequest {
        items: vec![],
        scan_for_existing: false,
        monitor_mode: None,
        quality_profile: None,
        release_profile: None,
    };
    let res = app
        .oneshot(common::post_json_request(
            "/api/system/organized_series/bulk",
            &payload,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_bulk_create_series_unselected_items_skipped() {
    let (app, _state, tmp) = common::setup_test_app().await;
    // All items are unselected → no series should be created
    let payload = ConfirmSeriesImportRequest {
        items: vec![PreviewSeriesItem {
            path: tmp.path().join("Some Show").to_string_lossy().to_string(),
            original_folder_name: "Some Show".to_string(),
            final_title: "Some Show".to_string(),
            season_count: 1,
            episode_count: 10,
            selected: false, // ← not selected
            already_exists: false,
            all_files_in_root: false,
        }],
        scan_for_existing: false,
        monitor_mode: None,
        quality_profile: None,
        release_profile: None,
    };
    let res = app
        .clone()
        .oneshot(common::post_json_request(
            "/api/system/organized_series/bulk",
            &payload,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let list_res = app
        .clone()
        .oneshot(common::get_request("/api/series"))
        .await
        .unwrap();
    let body = axum::body::to_bytes(list_res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json.as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn test_bulk_create_series_selected_item() {
    let (app, _state, tmp) = common::setup_test_app().await;

    let series_dir = tmp.path().join("organized").join("Bulk Created Show");
    tokio::fs::create_dir_all(&series_dir).await.unwrap();

    let payload = ConfirmSeriesImportRequest {
        items: vec![PreviewSeriesItem {
            path: series_dir.to_string_lossy().to_string(),
            original_folder_name: "Bulk Created Show".to_string(),
            final_title: "Bulk Created Show".to_string(),
            season_count: 1,
            episode_count: 5,
            selected: true,
            already_exists: false,
            all_files_in_root: false,
        }],
        scan_for_existing: false,
        monitor_mode: Some(MonitorMode::All),
        quality_profile: Some("Any".to_string()),
        release_profile: Some("Any".to_string()),
    };
    let res = app
        .clone()
        .oneshot(common::post_json_request(
            "/api/system/organized_series/bulk",
            &payload,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let list_res = app
        .clone()
        .oneshot(common::get_request("/api/series"))
        .await
        .unwrap();
    let body = axum::body::to_bytes(list_res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let titles: Vec<&str> = json
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|s| s["title"].as_str())
        .collect();
    assert!(
        titles.contains(&"Bulk Created Show"),
        "Expected 'Bulk Created Show' in series list, got: {:?}",
        titles
    );
}

#[tokio::test]
async fn test_bulk_create_series_no_duplicate() {
    // Importing the same series twice should not create a duplicate
    let (app, _state, tmp) = common::setup_test_app().await;

    let series_dir = tmp.path().join("organized").join("No Dupe Show");
    tokio::fs::create_dir_all(&series_dir).await.unwrap();

    let make_payload = || ConfirmSeriesImportRequest {
        items: vec![PreviewSeriesItem {
            path: series_dir.to_string_lossy().to_string(),
            original_folder_name: "No Dupe Show".to_string(),
            final_title: "No Dupe Show".to_string(),
            season_count: 1,
            episode_count: 1,
            selected: true,
            already_exists: false,
            all_files_in_root: false,
        }],
        scan_for_existing: false,
        monitor_mode: None,
        quality_profile: None,
        release_profile: None,
    };

    // First import
    let res1 = app
        .clone()
        .oneshot(common::post_json_request(
            "/api/system/organized_series/bulk",
            &make_payload(),
        ))
        .await
        .unwrap();
    assert_eq!(res1.status(), StatusCode::OK);

    // Second import — duplicate should be silently skipped
    let res2 = app
        .clone()
        .oneshot(common::post_json_request(
            "/api/system/organized_series/bulk",
            &make_payload(),
        ))
        .await
        .unwrap();
    assert_eq!(res2.status(), StatusCode::OK);

    // Still only one entry with this title
    let list_res = app
        .clone()
        .oneshot(common::get_request("/api/series"))
        .await
        .unwrap();
    let body = axum::body::to_bytes(list_res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let count = json
        .as_array()
        .unwrap()
        .iter()
        .filter(|s| s["title"].as_str() == Some("No Dupe Show"))
        .count();
    assert_eq!(count, 1, "Series should not be duplicated");
}

// Import with SXXEXX matching (different series name in filename)
//
// When importing a new series, the scanner must match episodes by SXXEXX
// patterns in filenames even if the embedded series title in the filename
// does NOT match the folder name.  This test creates a folder called
// "My Imported Show" with episode files named as a completely different
// series ("Other.Show.Name.S01E01.mkv" and "Other.Show.Name.S01E02.mkv")
// and verifies they get imported under "My Imported Show".

#[tokio::test]
async fn test_bulk_import_with_sxxexx_matching() {
    let (app, _state, tmp) = common::setup_test_app().await;

    // Create a series directory with episode files whose filename series
    // title does NOT match the folder name.
    let series_dir = tmp.path().join("organized").join("My Imported Show");
    tokio::fs::create_dir_all(&series_dir).await.unwrap();

    // File names use a different series title but have valid S01E01/02 patterns
    let ep1 = series_dir.join("Other.Show.Name.S01E01.1080p.mkv");
    let ep2 = series_dir.join("Other.Show.Name.S01E02.1080p.mkv");
    tokio::fs::write(&ep1, b"fake video content 1")
        .await
        .unwrap();
    tokio::fs::write(&ep2, b"fake video content 2")
        .await
        .unwrap();

    // Preview: should detect 2 episodes, 1 season, all files in root
    let preview_req = jumbie_shared::types::PreviewSeriesRequest {
        path: series_dir.to_string_lossy().to_string(),
        is_bulk: false,
    };
    let preview_res = app
        .clone()
        .oneshot(common::post_json_request(
            "/api/system/organized_series/preview",
            &preview_req,
        ))
        .await
        .unwrap();
    assert_eq!(preview_res.status(), StatusCode::OK);
    let preview_body = axum::body::to_bytes(preview_res.into_body(), usize::MAX)
        .await
        .unwrap();
    let preview_items: Vec<PreviewSeriesItem> = serde_json::from_slice(&preview_body).unwrap();

    assert_eq!(preview_items.len(), 1, "Should have 1 preview item");
    assert!(
        preview_items[0].all_files_in_root,
        "Files are all in root -> all_files_in_root should be true"
    );
    assert_eq!(preview_items[0].episode_count, 2);
    assert_eq!(preview_items[0].season_count, 1);
    assert_eq!(
        preview_items[0].final_title, "My Imported Show",
        "Preview should preserve the folder name as final_title"
    );

    // Import with scan_for_existing = true so import_scan_for_series runs
    let payload = ConfirmSeriesImportRequest {
        items: vec![PreviewSeriesItem {
            path: series_dir.to_string_lossy().to_string(),
            original_folder_name: "My Imported Show".to_string(),
            final_title: "My Imported Show".to_string(),
            season_count: 1,
            episode_count: 2,
            selected: true,
            already_exists: false,
            all_files_in_root: true,
        }],
        scan_for_existing: true,
        monitor_mode: Some(MonitorMode::All),
        quality_profile: Some("Any".to_string()),
        release_profile: Some("Any".to_string()),
    };
    let res = app
        .clone()
        .oneshot(common::post_json_request(
            "/api/system/organized_series/bulk",
            &payload,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let list_res = app
        .clone()
        .oneshot(common::get_request("/api/series"))
        .await
        .unwrap();
    let list_body = axum::body::to_bytes(list_res.into_body(), usize::MAX)
        .await
        .unwrap();
    let series_list: serde_json::Value = serde_json::from_slice(&list_body).unwrap();

    let series = series_list
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["title"] == "My Imported Show")
        .expect("Series 'My Imported Show' should exist after import");
    let series_id = series["id"].as_str().unwrap();

    let details_res = app
        .clone()
        .oneshot(common::get_request(&format!("/api/series/{}", series_id)))
        .await
        .unwrap();
    let details_body = axum::body::to_bytes(details_res.into_body(), usize::MAX)
        .await
        .unwrap();
    let details: serde_json::Value = serde_json::from_slice(&details_body).unwrap();
    assert!(
        details["config"]["flatten_season_folders"]
            .as_bool()
            .unwrap_or(false),
        "flatten_season_folders should be true for flat structure"
    );

    let episodes = details["episodes"]
        .as_array()
        .expect("Series should have episodes");

    // The series has 2 episodes (S01E01, S01E02)
    let organized_eps: Vec<&serde_json::Value> = episodes
        .iter()
        .filter(|ep| ep["status"] == "organized")
        .collect();
    assert_eq!(
        organized_eps.len(),
        2,
        "Should have 2 organized episodes, got: {}",
        organized_eps.len()
    );

    // series name "Other Show Name"). episode_id format is now series_id based.
    for ep in &organized_eps {
        let uid = ep["unique_id"].as_str().unwrap_or("");
        assert!(
            uid.starts_with(series_id),
            "Episode unique_id '{}' should start with series_id '{}',
             not the filename's series name",
            uid,
            series_id
        );
    }
}

// Import with duplicate episode numbers (conflict detection)
//
// When two non-part files map to the same (season, episode) tuple, both should
// be left unassigned (no episode row created) so the user can resolve manually.

#[tokio::test]
async fn test_bulk_import_with_duplicate_episodes() {
    let (app, _state, tmp) = common::setup_test_app().await;

    let series_dir = tmp.path().join("organized").join("Duplicate Show");
    tokio::fs::create_dir_all(&series_dir).await.unwrap();

    // Two files with the same S01E01 but different quality/release groups
    let ep1 = series_dir.join("Duplicate.Show.S01E01.720p.mkv");
    let ep2 = series_dir.join("Duplicate.Show.S01E01.1080p.mkv");
    tokio::fs::write(&ep1, b"fake video 720p").await.unwrap();
    tokio::fs::write(&ep2, b"fake video 1080p").await.unwrap();

    // A third, non-conflicting episode
    let ep3 = series_dir.join("Duplicate.Show.S01E02.mkv");
    tokio::fs::write(&ep3, b"fake video episode 2")
        .await
        .unwrap();

    // Import with scan
    let payload = ConfirmSeriesImportRequest {
        items: vec![PreviewSeriesItem {
            path: series_dir.to_string_lossy().to_string(),
            original_folder_name: "Duplicate Show".to_string(),
            final_title: "Duplicate Show".to_string(),
            season_count: 1,
            episode_count: 3,
            selected: true,
            already_exists: false,
            all_files_in_root: true,
        }],
        scan_for_existing: true,
        monitor_mode: Some(MonitorMode::All),
        quality_profile: Some("Any".to_string()),
        release_profile: Some("Any".to_string()),
    };
    let res = app
        .clone()
        .oneshot(common::post_json_request(
            "/api/system/organized_series/bulk",
            &payload,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let list_res = app
        .clone()
        .oneshot(common::get_request("/api/series"))
        .await
        .unwrap();
    let list_body = axum::body::to_bytes(list_res.into_body(), usize::MAX)
        .await
        .unwrap();
    let series_list: serde_json::Value = serde_json::from_slice(&list_body).unwrap();

    let series = series_list
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["title"] == "Duplicate Show")
        .expect("Series should exist after import");
    let series_id = series["id"].as_str().unwrap();

    let details_res = app
        .clone()
        .oneshot(common::get_request(&format!("/api/series/{}", series_id)))
        .await
        .unwrap();
    let details_body = axum::body::to_bytes(details_res.into_body(), usize::MAX)
        .await
        .unwrap();
    let details: serde_json::Value = serde_json::from_slice(&details_body).unwrap();

    let episodes = details["episodes"]
        .as_array()
        .expect("Series should have episodes");

    // Only S01E02 should be organized; S01E01 has a conflict and should NOT be organized
    let mut organized: Vec<&serde_json::Value> = episodes
        .iter()
        .filter(|ep| ep["status"] == "organized")
        .collect();
    organized.sort_by_key(|ep| ep["episode"].as_i64().unwrap_or(0));

    assert_eq!(
        organized.len(),
        1,
        "Only the non-conflicting episode (S01E02) should be organized; S01E01 has a conflict"
    );
    assert_eq!(
        organized[0]["season"], "S01",
        "Organized episode should be in season S01 (DB uses SXX format)"
    );
    assert_eq!(
        organized[0]["episode"], 2,
        "Organized episode should be episode 2 (the non-conflicting one)"
    );
}

// Bulk import multiple series must isolate episode IDs
//
// Regression test: When multiple series are bulk-imported with
// scan_for_existing=true, each series must get its own unique episode IDs.
// If `series_id` is empty (due to legacy mappings or deserialization issues),
// `fmt_episode_id` produces identical episode IDs for different series,
// causing cross-series episode contamination via SQLite ON CONFLICT.

#[tokio::test]
async fn test_bulk_create_multiple_series_episode_isolation() {
    let (app, _state, tmp) = common::setup_test_app().await;
    let root = tmp.path().join("organized");

    // Create 3 series directories, each with 2 episode files
    let series_names = ["Isolation Alpha", "Isolation Beta", "Isolation Gamma"];
    for name in &series_names {
        let dir = root.join(name);
        tokio::fs::create_dir_all(&dir).await.unwrap();
        // Both series have S01E01 and S01E02 — if series_id is empty,
        // their episode IDs collide and only the last series' episodes survive.
        let ep1 = dir.join(format!("{}.S01E01.mkv", name.replace(' ', ".")));
        let ep2 = dir.join(format!("{}.S01E02.mkv", name.replace(' ', ".")));
        tokio::fs::write(&ep1, b"content alpha").await.unwrap();
        tokio::fs::write(&ep2, b"content beta").await.unwrap();
    }

    // Preview all 3 via bulk scan of parent
    let preview_req = jumbie_shared::types::PreviewSeriesRequest {
        path: root.to_string_lossy().to_string(),
        is_bulk: true,
    };
    let preview_res = app
        .clone()
        .oneshot(common::post_json_request(
            "/api/system/organized_series/preview",
            &preview_req,
        ))
        .await
        .unwrap();
    assert_eq!(preview_res.status(), StatusCode::OK);
    let preview_body = axum::body::to_bytes(preview_res.into_body(), usize::MAX)
        .await
        .unwrap();
    let mut preview_items: Vec<PreviewSeriesItem> = serde_json::from_slice(&preview_body).unwrap();

    // Mark all as selected
    for item in &mut preview_items {
        item.selected = true;
    }
    assert_eq!(
        preview_items.len(),
        3,
        "Should have 3 preview items (one per series directory)"
    );

    // Import all with scan_for_existing=true
    let payload = ConfirmSeriesImportRequest {
        items: preview_items,
        scan_for_existing: true,
        monitor_mode: Some(MonitorMode::All),
        quality_profile: Some("Any".to_string()),
        release_profile: Some("Any".to_string()),
    };
    let res = app
        .clone()
        .oneshot(common::post_json_request(
            "/api/system/organized_series/bulk",
            &payload,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // Get all series to map names → IDs
    let list_res = app
        .clone()
        .oneshot(common::get_request("/api/series"))
        .await
        .unwrap();
    let list_body = axum::body::to_bytes(list_res.into_body(), usize::MAX)
        .await
        .unwrap();
    let series_list: serde_json::Value = serde_json::from_slice(&list_body).unwrap();

    for name in &series_names {
        // Find this series' ID by matching the title
        let series = series_list
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["title"] == *name)
            .unwrap_or_else(|| panic!("Series '{}' should exist after bulk import", name));
        let series_id = series["id"].as_str().unwrap();

        let details_res = app
            .clone()
            .oneshot(common::get_request(&format!("/api/series/{}", series_id)))
            .await
            .unwrap();
        assert_eq!(
            details_res.status(),
            StatusCode::OK,
            "Series '{}' details should be accessible",
            name
        );
        let details_body = axum::body::to_bytes(details_res.into_body(), usize::MAX)
            .await
            .unwrap();
        let details: serde_json::Value = serde_json::from_slice(&details_body).unwrap();

        // SeriesDetails nests MappingRule under "config"
        let mapping_series_id = details["config"]["series_id"]
            .as_str()
            .unwrap_or("")
            .to_string();
        assert!(
            !mapping_series_id.is_empty(),
            "Series '{}' must have a non-empty series_id in its mapping — got empty string which causes episode ID collisions across all series",
            name
        );

        let episodes = details["episodes"]
            .as_array()
            .expect("Series should have episodes");
        let organized: Vec<&serde_json::Value> = episodes
            .iter()
            .filter(|ep| ep["status"] == "organized")
            .collect();
        assert_eq!(
            organized.len(),
            2,
            "Series '{}' should have 2 organized episodes, got {} (episode IDs may be colliding with other series due to empty series_id)",
            name,
            organized.len()
        );

        // This ensures no cross-series episode ID collision
        for ep in &organized {
            let uid = ep["unique_id"].as_str().unwrap_or("");
            assert!(
                uid.starts_with(&mapping_series_id),
                "Episode unique_id '{}' does not start with series_id '{}' for series '{}'. \
                 This means episode IDs are NOT isolated across series — \
                 likely caused by an empty series_id in the mapping.",
                uid,
                mapping_series_id,
                name
            );
        }
    }
}

// Legacy mapping: empty series_id must be backfilled
//
// Regression test: A mapping stored in the DB without a series_id (pre-dating
// the field) must get a UUID backfilled on load.  Without this, fmt_episode_id
// produces identical episode IDs for different series, causing SQLite ON CONFLICT
// to overwrite episodes across series boundaries.

#[tokio::test]
async fn test_legacy_empty_series_id_gets_backfilled_on_load() {
    let (_app, state, _tmp) = common::setup_test_app().await;

    // Directly insert a mapping with EMPTY series_id into the DB,
    // simulating a legacy mapping that predates the series_id field.
    let legacy_uuid = jumbie_shared::config::generate_uuid();
    let legacy_mapping = jumbie_shared::types::MappingRule {
        target_title: "Legacy Show".to_string(),
        name: "legacy_show".to_string(),
        series_id: String::new(), // empty — legacy
        settings: jumbie_shared::mapping::SeriesSettings {
            path: Some("/tmp/legacy_show".to_string()),
            ..Default::default()
        },
        ..Default::default()
    };

    // Store it in DB (as JSON, without series_id)
    state
        .db
        .upsert_series_mapping(&legacy_uuid, &legacy_mapping)
        .await
        .unwrap();

    // Load it back — ensure_series_id should backfill a UUID
    let loaded = state
        .db
        .get_series_mapping(&legacy_uuid)
        .await
        .unwrap()
        .expect("Mapping should exist");

    assert!(
        !loaded.series_id.is_empty(),
        "Legacy mapping with empty series_id must be backfilled with a UUID on load"
    );
    assert_eq!(
        loaded.series_id.len(),
        36,
        "Backfilled series_id should be a UUID (36 chars), got '{}'",
        loaded.series_id
    );
}

// Legacy mapping: episode IDs must not collide across series
//
// Integration test: Two legacy series with empty series_id produce unique
// episode IDs after backfill, proving no cross-series collision.

#[tokio::test]
async fn test_legacy_empty_series_id_episodes_dont_collide() {
    let (_app, state, tmp) = common::setup_test_app().await;
    let root = tmp.path().join("organized");

    // Create two series directories with same episode numbering
    let series_info = [
        ("Legacy Collide A", "legacy_collide_a"),
        ("Legacy Collide B", "legacy_collide_b"),
    ];
    let mut uuids = Vec::new();

    for (title, key) in &series_info {
        let dir = root.join(title);
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let ep = dir.join(format!("{}.S01E01.mkv", title.replace(' ', ".")));
        tokio::fs::write(&ep, b"content").await.unwrap();

        // Insert mapping with EMPTY series_id (legacy)
        let uuid = jumbie_shared::config::generate_uuid();
        let mapping = jumbie_shared::types::MappingRule {
            target_title: title.to_string(),
            name: key.to_string(),
            series_id: String::new(), // empty!
            settings: jumbie_shared::mapping::SeriesSettings {
                path: Some(dir.to_string_lossy().to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        state
            .db
            .upsert_series_mapping(&uuid, &mapping)
            .await
            .unwrap();
        uuids.push((uuid, title.to_string()));
    }

    // Load each mapping — ensure_series_id backfills on load
    let mut episode_ids = std::collections::HashSet::new();
    let mut collisions = Vec::new();

    for (uuid, title) in &uuids {
        let loaded = state
            .db
            .get_series_mapping(uuid)
            .await
            .unwrap()
            .expect("Mapping should exist");

        assert!(
            !loaded.series_id.is_empty(),
            "series_id for '{}' must be backfilled",
            title
        );

        // Generate episode IDs the same way import_scan_for_series does
        let ep_id = loaded.get_episode_id("1", 1, false).unwrap();

        if !episode_ids.insert(ep_id.clone()) {
            collisions.push(format!(
                "{} produced duplicate episode_id '{}'",
                title, ep_id
            ));
        }
    }

    assert!(
        collisions.is_empty(),
        "Empty series_id caused episode ID collision between series: {}",
        collisions.join("; ")
    );
}

// Import with files in season subfolders (not flat)
//
// When episode files are organized in season subdirectories instead of the root,
// the preview should detect this and set all_files_in_root = false.  The import
// should NOT enable flatten_season_folders.

#[tokio::test]
async fn test_bulk_import_with_season_folders() {
    let (app, _state, tmp) = common::setup_test_app().await;

    let series_dir = tmp.path().join("organized").join("Folder Series");
    let season_dir = series_dir.join("Season 1");
    tokio::fs::create_dir_all(&season_dir).await.unwrap();

    // Files inside a "Season 1" subfolder, not the root
    let ep1 = season_dir.join("Folder.Series.S01E01.mkv");
    let ep2 = season_dir.join("Folder.Series.S01E02.mkv");
    tokio::fs::write(&ep1, b"fake video 1").await.unwrap();
    tokio::fs::write(&ep2, b"fake video 2").await.unwrap();

    // Preview: should detect files are NOT in root
    let preview_req = jumbie_shared::types::PreviewSeriesRequest {
        path: series_dir.to_string_lossy().to_string(),
        is_bulk: false,
    };
    let preview_res = app
        .clone()
        .oneshot(common::post_json_request(
            "/api/system/organized_series/preview",
            &preview_req,
        ))
        .await
        .unwrap();
    let preview_body = axum::body::to_bytes(preview_res.into_body(), usize::MAX)
        .await
        .unwrap();
    let preview_items: Vec<PreviewSeriesItem> = serde_json::from_slice(&preview_body).unwrap();

    assert_eq!(preview_items.len(), 1);
    assert!(
        !preview_items[0].all_files_in_root,
        "Files in Season 1/ subfolder -> all_files_in_root should be false"
    );
    assert_eq!(preview_items[0].episode_count, 2);
    assert_eq!(preview_items[0].season_count, 1);

    // Import with scan
    let payload = ConfirmSeriesImportRequest {
        items: vec![PreviewSeriesItem {
            path: series_dir.to_string_lossy().to_string(),
            original_folder_name: "Folder Series".to_string(),
            final_title: "Folder Series".to_string(),
            season_count: 1,
            episode_count: 2,
            selected: true,
            already_exists: false,
            all_files_in_root: false,
        }],
        scan_for_existing: true,
        monitor_mode: Some(MonitorMode::All),
        quality_profile: Some("Any".to_string()),
        release_profile: Some("Any".to_string()),
    };
    let res = app
        .clone()
        .oneshot(common::post_json_request(
            "/api/system/organized_series/bulk",
            &payload,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let list_res = app
        .clone()
        .oneshot(common::get_request("/api/series"))
        .await
        .unwrap();
    let list_body = axum::body::to_bytes(list_res.into_body(), usize::MAX)
        .await
        .unwrap();
    let series_list: serde_json::Value = serde_json::from_slice(&list_body).unwrap();

    let series = series_list
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["title"] == "Folder Series")
        .expect("Series should exist");
    let series_id = series["id"].as_str().unwrap();

    let details_res = app
        .clone()
        .oneshot(common::get_request(&format!("/api/series/{}", series_id)))
        .await
        .unwrap();
    let details_body = axum::body::to_bytes(details_res.into_body(), usize::MAX)
        .await
        .unwrap();
    let details: serde_json::Value = serde_json::from_slice(&details_body).unwrap();
    assert!(
        !details["config"]["flatten_season_folders"]
            .as_bool()
            .unwrap_or(true),
        "flatten_season_folders should be false for nested folder structure"
    );
}

// GET /api/series/:id/files
//
// Manage Series Files lists a series' unassigned files from the durable
// `unmatched_files` table (not by walking the download directories), scoped to
// the series that produced them, so a recorded path that no longer exists on
// disk — or that belongs to another series — is not listed.

#[tokio::test]
async fn test_get_series_files_lists_tracked_unassigned_files() {
    let (app, state, tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Manage Files Show").await;

    // A real video file recorded as an unassigned (kept-for-review) fingerprint.
    let file_path = tmp.path().join("downloads").join("Show - 03.mkv");
    tokio::fs::create_dir_all(file_path.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(&file_path, b"fake video").await.unwrap();
    let path_str = file_path.to_string_lossy().to_string();

    sqlx::query(
        "INSERT INTO unmatched_files (file_path, series_id, reason) VALUES (?, ?, 'unmatched')",
    )
    .bind(&path_str)
    .bind(&series_id)
    .execute(state.db.get_pool())
    .await
    .unwrap();

    // A kept file belonging to a DIFFERENT series must not leak in.
    let other_series_id = common::create_test_series(&app, "Other Manage Files Show").await;
    let other_path = tmp.path().join("downloads").join("Other - 07.mkv");
    tokio::fs::write(&other_path, b"fake video").await.unwrap();
    let other_str = other_path.to_string_lossy().to_string();
    sqlx::query(
        "INSERT INTO unmatched_files (file_path, series_id, reason)
         VALUES (?, ?, 'unmatched')",
    )
    .bind(&other_str)
    .bind(&other_series_id)
    .execute(state.db.get_pool())
    .await
    .unwrap();

    let (status, body) = common::get_json(&app, &format!("/api/series/{}/files", series_id)).await;
    assert_eq!(status, StatusCode::OK);
    let files = body.as_array().expect("files should be an array");
    let entry = files
        .iter()
        .find(|f| f["path"] == path_str)
        .expect("tracked unassigned file should be listed");
    assert!(
        entry["assigned_id"].is_null(),
        "a kept-for-review file must be listed as unassigned"
    );
    assert!(
        !files.iter().any(|f| f["path"] == other_str),
        "unassigned files from another series must not be listed"
    );

    // Once the stored path is gone from disk it must not be listed.
    tokio::fs::remove_file(&file_path).await.unwrap();
    let (_, body) = common::get_json(&app, &format!("/api/series/{}/files", series_id)).await;
    let still_listed = body
        .as_array()
        .unwrap()
        .iter()
        .any(|f| f["path"] == path_str);
    assert!(!still_listed, "missing file must not be listed");
}
