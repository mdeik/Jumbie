// Search-Missing-On-Add — exercised through the single `POST /api/series`
// request.  The backend must sync metadata for each provided ID and, when
// `search_missing_on_add` is set, submit search jobs for monitored + missing
// + RELEASED episodes — all inside the create request (no client sequencing).

mod common;

use axum::http::StatusCode;
use std::collections::HashMap;
use std::sync::Arc;
use tower::ServiceExt;

use jumbie::api::AppState;
use jumbie::datetime::UtcDateTime;
use jumbie::plugins::metadata::EpisodeMetadata;
use jumbie_shared::plugin::Capability;
use jumbie_shared::types::{CreateSeriesRequest, MonitorMode};

/// A metadata episode in the shape the plugin bridge deserializes.
///
/// Built from a real `EpisodeMetadata` so the serialized form (including
/// `UtcDateTime`'s derived serde shape `{naive, origin}`) matches exactly what
/// real metadata plugins emit.
fn episode_json(season: i32, episode: i32, meta_date: &str) -> serde_json::Value {
    let meta_date = chrono::NaiveDate::parse_from_str(meta_date, "%Y-%m-%d")
        .ok()
        .map(UtcDateTime::from_naive_date);
    serde_json::to_value(EpisodeMetadata {
        unique_id: format!("ep-{}-{}", season, episode),
        season,
        episode,
        title: format!("Episode {}x{:02}", season, episode),
        description: None,
        runtime: None,
        image_url: None,
        meta_date,
    })
    .expect("episode serialization cannot fail")
}

/// Register a metadata plugin returning the given episodes.
async fn register_metadata_plugin(state: &Arc<AppState>, episodes: Vec<serde_json::Value>) {
    state
        .plugin_manager
        .write()
        .await
        .add_internal_plugin(Arc::new(common::MockMetadataPlugin {
            instance_id: "mock.meta.search".to_string(),
            display_name: "Search Mock Metadata".to_string(),
            capabilities: vec![Capability::MetadataProviderNormal],
            series_identifier_label: Some("Mock ID".to_string()),
            series_name: "Search On Add Show".to_string(),
            overview: "A test series overview.".to_string(),
            aliases: vec![],
            episodes,
        }));
}

/// Poll the DB for the search job's `auto_search_last_attempt_{series}_{season}`
/// state, which the SearchQueue writes AFTER the search job runs.  This is the
/// deterministic signal that the job was actually executed (not just queued).
async fn poll_last_attempt(
    state: &Arc<AppState>,
    series_id: &str,
    season: &str,
) -> serde_json::Value {
    let key = format!("auto_search_last_attempt_{}_{}", series_id, season);
    for _ in 0..100 {
        if let Some(value) = state.db.get_system_state(&key).await.unwrap_or(None) {
            return serde_json::from_str(&value).expect("invalid last-attempt JSON");
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!(
        "search job for {} season {} never ran (no system state {})",
        series_id, season, key
    );
}

/// POST a create-series request and return the created series UUID.
async fn create_series(app: &axum::Router, payload: CreateSeriesRequest) -> String {
    let req = common::post_json_request("/api/series", &payload);
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(
        res.status(),
        StatusCode::CREATED,
        "expected 201 for create-series"
    );
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let id: String = serde_json::from_slice(&body).unwrap();
    id.trim_matches('"').to_string()
}

fn request_with(
    monitor_mode: MonitorMode,
    metadata_ids: HashMap<String, String>,
    search_missing_on_add: bool,
) -> CreateSeriesRequest {
    CreateSeriesRequest {
        path: "Search On Add Show".to_string(),
        series_name: Some("Search On Add Show".to_string()),
        scan_for_existing: Some(false),
        monitor_mode: Some(monitor_mode),
        quality_profile: None,
        release_profile: None,
        metadata_ids,
        search_missing_on_add,
        resolve_collisions: true,
        settings: Default::default(),
    }
}

// Happy path: released + monitored episodes get searched

#[tokio::test]
async fn test_create_series_search_on_add_searches_released_episodes() {
    let (app, state, _tmp) = common::setup_test_app().await;
    register_metadata_plugin(
        &state,
        vec![
            episode_json(1, 1, "2024-01-05"),
            episode_json(1, 2, "2024-01-12"),
        ],
    )
    .await;

    let series_id = create_series(
        &app,
        request_with(
            MonitorMode::All,
            HashMap::from([("mock.meta.search".to_string(), "test_123".to_string())]),
            true,
        ),
    )
    .await;

    // The in-request metadata sync persisted both episodes, and monitor mode
    // "All" marked them monitored.
    let episodes = state
        .db
        .get_series_episodes_details(&series_id, false)
        .await
        .unwrap();
    assert_eq!(episodes.len(), 2, "both mock episodes must be persisted");
    assert!(
        episodes.iter().all(|e| e.monitored),
        "MonitorMode::All must monitor every regular episode"
    );

    // The search job ran for season 1 with BOTH episodes.
    let last = poll_last_attempt(&state, &series_id, "1").await;
    let searched = last["episodes"].as_array().unwrap();
    assert_eq!(searched.len(), 2, "both released episodes must be searched");
}

// Release dates are respected: future-dated episodes are NOT searched

#[tokio::test]
async fn test_create_series_search_on_add_skips_unreleased_episodes() {
    let (app, state, _tmp) = common::setup_test_app().await;
    register_metadata_plugin(
        &state,
        vec![
            episode_json(1, 1, "2024-01-05"), // released
            episode_json(1, 2, "2999-01-05"), // not yet aired
        ],
    )
    .await;

    let series_id = create_series(
        &app,
        request_with(
            MonitorMode::All,
            HashMap::from([("mock.meta.search".to_string(), "test_123".to_string())]),
            true,
        ),
    )
    .await;

    // Both episodes are monitored, but only the released one is searched.
    let episodes = state
        .db
        .get_series_episodes_details(&series_id, false)
        .await
        .unwrap();
    assert_eq!(episodes.len(), 2);
    assert!(episodes.iter().all(|e| e.monitored));

    let last = poll_last_attempt(&state, &series_id, "1").await;
    assert_eq!(
        last["episodes"],
        serde_json::json!([1]),
        "future-dated episode must not be searched"
    );
}

// Monitor mode is respected: MonitorMode::None → nothing is searched

#[tokio::test]
async fn test_create_series_search_on_add_respects_monitor_mode_none() {
    let (app, state, _tmp) = common::setup_test_app().await;
    register_metadata_plugin(&state, vec![episode_json(1, 1, "2024-01-05")]).await;

    let series_id = create_series(
        &app,
        request_with(
            MonitorMode::None,
            HashMap::from([("mock.meta.search".to_string(), "test_123".to_string())]),
            true,
        ),
    )
    .await;

    // Episodes exist but are NOT monitored.
    let episodes = state
        .db
        .get_series_episodes_details(&series_id, false)
        .await
        .unwrap();
    assert_eq!(episodes.len(), 1);
    assert!(!episodes[0].monitored, "MonitorMode::None must not monitor");

    // No search job was submitted — `submit_monitored_missing` found 0
    // monitored episodes synchronously inside the create request.
    assert!(
        !state
            .search_queue
            .is_active(&format!("{}:1", series_id))
            .await,
        "no search job may be queued when nothing is monitored"
    );
    // Gap 5: the no-op is observable through the activity log — and it is
    // INFORMATIONAL, not an error: 0 episodes available is not a failure.
    let status = state
        .db
        .get_recent_activity(50)
        .await
        .unwrap()
        .into_iter()
        .find(|i| {
            i.details
                .as_deref()
                .is_some_and(|d| d.contains("Search-on-add found 0"))
        })
        .map(|i| i.status);
    assert_eq!(
        status.as_deref(),
        Some("Info"),
        "0-found search-on-add must be recorded as informational, not an error"
    );
}

// Fix 3: checkbox is ignored when no metadata ID is present

#[tokio::test]
async fn test_create_series_search_on_add_ignored_without_metadata_ids() {
    let (app, state, _tmp) = common::setup_test_app().await;

    let series_id = create_series(&app, request_with(MonitorMode::All, HashMap::new(), true)).await;

    // No metadata ID → no metadata sync → no episodes, no search.
    let episodes = state
        .db
        .get_series_episodes_details(&series_id, false)
        .await
        .unwrap();
    assert!(
        episodes.is_empty(),
        "no sync may happen without metadata IDs"
    );
    assert!(
        !state
            .search_queue
            .is_active(&format!("{}:1", series_id))
            .await,
        "search_missing_on_add must be ignored without metadata IDs"
    );
}

// Sync failure is non-fatal: series still created, no search

#[tokio::test]
async fn test_create_series_sync_failure_keeps_series() {
    let (app, state, _tmp) = common::setup_test_app().await;
    // Default mock: empty episode list → the fetch errors with
    // "No episodes found for metadata ID ...".
    register_metadata_plugin(&state, vec![]).await;

    let series_id = create_series(
        &app,
        request_with(
            MonitorMode::All,
            HashMap::from([("mock.meta.search".to_string(), "bad_id".to_string())]),
            true,
        ),
    )
    .await;

    // The series exists despite the failed provider sync.
    assert!(
        state
            .db
            .get_series_mapping(&series_id)
            .await
            .unwrap()
            .is_some(),
        "series must be created even when metadata sync fails"
    );

    // No episodes, no search.
    let episodes = state
        .db
        .get_series_episodes_details(&series_id, false)
        .await
        .unwrap();
    assert!(episodes.is_empty());
    assert!(
        !state
            .search_queue
            .is_active(&format!("{}:1", series_id))
            .await,
        "no search may run when the metadata sync failed"
    );
    // Gap 5: the failure is visible in the activity log — and it IS an error
    // (the provider fetch itself failed), unlike a 0-found search.
    let status = state
        .db
        .get_recent_activity(50)
        .await
        .unwrap()
        .into_iter()
        .find(|i| {
            i.details
                .as_deref()
                .is_some_and(|d| d.contains("Sync failed"))
        })
        .map(|i| i.status);
    assert_eq!(
        status.as_deref(),
        Some("Error"),
        "a failed provider sync must be recorded as an error"
    );
}
