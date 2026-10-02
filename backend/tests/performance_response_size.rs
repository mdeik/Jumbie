// Performance tests: response size & deserialization overhead.
//
// The heavy endpoints (calendar, activity) must return responses bounded in
// size: the frontend runs in a single-threaded WASM environment where large
// JSON deserialization blocks the UI thread, so small payloads are a
// correctness requirement, not just an optimization.

mod common;

use std::time::Instant;
use tower::ServiceExt;

// Helpers

/// Measures the wall-clock response time of a GET request and returns the
/// response body bytes along with the elapsed duration in milliseconds.
async fn timed_get(app: &axum::Router, uri: &str) -> (Vec<u8>, u128) {
    let start = Instant::now();
    let req = common::get_request(uri);
    let res = app.clone().oneshot(req).await.unwrap();
    let elapsed = start.elapsed().as_millis();
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    (body.to_vec(), elapsed)
}

// Calendar performance

#[tokio::test]
async fn test_calendar_response_has_empty_episode_details() {
    let (app, _state, _tmp) = common::setup_test_app().await;

    // 3-month window, matching what the frontend requests.
    let (body_bytes, elapsed_ms) = timed_get(
        &app,
        "/api/calendar?start_date=2026-01-01T00:00:00%2B00:00&end_date=2026-04-01T23:59:59%2B00:00",
    )
    .await;

    let json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();

    // `episode_details` must be absent entirely: it was removed from
    // CalendarResponse after cloning `all_episodes` N times; its return is a
    // regression.
    let details = &json["episode_details"];
    assert!(
        details.is_null(),
        "episode_details must be absent from the calendar response — \
         it previously contained all_episodes cloned N times"
    );

    assert!(json["episodes"].is_array(), "episodes must be present");

    assert!(
        elapsed_ms < 500,
        "Calendar endpoint responded in {}ms (exceeds 500ms threshold)",
        elapsed_ms
    );
}

#[tokio::test]
async fn test_calendar_response_size_is_bounded() {
    let (app, _state, _tmp) = common::setup_test_app().await;

    let (body_bytes, _elapsed_ms) = timed_get(
        &app,
        "/api/calendar?start_date=2025-01-01&end_date=2027-01-01",
    )
    .await;

    // Guards against the removed per-episode details payload returning: even a
    // 2-year empty window must stay under 1KB.
    assert!(
        body_bytes.len() < 1024,
        "Calendar response body is {} bytes (expected < 1KB). \
         The `episode_details` map is now always empty — if this test fails, \
         the endpoint may have regressed and is shipping bloat again.",
        body_bytes.len()
    );
}

// Activity performance

#[tokio::test]
async fn test_activity_response_time() {
    let (app, _state, _tmp) = common::setup_test_app().await;

    let (body_bytes, elapsed_ms) = timed_get(
        &app,
        "/api/activity?page=0&limit=50&sort=timestamp&order=desc",
    )
    .await;

    let json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    assert!(json["items"].is_array(), "activity must have items array");

    assert!(
        elapsed_ms < 200,
        "Activity endpoint responded in {}ms (exceeds 200ms threshold)",
        elapsed_ms
    );

    assert!(
        body_bytes.len() < 50_000,
        "Activity response is {} bytes (expected < 50KB)",
        body_bytes.len()
    );
}
