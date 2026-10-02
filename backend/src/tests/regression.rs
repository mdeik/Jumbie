// Regression tests for DB-layer schema changes (season INTEGER migration, surrogate
// PK change) plus edge cases in the activity feed, calendar, download queue, release
// estimation, and file assignment.

use crate::db::DbManager;
use crate::db::download_queue::AddToDownloadQueueParams;
use crate::db::episodes::InsertEpisodeParams;
use crate::db::episodes::assign::AssignFileToEpisodeParams;
use crate::models::activity::ActivityEvent;
use jumbie_shared::types::ActivityType;
use jumbie_shared::types::MappingRule;
use std::collections::HashMap;

#[tokio::test]
async fn test_activity_record_and_read() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test.db");
    let db = DbManager::new(&db_path).await.unwrap();

    db.record_activity(ActivityEvent {
        event_type: ActivityType::Download,
        series_title: "Test Series".to_string(),
        season: Some("1".to_string()),
        episode: Some(3),
        episode_end: Some(5),
        title: None,
        details: Some("S01E03-E05".to_string()),
        status: "Success".to_string(),
    })
    .await
    .unwrap();

    db.record_activity(ActivityEvent {
        event_type: ActivityType::Import,
        series_title: "Imported Series".to_string(),
        season: None,
        episode: None,
        episode_end: None,
        title: None,
        details: Some("Scanned file".to_string()),
        status: "Success".to_string(),
    })
    .await
    .unwrap();

    db.record_activity(ActivityEvent {
        event_type: ActivityType::Assign,
        series_title: "Assigned File".to_string(),
        season: None,
        episode: None,
        episode_end: None,
        title: None,
        details: Some("/path/to/file.mkv".to_string()),
        status: "Success".to_string(),
    })
    .await
    .unwrap();

    let activity = db.get_recent_activity(50).await.unwrap();
    assert_eq!(activity.len(), 3, "Should have 3 activity entries");

    // Order is non-deterministic within the same second, so check membership, not order.
    assert!(
        activity
            .iter()
            .any(|a| a.activity_type == ActivityType::Download),
        "Should contain Download"
    );
    assert!(
        activity
            .iter()
            .any(|a| a.activity_type == ActivityType::Import),
        "Should contain Import"
    );
    assert!(
        activity
            .iter()
            .any(|a| a.activity_type == ActivityType::Assign),
        "Should contain Assign"
    );

    assert!(
        activity.iter().any(|a| a.title == "Test Series - S01E03"),
        "Download title should be series + single episode"
    );
    assert!(
        activity
            .iter()
            .all(|a| a.activity_type != ActivityType::Download || a.details.is_none()),
        "Download details should be None when no release name stored"
    );
}

#[tokio::test]
async fn test_activity_all_event_types() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test.db");
    let db = DbManager::new(&db_path).await.unwrap();

    let types = vec![
        ActivityType::Download,
        ActivityType::Import,
        ActivityType::Metadata,
        ActivityType::Reassign,
        ActivityType::Assign,
        ActivityType::Analyze,
        ActivityType::Delete,
        ActivityType::Unassign,
    ];

    for t in &types {
        db.record_activity(ActivityEvent {
            event_type: t.clone(),
            series_title: format!("Event {}", t.as_str()),
            season: None,
            episode: None,
            episode_end: None,
            title: None,
            details: None,
            status: "Success".to_string(),
        })
        .await
        .unwrap();
    }

    let activity = db.get_recent_activity(50).await.unwrap();
    assert_eq!(activity.len(), 8, "Should have all 8 event types");

    for t in &types {
        assert!(
            activity.iter().any(|a| a.activity_type == *t),
            "Should contain a {} event",
            t.as_str()
        );
    }
}

#[tokio::test]
async fn test_activity_empty() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test.db");
    let db = DbManager::new(&db_path).await.unwrap();

    let activity = db.get_recent_activity(50).await.unwrap();
    assert!(
        activity.is_empty(),
        "Activity should be empty when no events recorded"
    );
}

#[tokio::test]
async fn test_activity_limit() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test.db");
    let db = DbManager::new(&db_path).await.unwrap();

    for i in 0..5 {
        db.record_activity(ActivityEvent {
            event_type: ActivityType::Download,
            series_title: format!("Series {}", i),
            season: None,
            episode: Some(i),
            episode_end: None,
            title: None,
            details: None,
            status: "Success".to_string(),
        })
        .await
        .unwrap();
    }

    let activity = db.get_recent_activity(3).await.unwrap();
    assert_eq!(activity.len(), 3, "Should return at most 3 entries");
}

// When a file is unlinked and re-linked to a different episode, the calendar must
// not show duplicate events for the same date range — each episode_id appears once.
#[tokio::test]
async fn test_calendar_no_duplicates_after_unassign_reassign() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test.db");
    let db = DbManager::new(&db_path).await.unwrap();
    let pool = db.get_pool();

    let series_id = "ts-cal-no-dup";
    let series_title = "Calendar Dup Test";
    let mapping = MappingRule {
        target_title: series_title.to_string(),
        series_id: series_id.to_string(),
        name: series_title.to_string(),
        ..Default::default()
    };
    db.upsert_series_mapping(series_id, &mapping).await.unwrap();

    let now = chrono::Utc::now().naive_utc();
    let ep1_id = format!("{}_1_1", series_id);
    db.insert_episode(crate::db::episodes::InsertEpisodeParams {
        episode_id: &ep1_id,
        series_id,
        season: 1,
        episode: 1,
        file_path: Some("/tmp/ep1_before.mkv"),
        title: Some("Ep1"),
        quality_profile_id: None,
        status: "organized",
        meta_date: Some(now),
        est_date: None,
        metadata_ids: &HashMap::new(),
        description: None,
        runtime: None,
        image_url: None,
        metadata_source: None,
        numbering_mode: None,
    })
    .await
    .unwrap();

    let ep2_id = format!("{}_1_2", series_id);
    db.insert_episode(crate::db::episodes::InsertEpisodeParams {
        episode_id: &ep2_id,
        series_id,
        season: 1,
        episode: 2,
        file_path: Some("/tmp/ep2.mkv"),
        title: Some("Ep2"),
        quality_profile_id: None,
        status: "organized",
        meta_date: Some(now + chrono::Duration::days(7)),
        est_date: None,
        metadata_ids: &HashMap::new(),
        description: None,
        runtime: None,
        image_url: None,
        metadata_source: None,
        numbering_mode: None,
    })
    .await
    .unwrap();

    let start = now - chrono::Duration::days(1);
    let end = now + chrono::Duration::days(14);
    let before = db.get_calendar_episodes(start, end, false).await.unwrap();
    assert_eq!(
        before.len(),
        2,
        "Should have 2 calendar entries before reassign"
    );

    db.unlink_episode_file(&ep1_id).await.unwrap();

    let ep1: Option<String> =
        sqlx::query_scalar("SELECT status FROM episodes WHERE episode_id = ?")
            .bind(&ep1_id)
            .fetch_optional(pool)
            .await
            .unwrap();
    assert_eq!(
        ep1,
        Some("missing".to_string()),
        "Ep1 should be missing after unlink"
    );

    db.assign_file_to_episode(AssignFileToEpisodeParams {
        episode_id: &ep2_id,
        series_id,
        series_title: "Test Series",
        season: 1,
        episode: 2,
        file_path: "/tmp/ep1_before.mkv",
        episode_title: "Ep2",
        all_target_episode_ids: None,
        numbering_mode: 0,
        only_unassigned: false,
    })
    .await
    .unwrap();

    let ep2_file: Option<String> = db.get_episode_file_path(&ep2_id).await.unwrap();
    assert_eq!(
        ep2_file,
        Some("/tmp/ep1_before.mkv".to_string()),
        "Ep2 should have the reassigned file"
    );

    let after = db.get_calendar_episodes(start, end, false).await.unwrap();
    assert_eq!(
        after.len(),
        2,
        "Calendar should have 2 entries after reassign, got {}",
        after.len()
    );

    let mut seen = std::collections::HashSet::new();
    for row in &after {
        assert!(
            seen.insert(&row.episode_id),
            "Duplicate episode_id {} in calendar after reassign",
            row.episode_id
        );
    }
}

// Same-day calendar events are ordered by series_title.
#[tokio::test]
async fn test_calendar_same_day_sorted_by_series_title() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test.db");
    let db = DbManager::new(&db_path).await.unwrap();

    let now = chrono::Utc::now().naive_utc();

    // Insert Z before A so the ORDER BY series_title ASC is actually exercised.
    let series_z = ("ts-sort-z", "Z Series");
    let series_a = ("ts-sort-a", "A Series");

    for (id, title) in [series_z, series_a] {
        let mapping = MappingRule {
            target_title: title.to_string(),
            series_id: id.to_string(),
            name: title.to_string(),
            ..Default::default()
        };
        db.upsert_series_mapping(id, &mapping).await.unwrap();

        db.insert_episode(crate::db::episodes::InsertEpisodeParams {
            episode_id: &format!("{}_ep1", id),
            series_id: id,
            season: 1,
            episode: 1,
            file_path: None,
            title: Some("Test Episode"),
            quality_profile_id: None,
            status: "unreleased",
            meta_date: Some(now),
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
    }

    let start = now - chrono::Duration::days(1);
    let end = now + chrono::Duration::days(1);
    let rows = db.get_calendar_episodes(start, end, false).await.unwrap();

    assert_eq!(rows.len(), 2, "Should have 2 episodes on the same day");
    assert_eq!(
        rows[0].series_title, "A Series",
        "First episode should be 'A Series' (alphabetically first), got '{}'",
        rows[0].series_title
    );
    assert_eq!(
        rows[1].series_title, "Z Series",
        "Second episode should be 'Z Series' (alphabetically second), got '{}'",
        rows[1].series_title
    );
}

// After a series rename, download queue items resolve the new title from series_id
// rather than showing the stale title stored at insert time.
#[tokio::test]
async fn test_download_queue_resolves_series_title_from_series_id() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test.db");
    let db = DbManager::new(&db_path).await.unwrap();

    let series_id = "ts-dl-title";
    let mut mapping = MappingRule {
        target_title: "Old Name".to_string(),
        series_id: series_id.to_string(),
        name: "old_name".to_string(),
        ..Default::default()
    };
    db.upsert_series_mapping(series_id, &mapping).await.unwrap();

    // Episode must exist first: download_queue has an FK to episodes.
    db.insert_episode(crate::db::episodes::InsertEpisodeParams {
        episode_id: "test_ep_id",
        series_id,
        season: 1,
        episode: 1,
        file_path: None,
        title: Some("Test Episode"),
        quality_profile_id: None,
        status: "missing",
        meta_date: Some(chrono::Utc::now().naive_utc()),
        est_date: None,
        metadata_ids: &HashMap::new(),
        description: None,
        runtime: None,
        image_url: None,
        metadata_source: None,
        numbering_mode: None,
    })
    .await
    .unwrap();

    let _ = db
        .add_to_download_queue(AddToDownloadQueueParams {
            media_name: "Test Release",
            media_link: "magnet:?xt=urn:btih:test",
            series_title: "Old Name",
            series_id,
            seasons: &[1],
            episodes: &[1],
            episode_id: Some("test_ep_id"),
            score: 100,
            is_user_requested: false,
            is_season_pack: false,
            category: "Series",
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

    mapping.target_title = "New Name".to_string();
    db.upsert_series_mapping(series_id, &mapping).await.unwrap();

    let items = db.get_download_queue().await.unwrap();
    assert!(!items.is_empty(), "Queue should have at least one item");
    let item = items
        .iter()
        .find(|i| i.media_name == "Test Release")
        .unwrap();
    assert_eq!(
        item.series_title, "New Name",
        "Download queue series_title should resolve from series_mappings on rename"
    );
}

// An episode with only an upload_date (no meta_date/est_date) must appear in the
// calendar when the upload_date is in range.
#[tokio::test]
async fn test_calendar_includes_episode_with_only_upload_date() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test.db");
    let db = DbManager::new(&db_path).await.unwrap();

    let series_id = "ts-src-only";
    let mapping = MappingRule {
        target_title: "Source Only Series".to_string(),
        series_id: series_id.to_string(),
        name: "Source Only Series".to_string(),
        ..Default::default()
    };
    db.upsert_series_mapping(series_id, &mapping).await.unwrap();

    let now = chrono::Utc::now().naive_utc();
    let yesterday = now - chrono::Duration::days(1);
    let tomorrow = now + chrono::Duration::days(1);
    let next_week = now + chrono::Duration::days(7);

    // upload_date only; no meta_date or est_date.
    db.insert_episode(crate::db::episodes::InsertEpisodeParams {
        episode_id: "src_only_ep",
        series_id,
        season: 1,
        episode: 1,
        file_path: None,
        title: Some("Src Only"),
        quality_profile_id: None,
        status: "unreleased",
        meta_date: None,
        est_date: None,
        metadata_ids: &HashMap::new(),
        description: None,
        runtime: None,
        image_url: None,
        metadata_source: None,
        numbering_mode: None,
    })
    .await
    .unwrap();
    // The file's source date is content-scoped; seed it via a file link.
    seed_file_date(&db, "src_only_ep", "/lib/fd1.mkv", "fd-h1", now).await;

    // Control: only meta_date, should also appear.
    db.insert_episode(crate::db::episodes::InsertEpisodeParams {
        episode_id: "meta_only_ep",
        series_id,
        season: 1,
        episode: 2,
        file_path: None,
        title: Some("Meta Only"),
        quality_profile_id: None,
        status: "unreleased",
        meta_date: Some(now),
        est_date: None,
        metadata_ids: &HashMap::new(),
        description: None,
        runtime: None,
        image_url: None,
        metadata_source: None,
        numbering_mode: None,
    })
    .await
    .unwrap();

    // All dates out of range — must NOT appear.
    db.insert_episode(crate::db::episodes::InsertEpisodeParams {
        episode_id: "out_of_range_ep",
        series_id,
        season: 1,
        episode: 3,
        file_path: None,
        title: Some("Out of Range"),
        quality_profile_id: None,
        status: "unreleased",
        meta_date: Some(next_week + chrono::Duration::days(30)),
        est_date: None,
        metadata_ids: &HashMap::new(),
        description: None,
        runtime: None,
        image_url: None,
        metadata_source: None,
        numbering_mode: None,
    })
    .await
    .unwrap();

    let rows = db
        .get_calendar_episodes(yesterday, tomorrow, false)
        .await
        .unwrap();

    let mut ids: Vec<&str> = rows.iter().map(|r| r.episode_id.as_str()).collect();
    ids.sort_unstable();

    assert!(
        ids.contains(&"src_only_ep"),
        "Episode with ONLY upload_date should appear in calendar, got: {:?}",
        ids
    );
    assert!(
        ids.contains(&"meta_only_ep"),
        "Episode with ONLY metadata_date should appear in calendar, got: {:?}",
        ids
    );
    assert!(
        !ids.contains(&"out_of_range_ep"),
        "Episode with all dates out of range should NOT appear, got: {:?}",
        ids
    );
    assert_eq!(ids.len(), 2, "Should have exactly 2 episodes in range");
}

// With both meta_date and upload_date set, an episode whose upload_date is in range
// must appear even when meta_date is outside it. Regression: COALESCE(meta_date, …)
// would incorrectly exclude it.
#[tokio::test]
async fn test_calendar_upload_date_in_range_metadata_out() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test.db");
    let db = DbManager::new(&db_path).await.unwrap();

    let series_id = "ts-src-in";
    let mapping = MappingRule {
        target_title: "Source In Series".to_string(),
        series_id: series_id.to_string(),
        name: "Source In Series".to_string(),
        ..Default::default()
    };
    db.upsert_series_mapping(series_id, &mapping).await.unwrap();

    let now = chrono::Utc::now().naive_utc();
    let last_month = now - chrono::Duration::days(30);
    let next_week = now + chrono::Duration::days(7);
    let yesterday = now - chrono::Duration::days(1);

    // meta_date is last month (out of range), upload_date is today (in range).
    db.insert_episode(crate::db::episodes::InsertEpisodeParams {
        episode_id: "src_in_meta_out",
        series_id,
        season: 1,
        episode: 1,
        file_path: None,
        title: Some("Source In Meta Out"),
        quality_profile_id: None,
        status: "unreleased",
        meta_date: Some(last_month),
        est_date: None,
        metadata_ids: &HashMap::new(),
        description: None,
        runtime: None,
        image_url: None,
        metadata_source: None,
        numbering_mode: None,
    })
    .await
    .unwrap();
    // The file's source date is content-scoped; in range even though meta_date is out.
    seed_file_date(&db, "src_in_meta_out", "/lib/fd2.mkv", "fd-h2", now).await;

    // Control: only meta_date, in range.
    db.insert_episode(crate::db::episodes::InsertEpisodeParams {
        episode_id: "meta_in_control",
        series_id,
        season: 1,
        episode: 2,
        file_path: None,
        title: Some("Meta In"),
        quality_profile_id: None,
        status: "unreleased",
        meta_date: Some(now),
        est_date: None,
        metadata_ids: &HashMap::new(),
        description: None,
        runtime: None,
        image_url: None,
        metadata_source: None,
        numbering_mode: None,
    })
    .await
    .unwrap();

    let rows = db
        .get_calendar_episodes(yesterday, next_week, false)
        .await
        .unwrap();

    let mut ids: Vec<&str> = rows.iter().map(|r| r.episode_id.as_str()).collect();
    ids.sort_unstable();

    assert!(
        ids.contains(&"src_in_meta_out"),
        "Episode with upload_date in range (metadata out) should appear, got: {:?}",
        ids
    );
    assert!(
        ids.contains(&"meta_in_control"),
        "Control episode should appear, got: {:?}",
        ids
    );
    assert_eq!(rows.len(), 2, "Should have exactly 2 episodes");
}

// An episode with only an est_date must also appear.
#[tokio::test]
async fn test_calendar_includes_episode_with_only_estimated_date() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test.db");
    let db = DbManager::new(&db_path).await.unwrap();

    let series_id = "ts-est-only";
    let mapping = MappingRule {
        target_title: "Est Only Series".to_string(),
        series_id: series_id.to_string(),
        name: "Est Only Series".to_string(),
        ..Default::default()
    };
    db.upsert_series_mapping(series_id, &mapping).await.unwrap();

    let now = chrono::Utc::now().naive_utc();
    let yesterday = now - chrono::Duration::days(1);
    let next_week = now + chrono::Duration::days(7);

    db.insert_episode(crate::db::episodes::InsertEpisodeParams {
        episode_id: "est_only_ep",
        series_id,
        season: 1,
        episode: 1,
        file_path: None,
        title: Some("Est Only"),
        quality_profile_id: None,
        status: "unreleased",
        meta_date: None,
        est_date: Some(next_week),
        metadata_ids: &HashMap::new(),
        description: None,
        runtime: None,
        image_url: None,
        metadata_source: None,
        numbering_mode: None,
    })
    .await
    .unwrap();

    let rows = db
        .get_calendar_episodes(yesterday, next_week, false)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1, "Should have 1 episode with estimated date");
    assert_eq!(
        rows[0].episode_id, "est_only_ep",
        "Episode with only est_date should appear"
    );
}

/// A multipart episode (`file_path = NULL`, parts in `episode_parts`) must appear
/// on the calendar from its part's source date — the calendar resolves the episode
/// through the same SSoT join as every other query.
#[tokio::test]
async fn test_calendar_includes_multipart_episode_from_its_part() {
    let tmp = tempfile::tempdir().unwrap();
    let db = DbManager::new(&tmp.path().join("test.db")).await.unwrap();

    let series_id = "ts-cal-multipart";
    let mapping = MappingRule {
        target_title: "Cal Multipart".to_string(),
        series_id: series_id.to_string(),
        name: "Cal Multipart".to_string(),
        ..Default::default()
    };
    db.upsert_series_mapping(series_id, &mapping).await.unwrap();

    let now = chrono::Utc::now().naive_utc();
    let yesterday = now - chrono::Duration::days(1);
    let next_week = now + chrono::Duration::days(7);

    let ep_id = format!("{}_S01E01", series_id);
    db.insert_episode(crate::db::episodes::InsertEpisodeParams {
        episode_id: &ep_id,
        series_id,
        season: 1,
        episode: 1,
        file_path: None,
        title: Some("Multipart"),
        quality_profile_id: None,
        status: "organized",
        meta_date: None,
        est_date: None,
        metadata_ids: &HashMap::new(),
        description: None,
        runtime: None,
        image_url: None,
        metadata_source: None,
        numbering_mode: None,
    })
    .await
    .unwrap();

    // A part carrying the source date, in range.
    let part_path = "/lib/Cal Multipart.S01E01.pt1.mkv";
    db.upsert_content(crate::db::fingerprints::ContentRecord {
        fingerprint: "cal-mp-h1",
        media_info: None,
        path: part_path,
    })
    .await
    .unwrap();
    db.upsert_path(crate::db::fingerprints::PathRecord {
        file_path: part_path,
        fingerprint: "cal-mp-h1",
        inode: 0,
        device: 0,
        size: 1,
        mtime: 0.0,
        state: "organized",
        expected_path: None,
    })
    .await
    .unwrap();
    sqlx::query("INSERT INTO release_info (quick_hash, upload_date) VALUES (?, ?)")
        .bind("cal-mp-h1")
        .bind(now.format("%Y-%m-%d %H:%M:%S").to_string())
        .execute(db.get_pool())
        .await
        .unwrap();
    db.upsert_episode_part(&ep_id, 1, part_path, None)
        .await
        .unwrap();

    let rows = db
        .get_calendar_episodes(yesterday, next_week, false)
        .await
        .unwrap();
    let ids: Vec<&str> = rows.iter().map(|r| r.episode_id.as_str()).collect();
    assert!(
        ids.contains(&ep_id.as_str()),
        "a multipart episode must appear on the calendar, got: {:?}",
        ids
    );
    assert!(
        rows.iter()
            .any(|r| r.episode_id == ep_id && r.upload_date.is_some()),
        "the calendar row must carry the part's source date"
    );
}

/// Seed a file's source date at content level and point `episode_id` at that file.
/// The file's date lives in `release_info` (keyed by fingerprint) and is resolved
/// through `file_paths` — the way every read query sees it now.
async fn seed_file_date(
    db: &DbManager,
    episode_id: &str,
    file_path: &str,
    hash: &str,
    date: chrono::NaiveDateTime,
) {
    db.upsert_content(crate::db::fingerprints::ContentRecord {
        fingerprint: hash,
        media_info: None,
        path: file_path,
    })
    .await
    .unwrap();
    db.upsert_path(crate::db::fingerprints::PathRecord {
        file_path,
        fingerprint: hash,
        inode: 0,
        device: 0,
        size: 1,
        mtime: 0.0,
        state: "complete",
        expected_path: None,
    })
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO release_info (quick_hash, upload_date) VALUES (?, ?) \
         ON CONFLICT(quick_hash) DO UPDATE SET upload_date = excluded.upload_date",
    )
    .bind(hash)
    .bind(date.format("%Y-%m-%d %H:%M:%S").to_string())
    .execute(db.get_pool())
    .await
    .unwrap();
    db.associate_main_file(episode_id, file_path, None)
        .await
        .unwrap();
}

#[tokio::test]
async fn test_trigger_estimation_populates_undated_episodes() {
    // trigger_estimation_for_series generates est_dates for undated episodes using
    // upload_date anchors from the same series.
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test.db");
    let db = DbManager::new(&db_path).await.unwrap();

    let series_id = "ts-trigger-est";
    let mapping = MappingRule {
        target_title: "Trigger Est Series".to_string(),
        series_id: series_id.to_string(),
        name: "Trigger Est Series".to_string(),
        ..Default::default()
    };
    db.upsert_series_mapping(series_id, &mapping).await.unwrap();

    // Second precision: the file date round-trips through `release_info` TEXT.
    use chrono::Timelike;
    let anchor_date = (chrono::Utc::now().naive_utc() + chrono::Duration::days(1))
        .with_nanosecond(0)
        .unwrap();
    let later_date = anchor_date + chrono::Duration::days(7);

    db.insert_episode(crate::db::episodes::InsertEpisodeParams {
        episode_id: "anchor_ep",
        series_id,
        season: 1,
        episode: 1,
        file_path: None,
        title: Some("Anchored"),
        quality_profile_id: None,
        status: "unreleased",
        meta_date: None,
        est_date: None,
        metadata_ids: &HashMap::new(),
        description: None,
        runtime: None,
        image_url: None,
        metadata_source: None,
        numbering_mode: None,
    })
    .await
    .unwrap();

    db.insert_episode(crate::db::episodes::InsertEpisodeParams {
        episode_id: "anchor_ep2",
        series_id,
        season: 1,
        episode: 2,
        file_path: None,
        title: Some("Anchored 2"),
        quality_profile_id: None,
        status: "unreleased",
        meta_date: None,
        est_date: None,
        metadata_ids: &HashMap::new(),
        description: None,
        runtime: None,
        image_url: None,
        metadata_source: None,
        numbering_mode: None,
    })
    .await
    .unwrap();

    db.insert_episode(crate::db::episodes::InsertEpisodeParams {
        episode_id: "undated_ep",
        series_id,
        season: 1,
        episode: 3,
        file_path: None,
        title: Some("Undated"),
        quality_profile_id: None,
        status: "unreleased",
        meta_date: None,
        est_date: None,
        metadata_ids: &HashMap::new(),
        description: None,
        runtime: None,
        image_url: None,
        metadata_source: None,
        numbering_mode: None,
    })
    .await
    .unwrap();

    // The file's date is content-scoped now: give the anchor episodes files whose
    // content carries the source date.
    seed_file_date(&db, "anchor_ep", "/lib/te1.mkv", "te-h1", anchor_date).await;
    seed_file_date(&db, "anchor_ep2", "/lib/te2.mkv", "te-h2", later_date).await;

    crate::release_estimator::trigger_estimation_for_series(&db, series_id)
        .await
        .expect("trigger_estimation_for_series should succeed");

    let ep3 = db
        .get_episode_by_id("undated_ep")
        .await
        .unwrap()
        .expect("undated_ep should exist");
    assert!(
        ep3.est_date.is_some(),
        "undated episode should have received an est_date after trigger"
    );

    let ep1 = db
        .get_episode_by_id("anchor_ep")
        .await
        .unwrap()
        .expect("anchor_ep should exist");
    assert_eq!(
        ep1.upload_date,
        Some(anchor_date),
        "anchor episode's upload_date should be preserved"
    );
}

#[tokio::test]
async fn test_estimation_is_idempotent_across_runs() {
    // Regression: an empty calculate_estimations result (which also occurs when every
    // est_date already matches the projection) used to clear valid est_dates, so
    // alternating runs wiped estimates and left series permanently "missing estimations".
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test.db");
    let db = DbManager::new(&db_path).await.unwrap();

    let series_id = "ts-idempotent-est";
    let mapping = MappingRule {
        target_title: "Idempotent Est Series".to_string(),
        series_id: series_id.to_string(),
        name: "Idempotent Est Series".to_string(),
        ..Default::default()
    };
    db.upsert_series_mapping(series_id, &mapping).await.unwrap();

    let anchor_date = chrono::Utc::now().naive_utc() + chrono::Duration::days(1);
    let later_date = anchor_date + chrono::Duration::days(7);

    for (ep_id, ep_num) in [("anchor_ep", 1i32), ("anchor_ep2", 2), ("undated_ep", 3)] {
        db.insert_episode(InsertEpisodeParams {
            episode_id: ep_id,
            series_id,
            season: 1,
            episode: ep_num,
            file_path: None,
            title: Some("Ep"),
            quality_profile_id: None,
            status: "unreleased",
            meta_date: None,
            est_date: None,
            metadata_ids: &HashMap::new(),
            description: None,
            runtime: None,
            image_url: None,
            metadata_source: None,
            numbering_mode: None,
        })
        .await
        .unwrap();
    }

    // File-scoped source dates live in release_info, resolved through file_paths.
    seed_file_date(&db, "anchor_ep", "/lib/id1.mkv", "id-h1", anchor_date).await;
    seed_file_date(&db, "anchor_ep2", "/lib/id2.mkv", "id-h2", later_date).await;

    crate::release_estimator::trigger_estimation_for_series(&db, series_id)
        .await
        .expect("first estimation should succeed");
    let ep3_first = db
        .get_episode_by_id("undated_ep")
        .await
        .unwrap()
        .expect("undated_ep should exist");
    assert!(
        ep3_first.est_date.is_some(),
        "undated episode should have an est_date after first run"
    );

    // Second run changes nothing; the estimate must survive (not be cleared).
    crate::release_estimator::trigger_estimation_for_series(&db, series_id)
        .await
        .expect("second estimation should succeed");
    let ep3_second = db
        .get_episode_by_id("undated_ep")
        .await
        .unwrap()
        .expect("undated_ep should exist");
    assert_eq!(
        ep3_second.est_date, ep3_first.est_date,
        "est_date must survive an idempotent second estimation run"
    );
}

#[tokio::test]
async fn test_trigger_estimation_for_missing_series_does_not_panic() {
    // Calling trigger_estimation_for_series on a non-existent series ID
    // should log a warning and return Ok(()), not panic.
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test.db");
    let db = DbManager::new(&db_path).await.unwrap();

    let result =
        crate::release_estimator::trigger_estimation_for_series(&db, "nonexistent-series-id").await;
    assert!(result.is_ok(), "Missing series should return Ok, not panic");
}

#[tokio::test]
async fn test_est_dates_cleared_when_all_source_dates_removed() {
    // When all upload_date values for a season are gone (files deleted or unassigned),
    // the estimator must clear stale est_date rows rather than leave dead projections.
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test.db");
    let db = DbManager::new(&db_path).await.unwrap();

    let series_id = "ts-stale-est";
    let mapping = MappingRule {
        target_title: "Stale Est Series".to_string(),
        series_id: series_id.to_string(),
        name: "Stale Est Series".to_string(),
        ..Default::default()
    };
    db.upsert_series_mapping(series_id, &mapping).await.unwrap();

    let now = chrono::Utc::now().naive_utc();

    // No upload_date but a stale est_date, as left behind after source dates were removed.
    for (ep_id, ep_num, est) in [
        ("ep1", 1i32, now),
        ("ep2", 2, now + chrono::Duration::days(7)),
    ] {
        db.insert_episode(InsertEpisodeParams {
            episode_id: ep_id,
            series_id,
            season: 1,
            episode: ep_num,
            file_path: None,
            title: Some("Stale Ep"),
            quality_profile_id: None,
            status: "unreleased",
            meta_date: None,
            est_date: Some(est),
            metadata_ids: &HashMap::new(),
            description: None,
            runtime: None,
            image_url: None,
            metadata_source: None,
            numbering_mode: None,
        })
        .await
        .unwrap();
    }

    crate::release_estimator::trigger_estimation_for_series(&db, series_id)
        .await
        .expect("trigger_estimation_for_series should succeed");

    for ep_id in ["ep1", "ep2"] {
        let ep = db
            .get_episode_by_id(ep_id)
            .await
            .unwrap()
            .expect("episode should exist");
        assert!(
            ep.est_date.is_none(),
            "est_date for {} should have been cleared (no source dates)",
            ep_id
        );
    }
}

#[tokio::test]
async fn test_est_dates_not_touched_when_no_source_and_no_stale() {
    // A season with neither upload_date nor existing est_date should complete without
    // error and without writing anything (validates the no-op gating).
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test.db");
    let db = DbManager::new(&db_path).await.unwrap();

    let series_id = "ts-clean-est";
    let mapping = MappingRule {
        target_title: "Clean Est Series".to_string(),
        series_id: series_id.to_string(),
        name: "Clean Est Series".to_string(),
        ..Default::default()
    };
    db.upsert_series_mapping(series_id, &mapping).await.unwrap();

    for (ep_id, ep_num) in [("ep1", 1i32), ("ep2", 2)] {
        db.insert_episode(InsertEpisodeParams {
            episode_id: ep_id,
            series_id,
            season: 1,
            episode: ep_num,
            file_path: None,
            title: Some("Clean Ep"),
            quality_profile_id: None,
            status: "unreleased",
            meta_date: None,
            est_date: None,
            metadata_ids: &HashMap::new(),
            description: None,
            runtime: None,
            image_url: None,
            metadata_source: None,
            numbering_mode: None,
        })
        .await
        .unwrap();
    }

    crate::release_estimator::trigger_estimation_for_series(&db, series_id)
        .await
        .expect("trigger_estimation_for_series should succeed");

    for ep_id in ["ep1", "ep2"] {
        let ep = db
            .get_episode_by_id(ep_id)
            .await
            .unwrap()
            .expect("episode should exist");
        assert!(
            ep.est_date.is_none(),
            "est_date for {} should remain None (no-op)",
            ep_id
        );
    }
}

#[tokio::test]
async fn test_est_date_cleared_when_no_upload_date() {
    // The estimator anchors only on upload_date, so an episode with meta_date but no
    // upload_date must have its est_date cleared.
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test.db");
    let db = DbManager::new(&db_path).await.unwrap();

    let series_id = "ts-anchored-est";
    let mapping = MappingRule {
        target_title: "Anchored Est Series".to_string(),
        series_id: series_id.to_string(),
        name: "Anchored Est Series".to_string(),
        ..Default::default()
    };
    db.upsert_series_mapping(series_id, &mapping).await.unwrap();

    let now = chrono::Utc::now().naive_utc();
    let meta_date = now - chrono::Duration::days(30);

    // meta_date and est_date set but no upload_date; the est_date must be cleared
    // since there is no upload_date cadence to anchor it.
    db.insert_episode(InsertEpisodeParams {
        episode_id: "ep1",
        series_id,
        season: 1,
        episode: 1,
        file_path: None,
        title: Some("Anchored Ep"),
        quality_profile_id: None,
        status: "unreleased",
        meta_date: Some(meta_date),
        est_date: Some(now),
        metadata_ids: &HashMap::new(),
        description: None,
        runtime: None,
        image_url: None,
        metadata_source: None,
        numbering_mode: None,
    })
    .await
    .unwrap();

    crate::release_estimator::trigger_estimation_for_series(&db, series_id)
        .await
        .expect("trigger_estimation_for_series should succeed");

    let ep = db
        .get_episode_by_id("ep1")
        .await
        .unwrap()
        .expect("episode should exist");
    assert!(
        ep.est_date.is_none(),
        "est_date should be cleared when no upload_date exists"
    );
}

// Auto-assignment (only_unassigned=true) skips if the file has ANY episode; manual
// assignment (only_unassigned=false) skips only the SAME episode and allows reassign.

async fn setup_assign_test() -> (tempfile::TempDir, DbManager, String, String, String) {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test.db");
    let db = DbManager::new(&db_path).await.unwrap();

    let series_id = "ts-assign-test";
    let series_title = "Assign Test";
    let mapping = MappingRule {
        target_title: series_title.to_string(),
        series_id: series_id.to_string(),
        name: series_title.to_string(),
        ..Default::default()
    };
    db.upsert_series_mapping(series_id, &mapping).await.unwrap();

    let ep1_id = format!("{}_1_1", series_id);
    let ep2_id = format!("{}_1_2", series_id);

    db.insert_episode(crate::db::episodes::InsertEpisodeParams {
        episode_id: &ep1_id,
        series_id,
        season: 1,
        episode: 1,
        file_path: Some("/media/series/E01.mkv"),
        title: Some("Episode 1"),
        quality_profile_id: None,
        status: "organized",
        meta_date: None,
        est_date: None,
        metadata_ids: &HashMap::new(),
        description: None,
        runtime: None,
        image_url: None,
        metadata_source: None,
        numbering_mode: None,
    })
    .await
    .unwrap();

    db.insert_episode(crate::db::episodes::InsertEpisodeParams {
        episode_id: &ep2_id,
        series_id,
        season: 1,
        episode: 2,
        file_path: None,
        title: Some("Episode 2"),
        quality_profile_id: None,
        status: "missing",
        meta_date: None,
        est_date: None,
        metadata_ids: &HashMap::new(),
        description: None,
        runtime: None,
        image_url: None,
        metadata_source: None,
        numbering_mode: None,
    })
    .await
    .unwrap();

    (tmp, db, series_id.to_string(), ep1_id, ep2_id)
}

/// Regression: an episode's file date is resolved from `release_info` through its
/// CURRENT file's fingerprint. Assigning a file that has no release metadata must
/// resolve to no date — it must not inherit the date of the file it replaced. The
/// source-feed precedence is covered by `test_assign_resolves_release_feed_date`.
#[tokio::test]
async fn test_assign_does_not_inherit_the_replaced_files_date() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test.db");
    let db = DbManager::new(&db_path).await.unwrap();

    let series_id = "ts-upload-date";
    let mapping = MappingRule {
        target_title: "Upload Date Series".to_string(),
        series_id: series_id.to_string(),
        name: "Upload Date Series".to_string(),
        ..Default::default()
    };
    db.upsert_series_mapping(series_id, &mapping).await.unwrap();

    let ep_id = format!("{}_1_1", series_id);
    let old_path = "/old/old.mkv";
    db.insert_episode(InsertEpisodeParams {
        episode_id: &ep_id,
        series_id,
        season: 1,
        episode: 1,
        file_path: Some(old_path),
        title: Some("Episode 1"),
        quality_profile_id: None,
        status: "organized",
        meta_date: None,
        est_date: None,
        metadata_ids: &HashMap::new(),
        description: None,
        runtime: None,
        image_url: None,
        metadata_source: None,
        numbering_mode: None,
    })
    .await
    .unwrap();

    // The old file's content carries a date; the replacement's content carries none.
    let stale = chrono::NaiveDate::from_ymd_opt(2020, 1, 1)
        .unwrap()
        .and_hms_opt(0, 0, 0)
        .unwrap();
    seed_file_date(&db, &ep_id, old_path, "oldhash", stale).await;

    let media_dir = tmp.path().join("series");
    std::fs::create_dir_all(&media_dir).unwrap();
    let file_path = media_dir.join("E01.mkv");
    std::fs::write(&file_path, b"x").unwrap();
    let file_path_str = file_path.to_string_lossy().to_string();

    db.assign_file_to_episode(AssignFileToEpisodeParams {
        episode_id: &ep_id,
        series_id,
        series_title: "Upload Date Series",
        season: 1,
        episode: 1,
        file_path: &file_path_str,
        episode_title: "",
        all_target_episode_ids: None,
        numbering_mode: 0,
        only_unassigned: false,
    })
    .await
    .unwrap();

    let row = db
        .get_episode_by_id(&ep_id)
        .await
        .unwrap()
        .expect("episode should exist");
    assert_eq!(row.file_path.as_deref(), Some(file_path_str.as_str()));
    assert_eq!(
        row.upload_date, None,
        "a file without release metadata must resolve to no date, not the replaced file's"
    );
}

/// Regression: reorganizing a multi-episode file (two episode rows share one
/// file_path, e.g. S01E01E02.mkv) must update ALL covered episodes. Mirrors
/// `handle_reorganization_success`, which calls `assign_file_to_episode` once
/// per covered episode with `only_unassigned: true` and the covered set as
/// `all_target_episode_ids`. Regression: the second episode was skipped because
/// the file was already held by the first (a target), leaving a stale path.
#[tokio::test]
async fn test_reorg_multi_episode_sets_path_on_all_covered() {
    let tmp = tempfile::tempdir().unwrap();
    let db = DbManager::new(&tmp.path().join("test.db")).await.unwrap();
    let series_id = "ts-multi-reorg";
    let mapping = MappingRule {
        target_title: "Multi Reorg".to_string(),
        series_id: series_id.to_string(),
        name: "Multi Reorg".to_string(),
        ..Default::default()
    };
    db.upsert_series_mapping(series_id, &mapping).await.unwrap();

    let ep1 = format!("{}_1_1", series_id);
    let ep2 = format!("{}_1_2", series_id);
    let old = "/media/old/S01E01E02.mkv";
    for (id, n) in [(&ep1, 1), (&ep2, 2)] {
        db.insert_episode(InsertEpisodeParams {
            episode_id: id,
            series_id,
            season: 1,
            episode: n,
            file_path: Some(old),
            title: None,
            quality_profile_id: None,
            status: "organized",
            meta_date: None,
            est_date: None,
            metadata_ids: &HashMap::new(),
            description: None,
            runtime: None,
            image_url: None,
            metadata_source: None,
            numbering_mode: None,
        })
        .await
        .unwrap();
    }

    // Simulate the post-move destination.
    let dir = tmp.path().join("new");
    std::fs::create_dir_all(&dir).unwrap();
    let new_file = dir.join("Multi Reorg - S01E01E02.mkv");
    std::fs::write(&new_file, b"x").unwrap();
    let new_str = new_file.to_string_lossy().to_string();
    let targets = vec![ep1.clone(), ep2.clone()];

    // Same call handle_reorganization_success makes, once per covered episode.
    for (id, n) in [(&ep1, 1), (&ep2, 2)] {
        db.assign_file_to_episode(AssignFileToEpisodeParams {
            episode_id: id,
            series_id,
            series_title: "Multi Reorg",
            season: 1,
            episode: n,
            file_path: &new_str,
            episode_title: "",
            all_target_episode_ids: Some(&targets),
            numbering_mode: 0,
            only_unassigned: true,
        })
        .await
        .unwrap();
    }

    let p1: Option<String> = db.get_episode_file_path(&ep1).await.unwrap();
    let p2: Option<String> = db.get_episode_file_path(&ep2).await.unwrap();
    assert_eq!(p1.as_deref(), Some(new_str.as_str()), "covered ep1 path");
    assert_eq!(p2.as_deref(), Some(new_str.as_str()), "covered ep2 path");
}

/// Regression: the episode's file date is resolved from the release's source-feed
/// date (`release_info`, keyed by content hash) through the CURRENT file — never
/// the file mtime.
#[tokio::test]
async fn test_assign_resolves_release_feed_date() {
    let tmp = tempfile::tempdir().unwrap();
    let db = DbManager::new(&tmp.path().join("test.db")).await.unwrap();

    let series_id = "ts-feed-date";
    let mapping = MappingRule {
        target_title: "Feed Date".to_string(),
        series_id: series_id.to_string(),
        name: "Feed Date".to_string(),
        ..Default::default()
    };
    db.upsert_series_mapping(series_id, &mapping).await.unwrap();

    let ep = format!("{}_1_1", series_id);
    // Episode currently holds an old file.
    db.insert_episode(InsertEpisodeParams {
        episode_id: &ep,
        series_id,
        season: 1,
        episode: 1,
        file_path: Some("/old/old.mkv"),
        title: None,
        quality_profile_id: None,
        status: "organized",
        meta_date: None,
        est_date: None,
        metadata_ids: &HashMap::new(),
        description: None,
        runtime: None,
        image_url: None,
        metadata_source: None,
        numbering_mode: None,
    })
    .await
    .unwrap();

    // New file on disk with a known release feed date for its content. mtime is
    // deliberately far from the feed date (1s after the epoch) so a mtime stamp
    // cannot pass by coincidence.
    let new_file = tmp.path().join("new.mkv");
    std::fs::write(&new_file, b"x").unwrap();
    let new_path = new_file.to_string_lossy().to_string();
    // Both the path and its content row — the read path joins through
    // `file_paths.fingerprint -> file_contents.fingerprint -> release_info`.
    db.upsert_content(crate::db::fingerprints::ContentRecord {
        fingerprint: "hashY",
        media_info: None,
        path: &new_path,
    })
    .await
    .unwrap();
    db.upsert_path(crate::db::fingerprints::PathRecord {
        file_path: &new_path,
        fingerprint: "hashY",
        inode: 0,
        device: 0,
        size: 1,
        mtime: 1.0,
        state: "complete",
        expected_path: None,
    })
    .await
    .unwrap();
    sqlx::query("INSERT INTO release_info (quick_hash, upload_date) VALUES (?, ?)")
        .bind("hashY")
        .bind("2026-06-18 20:00:00")
        .execute(db.get_pool())
        .await
        .unwrap();

    db.assign_file_to_episode(AssignFileToEpisodeParams {
        episode_id: &ep,
        series_id,
        series_title: "Feed Date",
        season: 1,
        episode: 1,
        file_path: &new_path,
        episode_title: "",
        all_target_episode_ids: None,
        numbering_mode: 0,
        only_unassigned: false,
    })
    .await
    .unwrap();

    let row = db
        .get_episode_by_id(&ep)
        .await
        .unwrap()
        .expect("episode should exist");
    assert_eq!(row.file_path.as_deref(), Some(new_path.as_str()));
    assert_eq!(
        row.upload_date,
        Some(
            chrono::NaiveDate::from_ymd_opt(2026, 6, 18)
                .unwrap()
                .and_hms_opt(20, 0, 0)
                .unwrap()
        ),
        "the episode's file date must come from release_info, not the file mtime"
    );
}

/// Regression: the "file already on an episode" guard must be scoped to the
/// caller's numbering_mode, so an absolute-mode row holding a file does not
/// block assigning the same path to the normal-mode row that represents the
/// same episode.
#[tokio::test]
async fn test_assign_holder_guard_is_mode_scoped() {
    let tmp = tempfile::tempdir().unwrap();
    let db = DbManager::new(&tmp.path().join("test.db")).await.unwrap();

    let series_id = "ts-mode-scope";
    let mapping = MappingRule {
        target_title: "Mode Scope".to_string(),
        series_id: series_id.to_string(),
        name: "Mode Scope".to_string(),
        ..Default::default()
    };
    db.upsert_series_mapping(series_id, &mapping).await.unwrap();

    let file = tmp.path().join("dual.mkv");
    std::fs::write(&file, b"x").unwrap();
    let path = file.to_string_lossy().to_string();

    // Absolute-mode row already holds the file; normal-mode row is empty.
    let ep_abs = format!("{}_ABS_1_1", series_id);
    let ep_norm = format!("{}_1_1", series_id);
    for (id, mode, fp) in [
        (ep_abs.as_str(), Some(1), Some(path.as_str())),
        (ep_norm.as_str(), Some(0), None),
    ] {
        db.insert_episode(InsertEpisodeParams {
            episode_id: id,
            series_id,
            season: 1,
            episode: 1,
            file_path: fp,
            title: None,
            quality_profile_id: None,
            status: "organized",
            meta_date: None,
            est_date: None,
            metadata_ids: &HashMap::new(),
            description: None,
            runtime: None,
            image_url: None,
            metadata_source: None,
            numbering_mode: mode,
        })
        .await
        .unwrap();
    }

    // Auto-assign (skip-if-taken) the same file to the normal-mode row.
    db.assign_file_to_episode(AssignFileToEpisodeParams {
        episode_id: &ep_norm,
        series_id,
        series_title: "Mode Scope",
        season: 1,
        episode: 1,
        file_path: &path,
        episode_title: "",
        all_target_episode_ids: None,
        numbering_mode: 0,
        only_unassigned: true,
    })
    .await
    .unwrap();

    let fp: Option<String> = db.get_episode_file_path(&ep_norm).await.unwrap();
    assert_eq!(
        fp.as_deref(),
        Some(path.as_str()),
        "normal-mode row must receive the file even though the absolute-mode row holds it"
    );
}

/// Auto-assign with the file already on the target episode is a no-op.
#[tokio::test]
async fn test_auto_assign_skips_same_episode() {
    let (_tmp, db, _series_id, ep1_id, _ep2_id) = setup_assign_test().await;

    db.assign_file_to_episode(AssignFileToEpisodeParams {
        episode_id: &ep1_id,
        series_id: &_series_id,
        series_title: "Test Series",
        season: 1,
        episode: 1,
        file_path: "/media/series/E01.mkv",
        episode_title: "Episode 1",
        all_target_episode_ids: None,
        numbering_mode: 0,
        only_unassigned: true,
    })
    .await
    .unwrap();

    let file_path: Option<String> = db.get_episode_file_path(&ep1_id).await.unwrap();
    assert_eq!(file_path, Some("/media/series/E01.mkv".to_string()));
}

/// Auto-assign must not move a file that is already on any episode.
#[tokio::test]
async fn test_auto_assign_skips_when_file_on_any_episode() {
    let (_tmp, db, _series_id, ep1_id, ep2_id) = setup_assign_test().await;

    db.assign_file_to_episode(AssignFileToEpisodeParams {
        episode_id: &ep2_id,
        series_id: &_series_id,
        series_title: "Test Series",
        season: 1,
        episode: 2,
        file_path: "/media/series/E01.mkv",
        episode_title: "Episode 2",
        all_target_episode_ids: None,
        numbering_mode: 0,
        only_unassigned: true,
    })
    .await
    .unwrap();

    let ep1_file: Option<String> = db.get_episode_file_path(&ep1_id).await.unwrap();
    assert_eq!(
        ep1_file,
        Some("/media/series/E01.mkv".to_string()),
        "File should remain on ep1 — auto-assign should not move it"
    );

    let ep2_file: Option<String> = db.get_episode_file_path(&ep2_id).await.unwrap();
    assert_eq!(
        ep2_file, None,
        "Ep2 should still have no file — auto-assign should not have moved the file"
    );
}

/// Auto-assign proceeds when the file is on no episode.
#[tokio::test]
async fn test_auto_assign_proceeds_for_unassigned_file() {
    let (_tmp, db, _series_id, _ep1_id, ep2_id) = setup_assign_test().await;

    db.assign_file_to_episode(AssignFileToEpisodeParams {
        episode_id: &ep2_id,
        series_id: &_series_id,
        series_title: "Test Series",
        season: 1,
        episode: 2,
        file_path: "/media/series/E02_new.mkv",
        episode_title: "Episode 2",
        all_target_episode_ids: None,
        numbering_mode: 0,
        only_unassigned: true,
    })
    .await
    .unwrap();

    let file_path: Option<String> = db.get_episode_file_path(&ep2_id).await.unwrap();
    assert_eq!(
        file_path,
        Some("/media/series/E02_new.mkv".to_string()),
        "Ep2 should have the new file assigned"
    );
}

/// Manual assign may move a file from one episode to another.
#[tokio::test]
async fn test_manual_assign_allows_reassign_to_different_episode() {
    let (_tmp, db, _series_id, ep1_id, ep2_id) = setup_assign_test().await;
    let pool = db.get_pool();

    db.assign_file_to_episode(AssignFileToEpisodeParams {
        episode_id: &ep2_id,
        series_id: &_series_id,
        series_title: "Test Series",
        season: 1,
        episode: 2,
        file_path: "/media/series/E01.mkv",
        episode_title: "Episode 2",
        all_target_episode_ids: None,
        numbering_mode: 0,
        only_unassigned: false,
    })
    .await
    .unwrap();

    let ep2_file: Option<String> = db.get_episode_file_path(&ep2_id).await.unwrap();
    assert_eq!(
        ep2_file,
        Some("/media/series/E01.mkv".to_string()),
        "Manual reassign should move file to ep2"
    );

    let ep1_status: Option<String> =
        sqlx::query_scalar("SELECT status FROM episodes WHERE episode_id = ?")
            .bind(&ep1_id)
            .fetch_optional(pool)
            .await
            .unwrap()
            .flatten();
    assert_eq!(
        ep1_status,
        Some("missing".to_string()),
        "Ep1 should be missing after reassign"
    );
}

/// Manual assign of the same file to the same episode is a no-op.
#[tokio::test]
async fn test_manual_assign_skips_same_episode() {
    let (_tmp, db, _series_id, ep1_id, _ep2_id) = setup_assign_test().await;

    db.assign_file_to_episode(AssignFileToEpisodeParams {
        episode_id: &ep1_id,
        series_id: &_series_id,
        series_title: "Test Series",
        season: 1,
        episode: 1,
        file_path: "/media/series/E01.mkv",
        episode_title: "Episode 1",
        all_target_episode_ids: None,
        numbering_mode: 0,
        only_unassigned: false,
    })
    .await
    .unwrap();

    let file_path: Option<String> = db.get_episode_file_path(&ep1_id).await.unwrap();
    assert_eq!(
        file_path,
        Some("/media/series/E01.mkv".to_string()),
        "Manual assign to same episode should be a no-op"
    );
}

// remove_completed_queue_item is status-filtered so a Completed item doesn't linger
// when a newer upgrade queue item exists for the same episode_id.

#[tokio::test]
async fn test_remove_completed_queue_item_removes_completed() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test.db");
    let db = DbManager::new(&db_path).await.unwrap();

    let episode_id = "cleanup_S01E01";
    let series_id = "cleanup-test";

    // The DOWNLOAD_QUEUE_BASE JOIN requires a series mapping.
    let mapping = MappingRule {
        target_title: "Cleanup Test".to_string(),
        series_id: series_id.to_string(),
        name: "cleanup_test".to_string(),
        ..Default::default()
    };
    db.upsert_series_mapping(series_id, &mapping).await.unwrap();

    // Episode must exist first (FK constraint).
    sqlx::query(
        "INSERT INTO episodes (episode_id, series_id, season, episode, status) VALUES (?, ?, 1, 1, 'downloaded')",
    )
    .bind(episode_id)
    .bind(series_id)
    .execute(db.get_pool())
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO download_queue (media_name, media_link, series_title, series_id, season, episode, episode_id, score, is_user_requested, status, downloaded_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, datetime('now'))",
    )
    .bind("Cleanup Test S01E01")
    .bind("magnet:?xt=urn:btih:cleanup1")
    .bind("Cleanup Test")
    .bind(series_id)
    .bind("1")
    .bind(1)
    .bind(episode_id)
    .bind(100)
    .bind(false)
    .bind("Completed")
    .execute(db.get_pool())
    .await
    .unwrap();

    let removed = db.remove_completed_queue_item(episode_id).await.unwrap();
    assert!(removed, "Should have removed the Completed queue item");

    let remaining: Option<(i64,)> =
        sqlx::query_as("SELECT id FROM download_queue WHERE episode_id = ?")
            .bind(episode_id)
            .fetch_optional(db.get_pool())
            .await
            .unwrap();
    assert!(remaining.is_none(), "Queue should be empty after removal");
}

#[tokio::test]
async fn test_remove_completed_queue_item_skips_queued() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test.db");
    let db = DbManager::new(&db_path).await.unwrap();

    let episode_id = "skip_queued_S01E01";
    let series_id = "skip-queued-test";

    let mapping = MappingRule {
        target_title: "Skip Queued Test".to_string(),
        series_id: series_id.to_string(),
        name: "skip_queued_test".to_string(),
        ..Default::default()
    };
    db.upsert_series_mapping(series_id, &mapping).await.unwrap();

    sqlx::query(
        "INSERT INTO episodes (episode_id, series_id, season, episode, status) VALUES (?, ?, 1, 1, 'downloaded')",
    )
    .bind(episode_id)
    .bind(series_id)
    .execute(db.get_pool())
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO download_queue (media_name, media_link, series_title, series_id, season, episode, episode_id, score, is_user_requested, status, downloaded_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, datetime('now'))",
    )
    .bind("Skip Queued S01E01")
    .bind("magnet:?xt=urn:btih:skip_queued")
    .bind("Skip Queued Test")
    .bind(series_id)
    .bind("1")
    .bind(1)
    .bind(episode_id)
    .bind(100)
    .bind(false)
    .bind("Queued")
    .execute(db.get_pool())
    .await
    .unwrap();

    let removed = db.remove_completed_queue_item(episode_id).await.unwrap();
    assert!(!removed, "Should NOT remove a Queued item");

    let status: Option<String> =
        sqlx::query_scalar("SELECT status FROM download_queue WHERE episode_id = ?")
            .bind(episode_id)
            .fetch_optional(db.get_pool())
            .await
            .unwrap()
            .flatten();
    assert_eq!(
        status.as_deref(),
        Some("Queued"),
        "Queued item should survive"
    );
}

#[tokio::test]
async fn test_remove_completed_queue_item_removes_only_completed_when_duplicate_exists() {
    // Regression: when add_to_download_queue leaves a "Completed" entry alongside a
    // newer duplicate, remove_completed_queue_item must remove ONLY the Completed one.
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test.db");
    let db = DbManager::new(&db_path).await.unwrap();

    let episode_id = "regression_dup_S01E01";
    let series_id = "regression-dup-test";

    let mapping = MappingRule {
        target_title: "Regression Dup Test".to_string(),
        series_id: series_id.to_string(),
        name: "regression_dup_test".to_string(),
        ..Default::default()
    };
    db.upsert_series_mapping(series_id, &mapping).await.unwrap();

    sqlx::query(
        "INSERT INTO episodes (episode_id, series_id, season, episode, status) VALUES (?, ?, 1, 1, 'downloaded')",
    )
    .bind(episode_id)
    .bind(series_id)
    .execute(db.get_pool())
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO download_queue (media_name, media_link, series_title, series_id, season, episode, episode_id, score, is_user_requested, status, downloaded_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, '2025-01-01 10:00:00')",
    )
    .bind("Old Completed S01E01")
    .bind("magnet:?xt=urn:btih:old_completed")
    .bind("Regression Dup Test")
    .bind(series_id)
    .bind("1")
    .bind(1)
    .bind(episode_id)
    .bind(50)
    .bind(false)
    .bind("Completed")
    .execute(db.get_pool())
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO download_queue (media_name, media_link, series_title, series_id, season, episode, episode_id, score, is_user_requested, status, downloaded_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, '2025-01-02 10:00:00')",
    )
    .bind("New Upgrade S01E01")
    .bind("magnet:?xt=urn:btih:new_upgrade")
    .bind("Regression Dup Test")
    .bind(series_id)
    .bind("1")
    .bind(1)
    .bind(episode_id)
    .bind(200)
    .bind(false)
    .bind("Queued")
    .execute(db.get_pool())
    .await
    .unwrap();

    let removed = db.remove_completed_queue_item(episode_id).await.unwrap();
    assert!(removed, "Should have removed the Completed queue item");

    let remaining: Vec<(String, String)> =
        sqlx::query_as("SELECT media_name, status FROM download_queue WHERE episode_id = ?")
            .bind(episode_id)
            .fetch_all(db.get_pool())
            .await
            .unwrap();
    assert_eq!(remaining.len(), 1, "Only one item should remain");
    assert_eq!(
        remaining[0].0, "New Upgrade S01E01",
        "The newer upgrade item should survive"
    );
    assert_eq!(
        remaining[0].1, "Queued",
        "The upgrade item should still be Queued"
    );
}

// Import events show "Series - S01E03" as title and the file path as details.
#[tokio::test]
async fn test_activity_import_display() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test.db");
    let db = DbManager::new(&db_path).await.unwrap();

    db.record_activity(ActivityEvent {
        event_type: ActivityType::Import,
        series_title: "Test Series".to_string(),
        season: Some("1".to_string()),
        episode: Some(3),
        episode_end: None,
        title: None,
        details: Some("/media/series/Test.Series.S01E03.mkv".to_string()),
        status: "Success".to_string(),
    })
    .await
    .unwrap();

    let activity = db.get_recent_activity(50).await.unwrap();
    let imp = activity
        .iter()
        .find(|a| a.activity_type == ActivityType::Import)
        .expect("Should have Import event");
    assert_eq!(
        imp.title, "Test Series - S01E03",
        "Import title should be Series - SxxEyy"
    );
    assert_eq!(
        imp.details.as_deref(),
        Some("/media/series/Test.Series.S01E03.mkv"),
        "Import details should be file path"
    );
}

// Analyze events show "Series - S01E03" as title and the media summary as details.
#[tokio::test]
async fn test_activity_analyze_display() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test.db");
    let db = DbManager::new(&db_path).await.unwrap();

    db.record_activity(ActivityEvent {
        event_type: ActivityType::Analyze,
        series_title: "Test Series".to_string(),
        season: Some("1".to_string()),
        episode: Some(3),
        episode_end: None,
        title: None,
        details: Some("h265 1080p".to_string()),
        status: "Success".to_string(),
    })
    .await
    .unwrap();

    let activity = db.get_recent_activity(50).await.unwrap();
    let a = activity
        .iter()
        .find(|a| a.activity_type == ActivityType::Analyze)
        .expect("Should have Analyze event");
    assert_eq!(
        a.title, "Test Series - S01E03",
        "Analyze title should be Series - SxxEyy"
    );
    assert_eq!(
        a.details.as_deref(),
        Some("h265 1080p"),
        "Analyze details should be media summary"
    );
}

// Metadata events show the series name as title and the episode range as details.
#[tokio::test]
async fn test_activity_metadata_display() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test.db");
    let db = DbManager::new(&db_path).await.unwrap();

    db.record_activity(ActivityEvent {
        event_type: ActivityType::Metadata,
        series_title: "Test Series".to_string(),
        season: Some("1".to_string()),
        episode: Some(1),
        episode_end: Some(10),
        title: None,
        details: Some("S01E01-E10".to_string()),
        status: "Success".to_string(),
    })
    .await
    .unwrap();

    let activity = db.get_recent_activity(50).await.unwrap();
    let m = activity
        .iter()
        .find(|a| a.activity_type == ActivityType::Metadata)
        .expect("Should have Metadata event");
    assert_eq!(
        m.title, "Test Series",
        "Metadata title should be series name"
    );
    assert_eq!(
        m.details.as_deref(),
        Some("S01E01-E10"),
        "Metadata details should be episode range"
    );
}

// Multiple Metadata events for the same series within 5 min merge into one row.
#[tokio::test]
async fn test_activity_metadata_merge() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test.db");
    let db = DbManager::new(&db_path).await.unwrap();

    // Two consecutive ranges merge into one row with details "S01E01-E06".
    db.record_activity(ActivityEvent {
        event_type: ActivityType::Metadata,
        series_title: "Test Series".to_string(),
        season: Some("1".to_string()),
        episode: Some(1),
        episode_end: Some(3),
        title: None,
        details: Some("S01E01-E03".to_string()),
        status: "Success".to_string(),
    })
    .await
    .unwrap();

    db.record_activity(ActivityEvent {
        event_type: ActivityType::Metadata,
        series_title: "Test Series".to_string(),
        season: Some("1".to_string()),
        episode: Some(4),
        episode_end: Some(6),
        title: None,
        details: Some("S01E04-E06".to_string()),
        status: "Success".to_string(),
    })
    .await
    .unwrap();

    let activity = db.get_recent_activity(50).await.unwrap();
    let metadata_events: Vec<_> = activity
        .iter()
        .filter(|a| a.activity_type == ActivityType::Metadata)
        .collect();
    assert_eq!(
        metadata_events.len(),
        1,
        "Metadata events should be merged into one"
    );
    assert_eq!(
        metadata_events[0].title, "Test Series",
        "Merged metadata title should be series name"
    );
    assert_eq!(
        metadata_events[0].details.as_deref(),
        Some("S01E01-E06"),
        "Merged metadata details should show consolidated range"
    );
}

// Metadata events for different seasons of the same series merge into one row.
#[tokio::test]
async fn test_activity_metadata_merge_multi_season() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test.db");
    let db = DbManager::new(&db_path).await.unwrap();

    db.record_activity(ActivityEvent {
        event_type: ActivityType::Metadata,
        series_title: "Test Series".to_string(),
        season: Some("1".to_string()),
        episode: Some(1),
        episode_end: Some(10),
        title: None,
        details: Some("S01E01-E10".to_string()),
        status: "Success".to_string(),
    })
    .await
    .unwrap();

    db.record_activity(ActivityEvent {
        event_type: ActivityType::Metadata,
        series_title: "Test Series".to_string(),
        season: Some("2".to_string()),
        episode: Some(1),
        episode_end: Some(8),
        title: None,
        details: Some("S02E01-E08".to_string()),
        status: "Success".to_string(),
    })
    .await
    .unwrap();

    let activity = db.get_recent_activity(50).await.unwrap();
    let metadata_events: Vec<_> = activity
        .iter()
        .filter(|a| a.activity_type == ActivityType::Metadata)
        .collect();
    assert_eq!(
        metadata_events.len(),
        1,
        "Multi-season Metadata events should be merged into one"
    );
    assert_eq!(
        metadata_events[0].title, "Test Series",
        "Merged metadata title should be series name"
    );
    assert_eq!(
        metadata_events[0].details.as_deref(),
        Some("S01E01-E10, S02E01-E08"),
        "Multi-season merged details should show both season ranges"
    );
}

// Import events without season/episode fall back to the series_title.
#[tokio::test]
async fn test_activity_import_no_episode_fallback() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test.db");
    let db = DbManager::new(&db_path).await.unwrap();

    db.record_activity(ActivityEvent {
        event_type: ActivityType::Import,
        series_title: "Unknown File".to_string(),
        season: None,
        episode: None,
        episode_end: None,
        title: None,
        details: Some("/path/to/file.mkv".to_string()),
        status: "Success".to_string(),
    })
    .await
    .unwrap();

    let activity = db.get_recent_activity(50).await.unwrap();
    let imp = activity
        .iter()
        .find(|a| a.activity_type == ActivityType::Import)
        .expect("Should have Import event");
    assert_eq!(
        imp.title, "Unknown File",
        "Import without episode should show series_title as-is"
    );
}

// Analyze events without season/episode show the series_title.
#[tokio::test]
async fn test_activity_analyze_fallback() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test.db");
    let db = DbManager::new(&db_path).await.unwrap();

    // Mirrors call sites (state.rs / files.rs) where season/episode aren't available.
    db.record_activity(ActivityEvent {
        event_type: ActivityType::Analyze,
        series_title: String::new(),
        season: None,
        episode: None,
        episode_end: None,
        title: None,
        details: Some("/path/to/file.mkv".to_string()),
        status: "Success".to_string(),
    })
    .await
    .unwrap();

    let activity = db.get_recent_activity(50).await.unwrap();
    let a = activity
        .iter()
        .find(|a| a.activity_type == ActivityType::Analyze)
        .expect("Should have Analyze event");
    // Empty series_title is derived from the path's filename.
    assert_eq!(
        a.title, "file.mkv",
        "Non-enriched Analyze should show derived filename"
    );
    assert_eq!(
        a.details.as_deref(),
        Some("/path/to/file.mkv"),
        "Analyze details should be file path"
    );
}

/// Assigning a shared multi-episode file to an episode that conflicts with its
/// current range disowns the ENTIRE range, not just one covered episode.
#[tokio::test]
async fn test_assignment_conflict_disowns_the_whole_multi_episode_range() {
    let tmp = tempfile::tempdir().unwrap();
    let db = DbManager::new(&tmp.path().join("test.db")).await.unwrap();

    let series_id = "ts-conflict-range";
    let mapping = MappingRule {
        target_title: "Conflict Range".to_string(),
        series_id: series_id.to_string(),
        name: "Conflict Range".to_string(),
        ..Default::default()
    };
    db.upsert_series_mapping(series_id, &mapping).await.unwrap();

    let shared = "/media/Conflict Range - S01E01E02.mkv";
    let e1 = format!("{}_S01E01", series_id);
    let e2 = format!("{}_S01E02", series_id);
    let e5 = format!("{}_S01E05", series_id);
    for (id, n) in [(&e1, 1), (&e2, 2), (&e5, 5)] {
        db.insert_episode(InsertEpisodeParams {
            episode_id: id,
            series_id,
            season: 1,
            episode: n,
            file_path: None,
            title: None,
            quality_profile_id: None,
            status: "missing",
            meta_date: None,
            est_date: None,
            metadata_ids: &HashMap::new(),
            description: None,
            runtime: None,
            image_url: None,
            metadata_source: None,
            numbering_mode: None,
        })
        .await
        .unwrap();
    }

    // A multi-episode file legitimately shared by E01 and E02 (both are targets).
    for (id, n) in [(&e1, 1), (&e2, 2)] {
        db.assign_file_to_episode(AssignFileToEpisodeParams {
            episode_id: id,
            series_id,
            series_title: "Conflict Range",
            season: 1,
            episode: n,
            file_path: shared,
            episode_title: "",
            all_target_episode_ids: Some(&[e1.clone(), e2.clone()]),
            numbering_mode: 0,
            only_unassigned: true,
        })
        .await
        .unwrap();
    }

    // Assigning the same file to E05 conflicts with the E01–E02 range.
    db.assign_file_to_episode(AssignFileToEpisodeParams {
        episode_id: &e5,
        series_id,
        series_title: "Conflict Range",
        season: 1,
        episode: 5,
        file_path: shared,
        episode_title: "",
        all_target_episode_ids: None,
        numbering_mode: 0,
        only_unassigned: false,
    })
    .await
    .unwrap();

    for id in [&e1, &e2] {
        let row = db.get_episode_by_id(id).await.unwrap().unwrap();
        assert_eq!(
            row.file_path, None,
            "the whole displaced range must be disowned: {id}"
        );
    }
    let e5_row = db.get_episode_by_id(&e5).await.unwrap().unwrap();
    assert_eq!(e5_row.file_path.as_deref(), Some(shared));
}

/// The calendar range query is a SUPERSET over all three date sources: an episode is
/// included when ANY of `meta_date` / `upload_date` / `est_date` is in range. This is
/// what keeps an in-range episode visible regardless of the user's current
/// effective-date priority/enabled configuration.
#[tokio::test]
async fn test_calendar_range_is_a_superset_over_all_date_sources() {
    let tmp = tempfile::tempdir().unwrap();
    let db = DbManager::new(&tmp.path().join("test.db")).await.unwrap();

    let series_id = "ts-cal-superset";
    let mapping = MappingRule {
        target_title: "Cal Superset".to_string(),
        series_id: series_id.to_string(),
        name: "Cal Superset".to_string(),
        ..Default::default()
    };
    db.upsert_series_mapping(series_id, &mapping).await.unwrap();

    let now = chrono::Utc::now().naive_utc();
    let yesterday = now - chrono::Duration::days(1);
    let next_week = now + chrono::Duration::days(7);
    let long_ago = now - chrono::Duration::days(40);

    async fn insert_ep(
        db: &DbManager,
        series_id: &str,
        id: &str,
        ep: i32,
        meta: Option<chrono::NaiveDateTime>,
        est: Option<chrono::NaiveDateTime>,
    ) {
        db.insert_episode(InsertEpisodeParams {
            episode_id: id,
            series_id,
            season: 1,
            episode: ep,
            file_path: None,
            title: None,
            quality_profile_id: None,
            status: "missing",
            meta_date: meta,
            est_date: est,
            metadata_ids: &HashMap::new(),
            description: None,
            runtime: None,
            image_url: None,
            metadata_source: None,
            numbering_mode: None,
        })
        .await
        .unwrap();
    }

    insert_ep(&db, series_id, "meta_only", 1, Some(now), None).await;
    insert_ep(&db, series_id, "est_only", 2, None, Some(now)).await;
    insert_ep(
        &db,
        series_id,
        "out_of_range",
        3,
        Some(long_ago),
        Some(long_ago),
    )
    .await;
    // upload-only: a file whose source date is in range, no metadata/estimate.
    insert_ep(&db, series_id, "upload_only", 4, None, None).await;
    seed_file_date(&db, "upload_only", "/lib/superset.mkv", "sup-h", now).await;

    let rows = db
        .get_calendar_episodes(yesterday, next_week, false)
        .await
        .unwrap();
    let ids: Vec<&str> = rows.iter().map(|r| r.episode_id.as_str()).collect();
    for expected in ["meta_only", "est_only", "upload_only"] {
        assert!(
            ids.contains(&expected),
            "{expected} must appear: any in-range source qualifies. got {ids:?}"
        );
    }
    assert!(
        !ids.contains(&"out_of_range"),
        "an episode with every source out of range must not appear"
    );
}
