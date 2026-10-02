//! Episode ↔ file ownership: reassignment, unassign, and delete must apply the
//! same "disowned" state, and must cover both multi-episode files (one path on
//! several episode rows) and multipart episodes (`episode_files` parts).
//!
//! These lock in the SSoT behavior so the delete / unassign / clear paths cannot
//! drift apart again. The file's `upload_date` is content-scoped (`release_info`),
//! so it is not an episode column and must survive these operations.

mod common;

use common::TestApp;

use jumbie::db::episodes::InsertEpisodeParams;
use jumbie_shared::types::{AssignFilePayload, BatchDeletePayload};
use std::collections::HashMap;
use tower::ServiceExt;

struct EpRow {
    file_path: Option<String>,
    status: Option<String>,
    meta_date: Option<String>,
    file_acquired_at: Option<String>,
}

fn episode_meta_date() -> chrono::NaiveDateTime {
    chrono::NaiveDate::from_ymd_opt(2024, 1, 1)
        .unwrap()
        .and_hms_opt(0, 0, 0)
        .unwrap()
}

fn feed_date() -> chrono::NaiveDateTime {
    chrono::NaiveDate::from_ymd_opt(2026, 6, 18)
        .unwrap()
        .and_hms_opt(20, 0, 0)
        .unwrap()
}

async fn add_episode(
    db: &jumbie::db::DbManager,
    episode_id: &str,
    series_id: &str,
    season: i32,
    episode: i32,
    file_path: Option<&str>,
) {
    db.insert_episode(InsertEpisodeParams {
        episode_id,
        series_id,
        season,
        episode,
        file_path,
        title: None,
        quality_profile_id: None,
        status: "organized",
        // Episode-level date; must be preserved by disown (it is not file-scoped).
        meta_date: Some(episode_meta_date()),
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

async fn ep_row(db: &jumbie::db::DbManager, episode_id: &str) -> EpRow {
    use sqlx::Row;
    let row = sqlx::query(
        "SELECT status, meta_date, file_acquired_at FROM episodes WHERE episode_id = ?",
    )
    .bind(episode_id)
    .fetch_one(db.get_pool())
    .await
    .unwrap();
    EpRow {
        // Ownership lives in `episode_files`; the primary main path is `None` once
        // disowned.
        file_path: db.get_episode_file_path(episode_id).await.unwrap(),
        status: row.try_get("status").unwrap(),
        meta_date: row.try_get("meta_date").unwrap(),
        file_acquired_at: row.try_get("file_acquired_at").unwrap(),
    }
}

async fn release_info_date(db: &jumbie::db::DbManager, hash: &str) -> Option<String> {
    sqlx::query_scalar("SELECT upload_date FROM release_info WHERE quick_hash = ?")
        .bind(hash)
        .fetch_optional(db.get_pool())
        .await
        .unwrap()
        .flatten()
}

async fn part_count(db: &jumbie::db::DbManager, episode_id: &str) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM episode_files WHERE episode_id = ? AND kind = 'main'")
        .bind(episode_id)
        .fetch_one(db.get_pool())
        .await
        .unwrap()
}

/// Path of the main file occupying `part_number` for `episode_id`, if any.
async fn part_path(
    db: &jumbie::db::DbManager,
    episode_id: &str,
    part_number: i64,
) -> Option<String> {
    sqlx::query_scalar(
        "SELECT fp.file_path FROM episode_files ef \
         JOIN file_paths fp ON fp.id = ef.file_path_id \
         WHERE ef.episode_id = ? AND ef.kind = 'main' AND ef.part_number = ?",
    )
    .bind(episode_id)
    .bind(part_number)
    .fetch_optional(db.get_pool())
    .await
    .unwrap()
}

/// Paths attached to an episode as `linked` (non-primary) videos.
async fn linked_paths(db: &jumbie::db::DbManager, episode_id: &str) -> Vec<String> {
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

/// POST a single manual assignment to S{season}E{episode}; returns the status.
async fn assign(
    app: &axum::Router,
    series_id: &str,
    path: &std::path::Path,
    episode: &str,
) -> axum::http::StatusCode {
    app.clone()
        .oneshot(common::post_json_request(
            &format!("/api/series/{series_id}/files/assign"),
            &AssignFilePayload {
                path: path.to_string_lossy().to_string(),
                season: "1".to_string(),
                episode: episode.to_string(),
            },
        ))
        .await
        .unwrap()
        .status()
}

/// The episode id of the first episode with the given number, from the details API.
async fn episode_id_for(app: &axum::Router, series_id: &str, episode: i32) -> String {
    let details: jumbie_shared::types::SeriesDetails =
        app.get_json(&format!("/api/series/{series_id}")).await;
    details
        .episodes
        .iter()
        .find(|e| e.episode == episode)
        .expect("episode present")
        .unique_id
        .clone()
}

fn write_file(tmp: &std::path::Path, name: &str) -> std::path::PathBuf {
    let p = tmp.join(name);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, b"video").unwrap();
    p
}

// ── Multi-episode files (one path shared by several episode rows) ────────────

#[tokio::test]
async fn unassign_multi_episode_clears_every_covering_episode() {
    let (app, state, tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Multi Ep Unassign").await;
    let file = write_file(tmp.path(), "lib/Multi Ep Unassign - S01E01E02.mkv");
    let path = file.to_string_lossy().to_string();

    let ep1 = format!("{series_id}_1_1");
    let ep2 = format!("{series_id}_1_2");
    for (id, n) in [(&ep1, 1), (&ep2, 2)] {
        add_episode(&state.db, id, &series_id, 1, n, Some(&path)).await;
    }

    let res = app
        .clone()
        .oneshot(common::post_json_request(
            &format!("/api/series/{series_id}/files/unassign"),
            &BatchDeletePayload {
                paths: vec![path.clone()],
            },
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), axum::http::StatusCode::OK);

    for id in [&ep1, &ep2] {
        let row = ep_row(&state.db, id).await;
        assert_eq!(row.file_path, None, "{id}: file_path must be cleared");
        assert_eq!(
            row.status.as_deref(),
            Some("missing"),
            "{id}: status must be 'missing'"
        );
        assert_eq!(
            row.file_acquired_at, None,
            "{id}: file_acquired_at must be cleared"
        );
        // Episode-level date is not file-scoped, so it is preserved.
        assert_eq!(
            row.meta_date.as_deref(),
            Some("2024-01-01 00:00:00"),
            "{id}: meta_date must be preserved"
        );
    }
}

#[tokio::test]
async fn delete_multi_episode_clears_every_covering_episode() {
    let (app, state, tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Multi Ep Delete").await;
    let file = write_file(tmp.path(), "lib/Multi Ep Delete - S01E01E02.mkv");
    let path = file.to_string_lossy().to_string();

    let ep1 = format!("{series_id}_1_1");
    let ep2 = format!("{series_id}_1_2");
    for (id, n) in [(&ep1, 1), (&ep2, 2)] {
        add_episode(&state.db, id, &series_id, 1, n, Some(&path)).await;
    }

    app.delete_json::<_, serde_json::Value>(
        &format!("/api/series/{series_id}/files"),
        &BatchDeletePayload {
            paths: vec![path.clone()],
        },
    )
    .await;

    for id in [&ep1, &ep2] {
        let row = ep_row(&state.db, id).await;
        assert_eq!(row.file_path, None, "{id}: file_path must be cleared");
        assert_eq!(row.status.as_deref(), Some("missing"), "{id}: status");
        assert_eq!(
            row.file_acquired_at, None,
            "{id}: file_acquired_at must be cleared"
        );
    }
}

#[tokio::test]
async fn unassign_preserves_the_file_date_in_release_info() {
    // The file's date is content-scoped. Unassigning the episode must NOT remove
    // the release's source feed date — release_info is the durable home.
    let (app, state, tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "File Date Kept").await;
    let file = write_file(tmp.path(), "lib/File Date Kept - S01E01.mkv");
    let path = file.to_string_lossy().to_string();

    let _ = state.db.record_hash_only(&file, "organized").await;
    let hash: String = sqlx::query_scalar("SELECT fingerprint FROM file_paths WHERE file_path = ?")
        .bind(&path)
        .fetch_one(state.db.get_pool())
        .await
        .unwrap();
    state
        .db
        .set_release_upload_date_fallback_by_path(&path, Some(feed_date()))
        .await
        .unwrap();

    let ep = format!("{series_id}_1_1");
    add_episode(&state.db, &ep, &series_id, 1, 1, Some(&path)).await;

    app.clone()
        .oneshot(common::post_json_request(
            &format!("/api/series/{series_id}/files/unassign"),
            &BatchDeletePayload {
                paths: vec![path.clone()],
            },
        ))
        .await
        .unwrap();

    assert_eq!(
        release_info_date(&state.db, &hash).await.as_deref(),
        Some("2026-06-18 20:00:00"),
        "unassign must preserve the file's date in release_info"
    );
}

// ── Multipart episodes (episode_files main parts) ────────────────────────────

#[tokio::test]
async fn unassign_one_part_keeps_the_other() {
    let (app, state, tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Multipart One").await;
    let ep = format!("{series_id}_1_1");
    add_episode(&state.db, &ep, &series_id, 1, 1, None).await;

    let p1 = write_file(tmp.path(), "dl/part1.mkv");
    let p2 = write_file(tmp.path(), "dl/part2.mkv");
    let p1s = p1.to_string_lossy().to_string();
    let p2s = p2.to_string_lossy().to_string();
    state
        .db
        .upsert_episode_part(&ep, 1, &p1s, None)
        .await
        .unwrap();
    state
        .db
        .upsert_episode_part(&ep, 2, &p2s, None)
        .await
        .unwrap();
    assert_eq!(part_count(&state.db, &ep).await, 2);

    let res = app
        .clone()
        .oneshot(common::post_json_request(
            &format!("/api/series/{series_id}/files/unassign"),
            &BatchDeletePayload {
                paths: vec![p1s.clone()],
            },
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), axum::http::StatusCode::OK);

    assert_eq!(
        part_count(&state.db, &ep).await,
        1,
        "only part 1 should be removed"
    );
    let remaining: Option<i64> = sqlx::query_scalar(
        "SELECT ef.part_number FROM episode_files ef \
         WHERE ef.episode_id = ? AND ef.kind = 'main' AND ef.part_number IS NOT NULL",
    )
    .bind(&ep)
    .fetch_optional(state.db.get_pool())
    .await
    .unwrap();
    assert_eq!(remaining, Some(2), "part 2 must survive");
}

#[tokio::test]
async fn unassign_all_parts_disowns_the_episode() {
    let (app, state, tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Multipart All").await;
    let ep = format!("{series_id}_1_1");
    add_episode(&state.db, &ep, &series_id, 1, 1, None).await;

    let p1 = write_file(tmp.path(), "dl/all1.mkv");
    let p2 = write_file(tmp.path(), "dl/all2.mkv");
    let p1s = p1.to_string_lossy().to_string();
    let p2s = p2.to_string_lossy().to_string();
    state
        .db
        .upsert_episode_part(&ep, 1, &p1s, None)
        .await
        .unwrap();
    state
        .db
        .upsert_episode_part(&ep, 2, &p2s, None)
        .await
        .unwrap();

    app.clone()
        .oneshot(common::post_json_request(
            &format!("/api/series/{series_id}/files/unassign"),
            &BatchDeletePayload {
                paths: vec![p1s.clone(), p2s.clone()],
            },
        ))
        .await
        .unwrap();

    assert_eq!(part_count(&state.db, &ep).await, 0, "all parts removed");
    let row = ep_row(&state.db, &ep).await;
    assert_eq!(row.status.as_deref(), Some("missing"));
    assert!(
        !state.db.is_episode_assigned(&ep).await.unwrap(),
        "an episode with no parts and no file is not downloaded"
    );
}

#[tokio::test]
async fn delete_one_part_removes_only_that_part() {
    let (app, state, tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Multipart Delete").await;
    let ep = format!("{series_id}_1_1");
    add_episode(&state.db, &ep, &series_id, 1, 1, None).await;

    let p1 = write_file(tmp.path(), "dl/del1.mkv");
    let p2 = write_file(tmp.path(), "dl/del2.mkv");
    let p1s = p1.to_string_lossy().to_string();
    let p2s = p2.to_string_lossy().to_string();
    state
        .db
        .upsert_episode_part(&ep, 1, &p1s, None)
        .await
        .unwrap();
    state
        .db
        .upsert_episode_part(&ep, 2, &p2s, None)
        .await
        .unwrap();

    app.delete_json::<_, serde_json::Value>(
        &format!("/api/series/{series_id}/files"),
        &BatchDeletePayload {
            paths: vec![p1s.clone()],
        },
    )
    .await;

    assert!(!p1.exists(), "the deleted part file is gone from disk");
    assert_eq!(
        part_count(&state.db, &ep).await,
        1,
        "only the deleted part's row is removed"
    );
    let remaining: Option<i64> = sqlx::query_scalar(
        "SELECT ef.part_number FROM episode_files ef \
         WHERE ef.episode_id = ? AND ef.kind = 'main' AND ef.part_number IS NOT NULL",
    )
    .bind(&ep)
    .fetch_optional(state.db.get_pool())
    .await
    .unwrap();
    assert_eq!(remaining, Some(2));
}

#[tokio::test]
async fn delete_all_parts_disowns_the_episode() {
    let (app, state, tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Multipart Delete All").await;
    let ep = format!("{series_id}_1_1");
    add_episode(&state.db, &ep, &series_id, 1, 1, None).await;

    let p1 = write_file(tmp.path(), "dl/dall1.mkv");
    let p2 = write_file(tmp.path(), "dl/dall2.mkv");
    let p1s = p1.to_string_lossy().to_string();
    let p2s = p2.to_string_lossy().to_string();
    state
        .db
        .upsert_episode_part(&ep, 1, &p1s, None)
        .await
        .unwrap();
    state
        .db
        .upsert_episode_part(&ep, 2, &p2s, None)
        .await
        .unwrap();

    app.delete_json::<_, serde_json::Value>(
        &format!("/api/series/{series_id}/files"),
        &BatchDeletePayload {
            paths: vec![p1s.clone(), p2s.clone()],
        },
    )
    .await;

    assert_eq!(part_count(&state.db, &ep).await, 0, "all parts removed");
    let row = ep_row(&state.db, &ep).await;
    assert_eq!(row.status.as_deref(), Some("missing"));
    assert!(
        !state.db.is_episode_assigned(&ep).await.unwrap(),
        "deleting every part leaves the episode not downloaded"
    );
}

#[tokio::test]
async fn multipart_part_path_update_keeps_episode_downloaded() {
    // Mirrors the reorganize path: re-registering a part under its new path
    // updates the existing row in place (no duplicate) and the episode stays
    // downloaded through its remaining parts.
    let (app, state, tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Multipart Rename").await;
    let ep = format!("{series_id}_1_1");
    add_episode(&state.db, &ep, &series_id, 1, 1, None).await;

    let p1 = write_file(tmp.path(), "dl/r1.mkv");
    let p2 = write_file(tmp.path(), "dl/r2.mkv");
    let p1s = p1.to_string_lossy().to_string();
    let p2s = p2.to_string_lossy().to_string();
    state
        .db
        .upsert_episode_part(&ep, 1, &p1s, Some(10))
        .await
        .unwrap();
    state
        .db
        .upsert_episode_part(&ep, 2, &p2s, Some(20))
        .await
        .unwrap();
    assert!(state.db.is_episode_assigned(&ep).await.unwrap());

    let renamed = tmp.path().join("lib/r1 - pt1.mkv");
    std::fs::create_dir_all(renamed.parent().unwrap()).unwrap();
    std::fs::rename(&p1, &renamed).unwrap();
    let renamed_str = renamed.to_string_lossy().to_string();
    state
        .db
        .upsert_episode_part(&ep, 1, &renamed_str, Some(10))
        .await
        .unwrap();

    assert_eq!(part_count(&state.db, &ep).await, 2, "no duplicate part row");
    let path: Option<String> = sqlx::query_scalar(
        "SELECT fp.file_path FROM episode_files ef \
         JOIN file_paths fp ON fp.id = ef.file_path_id \
         WHERE ef.episode_id = ? AND ef.kind = 'main' AND ef.part_number = 1",
    )
    .bind(&ep)
    .fetch_one(state.db.get_pool())
    .await
    .unwrap();
    assert_eq!(path.as_deref(), Some(renamed_str.as_str()));
    assert!(
        state.db.is_episode_assigned(&ep).await.unwrap(),
        "episode still downloaded after a part rename"
    );
}

#[tokio::test]
async fn multipart_assign_disowns_an_episode_that_held_the_part_file() {
    // Consolidation of assignment-collision handling: the manual multipart branch
    // routes through `is_assignment_conflict`, so a part file already owned by
    // another episode displaces that episode (its complete range).
    let (app, state, tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Multipart Collision").await;

    // pt1 becomes S01E01's (part-suffixed) primary; pt2 belongs to S01E02.
    let pt1 = write_file(tmp.path(), "lib/Conflict - S01E01-pt1.mkv");
    let pt2 = write_file(tmp.path(), "lib/Conflict - S01E02-pt2.mkv");
    let pt1s = pt1.to_string_lossy().to_string();
    let pt2s = pt2.to_string_lossy().to_string();

    let assign = |path: String, episode: &str| {
        let app = app.clone();
        let series_id = series_id.clone();
        let episode = episode.to_string();
        async move {
            app.oneshot(common::post_json_request(
                &format!("/api/series/{series_id}/files/assign"),
                &AssignFilePayload {
                    path,
                    season: "1".to_string(),
                    episode,
                },
            ))
            .await
            .unwrap()
        }
    };

    assert_eq!(
        assign(pt1s.clone(), "1").await.status(),
        axum::http::StatusCode::OK
    );
    assert_eq!(
        assign(pt2s.clone(), "2").await.status(),
        axum::http::StatusCode::OK
    );

    let details: jumbie_shared::types::SeriesDetails =
        app.get_json(&format!("/api/series/{series_id}")).await;
    let ep_a = details
        .episodes
        .iter()
        .find(|e| e.episode == 1)
        .expect("episode 1")
        .unique_id
        .clone();
    let ep_b = details
        .episodes
        .iter()
        .find(|e| e.episode == 2)
        .expect("episode 2")
        .unique_id
        .clone();
    assert_eq!(
        ep_row(&state.db, &ep_b).await.file_path.as_deref(),
        Some(pt2s.as_str()),
        "S01E02 starts owning pt2"
    );

    // Assign pt2 to S01E01: both files are part-suffixed, so this is the multipart
    // branch. pt2 is held by S01E02, which must be disowned.
    assert_eq!(
        assign(pt2s.clone(), "1").await.status(),
        axum::http::StatusCode::OK
    );

    let b = ep_row(&state.db, &ep_b).await;
    assert_eq!(b.file_path, None, "S01E02 must lose the reassigned file");
    assert_eq!(b.status.as_deref(), Some("missing"));
    assert_eq!(
        part_count(&state.db, &ep_a).await,
        2,
        "S01E01 now owns pt1 and pt2 as parts"
    );
}

// ── Language/version variants attach alongside the main file ─────────────────

#[tokio::test]
async fn language_variant_attaches_alongside_the_primary() {
    let (app, state, tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Variant Show").await;

    let primary = write_file(tmp.path(), "lib/Variant - S01E01.mkv");
    let variant = write_file(tmp.path(), "lib/Variant - S01E01.en.mkv");
    let variant_s = variant.to_string_lossy().to_string();

    assert_eq!(
        assign(&app, &series_id, &primary, "1").await,
        axum::http::StatusCode::OK
    );
    let ep = episode_id_for(&app, &series_id, 1).await;

    // Assigning the language variant to the same episode must not replace or
    // conflict with the primary video — it attaches alongside it.
    assert_eq!(
        assign(&app, &series_id, &variant, "1").await,
        axum::http::StatusCode::OK
    );

    let main = ep_row(&state.db, &ep).await.file_path;
    assert!(
        main.as_deref()
            .is_some_and(|p| p.ends_with("Variant - S01E01.mkv")),
        "the primary video must still be the episode's main file, got {main:?}"
    );
    assert_eq!(
        linked_paths(&state.db, &ep).await,
        vec![variant_s.clone()],
        "the language variant must be attached as a linked file"
    );

    // The Manage Files list must show the variant as assigned to the episode.
    let files: Vec<jumbie_shared::types::SeriesFileViewModel> = app
        .get_json(&format!("/api/series/{series_id}/files"))
        .await;
    let listed = files
        .iter()
        .find(|f| f.path == variant_s)
        .expect("variant listed");
    assert!(
        listed.assigned_id.is_some(),
        "the variant must be reported as assigned"
    );
}

#[tokio::test]
async fn alternative_version_and_collision_counter_replace_the_primary() {
    // A version marker (`v2`) and a rename collision counter (`.001`) are distinct
    // artifacts, not language variants: assigning them competes for the episode's
    // file and replaces the current primary.
    for (series_title, base, candidate) in [
        ("Version Show", "Version", "Version - S01E01.v2.mkv"),
        ("Counter Show", "Counter", "Counter - S01E01.001.mkv"),
    ] {
        let (app, state, tmp) = common::setup_test_app().await;
        let series_id = common::create_test_series(&app, series_title).await;

        let primary = write_file(tmp.path(), &format!("lib/{base} - S01E01.mkv"));
        let other = write_file(tmp.path(), &format!("lib/{candidate}"));

        assert_eq!(
            assign(&app, &series_id, &primary, "1").await,
            axum::http::StatusCode::OK
        );
        let ep = episode_id_for(&app, &series_id, 1).await;
        assert_eq!(
            assign(&app, &series_id, &other, "1").await,
            axum::http::StatusCode::OK
        );

        let main = ep_row(&state.db, &ep).await.file_path;
        assert!(
            main.as_deref().is_some_and(|p| p.ends_with(candidate)),
            "{candidate} must become the main file, got {main:?}"
        );
        assert!(
            linked_paths(&state.db, &ep).await.is_empty(),
            "{candidate} is not a language variant"
        );
    }
}

#[tokio::test]
async fn different_release_still_replaces_the_primary() {
    // A genuinely different release (different quality) is not a variant: assigning
    // it replaces the current main file, exactly as before.
    let (app, state, tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Upgrade Show").await;

    let old = write_file(tmp.path(), "lib/Upgrade - S01E01.1080p.mkv");
    let better = write_file(tmp.path(), "lib/Upgrade - S01E01.720p.mkv");

    assert_eq!(
        assign(&app, &series_id, &old, "1").await,
        axum::http::StatusCode::OK
    );
    let ep = episode_id_for(&app, &series_id, 1).await;
    assert_eq!(
        assign(&app, &series_id, &better, "1").await,
        axum::http::StatusCode::OK
    );

    let main = ep_row(&state.db, &ep).await.file_path;
    assert!(
        main.as_deref()
            .is_some_and(|p| p.ends_with("Upgrade - S01E01.720p.mkv")),
        "a different release replaces the primary, got {main:?}"
    );
    assert!(
        linked_paths(&state.db, &ep).await.is_empty(),
        "no linked attachment for a replacement"
    );
}

#[tokio::test]
async fn multipart_variant_attaches_alongside_its_part() {
    let (app, state, tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Part Variant").await;

    let pt1 = write_file(tmp.path(), "lib/Part Variant - S01E01-pt1.mkv");
    let pt1_lang = write_file(tmp.path(), "lib/Part Variant - S01E01-pt1.en.mkv");
    let pt1_s = pt1.to_string_lossy().to_string();
    let pt1_lang_s = pt1_lang.to_string_lossy().to_string();

    assert_eq!(
        assign(&app, &series_id, &pt1, "1").await,
        axum::http::StatusCode::OK
    );
    let ep = episode_id_for(&app, &series_id, 1).await;
    // Both files carry a part suffix, so this is the multipart branch; the language
    // variant of part 1 must attach alongside, not replace it.
    assert_eq!(
        assign(&app, &series_id, &pt1_lang, "1").await,
        axum::http::StatusCode::OK
    );

    assert_eq!(
        ep_row(&state.db, &ep).await.file_path.as_deref(),
        Some(pt1_s.as_str()),
        "the primary part must survive"
    );
    assert_eq!(linked_paths(&state.db, &ep).await, vec![pt1_lang_s]);
}

#[tokio::test]
async fn multi_episode_variant_attaches_alongside_every_covering_episode() {
    let (app, state, tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Range Variant").await;

    let range = write_file(tmp.path(), "lib/Range Variant - S01E01E02.mkv");
    let range_lang = write_file(tmp.path(), "lib/Range Variant - S01E01E02.en.mkv");
    let range_lang_s = range_lang.to_string_lossy().to_string();

    // One file covers S01E01–S01E02; a language variant of that same range must
    // attach alongside it on every episode it covers, not replace it.
    assert_eq!(
        assign(&app, &series_id, &range, "1-2").await,
        axum::http::StatusCode::OK
    );
    let ep1 = episode_id_for(&app, &series_id, 1).await;
    let ep2 = episode_id_for(&app, &series_id, 2).await;
    assert_eq!(
        assign(&app, &series_id, &range_lang, "1-2").await,
        axum::http::StatusCode::OK
    );

    for ep in [&ep1, &ep2] {
        let main = ep_row(&state.db, ep).await.file_path;
        assert!(
            main.as_deref()
                .is_some_and(|p| p.ends_with("S01E01E02.mkv")),
            "the range file must stay main on {ep}, got {main:?}"
        );
        assert_eq!(
            linked_paths(&state.db, ep).await,
            vec![range_lang_s.clone()],
            "the variant must be linked on {ep}"
        );
    }
}

#[tokio::test]
async fn subtitle_assignments_never_conflict_and_are_unlimited() {
    // Confirmation of existing behaviour: subtitles are auxiliary sidecars — several
    // attach to one episode without conflicting with each other or its video.
    let (app, state, tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Subtitle Show").await;

    let video = write_file(tmp.path(), "lib/Subtitle - S01E01.mkv");
    let en = write_file(tmp.path(), "lib/Subtitle - S01E01.en.srt");
    let es = write_file(tmp.path(), "lib/Subtitle - S01E01.es.srt");

    assert_eq!(
        assign(&app, &series_id, &video, "1").await,
        axum::http::StatusCode::OK
    );
    let ep = episode_id_for(&app, &series_id, 1).await;
    assert_eq!(
        assign(&app, &series_id, &en, "1").await,
        axum::http::StatusCode::OK
    );
    assert_eq!(
        assign(&app, &series_id, &es, "1").await,
        axum::http::StatusCode::OK
    );

    let aux = state.db.get_auxiliary_files_for_episode(&ep).await.unwrap();
    assert_eq!(aux.len(), 2, "both subtitles attach as auxiliary");
    let main = ep_row(&state.db, &ep).await.file_path;
    assert!(
        main.as_deref()
            .is_some_and(|p| p.ends_with("Subtitle - S01E01.mkv")),
        "subtitles never replace the video, got {main:?}"
    );
    assert!(linked_paths(&state.db, &ep).await.is_empty());
}

#[tokio::test]
async fn different_language_tags_attach_alongside_the_primary() {
    let (app, state, tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Tags Show").await;

    let plain = write_file(tmp.path(), "lib/Tags - S01E01.mkv");
    let en = write_file(tmp.path(), "lib/Tags - S01E01.en.mkv");
    let eng = write_file(tmp.path(), "lib/Tags - S01E01.eng.mkv");

    assert_eq!(
        assign(&app, &series_id, &plain, "1").await,
        axum::http::StatusCode::OK
    );
    let ep = episode_id_for(&app, &series_id, 1).await;
    for tag in [&en, &eng] {
        assert_eq!(
            assign(&app, &series_id, tag, "1").await,
            axum::http::StatusCode::OK
        );
    }

    let main = ep_row(&state.db, &ep).await.file_path;
    assert!(
        main.as_deref()
            .is_some_and(|p| p.ends_with("Tags - S01E01.mkv")),
        "the plain file must be the primary, got {main:?}"
    );
    let mut linked = linked_paths(&state.db, &ep).await;
    linked.sort();
    let mut expected = vec![
        en.to_string_lossy().to_string(),
        eng.to_string_lossy().to_string(),
    ];
    expected.sort();
    assert_eq!(linked, expected, "both language tags must attach");
}

#[tokio::test]
async fn a_duplicate_language_tag_replaces_the_existing_variant() {
    // A manual assignment is deliberate intent, so a second English file takes the
    // English slot, replacing the previous one (like a multipart part slot).
    let (app, state, tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Dup Tag Show").await;

    let plain = write_file(tmp.path(), "lib/Dup - S01E01.mkv");
    let en = write_file(tmp.path(), "lib/Dup - S01E01.en.mkv");
    // A second English file, elsewhere on disk.
    let en_dup = write_file(tmp.path(), "other/Dup - S01E01.en.mkv");

    assert_eq!(
        assign(&app, &series_id, &plain, "1").await,
        axum::http::StatusCode::OK
    );
    let ep = episode_id_for(&app, &series_id, 1).await;
    assert_eq!(
        assign(&app, &series_id, &en, "1").await,
        axum::http::StatusCode::OK
    );
    assert_eq!(
        assign(&app, &series_id, &en_dup, "1").await,
        axum::http::StatusCode::OK
    );

    assert_eq!(
        linked_paths(&state.db, &ep).await,
        vec![en_dup.to_string_lossy().to_string()],
        "one file per language tag: the new file replaces the old"
    );
    let main = ep_row(&state.db, &ep).await.file_path;
    assert!(
        main.as_deref()
            .is_some_and(|p| p.ends_with("Dup - S01E01.mkv")),
        "the primary must be untouched, got {main:?}"
    );
}

#[tokio::test]
async fn only_language_tagged_file_is_a_normal_assigned_episode() {
    // With no plain sibling, a language-tagged file is the episode's file (a normal
    // assigned episode), not an attachment to a missing primary.
    let (app, state, tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Lone Tag Show").await;
    let en = write_file(tmp.path(), "lib/Lone - S01E01.en.mkv");

    assert_eq!(
        assign(&app, &series_id, &en, "1").await,
        axum::http::StatusCode::OK
    );
    let ep = episode_id_for(&app, &series_id, 1).await;

    let row = ep_row(&state.db, &ep).await;
    assert!(
        row.file_path
            .as_deref()
            .is_some_and(|p| p.ends_with("Lone - S01E01.en.mkv")),
        "the lone tagged file must be the episode's file, got {:?}",
        row.file_path
    );
    assert!(linked_paths(&state.db, &ep).await.is_empty());

    // Reported as assigned (a normal episode file), not as an attachment.
    let files: Vec<jumbie_shared::types::SeriesFileViewModel> = app
        .get_json(&format!("/api/series/{series_id}/files"))
        .await;
    let listed = files
        .iter()
        .find(|f| f.path.ends_with("Lone - S01E01.en.mkv"))
        .expect("listed");
    assert!(listed.assigned_id.is_some());
}

// ── Language slots across episode shapes ────────────────────────────────────

#[tokio::test]
async fn multi_episode_distinct_language_tags_attach_on_every_covering_episode() {
    let (app, state, tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Range Tags").await;

    let range = write_file(tmp.path(), "lib/Range Tags - S01E01E02.mkv");
    let en = write_file(tmp.path(), "lib/Range Tags - S01E01E02.en.mkv");
    let eng = write_file(tmp.path(), "lib/Range Tags - S01E01E02.eng.mkv");

    assert_eq!(
        assign(&app, &series_id, &range, "1-2").await,
        axum::http::StatusCode::OK
    );
    for tag in [&en, &eng] {
        assert_eq!(
            assign(&app, &series_id, tag, "1-2").await,
            axum::http::StatusCode::OK
        );
    }

    let mut expected = vec![
        en.to_string_lossy().to_string(),
        eng.to_string_lossy().to_string(),
    ];
    expected.sort();
    for n in [1, 2] {
        let ep = episode_id_for(&app, &series_id, n).await;
        let main = ep_row(&state.db, &ep).await.file_path;
        assert!(
            main.as_deref()
                .is_some_and(|p| p.ends_with("Range Tags - S01E01E02.mkv")),
            "S01E0{n} main must be the range file, got {main:?}"
        );
        let mut linked = linked_paths(&state.db, &ep).await;
        linked.sort();
        assert_eq!(linked, expected, "S01E0{n} must hold both language tags");
    }
}

#[tokio::test]
async fn multi_episode_duplicate_language_tag_replaces_the_existing_variant() {
    let (app, state, tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Range Dup").await;

    let range = write_file(tmp.path(), "lib/Range Dup - S01E01E02.mkv");
    let en = write_file(tmp.path(), "lib/Range Dup - S01E01E02.en.mkv");
    // A second English range file, elsewhere on disk.
    let en_dup = write_file(tmp.path(), "other/Range Dup - S01E01E02.en.mkv");

    assert_eq!(
        assign(&app, &series_id, &range, "1-2").await,
        axum::http::StatusCode::OK
    );
    assert_eq!(
        assign(&app, &series_id, &en, "1-2").await,
        axum::http::StatusCode::OK
    );
    assert_eq!(
        assign(&app, &series_id, &en_dup, "1-2").await,
        axum::http::StatusCode::OK
    );

    for n in [1, 2] {
        let ep = episode_id_for(&app, &series_id, n).await;
        assert_eq!(
            linked_paths(&state.db, &ep).await,
            vec![en_dup.to_string_lossy().to_string()],
            "S01E0{n}: the new file replaces the old tag"
        );
    }
}

#[tokio::test]
async fn multi_episode_lone_language_tag_is_a_normal_assigned_episode() {
    let (app, state, tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Range Lone").await;
    let en = write_file(tmp.path(), "lib/Range Lone - S01E01E02.en.mkv");

    assert_eq!(
        assign(&app, &series_id, &en, "1-2").await,
        axum::http::StatusCode::OK
    );

    for n in [1, 2] {
        let ep = episode_id_for(&app, &series_id, n).await;
        assert!(
            ep_row(&state.db, &ep)
                .await
                .file_path
                .as_deref()
                .is_some_and(|p| p.ends_with("Range Lone - S01E01E02.en.mkv")),
            "S01E0{n}: the lone tagged range file must be the episode's file"
        );
        assert!(linked_paths(&state.db, &ep).await.is_empty());
    }
}

#[tokio::test]
async fn multipart_distinct_language_tags_attach_to_their_part() {
    let (app, state, tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Part Tags").await;

    let pt1 = write_file(tmp.path(), "lib/Part Tags - S01E01-pt1.mkv");
    let pt1_en = write_file(tmp.path(), "lib/Part Tags - S01E01-pt1.en.mkv");
    let pt1_eng = write_file(tmp.path(), "lib/Part Tags - S01E01-pt1.eng.mkv");

    assert_eq!(
        assign(&app, &series_id, &pt1, "1").await,
        axum::http::StatusCode::OK
    );
    let ep = episode_id_for(&app, &series_id, 1).await;
    for tag in [&pt1_en, &pt1_eng] {
        assert_eq!(
            assign(&app, &series_id, tag, "1").await,
            axum::http::StatusCode::OK
        );
    }

    let mut linked = linked_paths(&state.db, &ep).await;
    linked.sort();
    let mut expected = vec![
        pt1_en.to_string_lossy().to_string(),
        pt1_eng.to_string_lossy().to_string(),
    ];
    expected.sort();
    assert_eq!(linked, expected, "both tags attach to part 1");
}

#[tokio::test]
async fn multipart_duplicate_language_tag_replaces_the_existing_variant() {
    let (app, state, tmp) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Part Dup").await;

    let pt1 = write_file(tmp.path(), "lib/Part Dup - S01E01-pt1.mkv");
    let pt1_en = write_file(tmp.path(), "lib/Part Dup - S01E01-pt1.en.mkv");
    // A second English part 1, elsewhere on disk.
    let pt1_en_dup = write_file(tmp.path(), "other/Part Dup - S01E01-pt1.en.mkv");

    assert_eq!(
        assign(&app, &series_id, &pt1, "1").await,
        axum::http::StatusCode::OK
    );
    let ep = episode_id_for(&app, &series_id, 1).await;
    assert_eq!(
        assign(&app, &series_id, &pt1_en, "1").await,
        axum::http::StatusCode::OK
    );
    assert_eq!(
        assign(&app, &series_id, &pt1_en_dup, "1").await,
        axum::http::StatusCode::OK
    );

    assert_eq!(
        linked_paths(&state.db, &ep).await,
        vec![pt1_en_dup.to_string_lossy().to_string()],
        "one file per language tag on a part: the new file replaces the old"
    );
    assert_eq!(
        ep_row(&state.db, &ep).await.file_path.as_deref(),
        Some(pt1.to_string_lossy().as_ref()),
        "the primary part must survive"
    );
}

// ── Replacement obeys the organization collision strategy ──────────────────

#[tokio::test]
async fn episode_replacement_obeys_collision_handling() {
    for (mode, old_survives, replaced) in [
        ("overwrite", false, true),
        ("rename", true, true),
        ("skip", true, false),
    ] {
        let (app, state, tmp) = common::setup_test_app().await;
        common::set_collision_handling(&state, mode).await;
        let series_id = common::create_test_series(&app, &format!("Coll {mode}")).await;
        let new_name = format!("Coll {mode} - S01E01.720p.mkv");
        let old = write_file(tmp.path(), &format!("lib/Coll {mode} - S01E01.mkv"));
        let new = write_file(tmp.path(), &format!("lib/{new_name}"));

        assert_eq!(
            assign(&app, &series_id, &old, "1").await,
            axum::http::StatusCode::OK
        );
        let ep = episode_id_for(&app, &series_id, 1).await;
        let old_main = ep_row(&state.db, &ep)
            .await
            .file_path
            .expect("the first file is assigned");
        assert_eq!(
            assign(&app, &series_id, &new, "1").await,
            axum::http::StatusCode::OK
        );

        assert_eq!(
            std::path::Path::new(&old_main).exists(),
            old_survives,
            "{mode}: superseded episode file survival"
        );
        let main = ep_row(&state.db, &ep).await.file_path;
        if replaced {
            assert!(
                main.as_deref().is_some_and(|p| p.ends_with(&new_name)),
                "{mode}: the new release takes the slot, got {main:?}"
            );
        } else {
            assert_eq!(main.as_deref(), Some(old_main.as_str()), "{mode}: declined");
            assert!(new.exists(), "{mode}: the declined file is left on disk");
        }
    }
}

#[tokio::test]
async fn variant_replacement_obeys_collision_handling() {
    for (mode, old_survives, replaced) in [
        ("overwrite", false, true),
        ("rename", true, true),
        ("skip", true, false),
    ] {
        let (app, state, tmp) = common::setup_test_app().await;
        common::set_collision_handling(&state, mode).await;
        let series_id = common::create_test_series(&app, &format!("VColl {mode}")).await;
        let plain = write_file(tmp.path(), &format!("lib/VColl {mode} - S01E01.mkv"));
        let en = write_file(tmp.path(), &format!("lib/VColl {mode} - S01E01.en.mkv"));
        let en_dup = write_file(tmp.path(), &format!("other/VColl {mode} - S01E01.en.mkv"));

        assert_eq!(
            assign(&app, &series_id, &plain, "1").await,
            axum::http::StatusCode::OK
        );
        let ep = episode_id_for(&app, &series_id, 1).await;
        assert_eq!(
            assign(&app, &series_id, &en, "1").await,
            axum::http::StatusCode::OK
        );
        assert_eq!(
            assign(&app, &series_id, &en_dup, "1").await,
            axum::http::StatusCode::OK
        );

        assert_eq!(
            en.exists(),
            old_survives,
            "{mode}: superseded variant survival"
        );
        let linked = linked_paths(&state.db, &ep).await;
        if replaced {
            assert_eq!(
                linked,
                vec![en_dup.to_string_lossy().to_string()],
                "{mode}: the new variant takes the tag"
            );
        } else {
            assert_eq!(linked, vec![en.to_string_lossy().to_string()], "{mode}");
            assert!(en_dup.exists(), "{mode}: the declined file is left on disk");
        }
    }
}

#[tokio::test]
async fn part_replacement_obeys_collision_handling() {
    for (mode, old_survives, replaced) in [
        ("overwrite", false, true),
        ("rename", true, true),
        ("skip", true, false),
    ] {
        let (app, state, tmp) = common::setup_test_app().await;
        common::set_collision_handling(&state, mode).await;
        let series = format!("PColl {mode}");
        let series_id = common::create_test_series(&app, &series).await;

        let pt1 = write_file(tmp.path(), &format!("lib/{series} - S01E01-pt1.mkv"));
        let pt2 = write_file(tmp.path(), &format!("lib/{series} - S01E01-pt2.mkv"));
        // A different release for part 1, elsewhere on disk.
        let pt1_new = write_file(
            tmp.path(),
            &format!("other/{series} - S01E01-pt1.1080p.mkv"),
        );

        for part in [&pt1, &pt2] {
            assert_eq!(
                assign(&app, &series_id, part, "1").await,
                axum::http::StatusCode::OK
            );
        }
        let ep = episode_id_for(&app, &series_id, 1).await;
        assert_eq!(
            part_path(&state.db, &ep, 1).await.as_deref(),
            Some(pt1.to_string_lossy().as_ref()),
            "precondition: part 1 is the first file"
        );

        assert_eq!(
            assign(&app, &series_id, &pt1_new, "1").await,
            axum::http::StatusCode::OK
        );

        assert_eq!(
            pt1.exists(),
            old_survives,
            "{mode}: superseded part survival"
        );
        if replaced {
            assert_eq!(
                part_path(&state.db, &ep, 1).await.as_deref(),
                Some(pt1_new.to_string_lossy().as_ref()),
                "{mode}: the new file takes part 1"
            );
            assert_eq!(
                part_path(&state.db, &ep, 2).await.as_deref(),
                Some(pt2.to_string_lossy().as_ref()),
                "{mode}: part 2 is untouched"
            );
        } else {
            assert_eq!(
                part_path(&state.db, &ep, 1).await.as_deref(),
                Some(pt1.to_string_lossy().as_ref()),
                "{mode}: declined"
            );
            assert!(
                pt1_new.exists(),
                "{mode}: the declined file is left on disk"
            );
        }
    }
}

/// The episode's **first** part is stored as its non-part primary until a second
/// part arrives. Replacing it must still obey the collision strategy and must not
/// leave the superseded association behind as a second home for the same part.
#[tokio::test]
async fn first_part_replacement_obeys_collision_handling() {
    for (mode, old_survives, replaced) in [
        ("overwrite", false, true),
        ("rename", true, true),
        ("skip", true, false),
    ] {
        let (app, state, tmp) = common::setup_test_app().await;
        common::set_collision_handling(&state, mode).await;
        let series = format!("FPt {mode}");
        let series_id = common::create_test_series(&app, &series).await;

        let pt1 = write_file(tmp.path(), &format!("lib/{series} - S01E01-pt1.mkv"));
        let pt1_new = write_file(
            tmp.path(),
            &format!("other/{series} - S01E01-pt1.1080p.mkv"),
        );

        assert_eq!(
            assign(&app, &series_id, &pt1, "1").await,
            axum::http::StatusCode::OK
        );
        let ep = episode_id_for(&app, &series_id, 1).await;
        assert_eq!(
            part_path(&state.db, &ep, 1).await,
            None,
            "precondition: the first part is the implicit primary"
        );

        assert_eq!(
            assign(&app, &series_id, &pt1_new, "1").await,
            axum::http::StatusCode::OK
        );

        assert_eq!(
            pt1.exists(),
            old_survives,
            "{mode}: superseded part survival"
        );
        assert_eq!(
            part_count(&state.db, &ep).await,
            1,
            "{mode}: exactly one main file remains"
        );
        let main = ep_row(&state.db, &ep).await.file_path.unwrap_or_default();
        if replaced {
            assert!(
                main.ends_with(&format!("{series} - S01E01-pt1.1080p.mkv")),
                "{mode}: the new file takes the part, got {main}"
            );
        } else {
            assert!(
                main.ends_with(&format!("{series} - S01E01-pt1.mkv")),
                "{mode}: declined, got {main}"
            );
            assert!(
                pt1_new.exists(),
                "{mode}: the declined file is left on disk"
            );
        }
    }
}
