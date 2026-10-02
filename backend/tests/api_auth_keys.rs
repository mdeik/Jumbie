mod common;

use axum::http::Request;
use axum::http::StatusCode;
use chrono::Duration;
use common::{get_request, post_json_request, setup_authenticated_app};
use jumbie_shared::config::Config;
use tower::ServiceExt;

fn add_auth(mut req: Request<axum::body::Body>) -> Request<axum::body::Body> {
    req.headers_mut().insert(
        axum::http::header::AUTHORIZATION,
        "Basic YWRtaW46cGFzc3dvcmQ=".parse().unwrap(),
    );
    req
}

#[tokio::test]
async fn test_api_key_generation_and_scopes() {
    let (app, _state, _temp_dir) = setup_authenticated_app().await;

    // 1. Try to generate key with NO scopes (should fail)
    let payload = serde_json::json!({
        "name": "Test Key No Scopes",
        "scopes": [],
        "duration_days": 7
    });
    let req = add_auth(post_json_request(
        "/api/config/auth/api_keys/generate",
        &payload,
    ));
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);

    let payload = serde_json::json!({
        "name": "Test Key With Scopes",
        "scopes": ["series:read", "series:write"],
        "duration_days": 30
    });
    let req = add_auth(post_json_request(
        "/api/config/auth/api_keys/generate",
        &payload,
    ));
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let resp: serde_json::Value = serde_json::from_slice(&body).unwrap();

    let key = resp["key"].as_str().unwrap();
    let id = resp["id"].as_str().unwrap();
    let prefix = resp["prefix"].as_str().unwrap();

    assert!(key.starts_with("jb_"));
    assert!(prefix.starts_with("jb_"));
    assert_eq!(prefix.len(), 11); // jb_ + 8 chars

    let req = Request::builder()
        .method("GET")
        .uri("/api/series")
        .header("Authorization", format!("Bearer {}", key))
        .body(axum::body::Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let req = Request::builder()
        .method("GET")
        .uri("/api/config")
        .header("Authorization", format!("Bearer {}", key))
        .body(axum::body::Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    // The 403 names the required scope (and what the key is missing) via the
    // RFC 6750 header and the JSON envelope; the granted scopes are not echoed.
    assert_eq!(
        res.headers()
            .get(axum::http::header::WWW_AUTHENTICATE)
            .and_then(|v| v.to_str().ok()),
        Some("Bearer error=\"insufficient_scope\", scope=\"config:read\"")
    );
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let err: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(err["required_scopes"], serde_json::json!(["config:read"]));
    assert_eq!(err["missing_scopes"], serde_json::json!(["config:read"]));

    let req = add_auth(get_request("/api/config"));
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let config: Config = serde_json::from_slice(&body).unwrap();

    let found_key = config
        .auth
        .api_keys
        .iter()
        .find(|k| k.id == id)
        .expect("Key not found in config");
    assert_eq!(found_key.key, "********");
    assert_eq!(found_key.prefix, prefix);

    let payload = serde_json::json!({
        "name": "Admin Key",
        "scopes": ["config:read"],
        "duration_days": null
    });
    let req = add_auth(post_json_request(
        "/api/config/auth/api_keys/generate",
        &payload,
    ));
    let res = app.clone().oneshot(req).await.unwrap();
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let admin_resp: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let admin_key = admin_resp["key"].as_str().unwrap();

    let req = Request::builder()
        .method("GET")
        .uri("/api/config")
        .header("Authorization", format!("Bearer {}", admin_key))
        .body(axum::body::Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_calendar_token_generation() {
    let (app, _state, _temp_dir) = setup_authenticated_app().await;

    let payload = serde_json::json!({
        "name": "My Calendar"
    });
    let req = add_auth(post_json_request(
        "/api/config/auth/calendar_tokens/generate",
        &payload,
    ));
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let resp: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let token = resp["token"].as_str().unwrap();

    assert!(token.starts_with("cal_"));

    let req = get_request(&format!("/api/calendar/ical?token={}", token));
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        res.headers()["content-type"],
        "text/calendar; charset=utf-8"
    );

    let req = add_auth(get_request("/api/config"));
    let res = app.clone().oneshot(req).await.unwrap();
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let config: Config = serde_json::from_slice(&body).unwrap();

    let found_token = config
        .auth
        .calendar_tokens
        .iter()
        .find(|t| t.name == "My Calendar")
        .expect("Token not found in config");
    // The token value should be preserved (not masked) — the frontend uses it for URL building
    assert_eq!(
        found_token.token.as_str(),
        token,
        "Calendar token must be preserved in config response"
    );
}

#[tokio::test]
async fn test_calendar_token_with_flags() {
    let (app, _state, _temp_dir) = setup_authenticated_app().await;

    // Create token with hide_unmonitored enabled
    let payload = serde_json::json!({
        "name": "My Calendar With Flags",
        "hide_unmonitored": true,
        "show_as_all_day": true
    });
    let req = add_auth(post_json_request(
        "/api/config/auth/calendar_tokens/generate",
        &payload,
    ));
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let resp: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(resp["hide_unmonitored"], true);
    assert_eq!(resp["show_as_all_day"], true);

    let token = resp["token"].as_str().unwrap().to_string();

    // Create token with flags disabled (defaults)
    let payload = serde_json::json!({
        "name": "My Calendar Defaults"
    });
    let req = add_auth(post_json_request(
        "/api/config/auth/calendar_tokens/generate",
        &payload,
    ));
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let resp: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(resp["hide_unmonitored"], false);
    assert_eq!(resp["show_as_all_day"], false);
    let defaults_token = resp["token"].as_str().unwrap().to_string();

    let req = add_auth(get_request("/api/config"));
    let res = app.clone().oneshot(req).await.unwrap();
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let config: Config = serde_json::from_slice(&body).unwrap();

    let flagged = config
        .auth
        .calendar_tokens
        .iter()
        .find(|t| t.name == "My Calendar With Flags")
        .expect("Flagged token not found in config");
    assert_eq!(
        flagged.token.as_str(),
        token.as_str(),
        "Calendar token must be preserved in config"
    );
    assert!(flagged.hide_unmonitored, "hide_unmonitored should be true");
    assert!(flagged.show_as_all_day, "show_as_all_day should be true");

    let defaults = config
        .auth
        .calendar_tokens
        .iter()
        .find(|t| t.name == "My Calendar Defaults")
        .expect("Defaults token not found in config");
    assert_eq!(
        defaults.token.as_str(),
        defaults_token.as_str(),
        "Calendar token must be preserved in config"
    );
    assert!(
        !defaults.hide_unmonitored,
        "hide_unmonitored should default to false"
    );
    assert!(
        !defaults.show_as_all_day,
        "show_as_all_day should default to false"
    );
}

#[tokio::test]
async fn test_calendar_ical_hide_unmonitored() {
    let (app, state, _temp_dir) = setup_authenticated_app().await;

    // Insert test episodes — one monitored, one unmonitored
    let now = chrono::Utc::now().naive_utc();
    let tomorrow = now + Duration::days(1);
    let next_week = now + Duration::days(7);

    let pool = state.db.get_pool();

    // Insert monitored episode
    sqlx::query(
        "INSERT INTO episodes (episode_id, season, episode, title, meta_date, monitored, status)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind("ep-monitored-1")
    .bind("1")
    .bind(1)
    .bind("Monitored Episode")
    .bind(tomorrow)
    .bind(true)
    .bind("unreleased")
    .execute(pool)
    .await
    .unwrap();

    // Insert unmonitored episode
    sqlx::query(
        "INSERT INTO episodes (episode_id, season, episode, title, meta_date, monitored, status)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind("ep-unmonitored-1")
    .bind("1")
    .bind(2)
    .bind("Unmonitored Episode")
    .bind(next_week)
    .bind(false)
    .bind("unreleased")
    .execute(pool)
    .await
    .unwrap();

    // Create a token WITH hide_unmonitored = true (exclude unmonitored)
    let payload = serde_json::json!({
        "name": "Monitored Only",
        "hide_unmonitored": true,
        "show_as_all_day": false
    });
    let req = add_auth(post_json_request(
        "/api/config/auth/calendar_tokens/generate",
        &payload,
    ));
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let resp: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let token_monitored_only = resp["token"].as_str().unwrap().to_string();

    // Create a token WITHOUT hide_unmonitored (default false, include all)
    let payload = serde_json::json!({
        "name": "Show All",
        "hide_unmonitored": false,
        "show_as_all_day": false
    });
    let req = add_auth(post_json_request(
        "/api/config/auth/calendar_tokens/generate",
        &payload,
    ));
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let resp: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let token_show_all = resp["token"].as_str().unwrap().to_string();

    // Fetch iCal with monitored-only token — should contain only 1 event
    let req = get_request(&format!(
        "/api/calendar/ical?token={}",
        token_monitored_only
    ));
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let ical = String::from_utf8(body.to_vec()).unwrap();

    // Count VEVENT occurrences
    let monitored_count = ical.matches("BEGIN:VEVENT").count();
    assert_eq!(
        monitored_count, 1,
        "Monitored-only token should yield 1 event, got {}",
        monitored_count
    );
    assert!(
        ical.contains("Monitored Episode"),
        "Should contain the monitored episode"
    );
    assert!(
        !ical.contains("Unmonitored Episode"),
        "Should NOT contain the unmonitored episode"
    );

    // Fetch iCal with show-all token — should contain 2 events
    let req = get_request(&format!("/api/calendar/ical?token={}", token_show_all));
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let ical = String::from_utf8(body.to_vec()).unwrap();

    let all_count = ical.matches("BEGIN:VEVENT").count();
    assert_eq!(
        all_count, 2,
        "Show-all token should yield 2 events, got {}",
        all_count
    );
    assert!(
        ical.contains("Monitored Episode"),
        "Should contain the monitored episode"
    );
    assert!(
        ical.contains("Unmonitored Episode"),
        "Should contain the unmonitored episode"
    );
}

#[tokio::test]
async fn test_calendar_ical_show_as_all_day() {
    let (app, state, _temp_dir) = setup_authenticated_app().await;

    let now = chrono::Utc::now().naive_utc();
    let tomorrow = now + Duration::days(1);

    // Insert a test episode
    sqlx::query(
        "INSERT INTO episodes (episode_id, season, episode, title, meta_date, monitored, status)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind("ep-allday-test")
    .bind("1")
    .bind(5)
    .bind("All-Day Test Episode")
    .bind(tomorrow)
    .bind(true)
    .bind("unreleased")
    .execute(state.db.get_pool())
    .await
    .unwrap();

    // Create a token with show_as_all_day = false (time-specific)
    let payload = serde_json::json!({
        "name": "Time Specific",
        "hide_unmonitored": true,
        "show_as_all_day": false
    });
    let req = add_auth(post_json_request(
        "/api/config/auth/calendar_tokens/generate",
        &payload,
    ));
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let resp: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let token_time = resp["token"].as_str().unwrap().to_string();

    // Create a token with show_as_all_day = true
    let payload = serde_json::json!({
        "name": "All Day",
        "hide_unmonitored": true,
        "show_as_all_day": true
    });
    let req = add_auth(post_json_request(
        "/api/config/auth/calendar_tokens/generate",
        &payload,
    ));
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let resp: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let token_allday = resp["token"].as_str().unwrap().to_string();

    // Fetch iCal with time-specific token — DTSTART should contain 'T' (date-time format)
    let req = get_request(&format!("/api/calendar/ical?token={}", token_time));
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let ical_time = String::from_utf8(body.to_vec()).unwrap();

    // Time-specific DTSTART should be datetime format: 20250101T120000Z
    assert!(ical_time.contains("DTSTART:"), "Should have DTSTART");
    // Extract DTSTART line (may be "DTSTART:..." or "DTSTART;VALUE=DATE:...")
    let dtstart_time_line: Vec<&str> = ical_time
        .lines()
        .filter(|l| l.starts_with("DTSTART"))
        .collect();
    assert!(
        !dtstart_time_line.is_empty(),
        "Time-specific iCal should have DTSTART"
    );
    let time_val = dtstart_time_line[0].split(':').next_back().unwrap_or("");
    assert!(
        time_val.contains('T'),
        "Time-specific DTSTART value should contain 'T': {}",
        time_val
    );

    // Fetch iCal with all-day token — DTSTART should NOT contain 'T' (date-only format)
    let req = get_request(&format!("/api/calendar/ical?token={}", token_allday));
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let ical_allday = String::from_utf8(body.to_vec()).unwrap();

    // Find DTSTART line — may be "DTSTART;VALUE=DATE:..." for date-only
    let dtstart_allday_line: Vec<&str> = ical_allday
        .lines()
        .filter(|l| l.starts_with("DTSTART"))
        .collect();
    assert!(
        !dtstart_allday_line.is_empty(),
        "All-day iCal should have DTSTART"
    );
    // The value should be a pure date format (after "DTSTART:" or "DTSTART;VALUE=DATE:")
    let allday_val = dtstart_allday_line[0].split(':').next_back().unwrap_or("");
    assert!(
        !allday_val.contains('T'),
        "All-day DTSTART value should NOT contain 'T': {}",
        allday_val
    );

    // Also verify DTEND matches DTSTART for all-day (same date)
    // DTEND may be "DTEND:..." or "DTEND;VALUE=DATE:..."
    let dtend_allday_line: Vec<&str> = ical_allday
        .lines()
        .filter(|l| l.starts_with("DTEND"))
        .collect();
    assert!(
        !dtend_allday_line.is_empty(),
        "All-day iCal should have DTEND"
    );
    let allday_start_val = dtstart_allday_line[0]
        .strip_prefix("DTSTART:")
        .unwrap_or("");
    let allday_end_val = dtend_allday_line[0].strip_prefix("DTEND:").unwrap_or("");
    assert_eq!(
        allday_start_val, allday_end_val,
        "All-day DTSTART ({}) and DTEND ({}) should be the same date",
        allday_start_val, allday_end_val
    );
}

#[tokio::test]
async fn test_calendar_ical_media_info_duration_prioritized() {
    let (app, state, _temp_dir) = setup_authenticated_app().await;

    let now = chrono::Utc::now().naive_utc();
    let tomorrow = now + Duration::days(1);
    let file_path = "/tmp/test_media_info_ep.mkv";

    // Insert an episode with a runtime of 60 min and a file_path
    // so the file join has something to match on.
    sqlx::query(
        "INSERT INTO episodes (episode_id, series_id, season, episode, title, meta_date, monitored, status, runtime)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind("ep-media-info-test")
    .bind("ts-media-info")
    .bind(1)
    .bind(10)
    .bind("Media Info Ep")
    .bind(tomorrow)
    .bind(true)
    .bind("unreleased")
    .bind(60) // metadata runtime: 60 min — should NOT be used
    .execute(state.db.get_pool())
    .await
    .unwrap();

    // Insert a file_fingerprint with media_info containing a 42-minute duration
    let media_info_json = serde_json::json!({
        "codec": "h264",
        "resolution": "1920x1080",
        "duration": "42m 0s",
        "audio": "aac"
    });
    sqlx::query("INSERT INTO file_contents (fingerprint, media_info) VALUES (?, ?)")
        .bind("abc123")
        .bind(media_info_json.to_string())
        .execute(state.db.get_pool())
        .await
        .unwrap();

    sqlx::query(
        "INSERT INTO file_paths (file_path, fingerprint, inode, device, size, mtime, state)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(file_path)
    .bind("abc123")
    .bind(1001i64)
    .bind(8i64)
    .bind(524288000i64)
    .bind(1234567890.0f64)
    .bind("organized")
    .execute(state.db.get_pool())
    .await
    .unwrap();

    // Ownership lives in `episode_files` now, not on the episode row.
    state
        .db
        .associate_main_file("ep-media-info-test", file_path, None)
        .await
        .unwrap();

    // Create a token
    let payload = serde_json::json!({
        "name": "Media Info Test",
        "hide_unmonitored": true,
        "show_as_all_day": false
    });
    let req = add_auth(post_json_request(
        "/api/config/auth/calendar_tokens/generate",
        &payload,
    ));
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let resp: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let token = resp["token"].as_str().unwrap().to_string();

    // Fetch iCal
    let req = get_request(&format!("/api/calendar/ical?token={}", token));
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let ical = String::from_utf8(body.to_vec()).unwrap();

    // Extract DTSTART and DTEND
    let dtstart_line = ical.lines().find(|l| l.starts_with("DTSTART:")).unwrap();
    let dtend_line = ical.lines().find(|l| l.starts_with("DTEND:")).unwrap();

    let dtstart_str = dtstart_line.strip_prefix("DTSTART:").unwrap();
    let dtend_str = dtend_line.strip_prefix("DTEND:").unwrap();

    // Parse the datetime strings (format: YYYYMMDDTHHMMSSZ)
    let dtstart = chrono::NaiveDateTime::parse_from_str(dtstart_str, "%Y%m%dT%H%M%SZ")
        .expect("Failed to parse DTSTART");
    let dtend = chrono::NaiveDateTime::parse_from_str(dtend_str, "%Y%m%dT%H%M%SZ")
        .expect("Failed to parse DTEND");

    let diff_minutes = (dtend - dtstart).num_minutes();

    assert_eq!(
        diff_minutes, 42,
        "DTEND should be 42 minutes after DTSTART (media info duration), not {} (which would be the 60-min runtime)",
        diff_minutes
    );
}

// Canonical UTC storage preserves the time of day (not midnight)

#[tokio::test]
async fn test_api_key_expiry_stored_canonical_preserving_time() {
    let (_app, state, _temp_dir) = common::setup_test_app().await;
    state
        .db
        .insert_api_key(
            "k1",
            "name",
            "hash",
            "pfx",
            &[],
            Some("2026-06-18T20:30:45+00:00"),
        )
        .await
        .unwrap();

    let stored: Option<String> = sqlx::query_scalar("SELECT expires_at FROM api_keys WHERE id = ?")
        .bind("k1")
        .fetch_one(state.db.get_pool())
        .await
        .unwrap();
    assert_eq!(
        stored.as_deref(),
        Some("2026-06-18 20:30:45"),
        "expiry must keep the time of day in canonical UTC form, not collapse to midnight"
    );

    let keys = state.db.get_api_keys().await.unwrap();
    assert_eq!(
        keys[0].expires_at.as_deref(),
        Some("2026-06-18T20:30:45+00:00"),
        "the API/config layer still exposes RFC 3339"
    );
}

#[tokio::test]
async fn test_ban_timestamps_stored_canonical_preserving_time() {
    let (_app, state, _temp_dir) = common::setup_test_app().await;
    state
        .db
        .insert_banned_ip(&jumbie_shared::config::BannedIp {
            ip: "10.9.9.9".to_string(),
            fail_count: 1,
            ban_count: 1,
            banned_at: "2026-06-18T05:15:30+00:00".to_string(),
            banned_until: Some("2026-06-18T06:45:00+00:00".to_string()),
        })
        .await
        .unwrap();

    let row: (String, Option<String>) =
        sqlx::query_as("SELECT banned_at, banned_until FROM banned_ips WHERE ip = ?")
            .bind("10.9.9.9")
            .fetch_one(state.db.get_pool())
            .await
            .unwrap();
    assert_eq!(row.0, "2026-06-18 05:15:30");
    assert_eq!(row.1.as_deref(), Some("2026-06-18 06:45:00"));

    let bans = state.db.get_banned_ips().await.unwrap();
    assert_eq!(bans[0].banned_at, "2026-06-18T05:15:30+00:00");
    assert_eq!(
        bans[0].banned_until.as_deref(),
        Some("2026-06-18T06:45:00+00:00")
    );
}
