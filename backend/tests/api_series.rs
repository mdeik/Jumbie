mod common;

use axum::http::StatusCode;
use jumbie_shared::{
    mapping::{MonitorMode, SeasonOverride},
    types::UpdateSeriesPayload,
};
use std::collections::HashMap;
use tower::ServiceExt;

#[tokio::test]
async fn test_create_and_update_absolute_series() {
    let (app, _state, _temp_dir) = common::setup_test_app().await;

    let create_payload = common::test_fixtures::minimal_test_request("Anime Show");

    let req = common::post_json_request("/api/series", &create_payload);

    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);

    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let series_id: String = serde_json::from_slice(&body).unwrap();
    // remove quotes if any
    let series_id = series_id.trim_matches('"').to_string();

    // 2. Update Series to be absolute
    let update_payload = UpdateSeriesPayload {
        quality_profile: "Any".to_string(),
        release_profile: "Any".to_string(),
        title: Some("Anime Show".to_string()),
        path_operation: None,
        settings: jumbie_shared::mapping::SeriesSettings {
            aliases: vec![],
            reg_patterns: vec![],
            season: HashMap::new(),
            season_absolute: HashMap::new(),
            season_folder_format: None,
            episode_file_format: None,
            season_folder_format_absolute: None,
            episode_file_format_absolute: None,
            flatten_season_folders: None,
            absolute_numbering: Some(true),
            rename_episodes: None,
            search_format: None,
            search_format_absolute: None,
            path: Some("Anime Show".to_string()),
            monitor_mode: Some(MonitorMode::All),
            metadata_ids: HashMap::new(),
            metadata_last_synced_at: HashMap::new(),
            last_known_dir_mtimes: HashMap::new(),
        },
    };

    let req = common::put_json_request(&format!("/api/series/{}", series_id), &update_payload);

    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_create_and_update_series_with_offset() {
    let (app, state, _temp_dir) = common::setup_test_app().await;

    let create_payload = common::test_fixtures::minimal_test_request("Offset Show");

    let req = common::post_json_request("/api/series", &create_payload);

    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);

    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let series_id: String = serde_json::from_slice(&body).unwrap();
    let series_id = series_id.trim_matches('"').to_string();

    // Deliberately exhaustive (no `..Default::default()`): `update_series` copies
    // settings field-by-field, so a field it forgets is silently dropped on save.
    // Listing every field here makes adding a `SeriesSettings` field a compile error.
    let mut seasons_map = HashMap::new();
    seasons_map.insert(
        "1".to_string(),
        SeasonOverride {
            season: "1".to_string(),
            episode_start: None,
            episode_end: None,
            cell_count: None,
            episode_offset: Some(10),
            alias_season_number: None,
            search_format: None,
            aliases: vec![],
            reg_patterns: vec![],
        },
    );
    let mut season_absolute = HashMap::new();
    season_absolute.insert(
        "1".to_string(),
        SeasonOverride {
            season: "1".to_string(),
            episode_start: None,
            episode_end: None,
            cell_count: Some(24),
            episode_offset: None,
            alias_season_number: Some(2),
            search_format: None,
            aliases: vec![],
            reg_patterns: vec![],
        },
    );
    let mut metadata_ids = HashMap::new();
    metadata_ids.insert("plugin-1".to_string(), "meta-1".to_string());

    let update_payload = UpdateSeriesPayload {
        quality_profile: "Any".to_string(),
        release_profile: "Any".to_string(),
        title: Some("Offset Show".to_string()),
        path_operation: None,
        settings: jumbie_shared::mapping::SeriesSettings {
            // Duplicate + blank entries must be dropped on save.
            aliases: vec![
                "Alias A".to_string(),
                String::new(),
                "Alias A".to_string(),
                "Alias B".to_string(),
            ],
            reg_patterns: vec!["(?i)offset".to_string()],
            season: seasons_map,
            season_absolute,
            season_folder_format: Some("Season {season}".to_string()),
            episode_file_format: Some("Ep {episode}".to_string()),
            season_folder_format_absolute: Some("Abs {season}".to_string()),
            episode_file_format_absolute: Some("AbsEp {episode}".to_string()),
            flatten_season_folders: Some(true),
            absolute_numbering: Some(false),
            rename_episodes: Some(false),
            search_format: Some("S${season:02}E${episode:02}".to_string()),
            search_format_absolute: Some("E${episode:02}".to_string()),
            path: Some("Offset Show".to_string()),
            monitor_mode: Some(MonitorMode::All),
            metadata_ids,
            // Server-owned fields — never taken from the payload.
            metadata_last_synced_at: HashMap::new(),
            last_known_dir_mtimes: HashMap::new(),
        },
    };

    let req = common::put_json_request(&format!("/api/series/{}", series_id), &update_payload);

    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // 3. Every editable setting must survive the DB round-trip. The field-by-field
    //    assertions mirror `update_series`; a drop shows up as a mismatch here.
    let mapping = state
        .db
        .get_series_mapping(&series_id)
        .await
        .expect("db read ok")
        .expect("mapping exists");
    let s = &mapping.settings;

    assert_eq!(
        s.aliases,
        vec!["Alias A".to_string(), "Alias B".to_string()],
        "aliases must persist, deduplicated and blank-filtered"
    );
    assert_eq!(s.reg_patterns, vec!["(?i)offset".to_string()]);
    assert_eq!(
        s.season
            .get("1")
            .and_then(|o| o.episode_offset)
            .unwrap_or_default(),
        10,
        "normal-mode season offset must persist"
    );
    assert_eq!(
        s.season_absolute
            .get("1")
            .and_then(|o| o.cell_count)
            .unwrap_or_default(),
        24,
        "absolute-mode season override must persist independently"
    );
    assert_eq!(s.season_folder_format.as_deref(), Some("Season {season}"));
    assert_eq!(s.episode_file_format.as_deref(), Some("Ep {episode}"));
    assert_eq!(
        s.season_folder_format_absolute.as_deref(),
        Some("Abs {season}")
    );
    assert_eq!(
        s.episode_file_format_absolute.as_deref(),
        Some("AbsEp {episode}")
    );
    assert_eq!(s.flatten_season_folders, Some(true));
    assert_eq!(s.absolute_numbering, Some(false));
    assert_eq!(s.rename_episodes, Some(false));
    assert_eq!(
        s.search_format.as_deref(),
        Some("S${season:02}E${episode:02}"),
        "series-level search_format must persist"
    );
    assert_eq!(
        s.search_format_absolute.as_deref(),
        Some("E${episode:02}"),
        "series-level search_format_absolute must persist"
    );
    assert_eq!(s.monitor_mode, Some(MonitorMode::All));
    assert_eq!(
        s.metadata_ids.get("plugin-1").map(String::as_str),
        Some("meta-1")
    );
}
