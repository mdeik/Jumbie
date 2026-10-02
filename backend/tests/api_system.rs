mod common;

use axum::http::StatusCode;
use jumbie_shared::types::ReorganizeAllPayload;
use tower::ServiceExt;

#[tokio::test]
async fn test_file_rename_queue() {
    let (app, state, _temp_dir) = common::setup_test_app().await;

    let create_payload = common::test_fixtures::minimal_test_request("Rename Show");

    let req = common::post_json_request("/api/series", &create_payload);

    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);

    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let series_id: String = serde_json::from_slice(&body).unwrap();
    let series_id = series_id.trim_matches('"').to_string();

    // Add a dummy episode to the database so there is something to rename.
    state
        .db
        .insert_episode(jumbie::db::episodes::InsertEpisodeParams {
            episode_id: "Rename Show_S1E01",
            series_id: &series_id,
            season: 1,
            episode: 1,
            file_path: Some("/dummy/path/Rename Show - 1x1.mkv"),
            title: Some("Pilot"),
            quality_profile_id: None,
            status: "organized",
            meta_date: None,
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

    let reorg_payload = ReorganizeAllPayload {};

    let req = common::post_json_request("/api/series/actions/reorganize_all", &reorg_payload);

    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let req = common::get_request("/api/system/rename_queue");

    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // The dummy file is not on disk, so the rename tool may emit no renames or
    // log an error; this only asserts the endpoint does not fail.
}
