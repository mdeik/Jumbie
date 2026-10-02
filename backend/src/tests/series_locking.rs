//! Integration tests for the per-series modification lock, using a real AppState and
//! DB: lock lifecycle, rejection of concurrent locks, independent series, and
//! consistency of `is_series_locked` through realistic sequences.

use crate::api::AppState;
use crate::api::modifying_series::{is_series_locked, try_lock_series, unlock_series};
use crate::db::DbManager;
use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};

/// Minimal AppState with a real temp DB and a fresh lock set.
async fn setup_app_state() -> (Arc<AppState>, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test.db");
    let db = Arc::new(DbManager::new(&db_path).await.unwrap());

    // Write a minimal TOML config for ConfigManager
    let config_path = tmp.path().join("config.toml");
    std::fs::write(
        &config_path,
        format!(
            r#"database = "{}"
unknown_files_tmp_dir = "{}"
plugins_dir = "{}"
logs_dir = "{}"
"#,
            db_path.to_string_lossy().replace('\\', "/"),
            tmp.path().join("tmp").to_string_lossy().replace('\\', "/"),
            tmp.path()
                .join("plugins")
                .to_string_lossy()
                .replace('\\', "/"),
            tmp.path().join("logs").to_string_lossy().replace('\\', "/"),
        ),
    )
    .unwrap();

    let cfg = Arc::new(
        crate::config_manager::ConfigManager::new(db.clone(), config_path.to_str().unwrap())
            .await
            .unwrap(),
    );

    let state = Arc::new(AppState {
        cfg,
        db,
        downloader: None,
        notifications: None,
        organizer: None,
        ban_list: Arc::new(Mutex::new(HashMap::new())),
        is_reorganizing: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        failed_renames: Arc::new(RwLock::new(HashMap::new())),
        plugin_manager: Arc::new(RwLock::new(crate::plugins::PluginManager::new(
            std::path::PathBuf::from("/tmp/plugins"),
        ))),
        scan_queue: Arc::new(crate::scan_queue::ScanQueue::default()),
        processing_renames: Arc::new(RwLock::new(HashSet::new())),
        metadata_queue: Arc::new(crate::metadata_queue::MetadataQueue::new()),
        rename_queue_has_pending: Arc::new(RwLock::new(false)),
        rename_queue_trigger: Arc::new(tokio::sync::Notify::new()),
        search_queue: Arc::new(crate::search_queue::SearchQueue::new()),
        shutdown_token: tokio_util::sync::CancellationToken::new(),
        rate_limiter: crate::middleware::rate_limit::new_rate_limiter(),
        auth_cache: Arc::new(crate::middleware::auth_cache::AuthCache::new()),
        monitor_sweep_cancel: Arc::new(Mutex::new(None)),
        recalc_cancel: Arc::new(Mutex::new(None)),
        progress_tracker: crate::api::ProgressTracker::new(),
        modifying_series: Arc::new(RwLock::new(HashSet::new())),
        rename_plan_cache: crate::file_manager::RenamePlanCache::new(),
        logs_dir: "/tmp/logs".to_string(),
        log_level: "info".to_string(),
        log_buffer: Arc::new(std::sync::RwLock::new(jumbie_shared::types::LogRing::new())),
    });

    (state, tmp)
}

/// Insert a series mapping into the DB.
async fn insert_series(state: &Arc<AppState>, id: &str, path: &str) {
    let mapping = jumbie_shared::types::MappingRule {
        series_id: id.to_string(),
        target_title: id.to_string(),
        name: id.to_lowercase().replace(' ', "_"),
        settings: jumbie_shared::types::SeriesSettings {
            path: Some(path.to_string()),
            ..Default::default()
        },
        ..Default::default()
    };
    state.db.upsert_series_mapping(id, &mapping).await.unwrap();
}

/// Read a series mapping's path from the DB.
async fn get_series_path(state: &Arc<AppState>, id: &str) -> Option<String> {
    let mapping = state.db.get_series_mapping(id).await.unwrap();
    mapping.and_then(|m| m.settings.path)
}

#[tokio::test]
async fn test_lock_then_unlock_then_relock() {
    let (state, _tmp) = setup_app_state().await;

    assert!(try_lock_series(&state, "series-a").await);
    assert!(is_series_locked(&state, "series-a"));

    unlock_series(&state, "series-a").await;
    assert!(!is_series_locked(&state, "series-a"));

    assert!(try_lock_series(&state, "series-a").await);
    assert!(is_series_locked(&state, "series-a"));

    unlock_series(&state, "series-a").await;
}

#[tokio::test]
async fn test_double_lock_rejected() {
    let (state, _tmp) = setup_app_state().await;

    assert!(try_lock_series(&state, "series-1").await);
    assert!(!try_lock_series(&state, "series-1").await);
    assert!(is_series_locked(&state, "series-1"));

    unlock_series(&state, "series-1").await;
    assert!(!is_series_locked(&state, "series-1"));
}

#[tokio::test]
async fn test_unlock_nonexistent_is_safe() {
    let (state, _tmp) = setup_app_state().await;

    unlock_series(&state, "never-locked").await;
    assert!(!is_series_locked(&state, "never-locked"));
}

// Concurrent / independent series tests.

#[tokio::test]
async fn test_independent_series_do_not_interfere() {
    let (state, _tmp) = setup_app_state().await;

    assert!(try_lock_series(&state, "series-a").await);
    assert!(try_lock_series(&state, "series-b").await);
    assert!(try_lock_series(&state, "series-c").await);

    assert!(is_series_locked(&state, "series-a"));
    assert!(is_series_locked(&state, "series-b"));
    assert!(is_series_locked(&state, "series-c"));

    unlock_series(&state, "series-a").await;
    assert!(!is_series_locked(&state, "series-a"));
    assert!(is_series_locked(&state, "series-b"));
    assert!(is_series_locked(&state, "series-c"));

    unlock_series(&state, "series-b").await;
    assert!(!is_series_locked(&state, "series-b"));
    assert!(is_series_locked(&state, "series-c"));

    unlock_series(&state, "series-c").await;
    assert!(!is_series_locked(&state, "series-c"));

    assert!(try_lock_series(&state, "series-a").await);
    assert!(try_lock_series(&state, "series-b").await);
    assert!(try_lock_series(&state, "series-c").await);

    unlock_series(&state, "series-a").await;
    unlock_series(&state, "series-b").await;
    unlock_series(&state, "series-c").await;
}

#[tokio::test]
async fn test_many_concurrent_locks_and_unlocks() {
    let (state, _tmp) = setup_app_state().await;
    // Many independent lock/unlock cycles must not corrupt state.
    let n = 20;
    for i in 0..n {
        let id = format!("series-{}", i);
        assert!(try_lock_series(&state, &id).await);
    }
    for i in 0..n {
        assert!(is_series_locked(&state, &format!("series-{}", i)));
    }
    for i in (0..n).rev() {
        unlock_series(&state, &format!("series-{}", i)).await;
    }
    for i in 0..n {
        assert!(!is_series_locked(&state, &format!("series-{}", i)));
    }
    for i in 0..5 {
        assert!(try_lock_series(&state, &format!("series-{}", i)).await);
    }
    for i in 0..5 {
        unlock_series(&state, &format!("series-{}", i)).await;
    }
}

// Lock + DB operation integration tests.

#[tokio::test]
async fn test_lock_then_insert_series_in_db_then_unlock() {
    let (state, _tmp) = setup_app_state().await;

    // Mirrors remove_series: lock → DB delete → unlock.
    let series_id = "series-to-delete";
    insert_series(&state, series_id, "/media/tv/My Show").await;

    assert_eq!(
        get_series_path(&state, series_id).await,
        Some("/media/tv/My Show".to_string())
    );

    assert!(try_lock_series(&state, series_id).await);
    assert!(is_series_locked(&state, series_id));

    state.db.delete_series_data(series_id).await.unwrap();
    state.db.delete_series_mapping(series_id).await.unwrap();

    unlock_series(&state, series_id).await;
    assert!(!is_series_locked(&state, series_id));

    assert!(get_series_path(&state, series_id).await.is_none());
}

#[tokio::test]
async fn test_lock_then_update_path_in_db_then_unlock() {
    let (state, _tmp) = setup_app_state().await;

    // Mirrors update_series: lock → update DB path → unlock.
    let series_id = "series-to-move";
    insert_series(&state, series_id, "/media/tv/Old Path").await;

    assert!(try_lock_series(&state, series_id).await);

    let mut mapping = state
        .db
        .get_series_mapping(series_id)
        .await
        .unwrap()
        .unwrap();
    mapping.settings.path = Some("/media/tv/New Path".to_string());
    state
        .db
        .upsert_series_mapping(series_id, &mapping)
        .await
        .unwrap();

    // Release lock
    unlock_series(&state, series_id).await;

    assert_eq!(
        get_series_path(&state, series_id).await,
        Some("/media/tv/New Path".to_string())
    );
}

#[tokio::test]
async fn test_lock_prevents_second_lock_during_db_operation() {
    let (state, _tmp) = setup_app_state().await;

    let series_id = "series-contentious";
    insert_series(&state, series_id, "/media/tv/Some Show").await;

    assert!(try_lock_series(&state, series_id).await);

    // A second lock attempt (Operation B) must fail while A holds it.
    assert!(!try_lock_series(&state, series_id).await);
    assert!(is_series_locked(&state, series_id));

    unlock_series(&state, series_id).await;

    assert!(try_lock_series(&state, series_id).await);
    unlock_series(&state, series_id).await;
}

#[tokio::test]
async fn test_concurrent_lock_attempt_only_one_succeeds() {
    let (state, _tmp) = setup_app_state().await;
    let series_id = "series-race".to_string();

    let state_1 = state.clone();
    let id_1 = series_id.clone();
    let task_1 = tokio::spawn(async move { try_lock_series(&state_1, &id_1).await });

    let state_2 = state.clone();
    let id_2 = series_id.clone();
    let task_2 = tokio::spawn(async move { try_lock_series(&state_2, &id_2).await });

    let result_1 = task_1.await.unwrap();
    let result_2 = task_2.await.unwrap();

    assert!(
        result_1 != result_2,
        "Concurrent lock on same series: expected one success and one failure, got ({}, {})",
        result_1,
        result_2
    );

    assert!(is_series_locked(&state, &series_id));

    assert!(!try_lock_series(&state, &series_id).await);

    unlock_series(&state, &series_id).await;
    assert!(!is_series_locked(&state, &series_id));

    assert!(try_lock_series(&state, &series_id).await);
    unlock_series(&state, &series_id).await;
}

#[tokio::test]
async fn test_concurrent_lock_different_series_both_succeed() {
    let (state, _tmp) = setup_app_state().await;

    // Two tasks locking different series should both succeed.
    let state_1 = state.clone();
    let task_1 = tokio::spawn(async move { try_lock_series(&state_1, "series-a").await });

    let state_2 = state.clone();
    let task_2 = tokio::spawn(async move { try_lock_series(&state_2, "series-b").await });

    assert!(task_1.await.unwrap());
    assert!(task_2.await.unwrap());

    assert!(is_series_locked(&state, "series-a"));
    assert!(is_series_locked(&state, "series-b"));

    unlock_series(&state, "series-a").await;
    unlock_series(&state, "series-b").await;
}

#[tokio::test]
async fn test_is_series_locked_consistency_after_many_operations() {
    let (state, _tmp) = setup_app_state().await;

    insert_series(&state, "series-1", "/media/tv/Show 1").await;
    insert_series(&state, "series-2", "/media/tv/Show 2").await;
    insert_series(&state, "series-3", "/media/tv/Show 3").await;

    assert!(try_lock_series(&state, "series-1").await);
    assert!(try_lock_series(&state, "series-3").await);

    assert!(is_series_locked(&state, "series-1"));
    assert!(!is_series_locked(&state, "series-2"));
    assert!(is_series_locked(&state, "series-3"));

    assert!(try_lock_series(&state, "series-2").await);
    assert!(is_series_locked(&state, "series-2"));

    unlock_series(&state, "series-1").await;
    assert!(!is_series_locked(&state, "series-1"));
    assert!(is_series_locked(&state, "series-2"));
    assert!(is_series_locked(&state, "series-3"));

    unlock_series(&state, "series-2").await;
    unlock_series(&state, "series-3").await;

    assert!(!is_series_locked(&state, "series-1"));
    assert!(!is_series_locked(&state, "series-2"));
    assert!(!is_series_locked(&state, "series-3"));

    // Locking must not touch DB state.
    assert_eq!(
        get_series_path(&state, "series-1").await,
        Some("/media/tv/Show 1".to_string())
    );
    assert_eq!(
        get_series_path(&state, "series-2").await,
        Some("/media/tv/Show 2".to_string())
    );
    assert_eq!(
        get_series_path(&state, "series-3").await,
        Some("/media/tv/Show 3".to_string())
    );
}
