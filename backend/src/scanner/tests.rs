use super::*;
use crate::scanner::preview::evaluate_single_directory_preview;
use std::collections::HashMap;

#[test]
fn test_evaluate_single_directory_preview() {
    let temp_dir = tempfile::tempdir().unwrap();
    let sub_dir = temp_dir.path().join("Existing Show");
    std::fs::create_dir_all(&sub_dir).unwrap();

    let file_path = sub_dir.join("Existing Show - S01E01.mkv");
    std::fs::write(&file_path, b"test").unwrap();

    let existing = vec![sub_dir.to_string_lossy().to_string()];

    let result = evaluate_single_directory_preview(&sub_dir, &existing);
    assert!(result.is_some());

    let preview = result.unwrap();
    assert_eq!(preview.episode_count, 1);
    assert_eq!(preview.season_count, 1);
    assert!(preview.already_exists); // should be matched
    assert!(!preview.selected); // shouldn't be selected by default if exists

    let not_existing = vec![];
    let result = evaluate_single_directory_preview(&sub_dir, &not_existing).unwrap();
    assert!(!result.already_exists);
    assert!(result.selected);
}

#[test]
fn test_infer_season_from_path_season_folder() {
    // "Season 1" folder -> Some(1)
    let path = std::path::Path::new("/media/Series Name/Season 1/file.mkv");
    assert_eq!(
        crate::utils::path_utils::infer_season_from_path(path),
        Some(1)
    );

    // "Season 02" folder -> Some(2)
    let path = std::path::Path::new("/media/Series Name/Season 02/file.mkv");
    assert_eq!(
        crate::utils::path_utils::infer_season_from_path(path),
        Some(2)
    );

    // "S01" folder -> Some(1)
    let path = std::path::Path::new("/media/Series Name/S01/file.mkv");
    assert_eq!(
        crate::utils::path_utils::infer_season_from_path(path),
        Some(1)
    );

    // "S12" folder -> Some(12)
    let path = std::path::Path::new("/media/Series Name/S12/file.mkv");
    assert_eq!(
        crate::utils::path_utils::infer_season_from_path(path),
        Some(12)
    );

    // "Season_1" (underscore) folder -> Some(1)
    let path = std::path::Path::new("/media/Series Name/Season_1/file.mkv");
    assert_eq!(
        crate::utils::path_utils::infer_season_from_path(path),
        Some(1)
    );
}

#[test]
fn test_infer_season_from_path_no_match() {
    // No season pattern in any parent -> None
    let path = std::path::Path::new("/media/Series Name/Some Folder/file.mkv");
    assert_eq!(crate::utils::path_utils::infer_season_from_path(path), None);

    // Root level file -> None (no parent dirs with season patterns)
    let path = std::path::Path::new("/media/file.mkv");
    assert_eq!(crate::utils::path_utils::infer_season_from_path(path), None);

    // S01E01 looks like a season pattern but is an episode ID, not a folder
    let path = std::path::Path::new("/media/S01E01.mkv");
    // The parent is /media, which won't match
    assert_eq!(crate::utils::path_utils::infer_season_from_path(path), None);
}

#[test]
fn test_infer_season_from_path_ancestor_walk() {
    // Should walk up ancestors — "Season 2" is a grandparent
    let path = std::path::Path::new("/media/Series Name/Season 2/Extra Subdir/file.mkv");
    assert_eq!(
        crate::utils::path_utils::infer_season_from_path(path),
        Some(2)
    );
}

#[test]
fn test_resolve_season_explicit_in_filename() {
    // File with S01E05 in name -> explicit season 1
    let info = jumbie_shared::mapping::EpisodeInfo {
        raw_title: String::new(),
        series_key: String::new(),
        file_ext: String::new(),
        submitter: None,
        resolution: None,
        version: 1,
        part_number: None,
        is_season_pack: false,
        is_complete_pack: false,
        seasons: vec![1],
        episodes: vec![5],
        has_decimal_episode: false,
    };
    let path = std::path::Path::new("/media/Some Folder/file.mkv");
    let (season_str, was_explicit) = resolve_season_raw(&info, path, None);
    assert_eq!(season_str, "01");
    assert!(was_explicit, "Season from filename should be explicit");

    // Even if there's a "Season 3" folder, explicit in filename wins
    let path = std::path::Path::new("/media/Season 3/file.mkv");
    let (season_str, was_explicit) = resolve_season_raw(&info, path, None);
    assert_eq!(
        season_str, "01",
        "Explicit season in filename should override folder"
    );
    assert!(was_explicit);
}

#[test]
fn test_resolve_season_inferred_from_folder() {
    // E05 in a "Season 2" folder -> inferred season 2
    let info = jumbie_shared::mapping::EpisodeInfo {
        raw_title: String::new(),
        series_key: String::new(),
        file_ext: String::new(),
        submitter: None,
        resolution: None,
        version: 1,
        part_number: None,
        is_season_pack: false,
        is_complete_pack: false,
        seasons: vec![],
        episodes: vec![5],
        has_decimal_episode: false,
    };
    let path = std::path::Path::new("/media/Series Name/Season 2/file.mkv");
    let (season_str, was_explicit) = resolve_season_raw(&info, path, None);
    assert_eq!(season_str, "02");
    assert!(!was_explicit, "Season from folder should NOT be explicit");
}

#[test]
fn test_resolve_season_fallback_to_one() {
    // E05 with no season in filename and no season folder -> fallback to 1
    let info = jumbie_shared::mapping::EpisodeInfo {
        raw_title: String::new(),
        series_key: String::new(),
        file_ext: String::new(),
        submitter: None,
        resolution: None,
        version: 1,
        part_number: None,
        is_season_pack: false,
        is_complete_pack: false,
        seasons: vec![],
        episodes: vec![5],
        has_decimal_episode: false,
    };
    let path = std::path::Path::new("/media/Some Folder/file.mkv");
    let (season_str, was_explicit) = resolve_season_raw(&info, path, None);
    assert_eq!(season_str, "01");
    assert!(!was_explicit, "Fallback season should NOT be explicit");
}

// Season-alias resolution

fn override_with_alias(season: &str, alias: &str) -> jumbie_shared::mapping::SeasonOverride {
    jumbie_shared::mapping::SeasonOverride {
        season: season.to_string(),
        aliases: vec![alias.to_string()],
        ..Default::default()
    }
}

#[test]
fn test_resolve_season_for_series_favors_unique_season_alias() {
    // No season marker in the filename or folder, but season 0 is aliased
    // "Whisker Mini" and the title matches it → season 0, treated as explicit
    // so the normal-mode filter accepts the file.
    let info = jumbie_shared::mapping::EpisodeInfo {
        raw_title: String::new(),
        series_key: "Whisker Mini Anime".to_string(),
        file_ext: String::new(),
        submitter: None,
        resolution: None,
        version: 1,
        part_number: None,
        is_season_pack: false,
        is_complete_pack: false,
        seasons: vec![],
        episodes: vec![1],
        has_decimal_episode: false,
    };
    let mut settings = jumbie_shared::mapping::SeriesSettings::default();
    settings
        .season
        .insert("0".to_string(), override_with_alias("0", "Whisker Mini"));
    let path = std::path::Path::new("/dl/Whisker Mini Anime - 01.mkv");

    match resolve_season_for_series(&info, path, &settings, false) {
        ResolvedSeason::Season(season_str, was_explicit) => {
            assert_eq!(season_str, "00");
            assert!(was_explicit, "season alias should count as explicit");
        }
        ResolvedSeason::Unneeded => panic!("expected a resolved season"),
    }
}

#[test]
fn test_resolve_season_for_series_favors_season_over_series_alias() {
    // The title matches BOTH a series alias and a season alias → the season is
    // favoured (the parsed S03 is ignored).
    let info = jumbie_shared::mapping::EpisodeInfo {
        raw_title: String::new(),
        series_key: "Whisker Mini Anime".to_string(),
        file_ext: String::new(),
        submitter: None,
        resolution: None,
        version: 1,
        part_number: None,
        is_season_pack: false,
        is_complete_pack: false,
        seasons: vec![3],
        episodes: vec![1],
        has_decimal_episode: false,
    };
    let mut settings = jumbie_shared::mapping::SeriesSettings {
        aliases: vec!["Whisker Mini".to_string()],
        ..Default::default()
    };
    settings
        .season
        .insert("0".to_string(), override_with_alias("0", "Whisker Mini"));
    let path = std::path::Path::new("/dl/Whisker Mini Anime S03E01.mkv");

    match resolve_season_for_series(&info, path, &settings, false) {
        ResolvedSeason::Season(season_str, _) => assert_eq!(season_str, "00"),
        ResolvedSeason::Unneeded => panic!("expected the season alias to win"),
    }
}

#[test]
fn test_resolve_season_for_series_conflicting_season_aliases_unneeded() {
    // Two seasons share the same alias and the filename carries no season number
    // → the file is unneeded.
    let info = jumbie_shared::mapping::EpisodeInfo {
        raw_title: String::new(),
        series_key: "Whisker Mini Anime".to_string(),
        file_ext: String::new(),
        submitter: None,
        resolution: None,
        version: 1,
        part_number: None,
        is_season_pack: false,
        is_complete_pack: false,
        seasons: vec![],
        episodes: vec![1],
        has_decimal_episode: false,
    };
    let mut settings = jumbie_shared::mapping::SeriesSettings::default();
    settings
        .season
        .insert("0".to_string(), override_with_alias("0", "Whisker Mini"));
    settings
        .season
        .insert("2".to_string(), override_with_alias("2", "Whisker Mini"));
    let path = std::path::Path::new("/dl/Whisker Mini Anime - 01.mkv");

    assert!(matches!(
        resolve_season_for_series(&info, path, &settings, false),
        ResolvedSeason::Unneeded
    ));
}

#[test]
fn test_resolve_season_for_series_ignores_aliases_in_absolute_mode() {
    // Absolute mode has exactly one season; alias resolution must not apply.
    let info = jumbie_shared::mapping::EpisodeInfo {
        raw_title: String::new(),
        series_key: "Whisker Mini Anime".to_string(),
        file_ext: String::new(),
        submitter: None,
        resolution: None,
        version: 1,
        part_number: None,
        is_season_pack: false,
        is_complete_pack: false,
        seasons: vec![],
        episodes: vec![1],
        has_decimal_episode: false,
    };
    let mut settings = jumbie_shared::mapping::SeriesSettings::default();
    settings
        .season_absolute
        .insert("1".to_string(), override_with_alias("1", "Whisker Mini"));
    let path = std::path::Path::new("/dl/Whisker Mini Anime - 01.mkv");

    match resolve_season_for_series(&info, path, &settings, true) {
        // Falls through to the default season 1, not the (absolute) alias season.
        ResolvedSeason::Season(season_str, _) => assert_eq!(season_str, "01"),
        ResolvedSeason::Unneeded => panic!("absolute mode must not be unneeded"),
    }
}

// Stack Overflow Regression Tests
//
// Verifies that the constrained-stack runtime itself functions correctly
// for simple async operations. The actual stack overflow verification for
// the full nesting chain is in file_manager tests (test_organize_chain_
// does_not_overflow_tiny_stack), which exercises organize_completed →
// organize_file → link_sibling_episodes on a 128KB runtime.
//
// For scan_directory specifically: it requires an Arc<AppState> which needs
// ConfigManager + DbManager + ScanQueue — a complex setup that belongs in
// integration tests. The compile-time safety is guaranteed by the function
// signature returning Pin<Box<dyn Future>>: if someone reverts to `async fn`,
// all call sites break immediately.

#[test]
fn test_constrained_runtime_self_test() {
    // Verify the tiny-stack tokio runtime itself works for simple ops.
    // This prevents regressions in the test infrastructure.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .thread_stack_size(128 * 1024)
        .build()
        .expect("Failed to build runtime with constrained stack");

    rt.block_on(async {
        let tmp = tempfile::tempdir().unwrap();
        let file_path = tmp.path().join("test.mkv");
        tokio::fs::write(&file_path, b"content").await.unwrap();
        let meta = tokio::fs::metadata(&file_path).await.unwrap();
        assert!(meta.len() > 0);
    });
}

// collect_dir_mtimes tests

#[test]
fn test_collect_dir_mtimes_root_only() {
    let dir = tempfile::tempdir().unwrap();
    let mtimes = collect_dir_mtimes(dir.path(), 5);
    assert!(mtimes.contains_key("."), "root should have mtime");
    assert_eq!(mtimes.len(), 1, "only root, no subdirs");
}

#[test]
fn test_collect_dir_mtimes_with_subdirs() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("Season 1")).unwrap();
    std::fs::create_dir(dir.path().join("Season 2")).unwrap();
    std::fs::create_dir(dir.path().join("Specials")).unwrap();

    let mtimes = collect_dir_mtimes(dir.path(), 5);
    assert!(mtimes.contains_key("."));
    assert!(mtimes.contains_key("Season 1"));
    assert!(mtimes.contains_key("Season 2"));
    assert!(mtimes.contains_key("Specials"));
    assert_eq!(mtimes.len(), 4);
}

#[test]
fn test_collect_dir_mtimes_ignores_files() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("Season 1")).unwrap();
    std::fs::write(dir.path().join("video.mkv"), b"data").unwrap();
    std::fs::write(dir.path().join("Season 1/episode.mkv"), b"data").unwrap();

    let mtimes = collect_dir_mtimes(dir.path(), 5);
    // Should contain root + "Season 1", but NOT "video.mkv" or "episode.mkv"
    assert!(mtimes.contains_key("."));
    assert!(mtimes.contains_key("Season 1"));
    assert!(!mtimes.contains_key("video.mkv"));
    assert_eq!(mtimes.len(), 2);
}

#[test]
fn test_collect_dir_mtimes_respects_max_depth() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("Season 1/Episodes/Extra")).unwrap();

    // With depth 1: only root and Season 1
    let mtimes = collect_dir_mtimes(dir.path(), 1);
    assert!(mtimes.contains_key("."));
    assert!(mtimes.contains_key("Season 1"));
    assert!(
        !mtimes.contains_key("Season 1/Episodes"),
        "depth 1 should not reach Season 1/Episodes"
    );

    // With depth 3: root + Season 1 + Episodes + Extra
    let mtimes = collect_dir_mtimes(dir.path(), 3);
    assert!(mtimes.contains_key("Season 1/Episodes"));
    assert!(mtimes.contains_key("Season 1/Episodes/Extra"));
    assert_eq!(mtimes.len(), 4);
}

#[test]
fn test_collect_dir_mtimes_nonexistent_path() {
    let mtimes = collect_dir_mtimes(
        std::path::Path::new("/this/path/definitely/does/not/exist/42abc"),
        5,
    );
    assert!(
        mtimes.is_empty(),
        "nonexistent path should return empty map"
    );
}

// has_any_dir_changed tests

#[test]
fn test_has_any_dir_changed_never_scanned() {
    let current = HashMap::from([(".".to_string(), 1000.0)]);
    assert!(has_any_dir_changed(&current, &HashMap::new()));
}

#[test]
fn test_has_any_dir_changed_unchanged() {
    let m = HashMap::from([(".".to_string(), 1000.0)]);
    assert!(!has_any_dir_changed(&m, &m));
}

#[test]
fn test_has_any_dir_changed_mtime_beyond_fudge() {
    let last = HashMap::from([(".".to_string(), 1000.0)]);
    let current = HashMap::from([(".".to_string(), 1000.15)]);
    assert!(has_any_dir_changed(&current, &last));
}

#[test]
fn test_has_any_dir_changed_mtime_within_fudge() {
    let last = HashMap::from([(".".to_string(), 1000.0)]);
    let current = HashMap::from([(".".to_string(), 1000.05)]);
    assert!(!has_any_dir_changed(&current, &last));
}

#[test]
fn test_has_any_dir_changed_new_directory() {
    let last = HashMap::from([(".".to_string(), 1000.0)]);
    let current = HashMap::from([(".".to_string(), 1000.0), ("Season 2".to_string(), 2000.0)]);
    assert!(has_any_dir_changed(&current, &last));
}

#[test]
fn test_has_any_dir_changed_deleted_directory() {
    let last = HashMap::from([(".".to_string(), 1000.0), ("Season 1".to_string(), 1000.0)]);
    let current = HashMap::from([(".".to_string(), 1000.0)]);
    assert!(has_any_dir_changed(&current, &last));
}

#[test]
fn test_has_any_dir_changed_multiple_dirs_unchanged() {
    let m = HashMap::from([
        (".".to_string(), 1000.0),
        ("Season 1".to_string(), 1000.0),
        ("Season 2".to_string(), 1000.0),
    ]);
    assert!(!has_any_dir_changed(&m, &m));
}

// stable_hash_series_id tests

#[test]
fn test_stable_hash_is_deterministic() {
    let id = "550e8400-e29b-41d4-a716-446655440000";
    let h1 = stable_hash_series_id(id);
    let h2 = stable_hash_series_id(id);
    assert_eq!(h1, h2);
}

#[test]
fn test_stable_hash_different_ids_differ() {
    let h1 = stable_hash_series_id("00000000-0000-0000-0000-000000000001");
    let h2 = stable_hash_series_id("00000000-0000-0000-0000-000000000002");
    assert_ne!(h1, h2, "different IDs should produce different hashes");
}

#[test]
fn test_stable_hash_empty_string() {
    // Edge case: empty series_id should not panic/crash
    let h = stable_hash_series_id("");
    // djb2 of empty string = 5381 (the initial value)
    assert_eq!(h, 5381);
}

#[test]
fn test_stable_hash_bucket_distribution() {
    // Verify that 100 UUID-like IDs distribute across 6 buckets without
    // extreme imbalance (no bucket gets more than 3x the expected share).
    let buckets = 6u64;
    let expected_per_bucket: f64 = 100.0 / buckets as f64;
    let mut counts = [0usize; 6];

    for i in 0..100u64 {
        let id = format!("series-{:020}", i);
        let bucket = stable_hash_series_id(&id) % buckets;
        counts[bucket as usize] += 1;
    }

    for (i, &count) in counts.iter().enumerate() {
        let max_expected = (expected_per_bucket * 3.0).ceil() as usize;
        assert!(
            count <= max_expected,
            "Bucket {} has {} entries, expected at most {}",
            i,
            count,
            max_expected,
        );
    }
}
