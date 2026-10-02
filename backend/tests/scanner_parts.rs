// Part-file registration parity// Part-file registration parity
//
// `scan_directory` (generic) and `scan_series_directory` → `import_scan_for_series`
// (per-series) must register a multipart episode identically — including a
// parts-only episode with no non-part sibling. Both route through the shared
// SSoT `scanner::parts::register_part_file`.

mod common;

/// Part numbers registered as `main` parts for the S01E01 parent of `series_id`.
async fn part_numbers(state: &jumbie::api::AppState, series_id: &str) -> Vec<i32> {
    sqlx::query_scalar(
        "SELECT ef.part_number FROM episode_files ef \
         WHERE ef.episode_id = ? AND ef.kind = 'main' AND ef.part_number IS NOT NULL \
         ORDER BY ef.part_number",
    )
    .bind(format!("{series_id}_S01E01"))
    .fetch_all(state.db.get_pool())
    .await
    .unwrap()
}

/// `(episode row exists, primary non-part main file_path)` for the S01E01 parent
/// of `series_id`. `Some(None)` therefore means the row exists but owns no direct
/// file — i.e. it is a parts-only episode.
async fn parent_row(state: &jumbie::api::AppState, series_id: &str) -> Option<Option<String>> {
    let episode_id = format!("{series_id}_S01E01");
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM episodes WHERE episode_id = ?)")
            .bind(&episode_id)
            .fetch_one(state.db.get_pool())
            .await
            .unwrap();
    if !exists {
        return None;
    }
    let file_path: Option<String> = sqlx::query_scalar::<_, String>(
        "SELECT fp.file_path FROM episode_files ef \
         JOIN file_paths fp ON fp.id = ef.file_path_id \
         WHERE ef.episode_id = ? AND ef.kind = 'main' AND ef.part_number IS NULL",
    )
    .bind(&episode_id)
    .fetch_optional(state.db.get_pool())
    .await
    .unwrap();
    Some(file_path)
}

fn write_part(dir: &std::path::Path, title: &str, part: u32, bytes: &[u8]) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join(format!("{title} - S01E01 - pt{part}.mkv")), bytes).unwrap();
}

#[tokio::test]
async fn import_scanner_registers_parts_only_episode() {
    let (app, state, temp_dir) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Import Parts Show").await;
    let dir = temp_dir.path().join("organized").join("Import Parts Show");
    write_part(&dir, "Import Parts Show", 1, b"one");
    write_part(&dir, "Import Parts Show", 2, b"two");

    let mapping = state
        .db
        .get_series_mapping(&series_id)
        .await
        .unwrap()
        .unwrap();
    jumbie::scanner::scan_series_directory(&dir, &mapping, &state)
        .await
        .expect("scan_series_directory");

    // A parts-only episode is a parent row with no direct (non-part) file plus
    // both parts.
    assert_eq!(
        parent_row(&state, &series_id).await,
        Some(None),
        "the parent episode row must exist with no direct file"
    );
    assert_eq!(part_numbers(&state, &series_id).await, vec![1, 2]);
}

#[tokio::test]
async fn both_scan_paths_agree_on_parts_only_episode() {
    let (app, state, temp_dir) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Agree Parts Show").await;
    let dir = temp_dir.path().join("organized").join("Agree Parts Show");
    write_part(&dir, "Agree Parts Show", 1, b"one");
    write_part(&dir, "Agree Parts Show", 2, b"two");

    // Path 1: per-series import scanner (background scanner / refresh / batch move).
    let mapping = state
        .db
        .get_series_mapping(&series_id)
        .await
        .unwrap()
        .unwrap();
    jumbie::scanner::scan_series_directory(&dir, &mapping, &state)
        .await
        .expect("scan_series_directory");
    assert_eq!(part_numbers(&state, &series_id).await, vec![1, 2]);

    // Path 2: generic directory scanner over the SAME directory — must agree and
    // stay idempotent (no duplicate part rows).
    jumbie::scanner::scan_directory(&dir, &state, false)
        .await
        .expect("scan_directory");
    assert_eq!(
        part_numbers(&state, &series_id).await,
        vec![1, 2],
        "both scan paths must register the same parts"
    );
}

/// Scanned parts register as `main` associations in `episode_files` (the SSoT for
/// multipart) and their files are fingerprinted so the details join resolves their
/// content — identical to the download and manual-assign paths.
#[tokio::test]
async fn scanned_parts_are_registered_and_fingerprinted() {
    let (app, state, temp_dir) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Linked Parts Show").await;
    let dir = temp_dir.path().join("organized").join("Linked Parts Show");
    write_part(&dir, "Linked Parts Show", 1, b"one");
    write_part(&dir, "Linked Parts Show", 2, b"two");

    let mapping = state
        .db
        .get_series_mapping(&series_id)
        .await
        .unwrap()
        .unwrap();
    jumbie::scanner::scan_series_directory(&dir, &mapping, &state)
        .await
        .expect("scan_series_directory");

    let ep_id = format!("{series_id}_S01E01");
    assert_eq!(part_numbers(&state, &series_id).await, vec![1, 2]);

    // Each part file must be fingerprinted (content resolved), and the multipart
    // association must live only in `episode_files` — not duplicated into a
    // non-part main association.
    let fingerprinted: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM file_paths fp \
         WHERE fp.fingerprint IS NOT NULL \
           AND fp.id IN (SELECT ef.file_path_id FROM episode_files ef \
                         WHERE ef.episode_id = ? AND ef.kind = 'main' \
                           AND ef.part_number IS NOT NULL)",
    )
    .bind(&ep_id)
    .fetch_one(state.db.get_pool())
    .await
    .unwrap();
    assert_eq!(fingerprinted, 2, "both scanned parts must be fingerprinted");

    let non_part_mains: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM episode_files \
         WHERE episode_id = ? AND kind = 'main' AND part_number IS NULL",
    )
    .bind(&ep_id)
    .fetch_one(state.db.get_pool())
    .await
    .unwrap();
    assert_eq!(
        non_part_mains, 0,
        "parts must not be duplicated into a non-part main association"
    );
}

#[tokio::test]
async fn import_scanner_skips_blocked_part_file() {
    let (app, state, temp_dir) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Blocked Import Parts").await;
    let dir = temp_dir
        .path()
        .join("organized")
        .join("Blocked Import Parts");
    write_part(&dir, "Blocked Import Parts", 1, b"one");
    write_part(&dir, "Blocked Import Parts", 2, b"two");

    let p2_size = std::fs::metadata(dir.join("Blocked Import Parts - S01E01 - pt2.mkv"))
        .unwrap()
        .len() as i64;
    state
        .db
        .block_file(
            &series_id,
            "Blocked Import Parts - S01E01 - pt2.mkv",
            p2_size,
            None,
        )
        .await
        .unwrap();

    let mapping = state
        .db
        .get_series_mapping(&series_id)
        .await
        .unwrap()
        .unwrap();
    jumbie::scanner::scan_series_directory(&dir, &mapping, &state)
        .await
        .expect("scan_series_directory");

    assert_eq!(
        part_numbers(&state, &series_id).await,
        vec![1],
        "a blocked part file must be skipped by the import scanner too"
    );
}
