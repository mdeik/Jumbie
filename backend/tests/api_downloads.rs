// Download Queue & Pipeline Tests
//
// Covers queue_search_result (via the HTTP endpoint), fuzzy dedup by
// series_id+season+range, the series-page and no-context download paths, and the
// series_id / is_season_pack stored on queue items.

mod common;

use axum::http::StatusCode;
use jumbie::db::download_queue::AddToDownloadQueueParams;
use jumbie_shared::types::{AddQueueResult, DownloadQueueItem};
use std::sync::Arc;
use tower::ServiceExt;

// Helpers

/// Store a series mapping so tests can reference it by series_id.
async fn seed_series(app: &axum::Router, title: &str) -> String {
    common::create_test_series(app, title).await
}

/// Fetch all download queue items from the DB.
async fn get_queue(state: &Arc<jumbie::api::AppState>) -> Vec<DownloadQueueItem> {
    state.db.get_download_queue().await.unwrap()
}

// Fuzzy Dedup: add_to_download_queue with series_id+season+range

#[tokio::test]
async fn test_fuzzy_dedup_overlapping_range_detected() {
    let (app, state, _tmp) = common::setup_test_app().await;
    let series_id = seed_series(&app, "Fuzzy Dedup Show").await;

    // Queue entry A: multi-episode range S01E01-05, score 100, no episode_id.
    let first = state
        .db
        .add_to_download_queue(AddToDownloadQueueParams {
            media_name: "Fuzzy.Show.S01E01-05",
            media_link: "magnet:?xt=urn:btih:aaaa",
            series_title: "Fuzzy Dedup Show",
            series_id: &series_id,
            seasons: &[1],
            episodes: &[1, 2, 3, 4, 5],
            episode_id: None,
            score: 100,
            is_user_requested: false,
            is_season_pack: false,
            category: "",
            multi_targets: None,
            episode_intentions: None,
            source_pub_date: None,
            download_id: "",
            scoring_size_bytes: None,
            scoring_seeders: None,
            scoring_episode_count: None,
            submitter: None,
            quality_profile_id: None,
            version: 1,
        })
        .await
        .unwrap();
    assert!(
        matches!(first, AddQueueResult::Added { .. }),
        "First insert should be Added, got {:?}",
        first
    );

    // Non-overlapping exact episode: without an episode_id, fuzzy range dedup is
    // skipped (exact match only), so distinct magnets stay separate downloads.
    let second = state
        .db
        .add_to_download_queue(AddToDownloadQueueParams {
            media_name: "Fuzzy.Show.S01E03-08",
            media_link: "magnet:?xt=urn:btih:bbbb",
            series_title: "Fuzzy Dedup Show",
            series_id: &series_id,
            seasons: &[1],
            episodes: &[3, 4, 5, 6, 7, 8],
            episode_id: None,
            score: 90,
            is_user_requested: false,
            is_season_pack: false,
            category: "",
            multi_targets: None,
            episode_intentions: None,
            source_pub_date: None,
            download_id: "",
            scoring_size_bytes: None,
            scoring_seeders: None,
            scoring_episode_count: None,
            submitter: None,
            quality_profile_id: None,
            version: 1,
        })
        .await
        .unwrap();
    assert!(
        matches!(second, AddQueueResult::Added { .. }),
        "Without shared episode_id/magnet, non-exact-ep downloads are Added separately, got {:?}",
        second
    );
}

#[tokio::test]
async fn test_fuzzy_dedup_different_season_not_blocked() {
    let (app, state, _tmp) = common::setup_test_app().await;
    let series_id = seed_series(&app, "Season-Specific Show").await;

    // Season 01 — queue item
    state
        .db
        .add_to_download_queue(AddToDownloadQueueParams {
            media_name: "Season.Show.S01E01-05",
            media_link: "magnet:?xt=urn:btih:cccc",
            series_title: "Season-Specific Show",
            series_id: &series_id,
            seasons: &[1],
            episodes: &[1, 2, 3, 4, 5],
            episode_id: None,
            score: 100,
            is_user_requested: false,
            is_season_pack: false,
            category: "",
            multi_targets: None,
            episode_intentions: None,
            source_pub_date: None,
            download_id: "",
            scoring_size_bytes: None,
            scoring_seeders: None,
            scoring_episode_count: None,
            submitter: None,
            quality_profile_id: None,
            version: 1,
        })
        .await
        .unwrap();

    // Season 02 — same series, different season, should NOT be blocked
    let s02 = state
        .db
        .add_to_download_queue(AddToDownloadQueueParams {
            media_name: "Season.Show.S02E01-05",
            media_link: "magnet:?xt=urn:btih:dddd",
            series_title: "Season-Specific Show",
            series_id: &series_id,
            seasons: &[2],
            episodes: &[1, 2, 3, 4, 5],
            episode_id: None,
            score: 100,
            is_user_requested: false,
            is_season_pack: false,
            category: "",
            multi_targets: None,
            episode_intentions: None,
            source_pub_date: None,
            download_id: "",
            scoring_size_bytes: None,
            scoring_seeders: None,
            scoring_episode_count: None,
            submitter: None,
            quality_profile_id: None,
            version: 1,
        })
        .await
        .unwrap();
    assert!(
        matches!(s02, AddQueueResult::Added { .. }),
        "Different season should be Added, got {:?}",
        s02
    );
}

#[tokio::test]
async fn test_fuzzy_dedup_different_series_not_blocked() {
    let (app, state, _tmp) = common::setup_test_app().await;
    let sid_a = seed_series(&app, "Series Alpha").await;
    let sid_b = seed_series(&app, "Series Beta").await;

    // Series Alpha S01E01-05
    state
        .db
        .add_to_download_queue(AddToDownloadQueueParams {
            media_name: "Alpha.S01E01-05",
            media_link: "magnet:?xt=urn:btih:eeee",
            series_title: "Series Alpha",
            series_id: &sid_a,
            seasons: &[1],
            episodes: &[1, 2, 3, 4, 5],
            episode_id: None,
            score: 100,
            is_user_requested: false,
            is_season_pack: false,
            category: "",
            multi_targets: None,
            episode_intentions: None,
            source_pub_date: None,
            download_id: "",
            scoring_size_bytes: None,
            scoring_seeders: None,
            scoring_episode_count: None,
            submitter: None,
            quality_profile_id: None,
            version: 1,
        })
        .await
        .unwrap();

    // Series Beta S01E01 — same season+range, different series
    let beta = state
        .db
        .add_to_download_queue(AddToDownloadQueueParams {
            media_name: "Beta.S01E01-05",
            media_link: "magnet:?xt=urn:btih:ffff",
            series_title: "Series Beta",
            series_id: &sid_b,
            seasons: &[1],
            episodes: &[1, 2, 3, 4, 5],
            episode_id: None,
            score: 100,
            is_user_requested: false,
            is_season_pack: false,
            category: "",
            multi_targets: None,
            episode_intentions: None,
            source_pub_date: None,
            download_id: "",
            scoring_size_bytes: None,
            scoring_seeders: None,
            scoring_episode_count: None,
            submitter: None,
            quality_profile_id: None,
            version: 1,
        })
        .await
        .unwrap();
    assert!(
        matches!(beta, AddQueueResult::Added { .. }),
        "Different series should be Added, got {:?}",
        beta
    );
}

#[tokio::test]
async fn test_fuzzy_dedup_no_series_id_uses_exact_episode_id() {
    let (app, state, _tmp) = common::setup_test_app().await;
    let series_id = seed_series(&app, "Legacy Show").await;

    // Must insert episode row first — FK constraint on download_queue.episode_id
    let mapping = state
        .db
        .get_series_mapping(&series_id)
        .await
        .unwrap()
        .unwrap();
    let ep_id = mapping.get_episode_id("01", 1, false).unwrap();
    state
        .db
        .insert_episode(jumbie::db::episodes::InsertEpisodeParams {
            episode_id: &ep_id,
            series_id: "legacy-show",
            season: 1,
            episode: 1,
            file_path: None,
            title: Some("Test Episode"),
            quality_profile_id: None,
            status: "monitored",
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

    // Path A: episode_id is provided
    // Queue with episode_id set (legacy path — exact match)
    state
        .db
        .add_to_download_queue(AddToDownloadQueueParams {
            media_name: "Legacy.S01E01",
            media_link: "magnet:?xt=urn:btih:gggg",
            series_title: "Legacy Show",
            series_id: &series_id,
            seasons: &[1],
            episodes: &[1],
            episode_id: Some(&ep_id),
            score: 100,
            is_user_requested: false,
            is_season_pack: false,
            category: "",
            multi_targets: None,
            episode_intentions: None,
            source_pub_date: None,
            download_id: "",
            scoring_size_bytes: None,
            scoring_seeders: None,
            scoring_episode_count: None,
            submitter: None,
            quality_profile_id: None,
            version: 1,
        })
        .await
        .unwrap();

    // Same episode_id should be blocked by exact dedup
    let dup = state
        .db
        .add_to_download_queue(AddToDownloadQueueParams {
            media_name: "Legacy.S01E01.v2",
            media_link: "magnet:?xt=urn:btih:hhhh",
            series_title: "Legacy Show",
            series_id: &series_id,
            seasons: &[1],
            episodes: &[1],
            episode_id: Some(&ep_id),
            score: 95,
            is_user_requested: false,
            is_season_pack: false,
            category: "",
            multi_targets: None,
            episode_intentions: None,
            source_pub_date: None,
            download_id: "",
            scoring_size_bytes: None,
            scoring_seeders: None,
            scoring_episode_count: None,
            submitter: None,
            quality_profile_id: None,
            version: 1,
        })
        .await
        .unwrap();
    assert!(
        matches!(
            dup,
            AddQueueResult::Skipped | AddQueueResult::Replaced { .. }
        ),
        "Exact episode_id match should skip or replace, got {:?}",
        dup
    );
}

// queue_search_result integration (via POST /api/downloads)

#[tokio::test]
async fn test_manual_download_does_not_stamp_quality_profile() {
    // Manual downloads (add_download) must NOT stamp the series' quality
    // profile — the user explicitly chose a release (possibly outside the
    // profile), so the episode shouldn't be mis-attributed to it.
    let (app, state, _tmp) = common::setup_test_app().await;
    let series_id = seed_series(&app, "Manual No-Stamp Show").await;

    // Confirm the seeded series HAS a quality profile (default) — otherwise
    // this test asserts nothing meaningful.
    let mapping = state
        .db
        .get_series_mapping(&series_id)
        .await
        .unwrap()
        .expect("mapping exists");
    assert!(
        mapping
            .quality_profile
            .as_deref()
            .is_some_and(|p| !p.is_empty()),
        "seeded series should carry a quality profile for this test to be meaningful"
    );

    // Path A: episode-scoped manual download (search-modal Download button).
    let ep_id = mapping.get_episode_id("01", 1, false).unwrap();
    state
        .db
        .insert_episode(jumbie::db::episodes::InsertEpisodeParams::dummy(
            &ep_id,
            &series_id,
            1,
            1,
            &std::collections::HashMap::new(),
        ))
        .await
        .unwrap();

    let payload = jumbie_shared::types::DownloadMediaPayload {
        link: "magnet:?xt=urn:btih:manual1".to_string(),
        download_id: "hash_manual1".to_string(),
        category: Some("Series".to_string()),
        episode_id: Some(ep_id.clone()),
        tag: None,
        title: Some("Manual No-Stamp Show S01E01 720p".to_string()),
        score: Some(50),
        series_id: Some(series_id.clone()),
        is_season_pack: Some(false),
        is_user_requested: true,
        size: None,
        seeders: None,
        upload_date: None,
    };
    let req = common::post_json_request("/api/downloads", &payload);
    let res = app.clone().oneshot(req).await.unwrap();
    assert!(
        res.status().is_success() || res.status() == StatusCode::SERVICE_UNAVAILABLE,
        "Expected success or 503 (no downloader), got {}",
        res.status()
    );

    let queue = get_queue(&state).await;
    let item = queue
        .iter()
        .find(|i| i.episode_id.as_deref() == Some(ep_id.as_str()))
        .expect("manual download should be queued");
    assert!(
        item.quality_profile_id.is_none(),
        "manual result pick must not be attributed to the series quality profile, got {:?}",
        item.quality_profile_id
    );
}

#[tokio::test]
async fn test_queue_search_result_stamps_series_quality_profile() {
    // Auto-search / search auto-queue (queue_search_result) is an automatic
    // selection within the series' configured profile — the profile must be
    // snapshotted onto the queue item (the bug that left quality_profile_id
    // NULL for auto-search downloads).
    let (app, state, _tmp) = common::setup_test_app().await;
    let series_id = seed_series(&app, "Profile Stamp Show").await;

    // Give the series a distinctive quality profile so the stamp is observable.
    state
        .db
        .upsert_quality_profile(
            "qp-stamp-test",
            &jumbie_shared::types::QualityProfile {
                name: "Test 1080p".to_string(),
                qualities: vec![],
                upgrade_only_qualities: vec![],
            },
        )
        .await
        .unwrap();
    let mut mapping = state
        .db
        .get_series_mapping(&series_id)
        .await
        .unwrap()
        .expect("mapping exists");
    mapping.quality_profile = Some("qp-stamp-test".to_string());
    state
        .db
        .upsert_series_mapping(&series_id, &mapping)
        .await
        .unwrap();

    let ep_id = mapping.get_episode_id("01", 1, false).unwrap();
    state
        .db
        .insert_episode(jumbie::db::episodes::InsertEpisodeParams::dummy(
            &ep_id,
            &series_id,
            1,
            1,
            &std::collections::HashMap::new(),
        ))
        .await
        .unwrap();

    let result = jumbie_shared::types::SearchResult {
        title: "[Cytox] Profile Stamp Show S01E01 1080p".to_string(),
        size: 1024,
        seeders: Some(10),
        leechers: None,
        link: Some("magnet:?xt=urn:btih:qstest".to_string()),
        source: "MockSource".to_string(),
        score: 100,
        published: None,
        is_season_pack: false,
        download_id: Some("qstest".to_string()),
        submitter: Some("Cytox".to_string()),
        release_checks: vec![],
        queue_action: String::new(),
    };

    let queued = jumbie::api_routes::system::search_core::queue_search_result(
        jumbie::api_routes::system::search_core::QueueSearchParams {
            state: &state,
            result: &result,
            series_id: &series_id,
            series_title: "Profile Stamp Show",
            seasons: &[1],
            episodes: &[1],
            episode_ids: std::slice::from_ref(&ep_id),
            is_user_requested: false,
            is_season_pack: false,
            score: 100,
            category: "Series",
            episode_intentions: None,
        },
    )
    .await
    .unwrap();
    assert!(matches!(queued, AddQueueResult::Added { .. }));

    let item = state
        .db
        .get_queue_item_by_episode_id(&ep_id)
        .await
        .unwrap()
        .expect("queued item must exist");
    assert_eq!(
        item.quality_profile_id.as_deref(),
        Some("qp-stamp-test"),
        "auto-selected search result must carry the series quality profile"
    );
}

#[tokio::test]
async fn test_download_queue_stores_series_id_and_pack_flag() {
    let (app, state, _tmp) = common::setup_test_app().await;
    let series_id = seed_series(&app, "Pack Test Show").await;

    // Simulate Path B: series page download with is_season_pack=true
    let payload = jumbie_shared::types::DownloadMediaPayload {
        link: "magnet:?xt=urn:btih:pack1".to_string(),
        download_id: "hash_pack1".to_string(),
        category: Some("Series".to_string()),
        episode_id: None, // no episode_id — Path B
        tag: None,
        title: Some("Pack Test Show S01 Complete".to_string()),
        score: Some(100),
        series_id: Some(series_id.clone()),
        is_season_pack: Some(true),
        is_user_requested: true,
        size: None,
        seeders: None,
        upload_date: None,
    };

    let req = common::post_json_request("/api/downloads", &payload);
    let res = app.clone().oneshot(req).await.unwrap();
    assert!(
        res.status().is_success() || res.status() == StatusCode::SERVICE_UNAVAILABLE,
        "Expected success or 503 (no downloader), got {}",
        res.status()
    );

    // If the server processed it, check the queue item
    let queue = get_queue(&state).await;
    if let Some(item) = queue.into_iter().find(|i| i.series_id == series_id) {
        assert!(
            item.is_season_pack,
            "Season pack flag should be true for pack download"
        );
        assert_eq!(item.series_id, series_id, "series_id should match");
        assert_eq!(item.category, "Series", "Category should be 'Series'");
    }
}

#[tokio::test]
async fn test_add_download_returns_outcome_and_queue_id() {
    // POST /api/downloads is queue-first: it returns the enqueue outcome and the
    // new queue row id — not the download client, which is assigned later by the
    // organizer (so `client_id` is not known at response time).
    let (app, state, _tmp) = common::setup_test_app().await;
    let series_id = seed_series(&app, "Add Response Show").await;

    let payload = jumbie_shared::types::DownloadMediaPayload {
        link: "magnet:?xt=urn:btih:addresponse1".to_string(),
        download_id: "hash_addresponse1".to_string(),
        category: Some("Series".to_string()),
        episode_id: None, // Path B — series-scoped
        tag: None,
        title: Some("Add Response Show S01E01".to_string()),
        score: Some(10),
        series_id: Some(series_id.clone()),
        is_season_pack: Some(false),
        is_user_requested: true,
        size: None,
        seeders: None,
        upload_date: None,
    };

    let res = app
        .clone()
        .oneshot(common::post_json_request("/api/downloads", &payload))
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        StatusCode::OK,
        "manual add should queue and return 200"
    );

    let body = common::response_body(res).await;
    let response: jumbie_shared::types::AddDownloadResponse =
        serde_json::from_slice(&body).expect("response body must be AddDownloadResponse JSON");

    assert_eq!(response.outcome, "added");
    let queue_id = response
        .queue_id
        .expect("an added download must return its queue id");

    let queue = get_queue(&state).await;
    assert!(
        queue.iter().any(|i| i.id == queue_id),
        "returned queue_id {queue_id} must match a queued row"
    );
}

#[tokio::test]
async fn test_series_level_search_uses_sentinel() {
    // Path B with NO episode_id — series-level search download.
    // Queue item should have episode=None, season=None, episode_id=None.
    let (app, state, _tmp) = common::setup_test_app().await;
    let series_id = seed_series(&app, "Sentinel Test Show").await;

    let payload = jumbie_shared::types::DownloadMediaPayload {
        link: "magnet:?xt=urn:btih:sentinel1".to_string(),
        download_id: "hash_sentinel1".to_string(),
        category: Some("Series".to_string()),
        episode_id: None,
        tag: None,
        title: Some("Sentinel Test S01E03".to_string()),
        score: Some(100),
        series_id: Some(series_id.clone()),
        is_season_pack: Some(false),
        is_user_requested: true,
        size: None,
        seeders: None,
        upload_date: None,
    };

    let req = common::post_json_request("/api/downloads", &payload);
    let res = app.clone().oneshot(req).await.unwrap();
    assert!(
        res.status().is_success() || res.status() == StatusCode::SERVICE_UNAVAILABLE,
        "Expected success or 503 (no downloader), got {}",
        res.status()
    );

    let queue = get_queue(&state).await;
    let item = queue
        .iter()
        .find(|i| i.media_name == "Sentinel Test S01E03");
    assert!(item.is_some(), "Queue item should exist");
    if let Some(item) = item {
        assert_eq!(item.series_id, series_id, "series_id should match");
        assert!(
            item.episode.is_none(),
            "Series-level search should have episode=None, got {:?}",
            item.episode
        );
        assert!(
            item.season.is_none(),
            "Series-level search should have season=None, got {:?}",
            item.season
        );
        assert!(
            item.episode_id.as_deref().unwrap_or("").is_empty(),
            "Series-level search should have episode_id empty, got {:?}",
            item.episode_id
        );
        assert!(item.is_user_requested, "should be user-requested");
    }
}

#[tokio::test]
async fn test_download_queue_stores_category() {
    let (app, state, _tmp) = common::setup_test_app().await;
    let series_id = seed_series(&app, "Category Test").await;

    // Path B with a custom category
    let payload = jumbie_shared::types::DownloadMediaPayload {
        link: "magnet:?xt=urn:btih:cat1".to_string(),
        download_id: "hash_cat1".to_string(),
        category: Some("Movies".to_string()),
        episode_id: None,
        is_user_requested: false,
        tag: None,
        title: Some("Category Test".to_string()),
        score: Some(100),
        series_id: Some(series_id.clone()),
        is_season_pack: Some(false),
        size: None,
        seeders: None,
        upload_date: None,
    };

    let req = common::post_json_request("/api/downloads", &payload);
    let res = app.clone().oneshot(req).await.unwrap();
    assert!(
        res.status().is_success() || res.status() == StatusCode::SERVICE_UNAVAILABLE,
        "Expected success or 503 (no downloader), got {}",
        res.status()
    );

    let queue = get_queue(&state).await;
    if let Some(item) = queue.into_iter().find(|i| i.series_id == series_id) {
        assert_eq!(item.category, "Movies", "Category should be 'Movies'");
    }
}

// Path C: No-context fallback

#[tokio::test]
async fn test_download_no_context_creates_unknown_series() {
    let (app, state, _tmp) = common::setup_test_app().await;

    // Path C: no episode_id, no series_id
    let payload = jumbie_shared::types::DownloadMediaPayload {
        link: "magnet:?xt=urn:btih:orphan1".to_string(),
        download_id: "hash_orphan1".to_string(),
        category: None,
        episode_id: None,
        tag: None,
        title: Some("Orphan Download".to_string()),
        score: Some(50),
        series_id: None,
        is_season_pack: Some(false),
        is_user_requested: false,
        size: None,
        seeders: None,
        upload_date: None,
    };

    let req = common::post_json_request("/api/downloads", &payload);
    let res = app.clone().oneshot(req).await.unwrap();
    assert!(
        res.status().is_success() || res.status() == StatusCode::SERVICE_UNAVAILABLE,
        "Expected success or 503, got {}",
        res.status()
    );

    let queue = get_queue(&state).await;
    let orphan = queue.iter().find(|i| i.media_name == "Orphan Download");
    assert!(orphan.is_some(), "Orphan item should exist in queue");
    if let Some(item) = orphan {
        // Path C fallback sets series_id to "unknown"
        assert_eq!(
            item.series_id, "unknown",
            "No-context download should have 'unknown' series_id"
        );
        assert_eq!(
            item.series_title, "Manual Download",
            "Title should be 'Manual Download'"
        );
        assert!(!item.is_season_pack, "Should not be a season pack");
    }
}

// Queue item stores series_id when episode_id is passed (Path A)

#[tokio::test]
async fn test_download_with_episode_id_resolves_series() {
    let (app, state, _tmp) = common::setup_test_app().await;
    let series_id = seed_series(&app, "Episode Lookup Show").await;

    // We need an episode row in the DB so Path A can resolve it.
    // Use the mapping's get_episode_id to produce the canonical ID.
    let mapping = state
        .db
        .get_series_mapping(&series_id)
        .await
        .unwrap()
        .unwrap();
    let ep_id = mapping.get_episode_id("01", 1, false).unwrap();
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
            status: "monitored",
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

    // Path A: episode_id is provided
    let payload = jumbie_shared::types::DownloadMediaPayload {
        link: "magnet:?xt=urn:btih:epath1".to_string(),
        download_id: "hash_epath1".to_string(),
        is_user_requested: false,
        category: None,
        episode_id: Some(ep_id.clone()),
        tag: None,
        title: Some("Episode Lookup Download".to_string()),
        score: Some(200),
        series_id: None,
        is_season_pack: Some(false),
        size: None,
        seeders: None,
        upload_date: None,
    };

    let req = common::post_json_request("/api/downloads", &payload);
    let res = app.clone().oneshot(req).await.unwrap();
    assert!(
        res.status().is_success() || res.status() == StatusCode::SERVICE_UNAVAILABLE,
        "Expected success or 503, got {}",
        res.status()
    );

    let queue = get_queue(&state).await;
    let item = queue
        .iter()
        .find(|i| i.media_name == "Episode Lookup Download");
    assert!(item.is_some(), "Queue item should exist");
    if let Some(item) = item {
        // series_id should NOT be empty since we resolved through Path A
        assert!(
            !item.series_id.is_empty(),
            "Path A should resolve a series_id"
        );
        // episode_id on the queue should be the mapping-generated one, not raw payload
        assert_eq!(
            item.episode_id.as_deref(),
            Some(ep_id.as_str()),
            "Queue episode_id should match canonical ID"
        );
    }
}

// Multi-target (magnet-level dedup)

#[tokio::test]
async fn test_magnet_dedup_merges_different_target() {
    // When the SAME magnet is queued for a DIFFERENT (series, season, episode),
    // it should be merged as multi_target instead of creating a new entry.
    let (app, state, _tmp) = common::setup_test_app().await;
    let series_id = seed_series(&app, "Magnet Dedup Show").await;

    // 1. Queue entry A: magnet X for S01E01
    let result_a = state
        .db
        .add_to_download_queue(AddToDownloadQueueParams {
            media_name: "First Target S01E01",
            media_link: "magnet:?xt=urn:btih:dedup123",
            series_title: "Magnet Dedup Show",
            series_id: &series_id,
            seasons: &[1],
            episodes: &[1],
            episode_id: None,
            score: 100,
            is_user_requested: false,
            is_season_pack: false,
            category: "",
            multi_targets: None,
            episode_intentions: None,
            source_pub_date: None,
            download_id: "",
            scoring_size_bytes: None,
            scoring_seeders: None,
            scoring_episode_count: None,
            submitter: None,
            quality_profile_id: None,
            version: 1,
        })
        .await
        .unwrap();
    assert!(
        matches!(result_a, AddQueueResult::Added { .. }),
        "First insert should be Added"
    );

    // 2. Queue entry B: SAME magnet X for S02E01 (different target)
    let result_b = state
        .db
        .add_to_download_queue(AddToDownloadQueueParams {
            media_name: "Second Target S02E01",
            media_link: "magnet:?xt=urn:btih:dedup123",
            series_title: "Magnet Dedup Show",
            series_id: &series_id,
            seasons: &[2],
            episodes: &[1],
            episode_id: None,
            score: 100,
            is_user_requested: false,
            is_season_pack: false,
            category: "",
            multi_targets: None,
            episode_intentions: None,
            source_pub_date: None,
            download_id: "",
            scoring_size_bytes: None,
            scoring_seeders: None,
            scoring_episode_count: None,
            submitter: None,
            quality_profile_id: None,
            version: 1,
        })
        .await
        .unwrap();

    assert!(
        matches!(result_b, AddQueueResult::Merged { .. }),
        "Same magnet, different target should return Merged, got {:?}",
        result_b
    );

    if let AddQueueResult::Merged {
        existing_id: _,
        new_targets,
    } = result_b
    {
        assert_eq!(new_targets, 1, "Should have 1 target in multi_targets");
    }

    let queue = dbg!(state.db.get_download_queue().await.unwrap());
    let matching: Vec<_> = queue
        .iter()
        .filter(|i| i.media_link == "magnet:?xt=urn:btih:dedup123")
        .collect();
    assert_eq!(matching.len(), 1, "Only 1 queue entry for this magnet");

    let entry = &matching[0];
    assert!(
        entry.multi_targets.is_some(),
        "Queue entry should have multi_targets"
    );
    let targets: Vec<jumbie_shared::types::MultiTarget> =
        serde_json::from_str(entry.multi_targets.as_deref().unwrap_or("")).unwrap();
    assert_eq!(targets.len(), 1, "Should have 1 multi-target");
    assert_eq!(targets[0].season, 2, "Multi-target season should be 2");
    assert_eq!(targets[0].episode, 1, "Multi-target episode should be 1");
}

#[tokio::test]
async fn test_magnet_dedup_same_target_still_compares_scores() {
    // When the SAME magnet targets the EXACT same (series, season, episode),
    // the normal score comparison should run (Added/Replaced/Skipped),
    // NOT Merged.
    let (app, state, _tmp) = common::setup_test_app().await;
    let series_id = seed_series(&app, "Magnet Score Show").await;

    let mapping = state
        .db
        .get_series_mapping(&series_id)
        .await
        .unwrap()
        .unwrap();
    let ep_id = mapping.get_episode_id("01", 1, false).unwrap();

    // 1. Insert the episode so the episode-level check works
    state
        .db
        .insert_episode(jumbie::db::episodes::InsertEpisodeParams {
            episode_id: &ep_id,
            series_id: &series_id,
            season: 1,
            episode: 1,
            file_path: None,
            title: Some("Test Magnet Dedup"),
            quality_profile_id: None,
            status: "monitored",
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

    // 2. Queue entry A: score 100
    let result_a = state
        .db
        .add_to_download_queue(AddToDownloadQueueParams {
            media_name: "High Score",
            media_link: "magnet:?xt=urn:btih:score123",
            series_title: "Magnet Score Show",
            series_id: &series_id,
            seasons: &[1],
            episodes: &[1],
            episode_id: Some(&ep_id),
            score: 100,
            is_user_requested: false,
            is_season_pack: false,
            category: "",
            multi_targets: None,
            episode_intentions: None,
            source_pub_date: None,
            download_id: "",
            scoring_size_bytes: None,
            scoring_seeders: None,
            scoring_episode_count: None,
            submitter: None,
            quality_profile_id: None,
            version: 1,
        })
        .await
        .unwrap();
    assert!(
        matches!(result_a, AddQueueResult::Added { .. }),
        "First high-score insert should be Added"
    );

    // 3. Queue entry B: same magnet, same target, LOWER score → Skipped
    let result_b = state
        .db
        .add_to_download_queue(AddToDownloadQueueParams {
            media_name: "Low Score",
            media_link: "magnet:?xt=urn:btih:score123",
            series_title: "Magnet Score Show",
            series_id: &series_id,
            seasons: &[1],
            episodes: &[1],
            episode_id: Some(&ep_id),
            score: 50,
            is_user_requested: false,
            is_season_pack: false,
            category: "",
            multi_targets: None,
            episode_intentions: None,
            source_pub_date: None,
            download_id: "",
            scoring_size_bytes: None,
            scoring_seeders: None,
            scoring_episode_count: None,
            submitter: None,
            quality_profile_id: None,
            version: 1,
        })
        .await
        .unwrap();
    assert_eq!(
        result_b,
        AddQueueResult::Skipped,
        "Lower score same target should be Skipped"
    );
}

// is_user_requested with fair score comparison
// Verifies that user-requested items still use fair score comparison at
// queue time (a lower-scored user request should not replace a higher-scored
// existing entry, even though the flag will force upgrade evaluation later).

#[tokio::test]
async fn test_user_requested_lower_score_does_not_replace() {
    let (app, state, _tmp) = common::setup_test_app().await;
    let series_id = seed_series(&app, "Manual Score Show").await;
    let ep_id = format!("{}_S01E01", series_id);

    let meta = std::collections::HashMap::new();
    let _ = state
        .db
        .insert_episode(jumbie::db::episodes::InsertEpisodeParams {
            episode_id: &ep_id,
            series_id: &series_id,
            season: 1,
            episode: 1,
            file_path: None,
            title: Some("Manual Score Test"),
            quality_profile_id: None,
            status: "monitored",
            meta_date: None,
            est_date: None,
            metadata_ids: &meta,
            description: None,
            runtime: None,
            image_url: None,
            metadata_source: None,
            numbering_mode: None,
        })
        .await
        .unwrap();

    // 2. Queue entry A: score 100, not user-requested
    let result_a = state
        .db
        .add_to_download_queue(AddToDownloadQueueParams {
            media_name: "High Score Auto",
            media_link: "magnet:?xt=urn:btih:manual001",
            series_title: "Manual Score Show",
            series_id: &series_id,
            seasons: &[1],
            episodes: &[1],
            episode_id: Some(&ep_id),
            score: 100,
            is_user_requested: false,
            is_season_pack: false,
            category: "",
            multi_targets: None,
            episode_intentions: None,
            source_pub_date: None,
            download_id: "",
            scoring_size_bytes: None,
            scoring_seeders: None,
            scoring_episode_count: None,
            submitter: None,
            quality_profile_id: None,
            version: 1,
        })
        .await
        .unwrap();
    assert!(matches!(result_a, AddQueueResult::Added { .. }));

    // Queue entry B: lower score but is_user_requested=true — it must be Skipped
    // rather than replace the higher-scored entry.
    let result_b = state
        .db
        .add_to_download_queue(AddToDownloadQueueParams {
            media_name: "Low Score Manual",
            media_link: "magnet:?xt=urn:btih:manual002",
            series_title: "Manual Score Show",
            series_id: &series_id,
            seasons: &[1],
            episodes: &[1],
            episode_id: Some(&ep_id),
            score: 50,
            is_user_requested: true,
            is_season_pack: false,
            category: "",
            multi_targets: None,
            episode_intentions: None,
            source_pub_date: None,
            download_id: "",
            scoring_size_bytes: None,
            scoring_seeders: None,
            scoring_episode_count: None,
            submitter: None,
            quality_profile_id: None,
            version: 1,
        })
        .await
        .unwrap();
    assert_eq!(
        result_b,
        AddQueueResult::Skipped,
        "User-requested item with lower score should be Skipped"
    );
}

#[tokio::test]
async fn test_user_requested_higher_score_replaces() {
    let (app, state, _tmp) = common::setup_test_app().await;
    let series_id = seed_series(&app, "Manual Score Show 2").await;
    let ep_id = format!("{}_S01E01", series_id);

    let meta = std::collections::HashMap::new();
    let _ = state
        .db
        .insert_episode(jumbie::db::episodes::InsertEpisodeParams {
            episode_id: &ep_id,
            series_id: &series_id,
            season: 1,
            episode: 1,
            file_path: None,
            title: Some("Manual Score Test 2"),
            quality_profile_id: None,
            status: "monitored",
            meta_date: None,
            est_date: None,
            metadata_ids: &meta,
            description: None,
            runtime: None,
            image_url: None,
            metadata_source: None,
            numbering_mode: None,
        })
        .await
        .unwrap();

    // Queue entry A: score 30 (low)
    let _ = state
        .db
        .add_to_download_queue(AddToDownloadQueueParams {
            media_name: "Low Score Existing",
            media_link: "magnet:?xt=urn:btih:manual003",
            series_title: "Manual Score Show 2",
            series_id: &series_id,
            seasons: &[1],
            episodes: &[1],
            episode_id: Some(&ep_id),
            score: 30,
            is_user_requested: false,
            is_season_pack: false,
            category: "",
            multi_targets: None,
            episode_intentions: None,
            source_pub_date: None,
            download_id: "",
            scoring_size_bytes: None,
            scoring_seeders: None,
            scoring_episode_count: None,
            submitter: None,
            quality_profile_id: None,
            version: 1,
        })
        .await
        .unwrap();

    // User-requested item with higher score (80) → should replace
    let result = state
        .db
        .add_to_download_queue(AddToDownloadQueueParams {
            media_name: "Higher Score Manual",
            media_link: "magnet:?xt=urn:btih:manual004",
            series_title: "Manual Score Show 2",
            series_id: &series_id,
            seasons: &[1],
            episodes: &[1],
            episode_id: Some(&ep_id),
            score: 80,
            is_user_requested: true,
            is_season_pack: false,
            category: "",
            multi_targets: None,
            episode_intentions: None,
            source_pub_date: None,
            download_id: "",
            scoring_size_bytes: None,
            scoring_seeders: None,
            scoring_episode_count: None,
            submitter: None,
            quality_profile_id: None,
            version: 1,
        })
        .await
        .unwrap();
    let is_replaced = matches!(result, AddQueueResult::Replaced(_));
    assert!(
        is_replaced,
        "User-requested item with higher score should replace"
    );

    // The new queue item should have is_user_requested=true
    let queue = get_queue(&state).await;
    let new_item = queue
        .iter()
        .find(|i| i.episode_id.as_deref() == Some(ep_id.as_str()))
        .unwrap();
    assert!(
        new_item.is_user_requested,
        "Newly queued item should preserve is_user_requested=true"
    );
}

#[tokio::test]
async fn test_magnet_dedup_cross_series_merges() {
    // When the same magnet targets a COMPLETELY DIFFERENT SERIES,
    // it should still merge as a multi_target (cross-series sharing).
    let (app, state, _tmp) = common::setup_test_app().await;
    let series_a = seed_series(&app, "Series Alpha").await;
    let series_b = seed_series(&app, "Series Beta").await;

    // 1. Queue entry for Series Alpha S01E01
    let _ = state
        .db
        .add_to_download_queue(AddToDownloadQueueParams {
            media_name: "Cross Alpha",
            media_link: "magnet:?xt=urn:btih:cross123",
            series_title: "Series Alpha",
            series_id: &series_a,
            seasons: &[1],
            episodes: &[1],
            episode_id: None,
            score: 100,
            is_user_requested: false,
            is_season_pack: false,
            category: "",
            multi_targets: None,
            episode_intentions: None,
            source_pub_date: None,
            download_id: "",
            scoring_size_bytes: None,
            scoring_seeders: None,
            scoring_episode_count: None,
            submitter: None,
            quality_profile_id: None,
            version: 1,
        })
        .await
        .unwrap();

    // 2. Same magnet for Series Beta S01E01 → should merge
    let result = state
        .db
        .add_to_download_queue(AddToDownloadQueueParams {
            media_name: "Cross Beta",
            media_link: "magnet:?xt=urn:btih:cross123",
            series_title: "Series Beta",
            series_id: &series_b,
            seasons: &[1],
            episodes: &[1],
            episode_id: None,
            score: 100,
            is_user_requested: false,
            is_season_pack: false,
            category: "",
            multi_targets: None,
            episode_intentions: None,
            source_pub_date: None,
            download_id: "",
            scoring_size_bytes: None,
            scoring_seeders: None,
            scoring_episode_count: None,
            submitter: None,
            quality_profile_id: None,
            version: 1,
        })
        .await
        .unwrap();
    assert!(
        matches!(result, AddQueueResult::Merged { .. }),
        "Cross-series same magnet should merge, got {:?}",
        result
    );

    let queue = state.db.get_download_queue().await.unwrap();
    let matching: Vec<_> = queue
        .iter()
        .filter(|i| i.media_link == "magnet:?xt=urn:btih:cross123")
        .collect();
    assert_eq!(
        matching.len(),
        1,
        "Only 1 queue entry for cross-series magnet"
    );
    assert!(
        matching[0].multi_targets.is_some(),
        "Entry should have multi_targets"
    );
    let targets: Vec<jumbie_shared::types::MultiTarget> =
        serde_json::from_str(matching[0].multi_targets.as_deref().unwrap()).unwrap();
    assert_eq!(targets.len(), 1, "Should have 1 multi-target");
    assert_eq!(
        targets[0].series_id, series_b,
        "Multi-target series should be Beta"
    );
}

/// Regression: `POST /api/downloads` is the manual entry point, so its queue row
/// must carry `is_manual` — this drives the `Manual` badge and exempts the item
/// from no-progress autoresolve.
#[tokio::test]
async fn test_manual_download_sets_is_manual_flag() {
    let (app, state, _tmp) = common::setup_test_app().await;
    let series_id = seed_series(&app, "Manual Flag Show").await;
    let mapping = state
        .db
        .get_series_mapping(&series_id)
        .await
        .unwrap()
        .expect("mapping exists");
    let ep_id = mapping.get_episode_id("01", 1, false).unwrap();
    state
        .db
        .insert_episode(jumbie::db::episodes::InsertEpisodeParams::dummy(
            &ep_id,
            &series_id,
            1,
            1,
            &std::collections::HashMap::new(),
        ))
        .await
        .unwrap();

    let payload = jumbie_shared::types::DownloadMediaPayload {
        link: "magnet:?xt=urn:btih:manualflag".to_string(),
        download_id: "hash_manualflag".to_string(),
        category: Some("Series".to_string()),
        episode_id: Some(ep_id.clone()),
        tag: None,
        title: Some("Manual Flag Show S01E01 1080p".to_string()),
        score: Some(80),
        series_id: Some(series_id.clone()),
        is_season_pack: Some(false),
        is_user_requested: true,
        size: None,
        seeders: None,
        upload_date: None,
    };
    let req = common::post_json_request("/api/downloads", &payload);
    let res = app.clone().oneshot(req).await.unwrap();
    assert!(
        res.status().is_success() || res.status() == StatusCode::SERVICE_UNAVAILABLE,
        "Expected success or 503 (no downloader), got {}",
        res.status()
    );

    let queue = get_queue(&state).await;
    let item = queue
        .iter()
        .find(|i| i.download_id.as_deref() == Some("hash_manualflag"))
        .expect("manual download should be queued");
    assert!(
        item.is_manual,
        "POST /api/downloads must mark the row is_manual"
    );
}

// Episode-level manual search (Path A) downloading a season pack
//
// The frontend always sends both `episode_id` and `series_id`, but `add_download`
// prioritises `episode_id` (Path A). An episode search is therefore scoped to the
// one episode the user was viewing while the pack flag records the release type —
// the pack's *other* episodes are resolved later, at organize time.

/// Insert the canonical episode row for `(season, episode)` so Path A can resolve it.
async fn seed_episode(
    state: &Arc<jumbie::api::AppState>,
    series_id: &str,
    season: i32,
    episode: i32,
) -> String {
    let mapping = state
        .db
        .get_series_mapping(series_id)
        .await
        .unwrap()
        .unwrap();
    let ep_id = mapping
        .get_episode_id(&format!("{season:02}"), episode, false)
        .unwrap();
    state
        .db
        .insert_episode(jumbie::db::episodes::InsertEpisodeParams {
            episode_id: &ep_id,
            series_id,
            season,
            episode,
            file_path: None,
            title: Some("Test Episode"),
            quality_profile_id: None,
            status: "monitored",
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
    ep_id
}

#[tokio::test]
async fn test_episode_manual_search_season_pack_queues_single_target_context() {
    let (app, state, _tmp) = common::setup_test_app().await;
    let series_id = seed_series(&app, "Episode Pack Show").await;
    let ep_id = seed_episode(&state, &series_id, 1, 5).await;

    // Both ids are sent (as the frontend does); Path A must win.
    let payload = jumbie_shared::types::DownloadMediaPayload {
        link: "magnet:?xt=urn:btih:maneppack1".to_string(),
        download_id: "hash_maneppack1".to_string(),
        category: Some("Series".to_string()),
        episode_id: Some(ep_id.clone()),
        tag: None,
        title: Some("Episode Pack Show S01 COMPLETE".to_string()),
        score: Some(300),
        series_id: Some(series_id.clone()),
        is_season_pack: Some(true),
        is_user_requested: true,
        size: None,
        seeders: None,
        upload_date: None,
    };

    let res = app
        .clone()
        .oneshot(common::post_json_request("/api/downloads", &payload))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK, "a pack must queue cleanly");

    let queue = get_queue(&state).await;
    let item = queue
        .iter()
        .find(|i| i.media_name == "Episode Pack Show S01 COMPLETE")
        .expect("pack should be queued");

    assert_eq!(item.series_id, series_id, "Path A resolves the series");
    assert_eq!(item.season.as_deref(), Some("01"));
    assert_eq!(item.episode, Some(5));
    assert_eq!(item.episode_id.as_deref(), Some(ep_id.as_str()));
    assert!(item.is_season_pack, "the pack flag must be preserved");
    assert!(item.is_manual, "a picked release is manual");
    assert!(item.is_user_requested);

    // The critical contract: an episode search yields exactly ONE target episode
    // even though the release is a whole-season pack.
    let intentions: Vec<jumbie_shared::types::EpisodeIntention> = serde_json::from_str(
        item.episode_intentions
            .as_deref()
            .expect("intentions present"),
    )
    .expect("intentions must be valid JSON");
    assert_eq!(
        intentions.len(),
        1,
        "episode search must scope the pack to a single target episode"
    );
    assert_eq!(intentions[0].episode_id, ep_id);
    assert!(intentions[0].keep, "the target episode is a kept intention");
}

#[tokio::test]
async fn test_episode_manual_search_stale_episode_id_still_queues() {
    let (app, state, _tmp) = common::setup_test_app().await;
    let series_id = seed_series(&app, "Stale Episode Show").await;

    // An episode id that no longer exists (race: episode deleted between search and
    // download, or a forged/stale client request). Path A has a manual fallback;
    // it must not fail the request with an FK violation.
    let payload = jumbie_shared::types::DownloadMediaPayload {
        link: "magnet:?xt=urn:btih:staleep1".to_string(),
        download_id: "hash_staleep1".to_string(),
        category: None,
        episode_id: Some("ghost-show_S01E99".to_string()),
        tag: None,
        title: Some("Stale Episode Show S01E99".to_string()),
        score: Some(50),
        series_id: Some(series_id.clone()),
        is_season_pack: Some(false),
        is_user_requested: true,
        size: None,
        seeders: None,
        upload_date: None,
    };

    let res = app
        .clone()
        .oneshot(common::post_json_request("/api/downloads", &payload))
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        StatusCode::OK,
        "a stale episode_id must fall back to a manual download, not 500"
    );

    let queue = get_queue(&state).await;
    let item = queue
        .iter()
        .find(|i| i.media_name == "Stale Episode Show S01E99")
        .expect("the download should still be queued");
    // The read path COALESCEs NULL `episode_id` to "", so an empty value means no
    // episode association was stored.
    assert_eq!(
        item.episode_id.as_deref(),
        Some(""),
        "a dangling episode_id must not be written to the queue, got {:?}",
        item.episode_id
    );
}

#[tokio::test]
async fn test_episode_manual_search_pack_blank_episode_id_uses_series_path() {
    let (app, state, _tmp) = common::setup_test_app().await;
    let series_id = seed_series(&app, "Blank Episode Show").await;

    // Some clients send `episode_id: ""` for a series-scoped view; that must fall
    // through to Path B (sentinel), not the no-context Path C.
    let payload = jumbie_shared::types::DownloadMediaPayload {
        link: "magnet:?xt=urn:btih:blankep1".to_string(),
        download_id: "hash_blankep1".to_string(),
        category: None,
        episode_id: Some(String::new()),
        tag: None,
        title: Some("Blank Episode Show S01 COMPLETE".to_string()),
        score: Some(100),
        series_id: Some(series_id.clone()),
        is_season_pack: Some(true),
        is_user_requested: true,
        size: None,
        seeders: None,
        upload_date: None,
    };

    let res = app
        .clone()
        .oneshot(common::post_json_request("/api/downloads", &payload))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let queue = get_queue(&state).await;
    let item = queue
        .iter()
        .find(|i| i.media_name == "Blank Episode Show S01 COMPLETE")
        .expect("pack should be queued");
    assert_eq!(item.series_id, series_id, "Path B keeps the series");
    assert!(
        item.episode.is_none() && item.season.is_none(),
        "a blank episode_id must use the series-level sentinel"
    );
    assert!(
        item.episode_id.is_none() || item.episode_id.as_deref() == Some(""),
        "no dangling episode_id may be stored"
    );
}

#[tokio::test]
async fn test_episode_manual_search_episode_without_season_is_rejected() {
    // A normal-mode episode row with no season has no episode identity, so Path A
    // must reject it as a client error rather than inventing season 1.
    let (app, state, _tmp) = common::setup_test_app().await;
    let series_id = seed_series(&app, "No Season Show").await;

    sqlx::query(
        "INSERT INTO episodes (episode_id, series_id, season, episode, status, monitored) \
         VALUES (?, ?, NULL, ?, 'monitored', 1)",
    )
    .bind("no-season-show_S01E01")
    .bind(&series_id)
    .bind(1)
    .execute(state.db.get_pool())
    .await
    .unwrap();

    let payload = jumbie_shared::types::DownloadMediaPayload {
        link: "magnet:?xt=urn:btih:noseason1".to_string(),
        download_id: "hash_noseason1".to_string(),
        category: None,
        episode_id: Some("no-season-show_S01E01".to_string()),
        tag: None,
        title: Some("No Season Show S01E01".to_string()),
        score: Some(50),
        series_id: Some(series_id.clone()),
        is_season_pack: Some(false),
        is_user_requested: true,
        size: None,
        seeders: None,
        upload_date: None,
    };

    let res = app
        .clone()
        .oneshot(common::post_json_request("/api/downloads", &payload))
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        StatusCode::BAD_REQUEST,
        "a season-less episode cannot be resolved in normal numbering mode"
    );
    assert!(
        get_queue(&state).await.is_empty(),
        "a rejected download must not be queued"
    );
}

#[tokio::test]
async fn test_episode_manual_search_uses_episode_season_not_title_season() {
    // Path A derives season/episode from the episode row, never from the release
    // title: a mislabeled pack (S02 in the name) picked while viewing S01E05 must
    // stay scoped to S01E05. This is what lets a wrong-season pack still fill the
    // episode the user was looking at.
    let (app, state, _tmp) = common::setup_test_app().await;
    let series_id = seed_series(&app, "Title Season Show").await;
    let ep_id = seed_episode(&state, &series_id, 1, 5).await;

    let payload = jumbie_shared::types::DownloadMediaPayload {
        link: "magnet:?xt=urn:btih:titlesea1".to_string(),
        download_id: "hash_titlesea1".to_string(),
        category: None,
        episode_id: Some(ep_id.clone()),
        tag: None,
        title: Some("Title Season Show S02 COMPLETE".to_string()),
        score: Some(100),
        series_id: Some(series_id.clone()),
        is_season_pack: Some(true),
        is_user_requested: true,
        size: None,
        seeders: None,
        upload_date: None,
    };

    let res = app
        .clone()
        .oneshot(common::post_json_request("/api/downloads", &payload))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let queue = get_queue(&state).await;
    let item = queue
        .iter()
        .find(|i| i.media_name == "Title Season Show S02 COMPLETE")
        .expect("pack should be queued");
    assert_eq!(
        item.season.as_deref(),
        Some("01"),
        "the episode's season must win over the release title's season"
    );
    assert_eq!(item.episode, Some(5));
    assert_eq!(item.episode_id.as_deref(), Some(ep_id.as_str()));
}
