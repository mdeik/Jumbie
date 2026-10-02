//! Security attack-vector integration tests: the API must reject SQL injection
//! attempts, path traversal, oversized payloads, null bytes / control chars, XSS,
//! and numeric overflow. A regression suite — a future refactor that introduces
//! a vulnerability should fail here.

mod common;

use axum::http::StatusCode;
use tower::ServiceExt;

// SQL injection: send meta-characters and keywords in input fields and verify
// parameterized queries prevent injection — a vulnerable endpoint would return
// 500 (SQL error) rather than 400/200/404.

#[tokio::test]
async fn test_sql_injection_series_name() {
    let (app, _state, _tmp) = common::setup_test_app().await;

    let payload = serde_json::json!({
        "path": _tmp.path().join("organized/test_series").to_string_lossy().to_string(),
        "series_name": "'; DROP TABLE episodes; --",
        "scan_for_existing": false
    });

    let res = app
        .clone()
        .oneshot(common::post_json_request("/api/series", &payload))
        .await
        .unwrap();
    // `validate_title` allows `'` but parameterized queries keep the DB safe;
    // the point is that the app does not crash or return 500.
    assert_ne!(
        res.status(),
        StatusCode::INTERNAL_SERVER_ERROR,
        "SQL injection attempt caused 500 — possible injection vulnerability"
    );
}

#[tokio::test]
async fn test_sql_injection_search_query() {
    let (app, _state, _tmp) = common::setup_test_app().await;

    let payload = serde_json::json!({
        "query": "1' OR '1'='1",
        "mode": null,
        "series_id": null,
        "episode_id": null
    });

    let res = app
        .clone()
        .oneshot(common::post_json_request("/api/search", &payload))
        .await
        .unwrap();
    assert_ne!(
        res.status(),
        StatusCode::INTERNAL_SERVER_ERROR,
        "SQL injection in search query caused 500"
    );
}

#[tokio::test]
async fn test_sql_injection_quality_name() {
    let (app, _state, _tmp) = common::setup_test_app().await;

    let mut qualities = std::collections::HashMap::new();
    qualities.insert(
        "test_id".to_string(),
        jumbie_shared::types::Quality {
            name: "'; DROP TABLE qualities; --".to_string(),
            tags: vec![],
        },
    );

    let res = app
        .clone()
        .oneshot(common::put_json_request(
            "/api/config/qualities",
            &qualities,
        ))
        .await
        .unwrap();
    assert_ne!(
        res.status(),
        StatusCode::INTERNAL_SERVER_ERROR,
        "SQL injection in quality name caused 500"
    );
}

#[tokio::test]
async fn test_sql_injection_path_parameter() {
    let (app, _state, _tmp) = common::setup_test_app().await;

    // Raw semicolons are rejected by the HTTP URI parser, so smuggle SQL keywords
    // through a UUID-like path parameter; it must 404, not 500.
    let res = app
        .clone()
        .oneshot(common::get_request("/api/series/DROP-TABLE-episodes"))
        .await
        .unwrap();
    assert!(
        res.status() != StatusCode::INTERNAL_SERVER_ERROR,
        "SQL injection in path parameter caused 500"
    );
}

// Path traversal: attempts must be rejected with 400 before reaching the filesystem.

#[tokio::test]
async fn test_path_traversal_series_path() {
    let (app, _state, _tmp) = common::setup_test_app().await;

    let payload = serde_json::json!({
        "path": "../../etc/passwd",
        "series_name": "Test Series",
        "scan_for_existing": false
    });

    let res = app
        .clone()
        .oneshot(common::post_json_request("/api/series", &payload))
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        StatusCode::BAD_REQUEST,
        "Path traversal in series path should be rejected with 400"
    );
}

#[tokio::test]
async fn test_path_traversal_double_dot() {
    let (app, _state, _tmp) = common::setup_test_app().await;

    let payload = serde_json::json!({
        "path": "..\\..\\..\\etc\\passwd",
        "series_name": "Test Series",
        "scan_for_existing": false
    });

    let res = app
        .clone()
        .oneshot(common::post_json_request("/api/series", &payload))
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        StatusCode::BAD_REQUEST,
        "Backslash path traversal should be rejected with 400"
    );
}

#[tokio::test]
async fn test_path_traversal_encoded() {
    let (app, _state, _tmp) = common::setup_test_app().await;

    // Contains no "..", but an absolute path outside the destination root must
    // still be rejected by validate_path.
    let payload = serde_json::json!({
        "path": "/etc/passwd",
        "series_name": "Test Series",
        "scan_for_existing": false
    });

    let res = app
        .clone()
        .oneshot(common::post_json_request("/api/series", &payload))
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        StatusCode::BAD_REQUEST,
        "Absolute path escaping destination root should be rejected with 400"
    );
}

// XSS / HTML injection: input fields shown in the UI must escape HTML/JS content;
// the backend must not crash or store unescaped script content.

#[tokio::test]
async fn test_xss_series_name() {
    let (app, _state, _tmp) = common::setup_test_app().await;

    let payload = serde_json::json!({
        "path": _tmp.path().join("organized/test_xss").to_string_lossy().to_string(),
        "series_name": "<script>alert('XSS')</script>",
        "scan_for_existing": false
    });

    let res = app
        .clone()
        .oneshot(common::post_json_request("/api/series", &payload))
        .await
        .unwrap();
    // validate_title rejects control characters but allows angle brackets; the
    // point is that there is no 500.
    assert_ne!(
        res.status(),
        StatusCode::INTERNAL_SERVER_ERROR,
        "XSS in series name caused 500"
    );
}

// Overflow / boundary values must be rejected early rather than panic, OOM, or wrap.

#[tokio::test]
async fn test_overflow_season_number() {
    let (app, _state, _tmp) = common::setup_test_app().await;

    let res = app
        .clone()
        .oneshot(common::delete_request(
            "/api/series/test_series/season/99999",
        ))
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        StatusCode::BAD_REQUEST,
        "Overflow season number should be rejected with 400"
    );
}

#[tokio::test]
async fn test_overflow_download_score_negative() {
    let (app, _state, _tmp) = common::setup_test_app().await;

    let payload = serde_json::json!({
        "link": "magnet:?xt=urn:btih:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "download_id": "test_hash_negative_score",
        "score": -100,
        "title": "Test",
        "episode_id": "test_ep"
    });

    let res = app
        .clone()
        .oneshot(common::post_json_request("/api/downloads", &payload))
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        StatusCode::BAD_REQUEST,
        "Negative download score should be rejected"
    );
}

// Null bytes / control characters can break C-bindings, filesystem ops, and log
// parsers, so they must be rejected.

#[tokio::test]
async fn test_null_byte_in_series_name() {
    let (app, _state, _tmp) = common::setup_test_app().await;

    let payload = serde_json::json!({
        "path": _tmp.path().join("organized/null_test").to_string_lossy().to_string(),
        "series_name": "Test\x00Series",
        "scan_for_existing": false
    });

    let res = app
        .clone()
        .oneshot(common::post_json_request("/api/series", &payload))
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        StatusCode::BAD_REQUEST,
        "Null byte in series name should be rejected"
    );
}

// Oversized payloads must be rejected by length validation, not silently stored.

#[tokio::test]
async fn test_excessive_search_query() {
    let (app, _state, _tmp) = common::setup_test_app().await;

    let long_query = "a".repeat(300);
    let payload = serde_json::json!({
        "query": long_query,
        "mode": null,
        "series_id": null,
        "episode_id": null
    });

    let res = app
        .clone()
        .oneshot(common::post_json_request("/api/search", &payload))
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        StatusCode::BAD_REQUEST,
        "Excessive search query length should be rejected"
    );
}

#[tokio::test]
async fn test_excessive_quality_name() {
    let (app, _state, _tmp) = common::setup_test_app().await;

    let long_name = "a".repeat(101);
    let mut qualities = std::collections::HashMap::new();
    qualities.insert(
        "test_id".to_string(),
        jumbie_shared::types::Quality {
            name: long_name,
            tags: vec![],
        },
    );

    let res = app
        .clone()
        .oneshot(common::put_json_request(
            "/api/config/qualities",
            &qualities,
        ))
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        StatusCode::BAD_REQUEST,
        "Excessive quality name length should be rejected"
    );
}

#[tokio::test]
async fn test_excessive_quality_tag() {
    let (app, _state, _tmp) = common::setup_test_app().await;

    let long_tag = "a".repeat(51);
    let mut qualities = std::collections::HashMap::new();
    qualities.insert(
        "test_id".to_string(),
        jumbie_shared::types::Quality {
            name: "Test Quality".to_string(),
            tags: vec![long_tag],
        },
    );

    let res = app
        .clone()
        .oneshot(common::put_json_request(
            "/api/config/qualities",
            &qualities,
        ))
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        StatusCode::BAD_REQUEST,
        "Excessive quality tag length should be rejected"
    );
}

// Required fields must be explicitly rejected rather than silently defaulted.

#[tokio::test]
async fn test_empty_batch_edit_series_ids() {
    let (app, _state, _tmp) = common::setup_test_app().await;

    let payload = serde_json::json!({
        "series_ids": [],
        "quality_profile": null,
        "monitor_mode": null
    });

    let res = app
        .clone()
        .oneshot(common::post_json_request(
            "/api/series/batch-edit",
            &payload,
        ))
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        StatusCode::BAD_REQUEST,
        "Empty series_ids in batch edit should be rejected"
    );
}

#[tokio::test]
async fn test_empty_batch_delete_series_ids() {
    let (app, _state, _tmp) = common::setup_test_app().await;

    let payload = serde_json::json!({
        "series_ids": []
    });

    let res = app
        .clone()
        .oneshot(common::post_json_request(
            "/api/series/batch-delete",
            &payload,
        ))
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        StatusCode::BAD_REQUEST,
        "Empty series_ids in batch delete should be rejected"
    );
}

// Authentication bypass attempts

#[tokio::test]
async fn test_auth_bypass_with_empty_password() {
    let (app, state, _tmp) = common::setup_test_app().await;

    // Set a password
    let hash = jumbie::auth_utils::hash_password("secure_password").unwrap();
    state
        .db
        .set_user_password_hash("admin", &hash)
        .await
        .unwrap();

    // Attempt to access protected endpoint without auth
    let res = app
        .clone()
        .oneshot(common::get_request("/api/config"))
        .await
        .unwrap();
    assert!(
        res.status().is_client_error(),
        "Unauthenticated config access should be rejected, got {}",
        res.status()
    );
}
