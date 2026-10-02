// Search-scoring integration test for the season-pack score modifier.
//
// `score_results` is the shared scoring path for manual and automatic episode
// searches; it derives `is_season_pack` once and must apply the configured flat
// modifier to packs only.

mod common;

use jumbie::api_routes::system::search_core::score_results;
use jumbie::models::media::MediaEntry;
use jumbie_shared::scoring::ReleaseProfile;

fn entry(title: &str) -> MediaEntry {
    MediaEntry {
        title: title.to_string(),
        source: "test".to_string(),
        ..Default::default()
    }
}

#[tokio::test]
async fn test_pack_score_modifier_applies_only_to_packs() {
    let (_app, state, _tmp) = common::setup_test_app().await;
    {
        let mut cfg = state.cfg.write().await;
        cfg.general.season_pack_score_modifier = Some(250);
    }

    // A season pack and a single episode, each parseable by `parse_filename`.
    let pack = entry("Test Show S01 COMPLETE [1080p]");
    let single = entry("Test Show S01E01 [1080p]");

    // Empty profile → base score 0, so any non-zero score is the modifier.
    let profile = ReleaseProfile::default();
    let results = score_results(vec![pack, single], Some(&profile), &state).await;

    let pack = results
        .iter()
        .find(|r| r.is_season_pack)
        .expect("season pack entry should be scored");
    let single = results
        .iter()
        .find(|r| !r.is_season_pack)
        .expect("single-episode entry should be scored");

    assert_eq!(
        single.score, 0,
        "single episodes must not receive the modifier"
    );
    assert_eq!(pack.score, 250, "the pack must receive the flat modifier");
}

#[tokio::test]
async fn test_negative_pack_score_modifier_lowers_pack_score() {
    let (_app, state, _tmp) = common::setup_test_app().await;
    {
        let mut cfg = state.cfg.write().await;
        cfg.general.season_pack_score_modifier = Some(-40);
    }

    let pack = entry("Test Show S01 COMPLETE [1080p]");
    let profile = ReleaseProfile::default();
    let results = score_results(vec![pack], Some(&profile), &state).await;

    assert_eq!(results.len(), 1);
    assert!(results[0].is_season_pack);
    assert_eq!(results[0].score, -40);
}
