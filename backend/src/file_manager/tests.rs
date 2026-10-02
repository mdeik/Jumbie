use super::*;
use crate::db::DbManager;
use crate::file_manager::path::PathBuildVars;
use crate::organizer::ContentOrganizer;
use jumbie_shared::config::Config;
use jumbie_shared::mapping::MappingRule;
use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

/// Helper to construct a PathBuildVars for tests that don't need media info or dates.
/// Episode identity vars for path building.
struct EpVars<'a> {
    season_num: i32,
    episode_num: i32,
    episode_var: &'a str,
}

fn make_vars<'a>(
    config: &'a Config,
    mapping: &'a MappingRule,
    ep: EpVars<'a>,
    source_path: &'a Path,
    episode_title: Option<&'a str>,
    pad: &'a crate::utils::TemplatePadOptions,
) -> PathBuildVars<'a> {
    PathBuildVars {
        config,
        mapping,
        season_num: ep.season_num,
        episode_num: ep.episode_num,
        episode_var: ep.episode_var,
        source_path,
        episode_title,
        episode_quality: None,
        episode_submitter: None,
        release_date: None,
        created_at: None,
        pad_options: pad,
        part_number: None,
        media_info: None,
    }
}

fn build_test_path(config: &Config, mapping: &MappingRule) -> std::path::PathBuf {
    let pad = crate::utils::TemplatePadOptions::default();
    ContentOrganizer::build_target_path(&PathBuildVars {
        config,
        mapping,
        season_num: 1,
        episode_num: 5,
        episode_var: "5",
        source_path: std::path::Path::new("/downloads/test.mkv"),
        episode_title: None,
        episode_quality: None,
        episode_submitter: None,
        release_date: None,
        created_at: None,
        pad_options: &pad,
        part_number: None,
        media_info: None,
    })
    .unwrap()
}

#[test]
fn test_filename_has_part_indicator() {
    assert!(filename_has_part_indicator("video-pt1.mkv"));
    assert!(filename_has_part_indicator("video_part2.mkv"));
    assert!(filename_has_part_indicator("video cd3"));
    assert!(filename_has_part_indicator("video - part 4.mkv"));
    assert!(!filename_has_part_indicator("video-pattern.mkv"));
    assert!(!filename_has_part_indicator("video part.mkv"));
}

#[test]
fn test_build_target_path() {
    let mut config = crate::test_fixtures::default_config();
    config.organization.destination_roots = vec!["/organized".into()];
    config.organization.season_folder_format = "Season ${season:02}".to_string();
    config.organization.episode_file_format = "${series} S${season:02}E${episode:02}".to_string();

    let mapping = MappingRule {
        target_title: "Test Show".to_string(),
        ..Default::default()
    };

    let path = build_test_path(&config, &mapping);
    assert_eq!(
        path.to_string_lossy().replace("\\", "/"),
        "/organized/Test Show/Season 01/Test Show S01E05.mkv"
    );
}

#[test]
fn test_build_target_path_sanitizes_series_folder_from_template() {
    // A stored `${series}` template must substitute the SANITIZED title so the
    // series directory honors the illegal-char policy (default: underscore).
    let mut config = crate::test_fixtures::default_config();
    config.organization.destination_roots = vec!["/organized".into()];
    config.organization.season_folder_format = "Season ${season:02}".to_string();
    config.organization.episode_file_format =
        "${series} S${season:02}E${episode:02}.mkv".to_string();

    let mut mapping = MappingRule {
        target_title: "Show: The Best?".to_string(),
        ..Default::default()
    };
    // Template path — the `${series}` component must be sanitized.
    mapping.settings.path = Some("/organized/${series}".to_string());

    let path = build_test_path(&config, &mapping);
    let rendered = path.to_string_lossy().replace("\\", "/");
    assert!(
        rendered.starts_with("/organized/Show_ The Best_/Season 01/"),
        "Series folder from a template should be sanitized per policy, got: {}",
        rendered
    );
    // The filename embeds ${series} too and must stay sanitized.
    assert!(
        !rendered.contains(':') && !rendered.contains('?'),
        "Path should contain no illegal characters, got: {}",
        rendered
    );
}

#[test]
fn test_build_target_path_sanitizes_series_folder_without_path() {
    // A mapping without an explicit path falls back to {primary_root}/{title};
    // the synthesized folder name must be sanitized per the policy too.
    let mut config = crate::test_fixtures::default_config();
    config.organization.destination_roots = vec!["/organized".into()];
    config.organization.season_folder_format = "Season ${season:02}".to_string();
    config.organization.episode_file_format =
        "${series} S${season:02}E${episode:02}.mkv".to_string();

    let mapping = MappingRule {
        target_title: "Colon: Show".to_string(),
        ..Default::default()
    };

    let path = build_test_path(&config, &mapping);
    let rendered = path.to_string_lossy().replace("\\", "/");
    assert!(
        rendered.starts_with("/organized/Colon_ Show/Season 01/"),
        "Synthesized series folder should be sanitized per policy, got: {}",
        rendered
    );
}

#[test]
fn test_build_target_path_truncates_series_folder_name() {
    let mut config = crate::test_fixtures::default_config();
    config.organization.destination_roots = vec!["/organized".into()];
    config.organization.season_folder_format = "Season ${season:02}".to_string();
    config.organization.episode_file_format =
        "${series} S${season:02}E${episode:02}.mkv".to_string();

    let long_title = "A".repeat(crate::utils::file_naming::MAX_NAME_BYTES + 20);
    let mapping = MappingRule {
        target_title: long_title.clone(),
        ..Default::default()
    };

    let path = build_test_path(&config, &mapping);

    let series_folder = path.parent().and_then(|p| p.parent()).unwrap();
    let folder_name = series_folder.file_name().unwrap().to_str().unwrap();
    assert!(
        folder_name.len() <= crate::utils::file_naming::MAX_NAME_BYTES,
        "Series folder '{}' is {} bytes, max is {}",
        folder_name,
        folder_name.len(),
        crate::utils::file_naming::MAX_NAME_BYTES
    );
}

#[test]
fn test_build_target_path_truncates_season_folder_name() {
    let mut config = crate::test_fixtures::default_config();
    config.organization.destination_roots = vec!["/organized".into()];
    config.organization.season_folder_format = format!(
        "Season ${{season:02}} - {}",
        "X".repeat(crate::utils::file_naming::MAX_NAME_BYTES)
    );
    config.organization.episode_file_format =
        "${series} S${season:02}E${episode:02}.mkv".to_string();

    let mapping = MappingRule {
        target_title: "Show".to_string(),
        ..Default::default()
    };

    let path = build_test_path(&config, &mapping);

    let season_folder = path.parent().unwrap();
    let folder_name = season_folder.file_name().unwrap().to_str().unwrap();
    assert!(
        folder_name.len() <= crate::utils::file_naming::MAX_NAME_BYTES,
        "Season folder '{}' is {} bytes",
        folder_name,
        folder_name.len()
    );
}

#[test]
fn test_build_target_path_truncates_episode_filename() {
    let mut config = crate::test_fixtures::default_config();
    config.organization.destination_roots = vec!["/organized".into()];
    config.organization.season_folder_format = "S${season:02}".to_string();
    config.organization.episode_file_format =
        "${series} - S${season:02}E${episode:02} - ${title}.mkv".to_string();

    let mapping = MappingRule {
        target_title: "Short".to_string(),
        ..Default::default()
    };

    let pad = crate::utils::TemplatePadOptions {
        ..Default::default()
    };

    let long_title = "X".repeat(crate::utils::file_naming::MAX_NAME_BYTES);

    let path = ContentOrganizer::build_target_path_with_episode_var(&make_vars(
        &config,
        &mapping,
        EpVars {
            season_num: 1,
            episode_num: 5,
            episode_var: "05",
        },
        std::path::Path::new("/downloads/test.mkv"),
        Some(&long_title),
        &pad,
    ))
    .unwrap();

    let filename = path.file_name().unwrap().to_str().unwrap();
    assert!(
        filename.len() <= crate::utils::file_naming::MAX_NAME_BYTES,
        "Filename '{}' is {} bytes",
        filename,
        filename.len()
    );
    assert!(
        filename.ends_with(".mkv"),
        "Extension .mkv must be preserved, got: {}",
        filename
    );
}

#[test]
fn test_build_target_path_truncates_all_three_components_together() {
    let mut config = crate::test_fixtures::default_config();
    config.organization.destination_roots = vec!["/organized".into()];
    config.organization.season_folder_format = format!(
        "Season ${{season:02}} - {}",
        "X".repeat(crate::utils::file_naming::MAX_NAME_BYTES)
    );
    config.organization.episode_file_format =
        "${series} S${season:02}E${episode:02} - ${title}.mkv".to_string();

    let long_series = "Y".repeat(crate::utils::file_naming::MAX_NAME_BYTES + 10);
    let long_title = "Z".repeat(crate::utils::file_naming::MAX_NAME_BYTES);
    let mapping = MappingRule {
        target_title: long_series,
        ..Default::default()
    };

    let pad = crate::utils::TemplatePadOptions {
        ..Default::default()
    };

    let path = ContentOrganizer::build_target_path_with_episode_var(&make_vars(
        &config,
        &mapping,
        EpVars {
            season_num: 1,
            episode_num: 5,
            episode_var: "05",
        },
        std::path::Path::new("/downloads/test.mkv"),
        Some(&long_title),
        &pad,
    ))
    .unwrap();

    let series_folder = path.parent().and_then(|p| p.parent()).unwrap();
    let folder_name = series_folder.file_name().unwrap().to_str().unwrap();
    assert!(
        folder_name.len() <= crate::utils::file_naming::MAX_NAME_BYTES,
        "Series folder '{}' is {} bytes",
        folder_name,
        folder_name.len()
    );

    let season_folder = path.parent().unwrap();
    let season_name = season_folder.file_name().unwrap().to_str().unwrap();
    assert!(
        season_name.len() <= crate::utils::file_naming::MAX_NAME_BYTES,
        "Season folder '{}' is {} bytes",
        season_name,
        season_name.len()
    );

    let filename = path.file_name().unwrap().to_str().unwrap();
    assert!(
        filename.len() <= crate::utils::file_naming::MAX_NAME_BYTES,
        "Filename '{}' is {} bytes",
        filename,
        filename.len()
    );
    assert!(
        filename.ends_with(".mkv"),
        "Extension .mkv must be preserved, got: {}",
        filename
    );
}

#[test]
fn test_build_target_path_deterministic_across_calls() {
    let mut config = crate::test_fixtures::default_config();
    config.organization.destination_roots = vec!["/organized".into()];
    config.organization.season_folder_format = "Season ${season:02}".to_string();
    config.organization.episode_file_format =
        "${series} S${season:02}E${episode:02} - ${title}.mkv".to_string();

    let long_series = "A".repeat(crate::utils::file_naming::MAX_NAME_BYTES + 5);
    let long_title = "B".repeat(crate::utils::file_naming::MAX_NAME_BYTES);
    let mapping = MappingRule {
        target_title: long_series.clone(),
        ..Default::default()
    };

    let pad = crate::utils::TemplatePadOptions {
        ..Default::default()
    };

    let path1 = ContentOrganizer::build_target_path_with_episode_var(&make_vars(
        &config,
        &mapping,
        EpVars {
            season_num: 1,
            episode_num: 2,
            episode_var: "02",
        },
        std::path::Path::new("/downloads/test.mkv"),
        Some(&long_title),
        &pad,
    ))
    .unwrap();

    let path2 = ContentOrganizer::build_target_path_with_episode_var(&make_vars(
        &config,
        &mapping,
        EpVars {
            season_num: 1,
            episode_num: 2,
            episode_var: "02",
        },
        std::path::Path::new("/downloads/test.mkv"),
        Some(&long_title),
        &pad,
    ))
    .unwrap();

    assert_eq!(
        path1, path2,
        "Truncated paths must be deterministic: {:?} vs {:?}",
        path1, path2
    );
}

#[test]
fn test_quality_group_stable_across_renames() {
    let mut config = crate::test_fixtures::default_config();
    config.organization.destination_roots = vec!["/organized".into()];
    config.organization.season_folder_format = "S${season:02}".to_string();
    config.organization.episode_file_format =
        "${series} - S${season:02}E${episode:02} - ${quality} - ${group}".to_string();

    let mapping = MappingRule {
        target_title: "Stable Show".to_string(),
        ..Default::default()
    };
    let pad = crate::utils::TemplatePadOptions::default();

    let path1 = ContentOrganizer::build_target_path_with_episode_var(&PathBuildVars {
        config: &config,
        mapping: &mapping,
        season_num: 1,
        episode_num: 1,
        episode_var: "01",
        source_path: std::path::Path::new("/downloads/Some.Show.S01E01.1080p.WEB-DL.mkv"),
        episode_title: Some("First Episode"),
        episode_quality: Some("1080p"),
        episode_submitter: Some("GRP"),
        release_date: None,
        created_at: None,
        pad_options: &pad,
        part_number: None,
        media_info: None,
    })
    .unwrap();
    assert!(
        path1.to_string_lossy().contains("1080p"),
        "First rename should contain quality from DB: {:?}",
        path1,
    );
    assert!(
        path1.to_string_lossy().contains("GRP"),
        "First rename should contain group from DB: {:?}",
        path1,
    );

    let path2 = ContentOrganizer::build_target_path_with_episode_var(&PathBuildVars {
        config: &config,
        mapping: &mapping,
        season_num: 1,
        episode_num: 1,
        episode_var: "01",
        source_path: std::path::Path::new(
            "/organized/Stable Show/S01/Stable Show - S01E01 - 1080p - GRP.mkv",
        ),
        episode_title: Some("First Episode"),
        episode_quality: Some("1080p"),
        episode_submitter: Some("GRP"),
        release_date: None,
        created_at: None,
        pad_options: &pad,
        part_number: None,
        media_info: None,
    })
    .unwrap();
    assert!(
        path2.to_string_lossy().contains("1080p"),
        "Second rename should STILL contain quality from DB: {:?}",
        path2,
    );
    assert!(
        path2.to_string_lossy().contains("GRP"),
        "Second rename should STILL contain group from DB: {:?}",
        path2,
    );

    let name1 = path1.file_name().unwrap().to_string_lossy().to_string();
    let name2 = path2.file_name().unwrap().to_string_lossy().to_string();
    assert_eq!(name1, name2, "Filenames should be identical across renames");
}

#[test]
fn test_date_variables_in_template() {
    let mut config = crate::test_fixtures::default_config();
    config.organization.destination_roots = vec!["/organized".into()];
    config.organization.episode_file_format = "%{release:%Y-%m-%d}".to_string();

    let mapping = MappingRule {
        target_title: "Date Show".to_string(),
        ..Default::default()
    };
    let pad = crate::utils::TemplatePadOptions::default();

    let release_dt =
        chrono::NaiveDateTime::parse_from_str("2024-03-15 10:30:00", "%Y-%m-%d %H:%M:%S").unwrap();

    let path = ContentOrganizer::build_target_path_with_episode_var(&PathBuildVars {
        config: &config,
        mapping: &mapping,
        season_num: 1,
        episode_num: 1,
        episode_var: "01",
        source_path: std::path::Path::new("/downloads/test.mkv"),
        episode_title: None,
        episode_quality: None,
        episode_submitter: None,
        release_date: Some(release_dt),
        created_at: None,
        pad_options: &pad,
        part_number: None,
        media_info: None,
    })
    .unwrap();
    let name = path.file_name().unwrap().to_string_lossy().to_string();
    assert!(
        name.contains("2024-03-15"),
        "Expected formatted release date in filename, got: {}",
        name
    );

    let dl_dt =
        chrono::NaiveDateTime::parse_from_str("2024-06-20 14:30:00", "%Y-%m-%d %H:%M:%S").unwrap();

    config.organization.episode_file_format = "%{download:%Y%m%d_%H%M%S}".to_string();

    let path2 = ContentOrganizer::build_target_path_with_episode_var(&PathBuildVars {
        config: &config,
        mapping: &mapping,
        season_num: 1,
        episode_num: 1,
        episode_var: "01",
        source_path: std::path::Path::new("/downloads/test.mkv"),
        episode_title: None,
        episode_quality: None,
        episode_submitter: None,
        release_date: None,
        created_at: Some(dl_dt),
        pad_options: &pad,
        part_number: None,
        media_info: None,
    })
    .unwrap();
    let name2 = path2.file_name().unwrap().to_string_lossy().to_string();
    // The downloaded datetime is treated as UTC and converted to local time.
    // Compute the expected local-time representation so this test works
    // regardless of the system timezone.
    let local_dt = crate::datetime::UtcDateTime::from_naive_utc(dl_dt)
        .to_local()
        .format("%Y%m%d_%H%M%S")
        .to_string();
    assert!(
        name2.contains(&local_dt),
        "Expected downloaded datetime (local: {}) in filename, got: {}",
        local_dt,
        name2
    );

    config.organization.episode_file_format = "%{release:%Y-%m-%d}".to_string();

    let path3 = ContentOrganizer::build_target_path_with_episode_var(&make_vars(
        &config,
        &mapping,
        EpVars {
            season_num: 1,
            episode_num: 1,
            episode_var: "01",
        },
        std::path::Path::new("/downloads/test.mkv"),
        None,
        &pad,
    ))
    .unwrap();
    let name3 = path3.file_name().unwrap().to_string_lossy().to_string();
    assert_eq!(
        name3, ".mkv",
        "Expected empty release format, got: {}",
        name3
    );
}

struct TestEnv {
    pub tmp: tempfile::TempDir,
    pub db: Arc<DbManager>,
    pub organizer: ContentOrganizer,
}

impl TestEnv {
    async fn new(mapping: &MappingRule) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("test.db");
        let db = Arc::new(DbManager::new(&db_path).await.unwrap());

        db.upsert_series_mapping(&mapping.name, mapping)
            .await
            .unwrap();

        let organizer = crate::tests::organizer_fixtures::make_test_organizer(db.clone()).await;

        Self { tmp, db, organizer }
    }
}

#[tokio::test]
async fn move_files_cycle_safe_handles_chains() {
    // A chain a→b, b→c recycles `b` (a destination and a source). The helper must
    // temp-rename `b` first so `a→b` doesn't clobber it, then move the temp to `c`.
    let mapping = MappingRule {
        target_title: "Cycle Test".into(),
        ..Default::default()
    };
    let env = TestEnv::new(&mapping).await;
    let dir = env.tmp.path().join("moves");
    tokio::fs::create_dir_all(&dir).await.unwrap();

    let a = dir.join("a.mkv");
    let b = dir.join("b.mkv");
    let c = dir.join("c.mkv"); // fresh destination (does not exist yet)
    tokio::fs::write(&a, b"content-a").await.unwrap();
    tokio::fs::write(&b, b"content-b").await.unwrap();

    let moves = vec![(a.clone(), b.clone()), (b.clone(), c.clone())];
    let results = crate::file_manager::move_files_cycle_safe(&env.organizer, &moves).await;

    assert_eq!(results.len(), 2);
    assert!(results[0].is_some(), "a→b must succeed");
    assert!(results[1].is_some(), "b→c must succeed");
    assert!(!a.exists(), "source a must be moved away");
    assert_eq!(tokio::fs::read_to_string(&b).await.unwrap(), "content-a");
    assert_eq!(tokio::fs::read_to_string(&c).await.unwrap(), "content-b");

    // No leftover temp files.
    let leftovers: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with("jumbie_tmp_"))
        .collect();
    assert!(leftovers.is_empty(), "temp files must not remain");
}

/// Helper: constructs a minimal ContentOrganizer for testing (SSoT: shared
/// fixture in `crate::tests::organizer_fixtures::make_test_organizer`).
async fn setup_test_organizer_with_format(
    db: Arc<DbManager>,
    db_path: &std::path::Path,
    dest_root: &std::path::Path,
    episode_file_format: &str,
) -> ContentOrganizer {
    let mut config = crate::test_fixtures::config_with_dest_root(db_path, dest_root);
    config.organization.episode_file_format = episode_file_format.to_string();
    config.organization.auto_apply_renames = true;
    let _ = db.save_organization_config(&config.organization).await;
    crate::tests::organizer_fixtures::make_test_organizer(db).await
}

#[tokio::test]
async fn test_link_sibling_episodes_no_siblings() {
    let mapping = jumbie_shared::types::MappingRule {
        target_title: "Test Series".to_string(),
        name: "test_series".to_string(),
        quality_profile: Some("Any".to_string()),
        release_profile: Some("Any".to_string()),
        ..Default::default()
    };
    let env = TestEnv::new(&mapping).await;

    let src_path = env.tmp.path().join("Show.S01E01.mkv");
    tokio::fs::write(&src_path, b"test content").await.unwrap();

    let meta_ids = std::collections::HashMap::new();
    env.db
        .insert_episode(crate::db::episodes::crud::InsertEpisodeParams {
            file_path: Some(&src_path.to_string_lossy()),
            status: "downloaded",
            ..crate::db::episodes::crud::InsertEpisodeParams::dummy(
                "S01E01",
                "test_series",
                1,
                1,
                &meta_ids,
            )
        })
        .await
        .unwrap();

    let context = crate::file_manager::get_mapping_context(&[], false);

    env.organizer
        .link_sibling_episodes(super::sibling::LinkSiblingParams {
            src: &src_path,
            episode_id: "S01E01",
            mapping: &mapping,
            context: &context,
        })
        .await
        .unwrap();
    assert!(src_path.exists(), "Source file should still exist");
}

#[tokio::test]
async fn test_link_sibling_episodes_creates_hard_links() {
    let mapping = crate::test_fixtures::link_sibling_mapping();
    let env = TestEnv::new(&mapping).await;

    let src_path = env.tmp.path().join("Show.S01.Complete.mkv");
    tokio::fs::write(&src_path, b"season pack content")
        .await
        .unwrap();
    #[cfg(unix)]
    let src_inode = {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata(&src_path).unwrap().ino()
    };

    let meta_ids = std::collections::HashMap::new();
    env.db
        .insert_episode(crate::db::episodes::crud::InsertEpisodeParams {
            file_path: Some(&src_path.to_string_lossy()),
            status: "downloaded",
            ..crate::db::episodes::crud::InsertEpisodeParams::dummy(
                "S01E01",
                "test_series",
                1,
                1,
                &meta_ids,
            )
        })
        .await
        .unwrap();

    env.db
        .insert_episode(crate::db::episodes::crud::InsertEpisodeParams {
            episode_id: "S01E02",
            episode: 2,
            file_path: Some(&src_path.to_string_lossy()),
            status: "downloaded",
            ..crate::db::episodes::crud::InsertEpisodeParams::dummy(
                "S01E02",
                "test_series",
                1,
                2,
                &meta_ids,
            )
        })
        .await
        .unwrap();

    let context = crate::file_manager::get_mapping_context(&[], false);

    env.organizer
        .link_sibling_episodes(super::sibling::LinkSiblingParams {
            src: &src_path,
            episode_id: "S01E01",
            mapping: &mapping,
            context: &context,
        })
        .await
        .unwrap();

    assert!(src_path.exists(), "Source file should still exist");

    let status: Option<String> =
        sqlx::query_scalar("SELECT status FROM episodes WHERE episode_id = ?")
            .bind("S01E02")
            .fetch_one(env.db.get_pool())
            .await
            .unwrap();
    assert_eq!(status, Some("downloaded".to_string()));
    let dest = env
        .db
        .get_episode_file_path("S01E02")
        .await
        .unwrap()
        .expect("Sibling episode should have a file");
    assert!(
        std::path::Path::new(&dest).exists(),
        "Hard link should exist at: {}",
        dest
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let dest_meta = std::fs::metadata(&dest).unwrap();
        assert_eq!(
            dest_meta.ino(),
            src_inode,
            "Destination should be a hard link (same inode as source)"
        );
    }
    #[cfg(not(unix))]
    {
        let dest_meta = std::fs::metadata(&dest).unwrap();
        let src_meta = std::fs::metadata(&src_path).unwrap();
        assert_eq!(
            dest_meta.len(),
            src_meta.len(),
            "Hard-linked file should have the same size as source"
        );
    }

    let primary = env.db.get_episode_file_path("S01E01").await.unwrap();
    assert_eq!(
        primary.unwrap_or_default(),
        src_path.to_string_lossy().to_string()
    );
}

#[tokio::test]
async fn test_link_sibling_episodes_skips_already_organized() {
    let mapping = crate::test_fixtures::link_sibling_mapping();
    let env = TestEnv::new(&mapping).await;

    let src_path = env.tmp.path().join("Show.S01.Complete.mkv");
    tokio::fs::write(&src_path, b"test").await.unwrap();

    let meta_ids = std::collections::HashMap::new();
    env.db
        .insert_episode(crate::db::episodes::crud::InsertEpisodeParams {
            file_path: Some(&src_path.to_string_lossy()),
            status: "downloaded",
            ..crate::db::episodes::crud::InsertEpisodeParams::dummy(
                "S01E01",
                "test_series",
                1,
                1,
                &meta_ids,
            )
        })
        .await
        .unwrap();

    env.db
        .insert_episode(crate::db::episodes::crud::InsertEpisodeParams {
            episode_id: "S01E02",
            episode: 2,
            file_path: Some(&src_path.to_string_lossy()),
            status: "organized",
            ..crate::db::episodes::crud::InsertEpisodeParams::dummy(
                "S01E02",
                "test_series",
                1,
                2,
                &meta_ids,
            )
        })
        .await
        .unwrap();

    let context = crate::file_manager::get_mapping_context(&[], false);

    env.organizer
        .link_sibling_episodes(super::sibling::LinkSiblingParams {
            src: &src_path,
            episode_id: "S01E01",
            mapping: &mapping,
            context: &context,
        })
        .await
        .unwrap();

    let e02_path = env.db.get_episode_file_path("S01E02").await.unwrap();
    assert_eq!(
        e02_path.unwrap_or_default(),
        src_path.to_string_lossy().to_string()
    );
    let e02_status: Option<String> =
        sqlx::query_scalar("SELECT status FROM episodes WHERE episode_id = ?")
            .bind("S01E02")
            .fetch_one(env.db.get_pool())
            .await
            .unwrap();
    assert_eq!(e02_status, Some("organized".to_string()));
}

#[tokio::test]
async fn test_link_sibling_episodes_source_missing() {
    let mapping = jumbie_shared::types::MappingRule {
        target_title: "Test Series".to_string(),
        name: "test_series".to_string(),
        quality_profile: Some("Any".to_string()),
        release_profile: Some("Any".to_string()),
        ..Default::default()
    };
    let env = TestEnv::new(&mapping).await;

    let src_path = env.tmp.path().join("missing.mkv");

    let meta_ids = std::collections::HashMap::new();
    env.db
        .insert_episode(crate::db::episodes::crud::InsertEpisodeParams {
            file_path: Some(&src_path.to_string_lossy()),
            status: "downloaded",
            ..crate::db::episodes::crud::InsertEpisodeParams::dummy(
                "S01E01",
                "test_series",
                1,
                1,
                &meta_ids,
            )
        })
        .await
        .unwrap();

    let context = crate::file_manager::get_mapping_context(&[], false);

    env.organizer
        .link_sibling_episodes(super::sibling::LinkSiblingParams {
            src: &src_path,
            episode_id: "S01E01",
            mapping: &mapping,
            context: &context,
        })
        .await
        .unwrap();
}

// Media Info in Organize Flow

async fn setup_organize_test(
    episode_file_format: &str,
) -> (
    ContentOrganizer,
    std::path::PathBuf,
    Arc<DbManager>,
    tempfile::TempDir,
) {
    let tmp = tempfile::tempdir().unwrap();
    let tmp_path = tmp.path().to_path_buf();

    let src_path = tmp_path.join("My.Show.S01E01.1080p.WEB-DL.mkv");
    tokio::fs::write(&src_path, b"fake video content for testing")
        .await
        .unwrap();

    let db_path = tmp_path.join("test.db");
    let db = Arc::new(DbManager::new(&db_path).await.unwrap());

    let meta_ids = std::collections::HashMap::new();
    db.insert_episode(crate::db::episodes::crud::InsertEpisodeParams {
        file_path: Some(&src_path.to_string_lossy()),
        status: "downloaded",
        ..crate::db::episodes::crud::InsertEpisodeParams::dummy(
            "MyShow_S01E01",
            "My Show",
            1,
            1,
            &meta_ids,
        )
    })
    .await
    .unwrap();

    let mapping = jumbie_shared::types::MappingRule {
        target_title: "My Show".to_string(),
        name: "My Show".to_string(),
        quality_profile: Some("Any".to_string()),
        release_profile: Some("Any".to_string()),
        ..Default::default()
    };
    let dest_root = tmp_path.join("organized");
    db.upsert_series_mapping("My Show", &mapping).await.unwrap();
    let organizer =
        setup_test_organizer_with_format(db.clone(), &db_path, &dest_root, episode_file_format)
            .await;

    (organizer, src_path, db, tmp)
}

#[tokio::test]
async fn test_organize_with_media_info_renders_in_filename() {
    let format_str = "${series} - S${season:02}E${episode:02} - ${codec} - ${resolution}";
    let (organizer, src_path, db, _tmp_path) = setup_organize_test(format_str).await;

    let (_hash, _, _scanned_mi) = db.update_file_fingerprint(&src_path, "organized").await;

    let media_info = jumbie_shared::types::MediaInfo {
        codec: Some("h264".to_string()),
        resolution: Some("1920x1080".to_string()),
        ..Default::default()
    };

    let result = organizer
        .organize_file(&src_path, "MyShow_S01E01", "test_dl", Some(&media_info))
        .await
        .unwrap();

    let final_path = result.expect("organize_file should return Some path");
    let filename = final_path
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_string();

    assert!(
        filename.contains("h264"),
        "Filename should contain codec from MediaInfo: {}",
        filename
    );
    assert!(
        filename.contains("1920x1080"),
        "Filename should contain resolution from MediaInfo: {}",
        filename
    );
    assert!(
        filename.contains("My Show - S01E01"),
        "Filename should contain series/season/episode: {}",
        filename
    );

    assert!(
        !src_path.exists(),
        "Source file should be removed after organize"
    );
    assert!(final_path.exists(), "Organized file should exist at target");
}

#[tokio::test]
async fn test_organize_without_media_info_renders_empty() {
    let format_str =
        "${series} - S${season:02}E${episode:02}${codec:+-${codec}}${resolution:+-${resolution}}";
    let (organizer, src_path, _db, _tmp_path) = setup_organize_test(format_str).await;

    let result = organizer
        .organize_file(&src_path, "MyShow_S01E01", "test_dl", None)
        .await
        .unwrap();
    let final_path = result.expect("organize_file should return Some path");
    let filename = final_path
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_string();

    assert!(
        !filename.contains("h264"),
        "Filename should NOT contain codec when media info absent: {}",
        filename
    );
    assert!(
        !filename.contains("1920x1080"),
        "Filename should NOT contain resolution when media info absent: {}",
        filename
    );
    assert!(
        filename.contains("My Show - S01E01"),
        "Filename should still contain series/season/episode: {}",
        filename
    );

    assert!(final_path.exists(), "Organized file should exist at target");
}

#[cfg_attr(not(ffmpeg_installed), ignore)]
#[tokio::test]
async fn test_fingerprint_media_info_survives_organize() {
    let format_str = "${series} - S${season:02}E${episode:02}";
    let (organizer, src_path, db, _tmp_path) = setup_organize_test(format_str).await;

    let (_hash, _, _scanned_mi) = db.update_file_fingerprint(&src_path, "organized").await;

    let result = organizer
        .organize_file(&src_path, "MyShow_S01E01", "test_dl", None)
        .await
        .unwrap();
    let final_path = result.expect("organize_file should return Some path");
    let final_path_str = final_path.to_string_lossy().to_string();

    crate::state::FileStateManager::fingerprint_file(
        &db,
        &final_path,
        crate::state::FileState::Organized,
    )
    .await
    .ok();

    let media_json: Option<String> = sqlx::query_scalar(
        "SELECT fc.media_info FROM file_paths fp \
             JOIN file_contents fc ON fc.fingerprint = fp.fingerprint \
             WHERE fp.file_path = ?",
    )
    .bind(&final_path_str)
    .fetch_optional(db.get_pool())
    .await
    .unwrap()
    .flatten();

    assert!(
        media_json.is_some(),
        "Fingerprint should be stored at new path after organize"
    );

    assert!(!src_path.exists(), "Source file should be removed");
    assert!(final_path.exists(), "Organized file should exist at target");
}

#[tokio::test]
async fn test_auto_apply_renames_disabled_keeps_original_filename() {
    // When auto_apply_renames is false, organize_file should move the file
    // to the destination directory but keep its original filename — NOT
    // apply the episode file format template.
    let format_str = "${series} - S${season:02}E${episode:02}.mkv";
    let (organizer, src_path, _db, tmp_path) = setup_organize_test(format_str).await;

    {
        let mut org_cfg = organizer
            .db
            .get_organization_config()
            .await
            .unwrap_or_default();
        org_cfg.auto_apply_renames = false;
        let _ = organizer.db.save_organization_config(&org_cfg).await;
    }

    let original_filename = src_path.file_name().unwrap().to_string_lossy().to_string();

    let result = organizer
        .organize_file(&src_path, "MyShow_S01E01", "test_dl", None)
        .await
        .unwrap();

    let final_path = result.expect("organize_file should return Some path");
    let final_filename = final_path
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_string();

    assert!(
        final_filename == original_filename,
        "File should keep its original filename when auto_apply_renames is disabled. \
             Expected: {original_filename}, Got: {final_filename}"
    );
    // The template would produce "My Show - S01E01.mkv"; assert it was NOT applied.
    assert!(
        !final_filename.contains(" - S01E01"),
        "Filename should NOT contain the template format when auto_apply_renames is disabled: {}",
        final_filename
    );

    let dest_root = tmp_path.path().join("organized");
    assert!(
        final_path.starts_with(&dest_root),
        "File should be inside destination root: {:?}",
        final_path
    );
    assert!(!src_path.exists(), "Source file should be removed");
    assert!(final_path.exists(), "Organized file should exist at target");
}

fn assert_file_organized(
    src_path: &std::path::Path,
    final_path: &std::path::Path,
    tmp_path: &tempfile::TempDir,
) {
    let final_filename = final_path
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_string();
    let original_filename = src_path.file_name().unwrap().to_string_lossy().to_string();

    assert!(
        final_filename != original_filename,
        "File should be renamed when auto_apply_renames is enabled. \
         Original: {original_filename}, Got: {final_filename}"
    );
    assert!(
        final_filename.contains("S01E01"),
        "Filename should contain the template format when auto_apply_renames is enabled: {}",
        final_filename
    );

    let dest_root = tmp_path.path().join("organized");
    assert!(
        final_path.starts_with(&dest_root),
        "File should be inside destination root: {:?}",
        final_path
    );
    assert!(!src_path.exists(), "Source file should be removed");
    assert!(final_path.exists(), "Organized file should exist at target");
}

#[tokio::test]
async fn test_auto_apply_renames_enabled_uses_template() {
    // When auto_apply_renames is true, organize_file should rename the file
    // according to the episode file format template.
    let format_str = "${series} - S${season:02}E${episode:02}.mkv";
    let (organizer, src_path, _db, tmp_path) = setup_organize_test(format_str).await;

    let result = organizer
        .organize_file(&src_path, "MyShow_S01E01", "test_dl", None)
        .await
        .unwrap();

    let final_path = result.expect("organize_file should return Some path");
    assert_file_organized(&src_path, &final_path, &tmp_path);
}

#[tokio::test]
async fn test_organize_waits_for_scan_before_move() {
    let format_str = "${series} - S${season:02}E${episode:02}";
    let (organizer, src_path, db, _tmp_path) = setup_organize_test(format_str).await;

    let (_hash, _, scanned_mi) = db.update_file_fingerprint(&src_path, "organized").await;

    let result = organizer
        .organize_file(&src_path, "MyShow_S01E01", "test_dl", scanned_mi.as_ref())
        .await
        .unwrap();

    assert!(result.is_some(), "organize_file should return Some path");
    assert!(!src_path.exists(), "Source file removed after organize");
}

#[tokio::test]
async fn test_assign_auto_apply_renames_disabled_keeps_original_filename() {
    // When auto_apply_renames is false, organize_single_assigned_file should move
    // the file to the destination directory but keep its original filename — NOT
    // apply the episode file format template.
    let format_str = "${series} - S${season:02}E${episode:02}.mkv";
    let (organizer, src_path, _db, tmp_path) = setup_organize_test(format_str).await;

    {
        let mut org_cfg = organizer
            .db
            .get_organization_config()
            .await
            .unwrap_or_default();
        org_cfg.auto_apply_renames = false;
        let _ = organizer.db.save_organization_config(&org_cfg).await;
    }

    let original_filename = src_path.file_name().unwrap().to_string_lossy().to_string();

    let result = organizer
        .organize_single_assigned_file(&src_path, "My Show", "1", 1)
        .await
        .unwrap();

    let final_path = result.expect("organize_single_assigned_file should return Some path");
    let final_filename = final_path
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_string();

    assert!(
        final_filename == original_filename,
        "File should keep its original filename when auto_apply_renames is disabled. \
             Expected: {original_filename}, Got: {final_filename}"
    );
    assert!(
        !final_filename.contains(" - S01E01"),
        "Filename should NOT contain the template format when auto_apply_renames is disabled: {}",
        final_filename
    );

    let dest_root = tmp_path.path().join("organized");
    assert!(
        final_path.starts_with(&dest_root),
        "File should be inside destination root: {:?}",
        final_path
    );
    assert!(!src_path.exists(), "Source file should be removed");
    assert!(final_path.exists(), "Organized file should exist at target");
}

#[tokio::test]
async fn test_assign_auto_apply_renames_enabled_uses_template() {
    // When auto_apply_renames is true, organize_single_assigned_file should rename
    // the file according to the episode file format template.
    let format_str = "${series} - S${season:02}E${episode:02}.mkv";
    let (organizer, src_path, _db, tmp_path) = setup_organize_test(format_str).await;

    let result = organizer
        .organize_single_assigned_file(&src_path, "My Show", "1", 1)
        .await
        .unwrap();

    let final_path = result.expect("organize_single_assigned_file should return Some path");
    assert_file_organized(&src_path, &final_path, &tmp_path);
}

#[tokio::test]
async fn test_organize_single_assigned_file_resolves_episode_title() {
    // When an episode has a title in the DB, {title} in the filename template
    // should resolve to that title — not an empty string.
    // Regression test: callers were passing the series title string as series_id
    // to get_episode_title_by_number instead of the UUID.
    let format_str = "${series} - S${season:02}E${episode:02} - ${title}.mkv";
    let (organizer, src_path, db, _tmp_path) = setup_organize_test(format_str).await;

    sqlx::query("UPDATE episodes SET title = ? WHERE episode_id = ?")
        .bind("Test Episode Title")
        .bind("MyShow_S01E01")
        .execute(db.get_pool())
        .await
        .unwrap();

    let result = organizer
        .organize_single_assigned_file(&src_path, "My Show", "1", 1)
        .await
        .unwrap();

    let final_path = result.expect("organize_single_assigned_file should return Some path");
    let filename = final_path
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_string();

    assert!(
        filename.contains("Test Episode Title"),
        "Filename should contain the episode title: {}",
        filename
    );
    assert!(
        filename.contains("My Show - S01E01"),
        "Filename should contain series/season/episode: {}",
        filename
    );

    assert!(!src_path.exists(), "Source file should be removed");
    assert!(final_path.exists(), "Organized file should exist at target");
}

// maybe_rename_with_media_info Tests
//
// These tests verify that the post-fingerprint rename helper correctly
// renames files when the template requires media-info variables, and
// skips the rename when it doesn‘t.

#[tokio::test]
async fn test_maybe_rename_with_media_info_renames_when_template_has_media_vars() {
    // Template with {width} and {codec} — file should be renamed
    // after maybe_rename_with_media_info provides real media info.
    let format_str = "${series} - S${season:02}E${episode:02} - ${codec} - ${width}x${height}";
    let (organizer, src_path, _db, _tmp_path) = setup_organize_test(format_str).await;

    let result = organizer
        .organize_single_assigned_file(&src_path, "My Show", "1", 1)
        .await
        .unwrap();

    let organized_path = result.expect("organize_single_assigned_file should return Some path");

    // At this point the file was organized with media_info: None,
    // so the filename should have blank media vars.
    let filename_before = organized_path
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_string();
    assert!(
        !filename_before.contains("h264"),
        "Before rename: filename should NOT contain codec: {}",
        filename_before
    );

    let media_info = jumbie_shared::types::MediaInfo {
        codec: Some("h264".to_string()),
        width: 1920,
        height: 1080,
        ..Default::default()
    };

    organizer
        .maybe_rename_with_media_info(&organized_path, "My Show", "1", 1, &media_info)
        .await
        .unwrap();

    assert!(
        !organized_path.exists(),
        "Old path should not exist after rename"
    );

    let parent = organized_path.parent().unwrap();
    let mut entries = tokio::fs::read_dir(parent).await.unwrap();
    let mut renamed = None;
    while let Some(entry) = entries.next_entry().await.unwrap() {
        renamed = Some(entry.path());
    }
    let renamed = renamed.expect("Should have one file in directory");
    let filename_after = renamed.file_name().unwrap().to_string_lossy().to_string();
    assert!(
        filename_after.contains("h264"),
        "After rename: filename should contain codec: {}",
        filename_after
    );
    assert!(
        filename_after.contains("1920x1080") || filename_after.contains("1920"),
        "After rename: filename should contain resolution: {}",
        filename_after
    );
    assert!(
        filename_after.contains("My Show - S01E01"),
        "After rename: filename should retain series/season/episode: {}",
        filename_after
    );
}

#[tokio::test]
async fn test_maybe_rename_with_media_info_skips_when_template_has_no_media_vars() {
    // Simple template with no media-info vars — rename should be a no-op.
    let format_str = "${series} - S${season:02}E${episode:02}.mkv";
    let (organizer, src_path, _db, _tmp_path) = setup_organize_test(format_str).await;

    let result = organizer
        .organize_single_assigned_file(&src_path, "My Show", "1", 1)
        .await
        .unwrap();

    let organized_path = result.expect("organize_single_assigned_file should return Some path");
    let filename_before = organized_path
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_string();

    let media_info = jumbie_shared::types::MediaInfo {
        codec: Some("h264".to_string()),
        ..Default::default()
    };

    organizer
        .maybe_rename_with_media_info(&organized_path, "My Show", "1", 1, &media_info)
        .await
        .unwrap();

    // File should still be at the same path (no rename).
    assert!(
        organized_path.exists(),
        "File should still exist at original path"
    );
    let filename_after = organized_path
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_string();
    assert_eq!(
        filename_before, filename_after,
        "Filename should not change when template has no media vars"
    );
}

#[tokio::test]
async fn test_maybe_rename_with_media_info_skips_when_rename_disabled() {
    // Renaming disabled, template has media vars — rename should be a no-op.
    let format_str = "${series} - ${codec} - ${width}x${height}.mkv";
    let (organizer, src_path, _db, _tmp_path) = setup_organize_test(format_str).await;

    {
        let mut org_cfg = organizer
            .db
            .get_organization_config()
            .await
            .unwrap_or_default();
        org_cfg.auto_apply_renames = false;
        let _ = organizer.db.save_organization_config(&org_cfg).await;
    }

    let result = organizer
        .organize_single_assigned_file(&src_path, "My Show", "1", 1)
        .await
        .unwrap();

    let organized_path = result.expect("organize_single_assigned_file should return Some path");
    let filename_before = organized_path
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_string();

    let media_info = jumbie_shared::types::MediaInfo {
        codec: Some("h264".to_string()),
        width: 1920,
        height: 1080,
        ..Default::default()
    };

    organizer
        .maybe_rename_with_media_info(&organized_path, "My Show", "1", 1, &media_info)
        .await
        .unwrap();

    // File should still be at the same path (no rename).
    assert!(
        organized_path.exists(),
        "File should still exist at original path"
    );
    let filename_after = organized_path
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_string();
    assert_eq!(
        filename_before, filename_after,
        "Filename should not change when renaming is disabled"
    );
}

// Stack-overflow regression tests.
//
// Guards against the organize chain overflowing the tokio worker stack: the chain
// must be built from boxed futures (`Pin<Box<dyn Future>>`), which cap each nesting
// level at a pointer rather than a megabyte-sized inline future type.

#[test]
fn test_organize_chain_does_not_overflow_tiny_stack() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .thread_stack_size(128 * 1024)
        .build()
        .expect("Failed to build runtime with constrained stack");

    rt.block_on(async {
        let tmp = tempfile::tempdir().unwrap();
        let tmp_path = tmp.keep();

        let src_path = tmp_path.join("My.Show.S01E01.1080p.WEB-DL.mkv");
        tokio::fs::write(&src_path, b"fake video content for testing")
            .await
            .unwrap();

        let db_path = tmp_path.join("test.db");
        let db = std::sync::Arc::new(crate::db::DbManager::new(&db_path).await.unwrap());

        sqlx::query(
            "INSERT INTO episodes (episode_id, series_id, season, episode, status)
                 VALUES (?, ?, ?, ?, ?)",
        )
        .bind("MyShow_S01E01")
        .bind("My Show")
        .bind(1)
        .bind(1)
        .bind("downloaded")
        .execute(db.get_pool())
        .await
        .unwrap();
        db.associate_main_file("MyShow_S01E01", src_path.to_string_lossy().as_ref(), None)
            .await
            .unwrap();

        let mapping = jumbie_shared::types::MappingRule {
            target_title: "My Show".to_string(),
            name: "My Show".to_string(),
            quality_profile: Some("Any".to_string()),
            release_profile: Some("Any".to_string()),
            ..Default::default()
        };
        db.upsert_series_mapping("My Show", &mapping).await.unwrap();

        // Fingerprint with state='complete' so organize_completed picks it up.
        let (inode, dev, mtime) =
            crate::platform::file_identity(&tokio::fs::metadata(&src_path).await.unwrap());
        let hash = "test_hash".to_string();
        let _ = db
            .save_fingerprint(crate::db::files::SaveFingerprintParams {
                path: src_path.to_str().unwrap(),
                inode,
                dev,
                size: src_path.metadata().unwrap().len(),
                mtime,
                quick_hash: &hash,
                state: "complete",
                media_info: None::<&jumbie_shared::types::MediaInfo>,
            })
            .await;
        db.link_file_episode(src_path.to_str().unwrap(), "MyShow_S01E01")
            .await
            .unwrap();

        let dest_root = tmp_path.join("organized");
        let organizer = setup_test_organizer_with_format(
            db.clone(),
            &db_path,
            &dest_root,
            "${series} - S${season:02}E${episode:02}.mkv",
        )
        .await;

        organizer
            .organize_completed()
            .await
            .expect("organize_completed should not overflow stack");

        assert!(
            !src_path.exists(),
            "Source file should be removed after organize"
        );
    });
}

#[test]
fn test_resolve_path_vars_resolves_series() {
    let mut vars = HashMap::new();
    vars.insert("series".to_string(), "Test Show".to_string());
    vars.insert("season".to_string(), "1".to_string());

    let resolved = super::path::resolve_path_vars("/media/${series}/S${season}", &vars);
    assert_eq!(resolved, "/media/Test Show/S1");
}

#[test]
fn test_build_target_path_with_custom_path_resolves_known_vars() {
    // Custom paths resolve known variables (e.g. `${series}`, `${season}`).
    // `${series}` is resolved via the vars HashMap.
    let mut config = crate::test_fixtures::default_config();
    config.organization.destination_roots = vec!["/organized".into()];
    config.organization.season_folder_format = "S${season:02}".to_string();
    config.organization.episode_file_format =
        "${series} - S${season:02}E${episode:02}.mkv".to_string();

    let mut mapping = MappingRule {
        target_title: "Test Show".to_string(),
        ..Default::default()
    };
    mapping.settings.path = Some("data/${series}".to_string());

    let pad = crate::utils::TemplatePadOptions {
        ..Default::default()
    };

    let path = ContentOrganizer::build_target_path_with_episode_var(&make_vars(
        &config,
        &mapping,
        EpVars {
            season_num: 1,
            episode_num: 5,
            episode_var: "05",
        },
        std::path::Path::new("/downloads/test.mkv"),
        None,
        &pad,
    ))
    .unwrap();

    let path_str = path.to_string_lossy().replace("\\", "/");
    assert!(
        path_str.contains("data/Test Show/"),
        "Known variable should be resolved: {}",
        path_str
    );
    assert!(
        !path_str.contains("${series}"),
        "${{series}} should be resolved in path: {}",
        path_str
    );
}

// Helper: global-fallback resolution tests

fn make_config_with_globals(flatten: bool, absolute: bool) -> Config {
    let mut cfg = crate::test_fixtures::default_config();
    cfg.general.flatten_season_folders = flatten;
    cfg.general.absolute_numbering = absolute;
    cfg
}

fn make_mapping_with_overrides(flatten: Option<bool>, absolute: Option<bool>) -> MappingRule {
    MappingRule {
        settings: jumbie_shared::mapping::SeriesSettings {
            flatten_season_folders: flatten,
            absolute_numbering: absolute,
            ..Default::default()
        },
        ..Default::default()
    }
}

#[test]
fn test_should_flatten_season_folders_inherits_global() {
    let cfg = make_config_with_globals(true, false);
    let mapping = make_mapping_with_overrides(None, None);

    // None + global true → true
    assert!(should_flatten_season_folders(&cfg, &mapping));

    // None + global false → false
    let cfg_false = make_config_with_globals(false, false);
    assert!(!should_flatten_season_folders(&cfg_false, &mapping));
}

#[test]
fn test_should_flatten_season_folders_override_takes_precedence() {
    let cfg = make_config_with_globals(false, false);

    // Some(true) + global false → true
    let mapping_on = make_mapping_with_overrides(Some(true), None);
    assert!(should_flatten_season_folders(&cfg, &mapping_on));

    // Some(false) + global true → false
    let cfg_true = make_config_with_globals(true, false);
    let mapping_off = make_mapping_with_overrides(Some(false), None);
    assert!(!should_flatten_season_folders(&cfg_true, &mapping_off));
}

#[test]
fn test_should_use_absolute_numbering_inherits_global() {
    let cfg = make_config_with_globals(false, true);
    let mapping = make_mapping_with_overrides(None, None);

    // None + global true → true
    assert!(should_use_absolute_numbering(&cfg, &mapping));

    // None + global false → false
    let cfg_false = make_config_with_globals(false, false);
    assert!(!should_use_absolute_numbering(&cfg_false, &mapping));
}

#[test]
fn test_should_use_absolute_numbering_override_takes_precedence() {
    let cfg = make_config_with_globals(false, false);

    // Some(true) + global false → true
    let mapping_on = make_mapping_with_overrides(None, Some(true));
    assert!(should_use_absolute_numbering(&cfg, &mapping_on));

    // Some(false) + global true → false
    let cfg_true = make_config_with_globals(false, true);
    let mapping_off = make_mapping_with_overrides(None, Some(false));
    assert!(!should_use_absolute_numbering(&cfg_true, &mapping_off));
}

// organize_completed: destination root guard queue cleanup
//
// When a file is already inside a configured destination root, organize_completed
// skips the move but must still remove the queue entry so "Completed" items
// don't linger permanently in the download queue UI.
#[test]
fn test_organize_completed_removes_queue_item_when_in_destination_root() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("Failed to build runtime");

    rt.block_on(async {
            let tmp = tempfile::tempdir().unwrap();
            let tmp_path = tmp.keep();

            let dest_root = tmp_path.join("organized");
            tokio::fs::create_dir_all(&dest_root).await.unwrap();

            // File is INSIDE the destination root
            let file_path = dest_root.join("My.Show.S01E01.mkv");
            tokio::fs::write(&file_path, b"already organized content")
                .await
                .unwrap();

            let db_path = tmp_path.join("test.db");
            let db = std::sync::Arc::new(
                crate::db::DbManager::new(&db_path).await.unwrap(),
            );

            let episode_id = "MyShow_S01E01";

            sqlx::query(
                "INSERT INTO episodes (episode_id, series_id, season, episode, status)
                 VALUES (?, ?, ?, ?, ?)",
            )
            .bind(episode_id)
            .bind("My Show")
            .bind(1)
            .bind(1)
            .bind("downloaded")
            .execute(db.get_pool())
            .await
            .unwrap();
            db.associate_main_file(episode_id, file_path.to_string_lossy().as_ref(), None)
                .await
                .unwrap();

            let mapping = jumbie_shared::types::MappingRule {
                target_title: "My Show".to_string(),
                name: "My Show".to_string(),
                quality_profile: Some("Any".to_string()),
                release_profile: Some("Any".to_string()),
                ..Default::default()
            };
            db.upsert_series_mapping("My Show", &mapping).await.unwrap();

            let (inode, dev, mtime) =
                crate::platform::file_identity(&tokio::fs::metadata(&file_path).await.unwrap());
            let hash = "test_hash_dest_root".to_string();
            db.save_fingerprint(crate::db::files::SaveFingerprintParams {
                    path: file_path.to_str().unwrap(),
                    inode,
                    dev,
                    size: file_path.metadata().unwrap().len(),
                    mtime,
                    quick_hash: &hash,
                    state: "complete",
                    media_info: None::<&jumbie_shared::types::MediaInfo>,
                })
                .await
                .unwrap();

            db.link_file_episode(file_path.to_str().unwrap(), episode_id)
                .await
                .unwrap();

            sqlx::query(
                "INSERT INTO download_queue (media_name, media_link, series_title, season, episode, episode_id, score, is_user_requested, status)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind("My Show S01E01")
            .bind("magnet:?xt=urn:btih:test_dest_root")
            .bind("My Show")
            .bind("1")
            .bind(1)
            .bind(episode_id)
            .bind(100)
            .bind(false)
            .bind("Completed")
            .execute(db.get_pool())
            .await
            .unwrap();

            let organizer = setup_test_organizer_with_format(db.clone(), &db_path, &dest_root, "${series} - S${season:02}E${episode:02}.mkv").await;

            organizer
                .organize_completed()
                .await
                .expect("organize_completed should succeed");

            // The queue item should have been removed
            let remaining: Option<(i64,)> = sqlx::query_as(
                "SELECT id FROM download_queue WHERE episode_id = ?",
            )
            .bind(episode_id)
            .fetch_optional(db.get_pool())
            .await
            .unwrap();

            assert!(
                remaining.is_none(),
                "Queue item should be removed when file is already in destination root"
            );

            // The source file should still exist (it was NOT moved)
            assert!(
                file_path.exists(),
                "File inside destination root should not be moved"
            );
        });
}

// organize_completed: scanner-claimed episode cleans up orphan
//
// When the scanner picks up a file for the same episode_id between download
// completion and organize_completed (setting status = 'organized' with a
// different file_path), organize_completed should clean up the stale
// fingerprint and queue item rather than leaving it orphaned.
#[test]
fn test_organize_completed_cleans_up_when_scanner_claimed_episode() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("Failed to build runtime");

    rt.block_on(async {
            let tmp = tempfile::tempdir().unwrap();
            let tmp_path = tmp.keep();

            let dest_root = tmp_path.join("organized");
            tokio::fs::create_dir_all(&dest_root).await.unwrap();

            // The download file (fingerprint target) — outside destination root
            let download_path = tmp_path.join("downloads").join("Show.S01E01.mkv");
            tokio::fs::create_dir_all(download_path.parent().unwrap())
                .await
                .unwrap();
            tokio::fs::write(&download_path, b"downloaded content")
                .await
                .unwrap();

            // The scanner-discovered file (already in the series dir)
            let series_path = dest_root.join("Show").join("S01").join("Show.S01E01.mkv");
            tokio::fs::create_dir_all(series_path.parent().unwrap())
                .await
                .unwrap();
            tokio::fs::write(&series_path, b"scanner-discovered content")
                .await
                .unwrap();

            let db_path = tmp_path.join("test.db");
            let db = std::sync::Arc::new(
                crate::db::DbManager::new(&db_path).await.unwrap(),
            );

            let episode_id = "Show_S01E01";
            let series_id = "Test Show";

            // Series mapping
            let mapping = jumbie_shared::types::MappingRule {
                target_title: "Test Show".to_string(),
                name: "Test Show".to_string(),
                quality_profile: Some("Any".to_string()),
                release_profile: Some("Any".to_string()),
                ..Default::default()
            };
            db.upsert_series_mapping(series_id, &mapping).await.unwrap();

            // Episode is already 'organized' by the scanner, file_path = series_path
            sqlx::query(
                "INSERT INTO episodes (episode_id, series_id, season, episode, status)
                 VALUES (?, ?, ?, ?, ?)",
            )
            .bind(episode_id)
            .bind(series_id)
            .bind(1)
            .bind(1)
            .bind("organized")
            .execute(db.get_pool())
            .await
            .unwrap();
            db.associate_main_file(episode_id, series_path.to_string_lossy().as_ref(), None)
                .await
                .unwrap();

            // Stale fingerprint at the download path (state = 'complete')
            let (inode, dev, mtime) = crate::platform::file_identity(
                &tokio::fs::metadata(&download_path).await.unwrap(),
            );
            let hash = "test_hash_scanner_race".to_string();
            db.save_fingerprint(crate::db::files::SaveFingerprintParams {
                    path: download_path.to_str().unwrap(),
                    inode,
                    dev,
                    size: download_path.metadata().unwrap().len(),
                    mtime,
                    quick_hash: &hash,
                    state: "complete",
                    media_info: None::<&jumbie_shared::types::MediaInfo>,
                })
                .await
                .unwrap();

            db.link_file_episode(download_path.to_str().unwrap(), episode_id)
                .await
                .unwrap();

            // Queue item with status 'Completed'
            sqlx::query(
                "INSERT INTO download_queue (media_name, media_link, series_title, season, episode, episode_id, score, is_user_requested, status)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind("Test Show S01E01")
            .bind("magnet:?xt=urn:btih:test_scanner_race")
            .bind("Test Show")
            .bind("1")
            .bind(1)
            .bind(episode_id)
            .bind(100)
            .bind(false)
            .bind("Completed")
            .execute(db.get_pool())
            .await
            .unwrap();

            let organizer = setup_test_organizer_with_format(db.clone(), &db_path, &dest_root, "${series} - S${season:02}E${episode:02}.mkv").await;

            organizer
                .organize_completed()
                .await
                .expect("organize_completed should succeed");

            // The stale fingerprint should be gone
            let fingerprint_still_exists: Option<(String,)> = sqlx::query_as(
                "SELECT state FROM file_paths WHERE file_path = ?",
            )
            .bind(download_path.to_string_lossy().to_string())
            .fetch_optional(db.get_pool())
            .await
            .unwrap();

            assert!(
                fingerprint_still_exists.is_none(),
                "Stale fingerprint at download path should be removed"
            );

            // The queue item should be removed
            let queue_remaining: Option<(i64,)> = sqlx::query_as(
                "SELECT id FROM download_queue WHERE episode_id = ?",
            )
            .bind(episode_id)
            .fetch_optional(db.get_pool())
            .await
            .unwrap();

            assert!(
                queue_remaining.is_none(),
                "Queue item should be removed when scanner claimed the episode"
            );

            // The download file should be DELETED (no longer needs to exist —
            // the scanner's file takes precedence)
            assert!(
                !download_path.exists(),
                "Orphaned download file should be deleted from disk"
            );

            // The series file should still exist (untouched)
            assert!(
                series_path.exists(),
                "Scanner-discovered file should remain untouched"
            );
        });
}

// organize_completed: scanner-claimed episode with upgrades enabled
//
// When upgrades are enabled AND the download has a score > 0, the download
// should REPLACE the scanner-discovered file (not just clean up).
#[test]
fn test_organize_completed_upgrades_when_scanner_claimed_with_score() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("Failed to build runtime");

    rt.block_on(async {
            let tmp = tempfile::tempdir().unwrap();
            let tmp_path = tmp.keep();

            let dest_root = tmp_path.join("organized");
            tokio::fs::create_dir_all(&dest_root).await.unwrap();

            // The download file — outside destination root
            let download_path = tmp_path.join("downloads").join("Show.S01E01.mkv");
            tokio::fs::create_dir_all(download_path.parent().unwrap())
                .await
                .unwrap();
            tokio::fs::write(&download_path, b"superior download content")
                .await
                .unwrap();

            // The scanner-discovered file (in series dir)
            let series_path = dest_root.join("Show").join("S01").join("Show.S01E01.mkv");
            tokio::fs::create_dir_all(series_path.parent().unwrap())
                .await
                .unwrap();
            tokio::fs::write(&series_path, b"inferior scanner content")
                .await
                .unwrap();

            let db_path = tmp_path.join("test.db");
            let db = std::sync::Arc::new(
                crate::db::DbManager::new(&db_path).await.unwrap(),
            );

            let episode_id = "Show_S01E01";
            let series_id = "Test Show";

            let q_profile = jumbie_shared::scoring::QualityProfile {
                name: "Any".to_string(),
                qualities: vec![],
                upgrade_only_qualities: vec![],
            };
            db.upsert_quality_profile("Any", &q_profile).await.unwrap();

            // Series mapping with upgrades enabled (global false, but series overrides)
            let mapping = jumbie_shared::types::MappingRule {
                target_title: "Test Show".to_string(),
                name: "Test Show".to_string(),
                quality_profile: Some("Any".to_string()),
                release_profile: Some("Any".to_string()),
                settings: jumbie_shared::mapping::SeriesSettings {
                    ..Default::default()
                },
                ..Default::default()
            };
            db.upsert_series_mapping(series_id, &mapping).await.unwrap();

            // Episode is 'organized' by scanner
            sqlx::query(
                "INSERT INTO episodes (episode_id, series_id, season, episode, status)
                 VALUES (?, ?, ?, ?, ?)",
            )
            .bind(episode_id)
            .bind(series_id)
            .bind(1)
            .bind(1)
            .bind("organized")
            .execute(db.get_pool())
            .await
            .unwrap();
            db.associate_main_file(episode_id, series_path.to_string_lossy().as_ref(), None)
                .await
                .unwrap();

            // Stale fingerprint at the download path (state = 'complete')
            let (inode, dev, mtime) = crate::platform::file_identity(
                &tokio::fs::metadata(&download_path).await.unwrap(),
            );
            let hash = "test_hash_upgrade".to_string();
            db.save_fingerprint(crate::db::files::SaveFingerprintParams {
                    path: download_path.to_str().unwrap(),
                    inode,
                    dev,
                    size: download_path.metadata().unwrap().len(),
                    mtime,
                    quick_hash: &hash,
                    state: "complete",
                    media_info: None::<&jumbie_shared::types::MediaInfo>,
                })
                .await
                .unwrap();

            db.link_file_episode(download_path.to_str().unwrap(), episode_id)
                .await
                .unwrap();

            // Queue item with episode_intentions (intention-based upgrade path)
            sqlx::query(
                "INSERT INTO download_queue (media_name, media_link, series_title, season, episode, episode_id, score, is_user_requested, status, episode_intentions)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind("Test Show S01E01")
            .bind("magnet:?xt=urn:btih:test_upgrade")
            .bind("Test Show")
            .bind("1")
            .bind(1)
            .bind(episode_id)
            .bind(85)
            .bind(false)
            .bind("Completed")
            .bind(r#"[{"episode_num":1,"source_episode_num":1,"episode_id":"Show_S01E01","score":85,"keep":true}]"#)
            .execute(db.get_pool())
            .await
            .unwrap();

            let organizer = setup_test_organizer_with_format(db.clone(), &db_path, &dest_root, "${series} - S${season:02}E${episode:02}.mkv").await;

            organizer
                .organize_completed()
                .await
                .expect("organize_completed should succeed");

            // The download file should have been MOVED to the series path
            // (overwriting the scanner-discovered file)
            assert!(
                !download_path.exists(),
                "Download file should be moved (no longer at download path)"
            );
            assert!(
                series_path.exists(),
                "Series path should now contain the upgraded file"
            );
            let series_content = tokio::fs::read_to_string(&series_path).await.unwrap();
            assert_eq!(
                series_content, "superior download content",
                "Series file should contain the download's content (upgrade)"
            );

            // The stale fingerprint should be gone
            let fingerprint: Option<(String,)> = sqlx::query_as(
                "SELECT state FROM file_paths WHERE file_path = ?",
            )
            .bind(download_path.to_string_lossy().to_string())
            .fetch_optional(db.get_pool())
            .await
            .unwrap();
            assert!(
                fingerprint.is_none(),
                "Stale fingerprint at download path should be removed"
            );

            // New fingerprint should exist at series path with state 'organized'
            let new_fp: Option<(String,)> = sqlx::query_as(
                "SELECT state FROM file_paths WHERE file_path = ?",
            )
            .bind(series_path.to_string_lossy().to_string())
            .fetch_optional(db.get_pool())
            .await
            .unwrap();
            assert!(
                new_fp.is_some(),
                "Fingerprint should exist at series path after upgrade"
            );

            // The queue item should be removed
            let queue_remaining: Option<(i64,)> = sqlx::query_as(
                "SELECT id FROM download_queue WHERE episode_id = ?",
            )
            .bind(episode_id)
            .fetch_optional(db.get_pool())
            .await
            .unwrap();
            assert!(
                queue_remaining.is_none(),
                "Queue item should be removed after upgrade"
            );

            // Episode status should still be 'organized'
            let ep_status: Option<(String,)> = sqlx::query_as(
                "SELECT status FROM episodes WHERE episode_id = ?",
            )
            .bind(episode_id)
            .fetch_optional(db.get_pool())
            .await
            .unwrap();
            assert_eq!(
                ep_status.map(|s| s.0).as_deref(),
                Some("organized"),
                "Episode status should remain 'organized' after upgrade"
            );
        });
}

// organize_completed: download upgrade bypasses the collision strategy
//
// An upgrade is a deliberate replacement: it always overwrites the episode's
// existing canonical file, regardless of `collision_handling`. The superseded file
// is removed (never kept as a suffixed orphan) and the upgrade takes the canonical
// path.
#[test]
fn test_organize_completed_upgrade_ignores_collision_strategy() {
    for mode in ["rename", "skip", "overwrite"] {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("Failed to build runtime");

        rt.block_on(async move {
            let tmp = tempfile::tempdir().unwrap();
            let tmp_path = tmp.keep();
            let dest_root = tmp_path.join("organized");
            tokio::fs::create_dir_all(&dest_root).await.unwrap();

            let download_path = tmp_path.join("downloads").join("Show.S01E01.mkv");
            tokio::fs::create_dir_all(download_path.parent().unwrap())
                .await
                .unwrap();
            tokio::fs::write(&download_path, b"superior download content")
                .await
                .unwrap();

            let series_dir = dest_root.join("Show").join("S01");
            tokio::fs::create_dir_all(&series_dir).await.unwrap();
            let series_path = series_dir.join("Show.S01E01.mkv");
            tokio::fs::write(&series_path, b"inferior scanner content")
                .await
                .unwrap();

            let db_path = tmp_path.join("test.db");
            let db = std::sync::Arc::new(crate::db::DbManager::new(&db_path).await.unwrap());

            let episode_id = "Show_S01E01";
            let series_id = "Test Show";

            let mapping = jumbie_shared::types::MappingRule {
                target_title: "Test Show".to_string(),
                name: "Test Show".to_string(),
                quality_profile: Some("Any".to_string()),
                release_profile: Some("Any".to_string()),
                ..Default::default()
            };
            db.upsert_series_mapping(series_id, &mapping).await.unwrap();

            sqlx::query(
                "INSERT INTO episodes (episode_id, series_id, season, episode, status)
                 VALUES (?, ?, ?, ?, ?)",
            )
            .bind(episode_id)
            .bind(series_id)
            .bind(1)
            .bind(1)
            .bind("organized")
            .execute(db.get_pool())
            .await
            .unwrap();
            db.associate_main_file(episode_id, series_path.to_string_lossy().as_ref(), None)
                .await
                .unwrap();

            let (inode, dev, mtime) = crate::platform::file_identity(
                &tokio::fs::metadata(&download_path).await.unwrap(),
            );
            db.save_fingerprint(crate::db::files::SaveFingerprintParams {
                path: download_path.to_str().unwrap(),
                inode,
                dev,
                size: download_path.metadata().unwrap().len(),
                mtime,
                quick_hash: "test_hash_upgrade_ignores_strategy",
                state: "complete",
                media_info: None::<&jumbie_shared::types::MediaInfo>,
            })
            .await
            .unwrap();
            db.link_file_episode(download_path.to_str().unwrap(), episode_id)
                .await
                .unwrap();

            // A manual download always upgrades regardless of score.
            sqlx::query(
                "INSERT INTO download_queue (media_name, media_link, series_title, season, episode, episode_id, score, is_user_requested, is_manual, status)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind("Test Show S01E01")
            .bind("magnet:?xt=urn:btih:test_upgrade_ignores_strategy")
            .bind("Test Show")
            .bind("1")
            .bind(1)
            .bind(episode_id)
            .bind(100)
            .bind(false)
            .bind(true)
            .bind("Completed")
            .execute(db.get_pool())
            .await
            .unwrap();

            let organizer = setup_test_organizer_with_format(
                db.clone(),
                &db_path,
                &dest_root,
                "${series} - S${season:02}E${episode:02}.mkv",
            )
            .await;

            let mut org = db.get_organization_config().await.unwrap_or_default();
            org.collision_handling = mode.to_string();
            db.save_organization_config(&org).await.unwrap();

            organizer
                .organize_completed()
                .await
                .expect("organize_completed should succeed");

            assert_eq!(
                tokio::fs::read_to_string(&series_path).await.unwrap(),
                "superior download content",
                "{mode}: the upgrade overwrites the canonical path"
            );
            assert!(
                !download_path.exists(),
                "{mode}: the download is moved out of the downloads dir"
            );
            assert!(
                !series_dir.join("Show.S01E01.001.mkv").exists(),
                "{mode}: the superseded file is not kept as an orphan"
            );
            assert_eq!(
                db.get_episode_file_path(episode_id).await.unwrap().as_deref(),
                Some(series_path.to_string_lossy().as_ref()),
                "{mode}: the episode still points at the canonical path"
            );
        });
    }
}

// organize_completed: scanner-claimed episode with upgrades disabled
//
// Even with a score > 0, if upgrades are disabled the download should be
// cleaned up (not replace the scanner file).
#[test]
fn test_organize_completed_upgrades_nonzero_score_always_upgrades() {
    // Upgrades are always enabled for monitored episodes. A download with
    // a non-zero score (85) replaces the existing organized file.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("Failed to build runtime");

    rt.block_on(async {
            let tmp = tempfile::tempdir().unwrap();
            let tmp_path = tmp.keep();

            let dest_root = tmp_path.join("organized");
            tokio::fs::create_dir_all(&dest_root).await.unwrap();

            let download_path = tmp_path.join("downloads").join("Show.S01E01.mkv");
            tokio::fs::create_dir_all(download_path.parent().unwrap())
                .await
                .unwrap();
            tokio::fs::write(&download_path, b"download content")
                .await
                .unwrap();

            let series_path = dest_root.join("Show").join("S01").join("Show.S01E01.mkv");
            tokio::fs::create_dir_all(series_path.parent().unwrap())
                .await
                .unwrap();
            tokio::fs::write(&series_path, b"scanner content")
                .await
                .unwrap();

            let db_path = tmp_path.join("test.db");
            let db = std::sync::Arc::new(
                crate::db::DbManager::new(&db_path).await.unwrap(),
            );

            let episode_id = "Show_S01E01";
            let series_id = "Test Show";

            let q_profile = jumbie_shared::scoring::QualityProfile {
                name: "Any".to_string(),
                qualities: vec![],
                upgrade_only_qualities: vec![],
            };
            db.upsert_quality_profile("Any", &q_profile).await.unwrap();

            let mapping = jumbie_shared::types::MappingRule {
                target_title: "Test Show".to_string(),
                name: "Test Show".to_string(),
                quality_profile: Some("Any".to_string()),
                release_profile: Some("Any".to_string()),
                settings: jumbie_shared::mapping::SeriesSettings {
                    ..Default::default()
                },
                ..Default::default()
            };
            db.upsert_series_mapping(series_id, &mapping).await.unwrap();

            // Episode (score is now on release_info via fingerprint)
            sqlx::query(
                "INSERT INTO episodes (episode_id, series_id, season, episode, status)
                 VALUES (?, ?, ?, ?, ?)",
            )
            .bind(episode_id)
            .bind(series_id)
            .bind(1)
            .bind(1)
            .bind("organized")
            .execute(db.get_pool())
            .await
            .unwrap();
            db.associate_main_file(episode_id, series_path.to_string_lossy().as_ref(), None)
                .await
                .unwrap();

            let (inode, dev, mtime) = crate::platform::file_identity(
                &tokio::fs::metadata(&download_path).await.unwrap(),
            );
            let hash = "test_hash_upgrade".to_string();
            db.save_fingerprint(crate::db::files::SaveFingerprintParams {
                    path: download_path.to_str().unwrap(),
                    inode,
                    dev,
                    size: download_path.metadata().unwrap().len(),
                    mtime,
                    quick_hash: &hash,
                    state: "complete",
                    media_info: None::<&jumbie_shared::types::MediaInfo>,
                })
                .await
                .unwrap();

            db.link_file_episode(download_path.to_str().unwrap(), episode_id)
                .await
                .unwrap();

            sqlx::query(
                "INSERT INTO download_queue (media_name, media_link, series_title, season, episode, episode_id, score, is_user_requested, status, episode_intentions)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind("Test Show S01E01")
            .bind("magnet:?xt=urn:btih:test_upgrade")
            .bind("Test Show")
            .bind("1")
            .bind(1)
            .bind(episode_id)
            .bind(85)
            .bind(false)
            .bind("Completed")
            .bind(r#"[{"episode_num":1,"source_episode_num":1,"episode_id":"Show_S01E01","score":85,"keep":true}]"#)
            .execute(db.get_pool())
            .await
            .unwrap();

            let organizer = setup_test_organizer_with_format(db.clone(), &db_path, &dest_root, "${series} - S${season:02}E${episode:02}.mkv").await;

            organizer
                .organize_completed()
                .await
                .expect("organize_completed should succeed");

            // Download file should be MOVED to the series path (upgrade happened)
            assert!(
                !download_path.exists(),
                "Download file should be moved after upgrade"
            );

            // Series file should now have the download's content (upgraded)
            let series_content = tokio::fs::read_to_string(&series_path).await.unwrap();
            assert_eq!(
                series_content, "download content",
                "Series file should have upgraded content"
            );

            // Queue item should be removed
            let queue_remaining: Option<(i64,)> = sqlx::query_as(
                "SELECT id FROM download_queue WHERE episode_id = ?",
            )
            .bind(episode_id)
            .fetch_optional(db.get_pool())
            .await
            .unwrap();
            assert!(queue_remaining.is_none(), "Queue item should be removed");
        });
}

// organize_completed: is_manual overrides upgrades gate
//
// When a queue item has is_manual=true, it forces upgrade evaluation at organize
// time regardless of the series' upgrade settings, so a manually chosen release
// can replace an existing file even if it scores lower.

#[test]
fn test_organize_completed_manual_upgrade_overrides_disabled_setting() {
    // Manually chosen download with upgrades OFF + better score → REPLACES
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("Failed to build runtime");
    rt.block_on(async {
        let tmp = tempfile::tempdir().unwrap();
        let tmp_path = tmp.keep();

        let dest_root = tmp_path.join("organized");
        tokio::fs::create_dir_all(&dest_root).await.unwrap();

        let download_path = tmp_path.join("downloads").join("Show.S01E01.mkv");
        tokio::fs::create_dir_all(download_path.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(&download_path, b"better quality download")
            .await
            .unwrap();

        let series_path = dest_root.join("Show").join("S01").join("Show.S01E01.mkv");
        tokio::fs::create_dir_all(series_path.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(&series_path, b"scanner content")
            .await
            .unwrap();

        let db_path = tmp_path.join("test.db");
        let db = std::sync::Arc::new(
            crate::db::DbManager::new(&db_path).await.unwrap(),
        );

        let episode_id = "Show_S01E01";
        let series_id = "Test Show";

        let q_profile = jumbie_shared::scoring::QualityProfile {
            name: "Any".to_string(),
            qualities: vec![],
            upgrade_only_qualities: vec![],
        };
        db.upsert_quality_profile("Any", &q_profile).await.unwrap();

        // Series mapping with upgrades DISABLED
        let mapping = jumbie_shared::types::MappingRule {
            target_title: "Test Show".to_string(),
            name: "Test Show".to_string(),
            quality_profile: Some("Any".to_string()),
            release_profile: Some("Any".to_string()),
            settings: jumbie_shared::mapping::SeriesSettings {
                ..Default::default()
            },
            ..Default::default()
        };
        db.upsert_series_mapping(series_id, &mapping).await.unwrap();

        // Existing episode (score is now on release_info via fingerprint)
        sqlx::query(
            "INSERT INTO episodes (episode_id, series_id, season, episode, status)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(episode_id)
        .bind(series_id)
        .bind(1)
        .bind(1)
        .bind("organized")
        .execute(db.get_pool())
        .await
        .unwrap();
        db.associate_main_file(episode_id, series_path.to_string_lossy().as_ref(), None)
            .await
            .unwrap();

        let (inode, dev, mtime) = crate::platform::file_identity(
            &tokio::fs::metadata(&download_path).await.unwrap(),
        );
        let hash = "test_hash_manual_upgrade".to_string();
        db.save_fingerprint(crate::db::files::SaveFingerprintParams {
                path: download_path.to_str().unwrap(),
                inode,
                dev,
                size: download_path.metadata().unwrap().len(),
                mtime,
                quick_hash: &hash,
                state: "complete",
                media_info: None::<&jumbie_shared::types::MediaInfo>,
            })
            .await
            .unwrap();

        db.link_file_episode(download_path.to_str().unwrap(), episode_id)
            .await
            .unwrap();

        // Queue item: is_manual=true, score=80, with episode_intentions
        sqlx::query(
            "INSERT INTO download_queue (media_name, media_link, series_title, season, episode, episode_id, score, is_user_requested, is_manual, status, episode_intentions)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind("Test Show S01E01")
        .bind("magnet:?xt=urn:btih:test_manual_upgrade")
        .bind("Test Show")
        .bind("1")
        .bind(1)
        .bind(episode_id)
        .bind(80)
        .bind(true) // is_user_requested
        .bind(true) // is_manual
        .bind("Completed")
        .bind(r#"[{"episode_num":1,"source_episode_num":1,"episode_id":"Show_S01E01","score":80,"keep":true}]"#)
        .execute(db.get_pool())
        .await
        .unwrap();

        let organizer = setup_test_organizer_with_format(db.clone(), &db_path, &dest_root, "${series} - S${season:02}E${episode:02}.mkv").await;

        organizer
            .organize_completed()
            .await
            .expect("organize_completed should succeed");

        // Series file should be REPLACED with download content
        let series_content = tokio::fs::read_to_string(&series_path).await.unwrap();
        assert_eq!(
            series_content, "better quality download",
            "User-requested download should replace existing file despite upgrades being disabled"
        );

        // Download file should be gone (moved to series path)
        assert!(
            !download_path.exists(),
            "Download file should be removed after replacement"
        );
    });
}

// organize_completed: parsing/mapping failure marks queue as Failed
//
// When organize_file returns None (no series mapping exists for the episode),
// the queue item should be marked as Failed so the user can see the error
// and retry after fixing the mapping.
#[test]
fn test_organize_completed_marks_failed_when_no_mapping() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("Failed to build runtime");

    rt.block_on(async {
            let tmp = tempfile::tempdir().unwrap();
            let tmp_path = tmp.keep();

            let dest_root = tmp_path.join("organized");
            tokio::fs::create_dir_all(&dest_root).await.unwrap();

            // File is OUTSIDE the destination root (normal case)
            let file_path = tmp_path.join("Unknown.S01E01.mkv");
            tokio::fs::write(&file_path, b"unmapped content")
                .await
                .unwrap();

            let db_path = tmp_path.join("test.db");
            let db = std::sync::Arc::new(
                crate::db::DbManager::new(&db_path).await.unwrap(),
            );

            let episode_id = "Unknown_S01E01";
            let nonexistent_series_id = "nonexistent_series";

            // Insert episode with a series_id that has NO mapping
            sqlx::query(
                "INSERT INTO episodes (episode_id, series_id, season, episode, status)
                 VALUES (?, ?, ?, ?, ?)",
            )
            .bind(episode_id)
            .bind(nonexistent_series_id)
            .bind(1)
            .bind(1)
            .bind("downloaded")
            .execute(db.get_pool())
            .await
            .unwrap();
            db.associate_main_file(episode_id, file_path.to_string_lossy().as_ref(), None)
                .await
                .unwrap();

            // DO NOT insert a series_mapping — that's the failure condition

            let (inode, dev, mtime) =
                crate::platform::file_identity(&tokio::fs::metadata(&file_path).await.unwrap());
            let hash = "test_hash_unmapped".to_string();
            db.save_fingerprint(crate::db::files::SaveFingerprintParams {
                    path: file_path.to_str().unwrap(),
                    inode,
                    dev,
                    size: file_path.metadata().unwrap().len(),
                    mtime,
                    quick_hash: &hash,
                    state: "complete",
                    media_info: None::<&jumbie_shared::types::MediaInfo>,
                })
                .await
                .unwrap();

            db.link_file_episode(file_path.to_str().unwrap(), episode_id)
                .await
                .unwrap();

            sqlx::query(
                "INSERT INTO download_queue (media_name, media_link, series_title, season, episode, episode_id, score, is_user_requested, status)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind("Unknown S01E01")
            .bind("magnet:?xt=urn:btih:test_unmapped")
            .bind("Unknown Series")
            .bind("1")
            .bind(1)
            .bind(episode_id)
            .bind(100)
            .bind(false)
            .bind("Completed")
            .execute(db.get_pool())
            .await
            .unwrap();

            let organizer = setup_test_organizer_with_format(db.clone(), &db_path, &dest_root, "${series} - S${season:02}E${episode:02}.mkv").await;

            organizer
                .organize_completed()
                .await
                .expect("organize_completed should succeed");

            // The queue item should be marked as Failed with score -99999
            let queue_item: Option<(String, i32, Option<String>)> = sqlx::query_as(
                "SELECT status, score, error_message FROM download_queue WHERE episode_id = ?",
            )
            .bind(episode_id)
            .fetch_optional(db.get_pool())
            .await
            .unwrap();

            let (status, score, error_message) = queue_item
                .expect("Queue item should still exist (marked as Failed, not removed)");

            assert_eq!(
                status, "Failed",
                "Queue item should be Failed when no mapping exists"
            );
            assert_eq!(
                score, -99999,
                "Score should be set to -99999 to prevent re-selection"
            );
            assert!(
                error_message
                    .as_deref()
                    .unwrap_or("")
                    .contains("Could not parse filename or find series mapping"),
                "Error message should explain the failure, got: {:?}",
                error_message
            );

            // The source file should still exist (it was NOT moved)
            assert!(
                file_path.exists(),
                "Source file should remain when organize failed"
            );
        });
}

// organize_completed: rename failure during upgrade
//
// When the upgrade path's rename fails (e.g. target directory missing),
// the orphaned download file should still be cleaned up from disk, the
// client notified, and the queue item removed.
#[test]
fn test_organize_completed_handles_rename_failure_during_upgrade() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("Failed to build runtime");

    rt.block_on(async {
            let tmp = tempfile::tempdir().unwrap();
            let tmp_path = tmp.keep();

            let dest_root = tmp_path.join("organized");
            tokio::fs::create_dir_all(&dest_root).await.unwrap();

            // Download file — outside destination root
            let download_path = tmp_path.join("downloads").join("Show.S01E01.mkv");
            tokio::fs::create_dir_all(download_path.parent().unwrap())
                .await
                .unwrap();
            tokio::fs::write(&download_path, b"download content")
                .await
                .unwrap();

            // Episode's file_path points to a NON-EXISTENT directory
            // so rename will fail (ENOENT).
            let series_path = dest_root.join("nonexistent_subdir").join("Show.S01E01.mkv");
            // Deliberately do NOT create the parent directory

            let db_path = tmp_path.join("test.db");
            let db = std::sync::Arc::new(
                crate::db::DbManager::new(&db_path).await.unwrap(),
            );

            let episode_id = "Show_S01E01";
            let series_id = "Test Show";

            let q_profile = jumbie_shared::scoring::QualityProfile {
                name: "Any".to_string(),
                qualities: vec![],
                upgrade_only_qualities: vec![],
            };
            db.upsert_quality_profile("Any", &q_profile).await.unwrap();

            let mapping = jumbie_shared::types::MappingRule {
                target_title: "Test Show".to_string(),
                name: "Test Show".to_string(),
                quality_profile: Some("Any".to_string()),
                release_profile: Some("Any".to_string()),
                settings: jumbie_shared::mapping::SeriesSettings {
                    ..Default::default()
                },
                ..Default::default()
            };
            db.upsert_series_mapping(series_id, &mapping).await.unwrap();

            // Episode file_path pointing to non-existent dir
            sqlx::query(
                "INSERT INTO episodes (episode_id, series_id, season, episode, status)
                 VALUES (?, ?, ?, ?, ?)",
            )
            .bind(episode_id)
            .bind(series_id)
            .bind(1)
            .bind(1)
            .bind("organized")
            .execute(db.get_pool())
            .await
            .unwrap();
            db.associate_main_file(episode_id, series_path.to_string_lossy().as_ref(), None)
                .await
                .unwrap();

            let (inode, dev, mtime) = crate::platform::file_identity(
                &tokio::fs::metadata(&download_path).await.unwrap(),
            );
            let hash = "test_hash_rename_fail".to_string();
            db.save_fingerprint(crate::db::files::SaveFingerprintParams {
                    path: download_path.to_str().unwrap(),
                    inode,
                    dev,
                    size: download_path.metadata().unwrap().len(),
                    mtime,
                    quick_hash: &hash,
                    state: "complete",
                    media_info: None::<&jumbie_shared::types::MediaInfo>,
                })
                .await
                .unwrap();

            db.link_file_episode(download_path.to_str().unwrap(), episode_id)
                .await
                .unwrap();

            sqlx::query(
                "INSERT INTO download_queue (media_name, media_link, series_title, season, episode, episode_id, score, is_user_requested, status, episode_intentions, downloader_id)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind("Test Show S01E01")
            .bind("magnet:?xt=urn:btih:test_rename_fail")
            .bind("Test Show")
            .bind("1")
            .bind(1)
            .bind(episode_id)
            .bind(85)
            .bind(false)
            .bind("Completed")
            .bind(r#"[{"episode_num":1,"source_episode_num":1,"episode_id":"Show_S01E01","score":85,"keep":true}]"#)
            .bind("test_hash_rename_fail")
            .execute(db.get_pool())
            .await
            .unwrap();

            let organizer = setup_test_organizer_with_format(db.clone(), &db_path, &dest_root, "${series} - S${season:02}E${episode:02}.mkv").await;

            organizer
                .organize_completed()
                .await
                .expect("organize_completed should succeed");

            // The download file should be DELETED (rename failed → orphan removed)
            assert!(
                !download_path.exists(),
                "Download file should be deleted when rename fails"
            );

            // Stale fingerprint should be gone
            let fingerprint: Option<(String,)> = sqlx::query_as(
                "SELECT state FROM file_paths WHERE file_path = ?",
            )
            .bind(download_path.to_string_lossy().to_string())
            .fetch_optional(db.get_pool())
            .await
            .unwrap();
            assert!(
                fingerprint.is_none(),
                "Stale fingerprint should be removed after rename failure"
            );

            // Queue item should be removed
            let queue_remaining: Option<(i64,)> = sqlx::query_as(
                "SELECT id FROM download_queue WHERE episode_id = ?",
            )
            .bind(episode_id)
            .fetch_optional(db.get_pool())
            .await
            .unwrap();
            assert!(
                queue_remaining.is_none(),
                "Queue item should be removed after rename failure"
            );
        });
}

// organize_completed: manual upgrade overrides lower score
//
// When a queue item has is_manual=true but the candidate score is LOWER than the
// existing episode's score, the manual flag still forces the upgrade — the user
// explicitly chose this download.
#[test]
fn test_organize_completed_manual_lower_score_still_upgrades() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("Failed to build runtime");

    rt.block_on(async {
            let tmp = tempfile::tempdir().unwrap();
            let tmp_path = tmp.keep();

            let dest_root = tmp_path.join("organized");
            tokio::fs::create_dir_all(&dest_root).await.unwrap();

            // Download file — outside destination root
            let download_path = tmp_path.join("downloads").join("Show.S01E01.mkv");
            tokio::fs::create_dir_all(download_path.parent().unwrap())
                .await
                .unwrap();
            tokio::fs::write(&download_path, b"user chosen content")
                .await
                .unwrap();

            // Existing organized file
            let series_path = dest_root.join("Show").join("S01").join("Show.S01E01.mkv");
            tokio::fs::create_dir_all(series_path.parent().unwrap())
                .await
                .unwrap();
            tokio::fs::write(&series_path, b"existing better content")
                .await
                .unwrap();

            let db_path = tmp_path.join("test.db");
            let db = std::sync::Arc::new(
                crate::db::DbManager::new(&db_path).await.unwrap(),
            );

            let episode_id = "Show_S01E01";
            let series_id = "Test Show";

            let mapping = jumbie_shared::types::MappingRule {
                target_title: "Test Show".to_string(),
                name: "Test Show".to_string(),
                quality_profile: Some("Any".to_string()),
                release_profile: Some("Any".to_string()),
                settings: jumbie_shared::mapping::SeriesSettings {
                    ..Default::default()
                },
                ..Default::default()
            };
            db.upsert_series_mapping(series_id, &mapping).await.unwrap();

            // Episode is 'organized' with a high-scoring file
            sqlx::query(
                "INSERT INTO episodes (episode_id, series_id, season, episode, status)
                 VALUES (?, ?, ?, ?, ?)",
            )
            .bind(episode_id)
            .bind(series_id)
            .bind(1)
            .bind(1)
            .bind("organized")
            .execute(db.get_pool())
            .await
            .unwrap();
            db.associate_main_file(episode_id, series_path.to_string_lossy().as_ref(), None)
                .await
                .unwrap();

            // Existing file's fingerprint + release_info with a HIGH score (100)
            let (inode, dev, mtime) = crate::platform::file_identity(
                &tokio::fs::metadata(&series_path).await.unwrap(),
            );
            let existing_hash = "existing_hash_high_score".to_string();
            db.save_fingerprint(crate::db::files::SaveFingerprintParams {
                    path: series_path.to_str().unwrap(),
                    inode,
                    dev,
                    size: series_path.metadata().unwrap().len(),
                    mtime,
                    quick_hash: &existing_hash,
                    state: "organized",
                    media_info: None::<&jumbie_shared::types::MediaInfo>,
                })
                .await
                .unwrap();

            db.link_file_episode(series_path.to_str().unwrap(), episode_id)
                .await
                .unwrap();

            // Set a high score (100) on the existing release via release_info
            let _ = sqlx::query(
                "INSERT INTO release_info (quick_hash, score, version)
                 VALUES (?, ?, 1)
                 ON CONFLICT(quick_hash) DO UPDATE SET score = excluded.score",
            )
            .bind(&existing_hash)
            .bind(100)
            .execute(db.get_pool())
            .await;

            let (inode, dev, mtime) = crate::platform::file_identity(
                &tokio::fs::metadata(&download_path).await.unwrap(),
            );
            let new_hash = "new_hash_user_download".to_string();
            db.save_fingerprint(crate::db::files::SaveFingerprintParams {
                    path: download_path.to_str().unwrap(),
                    inode,
                    dev,
                    size: download_path.metadata().unwrap().len(),
                    mtime,
                    quick_hash: &new_hash,
                    state: "complete",
                    media_info: None::<&jumbie_shared::types::MediaInfo>,
                })
                .await
                .unwrap();

            db.link_file_episode(download_path.to_str().unwrap(), episode_id)
                .await
                .unwrap();

            // Queue item: is_manual=true with LOWER score (10)
            sqlx::query(
                "INSERT INTO download_queue (media_name, media_link, series_title, season, episode, episode_id, score, is_user_requested, is_manual, status, episode_intentions)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind("Test Show S01E01")
            .bind("magnet:?xt=urn:btih:test_user_lower")
            .bind("Test Show")
            .bind("1")
            .bind(1)
            .bind(episode_id)
            .bind(10)
            .bind(true) // is_user_requested
            .bind(true) // is_manual
            .bind("Completed")
            .bind(r#"[{"episode_num":1,"source_episode_num":1,"episode_id":"Show_S01E01","score":10,"keep":true}]"#)
            .execute(db.get_pool())
            .await
            .unwrap();

            let organizer = setup_test_organizer_with_format(db.clone(), &db_path, &dest_root, "${series} - S${season:02}E${episode:02}.mkv").await;

            organizer
                .organize_completed()
                .await
                .expect("organize_completed should succeed");

            // The download file should have REPLACED the existing file despite lower score
            assert!(
                !download_path.exists(),
                "Download file should be moved after user-requested upgrade"
            );
            let series_content = tokio::fs::read_to_string(&series_path).await.unwrap();
            assert_eq!(
                series_content, "user chosen content",
                "User-requested download should replace higher-scored existing file"
            );

            // Queue item should be removed
            let queue_remaining: Option<(i64,)> = sqlx::query_as(
                "SELECT id FROM download_queue WHERE episode_id = ?",
            )
            .bind(episode_id)
            .fetch_optional(db.get_pool())
            .await
            .unwrap();
            assert!(queue_remaining.is_none(), "Queue item should be removed after upgrade");
        });
}

// organize_completed: auto-download lower score cleans up
//
// When a non-user-requested download has a score LOWER than the existing
// episode's score, it should be cleaned up (not replace the existing file).
#[test]
fn test_organize_completed_auto_download_lower_score_cleans_up() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("Failed to build runtime");

    rt.block_on(async {
            let tmp = tempfile::tempdir().unwrap();
            let tmp_path = tmp.keep();

            let dest_root = tmp_path.join("organized");
            tokio::fs::create_dir_all(&dest_root).await.unwrap();

            // Download file — outside destination root
            let download_path = tmp_path.join("downloads").join("Show.S01E01.mkv");
            tokio::fs::create_dir_all(download_path.parent().unwrap())
                .await
                .unwrap();
            tokio::fs::write(&download_path, b"inferior download content")
                .await
                .unwrap();

            // Existing organized file
            let series_path = dest_root.join("Show").join("S01").join("Show.S01E01.mkv");
            tokio::fs::create_dir_all(series_path.parent().unwrap())
                .await
                .unwrap();
            tokio::fs::write(&series_path, b"superior existing content")
                .await
                .unwrap();

            let db_path = tmp_path.join("test.db");
            let db = std::sync::Arc::new(
                crate::db::DbManager::new(&db_path).await.unwrap(),
            );

            let episode_id = "Show_S01E01";
            let series_id = "Test Show";

            let mapping = jumbie_shared::types::MappingRule {
                target_title: "Test Show".to_string(),
                name: "Test Show".to_string(),
                quality_profile: Some("Any".to_string()),
                release_profile: Some("Any".to_string()),
                settings: jumbie_shared::mapping::SeriesSettings {
                    ..Default::default()
                },
                ..Default::default()
            };
            db.upsert_series_mapping(series_id, &mapping).await.unwrap();

            // Episode is 'organized' with a high-scoring file
            sqlx::query(
                "INSERT INTO episodes (episode_id, series_id, season, episode, status)
                 VALUES (?, ?, ?, ?, ?)",
            )
            .bind(episode_id)
            .bind(series_id)
            .bind(1)
            .bind(1)
            .bind("organized")
            .execute(db.get_pool())
            .await
            .unwrap();
            db.associate_main_file(episode_id, series_path.to_string_lossy().as_ref(), None)
                .await
                .unwrap();

            // Existing file's fingerprint + release_info with a HIGH score (100)
            let (inode, dev, mtime) = crate::platform::file_identity(
                &tokio::fs::metadata(&series_path).await.unwrap(),
            );
            let existing_hash = "existing_hash_high_score_2".to_string();
            db.save_fingerprint(crate::db::files::SaveFingerprintParams {
                    path: series_path.to_str().unwrap(),
                    inode,
                    dev,
                    size: series_path.metadata().unwrap().len(),
                    mtime,
                    quick_hash: &existing_hash,
                    state: "organized",
                    media_info: None::<&jumbie_shared::types::MediaInfo>,
                })
                .await
                .unwrap();

            db.link_file_episode(series_path.to_str().unwrap(), episode_id)
                .await
                .unwrap();

            // Set a high score (100) on the existing release via release_info
            let _ = sqlx::query(
                "INSERT INTO release_info (quick_hash, score, version)
                 VALUES (?, ?, 1)
                 ON CONFLICT(quick_hash) DO UPDATE SET score = excluded.score",
            )
            .bind(&existing_hash)
            .bind(100)
            .execute(db.get_pool())
            .await;

            let (inode, dev, mtime) = crate::platform::file_identity(
                &tokio::fs::metadata(&download_path).await.unwrap(),
            );
            let new_hash = "new_hash_auto_download".to_string();
            db.save_fingerprint(crate::db::files::SaveFingerprintParams {
                    path: download_path.to_str().unwrap(),
                    inode,
                    dev,
                    size: download_path.metadata().unwrap().len(),
                    mtime,
                    quick_hash: &new_hash,
                    state: "complete",
                    media_info: None::<&jumbie_shared::types::MediaInfo>,
                })
                .await
                .unwrap();

            db.link_file_episode(download_path.to_str().unwrap(), episode_id)
                .await
                .unwrap();

            // Queue item: is_user_requested=false with LOWER score (10)
            sqlx::query(
                "INSERT INTO download_queue (media_name, media_link, series_title, season, episode, episode_id, score, is_user_requested, status, episode_intentions)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind("Test Show S01E01")
            .bind("magnet:?xt=urn:btih:test_auto_lower")
            .bind("Test Show")
            .bind("1")
            .bind(1)
            .bind(episode_id)
            .bind(10)
            .bind(false) // is_user_requested
            .bind("Completed")
            .bind(r#"[{"episode_num":1,"source_episode_num":1,"episode_id":"Show_S01E01","score":10,"keep":true}]"#)
            .execute(db.get_pool())
            .await
            .unwrap();

            let organizer = setup_test_organizer_with_format(db.clone(), &db_path, &dest_root, "${series} - S${season:02}E${episode:02}.mkv").await;

            organizer
                .organize_completed()
                .await
                .expect("organize_completed should succeed");

            // The download file should be DELETED (not moved to series path)
            assert!(
                !download_path.exists(),
                "Inferior auto-download file should be deleted"
            );

            // The series file should still have the original content
            let series_content = tokio::fs::read_to_string(&series_path).await.unwrap();
            assert_eq!(
                series_content, "superior existing content",
                "Auto-download with lower score should NOT replace existing file"
            );

            // Queue item should be removed
            let queue_remaining: Option<(i64,)> = sqlx::query_as(
                "SELECT id FROM download_queue WHERE episode_id = ?",
            )
            .bind(episode_id)
            .fetch_optional(db.get_pool())
            .await
            .unwrap();
            assert!(queue_remaining.is_none(), "Queue item should be removed after cleanup");
        });
}

// Collision diagnosis (diagnose_collision)

#[tokio::test]
async fn test_diagnose_collision_no_collision_when_dst_missing() {
    // When dst doesn't exist — NoCollision
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test.db");
    let db = Arc::new(DbManager::new(&db_path).await.unwrap());
    let src = tmp.path().join("src.mkv");
    let dst = tmp.path().join("dst.mkv");
    tokio::fs::write(&src, b"content").await.unwrap();

    let kind = super::file_ops::diagnose_collision(&src, &dst, None, &db).await;
    assert_eq!(kind, super::file_ops::CollisionKind::NoCollision);
}

#[tokio::test]
async fn test_diagnose_collision_no_collision_when_same_file() {
    // When src and dst are the same inode — NoCollision
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test.db");
    let db = Arc::new(DbManager::new(&db_path).await.unwrap());
    let file = tmp.path().join("file.mkv");
    tokio::fs::write(&file, b"content").await.unwrap();

    let kind = super::file_ops::diagnose_collision(&file, &file, None, &db).await;
    assert_eq!(kind, super::file_ops::CollisionKind::NoCollision);
}

#[tokio::test]
async fn test_diagnose_collision_temp_occupant() {
    // When dst is in batch_sources — TempOccupant
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test.db");
    let db = Arc::new(DbManager::new(&db_path).await.unwrap());
    let src = tmp.path().join("src.mkv");
    let dst = tmp.path().join("dst.mkv");
    tokio::fs::write(&src, b"src").await.unwrap();
    tokio::fs::write(&dst, b"dst").await.unwrap();
    let mut batch = HashSet::new();
    batch.insert(dst.clone());

    let kind = super::file_ops::diagnose_collision(&src, &dst, Some(&batch), &db).await;
    assert_eq!(kind, super::file_ops::CollisionKind::TempOccupant);
}

#[tokio::test]
async fn test_diagnose_collision_unassigned_occupant() {
    // Dst exists, but no episode points to it — UnassignedOccupant
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test.db");
    let db = Arc::new(DbManager::new(&db_path).await.unwrap());
    let src = tmp.path().join("src.mkv");
    let dst = tmp.path().join("dst.mkv");
    tokio::fs::write(&src, b"src").await.unwrap();
    tokio::fs::write(&dst, b"dst").await.unwrap();

    // No episode rows exist — occupant is unassigned
    let kind = super::file_ops::diagnose_collision(&src, &dst, None, &db).await;
    assert_eq!(kind, super::file_ops::CollisionKind::UnassignedOccupant);
}

#[tokio::test]
async fn test_diagnose_collision_assigned_occupant() {
    // Dst exists and an episode points to it — AssignedOccupant
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test.db");
    let db = Arc::new(DbManager::new(&db_path).await.unwrap());
    let src = tmp.path().join("src.mkv");
    let dst = tmp.path().join("dst.mkv");
    tokio::fs::write(&src, b"src").await.unwrap();
    tokio::fs::write(&dst, b"occupied.mkv").await.unwrap();

    // Insert an episode that owns this file_path
    let dst_str = dst.to_string_lossy().to_string();
    let meta_ids = std::collections::HashMap::new();
    db.insert_episode(crate::db::episodes::crud::InsertEpisodeParams {
        file_path: Some(&dst_str),
        status: "organized",
        ..crate::db::episodes::crud::InsertEpisodeParams::dummy(
            "test_S01E01",
            "test_series",
            1,
            1,
            &meta_ids,
        )
    })
    .await
    .unwrap();

    let kind = super::file_ops::diagnose_collision(&src, &dst, None, &db).await;
    assert_eq!(kind, super::file_ops::CollisionKind::AssignedOccupant);
}

// move_file_to_target: unassigned occupant moved aside

#[tokio::test]
async fn test_move_file_unassigned_occupant_moved_aside() {
    // When dst is occupied by an unassigned file, move_file_to_target should
    // move the occupant aside and place the incoming file at the clean dst.
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test.db");
    let db = Arc::new(DbManager::new(&db_path).await.unwrap());
    let organizer = crate::tests::organizer_fixtures::make_test_organizer(db.clone()).await;

    let mut config = crate::test_fixtures::config_with_dest_root(&db_path, tmp.path());
    config.organization.collision_rename_suffix =
        jumbie_shared::config::organization::CollisionRenameSuffix::ParenNumeric;
    let _ = db.save_organization_config(&config.organization).await;

    let dst_path = tmp.path().join("Show - S01E01.mkv");
    tokio::fs::write(&dst_path, b"old occupant content")
        .await
        .unwrap();

    let src_path = tmp.path().join("new_source.mkv");
    tokio::fs::write(&src_path, b"new incoming content")
        .await
        .unwrap();

    let result = organizer
        .move_file_to_target(&src_path, &dst_path, None)
        .await
        .unwrap();

    // The incoming file should be at the clean dst path
    assert_eq!(result, dst_path, "Incoming file should get the clean path");
    assert!(dst_path.exists(), "Clean dst should exist");
    let content = tokio::fs::read_to_string(&dst_path).await.unwrap();
    assert!(
        content.contains("new incoming content"),
        "Clean dst should contain incoming file content"
    );

    // The occupant should have been moved aside with a suffix
    let occupant_path = tmp.path().join("Show - S01E01 (1).mkv");
    assert!(
        occupant_path.exists(),
        "Occupant should exist at suffixed path"
    );
    let occ_content = tokio::fs::read_to_string(&occupant_path).await.unwrap();
    assert!(
        occ_content.contains("old occupant content"),
        "Suffixed path should contain occupant content"
    );

    // Source file should be gone (it was renamed to dst)
    assert!(
        !src_path.exists(),
        "Source file should be removed after move"
    );
}

#[tokio::test]
async fn test_move_file_unassigned_occupant_strips_existing_suffix() {
    // When the occupant already has a suffix (e.g. "Show - S01E01 (1).mkv")
    // and the new file wants "Show - S01E01.mkv", the occupant should have its
    // suffix stripped first (if free), preventing nesting.
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test.db");
    let db = Arc::new(DbManager::new(&db_path).await.unwrap());
    let organizer = crate::tests::organizer_fixtures::make_test_organizer(db.clone()).await;

    let mut config = crate::test_fixtures::config_with_dest_root(&db_path, tmp.path());
    config.organization.collision_rename_suffix =
        jumbie_shared::config::organization::CollisionRenameSuffix::ParenNumeric;
    let _ = db.save_organization_config(&config.organization).await;

    // "Clean" name path — this is what the new file wants
    let clean_path = tmp.path().join("Show - S01E01.mkv");

    // Occupant already has a suffix — this simulates a previously displaced file
    let occupant_path = tmp.path().join("Show - S01E01 (1).mkv");
    tokio::fs::write(&occupant_path, b"already suffixed occupant")
        .await
        .unwrap();

    let src_path = tmp.path().join("new_file.mkv");
    tokio::fs::write(&src_path, b"incoming content")
        .await
        .unwrap();

    let result = organizer
        .move_file_to_target(&src_path, &clean_path, None)
        .await
        .unwrap();

    // Incoming file gets the clean path
    assert_eq!(result, clean_path);
    let content = tokio::fs::read_to_string(&clean_path).await.unwrap();
    assert!(content.contains("incoming content"));

    // The occupant should STILL be at its suffixed path (strip_collision_suffix
    // detected the suffix, but the stripped path "Show - S01E01.mkv" was already
    // taken by the new incoming file, so the occupant stays at its original
    // suffixed name — no nesting occurs).
    assert!(
        occupant_path.exists(),
        "Occupant stays at its existing suffixed path"
    );
    let occ = tokio::fs::read_to_string(&occupant_path).await.unwrap();
    assert!(occ.contains("already suffixed occupant"));
}

#[tokio::test]
async fn test_move_file_assigned_occupant_incoming_gets_suffix() {
    // When dst is occupied by an ASSIGNED file (episode still points to it),
    // the incoming file should get the suffix — not the occupant.
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test.db");
    let db = Arc::new(DbManager::new(&db_path).await.unwrap());
    let organizer = crate::tests::organizer_fixtures::make_test_organizer(db.clone()).await;

    let mut config = crate::test_fixtures::config_with_dest_root(&db_path, tmp.path());
    config.organization.collision_rename_suffix =
        jumbie_shared::config::organization::CollisionRenameSuffix::ParenNumeric;
    let _ = db.save_organization_config(&config.organization).await;

    let dst_path = tmp.path().join("Show - S01E01.mkv");
    tokio::fs::write(&dst_path, b"assigned occupant")
        .await
        .unwrap();

    let meta_ids = std::collections::HashMap::new();
    let dst_str = dst_path.to_string_lossy().to_string();
    db.insert_episode(crate::db::episodes::crud::InsertEpisodeParams {
        file_path: Some(&dst_str),
        status: "organized",
        ..crate::db::episodes::crud::InsertEpisodeParams::dummy(
            "assigned_S01E01",
            "test_series",
            1,
            1,
            &meta_ids,
        )
    })
    .await
    .unwrap();

    let src_path = tmp.path().join("new_file.mkv");
    tokio::fs::write(&src_path, b"incoming content")
        .await
        .unwrap();

    // Set collision_handling to "rename" (default) so incoming gets suffix
    let result = organizer
        .move_file_to_target(&src_path, &dst_path, None)
        .await
        .unwrap();

    // The incoming file should get a suffixed path
    assert_ne!(
        result, dst_path,
        "Incoming should NOT get clean path when occupant is assigned"
    );
    assert!(result.exists(), "Suffixed incoming should exist");
    let incoming_content = tokio::fs::read_to_string(&result).await.unwrap();
    assert!(incoming_content.contains("incoming content"));

    // The occupant should still be at the clean path (unchanged)
    assert!(dst_path.exists(), "Occupant should remain at clean path");
    let occ_content = tokio::fs::read_to_string(&dst_path).await.unwrap();
    assert!(occ_content.contains("assigned occupant"));
}

#[tokio::test]
async fn test_different_extensions_same_stem_never_collide() {
    // Collision detection keys on the FULL filename including the extension, so a
    // sidecar and a second container for the same episode are never collisions.
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test.db");
    let db = Arc::new(DbManager::new(&db_path).await.unwrap());
    let organizer = crate::tests::organizer_fixtures::make_test_organizer(db.clone()).await;

    let video = tmp.path().join("Show - S01E01.mkv");
    tokio::fs::write(&video, b"video").await.unwrap();

    // `.nfo` shares the stem with the `.mkv` but is a different file.
    let nfo_src = tmp.path().join("incoming.nfo");
    tokio::fs::write(&nfo_src, b"nfo").await.unwrap();
    let nfo_dst = tmp.path().join("Show - S01E01.nfo");
    let result = organizer
        .move_file_to_target(&nfo_src, &nfo_dst, None)
        .await
        .unwrap();
    assert_eq!(
        result, nfo_dst,
        "a sidecar must not be suffixed against the episode video"
    );

    // A second container (same episode, different format) is also a distinct name.
    let mp4_src = tmp.path().join("incoming.mp4");
    tokio::fs::write(&mp4_src, b"mp4").await.unwrap();
    let mp4_dst = tmp.path().join("Show - S01E01.mp4");
    let result = organizer
        .move_file_to_target(&mp4_src, &mp4_dst, None)
        .await
        .unwrap();
    assert_eq!(
        result, mp4_dst,
        "a different container is a distinct filename"
    );
    assert!(video.exists(), "the original video is untouched");
}

#[tokio::test]
async fn test_collision_gone_removes_suffix() {
    // A sidecar left with a counter by an old collision must return to the clean
    // name once the occupant is gone and the plan targets the clean name again.
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test.db");
    let db = Arc::new(DbManager::new(&db_path).await.unwrap());
    let organizer = crate::tests::organizer_fixtures::make_test_organizer(db.clone()).await;

    let suffixed = tmp.path().join("Show - S01E01.001.nfo");
    tokio::fs::write(&suffixed, b"nfo").await.unwrap();
    let clean = tmp.path().join("Show - S01E01.nfo");

    let result = organizer
        .move_file_to_target(&suffixed, &clean, None)
        .await
        .unwrap();

    assert_eq!(result, clean, "the counter must be dropped once free");
    assert!(clean.exists());
    assert!(!suffixed.exists(), "the suffixed name must be vacated");
}

#[tokio::test]
async fn test_repeated_collision_increments_counter() {
    // A clean target that is occupied, with its `.001` already taken, must land at
    // `.002` — never nest to `.001.001`.
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test.db");
    let db = Arc::new(DbManager::new(&db_path).await.unwrap());
    let organizer = crate::tests::organizer_fixtures::make_test_organizer(db.clone()).await;

    let dst_path = tmp.path().join("Show - S01E01.mkv");
    tokio::fs::write(&dst_path, b"assigned occupant")
        .await
        .unwrap();
    let meta_ids = std::collections::HashMap::new();
    let dst_str = dst_path.to_string_lossy().to_string();
    db.insert_episode(crate::db::episodes::crud::InsertEpisodeParams {
        file_path: Some(&dst_str),
        status: "organized",
        ..crate::db::episodes::crud::InsertEpisodeParams::dummy(
            "inc_S01E01",
            "test_series",
            1,
            1,
            &meta_ids,
        )
    })
    .await
    .unwrap();
    // The first counter is taken by an existing file.
    tokio::fs::write(tmp.path().join("Show - S01E01.001.mkv"), b"first")
        .await
        .unwrap();

    let src = tmp.path().join("incoming.mkv");
    tokio::fs::write(&src, b"incoming").await.unwrap();

    let result = organizer
        .move_file_to_target(&src, &dst_path, None)
        .await
        .unwrap();

    assert_eq!(result, tmp.path().join("Show - S01E01.002.mkv"));
}

#[tokio::test]
async fn test_template_suffix_like_name_is_not_mangled() {
    // A template that legitimately renders ` (1)` (e.g. an OVA title) must keep it;
    // the counter is appended on top, not treated as one of ours.
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test.db");
    let db = Arc::new(DbManager::new(&db_path).await.unwrap());
    let organizer = crate::tests::organizer_fixtures::make_test_organizer(db.clone()).await;

    let mut config = crate::test_fixtures::config_with_dest_root(&db_path, tmp.path());
    config.organization.collision_rename_suffix =
        jumbie_shared::config::organization::CollisionRenameSuffix::ParenNumeric;
    let _ = db.save_organization_config(&config.organization).await;

    let dst_path = tmp.path().join("Show - S01E01 - OVA (1).mkv");
    tokio::fs::write(&dst_path, b"assigned occupant")
        .await
        .unwrap();
    let meta_ids = std::collections::HashMap::new();
    let dst_str = dst_path.to_string_lossy().to_string();
    db.insert_episode(crate::db::episodes::crud::InsertEpisodeParams {
        file_path: Some(&dst_str),
        status: "organized",
        ..crate::db::episodes::crud::InsertEpisodeParams::dummy(
            "ova_S01E01",
            "test_series",
            1,
            1,
            &meta_ids,
        )
    })
    .await
    .unwrap();

    let src = tmp.path().join("incoming.mkv");
    tokio::fs::write(&src, b"incoming").await.unwrap();
    let result = organizer
        .move_file_to_target(&src, &dst_path, None)
        .await
        .unwrap();

    assert_eq!(
        result,
        tmp.path().join("Show - S01E01 - OVA (1) (1).mkv"),
        "the template's own (1) must survive as part of the base name"
    );
}

#[tokio::test]
async fn test_is_path_unassigned_query() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test.db");
    let db = Arc::new(DbManager::new(&db_path).await.unwrap());

    let path_a = tmp.path().join("file_a.mkv");
    let path_b = tmp.path().join("file_b.mkv");
    let a_str = path_a.to_string_lossy().to_string();
    let _b_str = path_b.to_string_lossy().to_string();

    // No episodes at all — both should be unassigned
    assert!(db.is_path_unassigned(&path_a).await.unwrap());

    let meta_ids = std::collections::HashMap::new();
    db.insert_episode(crate::db::episodes::crud::InsertEpisodeParams {
        file_path: Some(&a_str),
        status: "organized",
        ..crate::db::episodes::crud::InsertEpisodeParams::dummy("ep1", "s1", 1, 1, &meta_ids)
    })
    .await
    .unwrap();

    // path_a is now assigned
    assert!(!db.is_path_unassigned(&path_a).await.unwrap());
    // path_b is still unassigned
    assert!(db.is_path_unassigned(&path_b).await.unwrap());

    // Set status to 'missing' — should be treated as unassigned
    sqlx::query("UPDATE episodes SET status = 'missing' WHERE episode_id = 'ep1'")
        .execute(db.get_pool())
        .await
        .unwrap();
    assert!(db.is_path_unassigned(&path_a).await.unwrap());
}

// RenamePlanCache tests

#[test]
fn test_filter_plan_by_skip_paths_removes_matching_sources() {
    let plan = vec![
        PlannedMove {
            src: std::path::PathBuf::from("/media/file_a.mkv"),
            dst: std::path::PathBuf::from("/organized/Show/file_a.mkv"),
            covered_episodes: vec![EpisodeSummary {
                episode_id: "ep1".into(),
                episode_num: 1,
            }],
            series_title: "Show".into(),
            series_id: "s1".into(),
            season_val: "1".into(),
            episode_title: "Ep1".into(),
            part_number: None,
            aux_kind: None,
        },
        PlannedMove {
            src: std::path::PathBuf::from("/media/file_b.mkv"),
            dst: std::path::PathBuf::from("/organized/Show/file_b.mkv"),
            covered_episodes: vec![EpisodeSummary {
                episode_id: "ep2".into(),
                episode_num: 2,
            }],
            series_title: "Show".into(),
            series_id: "s1".into(),
            season_val: "1".into(),
            episode_title: "Ep2".into(),
            part_number: None,
            aux_kind: None,
        },
    ];

    let skip = std::collections::HashSet::from([std::path::PathBuf::from("/media/file_a.mkv")]);

    let filtered = filter_plan_by_skip_paths(std::sync::Arc::new(plan), &skip);
    assert_eq!(filtered.len(), 1, "Only file_b should remain");
    assert_eq!(filtered[0].src.to_string_lossy(), "/media/file_b.mkv");
}

#[test]
fn test_filter_plan_by_skip_paths_keeps_all_when_skip_empty() {
    let plan = vec![PlannedMove {
        src: std::path::PathBuf::from("/media/file_a.mkv"),
        dst: std::path::PathBuf::from("/organized/Show/file_a.mkv"),
        covered_episodes: vec![EpisodeSummary {
            episode_id: "ep1".into(),
            episode_num: 1,
        }],
        series_title: "Show".into(),
        series_id: "s1".into(),
        season_val: "1".into(),
        episode_title: "Ep1".into(),
        part_number: None,
        aux_kind: None,
    }];
    let skip = std::collections::HashSet::new();
    let filtered = filter_plan_by_skip_paths(std::sync::Arc::new(plan), &skip);
    assert_eq!(filtered.len(), 1, "All files kept when skip is empty");
}

#[test]
fn test_filter_plan_by_skip_paths_returns_empty_when_all_skipped() {
    let plan = vec![PlannedMove {
        src: std::path::PathBuf::from("/media/file_a.mkv"),
        dst: std::path::PathBuf::from("/organized/Show/file_a.mkv"),
        covered_episodes: vec![EpisodeSummary {
            episode_id: "ep1".into(),
            episode_num: 1,
        }],
        series_title: "Show".into(),
        series_id: "s1".into(),
        season_val: "1".into(),
        episode_title: "Ep1".into(),
        part_number: None,
        aux_kind: None,
    }];
    let skip = std::collections::HashSet::from([std::path::PathBuf::from("/media/file_a.mkv")]);
    let filtered = filter_plan_by_skip_paths(std::sync::Arc::new(plan), &skip);
    assert!(filtered.is_empty(), "All files skipped");
}

#[tokio::test]
async fn test_rename_plan_cache_hit_returns_same_plan() {
    let cache = RenamePlanCache::new();
    let config = crate::test_fixtures::default_config();
    let mapping = MappingRule {
        target_title: "Cache Test".into(),
        ..Default::default()
    };
    let episodes: Vec<crate::db::EpisodeDetailRow> = Vec::new();
    let parts: Vec<(String, crate::db::EpisodePartRow)> = Vec::new();

    // First call — cache miss, computes
    let plan1 = cache
        .get_or_compute(
            "s1",
            &config,
            &mapping,
            &episodes,
            &parts,
            &std::collections::HashSet::new(),
        )
        .await
        .unwrap();
    assert!(plan1.is_empty(), "Empty series should produce empty plan");

    // Second call — cache hit, same result
    let plan2 = cache
        .get_or_compute(
            "s1",
            &config,
            &mapping,
            &episodes,
            &parts,
            &std::collections::HashSet::new(),
        )
        .await
        .unwrap();
    assert!(plan2.is_empty());
}

// Build an episode row backed by a real file: `compute_batch_plan` skips paths
// that do not exist, so the move only materialises when the file is on disk.
fn cache_episode_row(
    episode_id: &str,
    episode: i32,
    file_path: &std::path::Path,
) -> crate::db::EpisodeDetailRow {
    crate::db::EpisodeDetailRow {
        episode_id: episode_id.to_string(),
        season: Some(1),
        episode,
        status: None,
        file_path: Some(file_path.to_string_lossy().to_string()),
        release_title: None,
        size: 0,
        title: None,
        quality_profile_id: None,
        submitter: None,
        media_info: None,
        quick_hash: None,
        created_at: None,
        file_acquired_at: None,
        monitored: false,
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
        series_id: "s1".to_string(),
        download_link: None,
        monitor_override: false,
        original_path: None,
    }
}

#[tokio::test]
async fn test_rename_plan_cache_recomputes_when_inputs_change() {
    let tmp = tempfile::tempdir().unwrap();
    let src_a = tmp.path().join("a.mkv");
    let src_b = tmp.path().join("b.mkv");
    std::fs::write(&src_a, b"x").unwrap();
    std::fs::write(&src_b, b"x").unwrap();

    let cache = RenamePlanCache::new();
    let config = crate::test_fixtures::default_config();
    let mapping = MappingRule {
        target_title: "Cache Recompute".into(),
        ..Default::default()
    };
    let parts: Vec<(String, crate::db::EpisodePartRow)> = Vec::new();
    let skip = std::collections::HashSet::new();

    let episodes_a = vec![cache_episode_row("s1_1_1", 1, &src_a)];
    let plan_a = cache
        .get_or_compute("s1", &config, &mapping, &episodes_a, &parts, &skip)
        .await
        .unwrap();
    assert_eq!(plan_a.len(), 1);
    assert_eq!(plan_a[0].src, src_a);

    // Changing the episode's file path changes the fingerprint, so the plan must
    // be recomputed rather than served from the previous entry.
    let episodes_b = vec![cache_episode_row("s1_1_1", 1, &src_b)];
    let plan_b = cache
        .get_or_compute("s1", &config, &mapping, &episodes_b, &parts, &skip)
        .await
        .unwrap();
    assert_eq!(plan_b.len(), 1);
    assert_eq!(plan_b[0].src, src_b, "cache must reflect the changed input");
}

#[tokio::test]
async fn test_rename_plan_cache_series_isolation() {
    let cache = RenamePlanCache::new();
    let config = crate::test_fixtures::default_config();
    let mapping = MappingRule {
        target_title: "Isolation".into(),
        ..Default::default()
    };
    let episodes: Vec<crate::db::EpisodeDetailRow> = Vec::new();
    let parts: Vec<(String, crate::db::EpisodePartRow)> = Vec::new();
    let skip = std::collections::HashSet::new();

    cache
        .get_or_compute("s1", &config, &mapping, &episodes, &parts, &skip)
        .await
        .unwrap();
    cache
        .get_or_compute("s2", &config, &mapping, &episodes, &parts, &skip)
        .await
        .unwrap();

    cache.invalidate("s1").await;
    assert!(!cache.contains("s1").await, "s1 should be dropped");
    assert!(cache.contains("s2").await, "s2 should be untouched");
}

#[tokio::test]
async fn test_rename_plan_cache_invalidate_removes_entry() {
    let cache = RenamePlanCache::new();
    let config = crate::test_fixtures::default_config();
    let mapping = MappingRule {
        target_title: "Invalidate".into(),
        ..Default::default()
    };
    let episodes: Vec<crate::db::EpisodeDetailRow> = Vec::new();
    let parts: Vec<(String, crate::db::EpisodePartRow)> = Vec::new();

    cache
        .get_or_compute(
            "s1",
            &config,
            &mapping,
            &episodes,
            &parts,
            &std::collections::HashSet::new(),
        )
        .await
        .unwrap();
    assert!(cache.contains("s1").await, "Should be cached");

    // Explicit invalidate
    cache.invalidate("s1").await;
    assert!(!cache.contains("s1").await, "Should be removed");
}

#[tokio::test]
async fn test_rename_plan_cache_invalidate_all_clears_everything() {
    let cache = RenamePlanCache::new();
    let config = crate::test_fixtures::default_config();
    let mapping = MappingRule {
        target_title: "Invalidate All".into(),
        ..Default::default()
    };
    let episodes: Vec<crate::db::EpisodeDetailRow> = Vec::new();
    let parts: Vec<(String, crate::db::EpisodePartRow)> = Vec::new();

    cache
        .get_or_compute(
            "s1",
            &config,
            &mapping,
            &episodes,
            &parts,
            &std::collections::HashSet::new(),
        )
        .await
        .unwrap();
    cache
        .get_or_compute(
            "s2",
            &config,
            &mapping,
            &episodes,
            &parts,
            &std::collections::HashSet::new(),
        )
        .await
        .unwrap();

    cache.invalidate_all().await;
    assert!(!cache.contains("s1").await, "s1 removed");
    assert!(!cache.contains("s2").await, "s2 removed");
}

#[tokio::test]
async fn test_rename_plan_cache_unknown_series_returns_none() {
    let cache = RenamePlanCache::new();
    assert!(
        !cache.contains("nonexistent").await,
        "Unknown series should be absent"
    );
}

/// Render one absolute-mode episode through the full batch planner and return
/// the destination filename, so the tests exercise the real series-max →
/// template path rather than calling `apply_template` directly.
async fn absolute_dst_filename(
    episode_file_format: &str,
    episodes: &[i32],
    target_episode: i32,
) -> String {
    let tmp = tempfile::tempdir().unwrap();
    let mut config = crate::test_fixtures::default_config();
    config.organization.episode_file_format_absolute = episode_file_format.to_string();
    let mapping = MappingRule {
        target_title: "Long Show".into(),
        settings: jumbie_shared::mapping::SeriesSettings {
            absolute_numbering: Some(true),
            ..Default::default()
        },
        ..Default::default()
    };

    let mut rows = vec![];
    for &n in episodes {
        let src = tmp.path().join(format!("E{n}.mkv"));
        std::fs::write(&src, b"x").unwrap();
        rows.push(cache_episode_row(&format!("s1_abs{n}"), n, &src));
    }
    let plan =
        compute_batch_plan_with_aux(&config, &mapping, &rows, &[], &[], true, &HashSet::new())
            .unwrap();
    let m = plan
        .iter()
        .find(|m| m.first_episode_num() == target_episode)
        .unwrap();
    m.dst.file_name().unwrap().to_string_lossy().to_string()
}

#[tokio::test]
async fn test_episode_auto_matches_highest_episode() {
    // `:auto` widens to fit the series' highest episode, so an absolute series
    // past episode 99 pads consistently (E005 and E105 both 3 digits) instead of
    // the mixed E05 / E105 a fixed `:02` produces.
    let all: Vec<i32> = (1..=105).collect();
    assert_eq!(
        absolute_dst_filename("${series} - S01E${episode:auto}", &all, 5).await,
        "Long Show - S01E005.mkv"
    );
    assert_eq!(
        absolute_dst_filename("${series} - S01E${episode:auto}", &all, 105).await,
        "Long Show - S01E105.mkv"
    );

    // A 12-episode series needs two digits.
    let short: Vec<i32> = (1..=12).collect();
    assert_eq!(
        absolute_dst_filename("${series} - S01E${episode:auto}", &short, 5).await,
        "Long Show - S01E05.mkv"
    );

    // With no minimum width, a series topping out below 10 stays single-digit.
    let tiny: Vec<i32> = (1..=5).collect();
    assert_eq!(
        absolute_dst_filename("${series} - S01E${episode:auto}", &tiny, 5).await,
        "Long Show - S01E5.mkv"
    );
    // `:auto2` floors the width at two digits for the same series.
    assert_eq!(
        absolute_dst_filename("${series} - S01E${episode:auto2}", &tiny, 5).await,
        "Long Show - S01E05.mkv"
    );
    assert_eq!(
        absolute_dst_filename("${series} - S01E${episode:auto2}", &all, 105).await,
        "Long Show - S01E105.mkv"
    );
}

#[tokio::test]
async fn test_episode_bare_has_no_padding_but_fixed_02_does() {
    let all: Vec<i32> = (1..=105).collect();
    assert_eq!(
        absolute_dst_filename("${series} - S01E${episode}", &all, 5).await,
        "Long Show - S01E5.mkv"
    );
    assert_eq!(
        absolute_dst_filename("${series} - S01E${episode:02}", &all, 5).await,
        "Long Show - S01E05.mkv"
    );
    // A fixed width does not grow for the 3-digit episode either.
    assert_eq!(
        absolute_dst_filename("${series} - S01E${episode:02}", &all, 105).await,
        "Long Show - S01E105.mkv"
    );
}

#[tokio::test]
async fn test_episode_auto_uses_the_episodes_own_season() {
    let tmp = tempfile::tempdir().unwrap();
    let mut config = crate::test_fixtures::default_config();
    config.organization.episode_file_format =
        "${series} - S${season:02}E${episode:auto}".to_string();
    let mapping = MappingRule {
        target_title: "Two Season Show".into(),
        settings: jumbie_shared::mapping::SeriesSettings {
            absolute_numbering: Some(false),
            ..Default::default()
        },
        ..Default::default()
    };

    // Season 1 runs 1..9 (1 digit); season 2 runs 1..105 (3 digits). The auto
    // width must follow each episode's own season, not the series maximum.
    let mut rows = vec![];
    for (season, count) in [(1, 9), (2, 105)] {
        for n in 1..=count {
            let src = tmp.path().join(format!("S{season}E{n}.mkv"));
            std::fs::write(&src, b"x").unwrap();
            let mut r = cache_episode_row(&format!("s1_S{season}E{n}"), n, &src);
            r.season = Some(season);
            rows.push(r);
        }
    }
    let plan =
        compute_batch_plan_with_aux(&config, &mapping, &rows, &[], &[], false, &HashSet::new())
            .unwrap();

    let name = |season: i32, ep: i32| {
        let m = plan
            .iter()
            .find(|m| m.season_val == season.to_string() && m.first_episode_num() == ep)
            .unwrap();
        m.dst.file_name().unwrap().to_string_lossy().to_string()
    };
    assert_eq!(name(1, 5), "Two Season Show - S01E5.mkv");
    assert_eq!(name(2, 5), "Two Season Show - S02E005.mkv");
}

#[test]
fn test_season_folder_name_renders_the_active_format() {
    // The `_unmatched` restore on a mode switch must place files in the folder the
    // organizer would create — not a hardcoded "S01".
    let mut config = crate::test_fixtures::default_config();
    config.organization.season_folder_format = "Season ${season:auto2}".to_string();
    config.organization.season_folder_format_absolute = "Abs ${season:auto2}".to_string();

    let mut mapping = MappingRule {
        target_title: "Test Show".to_string(),
        settings: jumbie_shared::mapping::SeriesSettings {
            absolute_numbering: Some(false),
            ..Default::default()
        },
        ..Default::default()
    };
    let pad = crate::utils::TemplatePadOptions::default();

    assert_eq!(
        crate::file_manager::season_folder_name(&config, &mapping, 2, &pad),
        "Season 02"
    );

    // Absolute mode selects the absolute format and the canonical season.
    mapping.settings.absolute_numbering = Some(true);
    assert_eq!(
        crate::file_manager::season_folder_name(&config, &mapping, 1, &pad),
        "Abs 01"
    );
}
