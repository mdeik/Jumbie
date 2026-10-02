mod common;

use crate::common::TestApp;
use axum::body::Body;
use axum::http::StatusCode;
use jumbie_shared::{
    mapping::MonitorMode, scoring::QualityProfile, types::SearchPayload, types::UpdateSeriesPayload,
};
use std::collections::HashMap;
use tower::ServiceExt;

/// Build a POST request to /api/search/auto-season with the given payload.
/// No auth headers — works with setup_test_app (no password = auth disabled).
fn auto_season_request(payload: &SearchPayload) -> axum::http::Request<Body> {
    let body = serde_json::to_string(payload).unwrap();
    axum::http::Request::builder()
        .method("POST")
        .uri("/api/search/auto-season")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap()
}

// GET /api/calendar

#[tokio::test]
async fn test_get_calendar() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let res = app
        .oneshot(common::get_request(
            "/api/calendar?start_date=2024-01-01T00:00:00%2B00:00&end_date=2024-01-31T23:59:59%2B00:00",
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    // CalendarResponse is an object with an episodes array
    assert!(
        json.is_object(),
        "Calendar endpoint must return a JSON object"
    );
    assert!(
        json["episodes"].is_array(),
        "Calendar response must have an 'episodes' array"
    );
}

#[tokio::test]
async fn test_get_calendar_bad_date() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let res = app
        .oneshot(common::get_request(
            "/api/calendar?start_date=not-a-date&end_date=also-invalid",
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

// POST /api/search

#[tokio::test]
async fn test_search_no_sources_returns_empty() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let payload = SearchPayload {
        query: "Test Anime".to_string(),
        series_id: None,
        episode_id: None,
        mode: None,
        season: None,
        episode_numbers: None,
        is_user_requested: false,
    };
    let res = app
        .oneshot(common::post_json_request("/api/search", &payload))
        .await
        .unwrap();
    // No sources configured in test env, so the endpoint should report
    // that no Search Plugins are available rather than returning empty results.
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        json.get("error").and_then(|v| v.as_str()),
        Some("No Search Plugins are available.")
    );
}

#[tokio::test]
async fn test_search_auto_episode_requires_season() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let payload = SearchPayload {
        query: "Test".to_string(),
        series_id: Some("some_series_id".to_string()),
        episode_id: None,
        mode: Some("auto_episode".to_string()),
        season: None, // missing → 400
        episode_numbers: Some(vec![1]),
        is_user_requested: false,
    };
    let res = app
        .oneshot(common::post_json_request("/api/search", &payload))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_search_auto_episode_requires_episode_numbers() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let payload = SearchPayload {
        query: "Test".to_string(),
        series_id: Some("some_series_id".to_string()),
        episode_id: None,
        mode: Some("auto_episode".to_string()),
        season: Some("01".to_string()),
        episode_numbers: None, // missing → 400
        is_user_requested: false,
    };
    let res = app
        .oneshot(common::post_json_request("/api/search", &payload))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_search_auto_episode_requires_series_id() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let payload = SearchPayload {
        query: "Test".to_string(),
        series_id: None, // missing
        episode_id: None,
        mode: Some("auto_episode".to_string()),
        season: Some("01".to_string()),
        episode_numbers: Some(vec![1]),
        is_user_requested: false,
    };
    let res = app
        .oneshot(common::post_json_request("/api/search", &payload))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

// POST /api/search — season mode now rejected with redirect
// Auto-season was moved to POST /api/search/auto-season (compound scope).
// The old endpoint now returns a clear redirect message.

#[tokio::test]
async fn test_search_rejects_season_mode() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let payload = SearchPayload {
        query: "Test Show S01".to_string(),
        series_id: Some("some_series_id".to_string()),
        episode_id: None,
        mode: Some("season".to_string()),
        season: Some("01".to_string()),
        episode_numbers: Some(vec![1]),
        is_user_requested: false,
    };
    let res = app
        .oneshot(common::post_json_request("/api/search", &payload))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let msg = json.get("error").and_then(|v| v.as_str()).unwrap_or("");
    assert!(
        msg.contains("/api/search/auto-season"),
        "Expected redirect message, got: {}",
        msg
    );
}

#[tokio::test]
async fn test_search_rejects_auto_season_mode() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let payload = SearchPayload {
        query: "Test Show S01".to_string(),
        series_id: Some("some_series_id".to_string()),
        episode_id: None,
        mode: Some("auto_season".to_string()),
        season: Some("01".to_string()),
        episode_numbers: Some(vec![1]),
        is_user_requested: false,
    };
    let res = app
        .oneshot(common::post_json_request("/api/search", &payload))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let msg = json.get("error").and_then(|v| v.as_str()).unwrap_or("");
    assert!(
        msg.contains("/api/search/auto-season"),
        "Expected redirect message, got: {}",
        msg
    );
}

#[tokio::test]
async fn test_search_season_kwargs_ignored_in_manual_mode() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let payload = SearchPayload {
        query: "Test Anime".to_string(),
        series_id: None,
        episode_id: None,
        mode: None,
        season: Some("99".to_string()),  // present but ignored
        episode_numbers: Some(vec![42]), // present but ignored
        is_user_requested: false,
    };
    let res = app
        .oneshot(common::post_json_request("/api/search", &payload))
        .await
        .unwrap();
    // No sources configured → the endpoint reports no plugins available
    // rather than returning an empty array.
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        json.get("error").and_then(|v| v.as_str()),
        Some("No Search Plugins are available.")
    );
}

// POST /api/search/auto-season — dedicated endpoint with compound scope
// These tests need Basic auth with admin:password (which has ALL scopes).

fn basic_auth_header() -> String {
    format!(
        "Basic {}",
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, "admin:password")
    )
}

fn post_auto_season_request(payload: &SearchPayload) -> axum::http::Request<axum::body::Body> {
    let body = serde_json::to_string(payload).unwrap();
    axum::http::Request::builder()
        .method("POST")
        .uri("/api/search/auto-season")
        .header("content-type", "application/json")
        .header("authorization", basic_auth_header())
        .body(axum::body::Body::from(body))
        .unwrap()
}

#[tokio::test]
async fn test_auto_season_no_episodes() {
    let (app, _state, _tmp) = common::setup_authenticated_app().await;
    let payload = SearchPayload {
        query: "Test Show S01".to_string(),
        series_id: Some("some_series_id".to_string()),
        episode_id: None,
        mode: Some("season".to_string()),
        season: Some("01".to_string()),
        episode_numbers: Some(vec![]), // empty → 400
        is_user_requested: false,
    };
    let res = app
        .clone()
        .oneshot(post_auto_season_request(&payload))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_auto_season_missing_series_id() {
    let (app, _state, _tmp) = common::setup_authenticated_app().await;
    let payload = SearchPayload {
        query: "Test Show S01".to_string(),
        series_id: None,
        episode_id: None,
        mode: Some("season".to_string()),
        season: Some("01".to_string()),
        episode_numbers: Some(vec![1]),
        is_user_requested: false,
    };
    let res = app
        .clone()
        .oneshot(post_auto_season_request(&payload))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_auto_season_missing_season_field() {
    let (app, _state, _tmp) = common::setup_authenticated_app().await;
    let payload = SearchPayload {
        query: "Test Show".to_string(),
        series_id: Some("some_series_id".to_string()),
        episode_id: None,
        mode: Some("season".to_string()),
        season: None,
        episode_numbers: Some(vec![1, 2, 3]),
        is_user_requested: false,
    };
    let res = app
        .clone()
        .oneshot(post_auto_season_request(&payload))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_auto_season_missing_episode_numbers() {
    let (app, _state, _tmp) = common::setup_authenticated_app().await;
    let payload = SearchPayload {
        query: "Test Show".to_string(),
        series_id: Some("some_series_id".to_string()),
        episode_id: None,
        mode: Some("season".to_string()),
        season: Some("01".to_string()),
        episode_numbers: None,
        is_user_requested: false,
    };
    let res = app
        .clone()
        .oneshot(post_auto_season_request(&payload))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_auto_season_returns_accepted() {
    // Use setup_test_app (no password = auth disabled = all scopes granted)
    let (app, _state, _tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Auto Search Show").await;
    let payload = SearchPayload {
        query: "Auto Search Show S01".to_string(),
        series_id: Some(series_id),
        episode_id: None,
        mode: Some("season".to_string()),
        season: Some("01".to_string()),
        episode_numbers: Some(vec![1, 2, 3]),
        is_user_requested: false,
    };
    let body = serde_json::to_string(&payload).unwrap();
    let req = axum::http::Request::builder()
        .method("POST")
        .uri("/api/search/auto-season")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(body))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    // Returns 200 OK with empty array; background task is spawned
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json.is_array());
    assert!(json.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn test_auto_season_requires_auth() {
    // Unauthenticated request should get 401 from auth middleware
    let (app, _state, _tmp) = common::setup_authenticated_app().await;
    let payload = SearchPayload {
        query: "Test".to_string(),
        series_id: Some("x".to_string()),
        episode_id: None,
        mode: Some("season".to_string()),
        season: Some("01".to_string()),
        episode_numbers: Some(vec![1]),
        is_user_requested: false,
    };
    let body = serde_json::to_string(&payload).unwrap();
    let req = axum::http::Request::builder()
        .method("POST")
        .uri("/api/search/auto-season")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(body))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

// POST /api/search (auto_episode mode)

#[tokio::test]
async fn test_search_auto_episode_mode_requires_queue_write_scope() {
    // Auth enabled so the request runs with a scoped key, not the auth-disabled
    // all-scopes path (which would make the guard unreachable).
    let (app, _state, _tmp) = common::setup_authenticated_app().await;

    // API key with only `search` — enough for the route, not for auto-episode.
    let key_payload = serde_json::json!({
        "name": "search only",
        "scopes": ["search"],
        "duration_days": 7
    });
    let mut req = common::post_json_request("/api/config/auth/api_keys/generate", &key_payload);
    req.headers_mut().insert(
        axum::http::header::AUTHORIZATION,
        "Basic YWRtaW46cGFzc3dvcmQ=".parse().unwrap(),
    );
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let key = serde_json::from_slice::<serde_json::Value>(&body).unwrap()["key"]
        .as_str()
        .unwrap()
        .to_string();

    let payload = SearchPayload {
        query: "Test Show".to_string(),
        series_id: Some("any_id".to_string()),
        episode_id: None,
        mode: Some("auto_episode".to_string()),
        season: Some("01".to_string()),
        episode_numbers: Some(vec![1]),
        is_user_requested: false,
    };
    let mut req = common::post_json_request("/api/search", &payload);
    req.headers_mut().insert(
        axum::http::header::AUTHORIZATION,
        format!("Bearer {key}").parse().unwrap(),
    );
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        res.headers()
            .get(axum::http::header::WWW_AUTHENTICATE)
            .and_then(|v| v.to_str().ok()),
        Some("Bearer error=\"insufficient_scope\", scope=\"queue:write\"")
    );
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let err: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(err["required_scopes"], serde_json::json!(["queue:write"]));
    assert_eq!(err["missing_scopes"], serde_json::json!(["queue:write"]));
}

#[tokio::test]
async fn test_search_auto_episode_validates_episode_numbers() {
    let (app, _state, _tmp) = common::setup_test_app().await;

    // Empty episode_numbers → 400
    let payload = SearchPayload {
        query: "Test Show S01".to_string(),
        series_id: Some("some_id".to_string()),
        episode_id: None,
        mode: Some("auto_episode".to_string()),
        season: Some("01".to_string()),
        episode_numbers: Some(vec![]),
        is_user_requested: false,
    };
    let res = app
        .clone()
        .oneshot(common::post_json_request("/api/search", &payload))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

// POST /api/search (season mode) — downloaded-episode filtering.
//
// Exercises `filter_downloaded_episodes` through the endpoint; with no source
// plugins configured the filtering effect isn't directly observable, so
// `auto_search::tests` asserts the function in isolation.

#[tokio::test]
async fn test_search_season_with_existing_downloads_replacement_off() {
    let (app, state, _tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Downloaded Show").await;

    let mut q_profiles: HashMap<String, QualityProfile> = HashMap::new();
    q_profiles.insert(
        "Any".to_string(),
        QualityProfile {
            name: "Any".to_string(),
            qualities: vec!["Any".to_string()],
            upgrade_only_qualities: vec![],
        },
    );
    let _: serde_json::Value = app
        .put_json("/api/config/quality_profiles", &q_profiles)
        .await;

    // Upgrades are always on for monitored episodes, so no upgrade toggle needed.
    let update = UpdateSeriesPayload {
        quality_profile: "Any".to_string(),
        release_profile: "Any".to_string(),
        title: Some("Downloaded Show".to_string()),
        path_operation: None,
        settings: jumbie_shared::mapping::SeriesSettings {
            path: Some("Downloaded Show".to_string()),
            monitor_mode: Some(MonitorMode::All),
            absolute_numbering: Some(false),
            ..Default::default()
        },
    };
    let _: serde_json::Value = app
        .put_json(&format!("/api/series/{}", series_id), &update)
        .await;

    // Insert some episodes with file_path (simulate already downloaded)
    let pool = state.db.get_pool();
    for ep in &[1, 3] {
        let episode_id = format!("ep-{ep}");
        sqlx::query(
            "INSERT INTO episodes (episode_id, season, episode, status)
             VALUES (?, ?, ?, 'organized')",
        )
        .bind(&episode_id)
        .bind(1)
        .bind(*ep)
        .execute(pool)
        .await
        .unwrap();
        state
            .db
            .associate_main_file(&episode_id, "/media/show/S01/ep.mkv", None)
            .await
            .unwrap();
    }

    // Call auto-search for episodes 1-5 (including already-downloaded 1, 3)
    let payload = SearchPayload {
        query: "Downloaded Show S01".to_string(),
        series_id: Some(series_id),
        episode_id: None,
        mode: Some("season".to_string()),
        season: Some("S01".to_string()),
        episode_numbers: Some(vec![1, 2, 3, 4, 5]),
        is_user_requested: false,
    };
    let res = app
        .clone()
        .oneshot(auto_season_request(&payload))
        .await
        .unwrap();
    let status = res.status();
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(
        status,
        StatusCode::OK,
        "Auto-search with replacement on should still be accepted"
    );
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json.is_array());
    assert!(json.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn test_search_season_with_existing_downloads_replacement_on() {
    let (app, state, _tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Upgrade Show").await;

    let mut q_profiles: HashMap<String, QualityProfile> = HashMap::new();
    q_profiles.insert(
        "Any".to_string(),
        QualityProfile {
            name: "Any".to_string(),
            qualities: vec!["Any".to_string()],
            upgrade_only_qualities: vec![],
        },
    );
    let _: serde_json::Value = app
        .put_json("/api/config/quality_profiles", &q_profiles)
        .await;

    // Upgrades are always on for monitored episodes, so no upgrade toggle needed.
    let update = UpdateSeriesPayload {
        quality_profile: "Any".to_string(),
        release_profile: "Any".to_string(),
        title: Some("Upgrade Show".to_string()),
        path_operation: None,
        settings: jumbie_shared::mapping::SeriesSettings {
            path: Some("Upgrade Show".to_string()),
            monitor_mode: Some(MonitorMode::All),
            absolute_numbering: Some(false),
            ..Default::default()
        },
    };
    let _: serde_json::Value = app
        .put_json(&format!("/api/series/{}", series_id), &update)
        .await;

    // Insert some episodes with file_path
    let pool = state.db.get_pool();
    for ep in &[2, 4] {
        let episode_id = format!("upgrade-ep-{ep}");
        sqlx::query(
            "INSERT INTO episodes (episode_id, season, episode, status)
             VALUES (?, ?, ?, 'organized')",
        )
        .bind(&episode_id)
        .bind(1)
        .bind(ep)
        .execute(pool)
        .await
        .unwrap();
        state
            .db
            .associate_main_file(&episode_id, "/media/show/S01/ep.mkv", None)
            .await
            .unwrap();
    }

    // Call auto-search: with replacement ON, the background task should keep ALL episodes
    let payload = SearchPayload {
        query: "Upgrade Show S01".to_string(),
        series_id: Some(series_id),
        episode_id: None,
        mode: Some("season".to_string()),
        season: Some("S01".to_string()),
        episode_numbers: Some(vec![1, 2, 3, 4, 5]),
        is_user_requested: false,
    };
    let res = app
        .clone()
        .oneshot(auto_season_request(&payload))
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        StatusCode::OK,
        "Auto-search with replacement on should still be accepted"
    );
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json.is_array());
}

#[tokio::test]
async fn test_search_season_all_downloaded_replacement_off() {
    let (app, state, _tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Full Show").await;

    let mut q_profiles: HashMap<String, QualityProfile> = HashMap::new();
    q_profiles.insert(
        "Any".to_string(),
        QualityProfile {
            name: "Any".to_string(),
            qualities: vec!["Any".to_string()],
            upgrade_only_qualities: vec![],
        },
    );
    let _: serde_json::Value = app
        .put_json("/api/config/quality_profiles", &q_profiles)
        .await;

    // Update series
    let update = UpdateSeriesPayload {
        quality_profile: "Any".to_string(),
        release_profile: "Any".to_string(),
        title: Some("Full Show".to_string()),
        path_operation: None,
        settings: jumbie_shared::mapping::SeriesSettings {
            path: Some("Full Show".to_string()),
            monitor_mode: Some(MonitorMode::All),
            absolute_numbering: Some(false),
            ..Default::default()
        },
    };
    let _: serde_json::Value = app
        .put_json(&format!("/api/series/{}", series_id), &update)
        .await;

    // Insert ALL episodes with file_path (all downloaded)
    let pool = state.db.get_pool();
    for ep in &[1, 2, 3] {
        let episode_id = format!("full-ep-{ep}");
        sqlx::query(
            "INSERT INTO episodes (episode_id, season, episode, status)
             VALUES (?, ?, ?, 'organized')",
        )
        .bind(&episode_id)
        .bind(1)
        .bind(ep)
        .execute(pool)
        .await
        .unwrap();
        state
            .db
            .associate_main_file(&episode_id, "/media/show/S01/ep.mkv", None)
            .await
            .unwrap();
    }

    // All 3 episodes are already downloaded
    let payload = SearchPayload {
        query: "Full Show S01".to_string(),
        series_id: Some(series_id),
        episode_id: None,
        mode: Some("season".to_string()),
        season: Some("S01".to_string()),
        episode_numbers: Some(vec![1, 2, 3]),
        is_user_requested: false,
    };
    let res = app
        .clone()
        .oneshot(auto_season_request(&payload))
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        StatusCode::OK,
        "Auto-search with all episodes already downloaded and replacement off should be accepted"
    );
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json.is_array());
}

// Auto-season dedup guard

#[tokio::test]
async fn test_auto_season_dedup_returns_conflict_on_duplicate() {
    let (app, state, _tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Dedup Show").await;

    let mut q_profiles: HashMap<String, QualityProfile> = HashMap::new();
    q_profiles.insert(
        "Any".to_string(),
        QualityProfile {
            name: "Any".to_string(),
            qualities: vec!["Any".to_string()],
            upgrade_only_qualities: vec![],
        },
    );
    let _: serde_json::Value = app
        .put_json("/api/config/quality_profiles", &q_profiles)
        .await;

    let payload = SearchPayload {
        query: "Dedup Show S01".to_string(),
        series_id: Some(series_id.clone()),
        episode_id: None,
        mode: Some("season".to_string()),
        season: Some("01".to_string()),
        episode_numbers: Some(vec![1, 2]),
        is_user_requested: false,
    };

    // Insert a fake entry into the search queue's active set to simulate a running background task
    let search_key = format!("{}:{}", series_id, "01");
    state.search_queue.mark_active(&search_key).await;

    // Call with fake entry active: should fail with 409 Conflict
    let res = app
        .clone()
        .oneshot(auto_season_request(&payload))
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        StatusCode::CONFLICT,
        "Auto-search should return 409 Conflict when already running for this season"
    );

    // Clean up the fake entry
    state.search_queue.mark_inactive(&search_key).await;

    // Now call without fake entry: should succeed
    let res = app
        .clone()
        .oneshot(auto_season_request(&payload))
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        StatusCode::OK,
        "Auto-search should succeed when no search is already running"
    );
}

#[tokio::test]
async fn test_auto_season_status_endpoint() {
    let (app, state, _tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Status Show").await;

    // Initially, no search is running → status should be false
    let (status, body): (_, serde_json::Value) = common::get_json_raw(
        &app,
        &format!("/api/search/auto-season/{}/01/status", series_id),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["running"], serde_json::json!(false));

    // Manually insert into the search queue's active set
    let search_key = format!("{}:{}", series_id, "01");
    state.search_queue.mark_active(&search_key).await;

    // Now status should be true
    let (status, body): (_, serde_json::Value) = common::get_json_raw(
        &app,
        &format!("/api/search/auto-season/{}/01/status", series_id),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["running"], serde_json::json!(true));

    // Clean up
    let search_key = format!("{}:{}", series_id, "01");
    state.search_queue.mark_inactive(&search_key).await;

    // After cleanup, status should be false again
    let (status, body): (_, serde_json::Value) = common::get_json_raw(
        &app,
        &format!("/api/search/auto-season/{}/01/status", series_id),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["running"], serde_json::json!(false));
}
