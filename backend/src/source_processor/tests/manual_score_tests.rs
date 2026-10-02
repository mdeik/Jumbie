// Tests for `default_score_for_manual_files`: the assumed current score of an
// already-downloaded episode that has no release info recorded (e.g. a file
// placed by hand). It feeds the upgrade decision in `select_winners`.

use std::collections::HashMap;
use std::sync::Arc;

use crate::db::DbManager;
use crate::db::episodes::crud::InsertEpisodeParams;
use crate::models::media::ReleaseCandidate;
use jumbie_shared::config::GeneralConfig;
use jumbie_shared::mapping::{EpisodeInfo, MappingRule};
use jumbie_shared::types::EpisodeStatus;

const SERIES_ID: &str = "manual-score-show";

fn mapping() -> MappingRule {
    MappingRule {
        series_id: SERIES_ID.to_string(),
        name: "manual-score-show".to_string(),
        target_title: "Manual Score Show".to_string(),
        ..Default::default()
    }
}

fn candidate(mapping: &MappingRule, score: i32) -> ReleaseCandidate {
    ReleaseCandidate {
        title: "Manual Score Show S01E01 [1080p]".to_string(),
        download_url: None,
        episode_info: EpisodeInfo {
            raw_title: String::new(),
            series_key: "manual-score-show".to_string(),
            file_ext: "mkv".to_string(),
            submitter: None,
            resolution: None,
            version: 1,
            part_number: None,
            is_season_pack: false,
            is_complete_pack: false,
            seasons: vec![1],
            episodes: vec![1],
            has_decimal_episode: false,
        },
        mapping: Arc::new(mapping.clone()),
        meta_date: None,
        score,
        score_breakdown: vec![],
        description: None,
        file_list: vec![],
        guid: None,
        needed_episodes: vec![1],
        unneeded_count: 0,
        seeders: 0,
        leechers: 0,
        indexer: "test".to_string(),
        size_bytes: 0,
        download_id: None,
        multi_targets: vec![],
        submitter: None,
    }
}

/// A DB with one downloaded, monitored episode that has no release info, so
/// `get_episode_release_info` returns None and the manual score fallback applies.
async fn setup_downloaded_episode(
    manual_score: Option<i32>,
) -> (crate::organizer::ContentOrganizer, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test.db");
    let db = Arc::new(DbManager::new(&db_path).await.unwrap());

    let m = mapping();
    db.upsert_series_mapping(SERIES_ID, &m).await.unwrap();
    db.save_general_config(&GeneralConfig {
        default_score_for_manual_files: manual_score,
        ..Default::default()
    })
    .await
    .unwrap();

    let ep_id = m.get_episode_id("1", 1, false).unwrap();
    let meta_ids = HashMap::new();
    db.insert_episode(InsertEpisodeParams {
        status: EpisodeStatus::Downloaded.as_str(),
        ..InsertEpisodeParams::dummy(&ep_id, SERIES_ID, 1, 1, &meta_ids)
    })
    .await
    .unwrap();
    sqlx::query("UPDATE episodes SET monitored = 1 WHERE episode_id = ?")
        .bind(&ep_id)
        .execute(db.get_pool())
        .await
        .unwrap();

    let organizer = crate::tests::organizer_fixtures::make_test_organizer(db.clone()).await;
    (organizer, tmp)
}

#[tokio::test]
async fn test_unset_manual_score_assumes_zero_and_allows_upgrade() {
    let (organizer, _tmp) = setup_downloaded_episode(None).await;

    let winners = organizer
        .select_winners(vec![candidate(&mapping(), 50)])
        .await
        .unwrap();

    assert_eq!(
        winners.len(),
        1,
        "with no manual score, current score is 0, so a 50-score candidate upgrades"
    );
}

#[tokio::test]
async fn test_manual_score_raises_current_score_and_blocks_weaker_candidate() {
    let (organizer, _tmp) = setup_downloaded_episode(Some(100)).await;

    let winners = organizer
        .select_winners(vec![candidate(&mapping(), 50)])
        .await
        .unwrap();

    assert!(
        winners.is_empty(),
        "manual score 100 means a 50-score candidate is not an upgrade"
    );
}

#[tokio::test]
async fn test_manual_score_still_allows_better_candidate() {
    let (organizer, _tmp) = setup_downloaded_episode(Some(100)).await;

    let winners = organizer
        .select_winners(vec![candidate(&mapping(), 150)])
        .await
        .unwrap();

    assert_eq!(
        winners.len(),
        1,
        "a 150-score candidate beats the manual score of 100"
    );
}

/// Every auto-search selector (feed polling and `auto_search_missing`) converges
/// on `select_winners`, so the reject gate lives there. A rejection recorded
/// after a stalled download must drop the release no matter how well it scores.
#[tokio::test]
async fn test_select_winners_drops_a_rejected_release() {
    let (organizer, _tmp) = setup_downloaded_episode(None).await;

    organizer
        .db
        .reject_download(
            "magnet:?xt=urn:btih:rejected",
            Some("rejected-hash"),
            "No progress after 30 min",
            crate::db::autoresolve::RejectionTarget::default(),
            chrono::Utc::now().naive_utc() + chrono::Duration::hours(48),
        )
        .await
        .unwrap();

    let mut rejected_candidate = candidate(&mapping(), 500);
    rejected_candidate.download_url = Some("magnet:?xt=urn:btih:rejected".to_string());
    rejected_candidate.download_id = Some("rejected-hash".to_string());

    let winners = organizer
        .select_winners(vec![rejected_candidate])
        .await
        .unwrap();

    assert!(
        winners.is_empty(),
        "a rejected release must never be selected, however high it scores"
    );
}
