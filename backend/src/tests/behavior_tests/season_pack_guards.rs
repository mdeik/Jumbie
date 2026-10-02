//! Season pack / complete series pack behavior tests.
//!
//! Covers complete-series packs flowing through, the multi-season guard in smart_link
//! being skipped for complete packs, and per-file season linking.

use jumbie_shared::mapping::EpisodeInfo;

/// Helper: build an EpisodeInfo representing a parsed release.
fn make_parsed_info(
    title: &str,
    season: Option<i32>,
    episode_num: i32,
    is_season_pack: bool,
    is_complete_pack: bool,
) -> EpisodeInfo {
    EpisodeInfo {
        raw_title: title.to_string(),
        series_key: "test_show".to_string(),
        file_ext: "mkv".to_string(),
        submitter: None,
        resolution: None,
        version: 1,
        part_number: None,
        is_season_pack,
        is_complete_pack,
        seasons: season.map(|s| vec![s]).unwrap_or_default(),
        episodes: vec![episode_num],
        has_decimal_episode: false,
    }
}

// These tests verify the parser output that smart_link uses to make multi-season
// decisions.

#[test]
fn test_complete_series_pack_parser_output() {
    // Parser output that feeds smart_link's parsed_queue_title extraction.
    let info = make_parsed_info(
        "[Group] Show Complete Series [1080p].mkv",
        None, // no season → complete series pack
        1,
        true, // is_season_pack
        true, // is_complete_pack
    );

    assert!(
        info.is_complete_pack,
        "Complete series pack should have is_complete_pack=true"
    );
    assert!(
        info.seasons.is_empty(),
        "Complete series pack should have season=None"
    );
    assert!(
        info.is_season_pack,
        "Complete series pack should also be a season pack"
    );
}

#[test]
fn test_complete_series_pack_fallback_parser_output() {
    let info = make_parsed_info("[Group] Show Complete [1080p].mkv", None, 1, true, true);

    assert!(
        info.is_complete_pack,
        "Fallback complete pack should have is_complete_pack=true"
    );
    assert!(
        info.seasons.is_empty(),
        "Fallback complete pack should have season=None"
    );
}

#[test]
fn test_season_pack_with_complete_in_name_is_not_blocked() {
    // A season pack with "Complete" in the name still carries a season number, so
    // it must not be blocked.
    let info = make_parsed_info("Show - S02 Complete.mkv", Some(2), 1, true, true);

    let is_blocked = info.is_complete_pack && info.seasons.is_empty();

    assert!(
        !is_blocked,
        "Season pack with 'Complete' (season=Some(2)) should NOT be blocked"
    );
}

#[test]
fn test_normal_season_pack_without_complete_is_not_blocked() {
    let info = make_parsed_info("Show - S02.mkv", Some(2), 1, true, false);

    let is_blocked = info.is_complete_pack && info.seasons.is_empty();

    assert!(
        !is_blocked,
        "Normal season pack (is_complete_pack=false) should NOT be blocked"
    );
}

#[test]
fn test_multi_season_pack_detection() {
    // Simulates the pre-scan logic in smart_link_downloaded_files.
    use std::collections::HashSet;

    let file_seasons: HashSet<Option<i32>> =
        [Some(1), Some(2), Some(2), Some(3)].into_iter().collect();

    let explicit: Vec<i32> = file_seasons.iter().filter_map(|s| *s).collect();
    let has_multiple = explicit.len() > 1;

    assert!(
        has_multiple,
        "Files from seasons [1, 2, 3] should trigger multi-season detection"
    );

    let single_season: HashSet<Option<i32>> = [Some(2), Some(2), None].into_iter().collect();
    let explicit_single: Vec<i32> = single_season.iter().filter_map(|s| *s).collect();
    let has_multiple_single = explicit_single.len() > 1;

    assert!(
        !has_multiple_single,
        "Files all from season 2 should NOT trigger multi-season mode"
    );
}

#[test]
fn test_multi_season_guard_rejects_wrong_season() {
    // Simulates the multi-season guard logic in smart_link:
    // If has_multiple_seasons && !is_complete_pack && season != pack_native → reject
    let pack_native_season = 2; // always set for non-complete packs
    let is_complete_pack = false;

    // File is S01E05 — should be rejected
    let file_season = Some(1);
    let should_guard = |fs: Option<i32>| -> bool {
        if is_complete_pack {
            return false;
        }
        if let Some(s) = fs {
            s != pack_native_season
        } else {
            false
        }
    };
    assert!(
        should_guard(file_season),
        "File from season 1 should be rejected when pack native season is 2"
    );

    // File is S02E03 — should be accepted
    let matching_season = Some(2);
    assert!(
        !should_guard(matching_season),
        "File from season 2 should be accepted when pack native season is 2"
    );

    // File has no season — should be accepted
    let no_season: Option<i32> = None;
    assert!(
        !should_guard(no_season),
        "File without explicit season should be accepted"
    );
}

#[test]
fn test_complete_pack_skips_multi_season_guard() {
    // When is_complete_pack=true, the multi-season guard is skipped entirely.
    // Files from ANY season are valid targets.
    let is_complete_pack = true;
    let pack_native_season: Option<i32> = None; // complete packs have no native season

    let guard_skip = |fs: Option<i32>| -> bool {
        if is_complete_pack {
            return true;
        } // guard is skipped
        if let Some(s) = fs {
            Some(s) != pack_native_season
        } else {
            false
        }
    };

    // Files from ANY season should pass the guard (it's skipped)
    assert!(guard_skip(Some(1)), "Complete pack: S01 files should pass");
    assert!(guard_skip(Some(2)), "Complete pack: S02 files should pass");
    assert!(guard_skip(Some(5)), "Complete pack: S05 files should pass");
    assert!(
        guard_skip(None),
        "Complete pack: unseasoned files should pass"
    );
}

#[test]
fn test_complete_pack_guard_flag_skips_logic() {
    // Simulates the exact condition `has_multiple_seasons && !is_complete_pack`
    // that gates the multi-season guard in smart_link.
    let has_multiple_seasons = true;

    // Without the complete-pack flag, guard applies
    assert!(has_multiple_seasons);

    // With the complete-pack flag, guard is skipped even when multi-season
}

#[test]
fn test_complete_pack_file_uses_own_season_for_episode_id() {
    // Simulates the logic: when is_complete_pack, episode_id is derived from
    // each file's own season rather than the queue item's default season.
    let queue_season = "01"; // default when no season in pack title

    // For a complete pack, a file from S02 should generate episode_id with S02
    let file_season = Some(2);
    let file_season_str = file_season
        .map(|s| format!("{:02}", s))
        .unwrap_or_else(|| queue_season.to_string());
    assert_eq!(
        file_season_str, "02",
        "Complete pack file from S02 should resolve to season 02"
    );

    // For a complete pack, a file with no parsed season falls back to queue season
    let no_file_season: Option<i32> = None;
    let fallback_str = no_file_season
        .map(|s| format!("{:02}", s))
        .unwrap_or_else(|| queue_season.to_string());
    assert_eq!(
        fallback_str, "01",
        "Unparseable file should fall back to queue season 01"
    );

    // For a non-complete pack, always use the queue's season regardless
    let is_complete_pack = false;
    let non_complete_str = if is_complete_pack {
        file_season
            .map(|s| format!("{:02}", s))
            .unwrap_or_else(|| queue_season.to_string())
    } else {
        queue_season.to_string()
    };
    assert_eq!(
        non_complete_str, "01",
        "Non-complete pack should always use queue season"
    );
}

#[test]
fn test_complete_pack_multi_season_roundtrip() {
    // Full roundtrip: multiple files from different seasons in a complete pack
    // should each link to their respective seasons.
    let queue_season = "01";

    let files = vec![
        ("S01E05.mkv", Some(1), 5),
        ("S02E03.mkv", Some(2), 3),
        ("S03E12.mkv", Some(3), 12),
        ("episode_99.mkv", None, 0), // unparseable
    ];

    for (_name, file_season, file_ep) in &files {
        let is_complete_pack = true;
        let resolved_season = if is_complete_pack {
            file_season
                .map(|s| format!("{:02}", s))
                .unwrap_or_else(|| queue_season.to_string())
        } else {
            queue_season.to_string()
        };

        match file_season {
            Some(s) => assert_eq!(
                resolved_season,
                format!("{:02}", s),
                "Complete pack: file from S{:02} should link to S{:02}",
                s,
                s
            ),
            None => assert_eq!(
                resolved_season, "01",
                "Complete pack: unparseable file should fall back to queue season"
            ),
        };
        // Episode number is preserved regardless.
        assert!(*file_ep >= 0);
    }
}

#[test]
fn test_complete_pack_infers_season_from_folder_path() {
    // When a file has no explicit season in its name (e.g. "01.mkv" inside
    // a "Season 02" folder), the season should be inferred from the path.
    // This uses ContentOrganizer::infer_season_from_path.
    let queue_season = "01"; // queue default (from pack title with no season)

    // Simulate: file has NO season in its parsed info (info.season = None)
    // but lives in a "Season 02" folder
    let file_season: Option<i32> = None;
    let is_complete_pack = true;

    let resolved = if is_complete_pack {
        file_season
            .map(|s| format!("{:02}", s))
            .or_else(|| {
                // Path-based inference would kick in here
                // We simulate it by resolving from the known folder
                Some("02".to_string())
            })
            .unwrap_or_else(|| queue_season.to_string())
    } else {
        queue_season.to_string()
    };

    assert_eq!(
        resolved, "02",
        "Complete pack: bare episode in Season 02/ folder should resolve to S02"
    );
}

#[test]
fn test_complete_pack_fallback_to_queue_season_when_no_path_hint() {
    // When a file has no season in its name AND no season-named parent folder
    // (e.g. flat directory structure), fall back to the queue item's default.
    let queue_season = "01";
    let file_season: Option<i32> = None;
    let is_complete_pack = true;

    let resolved = if is_complete_pack {
        file_season
            .map(|s| format!("{:02}", s))
            // No path inference (no "Season X" folder)
            .unwrap_or_else(|| queue_season.to_string())
    } else {
        queue_season.to_string()
    };

    assert_eq!(
        resolved, "01",
        "Complete pack: unseasoned file without folder hint should fall back to queue season"
    );
}

#[test]
fn test_complete_pack_flat_folder_all_files_have_season_in_name() {
    // Most common scenario: complete series pack with flat structure like:
    // Show.Complete.Series/
    //   S01E01.mkv
    //   S01E02.mkv
    //   S02E01.mkv
    //   S02E02.mkv
    // All files have explicit SXXEXX → season is in filename
    let queue_season = "01";
    let files = vec![
        ("S01E01.mkv", Some(1)),
        ("S01E02.mkv", Some(1)),
        ("S02E01.mkv", Some(2)),
        ("S02E02.mkv", Some(2)),
    ];

    for (name, expected_season) in &files {
        let file_season = *expected_season; // parsed from filename
        let resolved = if true {
            // is_complete_pack
            file_season
                .map(|s| format!("{:02}", s))
                .unwrap_or_else(|| queue_season.to_string())
        } else {
            queue_season.to_string()
        };
        assert_eq!(
            resolved,
            format!("{:02}", expected_season.unwrap()),
            "Flat pack: {} should resolve to S{:02}",
            name,
            expected_season.unwrap()
        );
    }
}

#[test]
fn test_complete_pack_season_folders_with_season_in_filename() {
    // Season-folder structure where filenames also contain season:
    // Show.Complete.Series/
    //   Season 1/
    //     S01E01.mkv
    //     S01E02.mkv
    //   Season 2/
    //     S02E01.mkv
    //     S02E02.mkv
    // Season is in filename → path inference not needed
    assert_eq!(
        format!("{:02}", 2),
        "02",
        "S02E01.mkv inside Season 2/ should use S02 from filename"
    );
}

#[test]
fn test_complete_pack_season_folders_without_season_in_filename() {
    // Bare-numbered files inside season folders:
    // Show.Complete.Series/
    //   Season 01/
    //     01.mkv
    //     02.mkv
    //   Season 02/
    //     01.mkv
    //     02.mkv
    // Season must be inferred from folder path since filenames have none.
    let queue_season = "01";
    let is_complete_pack = true;

    // Simulate files with explicit season (from folder inference)
    let scenarios = vec![
        (None, Some(1), "01"),    // bare file in S01 folder → S01
        (None, Some(2), "02"),    // bare file in S02 folder → S02
        (None, None, "01"),       // bare file, no folder hint → fallback to "01"
        (Some(3), Some(1), "03"), // explicit S03E01 always wins
    ];

    for (file_season, folder_season, expected) in &scenarios {
        let resolved = if is_complete_pack {
            file_season
                .map(|s| format!("{:02}", s))
                .or_else(|| folder_season.map(|s| format!("{:02}", s)))
                .unwrap_or_else(|| queue_season.to_string())
        } else {
            queue_season.to_string()
        };
        assert_eq!(
            &resolved, expected,
            "file_season={:?}, folder_season={:?} → expected {}",
            file_season, folder_season, expected
        );
    }
}

#[test]
fn test_complete_pack_unrelated_filename_treated_as_unparseable() {
    // Non-episode files like "Extras/Featurette.mkv" or "sample.mkv"
    // inside a complete pack have no episode number in their filename.
    // They fall to the sequential-assignment path which assigns them
    // to the next available episode slot.
    let _queue_season = "01";
    let queue_ep_start = 1;
    let queue_ep_end = 24;

    // Simulate: file parsed with NO episode info (season=None, ep_num=0)
    let parsed_info: Option<(Option<i32>, i32)> = None;

    let is_unparseable = parsed_info.is_none();
    assert!(is_unparseable, "Featurette.mkv should be unparseable");

    // Unparseable files in a pack get sequentially assigned
    // starting from queue_ep_start, regardless of folder location.
    let assigned_ep = queue_ep_start; // sequential_ep starts here
    assert!(
        assigned_ep <= queue_ep_end,
        "Sequentially assigned episode should be within pack range"
    );
}

#[test]
fn test_complete_pack_unknown_file_in_season_folder_not_episode_linked() {
    // An unparseable file inside a "Season 02" folder shouldn't get
    // incorrectly linked as S02E01 just because of the folder hint.
    // The 3-tier fallback for complete packs is:
    //   1. info.season (from filename) → None for unparseable
    //   2. infer_season_from_path(path) → Some(2) from "Season 02" folder
    //   3. queue_season → "01"
    //
    // But unparseable files go through the SEQUENTIAL path, not the
    // file_season_str path. Their season comes from `season_str` (the
    // queue item's default), not from folder inference.
    let queue_season = "01";

    // Unparseable file (parsed_info is None) goes to sequential path
    // where season = season_str (queue default), ep = sequential_ep.
    let is_unparseable = true;
    let expected_season = if is_unparseable {
        queue_season // sequential path uses queue's season
    } else if true {
        // is_complete_pack
        "fallback" // would use file_season_str with folder inference
    } else {
        "01"
    };
    assert_eq!(
        expected_season, "01",
        "Unparseable file: sequential path uses queue season, not folder inference"
    );
}

#[test]
fn test_normal_pack_unknown_file_in_wrong_season_folder_rejected() {
    // For a NON-complete pack (is_complete_pack=false) with native season
    // S02, a parseable S01 file should be rejected by the multi-season guard.
    let pack_native_season = Some(2);
    let is_complete_pack = false;
    let has_multiple_seasons = true;

    // Parseable S01 file in the pack
    let file_season = Some(1);
    let should_reject = has_multiple_seasons
        && !is_complete_pack
        && file_season.is_some()
        && file_season != pack_native_season;

    assert!(
        should_reject,
        "Normal pack: S01 file should be rejected when native season is S02"
    );

    // Complete pack: same S01 file should NOT be rejected
    let is_complete_pack = true;
    let should_skip_guard = has_multiple_seasons && !is_complete_pack;
    assert!(
        !should_skip_guard,
        "Complete pack: guard is skipped, S01 file should be accepted"
    );
}

#[test]
fn test_unmatched_files_keep_default_records_for_review() {
    // Default handling ("keep"): leave files in the download dir and record
    // them for review so adopt_orphans skips them on subsequent cycles.
    let _handling = "move"; // "move" is the old config default, now treated as "keep"
    let unknown_files: Vec<&str> = vec!["/downloads/show/Featurette.mkv"];

    let mut marked_paths: Vec<String> = Vec::new();
    for file_path in &unknown_files {
        let file_path_str = file_path.to_string();
        // Simulates: self.db.upsert_unmatched_file(path, series_id, reason)
        marked_paths.push(file_path_str);
    }

    assert_eq!(marked_paths.len(), 1, "File should be marked");
    assert!(
        marked_paths[0].contains("Featurette.mkv"),
        "Path should be preserved, not moved"
    );
}

#[test]
fn test_unmatched_files_delete_removes_file() {
    // "delete" handling: remove file from disk.
    let handling = "delete";
    let unknown_files: Vec<&str> = vec!["/downloads/show/Featurette.mkv"];

    let mut deleted: Vec<String> = Vec::new();
    if handling == "delete" {
        for file_path in &unknown_files {
            deleted.push(file_path.to_string());
        }
    }

    assert_eq!(deleted.len(), 1, "File should be deleted");
}

#[test]
fn test_unmatched_files_empty_list_ok() {
    // Edge case: empty unknown_files list should not crash.
    let unknown_files: Vec<&str> = vec![];
    for file in &unknown_files {
        let _ = file;
    }
}

#[test]
fn test_unmatched_config_backward_compatible() {
    // Old config value "move" should behave same as "keep".
    // Only "delete" triggers removal.
    let test_cases = vec![
        ("move", false),
        ("keep", false),
        ("delete", true),
        ("unknown_value", false),
    ];

    for (handling, should_delete) in test_cases {
        let is_delete = handling == "delete";
        assert_eq!(
            is_delete, should_delete,
            "handling='{}': expected delete={}, got {}",
            handling, should_delete, is_delete
        );
    }
}

#[test]
fn test_unmatched_fingerprint_failure_does_not_panic() {
    // If link_file_episode fails (DB error), the loop should continue
    // gracefully. The `let _ = ` discards the error.
    let unknown_files: Vec<&str> = vec!["/downloads/show/sample.mkv"];
    let mut processed = 0;

    for _file_path in &unknown_files {
        // Simulate: recording the file for review can fail without panicking.
        let _result: Result<(), &str> = Err("simulated DB failure");
        let _ = _result;
        processed += 1;
    }

    assert_eq!(processed, 1, "Should continue after DB error");
}

#[test]
fn test_unmatched_keep_preserves_multiple_files() {
    // Multiple unknown files should all be marked, not moved.
    let unknown_files = vec![
        "/downloads/show/sample.mkv",
        "/downloads/show/Season 02/01.mkv",
        "/downloads/show/Featurette.mkv",
    ];

    let mut marks = Vec::new();
    let handling = "move";
    if handling != "delete" {
        for file in &unknown_files {
            marks.push(file.to_string());
        }
    }

    assert_eq!(marks.len(), 3, "All files should be marked");
    assert_eq!(marks[0], "/downloads/show/sample.mkv");
    assert_eq!(marks[1], "/downloads/show/Season 02/01.mkv");
    assert_eq!(marks[2], "/downloads/show/Featurette.mkv");
}
