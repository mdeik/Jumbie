mod common;

use crate::common::TestApp;
use jumbie_shared::types::SeriesInfo;
use std::collections::HashMap;

#[tokio::test]
async fn test_media_info_scan_counts_empty() {
    let (app, _state, _tmp) = common::setup_test_app().await;

    let counts: HashMap<String, usize> = app.get_json("/api/media-info-scan/counts").await;
    assert!(
        counts.is_empty(),
        "Expected empty map when no active scans, got {:?}",
        counts
    );
}

#[tokio::test]
async fn test_media_info_scan_counts_with_series_no_scans() {
    let (app, _state, _tmp) = common::setup_test_app().await;

    let series_id = common::create_test_series(&app, "Scan Count Show").await;

    let series_list: Vec<SeriesInfo> = app.get_json("/api/series").await;
    assert!(
        series_list.iter().any(|s| s.id == series_id),
        "Series should exist"
    );

    let counts: HashMap<String, usize> = app.get_json("/api/media-info-scan/counts").await;
    assert!(
        counts.is_empty(),
        "Expected empty map when no scans running, got {:?}",
        counts
    );
}

#[tokio::test]
async fn test_media_info_scan_counts_with_multiple_series() {
    let (app, _state, _tmp) = common::setup_test_app().await;

    let id1 = common::create_test_series(&app, "Series Alpha").await;
    let id2 = common::create_test_series(&app, "Series Beta").await;

    let series_list: Vec<SeriesInfo> = app.get_json("/api/series").await;
    assert!(series_list.iter().any(|s| s.id == id1));
    assert!(series_list.iter().any(|s| s.id == id2));

    let counts: HashMap<String, usize> = app.get_json("/api/media-info-scan/counts").await;
    assert!(
        counts.is_empty(),
        "Expected empty map when no scans running, got {:?}",
        counts
    );
}

#[tokio::test]
async fn test_media_info_scan_counts_endpoint_returns_object() {
    let (app, _state, _tmp) = common::setup_test_app().await;

    // The endpoint must always return a JSON object (not an array or null).
    let res = common::send_request(&app, common::get_request("/api/media-info-scan/counts")).await;
    assert!(
        res.status().is_success(),
        "GET /api/media-info-scan/counts should return 200 OK"
    );

    let body = common::response_body(res).await;
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json.is_object(), "Response should be a JSON object");
}

#[tokio::test]
async fn test_media_info_scan_count_for_series_unknown_id() {
    let (app, _state, _tmp) = common::setup_test_app().await;

    let counts: HashMap<String, usize> = app
        .get_json("/api/media-info-scan/counts/nonexistent-id")
        .await;
    assert!(
        counts.is_empty(),
        "Expected empty map for unknown series, got {:?}",
        counts
    );
}

#[tokio::test]
async fn test_media_info_scan_count_for_series_no_scans() {
    let (app, _state, _tmp) = common::setup_test_app().await;

    let series_id = common::create_test_series(&app, "Count For Series Show").await;

    let counts: HashMap<String, usize> = app
        .get_json(&format!("/api/media-info-scan/counts/{}", series_id))
        .await;
    assert!(
        counts.is_empty(),
        "Expected empty map when no scans running, got {:?}",
        counts
    );
}

#[tokio::test]
async fn test_media_info_scan_count_for_series_endpoint_returns_object() {
    let (app, _state, _tmp) = common::setup_test_app().await;

    let series_id = common::create_test_series(&app, "Object Check Show").await;

    // The endpoint must always return a JSON object (not an array or null).
    let res = common::send_request(
        &app,
        common::get_request(&format!("/api/media-info-scan/counts/{}", series_id)),
    )
    .await;
    assert!(
        res.status().is_success(),
        "GET /api/media-info-scan/counts/{{id}} should return 200 OK"
    );

    let body = common::response_body(res).await;
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json.is_object(), "Response should be a JSON object");
}
