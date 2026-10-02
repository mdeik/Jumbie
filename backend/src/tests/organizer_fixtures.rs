// Deliberately NOT part of `tests::fixtures`: that file is re-included by the
// integration-test `common` module via `#[path]`, where `crate::` refers to the test
// binary, not the jumbie lib. This helper touches lib-internal types (plugins, state),
// so it must only be compiled into the lib's own `#[cfg(test)]` tests.

use crate::db::DbManager;
use crate::organizer::ContentOrganizer;
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Constructs a minimal `ContentOrganizer` for unit tests.
pub async fn make_test_organizer(db: Arc<DbManager>) -> ContentOrganizer {
    let dummy_plugin_dir = std::env::temp_dir().join("jb_test_plugins");
    let _ = std::fs::create_dir_all(&dummy_plugin_dir);
    let plugin_manager = Arc::new(RwLock::new(crate::plugins::PluginManager::new(
        dummy_plugin_dir,
    )));
    let download_manager = Arc::new(RwLock::new(
        crate::plugins::downloaders::DownloadManager::new(plugin_manager.clone()),
    ));
    let state_manager = Arc::new(tokio::sync::Mutex::new(
        crate::state::FileStateManager::new(db.clone()),
    ));
    let notifications = Arc::new(RwLock::new(
        crate::plugins::notifiers::NotifierManager::new(plugin_manager.clone()),
    ));

    ContentOrganizer {
        db,
        downloader: download_manager,
        _state_manager: state_manager,
        notifications,
        plugin_manager,
        shutdown_token: tokio_util::sync::CancellationToken::new(),
        modifying_series: Arc::new(RwLock::new(HashSet::new())),
    }
}
