use crate::db::DbManager;
use jumbie_shared::scoring::ReleaseProfile;
use std::collections::HashMap;
use std::sync::Arc;
use tempfile::TempDir;

jumbie_shared::test_module! {

/// Helper: set up a DB with a series, a release profile, and some episodes
/// with stored release titles and scoring inputs.
async fn setup_rescore_test_db() -> (Arc<DbManager>, String, TempDir) {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("test.db").to_string_lossy().to_string();
    let db = Arc::new(DbManager::new(std::path::Path::new(&db_path)).await.unwrap());

    let mut terms = HashMap::new();
    terms.insert("1080p".to_string(), 10);
    terms.insert("720p".to_string(), 5);
    terms.insert("BluRay".to_string(), 15);
    terms.insert("hevc".to_string(), 5);

    let profile = ReleaseProfile {
        name: "Test HD".to_string(),
        terms: terms.clone(),
        min_score: 0,
        ..Default::default()
    };

    let mut profiles = HashMap::new();
    profiles.insert("test-hd-uuid".to_string(), profile);
    db.save_all_release_profiles(&profiles).await.unwrap();

    let series_id = "test-series-001".to_string();
    let mut mapping = jumbie_shared::types::MappingRule {
        target_title: "Test Show".to_string(),
        release_profile: Some("test-hd-uuid".to_string()),
        ..Default::default()
    };
    mapping.ensure_series_id();
    db.upsert_series_mapping(&series_id, &mapping).await.unwrap();

    let empty_map = HashMap::new();

    // Episode 1: 1080p BluRay release (score = 10 + 15 = 25)
    db.insert_episode(crate::db::episodes::InsertEpisodeParams {
        episode_id: "test-series-001_S01E01",
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
    }).await.unwrap();

    // Episode 2: 720p release (score = 5)
    db.insert_episode(crate::db::episodes::InsertEpisodeParams {
        episode_id: "test-series-001_S01E02",
        series_id: &series_id,
        season: 1,
        episode: 2,
        file_path: None,
        title: Some("Episode 2"),
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
    }).await.unwrap();

    // Episode 3: no release_title (should be skipped by rescore)
    db.insert_episode(crate::db::episodes::InsertEpisodeParams {
        episode_id: "test-series-001_S01E03",
        series_id: &series_id,
        season: 1,
        episode: 3,
        file_path: None,
        title: Some("Episode 3"),
        quality_profile_id: None,
        status: "missing",
        meta_date: None,
        est_date: None,
        metadata_ids: &empty_map,
        description: None,
        runtime: None,
        image_url: None,
        metadata_source: None,
        numbering_mode: None,
    }).await.unwrap();

    // The rescore pipeline reads rm.release_title, rm.submitter from
    // release_info via file_paths joined on e.file_path, so we
    // need real fingerprints + release_info entries for scored episodes.

    // Episode 1: 1080p BluRay → scores 25, submitter "Group"
    let ep1_path = temp_dir.path().join("ep1_test.mkv");
    tokio::fs::write(&ep1_path, b"ep1 content").await.unwrap();
    let meta1 = tokio::fs::metadata(&ep1_path).await.unwrap();
    let (inode1, dev1, mtime1) = crate::platform::file_identity(&meta1);
    let hash1 = "ep1_rescore_hash";
    db.save_fingerprint(crate::db::files::SaveFingerprintParams {
        path: ep1_path.to_str().unwrap(),
        inode: inode1,
        dev: dev1,
        size: meta1.len(),
        mtime: mtime1,
        quick_hash: hash1,
        state: "complete",
        media_info: None::<&jumbie_shared::types::MediaInfo>,
    }).await.unwrap();
    db.set_release_info(
        hash1,
        Some("[Group] Test Show S01E01 [1080p][BluRay]"),
        Some("magnet:?xt=urn:btih:ep1"),
        Some("Group"),
    ).await.unwrap();
    // Scoring inputs live on release_info.
    sqlx::query("UPDATE release_info SET scoring_size_bytes = ?, scoring_seeders = ? WHERE quick_hash = ?")
        .bind(2_147_483_648i64)
        .bind(50i32)
        .bind(hash1)
        .execute(db.get_pool())
        .await
        .unwrap();
    db.link_file_episode(ep1_path.to_str().unwrap(), "test-series-001_S01E01")
        .await
        .unwrap();

    // Episode 2: 720p only → scores 5, no submitter
    let ep2_path = temp_dir.path().join("ep2_test.mkv");
    tokio::fs::write(&ep2_path, b"ep2 content").await.unwrap();
    let meta2 = tokio::fs::metadata(&ep2_path).await.unwrap();
    let (inode2, dev2, mtime2) = crate::platform::file_identity(&meta2);
    let hash2 = "ep2_rescore_hash";
    db.save_fingerprint(crate::db::files::SaveFingerprintParams {
        path: ep2_path.to_str().unwrap(),
        inode: inode2,
        dev: dev2,
        size: meta2.len(),
        mtime: mtime2,
        quick_hash: hash2,
        state: "complete",
        media_info: None::<&jumbie_shared::types::MediaInfo>,
    }).await.unwrap();
    db.set_release_info(
        hash2,
        Some("[Other] Test Show S01E02 [720p]"),
        None,
        None,
    ).await.unwrap();
    db.link_file_episode(ep2_path.to_str().unwrap(), "test-series-001_S01E02")
        .await
        .unwrap();

    (db, series_id, temp_dir)
}

#[tokio::test]
async fn test_rescore_matches_direct_calculation() {
    // Core invariant: rescore with the same profile must produce the
    // same score as calling calculate() directly with the same inputs.
    // This is the fundamental correctness guarantee of the whole feature.
    let (db, series_id, _tmp) = setup_rescore_test_db().await;

    let profile = db
        .get_release_profile("test-hd-uuid")
        .await
        .unwrap()
        .expect("test-hd-uuid profile must exist");
    let mut merged = profile.clone();
    merged.compile();

    let (expected_score, _) = merged.calculate_with_submitter(
        "[Group] Test Show S01E01 [1080p][BluRay]",
        2_147_483_648, // 2 GB (matching scoring_size_bytes)
        50,            // seeders (matching scoring_seeders)
        Some(chrono::DateTime::<chrono::Utc>::from_naive_utc_and_offset(
            chrono::NaiveDateTime::parse_from_str("2026-06-15 12:00:00", "%Y-%m-%d %H:%M:%S").unwrap(),
            chrono::Utc,
        )),
        None, // episode_count
        Some("Group"),
    );

    // Profile terms: 1080p(10) + 720p(5) + BluRay(15) + hevc(5).
    // E01 title "[Group] Test Show S01E01 [1080p][BluRay]" → 1080p+BluRay = 25.
    assert_eq!(expected_score, 25, "Direct calculate() must produce 25");

    db.rescore_episodes_for_profiles(std::slice::from_ref(&series_id)).await.unwrap();

    let ep1 = db.get_episode_by_id("test-series-001_S01E01").await.unwrap().unwrap();
    assert_eq!(
        ep1.score,
        Some(expected_score),
        "Rescore must produce the same value as direct calculate()"
    );

    let (expected_e02, _) = merged.calculate_with_submitter(
        "[Other] Test Show S01E02 [720p]",
        0,    // no size stored
        0,    // no seeders stored
        None, // no meta_date stored
        None,
        None, // no submitter
    );
    assert_eq!(expected_e02, 5, "Direct calculate() for E02 must produce 5");

    let ep2 = db.get_episode_by_id("test-series-001_S01E02").await.unwrap().unwrap();
    assert_eq!(
        ep2.score,
        Some(expected_e02),
        "Rescore for E02 must match direct calculate()"
    );
}

#[tokio::test]
async fn test_rescore_preserves_original_score_when_profile_unchanged() {
    let (db, series_id, _tmp) = setup_rescore_test_db().await;

    db.rescore_episodes_for_profiles(std::slice::from_ref(&series_id)).await.unwrap();

    let ep1 = db.get_episode_by_id("test-series-001_S01E01").await.unwrap().unwrap();
    assert_eq!(ep1.score, Some(25), "1080p BluRay should still score 25");

    let ep2 = db.get_episode_by_id("test-series-001_S01E02").await.unwrap().unwrap();
    assert_eq!(ep2.score, Some(5), "720p should still score 5");

    let ep3 = db.get_episode_by_id("test-series-001_S01E03").await.unwrap().unwrap();
    assert_eq!(ep3.score, None, "Episode without release_title should remain unscored");
}

#[tokio::test]
async fn test_rescore_after_increasing_term_weight() {
    let (db, series_id, _tmp) = setup_rescore_test_db().await;

    let mut terms = HashMap::new();
    terms.insert("1080p".to_string(), 20); // was 10
    terms.insert("720p".to_string(), 5);
    terms.insert("BluRay".to_string(), 15);
    terms.insert("hevc".to_string(), 5);

    let profile = ReleaseProfile {
        name: "Test HD".to_string(),
        terms,
        min_score: 0,
        ..Default::default()
    };
    let mut profiles = HashMap::new();
    profiles.insert("test-hd-uuid".to_string(), profile);
    db.save_all_release_profiles(&profiles).await.unwrap();

    db.rescore_episodes_for_profiles(std::slice::from_ref(&series_id)).await.unwrap();

    // Episode 1: 1080p(20) + BluRay(15) = 35 (was 25)
    let ep1 = db.get_episode_by_id("test-series-001_S01E01").await.unwrap().unwrap();
    assert_eq!(ep1.score, Some(35), "1080p BluRay should now score 35");

    // Episode 2: 720p(5) = 5 (unchanged)
    let ep2 = db.get_episode_by_id("test-series-001_S01E02").await.unwrap().unwrap();
    assert_eq!(ep2.score, Some(5), "720p should still score 5");

    // Episode 3: no release_title → still None
    let ep3 = db.get_episode_by_id("test-series-001_S01E03").await.unwrap().unwrap();
    assert_eq!(ep3.score, None, "No release_title episode should still be None");
}

#[tokio::test]
async fn test_rescore_after_adding_new_term() {
    let (db, series_id, _tmp) = setup_rescore_test_db().await;

    let mut terms = HashMap::new();
    terms.insert("1080p".to_string(), 10);
    terms.insert("720p".to_string(), 5);
    terms.insert("BluRay".to_string(), 15);
    terms.insert("hevc".to_string(), 5);
    terms.insert("HDR".to_string(), 25); // new term

    let profile = ReleaseProfile {
        name: "Test HD".to_string(),
        terms,
        min_score: 0,
        ..Default::default()
    };
    let mut profiles = HashMap::new();
    profiles.insert("test-hd-uuid".to_string(), profile);
    db.save_all_release_profiles(&profiles).await.unwrap();

    db.rescore_episodes_for_profiles(std::slice::from_ref(&series_id)).await.unwrap();

    // Episode 1: 1080p(10) + BluRay(15) = 25 (HDR not in title)
    let ep1 = db.get_episode_by_id("test-series-001_S01E01").await.unwrap().unwrap();
    assert_eq!(ep1.score, Some(25), "No HDR in title → still 25");

    // Now update E01's release_title to include HDR and rescore
    db.set_release_info(
        "ep1_rescore_hash",
        Some("[Group] Test Show S01E01 [1080p][BluRay][HDR]"),
        Some("magnet:?xt=urn:btih:ep1"),
        Some("Group"),
    ).await.unwrap();

    db.rescore_episodes_for_profiles(&[series_id]).await.unwrap();

    let ep1 = db.get_episode_by_id("test-series-001_S01E01").await.unwrap().unwrap();
    assert_eq!(ep1.score, Some(50), "HDR(25) + 1080p(10) + BluRay(15) = 50");
}

#[tokio::test]
async fn test_rescore_idempotent() {
    let (db, series_id, _tmp) = setup_rescore_test_db().await;

    db.rescore_episodes_for_profiles(std::slice::from_ref(&series_id)).await.unwrap();
    let score_after_1 = db.get_episode_by_id("test-series-001_S01E01").await.unwrap().unwrap().score;

    db.rescore_episodes_for_profiles(std::slice::from_ref(&series_id)).await.unwrap();
    let score_after_2 = db.get_episode_by_id("test-series-001_S01E01").await.unwrap().unwrap().score;

    db.rescore_episodes_for_profiles(std::slice::from_ref(&series_id)).await.unwrap();
    let score_after_3 = db.get_episode_by_id("test-series-001_S01E01").await.unwrap().unwrap().score;

    assert_eq!(score_after_1, score_after_2, "First and second rescore must match");
    assert_eq!(score_after_2, score_after_3, "Second and third rescore must match");
}

#[tokio::test]
async fn test_rescore_empty_series_list_is_noop() {
    let (db, _, _tmp) = setup_rescore_test_db().await;

    db.rescore_episodes_for_profiles(&[]).await.unwrap();
}

#[tokio::test]
async fn test_rescore_nonexistent_profile_skips_silently() {
    let (db, series_id, _tmp) = setup_rescore_test_db().await;

    let mut mapping = db.get_series_mapping(&series_id).await.unwrap().unwrap();
    mapping.release_profile = Some("nonexistent-profile".to_string());
    db.upsert_series_mapping(&series_id, &mapping).await.unwrap();

    // Rescoring with a non-existent profile should not error.
    // The profile resolves to an empty default → scores become 0 (stored as None).
    db.rescore_episodes_for_profiles(std::slice::from_ref(&series_id)).await.unwrap();

    let ep1 = db.get_episode_by_id("test-series-001_S01E01").await.unwrap().unwrap();
    assert_eq!(ep1.score, None, "Non-existent profile → empty profile → score 0 → None");
}

#[tokio::test]
async fn test_rescore_with_submitter_scoring() {
    let (db, series_id, _tmp) = setup_rescore_test_db().await;

    let mut terms = HashMap::new();
    terms.insert("1080p".to_string(), 10);
    terms.insert("720p".to_string(), 5);
    terms.insert("BluRay".to_string(), 15);

    let mut submitters = HashMap::new();
    submitters.insert("Group".to_string(), 20); // trusted release group

    let profile = ReleaseProfile {
        name: "Test HD".to_string(),
        terms,
        submitters,
        min_score: 0,
        ..Default::default()
    };
    let mut profiles = HashMap::new();
    profiles.insert("test-hd-uuid".to_string(), profile);
    db.save_all_release_profiles(&profiles).await.unwrap();

    db.rescore_episodes_for_profiles(&[series_id]).await.unwrap();

    // Episode 1: submitter "Group" matches → 1080p(10) + BluRay(15) + Group(20) = 45
    let ep1 = db.get_episode_by_id("test-series-001_S01E01").await.unwrap().unwrap();
    assert_eq!(ep1.score, Some(45), "1080p BluRay + Group submitter = 45");
}

#[tokio::test]
async fn test_rescore_preserves_size_score() {
    let (db, series_id, _tmp) = setup_rescore_test_db().await;

    let mut terms = HashMap::new();
    terms.insert("1080p".to_string(), 10);
    terms.insert("BluRay".to_string(), 15);

    let profile = ReleaseProfile {
        name: "Size Scoring".to_string(),
        terms,
        size_score_per_gb: 10, // +10 per GB
        min_score: 0,
        ..Default::default()
    };
    let mut profiles = HashMap::new();
    let profile_uuid = "size-scoring-uuid".to_string();
    profiles.insert(profile_uuid.clone(), profile);
    db.save_all_release_profiles(&profiles).await.unwrap();

    let mut mapping = db.get_series_mapping(&series_id).await.unwrap().unwrap();
    mapping.release_profile = Some(profile_uuid);
    db.upsert_series_mapping(&series_id, &mapping).await.unwrap();

    // E01 has scoring_size_bytes = 2GB → size_score = 2 * 10 = 20
    // Plus title: 1080p(10) + BluRay(15) = 25 → total = 45
    db.rescore_episodes_for_profiles(&[series_id]).await.unwrap();

    let ep1 = db.get_episode_by_id("test-series-001_S01E01").await.unwrap().unwrap();
    assert_eq!(ep1.score, Some(45), "2GB * 10 + 25 title = 45");
}

#[tokio::test]
async fn test_rescore_preserves_peers_score() {
    let (db, series_id, _tmp) = setup_rescore_test_db().await;

    let mut terms = HashMap::new();
    terms.insert("1080p".to_string(), 10);
    terms.insert("BluRay".to_string(), 15);

    let profile = ReleaseProfile {
        name: "Peers Scoring".to_string(),
        terms,
        peers_score_per_peer: 2, // +2 per seeder
        min_score: 0,
        ..Default::default()
    };
    let mut profiles = HashMap::new();
    let profile_uuid = "peers-scoring-uuid".to_string();
    profiles.insert(profile_uuid.clone(), profile);
    db.save_all_release_profiles(&profiles).await.unwrap();

    let mut mapping = db.get_series_mapping(&series_id).await.unwrap().unwrap();
    mapping.release_profile = Some(profile_uuid);
    db.upsert_series_mapping(&series_id, &mapping).await.unwrap();

    // E01 has scoring_seeders = 50 → peers_score = 50 * 2 = 100
    // Plus title: 1080p(10) + BluRay(15) = 25 → total = 125
    db.rescore_episodes_for_profiles(&[series_id]).await.unwrap();

    let ep1 = db.get_episode_by_id("test-series-001_S01E01").await.unwrap().unwrap();
    assert_eq!(ep1.score, Some(125), "50 seeders * 2 + 25 title = 125");
}

#[tokio::test]
async fn test_rescore_with_partial_null_inputs() {
    let (db, series_id, _tmp) = setup_rescore_test_db().await;

    // E02 has scoring_size_bytes=NULL, scoring_seeders=NULL — only title scoring works
    let mut terms = HashMap::new();
    terms.insert("1080p".to_string(), 10);
    terms.insert("720p".to_string(), 5);

    let profile = ReleaseProfile {
        name: "Partial Scoring".to_string(),
        terms,
        size_score_per_gb: 100, // large size bonus — but NULL size = 0
        min_score: 0,
        ..Default::default()
    };
    let mut profiles = HashMap::new();
    let profile_uuid = "partial-scoring-uuid".to_string();
    profiles.insert(profile_uuid.clone(), profile);
    db.save_all_release_profiles(&profiles).await.unwrap();

    let mut mapping = db.get_series_mapping(&series_id).await.unwrap().unwrap();
    mapping.release_profile = Some(profile_uuid);
    db.upsert_series_mapping(&series_id, &mapping).await.unwrap();

    db.rescore_episodes_for_profiles(&[series_id]).await.unwrap();

    // E02: 720p(5) + size(NULL→0 bytes = 0GB * 100 = 0) = 5
    let ep2 = db.get_episode_by_id("test-series-001_S01E02").await.unwrap().unwrap();
    assert_eq!(ep2.score, Some(5), "NULL size → 0 size score, just title term");

    // E01: 1080p(10) + size(2GB * 100 = 200) = 210 (BluRay not in profile's terms)
        let ep1 = db.get_episode_by_id("test-series-001_S01E01").await.unwrap().unwrap();
        assert_eq!(ep1.score, Some(210), "Size present → size score computed");
    }

    // When a season pack covers multiple episodes, every episode shares the same
    // scoring inputs on release_info so rescore yields the correct per-episode score.
    // Without scoring_episode_count, size normalization fails and each episode looks
    // like a 40GB file.

    /// Helper: set up a DB simulating a 4-episode pack where only the primary
    /// episode (E01) has scoring inputs stored on release_info. E02-E04
    /// only have release_title + submitter — mimicking the current gap where
    /// set_release_metadata_by_path doesn't propagate scoring inputs.
    async fn setup_multi_episode_pack_db() -> (Arc<DbManager>, String, TempDir) {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("test.db").to_string_lossy().to_string();
        let db = Arc::new(DbManager::new(std::path::Path::new(&db_path)).await.unwrap());

        let mut terms = HashMap::new();
        terms.insert("1080p".to_string(), 10);
        terms.insert("BluRay".to_string(), 15);

        let profile = ReleaseProfile {
            name: "Pack Size Scoring".to_string(),
            terms,
            size_score_per_gb: 10, // +10 per GB, so episode_count normalization matters
            min_score: 0,
            ..Default::default()
        };
        let mut profiles = HashMap::new();
        profiles.insert("pack-scoring-uuid".to_string(), profile);
        db.save_all_release_profiles(&profiles).await.unwrap();

        let series_id = "pack-series-001".to_string();
        let mut mapping = jumbie_shared::types::MappingRule {
            target_title: "Pack Show".to_string(),
            release_profile: Some("pack-scoring-uuid".to_string()),
            ..Default::default()
        };
        mapping.ensure_series_id();
        db.upsert_series_mapping(&series_id, &mapping).await.unwrap();

        let empty_map = HashMap::new();
        let release_title = "[Group] Pack Show S01 Complete [1080p][BluRay]";
        let submitter = "Group";

        let episode_ids: Vec<String> = (1..=4)
            .map(|ep| format!("pack-series-001_S01E{:02}", ep))
            .collect();

        for (i, ep_id) in episode_ids.iter().enumerate() {
            let ep_num = (i + 1) as i32;
            db.insert_episode(crate::db::episodes::InsertEpisodeParams {
                episode_id: ep_id,
                series_id: &series_id,
                season: 1,
                episode: ep_num,
                file_path: None,
                title: Some(&format!("Episode {}", ep_num)),
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
            }).await.unwrap();

            let path = temp_dir.path().join(format!("ep{}.mkv", ep_num));
            tokio::fs::write(&path, format!("ep{} content", ep_num)).await.unwrap();
            let meta = tokio::fs::metadata(&path).await.unwrap();
            let (inode, dev, mtime) = crate::platform::file_identity(&meta);
            let hash = format!("pack_hash_ep{}", ep_num);

            db.save_fingerprint(crate::db::files::SaveFingerprintParams {
                path: path.to_str().unwrap(),
                inode,
                dev,
                size: meta.len(),
                mtime,
                quick_hash: &hash,
                state: "complete",
                media_info: None::<&jumbie_shared::types::MediaInfo>,
            }).await.unwrap();

            // Only release_title, download_link, submitter — NO scoring inputs.
            db.set_release_info(
                &hash,
                Some(release_title),
                Some("magnet:?xt=urn:btih:pack"),
                Some(submitter),
            ).await.unwrap();

            // Link fingerprint → episode (also makes it the main file).
            db.link_file_episode(path.to_str().unwrap(), ep_id).await.unwrap();
        }

        // Scoring inputs are propagated to ALL episodes in the pack, not
        // just the primary one.  This ensures rescore can reproduce the same
        // per-episode score for every episode in the pack.
        let pack_size: i64 = 42_949_672_960; // 40 GB (40 * 1024^3)
        let pack_seeders: i32 = 100;
        let pack_episode_count: i32 = 4;

        for ep_num in 1..=4 {
            sqlx::query(
                "UPDATE release_info SET \
                 scoring_size_bytes = ?, \
                 scoring_seeders = ?, \
                 scoring_episode_count = ? \
                 WHERE quick_hash = ?",
            )
            .bind(pack_size)
            .bind(pack_seeders)
            .bind(pack_episode_count)
            .bind(format!("pack_hash_ep{}", ep_num))
            .execute(db.get_pool())
            .await
            .unwrap();
        }

        (db, series_id, temp_dir)
    }

    #[tokio::test]
    async fn test_rescore_multi_episode_pack_all_episodes_get_same_score() {
        // All episodes covered by a season pack must get the same score after rescore —
        // they share one release's title, size, and seeders.
        let (db, series_id, _tmp) = setup_multi_episode_pack_db().await;

        let profile = db
            .get_release_profile("pack-scoring-uuid")
            .await
            .unwrap()
            .expect("pack-scoring-uuid profile must exist");
        let mut merged = profile.clone();
        merged.compile();

        let pack_size: u64 = 42_949_672_960; // 40 GB
        let episode_count: u32 = 4;
        let seeders: u32 = 100;

        let (expected_score, _) = merged.calculate_with_submitter(
            "[Group] Pack Show S01 Complete [1080p][BluRay]",
            pack_size,
            seeders,
            None, // no meta_date
            Some(episode_count), // 4 episodes → size normalized to 10GB/ep
            Some("Group"),
        );
        // 1080p(10) + BluRay(15) + size(40GB / 4 = 10GB * 10 = 100) = 125
        assert_eq!(expected_score, 125, "Direct calculate() must produce 125");

        db.rescore_episodes_for_profiles(std::slice::from_ref(&series_id)).await.unwrap();

        let ep1 = db.get_episode_by_id("pack-series-001_S01E01").await.unwrap().unwrap();
        assert_eq!(
            ep1.score,
            Some(expected_score),
            "Primary episode (E01): must match direct calculate()"
        );

        for ep_num in 2..=4 {
            let ep_id = format!("pack-series-001_S01E{:02}", ep_num);
            let ep = db.get_episode_by_id(&ep_id).await.unwrap().unwrap();
            assert_eq!(
                ep.score,
                Some(expected_score),
                "Episode E{:02}: score must be identical to E01 — all episodes share the same pack release",
                ep_num
            );
        }
    }

    #[tokio::test]
    async fn test_rescore_multi_episode_pack_size_normalized_by_episode_count() {
        // Core correctness: size_score_per_gb must be normalized by
        // scoring_episode_count.  A 40GB pack with 4 episodes should score
        // as if each episode is 10GB, not 40GB.
        //
        // This test sets scoring_episode_count on ALL episodes to validate
        // the normalization logic itself (not the propagation).
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("test.db").to_string_lossy().to_string();
        let db = Arc::new(DbManager::new(std::path::Path::new(&db_path)).await.unwrap());

        let mut terms = HashMap::new();
        terms.insert("1080p".to_string(), 10);
        terms.insert("BluRay".to_string(), 15);
        let profile = ReleaseProfile {
            name: "Size Norm Test".to_string(),
            terms,
            size_score_per_gb: 10,
            min_score: 0,
            ..Default::default()
        };
        let mut profiles = HashMap::new();
        profiles.insert("size-norm-uuid".to_string(), profile);
        db.save_all_release_profiles(&profiles).await.unwrap();

        let series_id = "size-norm-series".to_string();
        let mut mapping = jumbie_shared::types::MappingRule {
            target_title: "Norm Show".to_string(),
            release_profile: Some("size-norm-uuid".to_string()),
            ..Default::default()
        };
        mapping.ensure_series_id();
        db.upsert_series_mapping(&series_id, &mapping).await.unwrap();

        let empty_map = HashMap::new();
        let pack_size: i64 = 42_949_672_960;
        let episode_count: i32 = 4;
        let seeders: i32 = 100;
        let release_title = "[Group] Norm Show S01 Complete [1080p][BluRay]";

        for ep_num in 1..=4 {
            let ep_id = format!("size-norm-series_S01E{:02}", ep_num);
            db.insert_episode(crate::db::episodes::InsertEpisodeParams {
                episode_id: &ep_id,
                series_id: &series_id,
                season: 1,
                episode: ep_num,
                file_path: None,
                title: Some(&format!("Episode {}", ep_num)),
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
            }).await.unwrap();

            let path = temp_dir.path().join(format!("norm_ep{}.mkv", ep_num));
            tokio::fs::write(&path, format!("norm content {}", ep_num)).await.unwrap();
            let meta = tokio::fs::metadata(&path).await.unwrap();
            let (inode, dev, mtime) = crate::platform::file_identity(&meta);
            let hash = format!("norm_hash_ep{}", ep_num);

            db.save_fingerprint(crate::db::files::SaveFingerprintParams {
                path: path.to_str().unwrap(),
                inode,
                dev,
                size: meta.len(),
                mtime,
                quick_hash: &hash,
                state: "complete",
                media_info: None::<&jumbie_shared::types::MediaInfo>,
            }).await.unwrap();

            db.set_release_info(
                &hash,
                Some(release_title),
                Some("magnet:?xt=urn:btih:norm"),
                Some("Group"),
            ).await.unwrap();
            sqlx::query(
                "UPDATE release_info SET \
                 scoring_size_bytes = ?, scoring_seeders = ?, scoring_episode_count = ? \
                 WHERE quick_hash = ?",
            )
            .bind(pack_size)
            .bind(seeders)
            .bind(episode_count)
            .bind(&hash)
            .execute(db.get_pool())
            .await
            .unwrap();

            db.link_file_episode(path.to_str().unwrap(), &ep_id).await.unwrap();
        }

        db.rescore_episodes_for_profiles(&[series_id]).await.unwrap();

        // Expected: 1080p(10) + BluRay(15) + size(40GB/4=10GB * 10=100) = 125
        // NOT:      1080p(10) + BluRay(15) + size(40GB/1=40GB * 10=400) = 425
        for ep_num in 1..=4 {
            let ep_id = format!("size-norm-series_S01E{:02}", ep_num);
            let ep = db.get_episode_by_id(&ep_id).await.unwrap().unwrap();
            assert_eq!(
                ep.score,
                Some(125),
                "Episode E{:02}: size must be normalized by episode_count (10GB/ep, not 40GB/ep)",
                ep_num
            );
            // Defend against inflation: score must NOT include the unnormalized 400pt
            assert_ne!(
                ep.score,
                Some(425),
                "Episode E{:02}: MUST NOT use unnormalized 40GB size",
                ep_num
            );
        }
    }
}
