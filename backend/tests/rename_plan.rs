//! Unit tests for `compute_batch_plan`.
//!
//! We use real temp files so the `src.exists()` guard inside the planner passes.

use jumbie::db::{EpisodeDetailRow, EpisodePartRow};
use jumbie::file_manager::{
    AuxiliaryPath, RenamePlanError, compute_batch_plan, compute_batch_plan_with_aux,
};
use jumbie_shared::config::Config;
use jumbie_shared::mapping::MappingRule;
use tempfile::TempDir;

// Helpers
fn test_config(tmp: &TempDir) -> Config {
    let json = serde_json::json!({
        "database": ":memory:",
        "sources": [],
        "downloader": {
            "client_type": "qbittorrent", "host": "localhost", "port": 8080,
            "username": "admin", "password": "password",
            "download_path": tmp.path().join("dl").to_string_lossy().to_string(),
            "enabled": false, "use_series_tags": false,
            "default_category": "Series", "verify_ssl": false,
            "priority": 0, "use_separate_paths": false, "name": "qBit"
        },
        "global_filters": {
            "required": [], "excluded": [], "regex_required": [], "regex_excluded": [],
            "case_sensitive": false, "match_mode": "substring",
            "search_in_description": false, "search_in_files": false
        },
        "series_mappings": {},
        "auth": { "password": null, "banned_ips": [] },
        "organization": {
            "destination_roots": [{ "path": tmp.path().join("organized").to_string_lossy().to_string() }],
            "collision_handling": "rename",
            "season_folder_format": "S${season:02}",
            "episode_file_format": "${series} - S${season:02}E${episode:02} - ${title}",
            "season_folder_format_absolute": "S01",
            "episode_file_format_absolute": "${series} - ${episode:02} - ${title}",
            "rename_episodes": true,
            "auto_apply_renames": false
        }
    });
    serde_json::from_value(json).expect("test config")
}

fn test_mapping(tmp: &TempDir) -> MappingRule {
    let json = serde_json::json!({
        "series_key": "test_show",
        "target_title": "Test Show",
        "season_offsets": {},
        "quality_profile": null,
        "release_profile": null,
        "path": tmp.path().join("organized").join("Test Show").to_string_lossy().to_string(),
        "absolute_numbering": false,
        "rename_episodes": null,
        "season_folder_format": null,
        "episode_file_format": null,
        "season_folder_format_absolute": null,
        "episode_file_format_absolute": null,
        "monitor_mode": null,
        "last_synced_at": null
    });
    serde_json::from_value(json).expect("test mapping")
}

fn episode_row(episode_id: &str, episode: i32, file_path: Option<&str>) -> EpisodeDetailRow {
    EpisodeDetailRow {
        episode_id: episode_id.to_string(),
        season: Some(1),
        episode,
        status: Some("organized".to_string()),
        file_path: file_path.map(|s| s.to_string()),
        release_title: None,
        size: 0,
        title: Some(format!("Episode {}", episode)),
        quality_profile_id: None,
        submitter: None,
        media_info: None,
        quick_hash: None,
        original_path: None,
        created_at: None,
        file_acquired_at: None,
        monitored: true,
        meta_date: None,
        upload_date: None,
        est_date: None,
        metadata_ids: None,
        description: None,
        runtime: None,
        image_url: None,
        download_id: None,
        score: None,
        numbering_mode: 0,
        metadata_source: None,
        download_link: None,
        series_id: String::new(),
        monitor_override: false,
    }
}

fn part_row(episode_id: &str, part_number: i32, file_path: &str) -> (String, EpisodePartRow) {
    (
        episode_id.to_string(),
        EpisodePartRow {
            part_number,
            file_path: file_path.to_string(),
            size: None,
            fingerprint: None,
            media_info: None,
            original_path: None,
        },
    )
}

// Tests
/// An auxiliary sidecar is planned as its own move, targeting the episode's name
/// with the sidecar's language suffix preserved.
#[test]
fn test_auxiliary_sidecar_gets_first_class_move() {
    let tmp = TempDir::new().unwrap();
    let video = tmp.path().join("show_s01e01.mkv");
    let sub = tmp.path().join("show_s01e01.en.srt");
    std::fs::write(&video, b"v").unwrap();
    std::fs::write(&sub, b"s").unwrap();
    let video_str = video.to_string_lossy().to_string();

    let db_episodes = vec![episode_row("ep1", 1, Some(&video_str))];
    let config = test_config(&tmp);
    let mapping = test_mapping(&tmp);
    let aux = vec![AuxiliaryPath {
        path: sub.to_string_lossy().to_string(),
        kind: jumbie_shared::media_format::FileKind::Subtitle,
        episode_id: "ep1".to_string(),
    }];

    let plan = compute_batch_plan_with_aux(
        &config,
        &mapping,
        &db_episodes,
        &[],
        &aux,
        false,
        &std::collections::HashSet::new(),
    )
    .unwrap();

    let aux_move = plan
        .iter()
        .find(|m| m.aux_kind == Some(jumbie_shared::media_format::FileKind::Subtitle))
        .expect("aux sidecar move planned");
    assert_eq!(aux_move.src, sub);
    let dst = aux_move.dst.to_string_lossy();
    assert!(dst.ends_with(".en.srt"), "language suffix preserved: {dst}");
    assert!(dst.contains("S01E01"), "matches episode name: {dst}");
}

/// Two episode rows sharing the same file_path → one PlannedMove, both episode IDs
/// covered, and the destination filename contains "E01-02".
#[test]
fn test_multiepisode_produces_range_filename() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("show_s01e01e02.mkv");
    std::fs::write(&src, b"data").unwrap();
    let src_str = src.to_string_lossy().to_string();

    let db_episodes = vec![
        episode_row("ep1", 1, Some(&src_str)),
        episode_row("ep2", 2, Some(&src_str)),
    ];
    let config = test_config(&tmp);
    let mapping = test_mapping(&tmp);

    let plan = compute_batch_plan(
        &config,
        "test_show",
        &mapping,
        &db_episodes,
        &[],
        false,
        &std::collections::HashSet::new(),
    )
    .expect("plan should succeed");

    assert_eq!(plan.len(), 1, "one physical file → one PlannedMove");

    let m = &plan[0];
    assert_eq!(m.covered_episodes.len(), 2, "both episodes must be covered");

    let ids: Vec<&str> = m
        .covered_episodes
        .iter()
        .map(|e| e.episode_id.as_str())
        .collect();
    assert!(ids.contains(&"ep1") && ids.contains(&"ep2"));

    assert!(m.part_number.is_none());

    let dst_name = m.dst.file_name().unwrap().to_string_lossy();
    // {episode} expands to the range string, "01-02" for episodes 1-2.
    assert!(
        dst_name.contains("01-02"),
        "destination filename should encode the episode range, got: {dst_name}"
    );
}

/// Episode with two part files → two PlannedMoves, each with the correct part_number.
#[test]
fn test_multipart_produces_individual_moves() {
    let tmp = TempDir::new().unwrap();
    let src1 = tmp.path().join("show_s01e03-cd1.mkv");
    let src2 = tmp.path().join("show_s01e03-cd2.mkv");
    std::fs::write(&src1, b"part1").unwrap();
    std::fs::write(&src2, b"part2").unwrap();

    // The parent episode row has no file_path (parts-based episode)
    let db_episodes = vec![episode_row("ep3", 3, None)];
    let parts = vec![
        part_row("ep3", 1, &src1.to_string_lossy()),
        part_row("ep3", 2, &src2.to_string_lossy()),
    ];

    let config = test_config(&tmp);
    let mapping = test_mapping(&tmp);

    let plan = compute_batch_plan(
        &config,
        "test_show",
        &mapping,
        &db_episodes,
        &parts,
        false,
        &std::collections::HashSet::new(),
    )
    .expect("plan should succeed");

    assert_eq!(plan.len(), 2, "two part files → two PlannedMoves");

    let part_nums: Vec<u32> = plan
        .iter()
        .map(|m| m.part_number.expect("must have part_number"))
        .collect();
    assert!(
        part_nums.contains(&1) && part_nums.contains(&2),
        "got: {:?}",
        part_nums
    );

    for m in &plan {
        assert_eq!(m.covered_episodes.len(), 1);
        assert_eq!(m.covered_episodes[0].episode_id, "ep3");
    }
}

/// Two separate episodes whose template resolution collides on the same dst path
/// → compute_batch_plan returns Err(DuplicateTarget).
#[test]
fn test_duplicate_dst_returns_err() {
    let tmp = TempDir::new().unwrap();

    // We need two *different* source files that both compute to the identical target.
    // The simplest way: the template inserts {episode}, but we override the episode_file_format
    // to a constant that ignores {episode}.
    let src1 = tmp.path().join("show_s01e01.mkv");
    let src2 = tmp.path().join("show_s01e02.mkv");
    std::fs::write(&src1, b"ep1").unwrap();
    std::fs::write(&src2, b"ep2").unwrap();

    let db_episodes = vec![
        episode_row("ep1", 1, Some(&src1.to_string_lossy())),
        episode_row("ep2", 2, Some(&src2.to_string_lossy())),
    ];

    let config = test_config(&tmp);

    let mapping_json = serde_json::json!({
        "series_key": "test_show",
        "target_title": "Test Show",
        "season_offsets": {},
        "quality_profile": null,
        "release_profile": null,
        "path": tmp.path().join("organized").join("Test Show").to_string_lossy().to_string(),
        "absolute_numbering": false,
        "rename_episodes": null,
        "season_folder_format": null,
        // Both episodes will produce "S01/Test Show - Same.mkv" regardless of episode number
        "episode_file_format": "Test Show - Same.${ext}",
        "season_folder_format_absolute": null,
        "episode_file_format_absolute": null,
        "monitor_mode": null,
        "last_synced_at": null
    });
    let mapping: MappingRule = serde_json::from_value(mapping_json).expect("mapping");

    let result = compute_batch_plan(
        &config,
        "test_show",
        &mapping,
        &db_episodes,
        &[],
        false,
        &std::collections::HashSet::new(),
    );

    match result {
        Ok(p) => {
            println!("Got Ok with {} items:", p.len());
            for m in p {
                println!("  src: {}", m.src.display());
                println!("  dst: {}", m.dst.display());
            }
            panic!("Expected DuplicateTarget, got Ok(2)");
        }
        Err(e) => {
            assert!(matches!(e, RenamePlanError::DuplicateTarget(_)));
        }
    }
}

/// When a source filename already contains a part indicator (e.g. "-cd1"),
/// the planner should NOT append an additional "-pt1" suffix.
#[test]
fn test_no_double_part_suffix_injection() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("show_s01e04-cd1.mkv");
    std::fs::write(&src, b"data").unwrap();

    let db_episodes = vec![episode_row("ep4", 4, None)];
    let parts = vec![part_row("ep4", 1, &src.to_string_lossy())];

    let config = test_config(&tmp);
    let mapping_json = serde_json::json!({
        "series_key": "test_show",
        "target_title": "Test Show",
        "season_offsets": {},
        "quality_profile": null,
        "release_profile": null,
        "path": tmp.path().join("organized").join("Test Show").to_string_lossy().to_string(),
        "absolute_numbering": false,
        "rename_episodes": null,
        "season_folder_format": null,
        // Provide a template that explicitly outputs a part indicator so we test the skip logic
        "episode_file_format": "${series} - ${title} cd1.${ext}",
        "season_folder_format_absolute": null,
        "episode_file_format_absolute": null,
        "monitor_mode": null,
        "last_synced_at": null
    });
    let mapping: MappingRule = serde_json::from_value(mapping_json).expect("mapping");

    let plan = compute_batch_plan(
        &config,
        "test_show",
        &mapping,
        &db_episodes,
        &parts,
        false,
        &std::collections::HashSet::new(),
    )
    .expect("plan should succeed");

    assert_eq!(plan.len(), 1);
    let dst_name = plan[0]
        .dst
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_lowercase();
    assert!(
        !dst_name.contains("pt1"),
        "suffix '-pt1' should not be injected when filename already has part indicator, got: {dst_name}"
    );
}

/// Multi-episode file in absolute mode → destination filename should contain the
/// absolute episode range (e.g. "04-06"), not just the first episode's absolute number.
#[test]
fn test_absolute_mode_multiepisode_uses_absolute_range() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("oshi_no_ko_e04e05e06.mkv");
    std::fs::write(&src, b"data").unwrap();
    let src_str = src.to_string_lossy().to_string();

    // Include episodes 1-3 with separate files so the absolute map correctly
    // maps episode 4 → absolute 4, episode 5 → absolute 5, episode 6 → absolute 6.
    let other_src = tmp.path().join("other.mkv");
    std::fs::write(&other_src, b"other").unwrap();
    let other_src_str = other_src.to_string_lossy().to_string();

    let db_episodes = vec![
        episode_row("ep1", 1, Some(&other_src_str)),
        episode_row("ep2", 2, Some(&other_src_str)),
        episode_row("ep3", 3, Some(&other_src_str)),
        episode_row("ep4", 4, Some(&src_str)),
        episode_row("ep5", 5, Some(&src_str)),
        episode_row("ep6", 6, Some(&src_str)),
    ];
    let config = test_config(&tmp);
    // Default mapping; absolute numbering is enabled via the target_absolute flag.
    let mapping = test_mapping(&tmp);

    let plan = compute_batch_plan(
        &config,
        "test_show",
        &mapping,
        &db_episodes,
        &[],
        true, // target_absolute = true
        &std::collections::HashSet::new(),
    )
    .expect("plan should succeed");

    assert_eq!(plan.len(), 2, "two physical files → two PlannedMoves");
    let multi_move = plan
        .iter()
        .find(|m| m.covered_episodes.iter().any(|e| e.episode_num == 4))
        .expect("should find move covering episode 4");
    assert_eq!(
        multi_move.covered_episodes.len(),
        3,
        "all three episodes must be covered"
    );

    let dst_name = multi_move.dst.file_name().unwrap().to_string_lossy();
    // Absolute template is "${series} - ${episode} - ${title}"; {episode} is "04-06" here.
    assert!(
        dst_name.contains("04-06"),
        "destination filename should contain the episode range, got: {dst_name}"
    );
    assert!(
        !dst_name.starts_with("Test Show - 04 -"),
        "destination should NOT contain single episode number, got: {dst_name}"
    );
}

/// Multipart files in absolute mode — each part covers a single episode, so
/// {episode} should be the single padded number (not a range).
#[test]
fn test_absolute_mode_multipart_single_absolute() {
    let tmp = TempDir::new().unwrap();

    // Include preceding episodes so absolute numbering starts at 1.
    let other_src = tmp.path().join("ep1.mkv");
    std::fs::write(&other_src, b"other").unwrap();
    let other_src_str = other_src.to_string_lossy().to_string();

    let src1 = tmp.path().join("show_s01e04-cd1.mkv");
    let src2 = tmp.path().join("show_s01e04-cd2.mkv");
    std::fs::write(&src1, b"part1").unwrap();
    std::fs::write(&src2, b"part2").unwrap();

    let db_episodes = vec![
        episode_row("ep1", 1, Some(&other_src_str)),
        episode_row("ep2", 2, None),
        episode_row("ep3", 3, None),
        episode_row("ep4", 4, None),
    ];
    let parts = vec![
        part_row("ep4", 1, &src1.to_string_lossy()),
        part_row("ep4", 2, &src2.to_string_lossy()),
    ];

    let config = test_config(&tmp);
    let mapping = test_mapping(&tmp);

    let plan = compute_batch_plan(
        &config,
        "test_show",
        &mapping,
        &db_episodes,
        &parts,
        true, // target_absolute = true
        &std::collections::HashSet::new(),
    )
    .expect("plan should succeed");

    // 1 move for ep1 + 2 part moves for ep4 → 3 total
    assert_eq!(plan.len(), 3);

    // The two part moves must carry single absolute numbers (e.g. "Test Show - 04 - Episode 4-pt1.mkv"),
    // not a range like "04-??".
    for m in &plan {
        if m.part_number.is_some() {
            let dst_name = m.dst.file_name().unwrap().to_string_lossy();
            assert!(
                dst_name.contains("04"),
                "part move should contain single absolute number, got: {dst_name}"
            );
            assert!(
                !dst_name.contains("-0") || dst_name.contains("-pt") || dst_name.contains("-cd"),
                "part move should NOT contain an absolute range, got: {dst_name}"
            );
        }
    }
}

#[test]
fn test_skip_paths_excludes_files() {
    let tmp = TempDir::new().unwrap();
    let src1 = tmp.path().join("ep1.mkv");
    let src2 = tmp.path().join("ep2.mkv");
    std::fs::write(&src1, b"data1").unwrap();
    std::fs::write(&src2, b"data2").unwrap();

    let src1_str = src1.to_string_lossy().to_string();
    let src2_str = src2.to_string_lossy().to_string();

    let db_episodes = vec![
        episode_row("ep1", 1, Some(&src1_str)),
        episode_row("ep2", 2, Some(&src2_str)),
    ];
    let config = test_config(&tmp);
    let mapping = test_mapping(&tmp);

    let empty_skip: std::collections::HashSet<std::path::PathBuf> =
        std::collections::HashSet::new();
    let plan_all = compute_batch_plan(
        &config,
        "test_show",
        &mapping,
        &db_episodes,
        &[],
        false,
        &empty_skip,
    )
    .expect("plan should succeed");
    assert_eq!(
        plan_all.len(),
        2,
        "without skip_paths, both files should be in the plan"
    );

    let mut skip_set: std::collections::HashSet<std::path::PathBuf> =
        std::collections::HashSet::new();
    skip_set.insert(src2.clone());
    let plan_filtered = compute_batch_plan(
        &config,
        "test_show",
        &mapping,
        &db_episodes,
        &[],
        false,
        &skip_set,
    )
    .expect("plan should succeed");
    assert_eq!(
        plan_filtered.len(),
        1,
        "with skip_paths, only the non-skipped file should be in the plan"
    );
    assert_eq!(
        plan_filtered[0].src, src1,
        "the remaining move should be for ep1 (not skipped)"
    );
}

#[test]
fn test_skip_paths_does_not_affect_parts_independently() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("show_s01e01.mkv");
    let part1 = tmp.path().join("show_s01e01-cd1.mkv");
    let part2 = tmp.path().join("show_s01e01-cd2.mkv");
    std::fs::write(&src, b"main").unwrap();
    std::fs::write(&part1, b"part1").unwrap();
    std::fs::write(&part2, b"part2").unwrap();

    let db_episodes = vec![episode_row("ep1", 1, Some(&src.to_string_lossy()))];
    let parts = vec![
        part_row("ep1", 1, &part1.to_string_lossy()),
        part_row("ep1", 2, &part2.to_string_lossy()),
    ];

    let config = test_config(&tmp);
    let mapping = test_mapping(&tmp);

    // Skip only part2 — it should be excluded while part1 remains.
    let mut skip_set: std::collections::HashSet<std::path::PathBuf> =
        std::collections::HashSet::new();
    skip_set.insert(part2.clone());

    let plan = compute_batch_plan(
        &config,
        "test_show",
        &mapping,
        &db_episodes,
        &parts,
        false,
        &skip_set,
    )
    .expect("plan should succeed");

    // 1 move for the main file + 1 move for part1 (part2 is skipped) = 2 total
    assert_eq!(
        plan.len(),
        2,
        "should include main file and non-skipped part only"
    );

    let part_moves: Vec<&jumbie::file_manager::PlannedMove> =
        plan.iter().filter(|m| m.part_number.is_some()).collect();
    assert_eq!(part_moves.len(), 1);
    assert_eq!(part_moves[0].part_number, Some(1));
}

/// When `rename_episodes` is `false`, the destination path should keep the
/// original filename from the source — only the directory changes.
#[test]
fn test_rename_episodes_disabled_keeps_original_filename() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("My.Original.Release-Group.mkv");
    std::fs::write(&src, b"data").unwrap();
    let src_str = src.to_string_lossy().to_string();

    let db_episodes = vec![episode_row("ep1", 1, Some(&src_str))];
    let config = test_config(&tmp);

    let mut mapping = test_mapping(&tmp);
    mapping.settings.rename_episodes = Some(false);

    let plan = compute_batch_plan(
        &config,
        "test_show",
        &mapping,
        &db_episodes,
        &[],
        false,
        &std::collections::HashSet::new(),
    )
    .expect("plan should succeed");

    assert_eq!(plan.len(), 1, "one file → one planned move");
    let dst_file_name = plan[0]
        .dst
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_string();

    assert_eq!(
        dst_file_name, "My.Original.Release-Group.mkv",
        "should preserve original filename when rename_episodes=false"
    );

    let expected_dir = tmp.path().join("organized").join("Test Show").join("S01");
    assert_eq!(
        plan[0].dst.parent().unwrap(),
        expected_dir,
        "should still organize into the correct directory"
    );
}

/// When `rename_episodes` is `false` with multi-part files, all parts should
/// keep their original filenames but still be organized into the correct directory.
#[test]
fn test_rename_episodes_disabled_multipart_keeps_original_filenames() {
    let tmp = TempDir::new().unwrap();

    let part1 = tmp.path().join("Show.S01E01.Part1.Release-Group.mkv");
    let part2 = tmp.path().join("Show.S01E01.Part2.Release-Group.mkv");
    std::fs::write(&part1, b"part1").unwrap();
    std::fs::write(&part2, b"part2").unwrap();

    let db_episodes = vec![episode_row("ep1", 1, None)];
    let parts = vec![
        part_row("ep1", 1, &part1.to_string_lossy()),
        part_row("ep1", 2, &part2.to_string_lossy()),
    ];

    let config = test_config(&tmp);
    let mut mapping = test_mapping(&tmp);
    mapping.settings.rename_episodes = Some(false);

    let plan = compute_batch_plan(
        &config,
        "test_show",
        &mapping,
        &db_episodes,
        &parts,
        false,
        &std::collections::HashSet::new(),
    )
    .expect("plan should succeed");

    assert_eq!(plan.len(), 2, "two part files → two planned moves");

    for m in &plan {
        let dst_name = m.dst.file_name().unwrap().to_string_lossy().to_string();
        let src_name = m.src.file_name().unwrap().to_string_lossy().to_string();
        assert_eq!(
            dst_name, src_name,
            "each part should keep its original filename when rename_episodes=false"
        );

        let expected_dir = tmp.path().join("organized").join("Test Show").join("S01");
        assert_eq!(
            m.dst.parent().unwrap(),
            expected_dir,
            "should still organize into the correct directory"
        );
    }
}

/// When `rename_episodes` is `true` (or default), the destination filename
/// should use the template — confirming the existing behavior is preserved.
#[test]
fn test_rename_episodes_enabled_uses_template() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("My.Original.Release-Group.mkv");
    std::fs::write(&src, b"data").unwrap();
    let src_str = src.to_string_lossy().to_string();

    let db_episodes = vec![episode_row("ep1", 1, Some(&src_str))];
    let config = test_config(&tmp);

    let mut mapping = test_mapping(&tmp);
    mapping.settings.rename_episodes = Some(true);

    let plan = compute_batch_plan(
        &config,
        "test_show",
        &mapping,
        &db_episodes,
        &[],
        false,
        &std::collections::HashSet::new(),
    )
    .expect("plan should succeed");

    assert_eq!(plan.len(), 1, "one file → one planned move");
    let dst_file_name = plan[0]
        .dst
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_string();

    // Templated filename; the default template pads the episode to 2 digits.
    assert!(
        dst_file_name.starts_with("Test Show - S01E01 - Episode 1"),
        "should use templated filename when rename_episodes=true, got: {dst_file_name}"
    );
}

/// When the global config says `rename_episodes: false` and the per-series
/// override is `None`, the global value should be used.
#[test]
fn test_rename_episodes_inherits_from_global_config() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("Original.Name.mkv");
    std::fs::write(&src, b"data").unwrap();
    let src_str = src.to_string_lossy().to_string();

    let db_episodes = vec![episode_row("ep1", 1, Some(&src_str))];

    let mut config = test_config(&tmp);
    config.organization.rename_episodes = false;

    let mapping = test_mapping(&tmp);
    assert!(
        mapping.settings.rename_episodes.is_none(),
        "mapping should have None to test inheritance"
    );

    let plan = compute_batch_plan(
        &config,
        "test_show",
        &mapping,
        &db_episodes,
        &[],
        false,
        &std::collections::HashSet::new(),
    )
    .expect("plan should succeed");

    assert_eq!(plan.len(), 1);
    let dst_file_name = plan[0]
        .dst
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_string();

    assert_eq!(
        dst_file_name, "Original.Name.mkv",
        "should inherit rename_episodes=false from global config"
    );
}
