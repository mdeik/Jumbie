mod common;

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use tower::ServiceExt;

#[tokio::test]
async fn test_ical_link() {
    let (app, state, _temp_dir) = common::setup_test_app().await;

    // Legacy calendar-token auth (config.auth.calendar_token) must still be accepted.
    {
        let mut config = state.cfg.write().await;
        config.auth.calendar_token = Some("test_token".to_string());
    }

    let req = Request::builder()
        .method("GET")
        .uri("/api/calendar/ical?token=test_token")
        .body(Body::empty())
        .unwrap();

    let res = app.clone().oneshot(req).await.unwrap();

    assert_eq!(res.status(), StatusCode::OK);

    let content_types = res
        .headers()
        .get_all("content-type")
        .iter()
        .map(|v| v.to_str().unwrap().to_string())
        .collect::<Vec<String>>()
        .join(" ");
    assert!(content_types.contains("text/calendar"));

    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let body_str = String::from_utf8(body.to_vec()).unwrap();
    assert!(body_str.contains("BEGIN:VCALENDAR"));
}
