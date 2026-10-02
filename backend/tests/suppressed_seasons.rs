// Durable season suppression tests.
//
// A manually deleted season must not reappear on metadata resync and is hidden
// everywhere (season list); an explicit restore (or match) brings it back. A
// season override's cell count survives a metadata merge because it lives in the
// series config, not the episode rows.

mod common;

use axum::http::StatusCode;
use std::collections::HashSet;
use tower::ServiceExt;

fn meta(season: i32, episode: i32) -> jumbie::plugins::metadata::EpisodeMetadata {
    jumbie::plugins::metadata::EpisodeMetadata {
        unique_id: format!("{season}-{episode}"),
        season,
        episode,
        title: format!("S{season}E{episode}"),
        description: None,
        runtime: None,
        image_url: None,
        meta_date: None,
    }
}

async fn season_episode_count(state: &jumbie::api::AppState, series_id: &str, season: i32) -> i64 {
    sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM episodes WHERE series_id = ? AND season = ?")
        .bind(series_id)
        .bind(season)
        .fetch_one(state.db.get_pool())
        .await
        .unwrap()
}

#[tokio::test]
async fn deleted_season_is_suppressed_and_not_reinserted_by_merge() {
    let (app, state, _tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Suppress Season Test").await;

    // Seed an episode so season 2 exists in DB.
    sqlx::query(
        "INSERT INTO episodes (episode_id, series_id, season, episode, numbering_mode, status) \
         VALUES (?, ?, 2, 1, 0, 'unreleased')",
    )
    .bind(format!("{series_id}_S02E01"))
    .bind(&series_id)
    .execute(state.db.get_pool())
    .await
    .unwrap();

    let res = app
        .clone()
        .oneshot(common::delete_request(&format!(
            "/api/series/{series_id}/season/2"
        )))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);

    assert_eq!(
        state
            .db
            .get_suppressed_seasons(&series_id, 0)
            .await
            .unwrap(),
        vec![2],
        "delete must record a durable suppression"
    );
    assert_eq!(season_episode_count(&state, &series_id, 2).await, 0);

    // Resync (merge) must NOT re-create the suppressed season, but must still
    // create non-suppressed seasons.
    let episodes = vec![
        (format!("{series_id}_S02E01"), meta(2, 1)),
        (format!("{series_id}_S01E01"), meta(1, 1)),
    ];
    state
        .db
        .merge_metadata_episodes("prov", &series_id, episodes, 0)
        .await
        .unwrap();

    assert_eq!(
        season_episode_count(&state, &series_id, 2).await,
        0,
        "suppressed season must not be re-inserted by a metadata sync"
    );
    assert_eq!(
        season_episode_count(&state, &series_id, 1).await,
        1,
        "non-suppressed seasons still sync"
    );
}

#[tokio::test]
async fn unsuppressing_a_season_allows_it_back_on_next_merge() {
    let (app, state, _tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Restore Season Test").await;

    state.db.suppress_season(&series_id, 3, 0).await.unwrap();

    // Explicit restore (what `restore_season_metadata` / `match_*` do).
    state.db.unsuppress_season(&series_id, 3, 0).await.unwrap();
    assert!(
        state
            .db
            .get_suppressed_seasons(&series_id, 0)
            .await
            .unwrap()
            .is_empty()
    );

    let episodes = vec![(format!("{series_id}_S03E01"), meta(3, 1))];
    state
        .db
        .merge_metadata_episodes("prov", &series_id, episodes, 0)
        .await
        .unwrap();
    assert_eq!(
        season_episode_count(&state, &series_id, 3).await,
        1,
        "restored season is re-adopted on the next sync"
    );
}

#[tokio::test]
async fn build_season_list_excludes_suppressed_seasons_including_overrides() {
    use jumbie_shared::types::SeasonOverride;

    let db_seasons = vec!["01".to_string(), "02".to_string(), "03".to_string()];
    let overrides = [SeasonOverride {
        season: "02".to_string(),
        episode_start: None,
        episode_end: None,
        cell_count: Some(12),
        episode_offset: None,
        alias_season_number: None,
        search_format: None,
        aliases: Vec::new(),
        reg_patterns: Vec::new(),
    }];
    let suppressed: HashSet<i32> = [2].into_iter().collect();

    let out =
        jumbie::api_routes::series::build_season_list(db_seasons, overrides.iter(), &suppressed);
    assert_eq!(
        out,
        vec!["01".to_string(), "03".to_string()],
        "suppressed season is hidden even when it has an override/cell_count"
    );
}

#[tokio::test]
async fn season_override_cell_count_survives_metadata_merge() {
    let (app, state, _tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Cell Count Persist Test").await;

    // Set a cell_count override on season 2.
    let mut mapping = state
        .db
        .get_series_mapping(&series_id)
        .await
        .unwrap()
        .expect("mapping");
    mapping.settings.season.insert(
        "02".to_string(),
        jumbie_shared::types::SeasonOverride {
            season: "02".to_string(),
            episode_start: None,
            episode_end: None,
            cell_count: Some(12),
            episode_offset: None,
            alias_season_number: None,
            search_format: None,
            aliases: Vec::new(),
            reg_patterns: Vec::new(),
        },
    );
    state
        .db
        .upsert_series_mapping(&series_id, &mapping)
        .await
        .unwrap();

    // A metadata merge (what a refresh does) must not touch the override.
    let episodes = vec![(format!("{series_id}_S02E01"), meta(2, 1))];
    state
        .db
        .merge_metadata_episodes("prov", &series_id, episodes, 0)
        .await
        .unwrap();

    let after = state
        .db
        .get_series_mapping(&series_id)
        .await
        .unwrap()
        .expect("mapping");
    assert_eq!(
        after.settings.season.get("02").and_then(|o| o.cell_count),
        Some(12),
        "a season override's cell_count must survive a metadata refresh"
    );
}

#[tokio::test]
async fn suppression_is_disabled_in_absolute_mode() {
    let (app, state, _tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Abs Suppression Disabled Test").await;

    // Tombstones stored for both modes (the absolute one models the old bug: a
    // delete that tombstoned the series' only, canonical season).
    state.db.suppress_season(&series_id, 1, 1).await.unwrap();
    state.db.suppress_season(&series_id, 2, 0).await.unwrap();

    assert!(
        state
            .db
            .get_suppressed_seasons(&series_id, 1)
            .await
            .unwrap()
            .is_empty(),
        "absolute mode has one canonical season and must never report it suppressed"
    );
    assert_eq!(
        state
            .db
            .get_suppressed_seasons(&series_id, 0)
            .await
            .unwrap(),
        vec![2],
        "normal-mode suppression is unaffected"
    );
}

#[tokio::test]
async fn deleting_absolute_season_is_a_reset_not_a_tombstone() {
    let (app, state, _tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Abs Delete Reset Test").await;

    // Switch the series to absolute numbering.
    let mut mapping = state
        .db
        .get_series_mapping(&series_id)
        .await
        .unwrap()
        .expect("mapping");
    mapping.settings.absolute_numbering = Some(true);
    state
        .db
        .upsert_series_mapping(&series_id, &mapping)
        .await
        .unwrap();

    // An absolute episode (absolute space is canonically season 1, mode 1).
    sqlx::query(
        "INSERT INTO episodes (episode_id, series_id, season, episode, numbering_mode, status) \
         VALUES (?, ?, 1, 1, 1, 'unreleased')",
    )
    .bind(format!("{series_id}_ABS0001"))
    .bind(&series_id)
    .execute(state.db.get_pool())
    .await
    .unwrap();

    // Delete the only season.
    let res = app
        .clone()
        .oneshot(common::delete_request(&format!(
            "/api/series/{series_id}/season/1"
        )))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);

    assert_eq!(
        season_episode_count(&state, &series_id, 1).await,
        0,
        "deleting the absolute season still clears its episode data"
    );
    assert!(
        state
            .db
            .get_suppressed_seasons(&series_id, 1)
            .await
            .unwrap()
            .is_empty(),
        "deleting the only season of an absolute series must not tombstone it"
    );

    // Because the season was not tombstoned, a later metadata sync re-adopts it.
    let episodes = vec![(format!("{series_id}_ABS0001"), meta(1, 1))];
    state
        .db
        .merge_metadata_episodes("prov", &series_id, episodes, 1)
        .await
        .unwrap();
    assert_eq!(
        season_episode_count(&state, &series_id, 1).await,
        1,
        "an absolute series must honour metadata changes after a season delete"
    );
}

#[tokio::test]
async fn legacy_absolute_tombstone_is_ignored_by_metadata_sync() {
    let (app, state, _tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Abs Legacy Tombstone Test").await;

    // A tombstone written for the canonical absolute season by the old, buggy
    // delete path.
    state.db.suppress_season(&series_id, 1, 1).await.unwrap();

    let episodes = vec![(format!("{series_id}_ABS0001"), meta(1, 1))];
    state
        .db
        .merge_metadata_episodes("prov", &series_id, episodes, 1)
        .await
        .unwrap();
    assert_eq!(
        season_episode_count(&state, &series_id, 1).await,
        1,
        "a stale absolute-mode tombstone must not block metadata sync forever"
    );
}

// Filesystem rescan must respect suppression: deleted seasons' files stay on
// disk, so a rescan would otherwise recreate their episode rows. Both
// `import_scan_for_series` and `scan_directory` consult `suppressed_seasons`.

fn scan_fixture_dir() -> tempfile::TempDir {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("Show.S01E01.1080p.mkv"), b"").unwrap();
    std::fs::write(dir.path().join("Show.S02E01.1080p.mkv"), b"").unwrap();
    dir
}

#[tokio::test]
async fn filesystem_rescan_does_not_resurrect_suppressed_season() {
    let (app, state, _tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Rescan Suppress Test").await;
    let dir = scan_fixture_dir();

    // What a season delete does: suppress season 2.
    state.db.suppress_season(&series_id, 2, 0).await.unwrap();

    let mapping = state
        .db
        .get_series_mapping(&series_id)
        .await
        .unwrap()
        .expect("mapping");
    let path = dir.path().to_path_buf();
    let (inserted, _conflicts) = jumbie::scanner::import_scan_for_series(&path, &mapping, &state)
        .await
        .expect("scan should succeed");

    assert_eq!(
        season_episode_count(&state, &series_id, 2).await,
        0,
        "a rescan must not recreate a suppressed season"
    );
    assert_eq!(
        season_episode_count(&state, &series_id, 1).await,
        1,
        "non-suppressed seasons are still scanned"
    );
    assert_eq!(inserted, 1, "only the non-suppressed episode is inserted");
}

#[tokio::test]
async fn filesystem_rescan_adopts_season_after_restore() {
    let (app, state, _tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Rescan Restore Test").await;
    let dir = scan_fixture_dir();

    // Suppress, then explicitly restore (what Match Seasons / restore does).
    state.db.suppress_season(&series_id, 2, 0).await.unwrap();
    state.db.unsuppress_season(&series_id, 2, 0).await.unwrap();

    let mapping = state
        .db
        .get_series_mapping(&series_id)
        .await
        .unwrap()
        .expect("mapping");
    let path = dir.path().to_path_buf();
    jumbie::scanner::import_scan_for_series(&path, &mapping, &state)
        .await
        .expect("scan should succeed");

    assert_eq!(
        season_episode_count(&state, &series_id, 2).await,
        1,
        "a restored season is scanned again"
    );
}
