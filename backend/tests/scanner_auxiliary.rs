// Auxiliary sidecar (subtitle/nfo) handling by the scanner.
//
// Both the generic scanner (`scan_directory`) and the per-series scanner
// (`scan_series_directory` -> `import_scan_for_series`) must:
//   - attach a sidecar to the episode cell it names, and
//   - never create an episode cell of their own.
//
// A sidecar whose cell does not exist stays unassigned (the user may materialize
// the cell through manual assignment). Sidecars are stashed during the walk and
// attached afterwards, so a sidecar visited before its video still attaches in a
// single pass.

mod common;

fn write(dir: &std::path::Path, name: &str, bytes: &[u8]) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join(name), bytes).unwrap();
}

/// Whether the S01E`episode` cell of `series_id` exists.
async fn cell_exists(state: &jumbie::api::AppState, series_id: &str, episode: i32) -> bool {
    sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM episodes WHERE episode_id = ?)")
        .bind(format!("{series_id}_S01E{episode:02}"))
        .fetch_one(state.db.get_pool())
        .await
        .unwrap()
}

/// `auxiliary` paths attached to the S01E`episode` cell of `series_id`.
async fn auxiliary_paths(
    state: &jumbie::api::AppState,
    series_id: &str,
    episode: i32,
) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT fp.file_path FROM episode_files ef \
         JOIN file_paths fp ON fp.id = ef.file_path_id \
         WHERE ef.episode_id = ? AND ef.kind = 'auxiliary' \
         ORDER BY fp.file_path",
    )
    .bind(format!("{series_id}_S01E{episode:02}"))
    .fetch_all(state.db.get_pool())
    .await
    .unwrap()
}

/// A sidecar alone must not create an episode cell. The import scanner resolves
/// sidecars in phase 1 (before any video cell exists), so this is the strictest
/// form of the rule.
#[tokio::test]
async fn import_scanner_does_not_create_cell_for_orphan_sidecar() {
    let (app, state, temp_dir) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Orphan Import Aux").await;
    let dir = temp_dir.path().join("organized").join("Orphan Import Aux");
    write(&dir, "Orphan Import Aux - S01E01.en.srt", b"sub");

    let mapping = state
        .db
        .get_series_mapping(&series_id)
        .await
        .unwrap()
        .unwrap();
    jumbie::scanner::scan_series_directory(&dir, &mapping, &state)
        .await
        .expect("scan_series_directory");

    assert!(
        !cell_exists(&state, &series_id, 1).await,
        "a sidecar must not create an episode cell"
    );
    assert!(
        auxiliary_paths(&state, &series_id, 1).await.is_empty(),
        "an unlinkable sidecar stays unassigned"
    );
}

/// The generic scanner must not create an episode cell from a sidecar either.
#[tokio::test]
async fn generic_scanner_does_not_create_cell_for_orphan_sidecar() {
    let (app, state, temp_dir) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Orphan Generic Aux").await;
    let dir = temp_dir.path().join("organized").join("Orphan Generic Aux");
    write(&dir, "Orphan Generic Aux - S01E01.en.srt", b"sub");

    jumbie::scanner::scan_directory(&dir, &state, false)
        .await
        .expect("scan_directory");

    assert!(
        !cell_exists(&state, &series_id, 1).await,
        "a sidecar must not create an episode cell"
    );
    assert!(auxiliary_paths(&state, &series_id, 1).await.is_empty());
}

/// A video and its sidecar in one directory must both land in a single pass: the
/// video creates the cell and the stashed sidecar attaches to it.
#[tokio::test]
async fn import_scanner_attaches_sidecar_to_video_cell_in_one_pass() {
    let (app, state, temp_dir) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "One Pass Aux").await;
    let dir = temp_dir.path().join("organized").join("One Pass Aux");
    write(&dir, "One Pass Aux - S01E01.mkv", b"video");
    write(&dir, "One Pass Aux - S01E01.en.srt", b"sub");

    let mapping = state
        .db
        .get_series_mapping(&series_id)
        .await
        .unwrap()
        .unwrap();
    jumbie::scanner::scan_series_directory(&dir, &mapping, &state)
        .await
        .expect("scan_series_directory");

    assert!(cell_exists(&state, &series_id, 1).await);
    let aux = auxiliary_paths(&state, &series_id, 1).await;
    assert_eq!(aux.len(), 1, "the sidecar must attach to its video's cell");
    assert!(aux[0].ends_with("One Pass Aux - S01E01.en.srt"));
}

/// The generic scanner agrees: video + sidecar in one pass.
#[tokio::test]
async fn generic_scanner_attaches_sidecar_to_video_cell_in_one_pass() {
    let (app, state, temp_dir) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Generic One Pass Aux").await;
    let dir = temp_dir
        .path()
        .join("organized")
        .join("Generic One Pass Aux");
    write(&dir, "Generic One Pass Aux - S01E01.mkv", b"video");
    write(&dir, "Generic One Pass Aux - S01E01.en.srt", b"sub");

    jumbie::scanner::scan_directory(&dir, &state, false)
        .await
        .expect("scan_directory");

    assert!(cell_exists(&state, &series_id, 1).await);
    let aux = auxiliary_paths(&state, &series_id, 1).await;
    assert_eq!(aux.len(), 1);
    assert!(aux[0].ends_with("Generic One Pass Aux - S01E01.en.srt"));
}

/// A sidecar added after its video (e.g. by Jellyfin) attaches to the existing
/// cell on the next scan.
#[tokio::test]
async fn import_scanner_attaches_late_added_sidecar_to_existing_cell() {
    let (app, state, temp_dir) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Late Sidecar Aux").await;
    let dir = temp_dir.path().join("organized").join("Late Sidecar Aux");
    write(&dir, "Late Sidecar Aux - S01E01.mkv", b"video");

    let mapping = state
        .db
        .get_series_mapping(&series_id)
        .await
        .unwrap()
        .unwrap();
    jumbie::scanner::scan_series_directory(&dir, &mapping, &state)
        .await
        .expect("scan_series_directory");
    assert!(cell_exists(&state, &series_id, 1).await);
    assert!(auxiliary_paths(&state, &series_id, 1).await.is_empty());

    write(&dir, "Late Sidecar Aux - S01E01.en.srt", b"sub");
    jumbie::scanner::scan_series_directory(&dir, &mapping, &state)
        .await
        .expect("scan_series_directory");

    let aux = auxiliary_paths(&state, &series_id, 1).await;
    assert_eq!(aux.len(), 1, "the late sidecar must attach to the cell");
    assert!(aux[0].ends_with("Late Sidecar Aux - S01E01.en.srt"));
}
