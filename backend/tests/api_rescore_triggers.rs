// Retroactive Rescore: Trigger Integration Tests
//
// Tests that the three trigger points actually rescore episodes when a release
// profile changes:

mod common;

use axum::http::StatusCode;
use jumbie_shared::scoring::ReleaseProfile;
use jumbie_shared::types::CreateSeriesRequest;
use std::collections::HashMap;

/// Create a release profile with known terms and return its UUID.
///
/// Helper to set up a test file, fingerprint, and release_metadata for an episode
/// so the rescore pipeline can find it (via file_path → fingerprint → release_metadata).
async fn setup_episode_for_rescore(
    db: &jumbie::db::DbManager,
    tmp_dir: &tempfile::TempDir,
    episode_id: &str,
    release_title: &str,
) {
    let file_path = tmp_dir.path().join(format!("{}.mkv", episode_id));
    tokio::fs::write(&file_path, b"test content").await.unwrap();
    let meta = tokio::fs::metadata(&file_path).await.unwrap();
    let (inode, dev, mtime) = jumbie::platform::file_identity(&meta);
    let hash = format!("rescore_hash_{}", episode_id.replace('-', "_"));
    db.save_fingerprint(jumbie::db::files::SaveFingerprintParams {
        path: file_path.to_str().unwrap(),
        inode,
        dev,
        size: meta.len(),
        mtime,
        quick_hash: &hash,
        state: "complete",
        media_info: None::<&jumbie_shared::types::MediaInfo>,
    })
    .await
    .unwrap();
    db.set_release_info(&hash, Some(release_title), None, None)
        .await
        .unwrap();
    db.link_file_episode(file_path.to_str().unwrap(), episode_id)
        .await
        .unwrap();
    db.associate_main_file(episode_id, file_path.to_str().unwrap(), None)
        .await
        .unwrap();
}

async fn create_profile(
    app: &axum::Router,
    uuid: &str,
    name: &str,
    terms: HashMap<String, i32>,
    min_score: i32,
) {
    let profile = ReleaseProfile {
        name: name.to_string(),
        terms,
        min_score,
        ..Default::default()
    };
    let mut profiles = HashMap::new();
    profiles.insert(uuid.to_string(), profile);
    let (status, _) = common::put_json(app, "/api/config/release_profiles", &profiles).await;
    assert_eq!(status, StatusCode::OK, "Failed to create profile {}", name);
}

#[tokio::test]
async fn test_global_profile_save_triggers_rescore() {
    let (app, state, _tmp) = common::setup_test_app().await;

    // Create a profile
    let mut terms = HashMap::new();
    terms.insert("1080p".to_string(), 10);
    terms.insert("BluRay".to_string(), 15);
    let profile_uuid = "test-global-trigger-uuid";
    create_profile(&app, profile_uuid, "Trigger Test", terms, 0).await;

    // Create a series using this profile
    let create_payload = CreateSeriesRequest {
        path: "Global Trigger Show".to_string(),
        series_name: Some("Display Name".to_string()),
        quality_profile: Some(profile_uuid.to_string()),
        scan_for_existing: None,
        monitor_mode: None,
        release_profile: None,
        metadata_ids: HashMap::new(),
        search_missing_on_add: false,
        resolve_collisions: true,
        settings: Default::default(),
    };
    let (status, body) = common::post_json(&app, "/api/series", &create_payload).await;
    assert_eq!(status, StatusCode::CREATED);
    let series_id = body.as_str().unwrap().to_string();

    // Insert an episode with release_title directly via DB
    let empty_map = HashMap::new();
    state
        .db
        .insert_episode(jumbie::db::episodes::InsertEpisodeParams {
            episode_id: &format!("{}_S01E01", series_id),
            series_id: &series_id,
            season: 1,
            episode: 1,
            file_path: None,
            title: Some("Episode 1"),
            quality_profile_id: None,
            status: "organized",
            meta_date: None,
            est_date: None,
            metadata_ids: &empty_map,
            description: None,
            runtime: None,
            image_url: None,
            metadata_source: None,
            numbering_mode: None,
        })
        .await
        .unwrap();

    // Set up fingerprint + release_metadata so the rescore pipeline can find this episode
    setup_episode_for_rescore(
        &state.db,
        &_tmp,
        &format!("{}_S01E01", series_id),
        "[Group] Trigger Show S01E01 [1080p][BluRay]",
    )
    .await;

    state
        .db
        .rescore_episodes_for_profiles(std::slice::from_ref(&series_id))
        .await
        .unwrap();
    let ep = state
        .db
        .get_episode_by_id(&format!("{}_S01E01", series_id))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ep.score, Some(25), "Direct rescore should produce 25");

    // Update the profile via API (increase 1080p from 10 → 20)
    let mut new_terms = HashMap::new();
    new_terms.insert("1080p".to_string(), 20);
    new_terms.insert("BluRay".to_string(), 15);
    let updated_profile = ReleaseProfile {
        name: "Trigger Test".to_string(),
        terms: new_terms,
        min_score: 0,
        ..Default::default()
    };
    let mut profiles = HashMap::new();
    profiles.insert(profile_uuid.to_string(), updated_profile);

    // Small delay to let any async rescope complete
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    let (status, _) = common::put_json(&app, "/api/config/release_profiles", &profiles).await;
    assert_eq!(status, StatusCode::OK);

    // Wait for async rescore to complete
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let ep = state
        .db
        .get_episode_by_id(&format!("{}_S01E01", series_id))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        ep.score,
        Some(35),
        "1080p(20) + BluRay(15) = 35 after rescore"
    );
}

#[tokio::test]
async fn test_series_update_triggers_rescore() {
    let (app, state, _tmp) = common::setup_test_app().await;

    // Create two profiles
    let mut terms_a = HashMap::new();
    terms_a.insert("1080p".to_string(), 10);
    let uuid_a = "profile-a-trigger-uuid";
    create_profile(&app, uuid_a, "Profile A", terms_a, 0).await;

    let mut terms_b = HashMap::new();
    terms_b.insert("1080p".to_string(), 50);
    terms_b.insert("hevc".to_string(), 10);
    let uuid_b = "profile-b-trigger-uuid";
    create_profile(&app, uuid_b, "Profile B", terms_b, 0).await;

    // Create series with profile A
    let create_payload = CreateSeriesRequest {
        path: "Series Update Show".to_string(),
        series_name: Some("Display Name".to_string()),
        quality_profile: Some(uuid_a.to_string()),
        scan_for_existing: None,
        monitor_mode: None,
        release_profile: None,
        metadata_ids: HashMap::new(),
        search_missing_on_add: false,
        resolve_collisions: true,
        settings: Default::default(),
    };
    let (status, body) = common::post_json(&app, "/api/series", &create_payload).await;
    assert_eq!(status, StatusCode::CREATED);
    let series_id = body.as_str().unwrap().to_string();

    // Insert episodes
    let empty_map = HashMap::new();
    state
        .db
        .insert_episode(jumbie::db::episodes::InsertEpisodeParams {
            episode_id: &format!("{}_S01E01", series_id),
            series_id: &series_id,
            season: 1,
            episode: 1,
            file_path: None,
            title: Some("Ep1"),
            quality_profile_id: None,
            status: "organized",
            meta_date: None,
            est_date: None,
            metadata_ids: &empty_map,
            description: None,
            runtime: None,
            image_url: None,
            metadata_source: None,
            numbering_mode: None,
        })
        .await
        .unwrap();

    // Set up fingerprint + release_metadata so the rescore pipeline can find this episode
    setup_episode_for_rescore(
        &state.db,
        &_tmp,
        &format!("{}_S01E01", series_id),
        "[Group] Trigger Show B S01E01 [1080p][hevc]",
    )
    .await;

    // Switch series to Profile B via update_series API
    let update_payload = serde_json::json!({
        "quality_profile": "Any",
        "release_profile": uuid_b,
    });
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let (status, _) =
        common::put_json(&app, &format!("/api/series/{}", series_id), &update_payload).await;
    assert_eq!(status, StatusCode::OK);

    // Wait for async rescore
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let ep = state
        .db
        .get_episode_by_id(&format!("{}_S01E01", series_id))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        ep.score,
        Some(60),
        "1080p(50) + hevc(10) = 60 after profile switch"
    );
}

#[tokio::test]
async fn test_profile_save_without_change_does_not_break_scores() {
    let (app, state, _tmp) = common::setup_test_app().await;

    // Create a profile
    let mut terms = HashMap::new();
    terms.insert("1080p".to_string(), 10);
    let profile_uuid = "nochange-trigger-uuid";
    create_profile(&app, profile_uuid, "No Change", terms, 0).await;

    // Create series
    let create_payload = CreateSeriesRequest {
        path: "No Change Show".to_string(),
        series_name: Some("Display Name".to_string()),
        quality_profile: Some(profile_uuid.to_string()),
        scan_for_existing: None,
        monitor_mode: None,
        release_profile: None,
        metadata_ids: HashMap::new(),
        search_missing_on_add: false,
        resolve_collisions: true,
        settings: Default::default(),
    };
    let (status, body) = common::post_json(&app, "/api/series", &create_payload).await;
    assert_eq!(status, StatusCode::CREATED);
    let series_id = body.as_str().unwrap().to_string();

    // Insert episode
    let empty_map = HashMap::new();
    state
        .db
        .insert_episode(jumbie::db::episodes::InsertEpisodeParams {
            episode_id: &format!("{}_S01E01", series_id),
            series_id: &series_id,
            season: 1,
            episode: 1,
            file_path: None,
            title: Some("Ep1"),
            quality_profile_id: None,
            status: "organized",
            meta_date: None,
            est_date: None,
            metadata_ids: &empty_map,
            description: None,
            runtime: None,
            image_url: None,
            metadata_source: None,
            numbering_mode: None,
        })
        .await
        .unwrap();

    // Set up fingerprint + release_metadata so the rescore pipeline can find this episode
    setup_episode_for_rescore(
        &state.db,
        &_tmp,
        &format!("{}_S01E01", series_id),
        "[Group] No Change Show S01E01 [1080p]",
    )
    .await;

    // Save the SAME profile again
    let same_profile = ReleaseProfile {
        name: "No Change".to_string(),
        terms: {
            let mut t = HashMap::new();
            t.insert("1080p".to_string(), 10);
            t
        },
        min_score: 0,
        ..Default::default()
    };
    let mut profiles = HashMap::new();
    profiles.insert(profile_uuid.to_string(), same_profile);

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let (status, _) = common::put_json(&app, "/api/config/release_profiles", &profiles).await;
    assert_eq!(status, StatusCode::OK);

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // Score should remain the same (idempotent)
    let ep = state
        .db
        .get_episode_by_id(&format!("{}_S01E01", series_id))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ep.score, Some(10), "Score unchanged when profile unchanged");
}

#[tokio::test]
async fn test_batch_edit_triggers_rescore() {
    let (app, state, _tmp) = common::setup_test_app().await;

    // Create two profiles
    let mut terms_a = HashMap::new();
    terms_a.insert("1080p".to_string(), 10);
    let uuid_old = "batch-old-uuid";
    create_profile(&app, uuid_old, "Batch Old", terms_a, 0).await;

    let mut terms_b = HashMap::new();
    terms_b.insert("1080p".to_string(), 99);
    let uuid_new = "batch-new-uuid";
    create_profile(&app, uuid_new, "Batch New", terms_b, 0).await;

    // Create series with old profile
    let create_payload = CreateSeriesRequest {
        path: "Batch Edit Show".to_string(),
        series_name: Some("Display Name".to_string()),
        quality_profile: Some(uuid_old.to_string()),
        scan_for_existing: None,
        monitor_mode: None,
        release_profile: None,
        metadata_ids: HashMap::new(),
        search_missing_on_add: false,
        resolve_collisions: true,
        settings: Default::default(),
    };
    let (status, body) = common::post_json(&app, "/api/series", &create_payload).await;
    assert_eq!(status, StatusCode::CREATED);
    let series_id = body.as_str().unwrap().to_string();

    // Insert episode
    let empty_map = HashMap::new();
    state
        .db
        .insert_episode(jumbie::db::episodes::InsertEpisodeParams {
            episode_id: &format!("{}_S01E01", series_id),
            series_id: &series_id,
            season: 1,
            episode: 1,
            file_path: None,
            title: Some("Ep1"),
            quality_profile_id: None,
            status: "organized",
            meta_date: None,
            est_date: None,
            metadata_ids: &empty_map,
            description: None,
            runtime: None,
            image_url: None,
            metadata_source: None,
            numbering_mode: None,
        })
        .await
        .unwrap();

    // Set up fingerprint + release_metadata so the rescore pipeline can find this episode
    setup_episode_for_rescore(
        &state.db,
        &_tmp,
        &format!("{}_S01E01", series_id),
        "[Group] Batch Show S01E01 [1080p]",
    )
    .await;

    // Batch edit: switch to new profile
    let batch_payload = serde_json::json!({
        "series_ids": [series_id],
        "release_profile": uuid_new,
    });
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let (status, _) = common::post_json(&app, "/api/series/batch-edit", &batch_payload).await;
    assert_eq!(status, StatusCode::OK);

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let ep = state
        .db
        .get_episode_by_id(&format!("{}_S01E01", series_id))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        ep.score,
        Some(99),
        "1080p(99) after batch edit profile switch"
    );
}
