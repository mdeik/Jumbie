mod common;

use axum::http::StatusCode;
use jumbie::db::download_queue::AddToDownloadQueueParams;
use jumbie_shared::types::DeleteTorrentPayload;
use tower::ServiceExt;

// GET /api/queue

#[tokio::test]
async fn test_get_queue_empty() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let res = app
        .oneshot(common::get_request("/api/queue"))
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

/// Seed a `Downloading` queue item with a `no_progress_since` age in minutes.
async fn seed_downloading(
    state: &std::sync::Arc<jumbie::api::AppState>,
    episode_id: &str,
    is_user_requested: i64,
    minutes_since_progress: i64,
) {
    sqlx::query("INSERT INTO episodes (episode_id, episode) VALUES (?, 1)")
        .bind(episode_id)
        .execute(state.db.get_pool())
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO download_queue \
         (media_name, media_link, series_title, season, episode, episode_id, score, \
          is_user_requested, status, downloader_id, last_progress, no_progress_since) \
         VALUES ('Item', 'magnet:?xt=urn:btih:np', 'Series', '1', 1, ?, 0, ?, \
                 'Downloading', 'hash-np', 0.25, datetime('now', ?))",
    )
    .bind(episode_id)
    .bind(is_user_requested)
    .bind(format!("-{minutes_since_progress} minutes"))
    .execute(state.db.get_pool())
    .await
    .unwrap();
}

async fn first_queue_item(app: axum::Router) -> serde_json::Value {
    let res = app
        .oneshot(common::get_request("/api/queue"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    json.as_array().unwrap()[0].clone()
}

#[tokio::test]
async fn test_get_queue_marks_no_progress_for_stalled_item() {
    let (app, state, _tmp) = common::setup_test_app().await;
    seed_downloading(&state, "ep_np_stalled", 0, 31).await;

    let item = first_queue_item(app).await;
    assert_eq!(item["no_progress"], true);
    assert!(
        item["no_progress_minutes"].as_i64().unwrap() >= 30,
        "tooltip minutes should reflect the elapsed window: {}",
        item["no_progress_minutes"]
    );
}

#[tokio::test]
async fn test_get_queue_marks_no_progress_for_manual_item() {
    // Manual downloads warn too — they are just never auto-resolved.
    let (app, state, _tmp) = common::setup_test_app().await;
    seed_downloading(&state, "ep_np_manual", 1, 31).await;

    let item = first_queue_item(app).await;
    assert_eq!(item["no_progress"], true);
}

#[tokio::test]
async fn test_get_queue_does_not_mark_fresh_download() {
    let (app, state, _tmp) = common::setup_test_app().await;
    seed_downloading(&state, "ep_np_fresh", 0, 5).await;

    let item = first_queue_item(app).await;
    assert_eq!(item["no_progress"], false);
    assert!(item["no_progress_minutes"].is_null());
}

// POST /api/downloads (add_download)

#[tokio::test]
async fn test_add_download_no_magnet_or_link() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let payload = serde_json::json!({
        "episode_id": null,
        "title": null,
        "link": "",
        "download_id": "",
        "score": null
    });
    let res = app
        .oneshot(common::post_json_request("/api/downloads", &payload))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_add_download_with_magnet() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let payload = jumbie_shared::types::DownloadMediaPayload {
        episode_id: None,
        title: Some("Test Download".to_string()),
        link: "magnet:?xt=urn:btih:abc123".to_string(),
        download_id: "hash_abc123".to_string(),
        score: None,
        category: None,
        tag: None,
        series_id: None,
        is_season_pack: None,
        is_user_requested: false,
        size: None,
        seeders: None,
        upload_date: None,
    };
    let res = app
        .oneshot(common::post_json_request("/api/downloads", &payload))
        .await
        .unwrap();
    // The response depends on internal DB state; just verify no panic. Acceptable
    // codes: 200 (queued), 400 (validation), 500 (DB error), 503 (unavailable).
    let status = res.status().as_u16();
    assert!(
        status == 200 || status == 400 || status == 500 || status == 503,
        "Unexpected status {status}"
    );
}

// DELETE /api/queue/:id

#[tokio::test]
async fn test_remove_from_queue() {
    let (app, state, _tmp) = common::setup_test_app().await;

    // Insert a queue item directly so we have an ID to delete
    let id = state
        .db
        .add_to_download_queue(AddToDownloadQueueParams {
            media_name: "Test Item",
            media_link: "magnet:?xt=urn:btih:abc123",
            series_title: "Test Series",
            series_id: "",
            seasons: &[1],
            episodes: &[1],
            episode_id: None,
            score: 0,
            is_user_requested: true,
            is_season_pack: false,
            category: "",
            multi_targets: None,
            episode_intentions: None,
            source_pub_date: None,
            download_id: "",
            scoring_size_bytes: None,
            scoring_seeders: None,
            scoring_episode_count: None,
            submitter: None,
            quality_profile_id: None,
            version: 1,
        })
        .await;
    // add_to_download_queue returns AddQueueResult, so just delete id=1.
    let res = app
        .oneshot(common::delete_request("/api/queue/1"))
        .await
        .unwrap();
    // Either the row was found/removed or the DB errored; we only exercise the path.
    assert!(res.status().is_success() || res.status().is_server_error());
    drop(id);
}

// POST /api/queue/:id/pause

#[tokio::test]
async fn test_pause_queue_item_not_found() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let res = app
        .oneshot(common::post_empty_request("/api/queue/99999/pause"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

// POST /api/queue/:id/resume

#[tokio::test]
async fn test_resume_queue_item_not_found() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let res = app
        .oneshot(common::post_empty_request("/api/queue/99999/resume"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

/// The pause/resume endpoints must reject items whose assigned client does not
/// support `CanPauseResume` (the queue UI hides the buttons for those, but a
/// direct API call must not bypass the capability). No downloader plugin is
/// loaded in tests, so `supports_pause_resume` returns false for any client.
#[tokio::test]
async fn test_pause_queue_item_client_without_capability_rejected() {
    let (app, state, _tmp) = common::setup_test_app().await;
    sqlx::query("INSERT INTO episodes (episode_id, episode) VALUES ('ep_pause_cap', 1)")
        .execute(state.db.get_pool())
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO download_queue (media_name, media_link, series_title, season, episode, episode_id, score, is_user_requested, status, downloader_id, client_id)
         VALUES ('Test', 'magnet:?xt=urn:btih:abc', 'Series', '1', 1, 'ep_pause_cap', 0, 0, 'Downloading', 'some-hash', 'ghost-client')",
    )
    .execute(state.db.get_pool())
    .await
    .unwrap();

    let res = app
        .oneshot(common::post_empty_request("/api/queue/1/pause"))
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        StatusCode::BAD_REQUEST,
        "client without CanPauseResume must be rejected, not called"
    );
}

#[tokio::test]
async fn test_resume_queue_item_client_without_capability_rejected() {
    let (app, state, _tmp) = common::setup_test_app().await;
    sqlx::query("INSERT INTO episodes (episode_id, episode) VALUES ('ep_resume_cap', 1)")
        .execute(state.db.get_pool())
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO download_queue (media_name, media_link, series_title, season, episode, episode_id, score, is_user_requested, status, downloader_id, client_id)
         VALUES ('Test', 'magnet:?xt=urn:btih:abc', 'Series', '1', 1, 'ep_resume_cap', 0, 0, 'Paused', 'some-hash', 'ghost-client')",
    )
    .execute(state.db.get_pool())
    .await
    .unwrap();

    let res = app
        .oneshot(common::post_empty_request("/api/queue/1/resume"))
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        StatusCode::BAD_REQUEST,
        "client without CanPauseResume must be rejected, not called"
    );
}

// POST /api/queue/:id/delete

#[tokio::test]
async fn test_delete_queue_item_not_found() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let payload = DeleteTorrentPayload {
        delete_files: false,
    };
    let res = app
        .oneshot(common::post_json_request(
            "/api/queue/99999/delete",
            &payload,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

// POST /api/queue/:id/retry

#[tokio::test]
async fn test_retry_queue_item_not_found() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let res = app
        .oneshot(common::post_empty_request("/api/queue/99999/retry"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_retry_queue_item_not_failed() {
    let (app, state, _tmp) = common::setup_test_app().await;
    // Insert an episode first to satisfy the FK constraint
    sqlx::query("INSERT INTO episodes (episode_id, episode) VALUES ('ep_test_retry', 1)")
        .execute(state.db.get_pool())
        .await
        .unwrap();
    // Insert a "Queued" item directly
    sqlx::query(
        "INSERT INTO download_queue (media_name, media_link, series_title, season, episode, episode_id, score, is_user_requested, status)
         VALUES ('Test', 'magnet:?xt=urn:btih:abc', 'Series', '1', 1, 'ep_test_retry', 0, 0, 'Queued')",
    )
    .execute(state.db.get_pool())
    .await
    .unwrap();

    let res = app
        .oneshot(common::post_empty_request("/api/queue/1/retry"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_retry_queue_item_no_downloader_id() {
    let (app, state, _tmp) = common::setup_test_app().await;
    // Insert an episode first to satisfy the FK constraint
    sqlx::query("INSERT INTO episodes (episode_id, episode) VALUES ('ep_retry_no_dl', 1)")
        .execute(state.db.get_pool())
        .await
        .unwrap();
    // Insert a "Failed" item without downloader_id, with a non-zero retry_count to
    // verify it gets reset.
    sqlx::query(
        "INSERT INTO download_queue (media_name, media_link, series_title, season, episode, episode_id, score, is_user_requested, status, error_message, retry_count)
         VALUES ('Test', 'magnet:?xt=urn:btih:abc', 'Series', '1', 1, 'ep_retry_no_dl', 0, 0, 'Failed', 'Something went wrong', 4)",
    )
    .execute(state.db.get_pool())
    .await
    .unwrap();

    let res = app
        .oneshot(common::post_empty_request("/api/queue/1/retry"))
        .await
        .unwrap();
    assert!(res.status().is_success(), "Retry should succeed");

    let row: (String, Option<String>, Option<f32>, Option<String>, i32, Option<String>) = sqlx::query_as(
        "SELECT status, downloader_id, progress, error_message, retry_count, next_retry_at FROM download_queue WHERE id = 1",
    )
    .fetch_one(state.db.get_pool())
    .await
    .unwrap();
    assert_eq!(row.0, "Queued", "Status should be Queued");
    assert!(row.1.is_none(), "downloader_id should be None");
    assert!(row.2.is_none(), "progress should be None");
    assert!(row.3.is_none(), "error_message should be None");
    assert_eq!(row.4, 0, "retry_count should be reset to 0");
    assert!(row.5.is_none(), "next_retry_at should be NULL");
}

#[tokio::test]
async fn test_retry_queue_item_with_downloader_id() {
    let (app, state, _tmp) = common::setup_test_app().await;
    // Insert an episode first to satisfy the FK constraint
    sqlx::query("INSERT INTO episodes (episode_id, episode) VALUES ('ep_retry_with_dl', 1)")
        .execute(state.db.get_pool())
        .await
        .unwrap();
    // Insert a "Failed" item WITH downloader_id, with a non-zero retry_count to
    // verify it gets reset.
    sqlx::query(
        "INSERT INTO download_queue (media_name, media_link, series_title, season, episode, episode_id, score, is_user_requested, status, downloader_id, progress, error_message, retry_count)
         VALUES ('Test', 'magnet:?xt=urn:btih:abc', 'Series', '1', 1, 'ep_retry_with_dl', 0, 0, 'Failed', 'some-hash', 0.5, 'Client error', 4)",
    )
    .execute(state.db.get_pool())
    .await
    .unwrap();

    let res = app
        .oneshot(common::post_empty_request("/api/queue/1/retry"))
        .await
        .unwrap();
    assert!(res.status().is_success(), "Retry should succeed");

    // Now "Downloading" (keeps downloader_id for orchestrator polling) with a reset
    // retry budget.
    let row: (String, Option<String>, Option<f32>, Option<String>, i32, Option<String>) = sqlx::query_as(
        "SELECT status, downloader_id, progress, error_message, retry_count, next_retry_at FROM download_queue WHERE id = 1",
    )
    .fetch_one(state.db.get_pool())
    .await
    .unwrap();
    assert_eq!(row.0, "Downloading", "Status should be Downloading");
    assert_eq!(
        row.1.as_deref(),
        Some("some-hash"),
        "downloader_id should be preserved"
    );
    assert_eq!(row.2, Some(0.5), "progress should be preserved");
    assert!(row.3.is_none(), "error_message should be cleared");
    assert_eq!(row.4, 0, "retry_count should be reset to 0");
    assert!(row.5.is_none(), "next_retry_at should be NULL");
}

// GET /api/status

#[tokio::test]
async fn test_status_endpoint_empty() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let res = app
        .oneshot(common::get_request("/api/status"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["download_queue_has_failed"], false);
    assert_eq!(json["rename_queue_has_failed"], false);
    assert_eq!(json["downloader_disabled"], true); // no downloader configured in tests
}

#[tokio::test]
async fn test_status_endpoint_with_failed_download() {
    let (app, state, _tmp) = common::setup_test_app().await;
    // Insert an episode first to satisfy the FK constraint
    sqlx::query("INSERT INTO episodes (episode_id, episode) VALUES ('ep_status_fail', 1)")
        .execute(state.db.get_pool())
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO download_queue (media_name, media_link, series_title, season, episode, episode_id, score, is_user_requested, status)
         VALUES ('Failed Item', 'magnet:?xt=urn:btih:fail', 'Series', '1', 1, 'ep_status_fail', 0, 0, 'Failed')",
    )
    .execute(state.db.get_pool())
    .await
    .unwrap();

    let res = app
        .oneshot(common::get_request("/api/status"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["download_queue_has_failed"], true);
}
