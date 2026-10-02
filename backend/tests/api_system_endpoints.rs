mod common;

use axum::Router;
use axum::http::StatusCode;
use jumbie::api::AppState;
use jumbie_shared::types::{MappingRule, RemediatePayload, SeriesSettings, ValidatePathPayload};
use std::collections::HashMap;
use std::sync::Arc;
use tempfile::TempDir;
use tower::ServiceExt;

// Public ping

#[tokio::test]
async fn test_ping() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let res = app
        .oneshot(common::get_request("/api/public/ping"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["status"], "ok");
}

// About

#[tokio::test]
async fn test_get_about() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let res = app
        .oneshot(common::get_request("/api/system/about"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json["version"].is_string());
    assert!(json["os"].is_string());
}

// Health

#[tokio::test]
async fn test_get_health() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let res = app
        .oneshot(common::get_request("/api/system/health"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["status"], "Healthy");
}

// Plugin metrics

#[tokio::test]
async fn test_get_plugin_metrics() {
    // No external plugins in the test environment — the endpoint must return an
    // empty array (per-TYPE metrics, one entry per external plugin type).
    let (app, _state, _tmp) = common::setup_test_app().await;
    let res = app
        .oneshot(common::get_request("/api/system/plugins-metrics"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json.is_array(), "expected array, got {json:?}");
}

// Log level

#[tokio::test]
async fn test_get_log_level() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let res = app
        .oneshot(common::get_request("/api/system/log-level"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["level"], "INFO");
}

// Logs

#[tokio::test]
async fn test_get_logs_empty() {
    // No log files exist in the test environment — should return empty array
    let (app, _state, _tmp) = common::setup_test_app().await;
    let res = app
        .oneshot(common::get_request("/api/system/logs"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json.is_object());
    assert!(json["items"].is_array());
}

// Raw log files (GET /api/system/logs/file)

/// Build an app whose log endpoints read from a fixture directory seeded with
/// `files` (`name -> contents`). Oldest archive first: `jumbie.log.2`,
/// `jumbie.log.1`, then current `jumbie.log`.
///
/// Returns **two** temp dirs, and both must stay alive for the app's lifetime:
/// the logs fixture dir, AND the setup-created dir that holds the SQLite DB,
/// `config.toml` and `plugins/`. Dropping the latter deletes the database file out
/// from under the open pool — existing connections keep working via their fd, but
/// any later connection/`-shm` open fails with `SQLITE_CANTOPEN`, which (under
/// parallel load) surfaces as an intermittent 401 from the auth middleware.
async fn app_with_log_files(files: &[(&str, &str)]) -> (Router, Arc<AppState>, Vec<TempDir>) {
    let logs_tmp = TempDir::new().expect("Failed to create temp dir");
    let logs_dir = logs_tmp.path().join("logs");
    std::fs::create_dir_all(&logs_dir).unwrap();
    for (name, contents) in files {
        std::fs::write(logs_dir.join(name), contents).unwrap();
    }
    let (app, state, setup_tmp) =
        common::setup_test_app_with_logs(&logs_dir.to_string_lossy()).await;
    (app, state, vec![logs_tmp, setup_tmp])
}

/// Seed a rotated set (oldest → newest): `.2`=INFO a, `.1`=DEBUG b + INFO c,
/// current=DEBUG d + INFO e, plus decoys that must never be served.
fn seeded_log_files() -> Vec<(&'static str, &'static str)> {
    vec![
        ("jumbie.log.2", "2026-01-01T00:00:00Z  INFO jumbie: a\n"),
        (
            "jumbie.log.1",
            "2026-01-01T00:00:01Z DEBUG jumbie: b\n2026-01-01T00:00:02Z  INFO jumbie: c\n",
        ),
        (
            "jumbie.log",
            "2026-01-01T00:00:03Z DEBUG jumbie: d\n2026-01-01T00:00:04Z  INFO jumbie: e\n",
        ),
        (
            "jumbie.log.2026-08-24",
            "2026-01-01T00:00:00Z  INFO jumbie: SECRET-OLD\n",
        ),
        ("other.txt", "SECRET-OTHER\n"),
    ]
}

async fn body_text(res: axum::response::Response) -> String {
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    String::from_utf8(body.to_vec()).unwrap()
}

#[tokio::test]
async fn test_get_logs_file_tail_returns_last_matching_lines_chronological() {
    let files = seeded_log_files();
    let (app, _state, _tmp) = app_with_log_files(&files).await;
    // Level defaults to INFO, so the DEBUG lines are filtered out. The last two
    // INFO lines are `c` (from .1) and `e` (current), oldest → newest.
    let res = app
        .oneshot(common::get_request("/api/system/logs/file?tail=2"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let text = body_text(res).await;
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(
        lines,
        vec![
            "2026-01-01T00:00:02Z  INFO jumbie: c",
            "2026-01-01T00:00:04Z  INFO jumbie: e",
        ]
    );
}

#[tokio::test]
async fn test_get_logs_file_tail_with_level_debug_never_serves_foreign_files() {
    let files = seeded_log_files();
    let (app, _state, _tmp) = app_with_log_files(&files).await;
    // level=debug + a tail larger than the set: everything in the scheme is
    // returned chronologically, and the decoys (date-named file, other.txt)
    // are never served.
    let res = app
        .oneshot(common::get_request(
            "/api/system/logs/file?tail=100&level=debug",
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let text = body_text(res).await;
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 5);
    assert_eq!(lines[0], "2026-01-01T00:00:00Z  INFO jumbie: a");
    assert_eq!(lines[1], "2026-01-01T00:00:01Z DEBUG jumbie: b");
    assert_eq!(lines[2], "2026-01-01T00:00:02Z  INFO jumbie: c");
    assert_eq!(lines[3], "2026-01-01T00:00:03Z DEBUG jumbie: d");
    assert_eq!(lines[4], "2026-01-01T00:00:04Z  INFO jumbie: e");
    assert!(!text.contains("SECRET-OLD"));
    assert!(!text.contains("SECRET-OTHER"));
}

#[tokio::test]
async fn test_get_logs_file_head_returns_first_matching_lines() {
    let files = seeded_log_files();
    let (app, _state, _tmp) = app_with_log_files(&files).await;
    // head=2 with the default INFO filter: `a` (from .2) then `c` (from .1),
    // stopping as soon as the quota is met.
    let res = app
        .oneshot(common::get_request("/api/system/logs/file?head=2"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let text = body_text(res).await;
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(
        lines,
        vec![
            "2026-01-01T00:00:00Z  INFO jumbie: a",
            "2026-01-01T00:00:02Z  INFO jumbie: c",
        ]
    );
}

#[tokio::test]
async fn test_get_logs_file_rejects_tail_and_head_together() {
    let files = seeded_log_files();
    let (app, _state, _tmp) = app_with_log_files(&files).await;
    let res = app
        .oneshot(common::get_request("/api/system/logs/file?tail=2&head=2"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_get_logs_file_rejects_unknown_level() {
    let files = seeded_log_files();
    let (app, _state, _tmp) = app_with_log_files(&files).await;
    let res = app
        .oneshot(common::get_request("/api/system/logs/file?level=critical"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_get_logs_file_ignores_file_param() {
    // A `file=` query param must not select an arbitrary file — the handler
    // only knows the size-rolled scheme; the param is simply ignored.
    let files = seeded_log_files();
    let (app, _state, _tmp) = app_with_log_files(&files).await;
    let res = app
        .oneshot(common::get_request(
            "/api/system/logs/file?tail=2&file=/etc/passwd",
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let text = body_text(res).await;
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(
        lines,
        vec![
            "2026-01-01T00:00:02Z  INFO jumbie: c",
            "2026-01-01T00:00:04Z  INFO jumbie: e",
        ]
    );
}

// Memory (GET /api/system/memory)

#[tokio::test]
async fn test_get_memory_reports_logs_category() {
    let files = seeded_log_files();
    let (app, _state, _tmp) = app_with_log_files(&files).await;
    let res = app
        .oneshot(common::get_request("/api/system/memory"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let logs = &json["logs"];
    assert!(logs.is_object());
    // The byte-bounded buffer must never exceed its budget — the invariant
    // that makes a recurrence of the ~1.5 GiB log-buffer incident impossible.
    let buffer_bytes = logs["buffer_bytes"].as_u64().unwrap();
    let budget = logs["buffer_budget_bytes"].as_u64().unwrap();
    assert_eq!(budget, jumbie_shared::types::LOG_BUFFER_BYTES as u64);
    assert!(buffer_bytes <= budget);
    assert!(logs["buffer_entries"].as_u64().is_some());
    // The fixture dir has 3 in-scheme files; the decoys are not counted.
    assert!(logs["disk_bytes"].as_u64().unwrap() > 0);
}

// Activity

#[tokio::test]
async fn test_get_activity() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let res = app
        .oneshot(common::get_request("/api/activity"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json.is_object());
    assert!(json["items"].is_array());
}

// Wanted

#[tokio::test]
async fn test_get_wanted() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let res = app
        .oneshot(common::get_request("/api/wanted"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    // The wanted endpoint returns a PaginatedResponse, which is an object with an "items" array
    assert!(
        json.is_object(),
        "Expected an object with items, got {:?}",
        json
    );
    assert!(
        json["items"].is_array(),
        "Expected items array in {:?}",
        json
    );
}

#[tokio::test]
async fn test_wanted_shows_in_queue_when_download_queued() {
    let (app, state, _tmp) = common::setup_test_app().await;

    let series_id = common::create_test_series(&app, "In Queue Show").await;
    let series_id = series_id.trim_matches('"').to_string();
    let ep_id = format!("{}_S01E01", series_id);

    // Seed a wanted episode (missing status, past pub_date, monitored)
    state
        .db
        .insert_episode(jumbie::db::episodes::InsertEpisodeParams {
            episode_id: &ep_id,
            series_id: &series_id,
            season: 1,
            episode: 1,
            file_path: None,
            title: Some("Test Episode"),
            quality_profile_id: None,
            status: "missing",
            meta_date: Some(chrono::NaiveDateTime::new(
                chrono::NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
                chrono::NaiveTime::from_hms_opt(0, 0, 0).unwrap(),
            )),
            est_date: None,
            metadata_ids: &std::collections::HashMap::new(),
            description: None,
            runtime: None,
            image_url: None,
            metadata_source: None,
            numbering_mode: None,
        })
        .await
        .unwrap();

    // Add a download queue entry for this episode
    sqlx::query(
        "INSERT INTO download_queue (media_name, media_link, series_title, season, episode, episode_id, score, is_user_requested, is_season_pack, category, multi_targets, status)
         VALUES (?, ?, ?, ?, ?, ?, 0, 0, 0, '', '[]', 'Queued')",
    )
    .bind("Test Release")
    .bind("magnet:?xt=urn:btih:test123")
    .bind("In Queue Show")
    .bind("1")
    .bind(1)
    .bind(&ep_id)
    .execute(state.db.get_pool())
    .await
    .unwrap();

    // Seed a second episode WITHOUT a queue entry (should stay "missing")
    let ep_id2 = format!("{}_S01E02", series_id);
    state
        .db
        .insert_episode(jumbie::db::episodes::InsertEpisodeParams {
            episode_id: &ep_id2,
            series_id: &series_id,
            season: 1,
            episode: 2,
            file_path: None,
            title: Some("Test Episode 2"),
            quality_profile_id: None,
            status: "missing",
            meta_date: Some(chrono::NaiveDateTime::new(
                chrono::NaiveDate::from_ymd_opt(2024, 1, 2).unwrap(),
                chrono::NaiveTime::from_hms_opt(0, 0, 0).unwrap(),
            )),
            est_date: None,
            metadata_ids: &std::collections::HashMap::new(),
            description: None,
            runtime: None,
            image_url: None,
            metadata_source: None,
            numbering_mode: None,
        })
        .await
        .unwrap();

    // Fetch wanted episodes
    let res = app
        .oneshot(common::get_request("/api/wanted"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let items = json["items"].as_array().unwrap();

    // Find our two episodes by episode_id
    let ep1 = items
        .iter()
        .find(|i| i["episode_id"] == ep_id)
        .expect("Episode 1 should be in wanted");
    let ep2 = items
        .iter()
        .find(|i| i["episode_id"] == ep_id2)
        .expect("Episode 2 should be in wanted");

    assert_eq!(
        ep1["status"], "in_queue",
        "Episode with queue entry should show 'in_queue', got: {}",
        ep1["status"]
    );
    assert_eq!(
        ep2["status"], "missing",
        "Episode without queue entry should show 'missing', got: {}",
        ep2["status"]
    );
}

/// Regression: age sort on the /api/wanted endpoint (WantedEpisode listing)
/// uses the computed effective date (eff_date) instead of raw meta_date.
/// If sorting by raw meta_date, an episode with meta_date=None (e.g. only
/// source/estimated) would sort before one with meta_date=Some(past).
/// With eff_date, both are computed and sorted chronologically.
/// Note: eff_date lives on the wire type `WantedEpisode`; the frontend's
/// episode-details payload is derived client-side from `SeriesDetails` and
/// never crosses the wire.
#[tokio::test]
async fn test_wanted_sort_uses_eff_date_not_raw_meta_date() {
    let (app, state, _tmp) = common::setup_test_app().await;

    let series_id = common::create_test_series(&app, "Sort Date Show").await;
    let series_id = series_id.trim_matches('"').to_string();

    // Episode A: only meta_date (2024-01-01).  eff_date = meta_date.
    let ep_a = format!("{}_S01E01", series_id);
    state
        .db
        .insert_episode(common::make_missing_episode_params(
            &ep_a,
            &series_id,
            1,
            1,
            &HashMap::new(),
        ))
        .await
        .unwrap();

    // Episode B: only est_date (2024-06-01), no meta_date.
    // eff_date = est_date (falls through metadata → source → estimated).
    // This date is MORE RECENT than A's meta_date.
    let ep_b = format!("{}_S01E02", series_id);
    state
        .db
        .insert_episode(jumbie::db::episodes::InsertEpisodeParams {
            episode_id: &ep_b,
            series_id: &series_id,
            season: 1,
            episode: 2,
            file_path: None,
            title: Some("Estimated Only"),
            quality_profile_id: None,
            status: "missing",
            meta_date: None,
            est_date: Some(chrono::NaiveDateTime::new(
                chrono::NaiveDate::from_ymd_opt(2024, 6, 1).unwrap(),
                chrono::NaiveTime::from_hms_opt(0, 0, 0).unwrap(),
            )),
            metadata_ids: &HashMap::new(),
            description: None,
            runtime: None,
            image_url: None,
            metadata_source: None,
            numbering_mode: None,
        })
        .await
        .unwrap();

    // Fetch wanted sorted by age ascending (oldest first)
    let res = app
        .oneshot(common::get_request("/api/wanted?sort=age&order=asc"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let items = json["items"].as_array().unwrap();

    // Both episodes should appear
    assert_eq!(items.len(), 2, "Expected 2 wanted episodes");

    // Ep A (2024-01-01) is older than Ep B (2024-06-01).
    // With eff_date sort: A first, B second.
    // With raw meta_date sort: B (meta_date=None sorts before Some) first, A second.
    let first_id = items[0]["episode_id"].as_str().unwrap();
    assert_eq!(
        first_id, ep_a,
        "Expected older episode (A) first when sorting by age asc, got {}. \
         If meta_date was used instead of eff_date, episode B with meta_date=None \
         would sort before episode A",
        first_id
    );

    // Also verify eff_date is populated on both items (non-empty RFC 3339 strings)
    for item in items {
        let eff = item["eff_date"].as_str().unwrap();
        assert!(
            !eff.is_empty(),
            "eff_date should be non-empty for episode {}",
            item["episode_id"]
        );
    }
}

// Validate path

#[tokio::test]
async fn test_validate_path_within_root() {
    let (app, _state, tmp) = common::setup_test_app().await;
    let organized = tmp.path().join("organized");
    let payload = ValidatePathPayload {
        path: organized.to_string_lossy().to_string(),
        series_id: None,
        resolve_collisions: false,
    };
    let res = app
        .oneshot(common::post_json_request(
            "/api/system/validate-path",
            &payload,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    // Path is within the configured destination root so it should be valid
    assert!(json["is_valid"].is_boolean());
}

#[tokio::test]
async fn test_validate_path_outside_root() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    // Directory traversal ("..") is rejected by validate_path on all platforms.
    // "/proc" is Linux-specific and on Windows resolves as a drive-relative path
    // (C:\proc); validate_path has no starts_with(root) check by design (to support
    // bind mounts and symlinked roots), so traversal is the portable guard.
    let payload = ValidatePathPayload {
        path: "../../../etc/passwd".to_string(),
        series_id: None,
        resolve_collisions: false,
    };
    let res = app
        .oneshot(common::post_json_request(
            "/api/system/validate-path",
            &payload,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["is_valid"], false);
}

#[tokio::test]
async fn test_validate_path_collision_with_series_id() {
    let (app, state, tmp) = common::setup_test_app().await;
    let organized = tmp.path().join("organized");
    let series_dir = organized.join("Existing Show");
    std::fs::create_dir_all(&series_dir).unwrap();

    // Register an existing series with this path
    let series_id = "test_series_123".to_string();
    let mapping = MappingRule {
        target_title: "Existing Show".to_string(),
        settings: SeriesSettings {
            path: Some(series_dir.to_string_lossy().to_string()),
            ..Default::default()
        },
        ..Default::default()
    };
    state
        .db
        .upsert_series_mapping(&series_id, &mapping)
        .await
        .unwrap();

    // Without series_id (a create): same path should be flagged as collision
    let payload = ValidatePathPayload {
        path: series_dir.to_string_lossy().to_string(),
        series_id: None,
        resolve_collisions: false,
    };
    let res = app
        .clone()
        .oneshot(common::post_json_request(
            "/api/system/validate-path",
            &payload,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        json["is_valid"], false,
        "Should detect collision without series_id"
    );
    assert!(
        json["message"].as_str().unwrap().contains("Existing Show"),
        "Error should mention the conflicting series"
    );

    // With the correct series_id (an update): same path should be allowed
    let payload = ValidatePathPayload {
        path: series_dir.to_string_lossy().to_string(),
        series_id: Some(series_id.clone()),
        resolve_collisions: false,
    };
    let res = app
        .clone()
        .oneshot(common::post_json_request(
            "/api/system/validate-path",
            &payload,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        json["is_valid"], true,
        "Should allow path when excluding own ID"
    );
}

#[tokio::test]
async fn test_validate_path_collision_with_wrong_series_id() {
    let (app, state, tmp) = common::setup_test_app().await;
    let organized = tmp.path().join("organized");
    let series_dir = organized.join("Other Show");
    std::fs::create_dir_all(&series_dir).unwrap();

    // Register an existing series
    let existing_id = "existing_series".to_string();
    let mapping = MappingRule {
        target_title: "Other Show".to_string(),
        settings: SeriesSettings {
            path: Some(series_dir.to_string_lossy().to_string()),
            ..Default::default()
        },
        ..Default::default()
    };
    state
        .db
        .upsert_series_mapping(&existing_id, &mapping)
        .await
        .unwrap();

    // With a *different* series_id: collision should still be detected
    let payload = ValidatePathPayload {
        path: series_dir.to_string_lossy().to_string(),
        series_id: Some("some_other_series".to_string()),
        resolve_collisions: false,
    };
    let res = app
        .clone()
        .oneshot(common::post_json_request(
            "/api/system/validate-path",
            &payload,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        json["is_valid"], false,
        "Wrong series_id should still detect collision"
    );
}

// Validate path: folder-level collision resolution (Add Series create flow)

#[tokio::test]
async fn test_validate_path_resolve_collisions_rename() {
    let (app, _state, tmp) = common::setup_test_app().await;
    let organized = tmp.path().join("organized");
    let series_dir = organized.join("Existing Folder");
    std::fs::create_dir_all(&series_dir).unwrap();

    // Default collision_handling is "rename" with DotNumeric suffix → ".001"
    let payload = ValidatePathPayload {
        path: series_dir.to_string_lossy().to_string(),
        series_id: None,
        resolve_collisions: true,
    };
    let res = app
        .oneshot(common::post_json_request(
            "/api/system/validate-path",
            &payload,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["is_valid"], true);
    let resolved = json["resolved_path"].as_str().unwrap();
    assert!(
        resolved.ends_with("Existing Folder.001"),
        "Expected a suffixed folder, got: {}",
        resolved
    );
}

#[tokio::test]
async fn test_validate_path_resolve_collisions_rename_respects_suffix_config() {
    let (app, state, tmp) = common::setup_test_app().await;
    let organized = tmp.path().join("organized");
    let series_dir = organized.join("Paren Show");
    std::fs::create_dir_all(&series_dir).unwrap();

    // Configure ParenNumeric suffix → " (1)"
    {
        let mut cfg = state.cfg.write().await;
        cfg.organization.collision_rename_suffix =
            jumbie_shared::config::organization::CollisionRenameSuffix::ParenNumeric;
    }

    let payload = ValidatePathPayload {
        path: series_dir.to_string_lossy().to_string(),
        series_id: None,
        resolve_collisions: true,
    };
    let res = app
        .oneshot(common::post_json_request(
            "/api/system/validate-path",
            &payload,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["is_valid"], true);
    let resolved = json["resolved_path"].as_str().unwrap();
    assert!(
        resolved.ends_with("Paren Show (1)"),
        "Expected paren-numeric suffixed folder, got: {}",
        resolved
    );
}

#[tokio::test]
async fn test_validate_path_resolve_collisions_skip_is_invalid() {
    let (app, state, tmp) = common::setup_test_app().await;
    let organized = tmp.path().join("organized");
    let series_dir = organized.join("Skip Show");
    std::fs::create_dir_all(&series_dir).unwrap();

    {
        let mut cfg = state.cfg.write().await;
        cfg.organization.collision_handling = "skip".to_string();
    }

    let payload = ValidatePathPayload {
        path: series_dir.to_string_lossy().to_string(),
        series_id: None,
        resolve_collisions: true,
    };
    let res = app
        .oneshot(common::post_json_request(
            "/api/system/validate-path",
            &payload,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["is_valid"], false);
    assert!(
        json["message"]
            .as_str()
            .unwrap()
            .to_lowercase()
            .contains("skip"),
        "Skip message should explain the collision handling, got: {}",
        json["message"]
    );
}

#[tokio::test]
async fn test_validate_path_resolve_collisions_overwrite_keeps_path() {
    let (app, state, tmp) = common::setup_test_app().await;
    let organized = tmp.path().join("organized");
    let series_dir = organized.join("Overwrite Show");
    std::fs::create_dir_all(&series_dir).unwrap();

    {
        let mut cfg = state.cfg.write().await;
        cfg.organization.collision_handling = "overwrite".to_string();
    }

    let payload = ValidatePathPayload {
        path: series_dir.to_string_lossy().to_string(),
        series_id: None,
        resolve_collisions: true,
    };
    let res = app
        .oneshot(common::post_json_request(
            "/api/system/validate-path",
            &payload,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["is_valid"], true);
    assert_eq!(
        json["resolved_path"].as_str().unwrap(),
        jumbie::validation::normalize_path(&series_dir).to_string_lossy(),
        "Overwrite should keep the original path"
    );
}

#[tokio::test]
async fn test_validate_path_resolve_collisions_free_path_unchanged() {
    let (app, _state, tmp) = common::setup_test_app().await;
    // The backend canonicalizes the deepest EXISTING ancestor (the temp dir —
    // macOS /var → /private/var) and re-appends the not-yet-created tail, so
    // build the expectation from the canonicalized base.
    let organized = jumbie::validation::normalize_path(tmp.path()).join("organized");
    let free_dir = organized.join("Brand New Show");
    // Do NOT create the directory — path is free

    let payload = ValidatePathPayload {
        path: free_dir.to_string_lossy().to_string(),
        series_id: None,
        resolve_collisions: true,
    };
    let res = app
        .oneshot(common::post_json_request(
            "/api/system/validate-path",
            &payload,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["is_valid"], true);
    assert_eq!(
        json["resolved_path"].as_str().unwrap(),
        jumbie::validation::normalize_path(&free_dir).to_string_lossy(),
        "Free path should be returned unchanged"
    );
}

#[tokio::test]
async fn test_validate_path_sanitizes_folder_name_per_policy() {
    // The Add Series preview must reflect the org illegal-char policy: a name
    // with filesystem-illegal characters gets sanitized in the effective path.
    let (app, _state, tmp) = common::setup_test_app().await;
    let organized = tmp.path().join("organized");
    let raw_dir = organized.join("Show: Question?");
    // Do NOT create the directory — the raw path is what the form sends.

    let payload = ValidatePathPayload {
        path: raw_dir.to_string_lossy().to_string(),
        series_id: None,
        resolve_collisions: true,
    };
    let res = app
        .oneshot(common::post_json_request(
            "/api/system/validate-path",
            &payload,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["is_valid"], true);
    let resolved = json["resolved_path"].as_str().unwrap();
    assert!(
        resolved.ends_with("Show_ Question_"),
        "Default underscore policy should sanitize `:` and `?`, got: {}",
        resolved
    );
}

// Host-legal chars differ per OS: allow_platform_specific_chars preserves chars
// valid on the CURRENT OS. On Linux/macOS `?` is legal and kept; on Windows it
// is ALWAYS illegal (NTFS), so it is filtered to `_` even with the flag enabled.
// The gating is per-OS (`PLATFORM_SAFE_CHARS`), asserted at RUNTIME so this test
// runs on every platform instead of being cfg-skipped.
#[tokio::test]
async fn test_validate_path_sanitization_respects_platform_specific_chars() {
    let on_windows = cfg!(target_os = "windows");
    let (app, state, tmp) = common::setup_test_app().await;
    let organized = tmp.path().join("organized");
    let raw_dir = organized.join("Show? Keep");

    {
        let mut cfg = state.cfg.write().await;
        cfg.organization.allow_platform_specific_chars = true;
    }

    let payload = ValidatePathPayload {
        path: raw_dir.to_string_lossy().to_string(),
        series_id: None,
        resolve_collisions: true,
    };
    let res = app
        .oneshot(common::post_json_request(
            "/api/system/validate-path",
            &payload,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["is_valid"], true);
    let resolved = json["resolved_path"].as_str().unwrap();
    if on_windows {
        assert!(
            resolved.ends_with("Show_ Keep"),
            "`?` is illegal on Windows and must be filtered even with the flag on, got: {}",
            resolved
        );
    } else {
        assert!(
            resolved.ends_with("Show? Keep"),
            "`?` is valid on the host OS and should be preserved, got: {}",
            resolved
        );
    }
}

// Remediate

#[tokio::test]
async fn test_remediate_cleanup_logs() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let payload = RemediatePayload {
        action: "cleanup_logs".to_string(),
    };
    let res = app
        .oneshot(common::post_json_request("/api/system/remediate", &payload))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_remediate_unknown_action() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let payload = RemediatePayload {
        action: "do_something_unknown".to_string(),
    };
    let res = app
        .oneshot(common::post_json_request("/api/system/remediate", &payload))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_get_health_includes_series_parent_dirs() {
    let (app, state, tmp) = common::setup_test_app().await;

    // Create a series with a path outside the default destination roots
    let custom_root = tmp.path().join("custom_root");
    let series_dir = custom_root.join("My Test Series");
    tokio::fs::create_dir_all(&series_dir).await.unwrap();

    let mut mapping = MappingRule {
        target_title: "My Test Series".to_string(),
        settings: SeriesSettings {
            path: Some(series_dir.to_string_lossy().to_string()),
            ..Default::default()
        },
        ..Default::default()
    };
    mapping.ensure_series_id();
    state
        .db
        .upsert_series_mapping("test-health-series", &mapping)
        .await
        .unwrap();

    // Also create a second series under the same parent — should dedup to one entry
    let series_dir2 = custom_root.join("Another Series");
    tokio::fs::create_dir_all(&series_dir2).await.unwrap();
    let mut mapping2 = MappingRule {
        target_title: "Another Series".to_string(),
        settings: SeriesSettings {
            path: Some(series_dir2.to_string_lossy().to_string()),
            ..Default::default()
        },
        ..Default::default()
    };
    mapping2.ensure_series_id();
    state
        .db
        .upsert_series_mapping("test-health-series-2", &mapping2)
        .await
        .unwrap();

    let res = app
        .oneshot(common::get_request("/api/system/health"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    let dir_perms = json["dir_permissions"].as_array().unwrap();
    let custom_root_str = jumbie::validation::normalize_path(&custom_root)
        .to_string_lossy()
        .to_string();

    // Find the entry for custom_root
    let entry = dir_perms
        .iter()
        .find(|e| e["path"].as_str() == Some(&custom_root_str));
    assert!(
        entry.is_some(),
        "Expected custom_root '{}' to appear in dir_permissions, got paths: {:?}",
        custom_root_str,
        dir_perms
            .iter()
            .map(|e| e["path"].as_str().unwrap_or("?"))
            .collect::<Vec<_>>(),
    );

    let entry = entry.unwrap();
    assert!(
        entry["can_read"].as_bool().unwrap(),
        "custom_root should be readable"
    );
    assert!(
        entry["can_write"].as_bool().unwrap(),
        "custom_root should be writable"
    );

    let count = dir_perms
        .iter()
        .filter(|e| e["path"].as_str() == Some(&custom_root_str))
        .count();
    assert_eq!(count, 1, "custom_root should appear exactly once (dedup)");
}

#[tokio::test]
async fn test_get_health_dir_permissions_stable_path_order() {
    // The backend builds dir_permissions from a HashSet, whose iteration order is
    // arbitrary — the endpoint must sort so clients that render in arrival order
    // (the system status page) get a stable list.
    let (app, state, tmp) = common::setup_test_app().await;

    // Seed series under two distinct parents so the checked set is non-trivial
    // even if the default destination roots collapse to one entry.
    for (id, name) in [("sort-a", "Alpha Show"), ("sort-b", "Beta Show")] {
        let series_dir = tmp.path().join("sort_root").join(name);
        tokio::fs::create_dir_all(&series_dir).await.unwrap();
        let mut mapping = MappingRule {
            target_title: name.to_string(),
            settings: SeriesSettings {
                path: Some(series_dir.to_string_lossy().to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        mapping.ensure_series_id();
        state.db.upsert_series_mapping(id, &mapping).await.unwrap();
    }

    let res = app
        .oneshot(common::get_request("/api/system/health"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    let paths: Vec<std::path::PathBuf> = json["dir_permissions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| std::path::PathBuf::from(e["path"].as_str().unwrap()))
        .collect();

    // Compare with PathBuf ordering (component-wise), which is what the backend
    // sorts by — not raw string ordering. Several shared parents are deduped, so
    // the list should be small but not vacuous.
    assert!(
        paths.len() >= 2,
        "expected multiple directory checks, got: {paths:?}"
    );
    let mut sorted = paths.clone();
    sorted.sort();
    assert_eq!(
        paths, sorted,
        "dir_permissions must be returned in stable path order"
    );
}
