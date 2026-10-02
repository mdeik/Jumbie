mod common;

use axum::http::StatusCode;
use jumbie_shared::types::AddBanPayload;
use tower::ServiceExt;

// GET /api/auth/bans

#[tokio::test]
async fn test_list_bans_empty() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let res = app
        .oneshot(common::get_request("/api/auth/bans"))
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

// POST /api/auth/bans

#[tokio::test]
async fn test_add_permanent_ban() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let payload = AddBanPayload {
        ip: "192.168.99.99".to_string(),
        duration_seconds: None, // permanent
    };
    let res = app
        .clone()
        .oneshot(common::post_json_request("/api/auth/bans", &payload))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);

    let res2 = app
        .oneshot(common::get_request("/api/auth/bans"))
        .await
        .unwrap();
    let body = axum::body::to_bytes(res2.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let ips: Vec<&str> = json
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["ip"].as_str())
        .collect();
    assert!(ips.contains(&"192.168.99.99"));
}

#[tokio::test]
async fn test_add_temporary_ban() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let payload = AddBanPayload {
        ip: "10.0.0.42".to_string(),
        duration_seconds: Some(3600),
    };
    let res = app
        .oneshot(common::post_json_request("/api/auth/bans", &payload))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
}

#[tokio::test]
async fn test_add_ban_invalid_ip() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let payload = AddBanPayload {
        ip: "not-an-ip".to_string(),
        duration_seconds: None,
    };
    let res = app
        .oneshot(common::post_json_request("/api/auth/bans", &payload))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

// Timestamp contract

/// Every ban timestamp on the wire must be RFC 3339 with an explicit UTC offset
/// — never zone-less. Covers `add_ban` (which now produces timestamps via the
/// shared `UtcDateTime` formatter) round-tripping through storage and `list_bans`.
#[tokio::test]
async fn test_list_bans_timestamps_are_rfc3339_utc() {
    let (app, _state, _tmp) = common::setup_test_app().await;

    // A temporary ban populates both `banned_at` and `banned_until`.
    let payload = AddBanPayload {
        ip: "203.0.113.7".to_string(),
        duration_seconds: Some(3600),
    };
    let res = app
        .clone()
        .oneshot(common::post_json_request("/api/auth/bans", &payload))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);

    let res2 = app
        .oneshot(common::get_request("/api/auth/bans"))
        .await
        .unwrap();
    let body = axum::body::to_bytes(res2.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let entry = json
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["ip"] == "203.0.113.7")
        .expect("the added ban should be listed");

    for field in ["banned_at", "banned_until"] {
        let ts = entry[field]
            .as_str()
            .unwrap_or_else(|| panic!("{field} should be a string"));
        assert!(
            ts.ends_with("+00:00") || ts.ends_with('Z'),
            "{field} must carry an explicit UTC offset, got {ts}"
        );
        jumbie_shared::datetime::parse_utc(ts)
            .unwrap_or_else(|e| panic!("{field} must be a parseable timestamp: {ts}: {e}"));
    }
}

// DELETE /api/auth/bans/:ip

#[tokio::test]
async fn test_remove_ban() {
    let (app, _state, _tmp) = common::setup_test_app().await;

    let payload = AddBanPayload {
        ip: "172.16.5.5".to_string(),
        duration_seconds: None,
    };
    app.clone()
        .oneshot(common::post_json_request("/api/auth/bans", &payload))
        .await
        .unwrap();

    let res = app
        .oneshot(common::delete_request("/api/auth/bans/172.16.5.5"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn test_remove_nonexistent_ban() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    // Removing a never-added ban is idempotent and still succeeds.
    let res = app
        .oneshot(common::delete_request("/api/auth/bans/1.2.3.4"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn test_remove_ban_invalid_ip() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let res = app
        .oneshot(common::delete_request("/api/auth/bans/not-an-ip"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

/// Regression: rehydrating bans from the DB on startup must not turn a
/// temporarily expired ban into a permanent one.
///
/// The auth middleware interprets `banned_until == None` as a *permanent* ban.
/// Expired temporary bans are still loaded (within the reset window) so their
/// `ban_count` survives for escalation — so they must be rehydrated with a PAST
/// instant, never `None`.
#[tokio::test]
async fn test_rehydrate_expired_temporary_ban_is_not_permanent() {
    use jumbie::api::router::rehydrate_bans;
    use jumbie::db::DbManager;
    use jumbie_shared::config::BannedIp;
    use std::net::IpAddr;
    use std::time::Instant;

    let tmp = tempfile::TempDir::new().unwrap();
    let db = DbManager::new(&tmp.path().join("test.db")).await.unwrap();

    let now = chrono::Utc::now();
    let banned_at = now.to_rfc3339();
    let expired = (now - chrono::Duration::hours(1)).to_rfc3339();
    let future = (now + chrono::Duration::hours(1)).to_rfc3339();

    // Expired temporary ban (within reset window): kept, but "not banned".
    db.insert_banned_ip(&BannedIp {
        ip: "10.9.0.1".to_string(),
        fail_count: 3,
        ban_count: 2,
        banned_at: banned_at.clone(),
        banned_until: Some(expired),
    })
    .await
    .unwrap();

    // Permanent ban: must stay permanent.
    db.insert_banned_ip(&BannedIp {
        ip: "10.9.0.2".to_string(),
        fail_count: 5,
        ban_count: 1,
        banned_at: banned_at.clone(),
        banned_until: None,
    })
    .await
    .unwrap();

    // Active temporary ban: must stay active (future instant).
    db.insert_banned_ip(&BannedIp {
        ip: "10.9.0.3".to_string(),
        fail_count: 1,
        ban_count: 1,
        banned_at,
        banned_until: Some(future),
    })
    .await
    .unwrap();

    let bans = rehydrate_bans(&db, 30).await;

    let expired_entry = bans
        .get(&"10.9.0.1".parse::<IpAddr>().unwrap())
        .expect("expired ban within reset window is kept for escalation");
    assert!(
        expired_entry.banned_until.is_some(),
        "expired temporary ban must NOT be rehydrated as permanent (None)"
    );
    assert!(
        expired_entry.banned_until.unwrap() < Instant::now(),
        "expired temporary ban must carry a past instant"
    );
    assert_eq!(
        expired_entry.ban_count, 2,
        "ban_count must be preserved for escalation"
    );

    let perm = bans.get(&"10.9.0.2".parse::<IpAddr>().unwrap()).unwrap();
    assert!(perm.banned_until.is_none(), "permanent ban stays permanent");

    let active = bans.get(&"10.9.0.3".parse::<IpAddr>().unwrap()).unwrap();
    assert!(
        active.banned_until.unwrap() > Instant::now(),
        "active ban stays in the future"
    );
}

/// Pruning removes long-expired temporary bans but never permanent bans or bans
/// still within their retention window.
#[tokio::test]
async fn test_prune_expired_bans_keeps_permanent_and_recent() {
    use jumbie::db::DbManager;
    use jumbie_shared::config::BannedIp;

    let tmp = tempfile::TempDir::new().unwrap();
    let db = DbManager::new(&tmp.path().join("test.db")).await.unwrap();
    let now = chrono::Utc::now();

    // Long-expired temporary ban → prune candidate.
    db.insert_banned_ip(&BannedIp {
        ip: "10.0.0.1".to_string(),
        fail_count: 1,
        ban_count: 1,
        banned_at: (now - chrono::Duration::days(10)).to_rfc3339(),
        banned_until: Some((now - chrono::Duration::days(9)).to_rfc3339()),
    })
    .await
    .unwrap();

    // Recent temporary ban → kept.
    db.insert_banned_ip(&BannedIp {
        ip: "10.0.0.2".to_string(),
        fail_count: 1,
        ban_count: 1,
        banned_at: now.to_rfc3339(),
        banned_until: Some((now + chrono::Duration::hours(1)).to_rfc3339()),
    })
    .await
    .unwrap();

    // Permanent ban → always kept.
    db.insert_banned_ip(&BannedIp {
        ip: "10.0.0.3".to_string(),
        fail_count: 1,
        ban_count: 1,
        banned_at: now.to_rfc3339(),
        banned_until: None,
    })
    .await
    .unwrap();

    let cutoff = (now - chrono::Duration::days(1)).to_rfc3339();
    let deleted = db.prune_expired_bans(&cutoff).await.unwrap();
    assert_eq!(deleted, 1, "only the long-expired temporary ban is pruned");

    let remaining: Vec<String> = db
        .get_banned_ips()
        .await
        .unwrap()
        .into_iter()
        .map(|b| b.ip)
        .collect();
    assert!(
        remaining.contains(&"10.0.0.2".to_string()),
        "recent ban kept"
    );
    assert!(
        remaining.contains(&"10.0.0.3".to_string()),
        "permanent ban kept"
    );
    assert!(
        !remaining.contains(&"10.0.0.1".to_string()),
        "expired ban pruned"
    );
}
