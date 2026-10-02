// Locks down the download → finalize → smart-link → organize chain so attribution
// metadata (quality profile, submitter, file_acquired_at) is never silently dropped.
// Each test maps to a previously-confirmed bug, noted above it.

use crate::db::DbManager;
use crate::db::OrganizeMetaRow;
use crate::db::download_queue::AddToDownloadQueueParams;
use crate::db::episodes::crud::InsertEpisodeParams;
use jumbie_shared::types::AddQueueResult;
use std::collections::HashMap;
use std::sync::Arc;
use tempfile::TempDir;

jumbie_shared::test_module! {

async fn setup_db() -> (Arc<DbManager>, TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test.db");
    let db = Arc::new(DbManager::new(&db_path).await.unwrap());
    (db, tmp)
}

/// Build an episode queue item via the real `add_to_download_queue` so the row
/// exists in the DB and `finalize_download`'s status writes apply.
async fn seed_queue_item(
    db: &DbManager,
    ep_id: &str,
    media_name: &str,
    submitter: Option<&str>,
    quality_profile_id: Option<&str>,
    episode_intentions: Option<&str>,
) -> jumbie_shared::types::DownloadQueueItem {
    // download_queue.series_id is an FK into series_mappings — seed the mapping
    // so queue inserts succeed regardless of caller order.
    db.upsert_series_mapping(
        "test-show",
        &jumbie_shared::types::MappingRule {
            target_title: "Test Show".to_string(),
            name: "test_show".to_string(),
            quality_profile: Some("qp-1".to_string()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    // download_queue.episode_id is an FK into episodes — seed the row too.
    let meta_ids = HashMap::new();
    db.insert_episode(InsertEpisodeParams::dummy(ep_id, "test-show", 1, 1, &meta_ids))
        .await
        .unwrap();
    let result = db
        .add_to_download_queue(AddToDownloadQueueParams {
            media_name,
            media_link: "magnet:?xt=urn:btih:download-pipeline-test",
            series_title: "Test Show",
            series_id: "test-show",
            seasons: &[1],
            episodes: &[1],
            episode_id: Some(ep_id),
            score: 10,
            is_user_requested: false,
            is_season_pack: false,
            category: "",
            multi_targets: None,
            episode_intentions,
            source_pub_date: None,
            download_id: "dl-pipeline-test",
            scoring_size_bytes: None,
            scoring_seeders: None,
            scoring_episode_count: None,
            submitter,
            quality_profile_id,
            version: 1,
        })
        .await
        .unwrap();
    assert!(matches!(result, AddQueueResult::Added { .. }));
    db.get_queue_item_by_episode_id(ep_id)
        .await
        .unwrap()
        .expect("queued item must be readable back")
}

fn intentions_json(ep_id: &str) -> String {
    serde_json::to_string(&vec![jumbie_shared::types::EpisodeIntention {
        episode_num: 1,
        source_episode_num: 1,
        episode_id: ep_id.to_string(),
        score: 10,
        keep: true,
    }])
    .unwrap()
}

// Regression: `downloaded_at`/`next_retry_at` were naive `NaiveDateTime`, so
// the API emitted zone-less strings and clients had to guess the timezone.
#[tokio::test]
async fn test_queue_item_timestamps_serialize_with_utc_offset() {
    let (db, _tmp) = setup_db().await;
    let ep_id = "test_show_S01E01";
    let item = seed_queue_item(&db, ep_id, "[Cytox] Test Show S01E01", None, None, None).await;

    let json = serde_json::to_value(&item).unwrap();
    let downloaded_at = json["downloaded_at"]
        .as_str()
        .expect("downloaded_at must serialize as a string");
    assert!(
        downloaded_at.ends_with('Z') || downloaded_at.contains("+00:00"),
        "downloaded_at must carry an explicit UTC offset, got {downloaded_at:?}"
    );
    // NULL here — when present it is zoned too (see the serde adapter tests).
    assert!(json["next_retry_at"].is_null());
}

// Regression: the INSERT listed 8 columns but supplied 9 VALUES, so SQLite
// rejected it and (with the swallowed error) no row was ever created for
// episodes not already present in the DB.
#[tokio::test]
async fn test_set_episode_status_inserts_new_episode_row() {
    let (db, _tmp) = setup_db().await;

    db.set_episode_status("test_show_S01E01", "test-show", 1, 1, "Queued")
        .await
        .unwrap();

    let row = db
        .get_episode_by_id("test_show_S01E01")
        .await
        .unwrap()
        .expect("set_episode_status must create the episode row");
    assert_eq!(row.status.as_deref(), Some("Queued"));
    assert_eq!(row.series_id, "test-show");
}

// Regression: the value came from release_info via a fingerprint-path join,
// which misses whenever the download-path fingerprint no longer resolves —
// yielding NULL and clobbering the attribution.
#[tokio::test]
async fn test_get_organize_meta_reads_queue_item_submitter() {
    let (db, _tmp) = setup_db().await;
    let ep_id = "test_show_S01E01";

    seed_queue_item(&db, ep_id, "[Cytox] Test Show S01E01", Some("Cytox"), None, None).await;

    // Regression guard: the legacy resolution read submitter from release_info via a
    // fingerprint-path join, which missed (NULL) whenever the download fingerprint no
    // longer resolved — the queue item is authoritative instead.
    let meta = db
        .get_organize_meta(ep_id)
        .await
        .unwrap()
        .expect("organize meta must exist for a queued episode");
    assert_eq!(meta.submitter.as_deref(), Some("Cytox"));
    assert_eq!(meta.media_name, "[Cytox] Test Show S01E01");
}

#[tokio::test]
async fn test_get_organize_meta_reads_queue_item_quality_profile() {
    let (db, _tmp) = setup_db().await;
    let ep_id = "test_show_S01E01";

    seed_queue_item(&db, ep_id, "[Cytox] Test Show S01E01", None, Some("qp-1"), None).await;

    let meta = db
        .get_organize_meta(ep_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(meta.quality_profile_id.as_deref(), Some("qp-1"));
}

// Regression: ON CONFLICT hard-overwrote `submitter = excluded.submitter`, so
// a NULL snapshot (from the broken join above) wiped the value that
// finalize_download had written to release_info.
#[tokio::test]
async fn test_upsert_organize_release_meta_preserves_existing_submitter() {
    let (db, _tmp) = setup_db().await;

    db.set_release_info(
        "hash1",
        Some("[Cytox] Test Show S01E01"),
        Some("magnet:?xt=urn:btih:test"),
        Some("Cytox"),
    )
    .await
    .unwrap();

    // Organize snapshot with a NULL submitter (e.g. legacy item) must not
    // clobber the attribution already recorded on release_info.
    let meta = OrganizeMetaRow {
        media_name: "[Cytox] Test Show S01E01".to_string(),
        media_link: "magnet:?xt=urn:btih:test".to_string(),
        score: 10,
        source_pub_date: None,
        download_id: Some("dl-1".to_string()),
        submitter: None,
        quality_profile_id: None,
        scoring_size_bytes: None,
        scoring_seeders: None,
        scoring_episode_count: None,
        version: 1,
    };
    db.upsert_organize_release_meta("hash1", &meta, None)
        .await
        .unwrap();

    let submitter: Option<String> = sqlx::query_scalar(
        "SELECT submitter FROM release_info WHERE quick_hash = 'hash1'",
    )
    .fetch_one(db.get_pool())
    .await
    .unwrap();
    assert_eq!(submitter.as_deref(), Some("Cytox"));
}

// Regression: `release_info.upload_date` is the durable home of the Source Feed
// Date (keyed by content hash). A later organize snapshot that carries no source
// feed date must not erase the date already persisted there.
#[tokio::test]
async fn test_upsert_organize_release_meta_preserves_existing_upload_date() {
    let (db, _tmp) = setup_db().await;

    let organize_meta = || OrganizeMetaRow {
        media_name: "[Cytox] Test Show S01E01".to_string(),
        media_link: "magnet:?xt=urn:btih:test".to_string(),
        score: 10,
        source_pub_date: None,
        download_id: Some("dl-1".to_string()),
        submitter: None,
        quality_profile_id: None,
        scoring_size_bytes: None,
        scoring_seeders: None,
        scoring_episode_count: None,
        version: 1,
    };
    let feed = chrono::NaiveDate::from_ymd_opt(2026, 6, 18)
        .unwrap()
        .and_hms_opt(20, 0, 0)
        .unwrap();

    // First organize persists the feed date.
    db.upsert_organize_release_meta("hash1", &organize_meta(), Some(feed))
        .await
        .unwrap();
    // A later organize with no source feed date must not clear it.
    db.upsert_organize_release_meta("hash1", &organize_meta(), None)
        .await
        .unwrap();

    let stored: Option<String> =
        sqlx::query_scalar("SELECT upload_date FROM release_info WHERE quick_hash = 'hash1'")
            .fetch_one(db.get_pool())
            .await
            .unwrap();
    assert_eq!(
        stored.as_deref(),
        Some("2026-06-18 20:00:00"),
        "release_info.upload_date must persist across a NULL organize snapshot"
    );
}

// Regression: `upsert_organize_release_meta` must not erase `download_id` or the
// stored scoring inputs when a later organize snapshot lacks them (legacy/partial
// queue item) — release_info is the durable home for that data.
#[tokio::test]
async fn test_upsert_organize_release_meta_preserves_download_id_and_scoring() {
    let (db, _tmp) = setup_db().await;

    // Attribution + inputs finalize_download already recorded.
    sqlx::query(
        "INSERT INTO release_info (quick_hash, download_id, scoring_size_bytes, scoring_seeders, scoring_episode_count) \
         VALUES ('hash1', 'btih-tracker', 123, 45, 3)",
    )
    .execute(db.get_pool())
    .await
    .unwrap();

    // Organize snapshot carrying none of them.
    let meta = OrganizeMetaRow {
        media_name: "[Cytox] Test Show S01E01".to_string(),
        media_link: "magnet:?xt=urn:btih:test".to_string(),
        score: 10,
        source_pub_date: None,
        download_id: None,
        submitter: None,
        quality_profile_id: None,
        scoring_size_bytes: None,
        scoring_seeders: None,
        scoring_episode_count: None,
        version: 1,
    };
    db.upsert_organize_release_meta("hash1", &meta, None)
        .await
        .unwrap();

    let row: (Option<String>, Option<i64>, Option<i32>, Option<i32>) = sqlx::query_as(
        "SELECT download_id, scoring_size_bytes, scoring_seeders, scoring_episode_count \
         FROM release_info WHERE quick_hash = 'hash1'",
    )
    .fetch_one(db.get_pool())
    .await
    .unwrap();
    assert_eq!(row.0.as_deref(), Some("btih-tracker"));
    assert_eq!(row.1, Some(123));
    assert_eq!(row.2, Some(45));
    assert_eq!(row.3, Some(3));
}

// Regression: the file's date recorded in `release_info` must be the release's
// Source Feed Date captured at queue time — never the moment the download
// finished. The queue item keeps them separate (`source_pub_date` vs
// `downloaded_at`); organize reads the former.
#[tokio::test]
async fn test_organize_meta_uses_source_feed_date_not_download_time() {
    let (db, _tmp) = setup_db().await;
    seed_queue_item(&db, "ep-feed", "[Cytox] Test Show S01E01", None, None, None).await;

    let feed = "2026-06-18 20:00:00";
    let downloaded = "2026-07-01 09:30:00";
    sqlx::query(
        "UPDATE download_queue SET source_pub_date = ?, downloaded_at = ? WHERE episode_id = ?",
    )
    .bind(feed)
    .bind(downloaded)
    .bind("ep-feed")
    .execute(db.get_pool())
    .await
    .unwrap();

    let meta = db
        .get_organize_meta("ep-feed")
        .await
        .unwrap()
        .expect("queue item should exist");
    assert_eq!(
        meta.source_pub_date.as_deref(),
        Some(feed),
        "organize meta must carry the source feed date, not the download time"
    );

    // The exact path organize takes: parse the feed date, then persist it.
    let upload_date = meta
        .source_pub_date
        .as_deref()
        .and_then(|s| crate::datetime::parse_utc(s).ok().map(|u| u.naive_utc()));
    db.upsert_organize_release_meta("hash-feed", &meta, upload_date)
        .await
        .unwrap();

    let stored: Option<String> =
        sqlx::query_scalar("SELECT upload_date FROM release_info WHERE quick_hash = 'hash-feed'")
            .fetch_one(db.get_pool())
            .await
            .unwrap();
    assert_eq!(stored.as_deref(), Some(feed));
    assert_ne!(
        stored.as_deref(),
        Some(downloaded),
        "the download time must never become the file's date"
    );
}

// Regression: a multi-target copy episode's `meta_date` was set to the queue
// item's `downloaded_at` (the download time). It must instead inherit the target
// episode's metadata (air) date when metadata sync already knows it, and stay
// NULL otherwise — never the download time.
#[tokio::test]
async fn test_multi_target_copy_inherits_metadata_date_never_download_time() {
    use jumbie_shared::mapping::SeriesSettings;
    use jumbie_shared::types::MultiTarget;

    let (db, tmp) = setup_db().await;
    seed_queue_item(
        &db,
        "test_show_S01E01",
        "[Cytox] Test Show S01E01",
        None,
        None,
        None,
    )
    .await;

    // Two target series. A's episode row already carries a metadata date; B has no
    // episode row at all (metadata sync hasn't run).
    let target_a = "target-a";
    let target_b = "target-b";
    for (sid, name) in [(target_a, "Target A"), (target_b, "Target B")] {
        let root = tmp.path().join(sid);
        db.upsert_series_mapping(
            sid,
            &jumbie_shared::types::MappingRule {
                target_title: name.to_string(),
                name: name.to_string(),
                series_id: sid.to_string(),
                settings: SeriesSettings {
                    path: Some(root.to_string_lossy().to_string()),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .await
        .unwrap();
    }

    let mapping_a = db.get_series_mapping(target_a).await.unwrap().unwrap();
    let ep_a = mapping_a.get_episode_id("01", 1, false).unwrap();
    let mapping_b = db.get_series_mapping(target_b).await.unwrap().unwrap();
    let ep_b = mapping_b.get_episode_id("01", 1, false).unwrap();

    let air_date = chrono::NaiveDate::from_ymd_opt(2024, 3, 5)
        .unwrap()
        .and_hms_opt(0, 0, 0)
        .unwrap();
    db.insert_episode(InsertEpisodeParams {
        episode_id: &ep_a,
        series_id: target_a,
        season: 1,
        episode: 1,
        file_path: None,
        title: None,
        quality_profile_id: None,
        status: "missing",
        meta_date: Some(air_date),
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

    // Attach both targets to the queue item.
    let targets = serde_json::to_string(&vec![
        MultiTarget {
            series_id: target_a.to_string(),
            season: 1,
            episode: 1,
        },
        MultiTarget {
            series_id: target_b.to_string(),
            season: 1,
            episode: 1,
        },
    ])
    .unwrap();
    sqlx::query("UPDATE download_queue SET multi_targets = ? WHERE episode_id = ?")
        .bind(&targets)
        .bind("test_show_S01E01")
        .execute(db.get_pool())
        .await
        .unwrap();
    let item = db
        .get_queue_item_by_episode_id("test_show_S01E01")
        .await
        .unwrap()
        .unwrap();

    let source = tmp.path().join("src/Show.S01E01.mkv");
    tokio::fs::create_dir_all(source.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(&source, b"video").await.unwrap();

    let organizer = crate::tests::organizer_fixtures::make_test_organizer(db.clone()).await;
    organizer
        .distribute_multi_target_copies(&source, &item, "hash-mt")
        .await
        .unwrap();

    let a = db
        .get_episode_by_id(&ep_a)
        .await
        .unwrap()
        .expect("target A row must exist");
    assert_eq!(
        a.meta_date,
        Some(air_date),
        "copy must inherit the target's metadata date"
    );
    assert_ne!(
        a.meta_date,
        Some(item.downloaded_at),
        "copy must never use the download time"
    );

    let b = db
        .get_episode_by_id(&ep_b)
        .await
        .unwrap()
        .expect("target B row must exist");
    assert_eq!(
        b.meta_date, None,
        "no metadata for the target → NULL, never the download time"
    );
}

// Regression: the doc comment promised "First-write-wins via ON CONFLICT DO
// NOTHING" but the SQL overwrote with the newest (e.g. filename-derived) value.
#[tokio::test]
async fn test_set_release_info_preserves_existing_submitter_on_second_write() {
    let (db, _tmp) = setup_db().await;

    db.set_release_info(
        "hash1",
        Some("[Cytox] Test Show S01E01"),
        Some("magnet:?xt=urn:btih:test"),
        Some("Cytox"),
    )
    .await
    .unwrap();

    // A later write (e.g. the scanner re-deriving the group from a renamed
    // file) must not overwrite the download attribution.
    db.set_release_info(
        "hash1",
        Some("[Other] Test Show S01E01"),
        None,
        Some("OtherGroup"),
    )
    .await
    .unwrap();

    let submitter: Option<String> = sqlx::query_scalar(
        "SELECT submitter FROM release_info WHERE quick_hash = 'hash1'",
    )
    .fetch_one(db.get_pool())
    .await
    .unwrap();
    assert_eq!(submitter.as_deref(), Some("Cytox"));
}

// Regression: `get_episode_submitter` used to be a second hand-rolled
// resolution query that returned an empty-string submitter as-is (blocking
// the download_queue fallback).  It now delegates to the shared SUBMITTER_EXPR
// resolution, where NULLIF treats '' as absent and falls through to the queue.
#[tokio::test]
async fn test_get_episode_submitter_falls_back_when_release_info_submitter_empty() {
    let (db, _tmp) = setup_db().await;
    let ep_id = "test_show_S01E01";

    // Episode with a file whose release_info has an EMPTY-STRING submitter.
    let meta_ids = HashMap::new();
    db.insert_episode(InsertEpisodeParams {
        episode_id: ep_id,
        series_id: "test-show",
        season: 1,
        episode: 1,
        file_path: Some("/tmp/library/Test.Show.S01E01.mkv"),
        status: "organized",
        ..InsertEpisodeParams::dummy(ep_id, "test-show", 1, 1, &meta_ids)
    })
    .await
    .unwrap();
    db.save_fingerprint(crate::db::files::SaveFingerprintParams {
        path: "/tmp/library/Test.Show.S01E01.mkv",
        inode: 1,
        dev: 1,
        size: 100,
        mtime: 1.0,
        quick_hash: "hash1",
        state: "organized",
        media_info: None::<&jumbie_shared::types::MediaInfo>,
    })
    .await
    .unwrap();
    db.link_file_episode("/tmp/library/Test.Show.S01E01.mkv", ep_id)
        .await
        .unwrap();
    db.set_release_info("hash1", Some("Title"), None, Some(""))
        .await
        .unwrap();

    // Queue fallback carries the real attribution.
    seed_queue_item(&db, ep_id, "[QueueGroup] Test Show S01E01", Some("QueueGroup"), None, None)
        .await;

    let submitter = db.get_episode_submitter(ep_id).await.unwrap();
    assert_eq!(
        submitter.as_deref(),
        Some("QueueGroup"),
        "empty-string release_info submitter must fall through to the queue"
    );
}

// Regression: insert_episode was called with quality_profile_id: None (the
// value died in enrich_and_enqueue) and file_acquired_at was never stamped by
// the download pipeline.
#[tokio::test]
async fn test_smart_link_persists_quality_profile_and_file_acquired_at() {
    let (db, tmp) = setup_db().await;
    let ep_id = "test_show_S01E01";

    let file_path = tmp.path().join("Test.Show.S01E01.mkv");
    tokio::fs::write(&file_path, b"fake video content").await.unwrap();

    let meta_ids = HashMap::new();
    db.insert_episode(InsertEpisodeParams::dummy(ep_id, "test-show", 1, 1, &meta_ids))
        .await
        .unwrap();

    let item = seed_queue_item(
        &db,
        ep_id,
        "[Cytox] Test Show S01E01 1080p",
        Some("Cytox"),
        Some("qp-1"),
        Some(&intentions_json(ep_id)),
    )
    .await;

    let organizer = crate::tests::organizer_fixtures::make_test_organizer(db.clone()).await;
    organizer
        .smart_link_downloaded_files(file_path.to_str().unwrap(), &item)
        .await
        .unwrap();

    let row = db
        .get_episode_by_id(ep_id)
        .await
        .unwrap()
        .expect("episode must exist after smart-link");
    assert_eq!(row.status.as_deref(), Some("downloaded"));
    assert_eq!(row.quality_profile_id.as_deref(), Some("qp-1"));
    assert!(
        row.file_acquired_at.is_some(),
        "smart-link must stamp file_acquired_at when it assigns the file"
    );
}

// A language variant arriving via download attaches alongside the episode's primary
// (linked) instead of displacing it.
#[tokio::test]
async fn test_smart_link_attaches_language_variant_alongside_primary() {
    let (db, tmp) = setup_db().await;
    let ep_id = "test_show_S01E01";

    // The episode already holds its primary file, alive on disk.
    let primary = tmp.path().join("Test.Show.S01E01.mkv");
    tokio::fs::write(&primary, b"primary").await.unwrap();
    let meta_ids = HashMap::new();
    db.insert_episode(InsertEpisodeParams {
        file_path: Some(primary.to_str().unwrap()),
        status: "organized",
        ..InsertEpisodeParams::dummy(ep_id, "test-show", 1, 1, &meta_ids)
    })
    .await
    .unwrap();

    let variant = tmp.path().join("Test.Show.S01E01.en.mkv");
    tokio::fs::write(&variant, b"variant").await.unwrap();

    let item = seed_queue_item(
        &db,
        ep_id,
        "[Cytox] Test Show S01E01 1080p",
        Some("Cytox"),
        None,
        Some(&intentions_json(ep_id)),
    )
    .await;
    let organizer = crate::tests::organizer_fixtures::make_test_organizer(db.clone()).await;
    organizer
        .smart_link_downloaded_files(variant.to_str().unwrap(), &item)
        .await
        .unwrap();

    assert_eq!(
        db.get_episode_file_path(ep_id).await.unwrap().as_deref(),
        Some(primary.to_str().unwrap()),
        "the downloaded variant must not displace the primary"
    );
    assert_eq!(
        linked_paths(&db, ep_id).await,
        vec![variant.to_string_lossy().to_string()],
        "the language variant must be attached as a linked file"
    );
}

/// Paths attached to an episode as `linked` (non-primary) videos.
async fn linked_paths(db: &DbManager, episode_id: &str) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT fp.file_path FROM episode_files ef \
         JOIN file_paths fp ON fp.id = ef.file_path_id \
         WHERE ef.episode_id = ? AND ef.kind = 'linked'",
    )
    .bind(episode_id)
    .fetch_all(db.get_pool())
    .await
    .unwrap()
}

/// The path held in a multipart slot.
async fn part_path(db: &DbManager, episode_id: &str, part: i32) -> Option<String> {
    sqlx::query_scalar(
        "SELECT fp.file_path FROM episode_files ef \
         JOIN file_paths fp ON fp.id = ef.file_path_id \
         WHERE ef.episode_id = ? AND ef.kind = 'main' AND ef.part_number = ?",
    )
    .bind(episode_id)
    .bind(part)
    .fetch_optional(db.get_pool())
    .await
    .unwrap()
}

fn write_at(path: &std::path::Path, bytes: &[u8]) -> String {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
    path.to_string_lossy().to_string()
}

// --- Multi-episode files (one file covering several episodes) ---------------
//
// A multi-episode file cannot be split, so it is assigned to every episode it
// covers (replacing overlapping existing files). Intentions mark the needed
// episodes (keep=true) and pack overspill (keep=false).

/// Intentions for a multi-episode file: `(episode_id, local_ep, source_ep, keep)`.
fn multi_intentions_json(entries: &[(&str, i32, i32, bool)]) -> String {
    let v: Vec<jumbie_shared::types::EpisodeIntention> = entries
        .iter()
        .map(|(id, local, src, keep)| jumbie_shared::types::EpisodeIntention {
            episode_num: *local,
            source_episode_num: *src,
            episode_id: id.to_string(),
            score: 10,
            keep: *keep,
        })
        .collect();
    serde_json::to_string(&v).unwrap()
}

async fn insert_episode_row(db: &DbManager, ep_id: &str, season: i32, episode: i32) {
    db.insert_episode(InsertEpisodeParams::dummy(
        ep_id,
        "test-show",
        season,
        episode,
        &HashMap::new(),
    ))
    .await
    .unwrap();
}

/// Both covered episodes get the range file (intention matches needed episodes).
#[tokio::test]
async fn test_smart_link_multi_episode_file_assigns_every_covered_episode() {
    let (db, tmp) = setup_db().await;
    let ep1 = "test_show_S01E01";
    let ep2 = "test_show_S01E02";
    insert_episode_row(&db, ep2, 1, 2).await;

    let file = write_at(&tmp.path().join("dl/Test.Show.S01E01E02.mkv"), b"range");
    let intentions = multi_intentions_json(&[(ep1, 1, 1, true), (ep2, 2, 2, true)]);
    let item = seed_queue_item(
        &db,
        ep1,
        "[Cytox] Test Show S01E01E02 1080p",
        Some("Cytox"),
        None,
        Some(&intentions),
    )
    .await;

    let organizer = crate::tests::organizer_fixtures::make_test_organizer(db.clone()).await;
    organizer
        .smart_link_downloaded_files(&file, &item)
        .await
        .unwrap();

    assert_eq!(
        db.get_episode_file_path(ep1).await.unwrap().as_deref(),
        Some(file.as_str()),
        "E01 must receive the range file"
    );
    assert_eq!(
        db.get_episode_file_path(ep2).await.unwrap().as_deref(),
        Some(file.as_str()),
        "E02 must receive the range file"
    );
}

/// A covered episode that already holds a file is replaced (upgrade/conflict).
#[tokio::test]
async fn test_smart_link_multi_episode_file_replaces_overlapping_existing() {
    let (db, tmp) = setup_db().await;
    let ep1 = "test_show_S01E01";
    let ep2 = "test_show_S01E02";
    insert_episode_row(&db, ep2, 1, 2).await;

    // E01 already holds an individual file on disk.
    let old = write_at(&tmp.path().join("lib/Test.Show.S01E01.mkv"), b"old");

    let file = write_at(&tmp.path().join("dl/Test.Show.S01E01E02.mkv"), b"range");
    let intentions = multi_intentions_json(&[(ep1, 1, 1, true), (ep2, 2, 2, true)]);
    let item = seed_queue_item(
        &db,
        ep1,
        "[Cytox] Test Show S01E01E02 1080p",
        Some("Cytox"),
        None,
        Some(&intentions),
    )
    .await;
    db.associate_main_file(ep1, &old, None).await.unwrap();

    let organizer = crate::tests::organizer_fixtures::make_test_organizer(db.clone()).await;
    organizer
        .smart_link_downloaded_files(&file, &item)
        .await
        .unwrap();

    assert_eq!(
        db.get_episode_file_path(ep1).await.unwrap().as_deref(),
        Some(file.as_str()),
        "E01's existing file must be replaced by the range file"
    );
    assert_eq!(
        db.get_episode_file_path(ep2).await.unwrap().as_deref(),
        Some(file.as_str()),
        "E02 must receive the range file"
    );
}

/// When every covered episode is overspill (keep=false) the file is unneeded and
/// is not assigned to anything.
#[tokio::test]
async fn test_smart_link_multi_episode_file_all_overspill_is_unneeded() {
    let (db, tmp) = setup_db().await;
    let ep1 = "test_show_S01E01";
    let ep2 = "test_show_S01E02";
    insert_episode_row(&db, ep2, 1, 2).await;

    let file = write_at(&tmp.path().join("dl/Test.Show.S01E01E02.mkv"), b"range");
    let intentions = multi_intentions_json(&[(ep1, 1, 1, false), (ep2, 2, 2, false)]);
    let item = seed_queue_item(
        &db,
        ep1,
        "[Cytox] Test Show S01E01E02 1080p",
        Some("Cytox"),
        None,
        Some(&intentions),
    )
    .await;

    let organizer = crate::tests::organizer_fixtures::make_test_organizer(db.clone()).await;
    organizer
        .smart_link_downloaded_files(&file, &item)
        .await
        .unwrap();

    assert_eq!(
        db.get_episode_file_path(ep1).await.unwrap(),
        None,
        "all-overspill range file must not be assigned to E01"
    );
    assert_eq!(
        db.get_episode_file_path(ep2).await.unwrap(),
        None,
        "all-overspill range file must not be assigned to E02"
    );
    assert!(linked_paths(&db, ep1).await.is_empty());
    assert!(linked_paths(&db, ep2).await.is_empty());
}

/// A multi-episode file replaces a single-episode file even when that single sits in
/// a different language slot: they are different kinds, not language variants.
#[tokio::test]
async fn test_smart_link_multi_episode_replaces_single_in_another_language_slot() {
    let (db, tmp) = setup_db().await;
    let ep1 = "test_show_S01E01";
    let ep2 = "test_show_S01E02";
    insert_episode_row(&db, ep2, 1, 2).await;

    // E01 holds a tagged single (language slot "en").
    let single_en = write_at(&tmp.path().join("lib/Test.Show.S01E01.en.mkv"), b"single-en");

    let file = write_at(&tmp.path().join("dl/Test.Show.S01E01E02.mkv"), b"range");
    let intentions = multi_intentions_json(&[(ep1, 1, 1, true), (ep2, 2, 2, true)]);
    let item = seed_queue_item(
        &db,
        ep1,
        "[Cytox] Test Show S01E01E02 1080p",
        Some("Cytox"),
        None,
        Some(&intentions),
    )
    .await;
    db.associate_main_file(ep1, &single_en, None).await.unwrap();

    let organizer = crate::tests::organizer_fixtures::make_test_organizer(db.clone()).await;
    organizer
        .smart_link_downloaded_files(&file, &item)
        .await
        .unwrap();

    assert_eq!(
        db.get_episode_file_path(ep1).await.unwrap().as_deref(),
        Some(file.as_str()),
        "a different kind (multi vs single) replaces, regardless of language slot"
    );
    assert_eq!(
        db.get_episode_file_path(ep2).await.unwrap().as_deref(),
        Some(file.as_str())
    );
}

/// A multi-episode file coexists (as a linked variant) with a same-range
/// multi-episode file in a different language slot.
#[tokio::test]
async fn test_smart_link_multi_episode_coexists_with_same_range_variant() {
    let (db, tmp) = setup_db().await;
    let ep1 = "test_show_S01E01";
    let ep2 = "test_show_S01E02";
    insert_episode_row(&db, ep2, 1, 2).await;

    // Both episodes already hold the (untagged) range file as their main.
    let main = write_at(&tmp.path().join("lib/Test.Show.S01E01E02.mkv"), b"range");

    let variant = write_at(&tmp.path().join("dl/Test.Show.S01E01E02.en.mkv"), b"range-en");
    let intentions = multi_intentions_json(&[(ep1, 1, 1, true), (ep2, 2, 2, true)]);
    let item = seed_queue_item(
        &db,
        ep1,
        "[Cytox] Test Show S01E01E02 1080p",
        Some("Cytox"),
        None,
        Some(&intentions),
    )
    .await;
    db.associate_main_file(ep1, &main, None).await.unwrap();
    db.associate_main_file(ep2, &main, None).await.unwrap();

    let organizer = crate::tests::organizer_fixtures::make_test_organizer(db.clone()).await;
    organizer
        .smart_link_downloaded_files(&variant, &item)
        .await
        .unwrap();

    for ep in [ep1, ep2] {
        assert_eq!(
            db.get_episode_file_path(ep).await.unwrap().as_deref(),
            Some(main.as_str()),
            "the untagged range file must stay main on {ep}"
        );
        assert_eq!(
            linked_paths(&db, ep).await,
            vec![variant.clone()],
            "the same-range language variant must attach alongside on {ep}"
        );
    }
}

/// A multi-episode file with a different episode range is a different artifact, so
/// it replaces rather than attaching alongside.
#[tokio::test]
async fn test_smart_link_multi_episode_different_range_replaces() {
    let (db, tmp) = setup_db().await;
    let ep1 = "test_show_S01E01";
    let ep2 = "test_show_S01E02";
    insert_episode_row(&db, ep2, 1, 2).await;

    // E01 holds a wider range file (E01-E03) as its main.
    let wider = write_at(&tmp.path().join("lib/Test.Show.S01E01E02E03.mkv"), b"wide");

    let file = write_at(&tmp.path().join("dl/Test.Show.S01E01E02.mkv"), b"range");
    let intentions = multi_intentions_json(&[(ep1, 1, 1, true), (ep2, 2, 2, true)]);
    let item = seed_queue_item(
        &db,
        ep1,
        "[Cytox] Test Show S01E01E02 1080p",
        Some("Cytox"),
        None,
        Some(&intentions),
    )
    .await;
    db.associate_main_file(ep1, &wider, None).await.unwrap();

    let organizer = crate::tests::organizer_fixtures::make_test_organizer(db.clone()).await;
    organizer
        .smart_link_downloaded_files(&file, &item)
        .await
        .unwrap();

    assert_eq!(
        db.get_episode_file_path(ep1).await.unwrap().as_deref(),
        Some(file.as_str()),
        "a different episode range is a different artifact and must replace"
    );
    assert!(linked_paths(&db, ep1).await.is_empty());
}

// A duplicate language tag arriving via download is not attached: ingestion keeps the
// file already in the episode's language slot (one file per language tag).
#[tokio::test]
async fn test_smart_link_does_not_attach_a_duplicate_multipart_language_tag() {
    let (db, tmp) = setup_db().await;
    let ep_id = "test_show_S01E01";
    let meta_ids = HashMap::new();
    db.insert_episode(InsertEpisodeParams::dummy(ep_id, "test-show", 1, 1, &meta_ids))
        .await
        .unwrap();

    // Part 1 is already held, with its English variant attached.
    let pt1 = write_at(&tmp.path().join("lib/Test.Show.S01E01-pt1.mkv"), b"pt1");
    let pt1_en = write_at(&tmp.path().join("lib/Test.Show.S01E01-pt1.en.mkv"), b"pt1-en");
    db.upsert_episode_part(ep_id, 1, &pt1, None).await.unwrap();
    db.link_file_episode_as(&pt1_en, ep_id, jumbie_shared::media_format::FileKind::Video)
        .await
        .unwrap();

    // A second English part 1 is downloaded.
    let dup = write_at(&tmp.path().join("dl/Test.Show.S01E01-pt1.en.mkv"), b"pt1-en-dup");
    let item = seed_queue_item(
        &db,
        ep_id,
        "[Cytox] Test Show S01E01 1080p",
        Some("Cytox"),
        None,
        Some(&intentions_json(ep_id)),
    )
    .await;
    let organizer = crate::tests::organizer_fixtures::make_test_organizer(db.clone()).await;
    organizer
        .smart_link_downloaded_files(&dup, &item)
        .await
        .unwrap();

    assert_eq!(
        linked_paths(&db, ep_id).await,
        vec![pt1_en.clone()],
        "the duplicate language tag must not be attached"
    );
    assert_eq!(
        part_path(&db, ep_id, 1).await.as_deref(),
        Some(pt1.as_str()),
        "part 1 must be untouched"
    );
}

#[tokio::test]
async fn test_smart_link_does_not_attach_a_duplicate_range_language_tag() {
    let (db, tmp) = setup_db().await;
    let ep_id = "test_show_S01E01";
    let meta_ids = HashMap::new();
    db.insert_episode(InsertEpisodeParams::dummy(ep_id, "test-show", 1, 1, &meta_ids))
        .await
        .unwrap();

    // A range file covers the episode, with its English variant attached.
    let range = write_at(&tmp.path().join("lib/Test.Show.S01E01E02.mkv"), b"range");
    let range_en = write_at(
        &tmp.path().join("lib/Test.Show.S01E01E02.en.mkv"),
        b"range-en",
    );
    db.associate_main_file(ep_id, &range, None).await.unwrap();
    db.link_file_episode_as(&range_en, ep_id, jumbie_shared::media_format::FileKind::Video)
        .await
        .unwrap();

    // A second English range file is downloaded.
    let dup = write_at(&tmp.path().join("dl/Test.Show.S01E01E02.en.mkv"), b"range-en-dup");
    let item = seed_queue_item(
        &db,
        ep_id,
        "[Cytox] Test Show S01E01E02 1080p",
        Some("Cytox"),
        None,
        Some(&intentions_json(ep_id)),
    )
    .await;
    let organizer = crate::tests::organizer_fixtures::make_test_organizer(db.clone()).await;
    organizer
        .smart_link_downloaded_files(&dup, &item)
        .await
        .unwrap();

    assert_eq!(
        linked_paths(&db, ep_id).await,
        vec![range_en.clone()],
        "the duplicate language tag must not be attached"
    );
    assert_eq!(
        db.get_episode_file_path(ep_id).await.unwrap().as_deref(),
        Some(range.as_str()),
        "the range file must stay the primary"
    );
}

// Regression: `resolved_submitter = item.submitter` wrote NULL to release_info
// when the queue item carried no submitter (common for search-driven
// downloads), losing the release group even though it was parseable from the
// release title.
#[tokio::test]
async fn test_finalize_download_writes_submitter_from_title() {
    let (db, tmp) = setup_db().await;
    let ep_id = "test_show_S01E01";

    let file_path = tmp.path().join("Test.Show.S01E01.mkv");
    tokio::fs::write(&file_path, b"fake video content").await.unwrap();

    let meta_ids = HashMap::new();
    db.insert_episode(InsertEpisodeParams::dummy(ep_id, "test-show", 1, 1, &meta_ids))
        .await
        .unwrap();

    // Queue item WITHOUT a submitter — must be recovered from the title.
    let item = seed_queue_item(
        &db,
        ep_id,
        "[Cytox] Test Show S01E01 1080p",
        None,
        None,
        Some(&intentions_json(ep_id)),
    )
    .await;

    let organizer = crate::tests::organizer_fixtures::make_test_organizer(db.clone()).await;
    organizer
        .finalize_download(&item, file_path.to_str().unwrap())
        .await
        .unwrap();

    let submitter = db.get_episode_submitter(ep_id).await.unwrap();
    assert_eq!(submitter.as_deref(), Some("Cytox"));
}

#[tokio::test]
async fn test_update_file_path_sets_file_acquired_at() {
    let (db, _tmp) = setup_db().await;
    let ep_id = "test_show_S01E01";

    let meta_ids = HashMap::new();
    db.insert_episode(InsertEpisodeParams::dummy(ep_id, "test-show", 1, 1, &meta_ids))
        .await
        .unwrap();

    db.update_file_path(ep_id, "/tmp/library/Test.Show.S01E01.mkv")
        .await
        .unwrap();

    let row = db
        .get_episode_by_id(ep_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.status.as_deref(), Some("organized"));
    assert!(
        row.file_acquired_at.is_some(),
        "update_file_path must stamp file_acquired_at"
    );
}

#[tokio::test]
async fn test_insert_episode_sets_file_acquired_at_when_file_assigned() {
    let (db, _tmp) = setup_db().await;
    let ep_id = "test_show_S01E01";

    let meta_ids = HashMap::new();
    db.insert_episode(InsertEpisodeParams {
        episode_id: ep_id,
        series_id: "test-show",
        season: 1,
        episode: 1,
        file_path: Some("/tmp/library/Test.Show.S01E01.mkv"),
        status: "downloaded",
        ..InsertEpisodeParams::dummy(ep_id, "test-show", 1, 1, &meta_ids)
    })
    .await
    .unwrap();

    let row = db
        .get_episode_by_id(ep_id)
        .await
        .unwrap()
        .unwrap();
    assert!(
        row.file_acquired_at.is_some(),
        "insert_episode must stamp file_acquired_at when it assigns a file"
    );
}

#[tokio::test]
async fn test_insert_episode_does_not_stamp_file_acquired_at_without_file() {
    let (db, _tmp) = setup_db().await;
    let ep_id = "test_show_S01E01";

    let meta_ids = HashMap::new();
    db.insert_episode(InsertEpisodeParams::dummy(ep_id, "test-show", 1, 1, &meta_ids))
        .await
        .unwrap();

    let row = db
        .get_episode_by_id(ep_id)
        .await
        .unwrap()
        .unwrap();
    assert!(
        row.file_acquired_at.is_none(),
        "metadata-only episodes must not be stamped as file-acquired"
    );
}

// Regression: `EnrichAndEnqueueParams.quality_profile_id` was dead — never
// threaded onto the queue item — so the modal's quality profile could never
// be resolved.  This also exercises the (fixed) `set_episode_status` INSERT
// which pre-creates the episode row at queue time.
#[tokio::test]
async fn test_enrich_and_enqueue_carries_attribution_to_queue_item() {
    let (db, _tmp) = setup_db().await;
    let ep_id = "test_show_S01E01";

    // download_queue.series_id FK — seed the mapping first.
    db.upsert_series_mapping(
        "test-show",
        &jumbie_shared::types::MappingRule {
            target_title: "Test Show".to_string(),
            name: "test_show".to_string(),
            quality_profile: Some("qp-1".to_string()),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let result = crate::organizer::ContentOrganizer::enrich_and_enqueue(
            crate::download_orchestrator::download::EnrichAndEnqueueParams {
                db: &db,
                notifications: None,
                media_name: "[Cytox] Test Show S01E01 1080p",
                media_link: "magnet:?xt=urn:btih:enrich-test",
                source: "",
                series_title: "Test Show",
                series_id: "test-show",
                seasons: &[1],
                episodes: &[1],
                episode_id: Some(ep_id),
                score: 10,
                is_user_requested: false,
                is_manual: false,
                is_season_pack: false,
                category: "",
                multi_targets: None,
                episode_intentions: Some(&intentions_json(ep_id)),
                quality_profile_id: Some("qp-1"),
                meta_date: None,
                source_pub_date: None,
                metadata_ids: None,
                description: None,
                runtime: None,
                image_url: None,
                download_id: "dl-enrich-test",
                title_override: None,
                submitter: Some("Cytox"),
                version: Some(1),
                scoring_size_bytes: None,
                scoring_seeders: None,
                scoring_episode_count: None,
            },
        )
        .await
        .unwrap();
    assert!(matches!(result, AddQueueResult::Added { .. }));

    let item = db
        .get_queue_item_by_episode_id(ep_id)
        .await
        .unwrap()
        .expect("queue item must exist");
    assert_eq!(item.quality_profile_id.as_deref(), Some("qp-1"));
    assert_eq!(item.submitter.as_deref(), Some("Cytox"));
    assert_eq!(item.download_id.as_deref(), Some("dl-enrich-test"));

    // The queue-time pre-insert must have created the row (broken INSERT fix).
    let row = db
        .get_episode_by_id(ep_id)
        .await
        .unwrap()
        .expect("episode row must exist after enrich_and_enqueue");
    assert_eq!(row.status.as_deref(), Some("Queued"));
}

// `enrich_and_enqueue` is a pure pass-through for quality_profile_id — the
// caller decides: `download_winner` and `queue_search_result` (auto-search /
// search auto-queue) resolve the series mapping's profile and pass it;
// `add_download` (manual result pick) passes None.  Those caller-level
// behaviors are covered by the integration tests in `backend/tests/api_downloads.rs`
// (test_queue_search_result_stamps_series_quality_profile /
// test_manual_download_does_not_stamp_quality_profile).

}
