mod common;

use axum::http::StatusCode;
use tower::ServiceExt;

// Rate-limiting middleware, exercised at the HTTP layer. Config is mutated
// directly in memory (not via the HTTP config endpoint) to avoid the auth
// dance — these tests use setup_test_app (no password).

/// Mutate the rate-limit config in place; the middleware reads `state.cfg` at
/// request time, so this bypasses the HTTP config endpoint (and its auth).
async fn enable_rate_limiting(state: &std::sync::Arc<jumbie::api::AppState>) {
    let mut config = state.cfg.write().await;
    config.security.rate_limit_enabled = true;
    config.security.rate_limit_per_minute = 2;
    config.security.rate_limit_burst = 0; // No burst — strict 2 req per min
}

#[tokio::test]
async fn test_rate_limit_config_mutation_works() {
    let (_app, state, _tmp) = common::setup_test_app().await;

    {
        let config = state.cfg.read().await;
        assert!(!config.security.rate_limit_enabled);
    }

    {
        let mut config = state.cfg.write().await;
        config.security.rate_limit_enabled = true;
        config.security.rate_limit_per_minute = 2;
        config.security.rate_limit_burst = 0;
    }

    {
        let config = state.cfg.read().await;
        assert!(config.security.rate_limit_enabled);
        assert_eq!(config.security.rate_limit_per_minute, 2);
    }
}

#[tokio::test]
async fn test_rate_limit_disabled_allows_all_requests() {
    let (app, _state, _tmp) = common::setup_test_app().await;

    for _ in 0..20 {
        let req = common::get_request("/api/public/ping");
        let res = app.clone().oneshot(req).await.unwrap();
        assert_ne!(
            res.status(),
            StatusCode::TOO_MANY_REQUESTS,
            "Rate limiting should not trigger when disabled"
        );
    }
}

#[tokio::test]
async fn test_rate_limit_returns_429_when_enabled() {
    let (app, state, _tmp) = common::setup_test_app().await;
    enable_rate_limiting(&state).await;

    // /api/status is a non-exempt path; within the 2/min limit these succeed.
    for _ in 0..2 {
        let req = common::get_request("/api/status");
        let res = app.clone().oneshot(req).await.unwrap();
        assert!(
            res.status().is_success(),
            "Request should succeed within limit, got: {}",
            res.status()
        );
    }

    let req = common::get_request("/api/status");
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(
        res.status(),
        StatusCode::TOO_MANY_REQUESTS,
        "Expected 429 after exceeding rate limit"
    );
}

#[tokio::test]
async fn test_rate_limit_exempt_paths_not_throttled() {
    let (app, state, _tmp) = common::setup_test_app().await;
    enable_rate_limiting(&state).await;

    for _ in 0..10 {
        let req = common::get_request("/api/public/theme");
        let res = app.clone().oneshot(req).await.unwrap();
        assert!(
            res.status().is_success(),
            "Exempt path should always succeed, got: {}",
            res.status()
        );
    }
}

#[tokio::test]
async fn test_rate_limit_on_api_paths() {
    let (app, state, _tmp) = common::setup_test_app().await;
    enable_rate_limiting(&state).await;

    for _ in 0..2 {
        let req = common::get_request("/api/status");
        let res = app.clone().oneshot(req).await.unwrap();
        assert!(res.status().is_success());
    }

    let req = common::get_request("/api/status");
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::TOO_MANY_REQUESTS);
}

/// The reaper evicts rate-limit windows once they have fully elapsed, so the
/// per-IP map cannot grow without bound.
#[tokio::test]
async fn test_rate_limiter_reap_evicts_elapsed_windows() {
    use axum::body::Body;
    use axum::extract::ConnectInfo;
    use axum::http::Request;
    use jumbie::middleware::rate_limit::reap;
    use std::net::{Ipv4Addr, SocketAddr};
    use std::time::{Duration, Instant};

    let (app, state, _tmp) = common::setup_test_app().await;
    enable_rate_limiting(&state).await;
    {
        let mut config = state.cfg.write().await;
        config.security.trusted_proxies = vec![Ipv4Addr::new(127, 0, 0, 1).to_string()];
    }

    // Two distinct resolved clients → two limiter entries.
    for xff in ["203.0.113.1", "203.0.113.2"] {
        let req = Request::builder()
            .uri("/api/status")
            .method("GET")
            .header("x-forwarded-for", xff)
            .extension(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 1234))))
            .body(Body::empty())
            .unwrap();
        let _ = app.clone().oneshot(req).await.unwrap();
    }

    // After a single window the entries are still needed to weight the sliding
    // estimate, so nothing is evicted yet.
    let evicted = reap(
        &state.rate_limiter,
        Instant::now() + Duration::from_secs(61),
    )
    .await;
    assert_eq!(
        evicted, 0,
        "entries survive one window for the sliding estimate"
    );

    // After two full windows they no longer contribute and are evicted.
    let evicted = reap(
        &state.rate_limiter,
        Instant::now() + Duration::from_secs(121),
    )
    .await;
    assert_eq!(evicted, 2, "both fully-elapsed entries should be evicted");
}
