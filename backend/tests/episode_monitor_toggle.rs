mod common;

use axum::http::StatusCode;
use common::{post_json, put_json, setup_test_app};

/// PUT /api/episodes/{id}/monitor toggles the monitored flag via a direct DB
/// write; it does NOT call reapply_monitor_for_series. Periodic sweeps
/// (fresh_apply=false) preserve manual toggles; explicit mode changes
/// (fresh_apply=true) reset them.
#[tokio::test]
async fn test_toggle_episode_monitor() {
    let (app, state, _temp_dir) = setup_test_app().await;

    let series_id = "ts-toggle-001";
    let episode_id = "ts-toggle-001_S01E01";

    let mapping = jumbie_shared::types::MappingRule {
        series_id: series_id.to_string(),
        target_title: "Toggle Test Series".to_string(),
        name: "toggle-test-series".to_string(),
        settings: jumbie_shared::types::SeriesSettings {
            monitor_mode: Some(jumbie_shared::types::MonitorMode::Future),
            ..Default::default()
        },
        ..Default::default()
    };

    state
        .db
        .upsert_series_mapping(series_id, &mapping)
        .await
        .expect("Failed to upsert series mapping");

    // Insert episode with monitored=true (default)
    let params = jumbie::db::episodes::InsertEpisodeParams {
        episode_id,
        series_id,
        season: 1,
        episode: 1,
        file_path: None,
        status: "missing",
        title: Some("Test Episode"),
        quality_profile_id: None,
        numbering_mode: Some(0),
        meta_date: None,
        est_date: None,
        metadata_ids: &std::collections::HashMap::new(),
        description: None,
        runtime: None,
        image_url: None,
        metadata_source: None,
    };
    state
        .db
        .insert_episode(params)
        .await
        .expect("Failed to insert episode");

    let rows = state
        .db
        .get_series_episodes_details(series_id, false)
        .await
        .expect("Failed to fetch episodes");
    assert_eq!(rows.len(), 1);
    assert!(rows[0].monitored, "Episode should start as monitored");

    let (status, _) = put_json(
        &app,
        &format!("/api/episodes/{}/monitor", episode_id),
        &jumbie_shared::types::EpisodeMonitorTogglePayload { monitored: false },
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let rows = state
        .db
        .get_series_episodes_details(series_id, false)
        .await
        .expect("Failed to fetch episodes");
    assert!(
        !rows[0].monitored,
        "Episode should be unmonitored after toggle"
    );

    let (status, _) = put_json(
        &app,
        &format!("/api/episodes/{}/monitor", episode_id),
        &jumbie_shared::types::EpisodeMonitorTogglePayload { monitored: true },
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let rows = state
        .db
        .get_series_episodes_details(series_id, false)
        .await
        .expect("Failed to fetch episodes");
    assert!(
        rows[0].monitored,
        "Episode should be monitored after second toggle"
    );

    // The DB just updates 0 rows, so the endpoint still returns OK: the episodes
    // table uses a TEXT primary key and a no-match SQLite UPDATE is a no-op.
    let (status, _) = put_json(
        &app,
        "/api/episodes/nonexistent/monitor",
        &jumbie_shared::types::EpisodeMonitorTogglePayload { monitored: false },
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

/// A manual toggle must survive a sweep (fresh_apply=false), which is what the
/// startup and periodic 60-minute sweeps do: a hand-toggled episode stays as set.
#[tokio::test]
async fn test_manual_toggle_survives_sweep() {
    let (app, state, _temp_dir) = setup_test_app().await;

    let series_id = "ts-sweep-toggle-001";
    let episode_id = "ts-sweep-toggle-001_S01E01";

    let mapping = jumbie_shared::types::MappingRule {
        series_id: series_id.to_string(),
        target_title: "Sweep Toggle Test".to_string(),
        name: "sweep-toggle-test".to_string(),
        settings: jumbie_shared::types::SeriesSettings {
            monitor_mode: Some(jumbie_shared::types::MonitorMode::Future),
            ..Default::default()
        },
        ..Default::default()
    };

    state
        .db
        .upsert_series_mapping(series_id, &mapping)
        .await
        .expect("Failed to upsert series mapping");

    // Insert episode with past date, no file, monitored=false (manually unmonitored)
    let past = chrono::Utc::now() - chrono::Duration::days(7);
    let params = jumbie::db::episodes::InsertEpisodeParams {
        episode_id,
        series_id,
        season: 1,
        episode: 1,
        file_path: None,
        status: "missing",
        title: None,
        quality_profile_id: None,
        numbering_mode: Some(0),
        meta_date: Some(past.naive_utc()),
        est_date: None,
        metadata_ids: &std::collections::HashMap::new(),
        description: None,
        runtime: None,
        image_url: None,
        metadata_source: None,
    };
    state
        .db
        .insert_episode(params)
        .await
        .expect("Failed to insert episode");

    // Manually unmonitor via the toggle endpoint
    let (status, _) = put_json(
        &app,
        &format!("/api/episodes/{}/monitor", episode_id),
        &jumbie_shared::types::EpisodeMonitorTogglePayload { monitored: false },
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let rows = state
        .db
        .get_series_episodes_details(series_id, false)
        .await
        .unwrap();
    assert!(
        !rows[0].monitored,
        "Episode should start unmonitored after toggle"
    );

    // Simulate a sweep (fresh_apply=false), as the startup and periodic sweeps do.
    jumbie::api_routes::series::batch_apply_monitor_mode(
        &state,
        &[series_id.to_string()],
        jumbie_shared::types::MonitorMode::Future,
        false, // sweep: preserve existing state
    )
    .await;

    let rows = state
        .db
        .get_series_episodes_details(series_id, false)
        .await
        .unwrap();
    assert!(
        !rows[0].monitored,
        "Manual unmonitor should survive sweep (fresh_apply=false preserves existing state)"
    );

    // Simulate a mode change (fresh_apply=true), as the API does when the user
    // explicitly changes the series' mode.
    jumbie::api_routes::series::batch_apply_monitor_mode(
        &state,
        &[series_id.to_string()],
        jumbie_shared::types::MonitorMode::Future,
        true, // fresh apply: ignore existing state
    )
    .await;

    // Under fresh_apply=true, currently_monitored=false for all past-date
    // episodes with no file.
    let rows = state
        .db
        .get_series_episodes_details(series_id, false)
        .await
        .unwrap();
    assert!(
        !rows[0].monitored,
        "Fresh apply should leave past-date episodes unmonitored (currently_monitored=false)"
    );

    // Manually re-monitor, then verify the sweep preserves it.
    let (status, _) = put_json(
        &app,
        &format!("/api/episodes/{}/monitor", episode_id),
        &jumbie_shared::types::EpisodeMonitorTogglePayload { monitored: true },
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    jumbie::api_routes::series::batch_apply_monitor_mode(
        &state,
        &[series_id.to_string()],
        jumbie_shared::types::MonitorMode::Future,
        false, // sweep: preserve
    )
    .await;

    let rows = state
        .db
        .get_series_episodes_details(series_id, false)
        .await
        .unwrap();
    assert!(
        rows[0].monitored,
        "Manual re-monitor should survive sweep (fresh_apply=false preserves db_row.monitored)"
    );
}

/// POST /api/episodes/batch-monitor batch-sets the monitored flag for a list of
/// episode IDs via a direct DB write (no monitor-mode re-evaluation). The flag
/// survives periodic sweeps (fresh_apply=false) but is reset on explicit mode
/// change.
#[tokio::test]
async fn test_batch_monitor_episodes() {
    let (app, state, _temp_dir) = setup_test_app().await;

    let series_id = "ts-batch-mon-001";

    let mapping = jumbie_shared::types::MappingRule {
        series_id: series_id.to_string(),
        target_title: "Batch Monitor Test".to_string(),
        name: "batch-mon-test".to_string(),
        ..Default::default()
    };
    state
        .db
        .upsert_series_mapping(series_id, &mapping)
        .await
        .expect("Failed to upsert series mapping");

    // Insert 3 episodes, all starting as monitored=true (default)
    let metadata_ids = std::collections::HashMap::new();
    let ids_01: Vec<String> = (1..=3)
        .map(|ep| format!("{}_S01E{:02}", series_id, ep))
        .collect();
    for eid in &ids_01 {
        let params = common::make_missing_episode_params(eid, series_id, 1, 1, &metadata_ids);
        state.db.insert_episode(params).await.unwrap();
    }

    // Insert 1 episode from another season to verify isolation
    let eid_other = format!("{}_S02E01", series_id);
    state
        .db
        .insert_episode(common::make_missing_episode_params(
            &eid_other,
            series_id,
            2,
            1,
            &metadata_ids,
        ))
        .await
        .unwrap();

    // Collect the IDs of season 01 episodes (ones we'll toggle)
    let all_episode_ids: Vec<String> = (1..=3)
        .map(|ep| format!("{}_S01E{:02}", series_id, ep))
        .collect();

    let rows = state
        .db
        .get_series_episodes_details(series_id, false)
        .await
        .unwrap();
    for row in &rows {
        assert!(
            row.monitored,
            "Episode {} should start monitored",
            row.episode_id
        );
    }

    let (status, _) = post_json(
        &app,
        "/api/episodes/batch-monitor",
        &jumbie_shared::types::BatchMonitorEpisodesPayload {
            ids: all_episode_ids.clone(),
            monitored: false,
        },
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Targeted episodes are unmonitored; the other season is unaffected.
    let rows = state
        .db
        .get_series_episodes_details(series_id, false)
        .await
        .unwrap();
    for row in &rows {
        if row.episode_id == eid_other {
            assert!(row.monitored, "Untargeted episode should remain monitored");
        } else {
            assert!(
                !row.monitored,
                "Targeted episode {} should be unmonitored",
                row.episode_id
            );
        }
    }

    let (status, _) = post_json(
        &app,
        "/api/episodes/batch-monitor",
        &jumbie_shared::types::BatchMonitorEpisodesPayload {
            ids: all_episode_ids.clone(),
            monitored: true,
        },
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let rows = state
        .db
        .get_series_episodes_details(series_id, false)
        .await
        .unwrap();
    for row in &rows {
        assert!(
            row.monitored,
            "All episodes should be monitored after re-enable"
        );
    }

    // An empty IDs list is a no-op, not an error.
    let (status, _) = post_json(
        &app,
        "/api/episodes/batch-monitor",
        &jumbie_shared::types::BatchMonitorEpisodesPayload {
            ids: vec![],
            monitored: true,
        },
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}
