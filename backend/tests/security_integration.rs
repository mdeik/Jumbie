use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use tower::ServiceExt;

mod common;

#[tokio::test]
async fn test_clickjacking_protection() {
    let (app, state, _temp_dir) = common::setup_test_app().await;

    {
        let mut config = state.cfg.write().await;
        config.security.clickjacking_protection = true;
    }

    let req = Request::builder()
        .uri("/api/system/health")
        .method("GET")
        .body(Body::empty())
        .unwrap();

    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        res.headers().get(header::X_FRAME_OPTIONS).unwrap(),
        "SAMEORIGIN"
    );
}

#[tokio::test]
async fn test_csrf_protection_headers() {
    let (app, state, _temp_dir) = common::setup_test_app().await;

    {
        let mut config = state.cfg.write().await;
        config.security.csrf_protection = true;
    }

    let req = Request::builder()
        .uri("/api/system/health")
        .method("GET")
        .body(Body::empty())
        .unwrap();

    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        res.headers().get(header::X_CONTENT_TYPE_OPTIONS).unwrap(),
        "nosniff"
    );
    assert_eq!(
        res.headers().get("referrer-policy").unwrap(),
        "strict-origin-when-cross-origin"
    );
}

#[tokio::test]
async fn test_host_header_validation() {
    let (app, state, _temp_dir) = common::setup_test_app().await;

    {
        let mut config = state.cfg.write().await;
        config.security.host_header_validation = true;
        config.security.allowed_domains = vec!["trusted.com".to_string()];
    }

    let req_ok = Request::builder()
        .uri("/api/system/health")
        .method("GET")
        .header(header::HOST, "trusted.com")
        .body(Body::empty())
        .unwrap();

    let res_ok = app.clone().oneshot(req_ok).await.unwrap();
    assert_eq!(res_ok.status(), StatusCode::OK);

    let req_bad = Request::builder()
        .uri("/api/system/health")
        .method("GET")
        .header(header::HOST, "evil.com")
        .body(Body::empty())
        .unwrap();

    let res_bad = app.oneshot(req_bad).await.unwrap();
    assert_eq!(res_bad.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_cors_restriction() {
    let (app, state, _temp_dir) = common::setup_test_app().await;

    {
        let mut config = state.cfg.write().await;
        config.security.restrict_cors = true;
        config.security.allowed_origins = vec!["https://trusted-ui.com".to_string()];
    }

    let req_ok = Request::builder()
        .uri("/api/system/health")
        .method("GET")
        .header(header::ORIGIN, "https://trusted-ui.com")
        .body(Body::empty())
        .unwrap();

    let res_ok = app.clone().oneshot(req_ok).await.unwrap();
    assert_eq!(res_ok.status(), StatusCode::OK);
    assert_eq!(
        res_ok
            .headers()
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .unwrap(),
        "https://trusted-ui.com"
    );

    // Denied origin (not in allowed_origins)
    let req_bad = Request::builder()
        .uri("/api/system/health")
        .method("GET")
        .header(header::ORIGIN, "https://evil-ui.com")
        .body(Body::empty())
        .unwrap();

    let res_bad = app.oneshot(req_bad).await.unwrap();
    assert_eq!(res_bad.status(), StatusCode::OK); // Still OK (not an error): the disallowed origin is simply omitted from the response.
    assert!(
        res_bad
            .headers()
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .is_none()
    );
}
