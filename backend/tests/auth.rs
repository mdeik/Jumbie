use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use tower::ServiceExt;

mod common;

#[tokio::test]
async fn test_auth_disabled_pass_through() {
    let (app, _state, _temp_dir) = common::setup_test_app().await;

    // No password set in default test DB
    let req = Request::builder()
        .uri("/api/system/health")
        .method("GET")
        .body(Body::empty())
        .unwrap();

    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_auth_enabled_no_credentials() {
    let (app, _state, _temp_dir) = common::setup_authenticated_app().await;

    // With no authorization header -> 401
    let req = Request::builder()
        .uri("/api/system/health")
        .method("GET")
        .body(Body::empty())
        .unwrap();

    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_auth_enabled_invalid_credentials() {
    let (app, _state, _temp_dir) = common::setup_authenticated_app().await;

    let invalid_creds = STANDARD.encode("admin:wrongpassword");
    let req = Request::builder()
        .uri("/api/system/health")
        .method("GET")
        .header("Authorization", format!("Basic {}", invalid_creds))
        .body(Body::empty())
        .unwrap();

    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_auth_enabled_valid_credentials() {
    let (app, _state, _temp_dir) = common::setup_authenticated_app().await;

    let valid_creds = STANDARD.encode("admin:password");
    let req = Request::builder()
        .uri("/api/system/health")
        .method("GET")
        .header("Authorization", format!("Basic {}", valid_creds))
        .body(Body::empty())
        .unwrap();

    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

/// Frontend sends Basic auth with an empty username (`:password`), not `admin:password`.
/// The backend must handle both formats correctly.
#[tokio::test]
async fn test_auth_enabled_valid_credentials_empty_username() {
    let (app, _state, _temp_dir) = common::setup_authenticated_app().await;

    // Match the frontend's login format: format!(":{}", p) → STANDARD.encode(":password")
    let valid_creds = STANDARD.encode(":password");
    let req = Request::builder()
        .uri("/api/system/health")
        .method("GET")
        .header("Authorization", format!("Basic {}", valid_creds))
        .body(Body::empty())
        .unwrap();

    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_auth_enabled_invalid_then_banned() {
    let (app, _state, _temp_dir) = common::setup_authenticated_app().await;

    // First attempt -> invalid
    let invalid_creds = STANDARD.encode("admin:wrongpassword");
    let req1 = Request::builder()
        .uri("/api/system/health")
        .method("GET")
        .header("Authorization", format!("Basic {}", invalid_creds))
        .body(Body::empty())
        .unwrap();

    let res1 = app.clone().oneshot(req1).await.unwrap();
    assert_eq!(res1.status(), StatusCode::UNAUTHORIZED);

    // Default config sets max_auth_fail_count = 5. We should test failing enough times.
    // In our test environment, we might not get 127.0.0.1 without connect info.
    // So banning might be a bit tricky to test with oneshot unless we use the same setup.
    // But this covers the core logic!
}

/// Regression test: after a wrong password attempt, the correct password must still work.
/// The bug: the ban list's `or_insert` used `banned_until: None` for IPs that were only
/// being tracked (not yet banned). The ban check treated `None` as "permanently banned",
/// so a single wrong attempt would lock the IP out forever — until backend restart cleared
/// the in-memory ban list.
#[tokio::test]
async fn test_wrong_password_then_correct_password_must_work() {
    use tower::ServiceExt;

    let (app, _state, _temp_dir) = common::setup_authenticated_app().await;

    // Step 1: Wrong password → 401
    let wrong_creds = STANDARD.encode(":wrongpassword");
    let req = Request::builder()
        .uri("/api/system/health")
        .method("GET")
        .header("Authorization", format!("Basic {}", wrong_creds))
        .body(Body::empty())
        .unwrap();

    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

    // Step 2: Correct password → must succeed (no restart needed!)
    let valid_creds = STANDARD.encode(":password");
    let req = Request::builder()
        .uri("/api/system/health")
        .method("GET")
        .header("Authorization", format!("Basic {}", valid_creds))
        .body(Body::empty())
        .unwrap();

    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(
        res.status(),
        StatusCode::OK,
        "Correct password must work after a wrong attempt. \
         If this fails, the ban list is incorrectly treating tracked-but-not-banned \
         IPs as permanently banned."
    );

    // Step 3: Second correct attempt should also work
    let req = Request::builder()
        .uri("/api/system/health")
        .method("GET")
        .header("Authorization", format!("Basic {}", valid_creds))
        .body(Body::empty())
        .unwrap();

    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(
        res.status(),
        StatusCode::OK,
        "second correct attempt should also work"
    );
}

/// Round-trip test: set a password via update_config (mimicking Account Settings save),
/// then immediately log in with it using the frontend's exact credential format (`:password`).
/// This catches the exact scenario the user reported — password set via UI but not working
/// until backend restart.
#[tokio::test]
async fn test_set_password_via_update_config_then_login() {
    use common::put_json_request;
    use serde_json::json;
    use tower::ServiceExt;

    // Start with an unauthenticated app (no password set)
    let (app, state, _temp_dir) = common::setup_test_app().await;

    // Step 1: Set a password via update_config (PUT /api/config)
    let payload = json!({
        "auth": {
            "password": "new_secret"
        }
    });

    let res = app
        .clone()
        .oneshot(put_json_request("/api/config", &payload))
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        StatusCode::OK,
        "update_config with password should succeed"
    );

    // Step 2: Verify the hash is in the DB immediately (no restart needed)
    let stored_hash = state
        .db
        .get_user_password_hash("admin")
        .await
        .expect("get_user_password_hash should not error")
        .expect("password hash must exist in DB immediately after update_config");
    assert!(!stored_hash.is_empty(), "stored hash should not be empty");

    // Step 3: Login with the password using the frontend's exact format
    // Frontend sends: Basic base64(":password")
    let valid_creds = STANDARD.encode(":new_secret");
    let req = Request::builder()
        .uri("/api/system/health")
        .method("GET")
        .header("Authorization", format!("Basic {}", valid_creds))
        .body(Body::empty())
        .unwrap();

    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(
        res.status(),
        StatusCode::OK,
        "Login must work immediately after setting password (no restart required). \
         This fails if the password hash was cached, not persisted, or not readable."
    );

    // Step 4: Verify old password format also works
    let valid_creds_with_user = STANDARD.encode("admin:new_secret");
    let req = Request::builder()
        .uri("/api/system/health")
        .method("GET")
        .header("Authorization", format!("Basic {}", valid_creds_with_user))
        .body(Body::empty())
        .unwrap();

    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(
        res.status(),
        StatusCode::OK,
        "admin:new_secret should also work"
    );

    // Step 5: Wrong password must fail
    let wrong_creds = STANDARD.encode(":wrong_password");
    let req = Request::builder()
        .uri("/api/system/health")
        .method("GET")
        .header("Authorization", format!("Basic {}", wrong_creds))
        .body(Body::empty())
        .unwrap();

    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(
        res.status(),
        StatusCode::UNAUTHORIZED,
        "wrong password should be rejected"
    );
}

#[tokio::test]
async fn test_auth_bypass_local_auth() {
    let (app, state, _temp_dir) = common::setup_authenticated_app().await;

    // Enable localhost bypass
    {
        let mut config = state.cfg.write().await;
        config.auth.bypass_local_auth = true;
    }

    // Request from 127.0.0.1 without credentials -> OK
    let req = Request::builder()
        .uri("/api/system/health")
        .method("GET")
        .extension(axum::extract::ConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            1234,
        ))))
        .body(Body::empty())
        .unwrap();

    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

/// Regression test: behind a trusted proxy, a successful auth must reset the
/// fail counter for the *resolved* client IP, not the proxy's TCP peer IP.
///
/// Failures are counted against the resolved client IP. Before the fix the
/// success path reset `connect_ip` (the proxy) instead, so the real client's
/// counter never cleared and it could later be banned despite authenticating.
#[tokio::test]
async fn test_proxy_success_resets_real_client_fail_count() {
    use axum::extract::ConnectInfo;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    let (app, state, _temp_dir) = common::setup_authenticated_app().await;

    let proxy_ip = Ipv4Addr::new(127, 0, 0, 1);
    let client_ip = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 7));
    let proxy_addr = SocketAddr::from((proxy_ip, 1234));

    // Trust the loopback proxy's X-Forwarded-For, enable failure tracking, and
    // disable rate limiting so it can't interfere with the request count.
    {
        let mut config = state.cfg.write().await;
        config.security.trusted_proxies = vec![proxy_ip.to_string()];
        config.security.rate_limit_enabled = false;
        config.auth.bypass_local_auth = false;
        config.auth.max_auth_fail_count = 5;
    }

    let wrong = STANDARD.encode(":wrongpassword");
    let right = STANDARD.encode(":password");

    // Three failures as seen through the proxy
    for _ in 0..3 {
        let req = Request::builder()
            .uri("/api/system/health")
            .method("GET")
            .header("Authorization", format!("Basic {}", wrong))
            .header("x-forwarded-for", client_ip.to_string())
            .extension(ConnectInfo(proxy_addr))
            .body(Body::empty())
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    // Failures are tracked against the real client, never the proxy.
    {
        let bans = state.ban_list.lock().await;
        let entry = bans
            .get(&client_ip)
            .expect("real client IP should be tracked after failures");
        assert_eq!(entry.fail_count, 3, "three failures should be counted");
        assert!(
            !bans.contains_key(&IpAddr::V4(proxy_ip)),
            "proxy IP must not be tracked for client auth failures"
        );
    }

    // A success through the same proxy clears the real client's counter
    let req = Request::builder()
        .uri("/api/system/health")
        .method("GET")
        .header("Authorization", format!("Basic {}", right))
        .header("x-forwarded-for", client_ip.to_string())
        .extension(ConnectInfo(proxy_addr))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    {
        let bans = state.ban_list.lock().await;
        assert_eq!(
            bans.get(&client_ip).map(|e| e.fail_count),
            Some(0),
            "successful auth must reset the resolved client's fail counter"
        );
        assert!(
            !bans.contains_key(&IpAddr::V4(proxy_ip)),
            "reset must not create an entry for the proxy IP"
        );
    }
}

#[tokio::test]
async fn test_auth_bypass_subnet_whitelist() {
    let (app, state, _temp_dir) = common::setup_authenticated_app().await;

    // Enable subnet whitelist bypass
    {
        let mut config = state.cfg.write().await;
        config.auth.bypass_subnet_whitelist = true;
        config.auth.subnet_whitelist = vec!["192.168.1.0/24".to_string()];
    }

    // Request from 192.168.1.5 without credentials -> OK
    let req = Request::builder()
        .uri("/api/system/health")
        .method("GET")
        .extension(axum::extract::ConnectInfo(std::net::SocketAddr::from((
            [192, 168, 1, 5],
            1234,
        ))))
        .body(Body::empty())
        .unwrap();

    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

// Proxy-aware helper

/// Issue `GET /api/system/health` through a loopback trusted proxy with a raw
/// `Authorization` header value (or none).
async fn proxied_health_auth(
    app: &axum::Router,
    xff: &str,
    auth_header: Option<&str>,
) -> StatusCode {
    use axum::extract::ConnectInfo;
    use std::net::SocketAddr;

    let mut builder = Request::builder()
        .uri("/api/system/health")
        .method("GET")
        .header("x-forwarded-for", xff)
        .extension(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 1234))));
    if let Some(h) = auth_header {
        builder = builder.header("Authorization", h);
    }
    let req = builder.body(Body::empty()).unwrap();
    app.clone().oneshot(req).await.unwrap().status()
}

/// Issue `GET /api/system/health` through a loopback trusted proxy with optional
/// Basic credentials.
async fn proxied_health(app: &axum::Router, xff: &str, credentials: Option<&str>) -> StatusCode {
    let header = credentials.map(|c| format!("Basic {}", c));
    proxied_health_auth(app, xff, header.as_deref()).await
}

/// Regression: a client behind a trusted proxy that trips the failure threshold
/// is banned and enforced under its *resolved* IP, never the proxy's. Also guards
/// the ban-trigger precondition (previously `banned_until.is_none()` — never true
/// for a tracked entry — so runtime auto-banning silently never fired).
#[tokio::test]
async fn test_proxy_failures_ban_real_client_not_proxy() {
    use std::net::{IpAddr, Ipv4Addr};

    let (app, state, _temp_dir) = common::setup_authenticated_app().await;

    let proxy_ip = Ipv4Addr::new(127, 0, 0, 1);
    let client_ip = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 9));
    let other_client = "203.0.113.10";

    {
        let mut config = state.cfg.write().await;
        config.security.trusted_proxies = vec![proxy_ip.to_string()];
        config.security.rate_limit_enabled = false;
        config.auth.max_auth_fail_count = 3;
    }

    let wrong = STANDARD.encode(":wrongpassword");
    let right = STANDARD.encode(":password");

    // Three failures trip the threshold.
    for _ in 0..3 {
        let status = proxied_health(&app, &client_ip.to_string(), Some(&wrong)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    // Ban recorded on the resolved client, not the proxy.
    {
        let bans = state.ban_list.lock().await;
        let entry = bans.get(&client_ip).expect("client should be tracked");
        assert!(
            entry
                .banned_until
                .is_some_and(|t| t > std::time::Instant::now()),
            "client should have an active ban after hitting the threshold"
        );
        assert!(
            !bans.contains_key(&IpAddr::V4(proxy_ip)),
            "proxy IP must not be banned"
        );
    }

    // Banned client is rejected even with correct credentials.
    let status = proxied_health(&app, &client_ip.to_string(), Some(&right)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Another client behind the same proxy is unaffected.
    let status = proxied_health(&app, other_client, Some(&right)).await;
    assert_eq!(status, StatusCode::OK);
}

/// Regression: the rate limiter keys on the resolved client IP, not the proxy's
/// TCP peer IP. With `rate_limit_per_minute = 1`, `burst = 0`, the second request
/// from the same resolved IP is limited; a different XFF client must get its own
/// bucket even though both share the same proxy peer.
#[tokio::test]
async fn test_rate_limit_keys_on_resolved_client_ip() {
    use std::net::Ipv4Addr;

    let (app, state, _temp_dir) = common::setup_authenticated_app().await;

    {
        let mut config = state.cfg.write().await;
        config.security.trusted_proxies = vec![Ipv4Addr::new(127, 0, 0, 1).to_string()];
        config.security.rate_limit_enabled = true;
        config.security.rate_limit_per_minute = 1;
        config.security.rate_limit_burst = 0;
    }

    // Client A: first request passes the limiter (then 401 from auth), second is limited.
    assert_ne!(
        proxied_health(&app, "203.0.113.1", None).await,
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(
        proxied_health(&app, "203.0.113.1", None).await,
        StatusCode::TOO_MANY_REQUESTS
    );

    // Client B has an independent bucket despite sharing the proxy peer IP.
    assert_ne!(
        proxied_health(&app, "203.0.113.2", None).await,
        StatusCode::TOO_MANY_REQUESTS
    );
}

/// Escalation: with `ban_increment_enabled`, a second ban (after the first
/// expires) lasts longer and `ban_count` increments.
#[tokio::test]
async fn test_ban_escalates_with_increment_enabled() {
    use std::net::{IpAddr, Ipv4Addr};

    let (app, state, _temp_dir) = common::setup_authenticated_app().await;

    let proxy_ip = Ipv4Addr::new(127, 0, 0, 1);
    let client_ip = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 20));

    {
        let mut cfg = state.cfg.write().await;
        cfg.security.trusted_proxies = vec![proxy_ip.to_string()];
        cfg.security.rate_limit_enabled = false;
        cfg.auth.max_auth_fail_count = 1;
        cfg.auth.ban_duration_seconds = 10;
        cfg.auth.ban_increment_enabled = true;
        cfg.auth.ban_increment_factor = 2.0;
        cfg.auth.ban_increment_max_seconds = 3600;
        cfg.auth.ban_count_reset_days = 30;
    }

    let wrong = STANDARD.encode(":wrongpassword");

    // First failure → first ban (base duration, ban_count 1).
    assert_eq!(
        proxied_health(&app, &client_ip.to_string(), Some(&wrong)).await,
        StatusCode::UNAUTHORIZED
    );
    {
        let bans = state.ban_list.lock().await;
        let entry = bans.get(&client_ip).expect("client tracked");
        assert_eq!(entry.ban_count, 1, "first ban should have ban_count 1");
        let remaining = entry
            .banned_until
            .expect("active ban")
            .saturating_duration_since(std::time::Instant::now())
            .as_secs();
        assert!(
            (8..=10).contains(&remaining),
            "first ban ≈10s, got {remaining}s"
        );
    }

    // Force-expire the ban in memory (banned_at stays recent, so the forgiveness
    // window does NOT reset ban_count) to test re-banning without sleeping.
    {
        let mut bans = state.ban_list.lock().await;
        let entry = bans.get_mut(&client_ip).expect("client tracked");
        entry.banned_until = Some(std::time::Instant::now() - std::time::Duration::from_secs(1));
    }

    // Second failure after expiry → escalated ban (10 * factor = 20s, ban_count 2).
    assert_eq!(
        proxied_health(&app, &client_ip.to_string(), Some(&wrong)).await,
        StatusCode::UNAUTHORIZED
    );
    {
        let bans = state.ban_list.lock().await;
        let entry = bans.get(&client_ip).expect("client tracked");
        assert_eq!(entry.ban_count, 2, "second ban should increment ban_count");
        let remaining = entry
            .banned_until
            .expect("active ban")
            .saturating_duration_since(std::time::Instant::now())
            .as_secs();
        assert!(
            (18..=20).contains(&remaining),
            "second ban ≈20s, got {remaining}s"
        );
    }
}

/// Forgiveness: once the last ban is older than `ban_count_reset_days`, the
/// escalation counter resets — the next ban starts again at `ban_count` 1 and
/// base duration.
#[tokio::test]
async fn test_ban_count_resets_after_forgiveness_window() {
    use std::net::{IpAddr, Ipv4Addr};

    let (app, state, _temp_dir) = common::setup_authenticated_app().await;

    let proxy_ip = Ipv4Addr::new(127, 0, 0, 1);
    let client_ip = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 21));

    {
        let mut cfg = state.cfg.write().await;
        cfg.security.trusted_proxies = vec![proxy_ip.to_string()];
        cfg.security.rate_limit_enabled = false;
        cfg.auth.max_auth_fail_count = 1;
        cfg.auth.ban_duration_seconds = 10;
        cfg.auth.ban_increment_enabled = true;
        cfg.auth.ban_increment_factor = 2.0;
        cfg.auth.ban_increment_max_seconds = 3600;
        cfg.auth.ban_count_reset_days = 30;
    }

    let wrong = STANDARD.encode(":wrongpassword");

    assert_eq!(
        proxied_health(&app, &client_ip.to_string(), Some(&wrong)).await,
        StatusCode::UNAUTHORIZED
    );

    // Simulate a ban that expired AND whose ban is older than the forgiveness
    // window (31 days), with a raised ban_count to prove it gets reset.
    {
        let mut bans = state.ban_list.lock().await;
        let entry = bans.get_mut(&client_ip).expect("client tracked");
        entry.ban_count = 3;
        entry.banned_at = Some(chrono::Utc::now() - chrono::Duration::days(31));
        entry.banned_until = Some(std::time::Instant::now() - std::time::Duration::from_secs(1));
    }

    // Next failure → ban_count reset to 0 then incremented to 1 (base duration).
    assert_eq!(
        proxied_health(&app, &client_ip.to_string(), Some(&wrong)).await,
        StatusCode::UNAUTHORIZED
    );
    {
        let bans = state.ban_list.lock().await;
        let entry = bans.get(&client_ip).expect("client tracked");
        assert_eq!(
            entry.ban_count, 1,
            "forgiveness window should reset ban_count"
        );
        let remaining = entry
            .banned_until
            .expect("active ban")
            .saturating_duration_since(std::time::Instant::now())
            .as_secs();
        assert!(
            (8..=10).contains(&remaining),
            "ban should be back to base ≈10s, got {remaining}s"
        );
    }
}

/// Security: a failure to read the admin password hash must FAIL CLOSED, not be
/// treated as "no password set" (which would disable auth and open all scopes).
#[tokio::test]
async fn test_password_lookup_failure_fails_closed() {
    let (app, state, _temp_dir) = common::setup_authenticated_app().await;

    // Force the lookup to error by removing the table it reads.
    sqlx::query("DROP TABLE users")
        .execute(state.db.get_pool())
        .await
        .unwrap();

    // Unauthenticated request must be rejected, not silently allowed.
    let req = Request::builder()
        .uri("/api/system/health")
        .method("GET")
        .body(Body::empty())
        .unwrap();
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(
        res.status(),
        StatusCode::UNAUTHORIZED,
        "a password-hash lookup error must fail closed (401), not disable auth"
    );
}

/// Security: with a trusted proxy, a spoofed value at the LEFT of the
/// X-Forwarded-For chain must be ignored (the rightmost/real peer is used), so it
/// cannot spoof localhost and bypass auth.
#[tokio::test]
async fn test_spoofed_leftmost_xff_does_not_bypass_local_auth() {
    let (app, state, _temp_dir) = common::setup_authenticated_app().await;

    {
        let mut cfg = state.cfg.write().await;
        cfg.security.trusted_proxies = vec!["127.0.0.1".to_string()];
        cfg.security.rate_limit_enabled = false;
        cfg.auth.bypass_local_auth = true;
    }

    // Attacker injects 127.0.0.1 at the left; the proxy appends the real peer.
    // The rightmost (real) value wins, so the localhost bypass must NOT trigger.
    let status = proxied_health(&app, "127.0.0.1, 203.0.113.50", None).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "spoofed leftmost XFF must not grant the localhost bypass"
    );

    // Sanity: a genuine loopback peer (no XFF) still bypasses.
    let req = Request::builder()
        .uri("/api/system/health")
        .method("GET")
        .extension(axum::extract::ConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            1234,
        ))))
        .body(Body::empty())
        .unwrap();
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

/// Gap 1: the state reaper bounds the ban map — it evicts stale, non-banned
/// entries but keeps active bans and freshly-seen entries.
#[tokio::test]
async fn test_reaper_evicts_stale_ban_entries_only() {
    use jumbie::api::BanInfo;
    use jumbie::middleware::reaper::reap_once;
    use std::net::{IpAddr, Ipv4Addr};
    use std::time::{Duration, Instant};

    let (_app, state, _temp_dir) = common::setup_authenticated_app().await;

    // reset_days = 0 → retention floor of 1 day.
    {
        let mut cfg = state.cfg.write().await;
        cfg.auth.ban_count_reset_days = 0;
    }

    let stale = IpAddr::V4(Ipv4Addr::new(198, 51, 100, 1));
    let fresh = IpAddr::V4(Ipv4Addr::new(198, 51, 100, 2));
    let banned = IpAddr::V4(Ipv4Addr::new(198, 51, 100, 3));

    {
        let mut bans = state.ban_list.lock().await;
        bans.insert(
            stale,
            BanInfo {
                fail_count: 3,
                ban_count: 0,
                banned_until: Some(Instant::now() - Duration::from_secs(1)),
                banned_at: None,
                last_seen: Instant::now() - Duration::from_secs(2 * 86_400),
            },
        );
        bans.insert(
            fresh,
            BanInfo {
                fail_count: 1,
                ban_count: 0,
                banned_until: Some(Instant::now()),
                banned_at: None,
                last_seen: Instant::now(),
            },
        );
        bans.insert(
            banned,
            BanInfo {
                fail_count: 5,
                ban_count: 1,
                banned_until: Some(Instant::now() + Duration::from_secs(3600)),
                banned_at: None,
                last_seen: Instant::now() - Duration::from_secs(2 * 86_400),
            },
        );
    }

    let (bans_evicted, _) = reap_once(&state).await;
    assert_eq!(
        bans_evicted, 1,
        "only the stale non-banned entry is evicted"
    );

    let bans = state.ban_list.lock().await;
    assert!(!bans.contains_key(&stale), "stale non-banned entry evicted");
    assert!(bans.contains_key(&fresh), "fresh entry kept");
    assert!(bans.contains_key(&banned), "active ban kept");
}

/// Conjunction: trusted-proxy IP resolution + failure threshold + escalating ban
/// all cooperate, and an IP-level ban outranks valid credentials (even a valid
/// API key), while a different client is unaffected.
#[tokio::test]
async fn test_auth_settings_work_in_combination() {
    use jumbie_shared::auth::ApiScope;
    use std::net::{IpAddr, Ipv4Addr};

    let (app, state, _temp_dir) = common::setup_authenticated_app().await;

    let proxy_ip = Ipv4Addr::new(127, 0, 0, 1);
    let client = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 30));
    let other = "203.0.113.31";

    let raw = "jb_combo_test_key";
    let hash = jumbie::auth_utils::hash_api_key(raw);
    state
        .db
        .insert_api_key(
            "k_combo",
            "combo",
            &hash,
            "jb_combo_",
            ApiScope::all(),
            None,
        )
        .await
        .unwrap();
    state.auth_cache.invalidate_api_keys().await;

    {
        let mut cfg = state.cfg.write().await;
        cfg.security.trusted_proxies = vec![proxy_ip.to_string()];
        cfg.security.rate_limit_enabled = false;
        cfg.auth.max_auth_fail_count = 2;
        cfg.auth.ban_duration_seconds = 300;
        cfg.auth.ban_increment_enabled = true;
    }

    let bearer = format!("Bearer {}", raw);
    let wrong = STANDARD.encode(":wrongpassword");

    // Valid API key authenticates.
    assert_eq!(
        proxied_health_auth(&app, &client.to_string(), Some(&bearer)).await,
        StatusCode::OK
    );

    // Two wrong passwords from the same client trip the ban.
    assert_eq!(
        proxied_health(&app, &client.to_string(), Some(&wrong)).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        proxied_health(&app, &client.to_string(), Some(&wrong)).await,
        StatusCode::UNAUTHORIZED
    );

    // Banned IP outranks valid credentials (ban is checked before crypto).
    assert_eq!(
        proxied_health_auth(&app, &client.to_string(), Some(&bearer)).await,
        StatusCode::FORBIDDEN
    );

    // A different client behind the same proxy is unaffected.
    assert_eq!(
        proxied_health_auth(&app, other, Some(&bearer)).await,
        StatusCode::OK
    );
}

/// Conjunction: changing the password through the settings API invalidates the
/// auth cache immediately — the old password stops working at once and the new
/// one works.
#[tokio::test]
async fn test_password_change_invalidates_auth_cache() {
    use serde_json::json;

    let (app, _state, _temp_dir) = common::setup_authenticated_app().await;

    let old = format!("Basic {}", STANDARD.encode(":password"));
    // Prime the verified cache.
    assert_eq!(
        proxied_health_auth(&app, "203.0.113.40", Some(&old)).await,
        StatusCode::OK
    );

    // Change the password through the settings API (authenticated).
    let body = serde_json::to_string(&json!({ "auth": { "password": "new_password" } })).unwrap();
    let req = Request::builder()
        .uri("/api/config")
        .method("PUT")
        .header("content-type", "application/json")
        .header("Authorization", old.clone())
        .body(Body::from(body))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // Old password now fails immediately; the new one works.
    assert_eq!(
        proxied_health_auth(&app, "203.0.113.40", Some(&old)).await,
        StatusCode::UNAUTHORIZED
    );
    let new = format!("Basic {}", STANDARD.encode(":new_password"));
    assert_eq!(
        proxied_health_auth(&app, "203.0.113.40", Some(&new)).await,
        StatusCode::OK
    );
}

/// An expired API key is rejected even though it resolves via the cache.
#[tokio::test]
async fn test_expired_api_key_rejected() {
    use jumbie_shared::auth::ApiScope;

    let (app, state, _temp_dir) = common::setup_authenticated_app().await;

    let raw = "jb_expired_key";
    let hash = jumbie::auth_utils::hash_api_key(raw);
    let past = (chrono::Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
    state
        .db
        .insert_api_key(
            "k_exp",
            "exp",
            &hash,
            "jb_expired",
            ApiScope::all(),
            Some(&past),
        )
        .await
        .unwrap();
    state.auth_cache.invalidate_api_keys().await;

    let status = proxied_health_auth(&app, "203.0.113.41", Some(&format!("Bearer {}", raw))).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

/// Conjunction: subnet-whitelist bypass resolves the client behind a trusted
/// proxy (XFF), so an in-subnet client is bypassed while an out-of-subnet one is
/// not.
#[tokio::test]
async fn test_subnet_bypass_uses_resolved_client_behind_proxy() {
    let (app, state, _temp_dir) = common::setup_authenticated_app().await;

    {
        let mut cfg = state.cfg.write().await;
        cfg.security.trusted_proxies = vec!["127.0.0.1".to_string()];
        cfg.security.rate_limit_enabled = false;
        cfg.auth.bypass_subnet_whitelist = true;
        cfg.auth.subnet_whitelist = vec!["10.0.0.0/8".to_string()];
    }

    // Resolved client 10.1.2.3 is inside the whitelist → allowed without creds.
    assert_eq!(proxied_health(&app, "10.1.2.3", None).await, StatusCode::OK);
    // Out-of-subnet client is not bypassed.
    assert_eq!(
        proxied_health(&app, "203.0.113.60", None).await,
        StatusCode::UNAUTHORIZED
    );
}

/// XFF chain with multiple trusted proxies: the rightmost entry that is NOT a
/// trusted proxy is chosen as the client — not the spoofed leftmost value and not
/// the trusted hops.
#[tokio::test]
async fn test_multi_proxy_chain_picks_rightmost_untrusted() {
    let (app, state, _temp_dir) = common::setup_authenticated_app().await;

    {
        let mut cfg = state.cfg.write().await;
        cfg.security.trusted_proxies = vec!["127.0.0.1".to_string(), "10.0.0.1".to_string()];
        cfg.security.rate_limit_enabled = false;
        cfg.auth.max_auth_fail_count = 1;
        cfg.auth.ban_duration_seconds = 300;
    }

    let wrong = STANDARD.encode(":wrongpassword");
    // Chain (left→right): spoofed value, real client, inner trusted proxy.
    let xff = "198.51.100.9, 203.0.113.80, 10.0.0.1";

    assert_eq!(
        proxied_health(&app, xff, Some(&wrong)).await,
        StatusCode::UNAUTHORIZED
    );

    // The ban landed on the real client, not the spoofed or trusted entries.
    let bans = state.ban_list.lock().await;
    let client: std::net::IpAddr = "203.0.113.80".parse().unwrap();
    assert!(
        bans.get(&client)
            .is_some_and(|e| e.is_banned(std::time::Instant::now())),
        "real client (rightmost untrusted) should be banned"
    );
    assert!(
        !bans.contains_key(&"198.51.100.9".parse().unwrap()),
        "spoofed leftmost value must be ignored"
    );
    assert!(
        !bans.contains_key(&"10.0.0.1".parse().unwrap()),
        "trusted hop must not be used as the client"
    );
}

/// Concurrency: simultaneous failed attempts must promote to exactly ONE ban
/// (no double-escalation) and leave the IP banned.
#[tokio::test]
async fn test_concurrent_failures_ban_once() {
    use std::net::{IpAddr, Ipv4Addr};

    let (app, state, _temp_dir) = common::setup_authenticated_app().await;

    let proxy_ip = Ipv4Addr::new(127, 0, 0, 1);
    let client_ip = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 70));

    {
        let mut cfg = state.cfg.write().await;
        cfg.security.trusted_proxies = vec![proxy_ip.to_string()];
        cfg.security.rate_limit_enabled = false;
        cfg.auth.max_auth_fail_count = 3;
        cfg.auth.ban_duration_seconds = 300;
        cfg.auth.ban_increment_enabled = true;
        cfg.auth.ban_count_reset_days = 30;
    }

    let wrong = STANDARD.encode(":wrongpassword");

    // Fire many failures concurrently from the same resolved client.
    let mut handles = Vec::new();
    for _ in 0..10 {
        let app = app.clone();
        let xff = client_ip.to_string();
        let wrong = wrong.clone();
        handles.push(tokio::spawn(async move {
            proxied_health(&app, &xff, Some(&wrong)).await
        }));
    }
    for handle in handles {
        let status = handle.await.unwrap();
        // Each is either a plain rejection or a ban rejection — never a success.
        assert!(
            matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN),
            "unexpected status: {status}"
        );
    }

    let bans = state.ban_list.lock().await;
    let entry = bans.get(&client_ip).expect("client tracked");
    assert!(
        entry.is_banned(std::time::Instant::now()),
        "client should be banned after the threshold"
    );
    assert_eq!(
        entry.ban_count, 1,
        "concurrent failures must promote to exactly one ban (no double-escalation)"
    );
}
