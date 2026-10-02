//! `handle_mode_switch` restores files quarantined in `_unmatched/` into the
//! series' configured season folder.
//!
//! Two regressions are covered:
//!   1. the season folder was hardcoded (`S01` for absolute) or taken as the
//!      whole `S02E05` episode-id token (normal), instead of the series'
//!      configured season-folder format;
//!   2. the `_unmatched` lookup used the series *title* as the episode-id
//!      prefix, but episode IDs are keyed by the series UUID.

mod common;

use jumbie_shared::mapping::MappingRule;
use jumbie_shared::plugin::Capability;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tempfile::TempDir;

struct Restored {
    /// Kept alive so the temp dir outlives the assertions.
    _tmp: TempDir,
    series_dir: PathBuf,
    src: PathBuf,
    file_name: String,
}

/// Seed a series whose season folders use a clearly customised format plus a
/// quarantined `_unmatched` file linked to `{series_id}_{id_suffix}`, then run
/// `handle_mode_switch`.
async fn run(absolute: bool, id_suffix: &str, season: i32, episode: i32) -> Restored {
    let (_app, state, tmp) = common::setup_test_app().await;

    state
        .plugin_manager
        .write()
        .await
        .add_internal_plugin(Arc::new(common::MockMetadataPlugin {
            instance_id: "mock.meta.mode".to_string(),
            display_name: "Mode Mock".to_string(),
            capabilities: vec![
                Capability::MetadataProviderNormal,
                Capability::MetadataProviderAbsolute,
            ],
            series_identifier_label: Some("Mock ID".to_string()),
            series_name: "Mode Show".to_string(),
            overview: String::new(),
            aliases: vec![],
            episodes: vec![],
        }));

    let series_id = "mode-switch-series";
    let series_dir = tmp.path().join("Mode Show");
    let unmatched_dir = series_dir.join("_unmatched");
    std::fs::create_dir_all(&unmatched_dir).unwrap();
    let file_name = format!("Some.Release.{id_suffix}.mkv");
    let src = unmatched_dir.join(&file_name);
    std::fs::write(&src, b"content").unwrap();

    // A customised folder format so a hardcoded `S01` / raw `S02E05` is obvious.
    let mut mapping = MappingRule {
        target_title: "Mode Show".to_string(),
        series_id: series_id.to_string(),
        ..Default::default()
    };
    mapping.settings.path = Some(series_dir.to_string_lossy().to_string());
    mapping.settings.absolute_numbering = Some(absolute);
    mapping.settings.season_folder_format = Some("Season ${season:auto2}".to_string());
    mapping.settings.season_folder_format_absolute = Some("Season ${season:auto2}".to_string());
    mapping.settings.metadata_ids =
        HashMap::from([("mock.meta.mode".to_string(), "mock-id-1".to_string())]);
    state
        .db
        .upsert_series_mapping(series_id, &mapping)
        .await
        .unwrap();

    // The quarantined file is linked to an episode id (the FK parent must exist).
    let episode_key = format!("{series_id}_{id_suffix}");
    let meta = HashMap::new();
    state
        .db
        .insert_episode(jumbie::db::episodes::InsertEpisodeParams {
            numbering_mode: Some(absolute as i32),
            ..jumbie::db::episodes::InsertEpisodeParams::dummy(
                &episode_key,
                series_id,
                season,
                episode,
                &meta,
            )
        })
        .await
        .unwrap();
    state
        .db
        .associate_main_file(&episode_key, &src.to_string_lossy(), None)
        .await
        .unwrap();

    let path_str = series_dir.to_string_lossy().to_string();
    let _ = jumbie::api_routes::series::handle_mode_switch(
        &state,
        series_id,
        "Mode Show",
        Some(path_str.as_str()),
        absolute,
    )
    .await;

    Restored {
        _tmp: tmp,
        series_dir,
        src,
        file_name,
    }
}

fn entries(dir: &std::path::Path) -> Vec<std::ffi::OsString> {
    std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.file_name()))
        .collect()
}

#[tokio::test]
async fn absolute_mode_switch_restores_into_the_configured_season_folder() {
    let r = run(true, "ABS0005", 1, 5).await;
    let restored = r.series_dir.join("Season 01").join(&r.file_name);
    assert!(
        restored.exists(),
        "expected the file at {}; series dir contains {:?}",
        restored.display(),
        entries(&r.series_dir)
    );
    assert!(!r.src.exists(), "file should have left _unmatched");
}

#[tokio::test]
async fn normal_mode_switch_restores_into_that_seasons_folder() {
    // Regression: the season used to be the whole `S02E05` episode-id token.
    let r = run(false, "S02E05", 2, 5).await;
    let restored = r.series_dir.join("Season 02").join(&r.file_name);
    assert!(
        restored.exists(),
        "expected the file at {}; series dir contains {:?}",
        restored.display(),
        entries(&r.series_dir)
    );
    assert!(!r.src.exists(), "file should have left _unmatched");
}
