// Language variants discovered by the scanner attach alongside the episode's primary
// instead of conflicting with it (or being silently dropped).
//
// Both the per-series scanner (`scan_series_directory` -> `import_scan_for_series`)
// and the generic scanner (`scan_directory`) must agree, and a non-language
// difference (`v2`, `.001`) must still be a conflict.

mod common;

/// `linked` (non-primary) video paths for `episode` (`S01E01`, `S01E02`, …).
async fn linked_paths_for(
    state: &jumbie::api::AppState,
    series_id: &str,
    episode: &str,
) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT fp.file_path FROM episode_files ef \
         JOIN file_paths fp ON fp.id = ef.file_path_id \
         WHERE ef.episode_id = ? AND ef.kind = 'linked' \
         ORDER BY fp.file_path",
    )
    .bind(format!("{series_id}_{episode}"))
    .fetch_all(state.db.get_pool())
    .await
    .unwrap()
}

/// The primary (non-part, `main`) path of `episode`, if any.
async fn primary_path_for(
    state: &jumbie::api::AppState,
    series_id: &str,
    episode: &str,
) -> Option<String> {
    sqlx::query_scalar::<_, String>(
        "SELECT fp.file_path FROM episode_files ef \
         JOIN file_paths fp ON fp.id = ef.file_path_id \
         WHERE ef.episode_id = ? AND ef.kind = 'main' AND ef.part_number IS NULL",
    )
    .bind(format!("{series_id}_{episode}"))
    .fetch_optional(state.db.get_pool())
    .await
    .unwrap()
}

/// `linked` (non-primary) video paths for the S01E01 episode of `series_id`.
async fn linked_paths(state: &jumbie::api::AppState, series_id: &str) -> Vec<String> {
    linked_paths_for(state, series_id, "S01E01").await
}

/// The episode's primary (non-part, `main`) path, if any.
async fn primary_path(state: &jumbie::api::AppState, series_id: &str) -> Option<String> {
    primary_path_for(state, series_id, "S01E01").await
}

/// Part numbers registered as `main` parts for S01E01.
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

fn write(dir: &std::path::Path, name: &str) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join(name), b"video").unwrap();
}

#[tokio::test]
async fn import_scanner_attaches_language_variant_alongside_primary() {
    let (app, state, temp_dir) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Variant Scan Show").await;
    let dir = temp_dir.path().join("organized").join("Variant Scan Show");
    write(&dir, "Variant Scan Show - S01E01.mkv");
    write(&dir, "Variant Scan Show - S01E01.en.mkv");

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
        primary_path(&state, &series_id)
            .await
            .is_some_and(|p| p.ends_with("Variant Scan Show - S01E01.mkv")),
        "the file without a language tag must be the primary"
    );
    let linked = linked_paths(&state, &series_id).await;
    assert_eq!(
        linked.len(),
        1,
        "the language variant must attach as linked"
    );
    assert!(linked[0].ends_with("Variant Scan Show - S01E01.en.mkv"));
}

#[tokio::test]
async fn generic_scanner_attaches_language_variant_alongside_primary() {
    let (app, state, temp_dir) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Generic Variant Show").await;
    let dir = temp_dir
        .path()
        .join("organized")
        .join("Generic Variant Show");
    write(&dir, "Generic Variant Show - S01E01.mkv");
    write(&dir, "Generic Variant Show - S01E01.jpn.mkv");

    jumbie::scanner::scan_directory(&dir, &state, false)
        .await
        .expect("scan_directory");

    assert!(
        primary_path(&state, &series_id)
            .await
            .is_some_and(|p| p.ends_with("Generic Variant Show - S01E01.mkv")),
        "the file without a language tag must be the primary"
    );
    let linked = linked_paths(&state, &series_id).await;
    assert_eq!(
        linked.len(),
        1,
        "the language variant must attach as linked"
    );
    assert!(linked[0].ends_with("Generic Variant Show - S01E01.jpn.mkv"));
}

#[tokio::test]
async fn import_scanner_attaches_part_language_variant() {
    let (app, state, temp_dir) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Part Variant Scan").await;
    let dir = temp_dir.path().join("organized").join("Part Variant Scan");
    write(&dir, "Part Variant Scan - S01E01 - pt1.mkv");
    write(&dir, "Part Variant Scan - S01E01 - pt1.en.mkv");

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
        "part 1 must be occupied by the plain file, not the variant"
    );
    let linked = linked_paths(&state, &series_id).await;
    assert_eq!(linked.len(), 1, "the part variant must attach as linked");
    assert!(linked[0].ends_with("Part Variant Scan - S01E01 - pt1.en.mkv"));
}

#[tokio::test]
async fn import_scanner_still_treats_version_and_counter_as_conflicts() {
    for (series_title, primary, other) in [
        (
            "Version Scan Show",
            "Version Scan Show - S01E01.mkv",
            "Version Scan Show - S01E01.v2.mkv",
        ),
        (
            "Counter Scan Show",
            "Counter Scan Show - S01E01.mkv",
            "Counter Scan Show - S01E01.001.mkv",
        ),
    ] {
        let (app, state, temp_dir) = common::setup_test_app().await;
        let series_id = common::create_test_series(&app, series_title).await;
        let dir = temp_dir.path().join("organized").join(series_title);
        write(&dir, primary);
        write(&dir, other);

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
            primary_path(&state, &series_id).await.is_none(),
            "{series_title}: a non-language difference must stay a conflict (slot unassigned)"
        );
        assert!(
            linked_paths(&state, &series_id).await.is_empty(),
            "{series_title}: nothing may attach as a variant"
        );
    }
}

#[tokio::test]
async fn import_scanner_assigns_primary_when_it_is_scanned_after_a_variant() {
    // The primary is whichever file carries no language tag, independent of scan order.
    let (app, state, temp_dir) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Ordered Scan Show").await;
    let dir = temp_dir.path().join("organized").join("Ordered Scan Show");
    // Create the variant first so a naive filesystem-order scan sees it first.
    write(&dir, "Ordered Scan Show - S01E01.en.mkv");
    write(&dir, "Ordered Scan Show - S01E01.mkv");

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
        primary_path(&state, &series_id)
            .await
            .is_some_and(|p| p.ends_with("Ordered Scan Show - S01E01.mkv")),
        "the file without a language tag must be the primary"
    );
    assert_eq!(linked_paths(&state, &series_id).await.len(), 1);
}

#[tokio::test]
async fn import_scanner_gives_each_part_its_own_language_slot() {
    let (app, state, temp_dir) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Part Slots Show").await;
    let dir = temp_dir.path().join("organized").join("Part Slots Show");
    for name in [
        "Part Slots Show - S01E01 - pt1.mkv",
        "Part Slots Show - S01E01 - pt1.en.mkv",
        "Part Slots Show - S01E01 - pt2.mkv",
        "Part Slots Show - S01E01 - pt2.en.mkv",
    ] {
        write(&dir, name);
    }

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
    let linked = linked_paths(&state, &series_id).await;
    assert_eq!(linked.len(), 2, "each part keeps its own language slot");
    assert!(linked.iter().any(|p| p.ends_with("pt1.en.mkv")));
    assert!(linked.iter().any(|p| p.ends_with("pt2.en.mkv")));
}

#[tokio::test]
async fn import_scanner_treats_resolution_variants_as_a_conflict() {
    // Resolution is not a language axis: two resolutions of the same tag are different
    // artifacts. The scanner reports a conflict and coexists neither of them.
    let (app, state, temp_dir) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Res Conflict Show").await;
    let dir = temp_dir.path().join("organized").join("Res Conflict Show");
    write(&dir, "Res Conflict Show - S01E01.720p.en.mkv");
    write(&dir, "Res Conflict Show - S01E01.1080p.en.mkv");

    let mapping = state
        .db
        .get_series_mapping(&series_id)
        .await
        .unwrap()
        .unwrap();
    let (_inserted, conflicts) = jumbie::scanner::import_scan_for_series(&dir, &mapping, &state)
        .await
        .expect("import_scan_for_series");

    assert!(!conflicts.is_empty(), "a resolution split must be reported");
    assert!(primary_path(&state, &series_id).await.is_none());
    assert!(linked_paths(&state, &series_id).await.is_empty());
}

#[tokio::test]
async fn import_scanner_attaches_multi_episode_language_variant() {
    // A concatenated range file (S01E01E02) is one artifact on both episodes; its
    // language variant attaches alongside it on every covered episode.
    let (app, state, temp_dir) = common::setup_test_app().await;
    let series_id = common::create_test_series(&app, "Range Scan Show").await;
    let dir = temp_dir.path().join("organized").join("Range Scan Show");
    write(&dir, "Range Scan Show - S01E01E02.mkv");
    write(&dir, "Range Scan Show - S01E01E02.en.mkv");

    let mapping = state
        .db
        .get_series_mapping(&series_id)
        .await
        .unwrap()
        .unwrap();
    jumbie::scanner::scan_series_directory(&dir, &mapping, &state)
        .await
        .expect("scan_series_directory");

    for episode in ["S01E01", "S01E02"] {
        assert!(
            primary_path_for(&state, &series_id, episode)
                .await
                .is_some_and(|p| p.ends_with("Range Scan Show - S01E01E02.mkv")),
            "{episode} must be covered by the range file"
        );
        let linked = linked_paths_for(&state, &series_id, episode).await;
        assert_eq!(linked.len(), 1, "{episode} must hold the variant");
        assert!(linked[0].ends_with("Range Scan Show - S01E01E02.en.mkv"));
    }
}
