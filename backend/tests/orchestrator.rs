use jumbie::db::autoresolve::RejectionTarget;
use jumbie::db::download_queue::AddToDownloadQueueParams;
use jumbie::download_orchestrator::download::EnrichAndEnqueueParams;
use jumbie::organizer::ContentOrganizer;
use jumbie::plugins::downloaders::DownloadManager;
use jumbie::plugins::{PluginInstance, PluginManager};
use jumbie_shared::mapping::{MappingRule, SeasonOverride, SeriesSettings};
use jumbie_shared::types::EpisodeIntention;
use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::Mutex;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

// Helpers

const CORRECT_BTIH_HASH: &str = "0123456789abcdef0123456789abcdef01234567";

/// Build a magnet link containing a 40-char BTIH hash.
fn magnet_with_correct_hash() -> String {
    format!(
        "magnet:?xt=urn:btih:{}&dn=Test+Show+S01E01&tr=http://tracker.test/announce",
        CORRECT_BTIH_HASH
    )
}

/// Build a plain HTTP link with no BTIH.
fn url_without_btih() -> String {
    "https://example.com/downloads/test.torrent".to_string()
}

/// Shared state between a MockDownloader and the test that controls it.
/// Stored behind `Arc<Mutex<>>` so the mock can read it and the test can write it.
#[derive(Default)]
struct MockState {
    /// Hashes to report as completed from `get_completed_downloads`.
    completed: Vec<String>,
    /// Hash → content_path mapping for `get_download_content_path`.
    content_paths: HashMap<String, String>,
    /// Hash → status string (e.g. "uploading") for `get_download_status`.
    statuses: HashMap<String, String>,
    /// Hash → progress (0.0–1.0) for `get_download_progress`.
    progress: HashMap<String, f32>,
    /// Hash → hard-failure reason for `get_download_failure`.
    failures: HashMap<String, String>,
}

/// Minimal mock downloader plugin that:
///   - Accepts any download (`add_download` → Ok)
///   - Reports state from the shared `MockState`
///   - Has no protocol filters (accepts anything)
struct MockDownloader {
    id: String,
    /// Shared mutable state controlled by the test.
    state: Arc<Mutex<MockState>>,
}

#[async_trait::async_trait]
impl PluginInstance for MockDownloader {
    fn instance_id(&self) -> &str {
        &self.id
    }

    fn plugin_info(&self) -> plugin_sdk::traits::PluginTypeInfo {
        plugin_sdk::traits::PluginTypeInfo {
            display_name: "MockDownloader".to_string(),
            version: "1.0.0".to_string(),
            author: jumbie_shared::plugin::JUMBIE_AUTHOR.to_string(),
            description: "Mock downloader for integration tests".to_string(),
            capabilities: vec![
                jumbie_shared::plugin::Capability::Downloader,
                jumbie_shared::plugin::Capability::CanPauseResume,
            ],
            supported_protocols: None,
            series_identifier_label: None,
            series_identifier_placeholder: None,
            rate_limit: None,
            supports_test: false,
        }
    }

    fn supported_protocols(&self) -> Option<&[String]> {
        None // accept all protocols
    }

    fn priority(&self) -> i32 {
        10
    }

    async fn call(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> anyhow::Result<serde_json::Value> {
        match method {
            "add_download" => Ok(serde_json::Value::Null),
            "get_completed_downloads" => {
                let state = self.state.lock().unwrap();
                Ok(serde_json::to_value(&state.completed)?)
            }
            "get_download_progress" => {
                let hash: String = serde_json::from_value(params.unwrap_or_default())?;
                let state = self.state.lock().unwrap();
                Ok(serde_json::to_value(state.progress.get(&hash).copied())?)
            }
            "get_download_status" => {
                let hash: String = serde_json::from_value(params.unwrap_or_default())?;
                let state = self.state.lock().unwrap();
                Ok(serde_json::to_value(state.statuses.get(&hash).cloned())?)
            }
            "get_download_failure" => {
                let hash: String = serde_json::from_value(params.unwrap_or_default())?;
                let state = self.state.lock().unwrap();
                Ok(serde_json::to_value(state.failures.get(&hash).cloned())?)
            }
            "get_download_content_path" => {
                let hash: String = serde_json::from_value(params.unwrap_or_default())?;
                let state = self.state.lock().unwrap();
                let path = state.content_paths.get(&hash).ok_or_else(|| {
                    anyhow::anyhow!("no content path configured for hash={}", hash)
                })?;
                Ok(serde_json::to_value(path)?)
            }

            "get_download_id_by_name" => Ok(serde_json::json!(None::<String>)),
            "get_download_path" => Ok(serde_json::json!(Some("/tmp/downloads"))),
            "get_organizer_path" => Ok(serde_json::json!(Some("/tmp/organizer"))),
            "delete_download" => Ok(serde_json::Value::Null),
            "test_connection" => Ok(serde_json::Value::Null),
            _ => anyhow::bail!("unsupported mock method: {}", method),
        }
    }
}

/// Minimal source plugin for `auto_search_missing`: returns a fixed entry list for
/// every auto-search call, ignoring the query.
struct MockSource {
    entries: Vec<serde_json::Value>,
}

#[async_trait::async_trait]
impl PluginInstance for MockSource {
    fn instance_id(&self) -> &str {
        "mock_source"
    }

    fn plugin_info(&self) -> plugin_sdk::traits::PluginTypeInfo {
        plugin_sdk::traits::PluginTypeInfo {
            display_name: "MockSource".to_string(),
            version: "1.0.0".to_string(),
            author: jumbie_shared::plugin::JUMBIE_AUTHOR.to_string(),
            description: "Mock source for integration tests".to_string(),
            capabilities: vec![
                jumbie_shared::plugin::Capability::FeedProvider,
                jumbie_shared::plugin::Capability::AutomaticSearch,
            ],
            supported_protocols: None,
            series_identifier_label: None,
            series_identifier_placeholder: None,
            rate_limit: None,
            supports_test: false,
        }
    }

    fn supported_protocols(&self) -> Option<&[String]> {
        None
    }

    fn priority(&self) -> i32 {
        10
    }

    async fn call(
        &self,
        method: &str,
        _params: Option<serde_json::Value>,
    ) -> anyhow::Result<serde_json::Value> {
        match method {
            "auto_search" => Ok(serde_json::json!({
                "entries": self.entries,
                "queries": ["mock"]
            })),
            _ => anyhow::bail!("unsupported mock source method: {}", method),
        }
    }
}

/// Build a test ContentOrganizer with a mock downloader registered.
/// Returns (organizer, db, downloader, _tmpdir) so the caller can manipulate
/// DB state, inspect downloader state, and run process_downloads().
async fn setup_with_mock_downloader() -> (
    ContentOrganizer,
    Arc<jumbie::db::DbManager>,
    Arc<tokio::sync::RwLock<DownloadManager>>,
    tempfile::TempDir,
) {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let db_path = temp_dir.path().join("test.db");

    // Register internal plugin factories so the system initialises correctly.
    jumbie::plugins::internal::register_all().await;

    let plugin_manager = Arc::new(RwLock::new(PluginManager::new(
        temp_dir.path().join("plugins"),
    )));

    let db = Arc::new(jumbie::db::DbManager::new(&db_path).await.expect("DB init"));

    let modifying_series: Arc<RwLock<HashSet<String>>> = Arc::new(RwLock::new(HashSet::new()));
    let organizer = ContentOrganizer::new(
        db_path.to_str().unwrap(),
        db.clone(),
        plugin_manager,
        tokio_util::sync::CancellationToken::new(),
        modifying_series,
    )
    .await
    .expect("ContentOrganizer init");
    let downloader = organizer.downloader();

    // Register the mock downloader AFTER ContentOrganizer::new, because that
    // method calls load_internal_plugins which wipes out any pre-registered
    // plugins and rebuilds from the DB plugin config.
    {
        let mock_state = Arc::new(Mutex::new(MockState::default()));
        {
            let mut state = mock_state.lock().unwrap();
            state.content_paths.insert(
                CORRECT_BTIH_HASH.to_string(),
                "/tmp/downloads/Test.Show.S01E01.mkv".to_string(),
            );
        }
        let mock: Arc<dyn PluginInstance> = Arc::new(MockDownloader {
            id: "mock_dl".to_string(),
            state: mock_state.clone(),
        });
        organizer
            .plugin_manager()
            .write()
            .await
            .add_internal_plugin(mock);
    }

    (organizer, db, downloader, temp_dir)
}

/// Like `setup_with_mock_downloader`, but returns the `MockState` handle so
/// the test can configure completion, content paths, etc. before triggering
/// `process_downloads()`.
async fn setup_with_controllable_mock() -> (
    ContentOrganizer,
    Arc<jumbie::db::DbManager>,
    Arc<tokio::sync::RwLock<DownloadManager>>,
    Arc<Mutex<MockState>>,
    tempfile::TempDir,
) {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let db_path = temp_dir.path().join("test.db");

    jumbie::plugins::internal::register_all().await;

    let plugin_manager = Arc::new(RwLock::new(PluginManager::new(
        temp_dir.path().join("plugins"),
    )));

    let db = Arc::new(jumbie::db::DbManager::new(&db_path).await.expect("DB init"));

    let modifying_series: Arc<RwLock<HashSet<String>>> = Arc::new(RwLock::new(HashSet::new()));
    let organizer = ContentOrganizer::new(
        db_path.to_str().unwrap(),
        db.clone(),
        plugin_manager,
        CancellationToken::new(),
        modifying_series,
    )
    .await
    .expect("ContentOrganizer init");
    let downloader = organizer.downloader();

    let mock_state = Arc::new(Mutex::new(MockState::default()));
    {
        let mock: Arc<dyn PluginInstance> = Arc::new(MockDownloader {
            id: "mock_dl".to_string(),
            state: mock_state.clone(),
        });
        organizer
            .plugin_manager()
            .write()
            .await
            .add_internal_plugin(mock);
    }

    (organizer, db, downloader, mock_state, temp_dir)
}

/// Helper to insert a test episode with standard parameters.
/// Optionally sets download_id (SSoT for process_download_queue dispatch).
async fn insert_test_episode(db: &jumbie::db::DbManager, episode_id: &str, series_key: &str) {
    db.insert_episode(jumbie::db::episodes::InsertEpisodeParams {
        title: Some("Test Show S01E01"),
        ..jumbie::db::episodes::InsertEpisodeParams::dummy(
            episode_id,
            series_key,
            1,
            1,
            &std::collections::HashMap::new(),
        )
    })
    .await
    .expect("insert_episode");
}

/// Helper to insert a test episode WITH a download_id.
async fn insert_test_episode_with_hash(
    db: &jumbie::db::DbManager,
    episode_id: &str,
    series_key: &str,
    _download_hash: &str,
) {
    db.insert_episode(jumbie::db::episodes::InsertEpisodeParams {
        title: Some("Test Show S01E01"),
        ..jumbie::db::episodes::InsertEpisodeParams::dummy(
            episode_id,
            series_key,
            1,
            1,
            &std::collections::HashMap::new(),
        )
    })
    .await
    .expect("insert_episode");
}

/// Helper to add a download to the queue and mark it as Downloading.
async fn queue_download(
    db: &jumbie::db::DbManager,
    title: &str,
    download_hash: &str,
    series: &str,
    season: &str,
    episode_id: &str,
    intentions_json: Option<&str>,
) {
    let queue_id = db
        .add_to_download_queue(AddToDownloadQueueParams {
            media_name: title,
            media_link: &magnet_with_hash(download_hash),
            series_title: series,
            series_id: "",
            seasons: &[season.parse().unwrap_or(1)],
            episodes: &[1],
            episode_id: Some(episode_id),
            score: 100,
            is_user_requested: false,
            is_season_pack: false,
            category: "",
            multi_targets: None,
            episode_intentions: intentions_json.map(|s| s.to_string()).as_deref(),
            source_pub_date: None,
            download_id: download_hash,
            scoring_size_bytes: None,
            scoring_seeders: None,
            scoring_episode_count: None,
            submitter: None,
            quality_profile_id: None,
            version: 1,
        })
        .await
        .expect("add_to_download_queue");
    let queue_id = match queue_id {
        jumbie_shared::types::AddQueueResult::Added { .. } => {
            let items = db.get_queued_items().await.unwrap();
            items.first().map(|i| i.id).unwrap()
        }
        _ => panic!("expected Added"),
    };
    db.update_queue_item_status(queue_id, "Downloading", Some(download_hash), None, None)
        .await
        .expect("update status");
}

// Download Queue: dispatch via episodes.download_id & same-cycle guard.
// episodes.download_id is the canonical identifier for dispatching queued items
// to the download client; items must not be re-dispatched in the same cycle.

#[tokio::test]
async fn test_download_id_used_when_episode_has_hash() {
    let (organizer, db, _dl, _tmp) = setup_with_mock_downloader().await;
    let episode_id = "test_dl_id_used_ep";

    // Queue item has a valid download_id (stored on the queue item at queue time).
    // process_download_queue reads it via get_episode_hash to dispatch.
    db.insert_episode(jumbie::db::episodes::InsertEpisodeParams {
        episode_id,
        series_id: "test-show",
        season: 1,
        episode: 1,
        file_path: None,
        title: Some("Test Show S01E01"),
        quality_profile_id: None,
        status: "missing",
        meta_date: None,
        est_date: None,
        metadata_ids: &HashMap::new(),
        description: None,
        runtime: None,
        image_url: None,
        metadata_source: None,
        numbering_mode: None,
    })
    .await
    .expect("insert_episode");

    // Queue item references the episode but has no downloader_id yet.
    // download_id is stored on the queue item and read by get_episode_hash.
    db.add_to_download_queue(AddToDownloadQueueParams {
        media_name: "Test Show S01E01",
        media_link: &magnet_with_correct_hash(),
        series_title: "Test Show",
        series_id: "",
        seasons: &[1],
        episodes: &[1],
        episode_id: Some(episode_id),
        score: 100,
        is_user_requested: false,
        is_season_pack: false,
        category: "",
        multi_targets: None,
        episode_intentions: None,
        source_pub_date: None,
        download_id: CORRECT_BTIH_HASH,
        scoring_size_bytes: None,
        scoring_seeders: None,
        scoring_episode_count: None,
        submitter: None,
        quality_profile_id: None,
        version: 1,
    })
    .await
    .expect("add_to_download_queue");

    organizer
        .process_downloads()
        .await
        .expect("process_downloads");

    let items = db.get_downloading_items().await.unwrap();
    let item = items
        .iter()
        .find(|i| i.episode_id.as_deref() == Some(episode_id))
        .expect("after process_downloads, item must be Downloading");
    assert_eq!(
        item.downloader_id.as_deref(),
        Some(CORRECT_BTIH_HASH),
        "episodes.download_id must be used when item has no downloader_id yet",
    );

    // episodes.download_id must remain unchanged.
    let db_hash = db.get_episode_hash(episode_id).await.unwrap();
    assert_eq!(
        db_hash.as_deref(),
        Some(CORRECT_BTIH_HASH),
        "episodes.download_id must remain unchanged",
    );
}

#[tokio::test]
async fn test_db_hash_fallback_when_no_btih_in_link() {
    let (organizer, db, _dl, _tmp) = setup_with_mock_downloader().await;
    let episode_id = "test_no_btih_ep";

    // Queue item has a valid download_id but the link is plain HTTP with no urn:btih.
    // The code reads from download_queue.download_id exclusively.
    db.insert_episode(jumbie::db::episodes::InsertEpisodeParams {
        episode_id,
        series_id: "test-show",
        season: 1,
        episode: 2,
        file_path: None,
        title: Some("Test Show S01E02"),
        quality_profile_id: None,
        status: "missing",
        meta_date: None,
        est_date: None,
        metadata_ids: &HashMap::new(),
        description: None,
        runtime: None,
        image_url: None,
        metadata_source: None,
        numbering_mode: None,
    })
    .await
    .expect("insert_episode");

    db.add_to_download_queue(AddToDownloadQueueParams {
        media_name: "Test Show S01E02",
        media_link: &url_without_btih(),
        series_title: "Test Show",
        series_id: "",
        seasons: &[1],
        episodes: &[2],
        episode_id: Some(episode_id),
        score: 90,
        is_user_requested: false,
        is_season_pack: false,
        category: "",
        multi_targets: None,
        episode_intentions: None,
        source_pub_date: None,
        download_id: CORRECT_BTIH_HASH,
        scoring_size_bytes: None,
        scoring_seeders: None,
        scoring_episode_count: None,
        submitter: None,
        quality_profile_id: None,
        version: 1,
    })
    .await
    .expect("add_to_download_queue");

    organizer
        .process_downloads()
        .await
        .expect("process_downloads");

    let items = db.get_downloading_items().await.unwrap();
    let item = items
        .iter()
        .find(|i| i.episode_id.as_deref() == Some(episode_id))
        .expect("after process_downloads, item must be Downloading");
    assert_eq!(
        item.downloader_id.as_deref(),
        Some(CORRECT_BTIH_HASH),
        "episodes.download_id must be used when no BTIH is in the link",
    );

    let db_hash = db.get_episode_hash(episode_id).await.unwrap();
    assert_eq!(
        db_hash.as_deref(),
        Some(CORRECT_BTIH_HASH),
        "episodes.download_id must remain unchanged",
    );
}

#[tokio::test]
async fn test_download_id_missing_stays_queued() {
    let (organizer, db, _dl, _tmp) = setup_with_mock_downloader().await;
    let episode_id = "test_missing_dl_id_ep";

    // Episode has no download_id — the source plugin did not provide one.
    db.insert_episode(jumbie::db::episodes::InsertEpisodeParams {
        episode_id,
        series_id: "test-show",
        season: 1,
        episode: 3,
        file_path: None,
        title: Some("Test Show S01E03"),
        quality_profile_id: None,
        status: "missing",
        meta_date: None,
        est_date: None,
        metadata_ids: &HashMap::new(),
        description: None,
        runtime: None,
        image_url: None,
        metadata_source: None,
        numbering_mode: None,
    })
    .await
    .expect("insert_episode");

    // Queue item with no download_id on the episode.
    db.add_to_download_queue(AddToDownloadQueueParams {
        media_name: "Test Show S01E03",
        media_link: &magnet_with_correct_hash(),
        series_title: "Test Show",
        series_id: "",
        episodes: &[3],
        seasons: &[1],
        episode_id: Some(episode_id),
        score: 80,
        is_user_requested: false,
        is_season_pack: false,
        category: "",
        multi_targets: None,
        episode_intentions: None,
        source_pub_date: None,
        download_id: "",
        scoring_size_bytes: None,
        scoring_seeders: None,
        scoring_episode_count: None,
        submitter: None,
        quality_profile_id: None,
        version: 1,
    })
    .await
    .expect("add_to_download_queue");

    // Act: process_downloads
    organizer
        .process_downloads()
        .await
        .expect("process_downloads");

    // Item should be dispatched (download_id found on queue item)
    // The item must still be Queued — without a download_id the orchestrator
    // cannot dispatch it to a download client.
    let queued = db.get_queued_items().await.unwrap();
    let item = queued
        .iter()
        .find(|i| i.episode_id.as_deref() == Some(episode_id))
        .expect("item must still be Queued");
    assert_eq!(
        item.status.as_str(),
        "Queued",
        "item without episodes.download_id must stay Queued"
    );

    // Also ensure it did NOT appear in downloading items.
    let downloading = db.get_downloading_items().await.unwrap();
    assert!(
        !downloading
            .iter()
            .any(|i| i.episode_id.as_deref() == Some(episode_id)),
        "item without download_id must not be Downloading",
    );
}

#[tokio::test]
async fn test_fresh_dispatch_not_re_queued_in_same_cycle() {
    let (organizer, db, _dl, _tmp) = setup_with_mock_downloader().await;
    let episode_id = "test_same_cycle_ep";

    db.insert_episode(jumbie::db::episodes::InsertEpisodeParams {
        episode_id,
        series_id: "test-show",
        season: 1,
        episode: 3,
        file_path: None,
        title: Some("Test Show S01E03"),
        quality_profile_id: None,
        status: "missing",
        meta_date: None,
        est_date: None,
        metadata_ids: &HashMap::new(),
        description: None,
        runtime: None,
        image_url: None,
        metadata_source: None,
        numbering_mode: None,
    })
    .await
    .expect("insert_episode");

    db.add_to_download_queue(AddToDownloadQueueParams {
        media_name: "Test Show S01E03",
        media_link: &magnet_with_correct_hash(),
        series_title: "Test Show",
        series_id: "",
        episodes: &[3],
        seasons: &[1],
        episode_id: Some(episode_id),
        score: 100,
        is_user_requested: false,
        is_season_pack: false,
        category: "",
        multi_targets: None,
        episode_intentions: None,
        source_pub_date: None,
        download_id: CORRECT_BTIH_HASH,
        scoring_size_bytes: None,
        scoring_seeders: None,
        scoring_episode_count: None,
        submitter: None,
        quality_profile_id: None,
        version: 1,
    })
    .await
    .expect("add_to_download_queue");

    organizer
        .process_downloads()
        .await
        .expect("process_downloads");

    // Item should be dispatched (download_id found on queue item)
    let items = db.get_downloading_items().await.unwrap();
    let item = items
        .iter()
        .find(|i| i.episode_id.as_deref() == Some(episode_id))
        .expect("after 1st cycle, item must be Downloading");
    assert_eq!(item.downloader_id.as_deref(), Some(CORRECT_BTIH_HASH),);

    // Second cycle: the mock has a content path but no progress/status, so the
    // progress gate keeps the item Downloading.
    organizer
        .process_downloads()
        .await
        .expect("2nd process_downloads");

    // Still Downloading: content path exists, file isn't on disk, progress unknown —
    // the progress gate keeps it Downloading instead of burning Organizing retries.
    let all = db.get_download_queue().await.unwrap();
    let item = all
        .iter()
        .find(|i| i.episode_id.as_deref() == Some(episode_id))
        .expect("item still exists");
    assert_eq!(
        item.status, "Downloading",
        "with content path always available, item stays Downloading until file appears",
    );
    assert_eq!(
        item.downloader_id.as_deref(),
        Some(CORRECT_BTIH_HASH),
        "downloader_id must be preserved"
    );
}

#[tokio::test]
async fn test_dispatch_calls_add_download_and_stores_client_id() {
    let (organizer, db, _dl, mock_state, _tmp) = setup_with_controllable_mock().await;
    let episode_id = "dispatch_add_dl_ep";
    let download_hash = "aabbccddee00112233445566778899aabbccddee";

    // Seed content path so dispatch-time check succeeds
    {
        let mut state = mock_state.lock().unwrap();
        state.content_paths.insert(
            download_hash.to_string(),
            "/tmp/downloads/Test.Show.S01E01.mkv".to_string(),
        );
    }

    insert_test_episode_with_hash(&db, episode_id, "test-show", download_hash).await;

    db.add_to_download_queue(AddToDownloadQueueParams {
        media_name: "Test Show S01E01",
        media_link: &magnet_with_hash(download_hash),
        series_title: "Test Show",
        series_id: "",
        seasons: &[1],
        episodes: &[1],
        episode_id: Some(episode_id),
        score: 100,
        is_user_requested: false,
        is_season_pack: false,
        category: "",
        multi_targets: None,
        episode_intentions: None,
        source_pub_date: None,
        download_id: download_hash,
        scoring_size_bytes: None,
        scoring_seeders: None,
        scoring_episode_count: None,
        submitter: None,
        quality_profile_id: None,
        version: 1,
    })
    .await
    .expect("add_to_download_queue");

    // Act: dispatch via process_download_queue
    organizer
        .process_download_queue()
        .await
        .expect("process_download_queue");

    let items = db.get_downloading_items().await.unwrap();
    let item = items
        .iter()
        .find(|i| i.episode_id.as_deref() == Some(episode_id))
        .expect("item must be in Downloading state");

    assert_eq!(
        item.status, "Downloading",
        "item must transition to Downloading after dispatch",
    );
    assert_eq!(
        item.downloader_id.as_deref(),
        Some(download_hash),
        "downloader_id must be set to the BTIH hash",
    );
    assert_eq!(
        item.client_id.as_deref(),
        Some("mock_dl"),
        "client_id must be set to the mock downloader's plugin ID, proving \
         add_download was called",
    );
}

#[tokio::test]
async fn test_dispatch_works_without_episode_id_series_level_download() {
    // Regression: series-level (Path B) downloads queue with episode_id = NULL
    // (smart-link resolves the episode from files later).  Dispatch used to
    // derive the torrent hash via a per-episode DB lookup, which returned None
    // for NULL episode_id — leaving the item stuck in "Queued" forever.  The
    // hash now comes from the queue item's own download_id column.
    let (organizer, db, _dl, mock_state, _tmp) = setup_with_controllable_mock().await;
    let download_hash = "99887766554433221100aabbccddeeff00112233";

    // Seed content path so dispatch-time check succeeds.
    {
        let mut state = mock_state.lock().unwrap();
        state.content_paths.insert(
            download_hash.to_string(),
            "/tmp/downloads/Test.Show.S01E01.mkv".to_string(),
        );
    }

    // episode_id = None + no episode context: the series-level (Path B)
    // download case — smart-link resolves the episode from files later.
    db.add_to_download_queue(AddToDownloadQueueParams {
        media_name: "Test Show S01E01",
        media_link: &magnet_with_hash(download_hash),
        series_title: "Test Show",
        series_id: "test-show",
        seasons: &[],
        episodes: &[],
        episode_id: None,
        score: 100,
        is_user_requested: false,
        is_season_pack: false,
        category: "",
        multi_targets: None,
        episode_intentions: None,
        source_pub_date: None,
        download_id: download_hash,
        scoring_size_bytes: None,
        scoring_seeders: None,
        scoring_episode_count: None,
        submitter: None,
        quality_profile_id: None,
        version: 1,
    })
    .await
    .expect("add_to_download_queue");

    // Act: dispatch via process_download_queue
    organizer
        .process_download_queue()
        .await
        .expect("process_download_queue");

    let items = db.get_downloading_items().await.unwrap();
    let item = items
        .iter()
        .find(|i| i.media_name == "Test Show S01E01")
        .expect("series-level item must be in Downloading state after dispatch");
    assert_eq!(item.status, "Downloading");
    assert_eq!(
        item.downloader_id.as_deref(),
        Some(download_hash),
        "downloader_id must be set to the BTIH hash"
    );
    assert_eq!(
        item.client_id.as_deref(),
        Some("mock_dl"),
        "add_download must have been called"
    );
}

// Smart-Link: season pack offset, multi-season guard, series_id extraction.
//
// Covers post-download smart-linking for season packs: episode offset applied
// (S02E101 → local ep 1 at offset=100), multi-season guard, multi-season + alias
// (S05←S02, pack S01+S02 → only S02 kept), and series_id extraction.

/// Helper: build a minimal SeasonOverride with just the fields under test.
fn make_season_override(
    season: &str,
    alias_season_number: Option<u32>,
    episode_offset: Option<i32>,
    episode_start: Option<i32>,
    episode_end: Option<i32>,
) -> SeasonOverride {
    SeasonOverride {
        season: season.to_string(),
        episode_start,
        episode_end,
        cell_count: None,
        episode_offset,
        alias_season_number,
        search_format: None,
        aliases: vec![],
        reg_patterns: vec![],
    }
}

/// Helper: insert an episode row so it exists when smart_link tries to link to it.
async fn insert_ep_for_linking(
    db: &jumbie::db::DbManager,
    episode_id: &str,
    _series_title: &str,
    season: i32,
    episode: i32,
) {
    db.insert_episode(jumbie::db::episodes::InsertEpisodeParams {
        episode_id,
        series_id: "test-series",
        season,
        episode,
        file_path: None,
        title: None,
        quality_profile_id: None,
        status: "missing",
        meta_date: None,
        est_date: None,
        metadata_ids: &HashMap::new(),
        description: None,
        runtime: None,
        image_url: None,
        metadata_source: None,
        numbering_mode: None,
    })
    .await
    .expect("insert_episode");
}

/// Helper: create a magnet link with a specific BTIH hash for test downloads.
fn magnet_with_hash(hash: &str) -> String {
    format!(
        "magnet:?xt=urn:btih:{}&dn=Test+Pack&tr=http://tracker.test/announce",
        hash
    )
}

#[tokio::test]
async fn test_smart_link_applies_episode_offset() {
    let (organizer, db, _dl, mock_state, tmp) = setup_with_controllable_mock().await;
    let series_id = "offset-test-series";
    let content_dir = tmp.path().join("Test.Show.S02.COMPLETE");
    let content_path_str = content_dir.to_string_lossy().to_string();
    let download_hash = "offset_test_hash_12345";

    // Arrange: series mapping with S05←S02 alias + offset 100
    let mut season_overrides = HashMap::new();
    season_overrides.insert(
        "5".to_string(),
        make_season_override("5", Some(2), Some(100), Some(1), Some(100)),
    );
    let mapping = MappingRule {
        target_title: "Test Show".to_string(),
        name: "test_show".to_string(),
        series_id: series_id.to_string(),
        quality_profile: None,
        release_profile: None,
        qb_category: None,
        filters: None,
        scoring: None,
        hidden_in_library: false,
        settings: SeriesSettings {
            aliases: vec![],
            absolute_numbering: Some(false),
            season: season_overrides,
            season_absolute: HashMap::new(),
            reg_patterns: vec![],
            season_folder_format: None,
            episode_file_format: None,
            season_folder_format_absolute: None,
            episode_file_format_absolute: None,
            flatten_season_folders: None,
            rename_episodes: None,
            search_format: None,
            search_format_absolute: None,
            path: None,
            monitor_mode: None,
            metadata_ids: HashMap::new(),
            metadata_last_synced_at: HashMap::new(),
            last_known_dir_mtimes: HashMap::new(),
        },
    };
    db.upsert_series_mapping(series_id, &mapping)
        .await
        .expect("upsert_series_mapping");

    // Pre-insert episode rows so smart_link can link files to them.
    // With offset 100: source ep 101 → local ep 1, source ep 150 → local ep 50
    insert_ep_for_linking(&db, &format!("{}_S05E01", series_id), "Test Show", 5, 1).await;
    insert_ep_for_linking(&db, &format!("{}_S05E50", series_id), "Test Show", 5, 50).await;

    tokio::fs::create_dir_all(&content_dir)
        .await
        .expect("create content dir");
    tokio::fs::write(content_dir.join("Test.Show.S02E101.mkv"), b"dummy ep 101")
        .await
        .expect("write file");
    tokio::fs::write(content_dir.join("Test.Show.S02E150.mkv"), b"dummy ep 150")
        .await
        .expect("write file");
    // An unparseable .txt file should become "unknown" — not linked, not an offense
    tokio::fs::write(content_dir.join("some_metadata.txt"), b"notes")
        .await
        .expect("write file");

    {
        let mut state = mock_state.lock().unwrap();
        state.completed.push(download_hash.to_string());
        state
            .statuses
            .insert(download_hash.to_string(), "uploading".to_string());
        state
            .content_paths
            .insert(download_hash.to_string(), content_path_str.clone());
        state.progress.insert(download_hash.to_string(), 1.0);
    }

    // Intentions encode the offset at queue time: source ep 101 → local ep 1.
    let intentions = vec![
        EpisodeIntention {
            episode_num: 1,
            source_episode_num: 101,
            episode_id: format!("{}_S05E01", series_id),
            score: 100,
            keep: true,
        },
        EpisodeIntention {
            episode_num: 50,
            source_episode_num: 150,
            episode_id: format!("{}_S05E50", series_id),
            score: 100,
            keep: true,
        },
    ];
    let intentions_json = serde_json::to_string(&intentions).unwrap();
    let base_ep_id = format!("{}_S05E01", series_id);
    queue_download(
        &db,
        "Test Show S02 Complete",
        download_hash,
        "Test Show",
        "5",
        &base_ep_id,
        Some(&intentions_json),
    )
    .await;

    organizer
        .process_downloads()
        .await
        .expect("process_downloads");

    // S02E101.mkv → local ep 1 (101 - 100)
    let ep1 = db
        .get_episode_by_id(&format!("{}_S05E01", series_id))
        .await
        .expect("get ep1")
        .expect("ep1 should exist after smart_link");
    assert!(
        ep1.file_path
            .as_ref()
            .is_some_and(|p| p.contains("S02E101")),
        "S02E101.mkv should be linked to S05E01. file_path={:?}",
        ep1.file_path
    );
    // assign_file_to_episode sets status to 'organized', but relink sets it to
    // 'downloaded'. Either is valid — we just need it non-NULL and non-'missing'.
    assert!(
        ep1.status.as_deref() == Some("organized") || ep1.status.as_deref() == Some("downloaded"),
        "ep1 status should be organized or downloaded, got {:?}",
        ep1.status
    );

    // S02E150.mkv → local ep 50 (150 - 100)
    let ep50 = db
        .get_episode_by_id(&format!("{}_S05E50", series_id))
        .await
        .expect("get ep50")
        .expect("ep50 should exist after smart_link");
    assert!(
        ep50.file_path
            .as_ref()
            .is_some_and(|p| p.contains("S02E150")),
        "S02E150.mkv should be linked to S05E50. file_path={:?}",
        ep50.file_path
    );
    assert!(
        ep50.status.as_deref() == Some("organized") || ep50.status.as_deref() == Some("downloaded"),
        "ep50 status should be organized or downloaded, got {:?}",
        ep50.status
    );
}

// ── Assigned downloads must assign their release-numbered files ─────────────
//
// Intentions are recorded at queue time in LOCAL (DB) numbering. Releases are
// numbered in SOURCE space (local + the season's episode_offset), so smart-link
// must translate before matching. Regression for an offset-numbered series:
// S23 offset 1155, local E14 = release 1169. Before the fix, every file missed
// the intention lookup, was classified as unexpected, and was deleted under the
// shipped
// `unexpected_files_handling = "delete"` default while the item was marked
// Completed.

/// `unexpected_files_handling = "delete"` matches the shipped default and is what
/// destroyed the files in the field.
async fn save_delete_unexpected_config(db: &jumbie::db::DbManager) {
    let general = jumbie_shared::config::GeneralConfig {
        unexpected_files_handling: "delete".to_string(),
        unneeded_episodes_handling: "keep".to_string(),
        ..Default::default()
    };
    db.save_general_config(&general)
        .await
        .expect("save general config");
}

/// Spec for [`queue_enriched_download`], grouped to keep the helper's argument
/// count down.
struct EnrichedDownload<'a> {
    series_id: &'a str,
    series_title: &'a str,
    media_name: &'a str,
    seasons: &'a [i32],
    episodes: &'a [i32],
    episode_id: &'a str,
    download_hash: &'a str,
}

async fn queue_enriched_download(db: &jumbie::db::DbManager, spec: EnrichedDownload<'_>) {
    ContentOrganizer::enrich_and_enqueue(EnrichAndEnqueueParams {
        db,
        notifications: None,
        media_name: spec.media_name,
        media_link: &magnet_with_hash(spec.download_hash),
        source: "Nyaa",
        series_title: spec.series_title,
        series_id: spec.series_id,
        seasons: spec.seasons,
        episodes: spec.episodes,
        episode_id: Some(spec.episode_id),
        score: 100,
        is_user_requested: false,
        is_manual: false,
        is_season_pack: false,
        category: "Series",
        multi_targets: None,
        episode_intentions: None,
        quality_profile_id: None,
        meta_date: None,
        source_pub_date: None,
        metadata_ids: None,
        description: None,
        runtime: None,
        image_url: None,
        download_id: spec.download_hash,
        title_override: None,
        submitter: Some("MockFansub"),
        version: None,
        scoring_size_bytes: None,
        scoring_seeders: None,
        scoring_episode_count: None,
    })
    .await
    .expect("enrich_and_enqueue");

    let queue_id = db
        .get_queued_items()
        .await
        .unwrap()
        .first()
        .map(|i| i.id)
        .expect("queue item");
    db.update_queue_item_status(
        queue_id,
        "Downloading",
        Some(spec.download_hash),
        None,
        None,
    )
    .await
    .expect("update status");
}

async fn queue_series_level_download(
    db: &jumbie::db::DbManager,
    series_id: &str,
    series_title: &str,
    media_name: &str,
    season: i32,
    download_hash: &str,
) {
    // Series-level downloads are user-picked releases: manual + user-initiated.
    db.add_manual_download_to_queue(AddToDownloadQueueParams {
        media_name,
        media_link: &magnet_with_hash(download_hash),
        series_title,
        series_id,
        seasons: &[season],
        episodes: &[],
        episode_id: None,
        score: 100,
        is_user_requested: true,
        is_season_pack: false,
        category: "Series",
        multi_targets: None,
        episode_intentions: None,
        source_pub_date: None,
        download_id: download_hash,
        scoring_size_bytes: None,
        scoring_seeders: None,
        scoring_episode_count: None,
        submitter: Some("MockFansub"),
        quality_profile_id: None,
        version: 1,
    })
    .await
    .expect("add_to_download_queue");
    let queue_id = db
        .get_queued_items()
        .await
        .unwrap()
        .first()
        .map(|i| i.id)
        .unwrap();
    db.update_queue_item_status(queue_id, "Downloading", Some(download_hash), None, None)
        .await
        .expect("update status");
}

fn mark_mock_download_complete(
    state: &Arc<Mutex<MockState>>,
    download_hash: &str,
    content_path: &str,
) {
    let mut s = state.lock().unwrap();
    s.completed.push(download_hash.to_string());
    s.statuses
        .insert(download_hash.to_string(), "uploading".to_string());
    s.content_paths
        .insert(download_hash.to_string(), content_path.to_string());
    s.progress.insert(download_hash.to_string(), 1.0);
}

async fn insert_offset_mapping(
    db: &jumbie::db::DbManager,
    series_id: &str,
    library: &std::path::Path,
    season: &str,
    offset: i32,
) {
    insert_season_alias_mapping(db, series_id, library, season, None, offset).await;
}

async fn insert_season_alias_mapping(
    db: &jumbie::db::DbManager,
    series_id: &str,
    library: &std::path::Path,
    season: &str,
    alias_season_number: Option<u32>,
    offset: i32,
) {
    let mut season_overrides = HashMap::new();
    season_overrides.insert(
        season.to_string(),
        make_season_override(season, alias_season_number, Some(offset), None, None),
    );
    let mapping = MappingRule {
        target_title: "Star Voyage".to_string(),
        name: "star_voyage".to_string(),
        series_id: series_id.to_string(),
        settings: SeriesSettings {
            absolute_numbering: Some(false),
            season: season_overrides,
            path: Some(library.to_string_lossy().to_string()),
            ..Default::default()
        },
        ..Default::default()
    };
    db.upsert_series_mapping(series_id, &mapping)
        .await
        .expect("upsert mapping");

    let mut org = db.get_organization_config().await.unwrap_or_default();
    org.destination_roots = vec![library.to_path_buf().into()];
    db.save_organization_config(&org)
        .await
        .expect("save org config");
}

#[tokio::test]
async fn test_assigned_offset_download_assigns_source_numbered_file() {
    let (organizer, db, _dl, mock_state, tmp) = setup_with_controllable_mock().await;
    let series_id = "starvoyage-offset-regression";
    let library = tmp.path().join("library");
    let content_dir = tmp.path().join("Star.Voyage.S23.Complete");
    let content_path_str = content_dir.to_string_lossy().to_string();
    let download_hash = "starvoyage_offset_regression_hash";

    save_delete_unexpected_config(&db).await;
    insert_offset_mapping(&db, series_id, &library, "23", 1155).await;

    // Queue through the production path: LOCAL episode 14 -> S23E14, offset 1155.
    let ep_id = format!("{}_S23E14", series_id);
    queue_enriched_download(
        &db,
        EnrichedDownload {
            series_id,
            series_title: "Star Voyage",
            media_name: "[MockFansub] Star Voyage - 1169 (1080p)",
            seasons: &[23],
            episodes: &[14],
            episode_id: &ep_id,
            download_hash,
        },
    )
    .await;

    // The torrent delivers SOURCE-numbered files.
    let grab_file = content_dir.join("[MockFansub] Star Voyage - 1169 (1080p).mkv");
    tokio::fs::create_dir_all(&content_dir).await.unwrap();
    tokio::fs::write(&grab_file, b"dummy episode 1169")
        .await
        .unwrap();
    mark_mock_download_complete(&mock_state, download_hash, &content_path_str);

    organizer
        .process_downloads()
        .await
        .expect("process_downloads");

    let ep = db
        .get_episode_by_id(&ep_id)
        .await
        .expect("get episode")
        .expect("S23E14 row must exist");
    let assigned = ep
        .file_path
        .clone()
        .expect("S23E14 must be assigned the 1169 release after smart-link");
    assert!(
        std::path::Path::new(&assigned).exists(),
        "assigned file must exist on disk (got '{assigned}')"
    );
    assert!(
        assigned.contains("1169") || assigned.contains("S23E14"),
        "S23E14 must be linked to the 1169 download (got '{assigned}')"
    );
}

// A single-target assigned download trusts the queue-time assignment: with
// exactly one intended episode and one video file, the parsed number must not
// prevent the assignment.
#[tokio::test]
async fn test_single_target_download_assigns_mismatched_number() {
    let (organizer, db, _dl, mock_state, tmp) = setup_with_controllable_mock().await;
    let series_id = "single-target-regression";
    let library = tmp.path().join("library-single");
    let content_dir = tmp.path().join("Single.Target.Download");
    let content_path_str = content_dir.to_string_lossy().to_string();
    let download_hash = "single_target_mismatch_hash";

    save_delete_unexpected_config(&db).await;
    insert_offset_mapping(&db, series_id, &library, "1", 0).await;

    let ep_id = format!("{}_S01E05", series_id);
    queue_enriched_download(
        &db,
        EnrichedDownload {
            series_id,
            series_title: "Star Voyage",
            media_name: "Show - 999",
            seasons: &[1],
            episodes: &[5],
            episode_id: &ep_id,
            download_hash,
        },
    )
    .await;

    let video = content_dir.join("Show - 999.mkv");
    tokio::fs::create_dir_all(&content_dir).await.unwrap();
    tokio::fs::write(&video, b"dummy mismatched number")
        .await
        .unwrap();
    mark_mock_download_complete(&mock_state, download_hash, &content_path_str);

    organizer
        .process_downloads()
        .await
        .expect("process_downloads");

    let ep = db
        .get_episode_by_id(&ep_id)
        .await
        .expect("get episode")
        .expect("S01E05 row must exist");
    let assigned = ep
        .file_path
        .clone()
        .expect("single-target download must be assigned to S01E05 despite the parsed number");
    assert!(
        std::path::Path::new(&assigned).exists(),
        "assigned file must exist on disk (got '{assigned}')"
    );
}

// Multiple files that are all parts of one episode must coalesce into a single
// episode (main-file associations), not be treated as separate/unexpected files.
#[tokio::test]
async fn test_single_target_multipart_files_form_one_episode() {
    let (organizer, db, _dl, mock_state, tmp) = setup_with_controllable_mock().await;
    let series_id = "multipart-single-target";
    let library = tmp.path().join("library-multipart");
    let content_dir = tmp.path().join("Multipart.Download");
    let content_path_str = content_dir.to_string_lossy().to_string();
    let download_hash = "multipart_single_target_hash";

    save_delete_unexpected_config(&db).await;
    insert_offset_mapping(&db, series_id, &library, "1", 0).await;

    let ep_id = format!("{}_S01E05", series_id);
    queue_enriched_download(
        &db,
        EnrichedDownload {
            series_id,
            series_title: "Star Voyage",
            media_name: "Show - 05 (multi-part)",
            seasons: &[1],
            episodes: &[5],
            episode_id: &ep_id,
            download_hash,
        },
    )
    .await;

    tokio::fs::create_dir_all(&content_dir).await.unwrap();
    for name in ["Show - 05 part 1.mkv", "Show - 05 part 2.mkv"] {
        tokio::fs::write(content_dir.join(name), b"dummy part")
            .await
            .unwrap();
    }
    mark_mock_download_complete(&mock_state, download_hash, &content_path_str);

    organizer
        .process_download_queue()
        .await
        .expect("process_download_queue");

    let parts: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM episode_files WHERE episode_id = ? AND kind = 'main'",
    )
    .bind(&ep_id)
    .fetch_one(db.get_pool())
    .await
    .expect("episode main-file count");
    assert_eq!(
        parts, 2,
        "both part files must register under the single intended episode {ep_id}"
    );
}

// The single-target fast path must coalesce parts even when their parsed numbers
// do not match the intended episode (the source-number match never fires here).
#[tokio::test]
async fn test_single_target_multipart_fast_path_ignores_parsed_numbers() {
    let (organizer, db, _dl, mock_state, tmp) = setup_with_controllable_mock().await;
    let series_id = "multipart-fast-path";
    let library = tmp.path().join("library-multipart-fast");
    let content_dir = tmp.path().join("Multipart.Fast.Download");
    let content_path_str = content_dir.to_string_lossy().to_string();
    let download_hash = "multipart_fast_path_hash";

    save_delete_unexpected_config(&db).await;
    insert_offset_mapping(&db, series_id, &library, "1", 0).await;

    let ep_id = format!("{}_S01E05", series_id);
    queue_enriched_download(
        &db,
        EnrichedDownload {
            series_id,
            series_title: "Star Voyage",
            media_name: "Show - 05 (multi-part)",
            seasons: &[1],
            episodes: &[5],
            episode_id: &ep_id,
            download_hash,
        },
    )
    .await;

    tokio::fs::create_dir_all(&content_dir).await.unwrap();
    for name in ["Show - 900 part 1.mkv", "Show - 900 part 2.mkv"] {
        tokio::fs::write(content_dir.join(name), b"dummy part")
            .await
            .unwrap();
    }
    mark_mock_download_complete(&mock_state, download_hash, &content_path_str);

    organizer
        .process_download_queue()
        .await
        .expect("process_download_queue");

    let parts: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM episode_files WHERE episode_id = ? AND kind = 'main'",
    )
    .bind(&ep_id)
    .fetch_one(db.get_pool())
    .await
    .expect("episode main-file count");
    assert_eq!(
        parts, 2,
        "the fast path must coalesce parts into {ep_id} despite mismatched numbers"
    );
}

// The single-target fast path must NOT fire for a real multi-episode pack.
#[tokio::test]
async fn test_multi_episode_pack_links_each_file_by_number() {
    let (organizer, db, _dl, mock_state, tmp) = setup_with_controllable_mock().await;
    let series_id = "multi-episode-pack";
    let library = tmp.path().join("library-pack");
    let content_dir = tmp.path().join("Pack.Download");
    let content_path_str = content_dir.to_string_lossy().to_string();
    let download_hash = "multi_episode_pack_hash";

    save_delete_unexpected_config(&db).await;
    insert_offset_mapping(&db, series_id, &library, "1", 0).await;

    let ep1 = format!("{}_S01E01", series_id);
    let ep2 = format!("{}_S01E02", series_id);
    queue_enriched_download(
        &db,
        EnrichedDownload {
            series_id,
            series_title: "Star Voyage",
            media_name: "Show - 01-02 pack",
            seasons: &[1],
            episodes: &[1, 2],
            episode_id: &ep1,
            download_hash,
        },
    )
    .await;

    tokio::fs::create_dir_all(&content_dir).await.unwrap();
    tokio::fs::write(content_dir.join("Show.S01E01.mkv"), b"ep1")
        .await
        .unwrap();
    tokio::fs::write(content_dir.join("Show.S01E02.mkv"), b"ep2")
        .await
        .unwrap();
    mark_mock_download_complete(&mock_state, download_hash, &content_path_str);

    organizer
        .process_downloads()
        .await
        .expect("process_downloads");

    for (ep_id, marker) in [(&ep1, "S01E01"), (&ep2, "S01E02")] {
        let ep = db
            .get_episode_by_id(ep_id)
            .await
            .expect("get episode")
            .unwrap_or_else(|| panic!("{marker} row must exist"));
        let assigned = ep
            .file_path
            .clone()
            .unwrap_or_else(|| panic!("{marker} must be assigned its own file"));
        assert!(
            assigned.contains(marker),
            "{marker} must link its own file (got '{assigned}')"
        );
    }
}

// An assigned download whose intended video files all fail to link must be kept
// for review, not deleted as unexpected content.
#[tokio::test]
async fn test_assigned_download_with_no_matching_file_is_kept() {
    let (organizer, db, _dl, mock_state, tmp) = setup_with_controllable_mock().await;
    let series_id = "assigned-nomatch-guard";
    let library = tmp.path().join("library-guard");
    let content_dir = tmp.path().join("Guard.Download");
    let content_path_str = content_dir.to_string_lossy().to_string();
    let download_hash = "assigned_nomatch_guard_hash";

    save_delete_unexpected_config(&db).await;
    insert_offset_mapping(&db, series_id, &library, "1", 0).await;

    let ep1 = format!("{}_S01E01", series_id);
    // Two intended episodes: not a single-target download, so the fast path cannot
    // mask the mismatch.
    queue_enriched_download(
        &db,
        EnrichedDownload {
            series_id,
            series_title: "Star Voyage",
            media_name: "Show - 01-02 pack",
            seasons: &[1],
            episodes: &[1, 2],
            episode_id: &ep1,
            download_hash,
        },
    )
    .await;

    tokio::fs::create_dir_all(&content_dir).await.unwrap();
    let stray = content_dir.join("Show - 900.mkv");
    tokio::fs::write(&stray, b"mismatched").await.unwrap();
    mark_mock_download_complete(&mock_state, download_hash, &content_path_str);

    organizer
        .process_downloads()
        .await
        .expect("process_downloads");

    assert!(
        stray.exists(),
        "an assigned download whose files cannot be placed must not be deleted"
    );
    let reason: Option<String> =
        sqlx::query_scalar("SELECT reason FROM unmatched_files WHERE file_path = ?")
            .bind(stray.to_string_lossy().to_string())
            .fetch_optional(db.get_pool())
            .await
            .expect("unmatched_files query")
            .flatten();
    assert_eq!(
        reason.as_deref(),
        Some("unmatched"),
        "the file must be recorded for review"
    );
    let (status, error): (String, Option<String>) =
        sqlx::query_as("SELECT status, error_message FROM download_queue ORDER BY id DESC LIMIT 1")
            .fetch_one(db.get_pool())
            .await
            .expect("queue row");
    assert_eq!(
        status, "Review",
        "an assigned download that placed nothing needs review, not Completed/Failed"
    );
    assert!(error.is_none(), "review is not an error: {error:?}");
}

// When some intended files link, the remaining unmatched files are ordinary
// unexpected content and still follow the configured handling.
#[tokio::test]
async fn test_partially_matched_download_still_applies_unexpected_handling() {
    let (organizer, db, _dl, mock_state, tmp) = setup_with_controllable_mock().await;
    let series_id = "partial-match-handling";
    let library = tmp.path().join("library-partial");
    let content_dir = tmp.path().join("Partial.Download");
    let content_path_str = content_dir.to_string_lossy().to_string();
    let download_hash = "partial_match_handling_hash";

    save_delete_unexpected_config(&db).await;
    insert_offset_mapping(&db, series_id, &library, "1", 0).await;

    let ep1 = format!("{}_S01E01", series_id);
    queue_enriched_download(
        &db,
        EnrichedDownload {
            series_id,
            series_title: "Star Voyage",
            media_name: "Show - 01-02 pack",
            seasons: &[1],
            episodes: &[1, 2],
            episode_id: &ep1,
            download_hash,
        },
    )
    .await;

    tokio::fs::create_dir_all(&content_dir).await.unwrap();
    tokio::fs::write(content_dir.join("Show.S01E01.mkv"), b"ep1")
        .await
        .unwrap();
    let stray = content_dir.join("Show - 900.mkv");
    tokio::fs::write(&stray, b"unexpected").await.unwrap();
    mark_mock_download_complete(&mock_state, download_hash, &content_path_str);

    organizer
        .process_downloads()
        .await
        .expect("process_downloads");

    let ep = db.get_episode_by_id(&ep1).await.unwrap().unwrap();
    assert!(
        ep.file_path.is_some(),
        "the matching file must still be assigned"
    );
    assert!(
        !stray.exists(),
        "an unexpected file in a partially matched pack still follows delete handling"
    );
}

// One intended episode, but several non-part video files: the fast path must stay
// off and the ambiguous files must be kept for review.
#[tokio::test]
async fn test_single_target_multiple_non_part_files_kept_for_review() {
    let (organizer, db, _dl, mock_state, tmp) = setup_with_controllable_mock().await;
    let series_id = "single-target-multi-file";
    let library = tmp.path().join("library-multi-file");
    let content_dir = tmp.path().join("Multi.File.Download");
    let content_path_str = content_dir.to_string_lossy().to_string();
    let download_hash = "single_target_multi_file_hash";

    save_delete_unexpected_config(&db).await;
    insert_offset_mapping(&db, series_id, &library, "1", 0).await;

    let ep_id = format!("{}_S01E05", series_id);
    queue_enriched_download(
        &db,
        EnrichedDownload {
            series_id,
            series_title: "Star Voyage",
            media_name: "Show - 05 (ambiguous)",
            seasons: &[1],
            episodes: &[5],
            episode_id: &ep_id,
            download_hash,
        },
    )
    .await;

    tokio::fs::create_dir_all(&content_dir).await.unwrap();
    let a = content_dir.join("Show - 900.mkv");
    let b = content_dir.join("Show - 901.mkv");
    tokio::fs::write(&a, b"x").await.unwrap();
    tokio::fs::write(&b, b"y").await.unwrap();
    mark_mock_download_complete(&mock_state, download_hash, &content_path_str);

    organizer
        .process_downloads()
        .await
        .expect("process_downloads");

    assert!(
        a.exists() && b.exists(),
        "multiple non-part files under one target are ambiguous and must be kept"
    );
    let reviewed: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM unmatched_files WHERE file_path IN (?, ?)")
            .bind(a.to_string_lossy().to_string())
            .bind(b.to_string_lossy().to_string())
            .fetch_one(db.get_pool())
            .await
            .expect("unmatched count");
    assert_eq!(reviewed, 2, "both ambiguous files must be kept for review");
}

// A keep=false overspill intention must be routed by that intention even while a
// single keep-target parts download is in flight.
#[tokio::test]
async fn test_single_target_with_overspill_keeps_only_intended_parts() {
    let (organizer, db, _dl, mock_state, tmp) = setup_with_controllable_mock().await;
    let series_id = "single-target-overspill";
    let library = tmp.path().join("library-overspill");
    let content_dir = tmp.path().join("Overspill.Download");
    let content_path_str = content_dir.to_string_lossy().to_string();
    let download_hash = "single_target_overspill_hash";

    save_delete_unexpected_config(&db).await;
    insert_offset_mapping(&db, series_id, &library, "1", 0).await;

    let ep5 = format!("{}_S01E05", series_id);
    let ep6 = format!("{}_S01E06", series_id);
    // The queue's episode_id FK needs the row to exist before enqueue.
    insert_ep_for_linking(&db, &ep5, "Star Voyage", 1, 5).await;
    let intentions = format!(
        r#"[{{"episode_num":5,"source_episode_num":5,"episode_id":"{ep5}","score":100,"keep":true}},{{"episode_num":6,"source_episode_num":6,"episode_id":"{ep6}","score":100,"keep":false}}]"#
    );
    db.add_to_download_queue(AddToDownloadQueueParams {
        media_name: "Show - 05 (parts + overspill)",
        media_link: &magnet_with_hash(download_hash),
        series_title: "Star Voyage",
        series_id,
        seasons: &[1],
        episodes: &[5],
        episode_id: Some(&ep5),
        score: 100,
        is_user_requested: false,
        is_season_pack: false,
        category: "Series",
        multi_targets: None,
        episode_intentions: Some(&intentions),
        source_pub_date: None,
        download_id: download_hash,
        scoring_size_bytes: None,
        scoring_seeders: None,
        scoring_episode_count: None,
        submitter: Some("MockFansub"),
        quality_profile_id: None,
        version: 1,
    })
    .await
    .expect("add_to_download_queue");
    let queue_id = db
        .get_queued_items()
        .await
        .unwrap()
        .first()
        .map(|i| i.id)
        .unwrap();
    db.update_queue_item_status(queue_id, "Downloading", Some(download_hash), None, None)
        .await
        .expect("update status");

    tokio::fs::create_dir_all(&content_dir).await.unwrap();
    let overspill = content_dir.join("Show - 06 part 1.mkv");
    for name in ["Show - 05 part 1.mkv", "Show - 05 part 2.mkv"] {
        tokio::fs::write(content_dir.join(name), b"part")
            .await
            .unwrap();
    }
    tokio::fs::write(&overspill, b"overspill").await.unwrap();
    mark_mock_download_complete(&mock_state, download_hash, &content_path_str);

    organizer
        .process_download_queue()
        .await
        .expect("process_download_queue");

    let kept_parts: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM episode_files WHERE episode_id = ? AND kind = 'main'",
    )
    .bind(&ep5)
    .fetch_one(db.get_pool())
    .await
    .expect("ep5 main files");
    assert_eq!(
        kept_parts, 2,
        "the intended episode's parts must be registered"
    );

    let overspill_parts: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM episode_files WHERE episode_id = ? AND kind = 'main'",
    )
    .bind(&ep6)
    .fetch_one(db.get_pool())
    .await
    .expect("ep6 main files");
    assert_eq!(overspill_parts, 0, "overspill must not be linked or parted");
    assert!(
        overspill.exists(),
        "overspill follows unneeded handling (keep by default)"
    );
}

// A download assigned at queue time that placed nothing keeps its unmatched
// subtitles too, not just its videos.
#[tokio::test]
async fn test_assigned_download_unmatched_subtitle_kept_for_review() {
    let (organizer, db, _dl, mock_state, tmp) = setup_with_controllable_mock().await;
    let series_id = "assigned-subtitle-guard";
    let library = tmp.path().join("library-sub-guard");
    let content_dir = tmp.path().join("Subtitle.Guard.Download");
    let content_path_str = content_dir.to_string_lossy().to_string();
    let download_hash = "assigned_subtitle_guard_hash";

    save_delete_unexpected_config(&db).await;
    insert_offset_mapping(&db, series_id, &library, "1", 0).await;

    let ep1 = format!("{}_S01E01", series_id);
    queue_enriched_download(
        &db,
        EnrichedDownload {
            series_id,
            series_title: "Star Voyage",
            media_name: "Show - 01-02 pack",
            seasons: &[1],
            episodes: &[1, 2],
            episode_id: &ep1,
            download_hash,
        },
    )
    .await;

    tokio::fs::create_dir_all(&content_dir).await.unwrap();
    let video = content_dir.join("Show - 900.mkv");
    let subtitle = content_dir.join("Show - 900.ass");
    tokio::fs::write(&video, b"video").await.unwrap();
    tokio::fs::write(&subtitle, b"subtitle").await.unwrap();
    mark_mock_download_complete(&mock_state, download_hash, &content_path_str);

    organizer
        .process_downloads()
        .await
        .expect("process_downloads");

    assert!(
        subtitle.exists(),
        "an unmatched subtitle of an unplaced assigned download must be kept"
    );
    let reviewed: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM unmatched_files WHERE file_path = ?")
            .bind(subtitle.to_string_lossy().to_string())
            .fetch_one(db.get_pool())
            .await
            .expect("unmatched count");
    assert_eq!(reviewed, 1, "the subtitle must be recorded for review");
}

// Two target episodes whose files are each split into parts: the fast path must
// not fire, and each episode must receive its own parts.
#[tokio::test]
async fn test_multipart_pack_does_not_use_fast_path() {
    let (organizer, db, _dl, mock_state, tmp) = setup_with_controllable_mock().await;
    let series_id = "multipart-pack";
    let library = tmp.path().join("library-multipart-pack");
    let content_dir = tmp.path().join("Multipart.Pack.Download");
    let content_path_str = content_dir.to_string_lossy().to_string();
    let download_hash = "multipart_pack_hash";

    save_delete_unexpected_config(&db).await;
    insert_offset_mapping(&db, series_id, &library, "1", 0).await;

    let ep5 = format!("{}_S01E05", series_id);
    let ep6 = format!("{}_S01E06", series_id);
    insert_ep_for_linking(&db, &ep5, "Star Voyage", 1, 5).await;
    insert_ep_for_linking(&db, &ep6, "Star Voyage", 1, 6).await;
    let intentions = format!(
        r#"[{{"episode_num":5,"source_episode_num":5,"episode_id":"{ep5}","score":100,"keep":true}},{{"episode_num":6,"source_episode_num":6,"episode_id":"{ep6}","score":100,"keep":true}}]"#
    );
    db.add_to_download_queue(AddToDownloadQueueParams {
        media_name: "Show - 05-06 (parts)",
        media_link: &magnet_with_hash(download_hash),
        series_title: "Star Voyage",
        series_id,
        seasons: &[1],
        episodes: &[5, 6],
        episode_id: Some(&ep5),
        score: 100,
        is_user_requested: false,
        is_season_pack: false,
        category: "Series",
        multi_targets: None,
        episode_intentions: Some(&intentions),
        source_pub_date: None,
        download_id: download_hash,
        scoring_size_bytes: None,
        scoring_seeders: None,
        scoring_episode_count: None,
        submitter: Some("MockFansub"),
        quality_profile_id: None,
        version: 1,
    })
    .await
    .expect("add_to_download_queue");
    let queue_id = db
        .get_queued_items()
        .await
        .unwrap()
        .first()
        .map(|i| i.id)
        .unwrap();
    db.update_queue_item_status(queue_id, "Downloading", Some(download_hash), None, None)
        .await
        .expect("update status");

    tokio::fs::create_dir_all(&content_dir).await.unwrap();
    for name in [
        "Show - 05 part 1.mkv",
        "Show - 05 part 2.mkv",
        "Show - 06 part 1.mkv",
        "Show - 06 part 2.mkv",
    ] {
        tokio::fs::write(content_dir.join(name), b"part")
            .await
            .unwrap();
    }
    mark_mock_download_complete(&mock_state, download_hash, &content_path_str);

    organizer
        .process_download_queue()
        .await
        .expect("process_download_queue");

    for (ep_id, label) in [(&ep5, "S01E05"), (&ep6, "S01E06")] {
        let parts: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM episode_files WHERE episode_id = ? AND kind = 'main'",
        )
        .bind(ep_id)
        .fetch_one(db.get_pool())
        .await
        .expect("episode main-file count");
        assert_eq!(parts, 2, "{label} must receive exactly its own two parts");
    }
}

// Two targets sharing the same source number cannot be told apart by number, so
// the file must be kept for review rather than attached to a guessed target.
#[tokio::test]
async fn test_colliding_target_source_numbers_not_fast_pathed() {
    let (organizer, db, _dl, mock_state, tmp) = setup_with_controllable_mock().await;
    let series_id = "colliding-targets";
    let library = tmp.path().join("library-colliding");
    let content_dir = tmp.path().join("Colliding.Download");
    let content_path_str = content_dir.to_string_lossy().to_string();
    let download_hash = "colliding_targets_hash";

    save_delete_unexpected_config(&db).await;
    insert_offset_mapping(&db, series_id, &library, "1", 0).await;

    let ep5 = format!("{}_S05E01", series_id);
    let ep6 = format!("{}_S06E01", series_id);
    insert_ep_for_linking(&db, &ep5, "Star Voyage", 5, 1).await;
    let intentions = format!(
        r#"[{{"episode_num":1,"source_episode_num":101,"episode_id":"{ep5}","score":100,"keep":true}},{{"episode_num":1,"source_episode_num":101,"episode_id":"{ep6}","score":100,"keep":true}}]"#
    );
    db.add_to_download_queue(AddToDownloadQueueParams {
        media_name: "Show - 101 (two targets)",
        media_link: &magnet_with_hash(download_hash),
        series_title: "Star Voyage",
        series_id,
        seasons: &[5],
        episodes: &[1],
        episode_id: Some(&ep5),
        score: 100,
        is_user_requested: false,
        is_season_pack: false,
        category: "Series",
        multi_targets: None,
        episode_intentions: Some(&intentions),
        source_pub_date: None,
        download_id: download_hash,
        scoring_size_bytes: None,
        scoring_seeders: None,
        scoring_episode_count: None,
        submitter: Some("MockFansub"),
        quality_profile_id: None,
        version: 1,
    })
    .await
    .expect("add_to_download_queue");
    let queue_id = db
        .get_queued_items()
        .await
        .unwrap()
        .first()
        .map(|i| i.id)
        .unwrap();
    db.update_queue_item_status(queue_id, "Downloading", Some(download_hash), None, None)
        .await
        .expect("update status");

    tokio::fs::create_dir_all(&content_dir).await.unwrap();
    let video = content_dir.join("Show - 101.mkv");
    tokio::fs::write(&video, b"video").await.unwrap();
    mark_mock_download_complete(&mock_state, download_hash, &content_path_str);

    organizer
        .process_downloads()
        .await
        .expect("process_downloads");

    assert!(
        video.exists(),
        "an ambiguous two-target file must be kept for review, not deleted"
    );
    let reviewed: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM unmatched_files WHERE file_path = ?")
            .bind(video.to_string_lossy().to_string())
            .fetch_one(db.get_pool())
            .await
            .expect("unmatched count");
    assert_eq!(
        reviewed, 1,
        "the ambiguous file must be recorded for review"
    );
}

// A release that a real season and a season-number alias both claim is ambiguous
// and must go to manual review, like conflicting season-title aliases.
#[tokio::test]
async fn test_conflicting_season_number_alias_is_ambiguous() {
    let (organizer, db, _dl, mock_state, tmp) = setup_with_controllable_mock().await;
    let series_id = "conflicting-season-alias";
    let library = tmp.path().join("library-conflict");
    let content_dir = tmp.path().join("Conflict.Download");
    let content_path_str = content_dir.to_string_lossy().to_string();
    let download_hash = "conflicting_season_alias_hash";

    save_delete_unexpected_config(&db).await;

    // Season 2 is real; season 5 is offset-numbered and aliases release season 2.
    let mut season = HashMap::new();
    season.insert(
        "2".to_string(),
        SeasonOverride {
            season: "2".to_string(),
            ..Default::default()
        },
    );
    season.insert(
        "5".to_string(),
        SeasonOverride {
            season: "5".to_string(),
            alias_season_number: Some(2),
            episode_offset: Some(100),
            ..Default::default()
        },
    );
    let mapping = MappingRule {
        target_title: "Star Voyage".to_string(),
        name: "star_voyage".to_string(),
        series_id: series_id.to_string(),
        settings: SeriesSettings {
            absolute_numbering: Some(false),
            season,
            path: Some(library.to_string_lossy().to_string()),
            ..Default::default()
        },
        ..Default::default()
    };
    db.upsert_series_mapping(series_id, &mapping)
        .await
        .expect("upsert mapping");
    let mut org = db.get_organization_config().await.unwrap_or_default();
    org.destination_roots = vec![library.to_path_buf().into()];
    db.save_organization_config(&org)
        .await
        .expect("save org config");

    queue_series_level_download(
        &db,
        series_id,
        "Star Voyage",
        "[MockFansub] Star Voyage S02E101",
        2,
        download_hash,
    )
    .await;

    tokio::fs::create_dir_all(&content_dir).await.unwrap();
    let video = content_dir.join("[MockFansub] Star Voyage S02E101.mkv");
    tokio::fs::write(&video, b"video").await.unwrap();
    mark_mock_download_complete(&mock_state, download_hash, &content_path_str);

    organizer
        .process_downloads()
        .await
        .expect("process_downloads");

    assert!(
        video.exists(),
        "a season claimed by two overrides must be kept for review, not deleted"
    );
    let reviewed: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM unmatched_files WHERE file_path = ?")
            .bind(video.to_string_lossy().to_string())
            .fetch_one(db.get_pool())
            .await
            .expect("unmatched count");
    assert_eq!(
        reviewed, 1,
        "the ambiguous file must be recorded for review"
    );
}

// Orphan adoption is a download-side path: a release that parses as season 1 but
// belongs to DB season 23 must use the season-number alias, then that season's
// episode offset.
#[tokio::test]
async fn test_orphan_adoption_applies_season_number_alias_and_offset() {
    let (organizer, db, _dl, _mock_state, tmp) = setup_with_controllable_mock().await;
    let series_id = "orphan-alias-offset";
    let library = tmp.path().join("library-orphan");
    insert_season_alias_mapping(&db, series_id, &library, "23", Some(1), 1155).await;

    let grab_file = tmp
        .path()
        .join("grab")
        .join("uuid")
        .join("[MockFansub] Star Voyage - 1169 (1080p).mkv");
    tokio::fs::create_dir_all(grab_file.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(&grab_file, b"episode 1169").await.unwrap();
    jumbie::state::FileStateManager::fingerprint_file(
        &db,
        &grab_file,
        jumbie::state::FileState::Complete,
    )
    .await
    .expect("fingerprint");

    // The episode row must exist first: `episode_files` references `episodes`.
    insert_ep_for_linking(&db, &format!("{}_S23E14", series_id), "Star Voyage", 23, 14).await;

    organizer
        .process_downloads()
        .await
        .expect("process_downloads");

    let adopted = format!("{}_S23E14", series_id);
    let linked: Option<String> = db
        .get_episode_file_path(&adopted)
        .await
        .expect("fingerprint query");
    assert!(
        linked.is_some(),
        "orphan must adopt to {adopted}; fingerprint link is {linked:?}"
    );
}

// The permissive link path serves series-level/user downloads; it must resolve a
// relabelled, offset-numbered release the same way the search path does.
#[tokio::test]
async fn test_permissive_link_applies_season_number_alias_and_offset() {
    let (organizer, db, _dl, mock_state, tmp) = setup_with_controllable_mock().await;
    let series_id = "permissive-alias-offset";
    let library = tmp.path().join("library-permissive");
    let content_dir = tmp.path().join("Permissive.Download");
    let content_path_str = content_dir.to_string_lossy().to_string();
    let download_hash = "permissive_alias_offset_hash";

    save_delete_unexpected_config(&db).await;
    insert_season_alias_mapping(&db, series_id, &library, "23", Some(1), 1155).await;

    // Series-level download: no episode context, so the permissive path resolves it.
    queue_series_level_download(
        &db,
        series_id,
        "Star Voyage",
        "[MockFansub] Star Voyage - 1169 (1080p)",
        23,
        download_hash,
    )
    .await;

    tokio::fs::create_dir_all(&content_dir).await.unwrap();
    tokio::fs::write(
        content_dir.join("[MockFansub] Star Voyage - 1169 (1080p).mkv"),
        b"episode 1169",
    )
    .await
    .unwrap();
    mark_mock_download_complete(&mock_state, download_hash, &content_path_str);

    organizer
        .process_downloads()
        .await
        .expect("process_downloads");

    let expected = format!("{}_S23E14", series_id);
    let linked: Option<String> = db
        .get_episode_file_path(&expected)
        .await
        .expect("fingerprint query");
    assert!(
        linked.is_some(),
        "permissive link must resolve to {expected}; fingerprint link is {linked:?}"
    );
}

// Season-alias titles are part of the download-side season rule, so orphan
// adoption must use them even when there is no episode offset.
#[tokio::test]
async fn test_orphan_adoption_uses_season_alias_title() {
    let (organizer, db, _dl, _mock_state, tmp) = setup_with_controllable_mock().await;
    let series_id = "orphan-alias-title";
    let library = tmp.path().join("library-alias-title");

    let mut season = HashMap::new();
    season.insert(
        "5".to_string(),
        SeasonOverride {
            season: "5".to_string(),
            aliases: vec!["Star Voyage Mini".to_string()],
            ..Default::default()
        },
    );
    let mapping = MappingRule {
        target_title: "Star Voyage".to_string(),
        name: "star_voyage".to_string(),
        series_id: series_id.to_string(),
        settings: SeriesSettings {
            absolute_numbering: Some(false),
            aliases: vec!["Star Voyage Mini".to_string()],
            season,
            path: Some(library.to_string_lossy().to_string()),
            ..Default::default()
        },
        ..Default::default()
    };
    db.upsert_series_mapping(series_id, &mapping)
        .await
        .expect("upsert mapping");
    let mut org = db.get_organization_config().await.unwrap_or_default();
    org.destination_roots = vec![library.to_path_buf().into()];
    db.save_organization_config(&org)
        .await
        .expect("save org config");

    let grab_file = tmp
        .path()
        .join("grab")
        .join("uuid")
        .join("[MockFansub] Star Voyage Mini - 05 (1080p).mkv");
    tokio::fs::create_dir_all(grab_file.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(&grab_file, b"episode 05").await.unwrap();
    jumbie::state::FileStateManager::fingerprint_file(
        &db,
        &grab_file,
        jumbie::state::FileState::Complete,
    )
    .await
    .expect("fingerprint");

    // The episode row must exist first: `episode_files` references `episodes`.
    insert_ep_for_linking(&db, &format!("{}_S05E05", series_id), "Star Voyage", 5, 5).await;

    organizer
        .process_downloads()
        .await
        .expect("process_downloads");

    let expected = format!("{}_S05E05", series_id);
    let linked: Option<String> = db
        .get_episode_file_path(&expected)
        .await
        .expect("fingerprint query");
    assert!(
        linked.is_some(),
        "season-alias title must adopt to {expected}; fingerprint link is {linked:?}"
    );
}

// The numeric season alias is only meaningful where the season's episodes are
// offset-numbered; for an offset-less season it must be ignored.
#[tokio::test]
async fn test_season_number_alias_ignored_without_offset() {
    let (organizer, db, _dl, mock_state, tmp) = setup_with_controllable_mock().await;
    let series_id = "alias-without-offset";
    let library = tmp.path().join("library-no-offset");
    let content_dir = tmp.path().join("No.Offset.Download");
    let content_path_str = content_dir.to_string_lossy().to_string();
    let download_hash = "alias_without_offset_hash";

    save_delete_unexpected_config(&db).await;
    // Season 5 aliases release season 2, but carries no episode offset.
    insert_season_alias_mapping(&db, series_id, &library, "5", Some(2), 0).await;

    queue_series_level_download(
        &db,
        series_id,
        "Star Voyage",
        "[MockFansub] Star Voyage S02E01",
        5,
        download_hash,
    )
    .await;

    tokio::fs::create_dir_all(&content_dir).await.unwrap();
    tokio::fs::write(
        content_dir.join("[MockFansub] Star Voyage S02E01.mkv"),
        b"ep1",
    )
    .await
    .unwrap();
    mark_mock_download_complete(&mock_state, download_hash, &content_path_str);

    organizer
        .process_downloads()
        .await
        .expect("process_downloads");

    let relabelled = format!("{}_S05E01", series_id);
    let native = format!("{}_S02E01", series_id);
    let linked: Option<String> = sqlx::query_scalar(
        "SELECT ef.episode_id FROM episode_files ef \
         JOIN file_paths fp ON fp.id = ef.file_path_id \
         WHERE fp.file_path LIKE '%S02E01%' LIMIT 1",
    )
    .fetch_optional(db.get_pool())
    .await
    .expect("fingerprint query");
    assert_eq!(
        linked.as_deref(),
        Some(native.as_str()),
        "without an offset the numeric alias must be ignored (not {relabelled})"
    );
}

#[tokio::test]
async fn test_smart_link_multi_season_guard() {
    let (organizer, db, _dl, mock_state, tmp) = setup_with_controllable_mock().await;
    let series_id = "multi-season-test";
    let content_dir = tmp.path().join("Show.S03.Complete");
    let content_path_str = content_dir.to_string_lossy().to_string();
    let download_hash = "multi_season_hash_999";

    // Arrange: series mapping for season 3 (no alias, no offset)
    let mapping = MappingRule {
        target_title: "Test Show".to_string(),
        name: "test_show".to_string(),
        series_id: series_id.to_string(),
        quality_profile: None,
        release_profile: None,
        qb_category: None,
        filters: None,
        scoring: None,
        hidden_in_library: false,
        settings: SeriesSettings {
            season: HashMap::from([(
                "3".to_string(),
                make_season_override("3", None, None, None, None),
            )]),
            ..Default::default()
        },
    };
    db.upsert_series_mapping(series_id, &mapping)
        .await
        .expect("upsert_series_mapping");

    // Pre-insert episodes for season 3
    insert_ep_for_linking(&db, &format!("{}_S03E01", series_id), "Test Show", 3, 1).await;
    insert_ep_for_linking(&db, &format!("{}_S03E02", series_id), "Test Show", 3, 2).await;

    tokio::fs::create_dir_all(&content_dir)
        .await
        .expect("create content dir");
    tokio::fs::write(content_dir.join("Show.S03E01.mkv"), b"correct ep")
        .await
        .expect("write file");
    tokio::fs::write(content_dir.join("Show.S03E02.mkv"), b"correct ep 2")
        .await
        .expect("write file");
    // Intruder from a different season — should be treated as unknown
    tokio::fs::write(content_dir.join("Show.S01E05.mkv"), b"wrong season")
        .await
        .expect("write file");

    {
        let mut state = mock_state.lock().unwrap();
        state.completed.push(download_hash.to_string());
        state
            .statuses
            .insert(download_hash.to_string(), "uploading".to_string());
        state
            .content_paths
            .insert(download_hash.to_string(), content_path_str);
        state.progress.insert(download_hash.to_string(), 1.0);
    }

    // Intentions cover eps 1 and 2 (keep=true). The S01E05 intruder is not in
    // the intention map — it will be treated as an unparseable/unknown file.
    let intentions = vec![
        EpisodeIntention {
            episode_num: 1,
            source_episode_num: 1,
            episode_id: format!("{}_S03E01", series_id),
            score: 100,
            keep: true,
        },
        EpisodeIntention {
            episode_num: 2,
            source_episode_num: 2,
            episode_id: format!("{}_S03E02", series_id),
            score: 100,
            keep: true,
        },
    ];
    let intentions_json = serde_json::to_string(&intentions).unwrap();
    let base_ep_id = format!("{}_S03E01", series_id);
    queue_download(
        &db,
        "Test Show S03",
        download_hash,
        "Test Show",
        "3",
        &base_ep_id,
        Some(&intentions_json),
    )
    .await;

    organizer
        .process_downloads()
        .await
        .expect("process_downloads");

    let ep1 = db
        .get_episode_by_id(&format!("{}_S03E01", series_id))
        .await
        .expect("get ep1")
        .expect("S03E01 should exist");
    assert!(
        ep1.file_path.is_some_and(|p| p.contains("S03E01")),
        "S03E01 should be linked"
    );

    let ep2 = db
        .get_episode_by_id(&format!("{}_S03E02", series_id))
        .await
        .expect("get ep2")
        .expect("S03E02 should exist");
    assert!(
        ep2.file_path.is_some_and(|p| p.contains("S03E02")),
        "S03E02 should be linked"
    );

    // S01E05 should NOT be linked to S03 — it should have gone to unknown files
    // not even exist — the intruder was silently discarded).
    let s01_ep = db
        .get_episode_by_id(&format!("{}_S03E05", series_id))
        .await
        .expect("get s01 intruder");
    assert!(
        s01_ep.is_none() || s01_ep.unwrap().file_path.is_none(),
        "Intruder S01E05 should NOT be linked to season 3"
    );
}

#[tokio::test]
async fn test_smart_link_multi_season_with_alias() {
    let (organizer, db, _dl, mock_state, tmp) = setup_with_controllable_mock().await;
    let series_id = "alias-multi-test";
    let content_dir = tmp.path().join("Show.S02.Complete");
    let content_path_str = content_dir.to_string_lossy().to_string();
    let download_hash = "alias_multi_hash_777";

    // Arrange: S05←S02 alias (no offset, range 1-100)
    let mapping = MappingRule {
        target_title: "Test Show".to_string(),
        name: "test_show".to_string(),
        series_id: series_id.to_string(),
        ..Default::default()
    };
    // Manually set the season override since MappingRule default doesn't include it
    let mut mapping_with_overrides = MappingRule {
        settings: SeriesSettings {
            season: HashMap::from([(
                "5".to_string(),
                make_season_override("5", Some(2), None, Some(1), Some(100)),
            )]),
            ..Default::default()
        },
        ..mapping
    };
    mapping_with_overrides.series_id = series_id.to_string();
    mapping_with_overrides.target_title = "Test Show".to_string();
    mapping_with_overrides.name = "test_show".to_string();

    db.upsert_series_mapping(series_id, &mapping_with_overrides)
        .await
        .expect("upsert_series_mapping");

    // Pre-insert episode for S05E01 (aliased from S02)
    insert_ep_for_linking(&db, &format!("{}_S05E01", series_id), "Test Show", 5, 1).await;
    insert_ep_for_linking(&db, &format!("{}_S05E02", series_id), "Test Show", 5, 2).await;

    // Arrange: S02 (aliased) files + S01 (intruder)
    tokio::fs::create_dir_all(&content_dir)
        .await
        .expect("create content dir");
    // This is the aliased season — should be kept
    tokio::fs::write(content_dir.join("Show.S02E01.mkv"), b"aliased correct")
        .await
        .expect("write file");
    tokio::fs::write(content_dir.join("Show.S02E02.mkv"), b"aliased correct 2")
        .await
        .expect("write file");
    // Intruder from S01 — should be treated as unknown
    tokio::fs::write(content_dir.join("Show.S01E99.mkv"), b"intruder")
        .await
        .expect("write file");

    {
        let mut state = mock_state.lock().unwrap();
        state.completed.push(download_hash.to_string());
        state
            .statuses
            .insert(download_hash.to_string(), "uploading".to_string());
        state
            .content_paths
            .insert(download_hash.to_string(), content_path_str);
        state.progress.insert(download_hash.to_string(), 1.0);
    }

    // Intentions cover the aliased season: S02E01.mkv → ep 1 → S05E01, etc.
    let intentions = vec![
        EpisodeIntention {
            episode_num: 1,
            source_episode_num: 1,
            episode_id: format!("{}_S05E01", series_id),
            score: 100,
            keep: true,
        },
        EpisodeIntention {
            episode_num: 2,
            source_episode_num: 2,
            episode_id: format!("{}_S05E02", series_id),
            score: 100,
            keep: true,
        },
    ];
    let intentions_json = serde_json::to_string(&intentions).unwrap();
    let base_ep_id = format!("{}_S05E01", series_id);
    queue_download(
        &db,
        "Test Show S02 Complete",
        download_hash,
        "Test Show",
        "5",
        &base_ep_id,
        Some(&intentions_json),
    )
    .await;

    organizer
        .process_downloads()
        .await
        .expect("process_downloads");

    let ep1 = db
        .get_episode_by_id(&format!("{}_S05E01", series_id))
        .await
        .expect("get ep1")
        .expect("S05E01 should exist");
    assert!(
        ep1.file_path.is_some_and(|p| p.contains("S02E01")),
        "S02E01 should be linked to S05E01"
    );

    let ep2 = db
        .get_episode_by_id(&format!("{}_S05E02", series_id))
        .await
        .expect("get ep2")
        .expect("S05E02 should exist");
    assert!(
        ep2.file_path.is_some_and(|p| p.contains("S02E02")),
        "S02E02 should be linked to S05E02"
    );

    // S01 intruder should not be linked to season 5
    let intruder_ep = db
        .get_episode_by_id(&format!("{}_S05E99", series_id))
        .await
        .expect("get intruder ep");
    assert!(
        intruder_ep.is_none() || intruder_ep.unwrap().file_path.is_none(),
        "S01E99 intruder should NOT be linked to season 5"
    );
}

#[tokio::test]
async fn test_smart_link_range_pack_with_offset() {
    // A range-named pack like "Show.S02E101-200" hits RANGE_PATTERNS, not
    // SEASON_PACK_PATTERNS.  This means is_season_pack=false, is_pack=true,
    // and queue_ep_end is the explicit range end (100), not 9999.
    // The offset must still be applied: S02E101.mkv → local ep 1.
    let (organizer, db, _dl, mock_state, tmp) = setup_with_controllable_mock().await;
    let series_id = "range-offset-test";
    let content_dir = tmp.path().join("Test.Show.S02E101-200");
    let content_path_str = content_dir.to_string_lossy().to_string();
    let download_hash = "range_offset_hash_555";

    // Arrange: S05←S02 alias + offset 100, range 1-100
    let mapping = MappingRule {
        target_title: "Test Show".to_string(),
        name: "test_show".to_string(),
        series_id: series_id.to_string(),
        ..Default::default()
    };
    let mut mapping_with_overrides = MappingRule {
        settings: SeriesSettings {
            season: HashMap::from([(
                "5".to_string(),
                make_season_override("5", Some(2), Some(100), Some(1), Some(100)),
            )]),
            ..Default::default()
        },
        ..mapping
    };
    mapping_with_overrides.series_id = series_id.to_string();
    mapping_with_overrides.target_title = "Test Show".to_string();
    mapping_with_overrides.name = "test_show".to_string();
    db.upsert_series_mapping(series_id, &mapping_with_overrides)
        .await
        .expect("upsert_series_mapping");

    // Pre-insert: source ep 101 → local ep 1, source ep 200 → local ep 100
    insert_ep_for_linking(&db, &format!("{}_S05E01", series_id), "Test Show", 5, 1).await;
    insert_ep_for_linking(&db, &format!("{}_S05E100", series_id), "Test Show", 5, 100).await;
    // Ep 201 is outside the range (local 101) — should NOT be linked
    insert_ep_for_linking(&db, &format!("{}_S05E101", series_id), "Test Show", 5, 101).await;

    tokio::fs::create_dir_all(&content_dir)
        .await
        .expect("create content dir");
    // In range: S02E101 → local ep 1 (101-100=1, in [1,100])
    tokio::fs::write(content_dir.join("Test.Show.S02E101.mkv"), b"in range")
        .await
        .expect("write file");
    // In range: S02E200 → local ep 100 (200-100=100, in [1,100])
    tokio::fs::write(content_dir.join("Test.Show.S02E200.mkv"), b"boundary")
        .await
        .expect("write file");
    // Out of range: S02E201 → local ep 101 (not in [1,100])
    tokio::fs::write(content_dir.join("Test.Show.S02E201.mkv"), b"out of range")
        .await
        .expect("write file");

    {
        let mut state = mock_state.lock().unwrap();
        state.completed.push(download_hash.to_string());
        state
            .statuses
            .insert(download_hash.to_string(), "uploading".to_string());
        state
            .content_paths
            .insert(download_hash.to_string(), content_path_str);
        state.progress.insert(download_hash.to_string(), 1.0);
    }

    // Intentions encode offset at queue time: source ep 101 → local ep 1.
    // Eps 101 and 200 are in the intended range (1-100 local).
    // Ep 201 (local 101) is outside — marked keep=false.
    let intentions = vec![
        EpisodeIntention {
            episode_num: 1,
            source_episode_num: 101,
            episode_id: format!("{}_S05E01", series_id),
            score: 100,
            keep: true,
        },
        EpisodeIntention {
            episode_num: 100,
            source_episode_num: 200,
            episode_id: format!("{}_S05E100", series_id),
            score: 100,
            keep: true,
        },
        EpisodeIntention {
            episode_num: 101,
            source_episode_num: 201,
            episode_id: format!("{}_S05E101", series_id),
            score: 100,
            keep: false,
        },
    ];
    let intentions_json = serde_json::to_string(&intentions).unwrap();
    let base_ep_id = format!("{}_S05E01", series_id);
    let queue_id = db
        .add_to_download_queue(AddToDownloadQueueParams {
            media_name: "Test Show S02E101-200",
            media_link: &magnet_with_hash(download_hash),
            series_title: "Test Show",
            series_id: "",
            seasons: &[5],
            episodes: &[1],
            episode_id: Some(&base_ep_id),
            score: 100,
            is_user_requested: false,
            is_season_pack: false,
            category: "",
            multi_targets: None,
            episode_intentions: Some(&intentions_json),
            source_pub_date: None,
            download_id: download_hash,
            scoring_size_bytes: None,
            scoring_seeders: None,
            scoring_episode_count: None,
            submitter: None,
            quality_profile_id: None,
            version: 1,
        })
        .await
        .expect("add_to_download_queue");
    let queue_id = match queue_id {
        jumbie_shared::types::AddQueueResult::Added { .. } => {
            let items = db.get_queued_items().await.unwrap();
            items.first().map(|i| i.id).unwrap()
        }
        _ => panic!("expected Added"),
    };
    db.update_queue_item_status(queue_id, "Downloading", Some(download_hash), None, None)
        .await
        .expect("update status");

    organizer
        .process_downloads()
        .await
        .expect("process_downloads");

    // S02E101.mkv → local ep 1 (101-100=1, in range [1,100]) ✓
    let ep1 = db
        .get_episode_by_id(&format!("{}_S05E01", series_id))
        .await
        .expect("get ep1")
        .expect("S05E01 should exist");
    assert!(
        ep1.file_path.is_some_and(|p| p.contains("S02E101")),
        "S02E101 should be linked to S05E01"
    );

    // S02E200.mkv → local ep 100 (200-100=100, in range [1,100]) ✓
    let ep100 = db
        .get_episode_by_id(&format!("{}_S05E100", series_id))
        .await
        .expect("get ep100")
        .expect("S05E100 should exist");
    assert!(
        ep100.file_path.is_some_and(|p| p.contains("S02E200")),
        "S02E200 should be linked to S05E100"
    );

    // S02E201.mkv → local ep 101 — outside range [1,100] → NOT linked
    let ep_outside = db
        .get_episode_by_id(&format!("{}_S05E101", series_id))
        .await
        .expect("get ep outside range");
    assert!(
        ep_outside.is_none() || ep_outside.unwrap().file_path.is_none(),
        "S02E201 should NOT be linked (local ep 101 is outside range [1,100])"
    );
}

#[tokio::test]
async fn test_intention_mixed_keep() {
    let (organizer, db, _dl, mock_state, tmp) = setup_with_controllable_mock().await;
    let series_id = "mixed-keep-test";
    let content_dir = tmp.path().join("Show.S01E01-05");
    let content_path_str = content_dir.to_string_lossy().to_string();
    let download_hash = "mixed_keep_hash";

    // Arrange: series mapping
    let mapping = MappingRule {
        target_title: "Test Show".to_string(),
        name: "test_show".to_string(),
        series_id: series_id.to_string(),
        ..Default::default()
    };
    db.upsert_series_mapping(series_id, &mapping)
        .await
        .expect("upsert_series_mapping");

    // Pre-insert episodes for eps 1-5
    for ep in 1..=5 {
        insert_ep_for_linking(
            &db,
            &format!("{}_S01E{:02}", series_id, ep),
            "Test Show",
            1,
            ep,
        )
        .await;
    }

    tokio::fs::create_dir_all(&content_dir)
        .await
        .expect("create content dir");
    for ep in 1..=5 {
        tokio::fs::write(
            content_dir.join(format!("Show.S01E{:02}.mkv", ep)),
            format!("episode {}", ep),
        )
        .await
        .expect("write file");
    }

    {
        let mut state = mock_state.lock().unwrap();
        state.completed.push(download_hash.to_string());
        state
            .statuses
            .insert(download_hash.to_string(), "uploading".to_string());
        state
            .content_paths
            .insert(download_hash.to_string(), content_path_str);
        state.progress.insert(download_hash.to_string(), 1.0);
    }

    // Arrange: intentions — eps 1-3 keep, eps 4-5 keep=false
    let intentions: Vec<EpisodeIntention> = (1..=5)
        .map(|ep| EpisodeIntention {
            episode_num: ep,
            source_episode_num: ep,
            episode_id: format!("{}_S01E{:02}", series_id, ep),
            score: 100,
            keep: ep <= 3,
        })
        .collect();
    let intentions_json = serde_json::to_string(&intentions).unwrap();
    let base_ep_id = format!("{}_S01E01", series_id);
    let queue_id = db
        .add_to_download_queue(AddToDownloadQueueParams {
            media_name: "Test Show S01E01-05",
            media_link: &magnet_with_hash(download_hash),
            series_title: "Test Show",
            series_id: "",
            seasons: &[1],
            episodes: &[1],
            episode_id: Some(&base_ep_id),
            score: 100,
            is_user_requested: false,
            is_season_pack: true,
            category: "",
            multi_targets: None,
            episode_intentions: Some(&intentions_json),
            source_pub_date: None,
            download_id: download_hash,
            scoring_size_bytes: None,
            scoring_seeders: None,
            scoring_episode_count: None,
            submitter: None,
            quality_profile_id: None,
            version: 1,
        })
        .await
        .expect("add_to_download_queue");
    let queue_id = match queue_id {
        jumbie_shared::types::AddQueueResult::Added { .. } => {
            let items = db.get_queued_items().await.unwrap();
            items.first().map(|i| i.id).unwrap()
        }
        _ => panic!("expected Added"),
    };
    db.update_queue_item_status(queue_id, "Downloading", Some(download_hash), None, None)
        .await
        .expect("update status");

    organizer
        .process_downloads()
        .await
        .expect("process_downloads");

    for ep in 1..=3 {
        let ep_row = db
            .get_episode_by_id(&format!("{}_S01E{:02}", series_id, ep))
            .await
            .expect("get ep")
            .unwrap_or_else(|| panic!("S01E{:02} should exist", ep));
        assert!(
            ep_row
                .file_path
                .is_some_and(|p| p.contains(&format!("S01E{:02}", ep))),
            "S01E{:02} should be linked",
            ep
        );
    }
    for ep in 4..=5 {
        // File should have been deleted (not linked)
        assert!(
            !content_dir.join(format!("Show.S01E{:02}.mkv", ep)).exists(),
            "S01E{:02}.mkv should have been deleted (keep=false)",
            ep
        );
    }
}

#[tokio::test]
async fn test_intention_subtitle_keep_false() {
    let (organizer, db, _dl, mock_state, tmp) = setup_with_controllable_mock().await;
    let series_id = "sub-keep-false-test";
    let content_dir = tmp.path().join("Show.S01E01-02");
    let content_path_str = content_dir.to_string_lossy().to_string();
    let download_hash = "sub_keep_false_hash";

    // Arrange: series mapping
    let mapping = MappingRule {
        target_title: "Test Show".to_string(),
        name: "test_show".to_string(),
        series_id: series_id.to_string(),
        ..Default::default()
    };
    db.upsert_series_mapping(series_id, &mapping)
        .await
        .expect("upsert_series_mapping");
    insert_ep_for_linking(&db, &format!("{}_S01E01", series_id), "Test Show", 1, 1).await;
    insert_ep_for_linking(&db, &format!("{}_S01E02", series_id), "Test Show", 1, 2).await;

    // Arrange: create files — video for keep=true, subtitle for keep=false
    tokio::fs::create_dir_all(&content_dir)
        .await
        .expect("create content dir");
    tokio::fs::write(content_dir.join("Show.S01E01.mkv"), b"wanted video")
        .await
        .expect("write file");
    // Subtitle paired with keep=false episode
    tokio::fs::write(content_dir.join("Show.S01E02.srt"), b"unwanted subtitle")
        .await
        .expect("write file");

    {
        let mut state = mock_state.lock().unwrap();
        state.completed.push(download_hash.to_string());
        state
            .statuses
            .insert(download_hash.to_string(), "uploading".to_string());
        state
            .content_paths
            .insert(download_hash.to_string(), content_path_str);
        state.progress.insert(download_hash.to_string(), 1.0);
    }

    // Arrange: intentions
    let intentions = vec![
        EpisodeIntention {
            episode_num: 1,
            source_episode_num: 1,
            episode_id: format!("{}_S01E01", series_id),
            score: 100,
            keep: true,
        },
        EpisodeIntention {
            episode_num: 2,
            source_episode_num: 2,
            episode_id: format!("{}_S01E02", series_id),
            score: 100,
            keep: false,
        },
    ];
    let intentions_json = serde_json::to_string(&intentions).unwrap();
    let base_ep_id = format!("{}_S01E01", series_id);
    let queue_id = db
        .add_to_download_queue(AddToDownloadQueueParams {
            media_name: "Test Show S01E01-02",
            media_link: &magnet_with_hash(download_hash),
            series_title: "Test Show",
            series_id: "",
            seasons: &[1],
            episodes: &[1],
            episode_id: Some(&base_ep_id),
            score: 100,
            is_user_requested: false,
            is_season_pack: false,
            category: "",
            multi_targets: None,
            episode_intentions: Some(&intentions_json),
            source_pub_date: None,
            download_id: download_hash,
            scoring_size_bytes: None,
            scoring_seeders: None,
            scoring_episode_count: None,
            submitter: None,
            quality_profile_id: None,
            version: 1,
        })
        .await
        .expect("add_to_download_queue");
    let queue_id = match queue_id {
        jumbie_shared::types::AddQueueResult::Added { .. } => {
            let items = db.get_queued_items().await.unwrap();
            items.first().map(|i| i.id).unwrap()
        }
        _ => panic!("expected Added"),
    };
    db.update_queue_item_status(queue_id, "Downloading", Some(download_hash), None, None)
        .await
        .expect("update status");

    organizer
        .process_downloads()
        .await
        .expect("process_downloads");

    let ep1 = db
        .get_episode_by_id(&format!("{}_S01E01", series_id))
        .await
        .expect("get ep1")
        .expect("S01E01 should exist");
    assert!(
        ep1.file_path.is_some_and(|p| p.contains("S01E01")),
        "S01E01 should be linked"
    );
    // Subtitle file should have been deleted (not linked, not fingerprinted)
    assert!(
        !content_dir.join("Show.S01E02.srt").exists(),
        "Unwanted subtitle should have been deleted"
    );
}

#[tokio::test]
async fn test_intention_legacy_fallback() {
    let (organizer, db, _dl, mock_state, tmp) = setup_with_controllable_mock().await;
    let series_id = "legacy-fallback-test";
    let content_dir = tmp.path().join("Show.S01E01");
    let content_path_str = content_dir.to_string_lossy().to_string();
    let download_hash = "legacy_fallback_hash";

    // Arrange: series mapping
    let mapping = MappingRule {
        target_title: "Test Show".to_string(),
        name: "test_show".to_string(),
        series_id: series_id.to_string(),
        ..Default::default()
    };
    db.upsert_series_mapping(series_id, &mapping)
        .await
        .expect("upsert_series_mapping");
    insert_ep_for_linking(&db, &format!("{}_S01E01", series_id), "Test Show", 1, 1).await;

    // Arrange: create test file
    tokio::fs::create_dir_all(&content_dir)
        .await
        .expect("create content dir");
    tokio::fs::write(content_dir.join("Show.S01E01.mkv"), b"legacy download")
        .await
        .expect("write file");

    {
        let mut state = mock_state.lock().unwrap();
        state.completed.push(download_hash.to_string());
        state
            .statuses
            .insert(download_hash.to_string(), "uploading".to_string());
        state
            .content_paths
            .insert(download_hash.to_string(), content_path_str);
        state.progress.insert(download_hash.to_string(), 1.0);
    }

    let base_ep_id = format!("{}_S01E01", series_id);
    queue_download(
        &db,
        "Test Show S01E01",
        download_hash,
        "Test Show",
        "1",
        &base_ep_id,
        None,
    )
    .await;

    organizer
        .process_downloads()
        .await
        .expect("process_downloads");

    let ep_check = db
        .get_episode_by_id(&base_ep_id)
        .await
        .expect("DB query should succeed");
    let ep_row = ep_check.unwrap();
    assert!(
        ep_row.file_path.is_some(),
        "Fallback-linked file should have a file_path set. Current status: {:?}, file_path: {:?}",
        ep_row.status,
        ep_row.file_path
    );
}

// Organizing retry: content-path resolution with exponential backoff

#[tokio::test]
async fn test_unfinalized_download_keeps_polling_when_content_path_none() {
    let (organizer, db, _dl, mock_state, _tmp) = setup_with_controllable_mock().await;
    let episode_id = "org_retry_missing_ep";
    let download_hash = "org_retry_missing_hash";

    insert_test_episode(&db, episode_id, "test-show").await;

    // SSoT: content path is always returned for any active download.  A missing
    // hash means it's not in any downloader plugin.  The item gets re-queued
    // with retry so the next cycle can re-resolve the path.
    {
        let mut state = mock_state.lock().unwrap();
        state.completed.push(download_hash.to_string());
        state
            .statuses
            .insert(download_hash.to_string(), "uploading".to_string());
        state.progress.insert(download_hash.to_string(), 1.0);
        // Intentionally do NOT set content_path — hash not found in any plugin
    }

    db.add_to_download_queue(AddToDownloadQueueParams {
        media_name: "Test Show S01E01",
        media_link: &magnet_with_hash(download_hash),
        series_title: "Test Show",
        series_id: "",
        seasons: &[1],
        episodes: &[1],
        episode_id: Some(episode_id),
        score: 100,
        is_user_requested: false,
        is_season_pack: false,
        category: "",
        multi_targets: None,
        episode_intentions: None,
        source_pub_date: None,
        download_id: download_hash,
        scoring_size_bytes: None,
        scoring_seeders: None,
        scoring_episode_count: None,
        submitter: None,
        quality_profile_id: None,
        version: 1,
    })
    .await
    .expect("add_to_download_queue");
    // Find the queued item and move it to Downloading
    let items = db.get_queued_items().await.unwrap();
    let item = items
        .iter()
        .find(|i| i.episode_id.as_deref() == Some(episode_id))
        .unwrap();
    db.update_queue_item_status(item.id, "Downloading", Some(download_hash), None, None)
        .await
        .expect("update status");

    organizer
        .process_download_queue()
        .await
        .expect("process_download_queue");

    // Without a content path (hash not in any plugin), the item is re-queued
    // with retry so the next cycle can re-resolve the path if the plugin recovers.
    let all = db.get_download_queue().await.unwrap();
    let item = all
        .iter()
        .find(|i| i.episode_id.as_deref() == Some(episode_id))
        .expect("item must exist");
    assert_eq!(
        item.status, "Queued",
        "missing content path (hash not in any plugin) should re-queue with retry, got status {:?}",
        item.status
    );
    assert_eq!(
        item.retry_count, 1,
        "retry_count should be 1 after first re-queue"
    );
}

#[tokio::test]
async fn test_completed_download_transitions_to_organizing_when_path_not_on_disk() {
    let (organizer, db, _dl, mock_state, _tmp) = setup_with_controllable_mock().await;
    let episode_id = "org_path_missing_ep";
    let download_hash = "org_path_missing_hash";

    insert_test_episode(&db, episode_id, "test-show").await;

    let fake_path = "/tmp/nonexistent_path_for_test/Show.S01E01.mkv";
    {
        let mut state = mock_state.lock().unwrap();
        state.completed.push(download_hash.to_string());
        state
            .statuses
            .insert(download_hash.to_string(), "uploading".to_string());
        state.progress.insert(download_hash.to_string(), 1.0);
        state
            .content_paths
            .insert(download_hash.to_string(), fake_path.to_string());
    }

    db.add_to_download_queue(AddToDownloadQueueParams {
        media_name: "Test Show S01E01",
        media_link: &magnet_with_hash(download_hash),
        series_title: "Test Show",
        series_id: "",
        seasons: &[1],
        episodes: &[1],
        episode_id: Some(episode_id),
        score: 100,
        is_user_requested: false,
        is_season_pack: false,
        category: "",
        multi_targets: None,
        episode_intentions: None,
        source_pub_date: None,
        download_id: download_hash,
        scoring_size_bytes: None,
        scoring_seeders: None,
        scoring_episode_count: None,
        submitter: None,
        quality_profile_id: None,
        version: 1,
    })
    .await
    .expect("add_to_download_queue");
    let items = db.get_queued_items().await.unwrap();
    let item = items
        .iter()
        .find(|i| i.episode_id.as_deref() == Some(episode_id))
        .unwrap();
    db.update_queue_item_status(item.id, "Downloading", Some(download_hash), None, None)
        .await
        .expect("update status");

    organizer
        .process_download_queue()
        .await
        .expect("process_download_queue");

    let all = db.get_download_queue().await.unwrap();
    let item = all
        .iter()
        .find(|i| i.episode_id.as_deref() == Some(episode_id))
        .expect("item must exist");
    assert_eq!(
        item.status, "Organizing",
        "download should transition to Organizing when qBittorrent reports path but it doesn't exist on disk"
    );
    assert_eq!(
        item.retry_count, 1,
        "retry_count should be 1 after first transition"
    );
    assert!(item.next_retry_at.is_some(), "next_retry_at should be set");
}

#[tokio::test]
async fn test_organizing_retry_succeeds_when_path_appears() {
    let (organizer, db, _dl, mock_state, tmp) = setup_with_controllable_mock().await;
    let episode_id = "org_retry_success_ep";
    let download_hash = "org_retry_success_hash";

    // Create a real file on disk so the content path check passes
    let content_dir = tmp.path().join("Show.S01E01.Complete");
    tokio::fs::create_dir_all(&content_dir)
        .await
        .expect("create content dir");
    tokio::fs::write(content_dir.join("Show.S01E01.mkv"), b"content")
        .await
        .expect("write file");
    let content_path_str = content_dir.to_string_lossy().to_string();

    insert_test_episode(&db, episode_id, "test-show").await;

    let queue_id = db
        .add_to_download_queue(AddToDownloadQueueParams {
            media_name: "Test Show S01E01",
            media_link: &magnet_with_hash(download_hash),
            series_title: "Test Show",
            series_id: "",
            seasons: &[1],
            episodes: &[1],
            episode_id: Some(episode_id),
            score: 100,
            is_user_requested: false,
            is_season_pack: false,
            category: "",
            multi_targets: None,
            episode_intentions: None,
            source_pub_date: None,
            download_id: download_hash,
            scoring_size_bytes: None,
            scoring_seeders: None,
            scoring_episode_count: None,
            submitter: None,
            quality_profile_id: None,
            version: 1,
        })
        .await
        .expect("add_to_download_queue");
    let queue_id = match queue_id {
        jumbie_shared::types::AddQueueResult::Added { .. } => {
            let items = db.get_queued_items().await.unwrap();
            items.first().map(|i| i.id).unwrap()
        }
        _ => panic!("expected Added"),
    };

    // Set to Organizing with a past retry time so it's picked up immediately
    let past = chrono::Utc::now().naive_utc() - chrono::Duration::hours(1);
    db.update_queue_item_retry(queue_id, "Organizing", 2, past)
        .await
        .expect("set organizing");
    db.update_queue_item_status(queue_id, "Organizing", Some(download_hash), None, None)
        .await
        .expect("set organizing status");

    {
        let mut state = mock_state.lock().unwrap();
        state.completed.push(download_hash.to_string());
        state
            .statuses
            .insert(download_hash.to_string(), "uploading".to_string());
        state.progress.insert(download_hash.to_string(), 1.0);
        state
            .content_paths
            .insert(download_hash.to_string(), content_path_str.clone());
    }

    organizer
        .process_download_queue()
        .await
        .expect("process_download_queue");

    // Note: we only run process_download_queue (not process_downloads) because
    // organize_completed would try to organize the test file and fail due to
    // missing series mapping.  The retry-phase completion is what we test here.
    let all = db.get_download_queue().await.unwrap();
    let item = all
        .iter()
        .find(|i| i.episode_id.as_deref() == Some(episode_id))
        .expect("item must exist");
    assert_eq!(
        item.status, "Completed",
        "organizing retry should complete when path becomes available"
    );
    assert_eq!(
        item.retry_count, 0,
        "retry_count should be reset on completion"
    );
}

#[tokio::test]
async fn test_organizing_retry_fails_after_max_attempts() {
    let (organizer, db, _dl, mock_state, _tmp) = setup_with_controllable_mock().await;
    let episode_id = "org_retry_fail_ep";
    let download_hash = "org_retry_fail_hash";

    insert_test_episode(&db, episode_id, "test-show").await;

    db.add_to_download_queue(AddToDownloadQueueParams {
        media_name: "Test Show S01E01",
        media_link: &magnet_with_hash(download_hash),
        series_title: "Test Show",
        series_id: "",
        seasons: &[1],
        episodes: &[1],
        episode_id: Some(episode_id),
        score: 100,
        is_user_requested: false,
        is_season_pack: false,
        category: "",
        multi_targets: None,
        episode_intentions: None,
        source_pub_date: None,
        download_id: download_hash,
        scoring_size_bytes: None,
        scoring_seeders: None,
        scoring_episode_count: None,
        submitter: None,
        quality_profile_id: None,
        version: 1,
    })
    .await
    .expect("add_to_download_queue");
    let items = db.get_queued_items().await.unwrap();
    let item = items
        .iter()
        .find(|i| i.episode_id.as_deref() == Some(episode_id))
        .unwrap();
    db.update_queue_item_status(item.id, "Downloading", Some(download_hash), None, None)
        .await
        .expect("update status");

    // Mock reports completed with a content path that doesn't exist on disk.
    // This triggers the Downloading → Organizing transition (exponential backoff).
    {
        let mut state = mock_state.lock().unwrap();
        state.completed.push(download_hash.to_string());
        state
            .statuses
            .insert(download_hash.to_string(), "uploading".to_string());
        state.progress.insert(download_hash.to_string(), 1.0);
        state.content_paths.insert(
            download_hash.to_string(),
            "/tmp/nonexistent_test_path/file.mkv".to_string(),
        );
    }

    // Act: run process_download_queue 6 times (advancing the clock manually)
    // Each cycle: first call moves Downloading → Organizing (retry_count=1),
    // subsequent calls advance retry_count until >= 5 → Failed.
    // We force next_retry_at to the past before each call so get_organizing_items
    // picks them up.
    // Uses process_download_queue (not process_downloads) to avoid organize_completed
    // interfering with the retry-failure assertion.
    for _ in 0..6 {
        // Bump next_retry_at to the past so the organizing query picks it up
        let past = chrono::Utc::now().naive_utc() - chrono::Duration::hours(1);
        sqlx::query("UPDATE download_queue SET next_retry_at = ? WHERE status = 'Organizing'")
            .bind(past)
            .execute(db.get_pool())
            .await
            .ok();

        organizer
            .process_download_queue()
            .await
            .expect("process_download_queue");
    }

    let all = db.get_download_queue().await.unwrap();
    let item = all
        .iter()
        .find(|i| i.episode_id.as_deref() == Some(episode_id))
        .expect("item must exist");
    assert_eq!(
        item.status, "Failed",
        "organizing retry should fail after max attempts"
    );
    assert!(
        item.error_message.is_some(),
        "failed item should have an error message"
    );
}

#[tokio::test]
async fn test_organizing_retry_skipped_when_next_retry_in_future() {
    // Items with next_retry_at in the future MUST NOT be picked up
    // by retry_organizing_items — the backoff timing is enforced at the DB query level.
    let (organizer, db, _dl, _mock_state, _tmp) = setup_with_controllable_mock().await;
    let episode_id = "org_retry_future_ep";
    let download_hash = "org_retry_future_hash";

    insert_test_episode(&db, episode_id, "test-show").await;

    db.add_to_download_queue(AddToDownloadQueueParams {
        media_name: "Test Show S01E01",
        media_link: &magnet_with_hash(download_hash),
        series_title: "Test Show",
        series_id: "",
        seasons: &[1],
        episodes: &[1],
        episode_id: Some(episode_id),
        score: 100,
        is_user_requested: false,
        is_season_pack: false,
        category: "",
        multi_targets: None,
        episode_intentions: None,
        source_pub_date: None,
        download_id: download_hash,
        scoring_size_bytes: None,
        scoring_seeders: None,
        scoring_episode_count: None,
        submitter: None,
        quality_profile_id: None,
        version: 1,
    })
    .await
    .expect("add_to_download_queue");
    let items = db.get_queued_items().await.unwrap();
    let item = items
        .iter()
        .find(|i| i.episode_id.as_deref() == Some(episode_id))
        .unwrap();
    let item_id = item.id;

    let future = chrono::Utc::now().naive_utc() + chrono::Duration::hours(1);
    db.update_queue_item_retry(item_id, "Organizing", 2, future)
        .await
        .expect("set organizing with future next_retry");

    // Act: process the queue
    organizer
        .process_download_queue()
        .await
        .expect("process_download_queue");

    let all = db.get_download_queue().await.unwrap();
    let item = all
        .iter()
        .find(|i| i.episode_id.as_deref() == Some(episode_id))
        .unwrap();
    assert_eq!(
        item.status, "Organizing",
        "item should remain Organizing when next_retry_at is in the future"
    );
    assert_eq!(
        item.retry_count, 2,
        "retry_count should not advance when next_retry_at is in the future"
    );
}

#[tokio::test]
async fn test_smart_link_series_scan_sentinel() {
    // SERIES-SCAN path: queue_item.episode.is_none() triggers
    // link_permissive_episode for every parsed video file.
    let (organizer, db, _dl, mock_state, tmp) = setup_with_controllable_mock().await;
    let series_id = "sentinel-scan-test";
    let content_dir = tmp.path().join("Sentinel.Show.S01.COMPLETE");
    let content_path_str = content_dir.to_string_lossy().to_string();
    let download_hash = "sentinel_scan_hash_abc";

    // Arrange: series mapping
    let mapping = MappingRule {
        target_title: "Sentinel Show".to_string(),
        name: "sentinel_show".to_string(),
        series_id: series_id.to_string(),
        quality_profile: None,
        release_profile: None,
        qb_category: None,
        filters: None,
        scoring: None,
        hidden_in_library: false,
        settings: SeriesSettings {
            aliases: vec![],
            absolute_numbering: Some(false),
            season: HashMap::new(),
            season_absolute: HashMap::new(),
            reg_patterns: vec![],
            season_folder_format: None,
            episode_file_format: None,
            season_folder_format_absolute: None,
            episode_file_format_absolute: None,
            flatten_season_folders: None,
            rename_episodes: None,
            search_format: None,
            search_format_absolute: None,
            path: None,
            monitor_mode: None,
            metadata_ids: HashMap::new(),
            metadata_last_synced_at: HashMap::new(),
            last_known_dir_mtimes: HashMap::new(),
        },
    };
    db.upsert_series_mapping(series_id, &mapping)
        .await
        .expect("upsert_series_mapping");

    // Arrange: content dir with a video file
    tokio::fs::create_dir_all(&content_dir)
        .await
        .expect("create content dir");
    tokio::fs::write(content_dir.join("Sentinel.Show.S01E03.mkv"), b"dummy")
        .await
        .expect("write file");

    {
        let mut state = mock_state.lock().unwrap();
        state.completed.push(download_hash.to_string());
        state
            .statuses
            .insert(download_hash.to_string(), "uploading".to_string());
        state
            .content_paths
            .insert(download_hash.to_string(), content_path_str);
        state.progress.insert(download_hash.to_string(), 1.0);
    }

    // episode=None, season=None, episode_id=None, episode_intentions=None
    let queued = db
        .add_to_download_queue(AddToDownloadQueueParams {
            media_name: "Sentinel Show Series Scan",
            media_link: &magnet_with_hash(download_hash),
            series_title: "Sentinel Show",
            series_id,
            seasons: &[],
            episodes: &[],
            episode_id: None,
            score: 100,
            is_user_requested: true,
            is_season_pack: false,
            category: "Series",
            multi_targets: None,
            episode_intentions: None,
            source_pub_date: None,
            download_id: download_hash,
            scoring_size_bytes: None,
            scoring_seeders: None,
            scoring_episode_count: None,
            submitter: None,
            quality_profile_id: None,
            version: 1,
        })
        .await
        .expect("add_to_download_queue");
    let queue_id = match queued {
        jumbie_shared::types::AddQueueResult::Added { .. } => {
            let items = db.get_queued_items().await.unwrap();
            items.first().map(|i| i.id).unwrap()
        }
        _ => panic!("expected Added"),
    };
    db.update_queue_item_status(queue_id, "Downloading", Some(download_hash), None, None)
        .await
        .expect("update status");

    organizer
        .process_downloads()
        .await
        .expect("process_downloads");

    // File "Sentinel.Show.S01E03.mkv" → parsed S01E03 → ep_id = "sentinel-scan-test_S01E03"
    let ep_id = format!("{}_S01E03", series_id);
    let episode = db
        .get_episode_by_id(&ep_id)
        .await
        .expect("get episode")
        .expect("episode should exist after smart_link");
    assert!(
        episode
            .file_path
            .as_ref()
            .is_some_and(|p| p.contains("S01E03")),
        "S01E03.mkv should be linked to episode. file_path={:?}",
        episode.file_path
    );
    assert!(
        episode.status.as_deref() == Some("organized")
            || episode.status.as_deref() == Some("downloaded"),
        "episode status should be organized or downloaded, got {:?}",
        episode.status
    );
}

// Series-scan pack overspill → unexpected_files_handling (unmatched)
//
// A series-level search (or pack) can contain episodes we already have on disk.
// The user intentionally downloaded the release, so the redundant files must
// obey the unexpected-files keep/delete preference instead of being relinked and
// then deleted unconditionally by organize.

struct PresentEpisodeOutcome {
    incoming_exists: bool,
    existing_exists: bool,
    episode_file_path: Option<String>,
    fingerprint_episode_id: Option<String>,
    review_row_series: Option<String>,
    review_row_reason: Option<String>,
    orphan_listed: bool,
}

/// Arrange a series-scan download whose only file maps to an episode that
/// already has an organized file on disk, run the pipeline, and report the
/// observable outcome.
async fn run_series_scan_present_episode(
    user_requested: bool,
    handling: &str,
) -> PresentEpisodeOutcome {
    let (organizer, db, _dl, mock_state, tmp) = setup_with_controllable_mock().await;
    let series_id = "unneeded-present-test";
    let content_dir = tmp.path().join("Unneeded.Show.S01.COMPLETE");
    let content_path_str = content_dir.to_string_lossy().to_string();
    let download_hash = "unneeded_present_hash_abc";

    // Pre-existing organized episode file (already on disk)
    let existing_dir = tmp.path().join("library");
    tokio::fs::create_dir_all(&existing_dir).await.unwrap();
    let existing_file = existing_dir.join("Unneeded.Show.S01E03.mkv");
    tokio::fs::write(&existing_file, b"existing").await.unwrap();
    let existing_path_str = existing_file.to_string_lossy().to_string();

    let mapping = MappingRule {
        target_title: "Unneeded Show".to_string(),
        name: "unneeded_show".to_string(),
        series_id: series_id.to_string(),
        quality_profile: None,
        release_profile: None,
        qb_category: None,
        filters: None,
        scoring: None,
        hidden_in_library: false,
        settings: SeriesSettings {
            aliases: vec![],
            absolute_numbering: Some(false),
            season: HashMap::new(),
            season_absolute: HashMap::new(),
            reg_patterns: vec![],
            season_folder_format: None,
            episode_file_format: None,
            season_folder_format_absolute: None,
            episode_file_format_absolute: None,
            flatten_season_folders: None,
            rename_episodes: None,
            search_format: None,
            search_format_absolute: None,
            path: None,
            monitor_mode: None,
            metadata_ids: HashMap::new(),
            metadata_last_synced_at: HashMap::new(),
            last_known_dir_mtimes: HashMap::new(),
        },
    };
    db.upsert_series_mapping(series_id, &mapping)
        .await
        .expect("upsert_series_mapping");

    let ep_id = format!("{}_S01E03", series_id);
    db.insert_episode(jumbie::db::episodes::InsertEpisodeParams {
        episode_id: &ep_id,
        series_id,
        season: 1,
        episode: 3,
        file_path: Some(&existing_path_str),
        title: None,
        quality_profile_id: None,
        status: "organized",
        meta_date: None,
        est_date: None,
        metadata_ids: &HashMap::new(),
        description: None,
        runtime: None,
        image_url: None,
        metadata_source: None,
        numbering_mode: None,
    })
    .await
    .expect("insert existing episode");

    // Incoming pack file for the same episode
    tokio::fs::create_dir_all(&content_dir).await.unwrap();
    let incoming = content_dir.join("Unneeded.Show.S01E03.mkv");
    tokio::fs::write(&incoming, b"incoming").await.unwrap();

    // Preference under test
    // The governing setting depends on whether the release was manually chosen
    // (manual → unexpected_files_handling) or automatic
    // (→ unneeded_episodes_handling). The other one is set to the opposite so a
    // wrong branch flips the outcome and fails the test.
    let opposite = if handling == "keep" { "delete" } else { "keep" };
    let general = jumbie_shared::config::GeneralConfig {
        unexpected_files_handling: if user_requested { handling } else { opposite }.to_string(),
        unneeded_episodes_handling: if user_requested { opposite } else { handling }.to_string(),
        ..Default::default()
    };
    db.save_general_config(&general).await.unwrap();

    // Mock reports the download as completed
    {
        let mut state = mock_state.lock().unwrap();
        state.completed.push(download_hash.to_string());
        state
            .statuses
            .insert(download_hash.to_string(), "uploading".to_string());
        state
            .content_paths
            .insert(download_hash.to_string(), content_path_str);
        state.progress.insert(download_hash.to_string(), 1.0);
    }

    // Queue item with no episode context (series scan)
    let params = AddToDownloadQueueParams {
        media_name: "[Group] Unneeded Show - 03 (1080p)",
        media_link: &magnet_with_hash(download_hash),
        series_title: "Unneeded Show",
        series_id,
        seasons: &[],
        episodes: &[],
        episode_id: None,
        score: 100,
        is_user_requested: user_requested,
        is_season_pack: false,
        category: "Series",
        multi_targets: None,
        episode_intentions: None,
        source_pub_date: None,
        download_id: download_hash,
        scoring_size_bytes: None,
        scoring_seeders: None,
        scoring_episode_count: None,
        submitter: None,
        quality_profile_id: None,
        version: 1,
    };
    let queued = if user_requested {
        db.add_manual_download_to_queue(params).await
    } else {
        db.add_to_download_queue(params).await
    }
    .expect("add_to_download_queue");
    assert!(matches!(
        queued,
        jumbie_shared::types::AddQueueResult::Added { .. }
    ));
    let queue_id = db
        .get_queued_items()
        .await
        .unwrap()
        .first()
        .map(|i| i.id)
        .unwrap();
    db.update_queue_item_status(queue_id, "Downloading", Some(download_hash), None, None)
        .await
        .expect("update status");

    organizer
        .process_downloads()
        .await
        .expect("process_downloads");

    let episode = db
        .get_episode_by_id(&ep_id)
        .await
        .expect("get episode")
        .expect("episode should still exist");
    let incoming_str = incoming.to_string_lossy().to_string();
    let fingerprint_episode_id: Option<String> = sqlx::query_scalar(
        "SELECT ef.episode_id FROM episode_files ef \
         JOIN file_paths fp ON fp.id = ef.file_path_id \
         WHERE fp.file_path = ? LIMIT 1",
    )
    .bind(&incoming_str)
    .fetch_optional(db.get_pool())
    .await
    .expect("fingerprint query");
    let review_row_series: Option<String> =
        sqlx::query_scalar("SELECT series_id FROM unmatched_files WHERE file_path = ?")
            .bind(&incoming_str)
            .fetch_optional(db.get_pool())
            .await
            .expect("unmatched_files query")
            .flatten();
    let review_row_reason: Option<String> =
        sqlx::query_scalar("SELECT reason FROM unmatched_files WHERE file_path = ?")
            .bind(&incoming_str)
            .fetch_optional(db.get_pool())
            .await
            .expect("unmatched_files reason query")
            .flatten();
    let orphan_listed = db
        .get_orphan_files()
        .await
        .expect("orphan query")
        .contains(&incoming_str);

    PresentEpisodeOutcome {
        incoming_exists: incoming.exists(),
        existing_exists: existing_file.exists(),
        episode_file_path: episode.file_path,
        fingerprint_episode_id,
        review_row_series,
        review_row_reason,
        orphan_listed,
    }
}

#[tokio::test]
async fn test_series_scan_present_episode_kept_for_review() {
    let out = run_series_scan_present_episode(true, "keep").await;

    assert!(
        out.existing_exists,
        "the pre-existing library file must be untouched"
    );
    assert!(
        out.incoming_exists,
        "unmatched file must be left in the download dir when handling=keep"
    );
    assert_eq!(
        out.fingerprint_episode_id, None,
        "a kept file must not be linked to a fake (sentinel) episode id"
    );
    assert!(
        !out.orphan_listed,
        "a kept-for-review file must be excluded from orphan adoption"
    );
    assert_eq!(
        out.review_row_series.as_deref(),
        Some("unneeded-present-test"),
        "the review row must be scoped to the downloading series"
    );
    assert_eq!(
        out.review_row_reason.as_deref(),
        Some("unmatched"),
        "a manually requested download's redundant file is 'unmatched'"
    );
    assert!(
        out.episode_file_path
            .as_deref()
            .is_some_and(|p| p.contains("library")),
        "the episode must keep its existing file, not the download: {:?}",
        out.episode_file_path
    );
}

#[tokio::test]
async fn test_series_scan_present_episode_deleted_when_configured() {
    // Automatic pack: redundant files follow unneeded_episodes_handling=delete.
    let out = run_series_scan_present_episode(false, "delete").await;

    assert!(
        out.existing_exists,
        "the pre-existing library file must be untouched"
    );
    assert!(
        !out.incoming_exists,
        "unmatched file must be deleted when handling=delete"
    );
    assert_eq!(
        out.fingerprint_episode_id, None,
        "deleted unmatched file must not leave a stale fingerprint"
    );
    assert_eq!(
        out.review_row_series, None,
        "deleted file must not leave a review row"
    );
    assert_eq!(out.review_row_reason, None);
    assert!(
        !out.orphan_listed,
        "a deleted file must not appear as an adoption candidate"
    );
    assert!(
        out.episode_file_path
            .as_deref()
            .is_some_and(|p| p.contains("library")),
        "the episode must keep its existing file: {:?}",
        out.episode_file_path
    );
}

/// A manually chosen download treats every file as wanted: even with
/// `unexpected_files_handling=delete`, its redundant file is kept for review.
#[tokio::test]
async fn test_series_scan_present_episode_manual_keeps_when_delete_configured() {
    let out = run_series_scan_present_episode(true, "delete").await;

    assert!(
        out.existing_exists,
        "the pre-existing library file must be untouched"
    );
    assert!(
        out.incoming_exists,
        "a manual download's file must be kept for review, never deleted by policy"
    );
    assert_eq!(out.fingerprint_episode_id, None);
    assert!(
        !out.orphan_listed,
        "a kept-for-review file must be excluded from orphan adoption"
    );
    assert_eq!(
        out.review_row_series.as_deref(),
        Some("unneeded-present-test")
    );
    assert_eq!(out.review_row_reason.as_deref(), Some("unmatched"));
    assert!(
        out.episode_file_path
            .as_deref()
            .is_some_and(|p| p.contains("library")),
        "the episode must keep its existing file: {:?}",
        out.episode_file_path
    );
}

#[tokio::test]
async fn test_series_scan_present_episode_auto_uses_unneeded_handling() {
    // Automatic (non-user-intentional) pack: redundant files follow
    // unneeded_episodes_handling, NOT unexpected_files_handling. The helper sets
    // unexpected_files_handling to the opposite, so a wrong branch would flip the
    // outcome and fail the assertions below.
    let out = run_series_scan_present_episode(false, "keep").await;

    assert!(
        out.incoming_exists,
        "auto pack must keep the redundant file under unneeded=keep"
    );
    assert_eq!(
        out.fingerprint_episode_id, None,
        "a kept file must not be linked to a fake (sentinel) episode id"
    );
    assert!(
        !out.orphan_listed,
        "a kept-for-review file must be excluded from orphan adoption"
    );
    assert_eq!(
        out.review_row_series.as_deref(),
        Some("unneeded-present-test"),
        "auto pack review row must still be scoped to the series"
    );
    assert_eq!(
        out.review_row_reason.as_deref(),
        Some("unneeded"),
        "an automatic pack's redundant file is 'unneeded', not 'unmatched'"
    );
    assert!(
        out.episode_file_path
            .as_deref()
            .is_some_and(|p| p.contains("library")),
        "the episode must keep its existing file: {:?}",
        out.episode_file_path
    );
}

#[tokio::test]
async fn test_intention_subtitle_keep_false_kept_when_configured() {
    // keep=false subtitles are unneeded episodes and must honour the preference
    // instead of being deleted unconditionally.
    let (organizer, db, _dl, mock_state, tmp) = setup_with_controllable_mock().await;
    let series_id = "sub-keep-false-keep-test";
    let content_dir = tmp.path().join("Show.S01E01-02");
    let content_path_str = content_dir.to_string_lossy().to_string();
    let download_hash = "sub_keep_false_keep_hash";

    let mapping = MappingRule {
        target_title: "Test Show".to_string(),
        name: "test_show".to_string(),
        series_id: series_id.to_string(),
        ..Default::default()
    };
    db.upsert_series_mapping(series_id, &mapping)
        .await
        .expect("upsert_series_mapping");
    insert_ep_for_linking(&db, &format!("{}_S01E01", series_id), "Test Show", 1, 1).await;
    insert_ep_for_linking(&db, &format!("{}_S01E02", series_id), "Test Show", 1, 2).await;

    let general = jumbie_shared::config::GeneralConfig {
        unneeded_episodes_handling: "keep".to_string(),
        ..Default::default()
    };
    db.save_general_config(&general).await.unwrap();

    tokio::fs::create_dir_all(&content_dir)
        .await
        .expect("create content dir");
    tokio::fs::write(content_dir.join("Show.S01E01.mkv"), b"wanted video")
        .await
        .expect("write file");
    tokio::fs::write(content_dir.join("Show.S01E02.srt"), b"unwanted subtitle")
        .await
        .expect("write file");

    {
        let mut state = mock_state.lock().unwrap();
        state.completed.push(download_hash.to_string());
        state
            .statuses
            .insert(download_hash.to_string(), "uploading".to_string());
        state
            .content_paths
            .insert(download_hash.to_string(), content_path_str);
        state.progress.insert(download_hash.to_string(), 1.0);
    }

    let intentions = vec![
        EpisodeIntention {
            episode_num: 1,
            source_episode_num: 1,
            episode_id: format!("{}_S01E01", series_id),
            score: 100,
            keep: true,
        },
        EpisodeIntention {
            episode_num: 2,
            source_episode_num: 2,
            episode_id: format!("{}_S01E02", series_id),
            score: 100,
            keep: false,
        },
    ];
    let intentions_json = serde_json::to_string(&intentions).unwrap();
    let base_ep_id = format!("{}_S01E01", series_id);
    let queued = db
        .add_to_download_queue(AddToDownloadQueueParams {
            media_name: "Test Show S01E01-02",
            media_link: &magnet_with_hash(download_hash),
            series_title: "Test Show",
            series_id: "",
            seasons: &[1],
            episodes: &[1],
            episode_id: Some(&base_ep_id),
            score: 100,
            is_user_requested: false,
            is_season_pack: false,
            category: "",
            multi_targets: None,
            episode_intentions: Some(&intentions_json),
            source_pub_date: None,
            download_id: download_hash,
            scoring_size_bytes: None,
            scoring_seeders: None,
            scoring_episode_count: None,
            submitter: None,
            quality_profile_id: None,
            version: 1,
        })
        .await
        .expect("add_to_download_queue");
    let queue_id = match queued {
        jumbie_shared::types::AddQueueResult::Added { .. } => db
            .get_queued_items()
            .await
            .unwrap()
            .first()
            .map(|i| i.id)
            .unwrap(),
        _ => panic!("expected Added"),
    };
    db.update_queue_item_status(queue_id, "Downloading", Some(download_hash), None, None)
        .await
        .expect("update status");

    organizer
        .process_downloads()
        .await
        .expect("process_downloads");

    assert!(
        content_dir.join("Show.S01E02.srt").exists(),
        "unneeded subtitle must be left in place when handling=keep"
    );
}

// Season-alias handling in the permissive (series-scan) link path

fn alias_scan_mapping(
    series_id: &str,
    aliased_seasons: &[&str],
    library_root: &str,
) -> MappingRule {
    let mut mapping = MappingRule {
        target_title: "Charcoal Tabby".to_string(),
        name: "charcoal_tabby".to_string(),
        series_id: series_id.to_string(),
        ..Default::default()
    };
    // Keep the organize destination inside the test's temp dir so the test is
    // hermetic (otherwise files are moved to a relative path in the CWD).
    mapping.settings.path = Some(library_root.to_string());
    for key in aliased_seasons {
        mapping.settings.season.insert(
            key.to_string(),
            SeasonOverride {
                season: key.to_string(),
                aliases: vec!["Whisker Mini".to_string()],
                ..Default::default()
            },
        );
    }
    mapping
}

#[tokio::test]
async fn test_series_scan_unique_season_alias_resolves_season() {
    // A series-scan download whose title matches one season alias must be linked
    // to that alias's season, ignoring the (default) season number.
    let (organizer, db, _dl, mock_state, tmp) = setup_with_controllable_mock().await;
    let series_id = "alias-unique-test";
    let content_dir = tmp.path().join("alias-unique-scan");
    let content_path_str = content_dir.to_string_lossy().to_string();
    let download_hash = "alias_unique_hash";

    let library = tmp.path().join("library");
    let library_str = library.to_string_lossy().to_string();
    db.upsert_series_mapping(
        series_id,
        &alias_scan_mapping(series_id, &["0"], &library_str),
    )
    .await
    .expect("upsert_series_mapping");

    tokio::fs::create_dir_all(&content_dir).await.unwrap();
    let incoming = content_dir.join("Whisker Mini Anime - 01.mkv");
    tokio::fs::write(&incoming, b"incoming").await.unwrap();

    {
        let mut state = mock_state.lock().unwrap();
        state.completed.push(download_hash.to_string());
        state
            .statuses
            .insert(download_hash.to_string(), "uploading".to_string());
        state
            .content_paths
            .insert(download_hash.to_string(), content_path_str);
        state.progress.insert(download_hash.to_string(), 1.0);
    }

    let queued = db
        .add_to_download_queue(AddToDownloadQueueParams {
            media_name: "Whisker Mini Anime - 01",
            media_link: &magnet_with_hash(download_hash),
            series_title: "Charcoal Tabby",
            series_id,
            seasons: &[],
            episodes: &[],
            episode_id: None,
            score: 100,
            is_user_requested: true,
            is_season_pack: false,
            category: "Series",
            multi_targets: None,
            episode_intentions: None,
            source_pub_date: None,
            download_id: download_hash,
            scoring_size_bytes: None,
            scoring_seeders: None,
            scoring_episode_count: None,
            submitter: None,
            quality_profile_id: None,
            version: 1,
        })
        .await
        .expect("add_to_download_queue");
    assert!(matches!(
        queued,
        jumbie_shared::types::AddQueueResult::Added { .. }
    ));
    let queue_id = db
        .get_queued_items()
        .await
        .unwrap()
        .first()
        .map(|i| i.id)
        .unwrap();
    db.update_queue_item_status(queue_id, "Downloading", Some(download_hash), None, None)
        .await
        .expect("update status");

    organizer
        .process_downloads()
        .await
        .expect("process_downloads");

    let episodes = db
        .get_series_episodes_details(series_id, false)
        .await
        .expect("episodes");
    let linked = episodes
        .iter()
        .find(|e| {
            e.file_path
                .as_deref()
                .is_some_and(|p| p.ends_with("Whisker Mini Anime - 01.mkv"))
        })
        .expect("file should be linked to an episode");
    assert_eq!(
        linked.season,
        Some(0),
        "the season alias (season 0) must win over the default season"
    );
}

#[tokio::test]
async fn test_series_scan_ambiguous_season_alias_is_unmatched() {
    // Two seasons share the alias and the filename has no season number → the
    // file cannot be assigned and must be treated as unmatched (left for review
    // under unexpected_files_handling).
    let (organizer, db, _dl, mock_state, tmp) = setup_with_controllable_mock().await;
    let series_id = "alias-ambiguous-test";
    let content_dir = tmp.path().join("alias-ambiguous-scan");
    let content_path_str = content_dir.to_string_lossy().to_string();
    let download_hash = "alias_ambiguous_hash";

    let library = tmp.path().join("library");
    let library_str = library.to_string_lossy().to_string();
    db.upsert_series_mapping(
        series_id,
        &alias_scan_mapping(series_id, &["0", "2"], &library_str),
    )
    .await
    .expect("upsert_series_mapping");

    let general = jumbie_shared::config::GeneralConfig {
        unexpected_files_handling: "keep".to_string(),
        ..Default::default()
    };
    db.save_general_config(&general).await.unwrap();

    tokio::fs::create_dir_all(&content_dir).await.unwrap();
    let incoming = content_dir.join("Whisker Mini Anime E01.mkv");
    tokio::fs::write(&incoming, b"incoming").await.unwrap();

    {
        let mut state = mock_state.lock().unwrap();
        state.completed.push(download_hash.to_string());
        state
            .statuses
            .insert(download_hash.to_string(), "uploading".to_string());
        state
            .content_paths
            .insert(download_hash.to_string(), content_path_str);
        state.progress.insert(download_hash.to_string(), 1.0);
    }

    let queued = db
        .add_to_download_queue(AddToDownloadQueueParams {
            media_name: "Whisker Mini Anime E01",
            media_link: &magnet_with_hash(download_hash),
            series_title: "Charcoal Tabby",
            series_id,
            seasons: &[],
            episodes: &[],
            episode_id: None,
            score: 100,
            is_user_requested: true,
            is_season_pack: false,
            category: "Series",
            multi_targets: None,
            episode_intentions: None,
            source_pub_date: None,
            download_id: download_hash,
            scoring_size_bytes: None,
            scoring_seeders: None,
            scoring_episode_count: None,
            submitter: None,
            quality_profile_id: None,
            version: 1,
        })
        .await
        .expect("add_to_download_queue");
    assert!(matches!(
        queued,
        jumbie_shared::types::AddQueueResult::Added { .. }
    ));
    let queue_id = db
        .get_queued_items()
        .await
        .unwrap()
        .first()
        .map(|i| i.id)
        .unwrap();
    db.update_queue_item_status(queue_id, "Downloading", Some(download_hash), None, None)
        .await
        .expect("update status");

    organizer
        .process_downloads()
        .await
        .expect("process_downloads");

    let incoming_str = incoming.to_string_lossy().to_string();
    let episodes = db
        .get_series_episodes_details(series_id, false)
        .await
        .expect("episodes");
    assert!(
        episodes
            .iter()
            .all(|e| e.file_path.as_deref() != Some(incoming_str.as_str())),
        "an ambiguous season alias must not be linked to any episode"
    );
    assert!(
        incoming.exists(),
        "unmatched file must be left for review when handling=keep"
    );
    let fingerprint_episode_id: Option<String> = sqlx::query_scalar(
        "SELECT ef.episode_id FROM episode_files ef \
         JOIN file_paths fp ON fp.id = ef.file_path_id \
         WHERE fp.file_path = ? LIMIT 1",
    )
    .bind(&incoming_str)
    .fetch_optional(db.get_pool())
    .await
    .expect("fingerprint query");
    assert_eq!(
        fingerprint_episode_id, None,
        "an unmatched file must not be linked to a fake (sentinel) episode id"
    );
    assert!(
        !db.get_orphan_files()
            .await
            .expect("orphan query")
            .contains(&incoming_str),
        "a kept-for-review file must be excluded from orphan adoption"
    );
    let review_row_series: Option<String> =
        sqlx::query_scalar("SELECT series_id FROM unmatched_files WHERE file_path = ?")
            .bind(&incoming_str)
            .fetch_optional(db.get_pool())
            .await
            .expect("unmatched_files query")
            .flatten();
    assert_eq!(
        review_row_series.as_deref(),
        Some(series_id),
        "the review row must be scoped to the downloading series"
    );
}

// Blocked files: orphan adoption (discovery) must defer to the block

#[tokio::test]
async fn test_orphan_adoption_respects_blocked_files() {
    let (organizer, db, _dl, _mock_state, tmp) = setup_with_controllable_mock().await;
    let series_id = "blocked-orphan-test";

    let mapping = MappingRule {
        target_title: "Test Show".to_string(),
        name: "test_show".to_string(),
        series_id: series_id.to_string(),
        ..Default::default()
    };
    db.upsert_series_mapping(series_id, &mapping)
        .await
        .expect("upsert_series_mapping");

    // Guard against a false-positive test: the orphan filename must resolve to
    // this mapping, otherwise nothing would be adopted and the block assertion
    // would pass vacuously.
    let probe_key = jumbie::utils::generate_series_key("Test Show S01E01.mkv");
    assert!(
        db.get_mapping_by_key(&probe_key)
            .await
            .expect("mapping lookup")
            .is_some(),
        "series key {probe_key:?} must resolve to the seeded mapping"
    );

    // Two orphan files: on disk, fingerprinted, no episode association.
    let orphan_dir = tmp.path().join("orphans");
    std::fs::create_dir_all(&orphan_dir).unwrap();
    let orphan_e1 = orphan_dir.join("Test Show S01E01.mkv");
    let orphan_e2 = orphan_dir.join("Test Show S01E02.mkv");
    std::fs::write(&orphan_e1, b"orphan-one").unwrap();
    std::fs::write(&orphan_e2, b"orphan-two").unwrap();
    let e1_str = orphan_e1.to_string_lossy().to_string();
    let e2_str = orphan_e2.to_string_lossy().to_string();
    for path in [&e1_str, &e2_str] {
        let size = std::fs::metadata(path).unwrap().len() as i64;
        sqlx::query(
            "INSERT INTO file_paths (file_path, fingerprint, size, state) \
             VALUES (?, 'orphan', ?, 'complete')",
        )
        .bind(path)
        .bind(size)
        .execute(db.get_pool())
        .await
        .unwrap();
    }

    // E01 was unassigned by the user as wrong → durable block. E02 is free to adopt.
    let e1_size = std::fs::metadata(&orphan_e1).unwrap().len() as i64;
    db.block_file(series_id, "Test Show S01E01.mkv", e1_size, None)
        .await
        .unwrap();

    // The episode row must exist first: `episode_files` references `episodes`.
    insert_ep_for_linking(&db, &format!("{series_id}_S01E02"), "Test Show", 1, 2).await;

    organizer
        .process_downloads()
        .await
        .expect("process_downloads");

    // The blocked orphan must NOT be linked…
    let e1_fp: Option<String> = sqlx::query_scalar(
        "SELECT ef.episode_id FROM episode_files ef \
         JOIN file_paths fp ON fp.id = ef.file_path_id \
         WHERE fp.file_path = ? LIMIT 1",
    )
    .bind(&e1_str)
    .fetch_optional(db.get_pool())
    .await
    .unwrap();
    assert!(e1_fp.is_none(), "a blocked orphan must not be adopted");

    // …while the unblocked orphan still is (proves the mapping resolved and the
    // gate is what prevented adoption, not a lookup miss).
    let e2_fp: Option<String> = sqlx::query_scalar(
        "SELECT ef.episode_id FROM episode_files ef \
         JOIN file_paths fp ON fp.id = ef.file_path_id \
         WHERE fp.file_path = ? LIMIT 1",
    )
    .bind(&e2_str)
    .fetch_optional(db.get_pool())
    .await
    .unwrap();
    let expected = format!("{series_id}_S01E02");
    assert_eq!(
        e2_fp.as_deref(),
        Some(expected.as_str()),
        "the unblocked orphan must be adopted"
    );
}

// Season aliases must apply to orphan adoption (discovery), not just scans.

fn alias_orphan_mapping(
    series_id: &str,
    aliased_seasons: &[&str],
    library_root: &str,
) -> MappingRule {
    let mut mapping = MappingRule {
        target_title: "Whisker Mini Anime".to_string(),
        name: jumbie::utils::generate_series_key("Whisker Mini Anime - 01.mkv"),
        series_id: series_id.to_string(),
        ..Default::default()
    };
    // Keep the organize destination inside the test's temp dir so the test is
    // hermetic.
    mapping.settings.path = Some(library_root.to_string());
    for key in aliased_seasons {
        mapping.settings.season.insert(
            key.to_string(),
            SeasonOverride {
                season: key.to_string(),
                aliases: vec!["Whisker Mini".to_string()],
                ..Default::default()
            },
        );
    }
    mapping
}

/// Seed a fingerprinted orphan file on disk and return its path string.
async fn seed_orphan(
    db: &Arc<jumbie::db::DbManager>,
    dir: &std::path::Path,
    filename: &str,
) -> String {
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(filename);
    std::fs::write(&path, b"orphan").unwrap();
    let path_str = path.to_string_lossy().to_string();
    let size = std::fs::metadata(&path).unwrap().len() as i64;
    sqlx::query(
        "INSERT INTO file_paths (file_path, fingerprint, size, state) \
         VALUES (?, 'orphan', ?, 'complete')",
    )
    .bind(&path_str)
    .bind(size)
    .execute(db.get_pool())
    .await
    .unwrap();
    path_str
}

#[tokio::test]
async fn test_orphan_adoption_uses_season_alias() {
    // An orphan whose title matches a season alias must be adopted into that
    // alias's season, exactly like the scan and permissive-link paths.
    let (organizer, db, _dl, _mock_state, tmp) = setup_with_controllable_mock().await;
    let series_id = "alias-orphan-unique";
    let library = tmp.path().join("library");

    db.upsert_series_mapping(
        series_id,
        &alias_orphan_mapping(series_id, &["0"], &library.to_string_lossy()),
    )
    .await
    .expect("upsert_series_mapping");

    // Guard against a vacuous test: the orphan filename must resolve to the
    // seeded mapping, otherwise nothing would be adopted.
    let probe_key = jumbie::utils::generate_series_key("Whisker Mini Anime - 01.mkv");
    assert!(
        db.get_mapping_by_key(&probe_key)
            .await
            .expect("mapping lookup")
            .is_some(),
        "series key {probe_key:?} must resolve to the seeded mapping"
    );

    let orphan_str = seed_orphan(
        &db,
        &tmp.path().join("orphans"),
        "Whisker Mini Anime - 01.mkv",
    )
    .await;

    // The episode row must exist first: `episode_files` references `episodes`.
    insert_ep_for_linking(
        &db,
        &format!("{series_id}_S00E01"),
        "Whisker Mini Anime",
        0,
        1,
    )
    .await;

    organizer
        .process_downloads()
        .await
        .expect("process_downloads");

    let fp: Option<String> = sqlx::query_scalar(
        "SELECT ef.episode_id FROM episode_files ef \
         JOIN file_paths fp ON fp.id = ef.file_path_id \
         WHERE fp.file_path = ? LIMIT 1",
    )
    .bind(&orphan_str)
    .fetch_optional(db.get_pool())
    .await
    .unwrap();
    let fp = fp.expect("orphan should have been adopted");
    assert!(
        fp.ends_with("_S00E01"),
        "the season alias (season 0) must resolve during adoption, got {fp}"
    );
}

#[tokio::test]
async fn test_orphan_adoption_ambiguous_season_alias_marked_for_review() {
    // Two seasons share the alias and the filename carries no season number →
    // the orphan cannot be placed; it must be routed to review instead of being
    // retried forever.
    let (organizer, db, _dl, _mock_state, tmp) = setup_with_controllable_mock().await;
    let series_id = "alias-orphan-ambiguous";
    let library = tmp.path().join("library");

    db.upsert_series_mapping(
        series_id,
        &alias_orphan_mapping(series_id, &["0", "2"], &library.to_string_lossy()),
    )
    .await
    .expect("upsert_series_mapping");

    let orphan_str = seed_orphan(
        &db,
        &tmp.path().join("orphans-ambiguous"),
        "Whisker Mini Anime - 01.mkv",
    )
    .await;

    organizer
        .process_downloads()
        .await
        .expect("process_downloads");

    let fp: Option<String> = sqlx::query_scalar(
        "SELECT ef.episode_id FROM episode_files ef \
         JOIN file_paths fp ON fp.id = ef.file_path_id \
         WHERE fp.file_path = ? LIMIT 1",
    )
    .bind(&orphan_str)
    .fetch_optional(db.get_pool())
    .await
    .unwrap();
    assert!(
        fp.is_none(),
        "an ambiguous season alias orphan must not be adopted"
    );

    let review: Option<(Option<String>, Option<String>)> =
        sqlx::query_as("SELECT series_id, reason FROM unmatched_files WHERE file_path = ?")
            .bind(&orphan_str)
            .fetch_optional(db.get_pool())
            .await
            .unwrap();
    let (review_series, review_reason) =
        review.expect("ambiguous orphan must be recorded for review");
    assert_eq!(review_series.as_deref(), Some(series_id));
    assert_eq!(review_reason.as_deref(), Some("unmatched"));
}

// No-progress detection and autoresolve.

/// Seed a `Downloading` queue item whose tracker reports a frozen progress and a
/// content path that does not exist on disk (so it stays in the polling branch).
/// Returns the queue item id and the link used.
async fn seed_downloading_item(
    db: &jumbie::db::DbManager,
    mock_state: &Arc<Mutex<MockState>>,
    series_id: &str,
    episode_id: &str,
    download_hash: &str,
    is_user_requested: bool,
) -> i64 {
    db.upsert_series_mapping(
        series_id,
        &MappingRule {
            target_title: "Test Show".to_string(),
            series_id: series_id.to_string(),
            name: "test_show".to_string(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    insert_test_episode(db, episode_id, series_id).await;

    let link = magnet_with_hash(download_hash);
    let result = db
        .add_to_download_queue(AddToDownloadQueueParams {
            media_name: "Test Show S01E01",
            media_link: &link,
            series_title: "Test Show",
            series_id,
            seasons: &[1],
            episodes: &[1],
            episode_id: Some(episode_id),
            score: 100,
            is_user_requested,
            is_season_pack: false,
            category: "",
            multi_targets: None,
            episode_intentions: None,
            source_pub_date: None,
            download_id: download_hash,
            scoring_size_bytes: None,
            scoring_seeders: None,
            scoring_episode_count: None,
            submitter: None,
            quality_profile_id: None,
            version: 1,
        })
        .await
        .expect("add_to_download_queue");
    assert!(matches!(
        result,
        jumbie_shared::types::AddQueueResult::Added { .. }
    ));
    let id = db
        .get_queued_items()
        .await
        .unwrap()
        .first()
        .map(|i| i.id)
        .expect("queued item");
    db.update_queue_item_status(
        id,
        "Downloading",
        Some(download_hash),
        Some("mock_dl"),
        None,
    )
    .await
    .unwrap();

    // Content path exists from the client's view but not on disk.
    {
        let mut state = mock_state.lock().unwrap();
        state.content_paths.insert(
            download_hash.to_string(),
            format!("/tmp/jumbie-nonexistent/{download_hash}.mkv"),
        );
        state.progress.insert(download_hash.to_string(), 0.25);
    }
    id
}

/// Backdate a queue item's no-progress window so the threshold has elapsed.
async fn backdate_no_progress(db: &jumbie::db::DbManager, id: i64, minutes: i64) {
    let since = chrono::Utc::now().naive_utc() - chrono::Duration::minutes(minutes);
    sqlx::query(
        "UPDATE download_queue SET last_progress = 0.25, no_progress_since = ? WHERE id = ?",
    )
    .bind(since)
    .bind(id)
    .execute(db.get_pool())
    .await
    .unwrap();
}

/// Mark a queued item as a user-picked (manual) release, so no-progress handling
/// treats it as deliberate and leaves it alone.
async fn mark_manual(db: &jumbie::db::DbManager, id: i64) {
    sqlx::query("UPDATE download_queue SET is_manual = 1 WHERE id = ?")
        .bind(id)
        .execute(db.get_pool())
        .await
        .unwrap();
}

#[tokio::test]
async fn test_reported_failure_marks_item_failed() {
    let (organizer, db, _dl, mock_state, _tmp) = setup_with_controllable_mock().await;
    let hash = "failure_reported_hash_0001";
    let id =
        seed_downloading_item(&db, &mock_state, "failure-show", "failure_ep", hash, false).await;

    // A hard failure presents as frozen progress; the probe runs once stalled.
    backdate_no_progress(&db, id, 31).await;
    mock_state
        .lock()
        .unwrap()
        .failures
        .insert(hash.to_string(), "torrent errored".to_string());

    organizer.process_download_queue().await.unwrap();

    let item = db
        .get_download_queue()
        .await
        .unwrap()
        .into_iter()
        .find(|i| i.id == id)
        .expect("item retained as Failed");
    assert_eq!(item.status, "Failed");
    // The plugin's reason wins over the generic no-progress message, and the item is
    // never auto-resolved (still present, no rejection recorded).
    assert_eq!(item.error_message.as_deref(), Some("torrent errored"));
    assert!(
        db.get_rejected_downloads().await.unwrap().is_empty(),
        "an explicitly failed release must not be treated as a stall"
    );
}

#[tokio::test]
async fn test_no_progress_automatic_item_is_removed_and_denied() {
    let (organizer, db, _dl, mock_state, _tmp) = setup_with_controllable_mock().await;
    let hash = "no_progress_auto_hash_0002";
    let id = seed_downloading_item(&db, &mock_state, "auto-show", "auto_ep", hash, false).await;
    backdate_no_progress(&db, id, 31).await;

    organizer.process_download_queue().await.unwrap();

    assert!(
        db.get_download_queue()
            .await
            .unwrap()
            .iter()
            .all(|i| i.id != id),
        "stalled automatic item must be removed"
    );
    let rejected = db.get_rejected_downloads().await.unwrap();
    let link = magnet_with_hash(hash);
    assert!(
        rejected.contains(Some(link.as_str()), Some(hash)),
        "removed release must be rejected so re-search skips it"
    );
}

/// A stall consumes exactly one attempt. Once autoresolve removes the item, the
/// next pass sees no Downloading row for that episode and must not re-count the
/// stall. Guards the remove-before-re-trigger ordering in `autoresolve_no_progress`.
#[tokio::test]
async fn test_no_progress_attempt_not_recounted_on_next_pass() {
    let (organizer, db, _dl, mock_state, _tmp) = setup_with_controllable_mock().await;
    let series_id = "idempotent-stall-show";
    let hash = "idempotent_stall_hash_0001";
    let id = seed_downloading_item(
        &db,
        &mock_state,
        series_id,
        "idempotent_stall_ep",
        hash,
        false,
    )
    .await;
    backdate_no_progress(&db, id, 31).await;

    let target = RejectionTarget {
        series_id: Some(series_id),
        season: Some(1),
        episode: Some(1),
    };

    organizer.process_download_queue().await.unwrap();
    assert_eq!(
        db.get_autoresolve_attempts(target, 12, 2).await.unwrap(),
        1,
        "first stall spends one attempt"
    );

    // The item is gone, so a second pass must not spend another attempt.
    organizer.process_download_queue().await.unwrap();
    assert_eq!(
        db.get_autoresolve_attempts(target, 12, 2).await.unwrap(),
        1,
        "a repeated pass must not re-count an already-resolved stall"
    );
}

#[tokio::test]
async fn test_no_progress_manual_item_is_kept() {
    let (organizer, db, _dl, mock_state, _tmp) = setup_with_controllable_mock().await;
    let hash = "no_progress_manual_hash_0003";
    let id = seed_downloading_item(&db, &mock_state, "manual-show", "manual_ep", hash, true).await;
    // User-picked release: both user-initiated and manual.
    mark_manual(&db, id).await;
    backdate_no_progress(&db, id, 31).await;

    organizer.process_download_queue().await.unwrap();

    let item = db
        .get_download_queue()
        .await
        .unwrap()
        .into_iter()
        .find(|i| i.id == id)
        .expect("manual item must be kept");
    assert_eq!(item.status, "Downloading");
    assert!(db.get_rejected_downloads().await.unwrap().is_empty());

    // Manual downloads are excluded before the budget is read: zero attempts.
    let target = RejectionTarget {
        series_id: Some("manual-show"),
        season: Some(1),
        episode: Some(1),
    };
    assert_eq!(
        db.get_autoresolve_attempts(target, 12, 2).await.unwrap(),
        0,
        "manual downloads must not consume the autoresolve attempt budget"
    );
}

/// A user-initiated auto-search (is_user_requested) is not a user-picked release,
/// so a stall is still autoresolved. Guards the `is_manual` vs `is_user_requested`
/// split in `track_no_progress`.
#[tokio::test]
async fn test_no_progress_user_initiated_auto_item_is_autoresolved() {
    let (organizer, db, _dl, mock_state, _tmp) = setup_with_controllable_mock().await;
    let series_id = "user-auto-show";
    let hash = "user_auto_hash_0001";
    let id = seed_downloading_item(&db, &mock_state, series_id, "user_auto_ep", hash, true).await;
    backdate_no_progress(&db, id, 31).await;

    organizer.process_download_queue().await.unwrap();

    assert!(
        db.get_download_queue()
            .await
            .unwrap()
            .iter()
            .all(|i| i.id != id),
        "a user-initiated auto-search stall must still be autoresolved"
    );
    let target = RejectionTarget {
        series_id: Some(series_id),
        season: Some(1),
        episode: Some(1),
    };
    assert_eq!(
        db.get_autoresolve_attempts(target, 12, 2).await.unwrap(),
        1,
        "resolving a user-initiated auto-search spends one attempt"
    );
}

#[tokio::test]
async fn test_no_progress_resumes_without_action_when_progress_advances() {
    let (organizer, db, _dl, mock_state, _tmp) = setup_with_controllable_mock().await;
    let hash = "no_progress_resume_hash_0004";
    let id = seed_downloading_item(&db, &mock_state, "resume-show", "resume_ep", hash, false).await;
    backdate_no_progress(&db, id, 31).await;

    // Progress advanced past the recorded sample → the window resets, no action.
    mock_state
        .lock()
        .unwrap()
        .progress
        .insert(hash.to_string(), 0.5);
    organizer.process_download_queue().await.unwrap();

    let item = db
        .get_download_queue()
        .await
        .unwrap()
        .into_iter()
        .find(|i| i.id == id)
        .expect("item retained");
    assert_eq!(item.status, "Downloading");
    assert_eq!(item.last_progress, Some(0.5));
}

#[tokio::test]
async fn test_no_progress_budget_exhaustion_marks_failed() {
    let (organizer, db, _dl, mock_state, _tmp) = setup_with_controllable_mock().await;

    // Two autor resolves are allowed; a third stall then fails.
    let mut last_id = 0i64;
    for n in 1..=3 {
        let hash = format!("budget_exhaust_hash_000{n}");
        let id =
            seed_downloading_item(&db, &mock_state, "budget-show", "budget_ep", &hash, false).await;
        backdate_no_progress(&db, id, 31).await;
        organizer.process_download_queue().await.unwrap();
        last_id = id;
    }

    let item = db
        .get_download_queue()
        .await
        .unwrap()
        .into_iter()
        .find(|i| i.id == last_id)
        .expect("third item kept as Failed");
    assert_eq!(item.status, "Failed");
    assert!(
        item.error_message
            .as_deref()
            .unwrap_or("")
            .contains("alternative attempt"),
        "error should explain the exhausted budget: {:?}",
        item.error_message
    );

    // The release that exhausted the budget must also be excluded, so the retry
    // after the cooldown does not re-pick it.
    let third_link = magnet_with_hash("budget_exhaust_hash_0003");
    let rejected = db.get_rejected_downloads().await.unwrap();
    assert!(
        rejected.contains(Some(third_link.as_str()), Some("budget_exhaust_hash_0003")),
        "the final failing release must be rejected"
    );
}

#[tokio::test]
async fn test_successful_download_resets_autoresolve_budget() {
    let (organizer, db, _dl, mock_state, tmp) = setup_with_controllable_mock().await;
    let series_id = "reset-success-show";
    let episode_id = format!("{series_id}_S01E01");
    let hash = "reset_success_hash_0001";

    let target = RejectionTarget {
        series_id: Some(series_id),
        season: Some(1),
        episode: Some(1),
    };
    // Spend the budget first (as if two alternatives had stalled).
    db.increment_autoresolve_attempts(target, 12, 2)
        .await
        .unwrap();
    db.increment_autoresolve_attempts(target, 12, 2)
        .await
        .unwrap();
    assert_eq!(db.get_autoresolve_attempts(target, 12, 2).await.unwrap(), 2);

    let _id = seed_downloading_item(&db, &mock_state, series_id, &episode_id, hash, false).await;

    // A real content directory whose file smart-link assigns to S01E01, so
    // finalize reaches the Completed branch.
    let content_dir = tmp.path().join("Reset.Success.S01E01");
    tokio::fs::create_dir_all(&content_dir).await.unwrap();
    tokio::fs::write(content_dir.join("Test.Show.S01E01.mkv"), b"video")
        .await
        .unwrap();
    mock_state
        .lock()
        .unwrap()
        .content_paths
        .insert(hash.to_string(), content_dir.to_string_lossy().to_string());

    organizer.process_download_queue().await.unwrap();

    assert_eq!(
        db.get_autoresolve_attempts(target, 12, 2).await.unwrap(),
        0,
        "a successful download must clear the episode's attempt budget"
    );
}

/// Register a mock source returning `entries` for every auto-search.
async fn add_mock_source(organizer: &ContentOrganizer, entries: Vec<serde_json::Value>) {
    let plugin: Arc<dyn PluginInstance> = Arc::new(MockSource { entries });
    organizer
        .plugin_manager()
        .write()
        .await
        .add_internal_plugin(plugin);
}

// The re-search used by autoresolve must not re-select a rejected release.

const REJECT_SEARCH_SERIES: &str = "reject-search-show";
/// Scores 100 via the mapping's `1080p` term, so it is the clear winner.
const REJECT_SEARCH_BAD: &str = "magnet:?xt=urn:btih:1111111111111111111111111111111111111111";
const REJECT_SEARCH_GOOD: &str = "magnet:?xt=urn:btih:2222222222222222222222222222222222222222";

/// Series + episode + a source offering a high-scoring (1080p) and a
/// low-scoring (720p) release for S01E01, with no rejection applied yet.
async fn reject_search_env() -> (
    ContentOrganizer,
    Arc<jumbie::db::DbManager>,
    tempfile::TempDir,
) {
    let (organizer, db, _dl, _mock_state, tmp) = setup_with_controllable_mock().await;

    db.upsert_series_mapping(
        REJECT_SEARCH_SERIES,
        &MappingRule {
            target_title: "Test Show".to_string(),
            name: "test_show".to_string(),
            series_id: REJECT_SEARCH_SERIES.to_string(),
            scoring: Some(jumbie_shared::scoring::ReleaseProfile {
                terms: HashMap::from([("1080p".to_string(), 100)]),
                ..Default::default()
            }),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    insert_test_episode(
        &db,
        &format!("{REJECT_SEARCH_SERIES}_S01E01"),
        REJECT_SEARCH_SERIES,
    )
    .await;

    add_mock_source(
        &organizer,
        vec![
            serde_json::json!({
                "title": "Test Show S01E01 1080p",
                "source": "Mock",
                "link": REJECT_SEARCH_BAD,
                "download_url": REJECT_SEARCH_BAD,
                "download_id": "badhash",
                "size": 1_000_000_000u64,
                "seeders": 50
            }),
            serde_json::json!({
                "title": "Test Show S01E01 720p",
                "source": "Mock",
                "link": REJECT_SEARCH_GOOD,
                "download_url": REJECT_SEARCH_GOOD,
                "download_id": "goodhash",
                "size": 900_000_000u64,
                "seeders": 20
            }),
        ],
    )
    .await;

    (organizer, db, tmp)
}

// Control: with no rejection, the highest-scoring release wins. This proves the
// filter test below is exercising the rejection, not a broken search harness.
#[tokio::test]
async fn test_auto_search_missing_queues_best_release_when_not_rejected() {
    let (organizer, db, _tmp) = reject_search_env().await;

    organizer
        .auto_search_missing(REJECT_SEARCH_SERIES, "01", &[1])
        .await
        .unwrap();

    let queue = db.get_download_queue().await.unwrap();
    assert_eq!(queue.len(), 1);
    assert_eq!(
        queue[0].media_link, REJECT_SEARCH_BAD,
        "the 1080p release scores higher and should be the winner"
    );
}

#[tokio::test]
async fn test_auto_search_missing_skips_rejected_release() {
    let (organizer, db, _tmp) = reject_search_env().await;

    // Exclude the release that would otherwise win.
    let target = RejectionTarget {
        series_id: Some(REJECT_SEARCH_SERIES),
        season: Some(1),
        episode: Some(1),
    };
    db.reject_download(
        REJECT_SEARCH_BAD,
        Some("badhash"),
        "no progress",
        target,
        chrono::Utc::now().naive_utc() + chrono::Duration::hours(48),
    )
    .await
    .unwrap();

    organizer
        .auto_search_missing(REJECT_SEARCH_SERIES, "01", &[1])
        .await
        .unwrap();

    let queue = db.get_download_queue().await.unwrap();
    assert_eq!(
        queue.len(),
        1,
        "exactly one release should be queued: {:?}",
        queue.iter().map(|i| &i.media_link).collect::<Vec<_>>()
    );
    assert_eq!(
        queue[0].media_link, REJECT_SEARCH_GOOD,
        "the rejected winner must be skipped in favour of the next release"
    );
}

// Result scoping for auto-search is parse-based: the search template only shapes
// the query sent to the source (it may carry source-specific syntax). A release
// is accepted when it parses to the searched season + episode; when the template
// contains `${season}` the season must be declared, and a bare-number "hail mary"
// (`Test Show 2 - 45` for S02E45) can still place an otherwise unparseable title.

const SEASON_SCOPE_SERIES: &str = "season-scope-show";
/// A release that should be accepted; lower score, so it only wins when the
/// competing result is rejected.
const KEPT_LINK: &str = "magnet:?xt=urn:btih:3333333333333333333333333333333333333333";
/// A release the scope rules must reject (wrong season, no episode, reversed
/// numbers, …); scores higher, so it would be queued if wrongly accepted.
const DROPPED_LINK: &str = "magnet:?xt=urn:btih:4444444444444444444444444444444444444444";

/// One mock source result. The series scoring (below) only rewards `1080p`.
fn scope_entry(title: &str, link: &str) -> serde_json::Value {
    serde_json::json!({
        "title": title,
        "source": "Mock",
        "link": link,
        "download_url": link,
        "download_id": link,
        "size": 1_000_000_000u64,
        "seeders": 50
    })
}

/// Series with the given search template, an existing episode row (`episode_row_id`
/// is the `get_episode_id` the parser will expect), and mock source results.
/// Scoring only rewards `1080p`.
async fn season_scope_env(
    search_format: Option<&str>,
    episode_row_id: &str,
    entries: Vec<serde_json::Value>,
) -> (
    ContentOrganizer,
    Arc<jumbie::db::DbManager>,
    tempfile::TempDir,
) {
    let (organizer, db, _dl, _mock_state, tmp) = setup_with_controllable_mock().await;

    db.upsert_series_mapping(
        SEASON_SCOPE_SERIES,
        &MappingRule {
            target_title: "Test Show".to_string(),
            name: "test_show".to_string(),
            series_id: SEASON_SCOPE_SERIES.to_string(),
            scoring: Some(jumbie_shared::scoring::ReleaseProfile {
                terms: HashMap::from([("1080p".to_string(), 100)]),
                ..Default::default()
            }),
            settings: jumbie_shared::types::SeriesSettings {
                search_format: search_format.map(str::to_string),
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .await
    .unwrap();
    insert_test_episode(&db, episode_row_id, SEASON_SCOPE_SERIES).await;

    add_mock_source(&organizer, entries).await;

    (organizer, db, tmp)
}

#[tokio::test]
async fn test_auto_search_blocks_seasonless_when_template_scopes_season() {
    // The template contains `${season}`, so the season-less release (higher score)
    // is blocked and the declared season-1 release wins.
    let (organizer, db, _tmp) = season_scope_env(
        Some("S${season:02}E${episode:02}"),
        "season-scope-show_S01E01",
        vec![
            scope_entry("Test Show - 01 1080p", DROPPED_LINK),
            scope_entry("Test Show S01E01 720p", KEPT_LINK),
        ],
    )
    .await;

    organizer
        .auto_search_missing(SEASON_SCOPE_SERIES, "01", &[1])
        .await
        .unwrap();

    let queue = db.get_download_queue().await.unwrap();
    assert_eq!(
        queue.len(),
        1,
        "exactly one release should be queued: {:?}",
        queue.iter().map(|i| &i.media_link).collect::<Vec<_>>()
    );
    assert_eq!(
        queue[0].media_link, KEPT_LINK,
        "a season-less release must be blocked when the template scopes the season"
    );
}

#[tokio::test]
async fn test_auto_search_allows_seasonless_when_template_has_no_season() {
    // Without `${season}`, the lenient parse may place a season-less release.
    let (organizer, db, _tmp) = season_scope_env(
        Some("${episode:02}"),
        "season-scope-show_S01E01",
        vec![scope_entry("Test Show - 01 1080p", KEPT_LINK)],
    )
    .await;

    organizer
        .auto_search_missing(SEASON_SCOPE_SERIES, "01", &[1])
        .await
        .unwrap();

    let queue = db.get_download_queue().await.unwrap();
    assert_eq!(
        queue.len(),
        1,
        "exactly one release should be queued: {:?}",
        queue.iter().map(|i| &i.media_link).collect::<Vec<_>>()
    );
    assert_eq!(
        queue[0].media_link, KEPT_LINK,
        "an episode-only template accepts the season-less release"
    );
}

#[tokio::test]
async fn test_auto_search_matching_ignores_template_syntax() {
    // The template carries source-specific alternation syntax, rendering a token no
    // release contains. Matching is parse-based, so the release is still accepted.
    let (organizer, db, _tmp) = season_scope_env(
        Some("S${season:02}E${episode:02}|${episode:02}"),
        "season-scope-show_S01E01",
        vec![scope_entry("Test Show S01E01 1080p", KEPT_LINK)],
    )
    .await;

    organizer
        .auto_search_missing(SEASON_SCOPE_SERIES, "01", &[1])
        .await
        .unwrap();

    let queue = db.get_download_queue().await.unwrap();
    assert_eq!(
        queue.len(),
        1,
        "exactly one release should be queued: {:?}",
        queue.iter().map(|i| &i.media_link).collect::<Vec<_>>()
    );
    assert_eq!(
        queue[0].media_link, KEPT_LINK,
        "template syntax must not gate matching"
    );
}

#[tokio::test]
async fn test_auto_search_hail_mary_matches_ordered_bare_numbers() {
    // `Test Show 2 - 45` has no structured season/episode markers (the parser
    // reads it as season 1, episode 45), so a season-2 search is only satisfied by
    // the bare-number hail mary. The reversed decoy must be rejected. The episode
    // row matches what the parser expects (S01E45), so the candidate is viable.
    let (organizer, db, _tmp) = season_scope_env(
        Some("S${season:02}E${episode:02}"),
        "season-scope-show_S01E45",
        vec![
            scope_entry("Test Show 45 - 2 1080p", DROPPED_LINK),
            scope_entry("Test Show 2 - 45 720p", KEPT_LINK),
        ],
    )
    .await;

    organizer
        .auto_search_missing(SEASON_SCOPE_SERIES, "02", &[45])
        .await
        .unwrap();

    let queue = db.get_download_queue().await.unwrap();
    assert_eq!(
        queue.len(),
        1,
        "exactly one release should be queued: {:?}",
        queue.iter().map(|i| &i.media_link).collect::<Vec<_>>()
    );
    assert_eq!(
        queue[0].media_link, KEPT_LINK,
        "the ordered bare-number release must be accepted"
    );
}

#[tokio::test]
async fn test_auto_search_missing_rejects_wrong_season() {
    // The wrong-season result scores higher, so it only stays out of the queue
    // because its parsed season does not match the search.
    let (organizer, db, _tmp) = season_scope_env(
        Some("S${season:02}E${episode:02}"),
        "season-scope-show_S01E01",
        vec![
            scope_entry("Test Show S02E01 1080p", DROPPED_LINK),
            scope_entry("Test Show S01E01 720p", KEPT_LINK),
        ],
    )
    .await;

    organizer
        .auto_search_missing(SEASON_SCOPE_SERIES, "01", &[1])
        .await
        .unwrap();

    let queue = db.get_download_queue().await.unwrap();
    assert_eq!(
        queue.len(),
        1,
        "exactly one release should be queued: {:?}",
        queue.iter().map(|i| &i.media_link).collect::<Vec<_>>()
    );
    assert_eq!(
        queue[0].media_link, KEPT_LINK,
        "a wrong-season result must be rejected"
    );
}

#[tokio::test]
async fn test_auto_search_missing_rejects_unparseable_result() {
    // A result with no parseable episode scores higher but must be rejected.
    let (organizer, db, _tmp) = season_scope_env(
        Some("S${season:02}E${episode:02}"),
        "season-scope-show_S01E01",
        vec![
            scope_entry("Test Show 1080p", DROPPED_LINK),
            scope_entry("Test Show S01E01 720p", KEPT_LINK),
        ],
    )
    .await;

    organizer
        .auto_search_missing(SEASON_SCOPE_SERIES, "01", &[1])
        .await
        .unwrap();

    let queue = db.get_download_queue().await.unwrap();
    assert_eq!(
        queue.len(),
        1,
        "exactly one release should be queued: {:?}",
        queue.iter().map(|i| &i.media_link).collect::<Vec<_>>()
    );
    assert_eq!(
        queue[0].media_link, KEPT_LINK,
        "a result with no parseable episode must be rejected"
    );
}

// Canonical release ranking (score ↓ → date ↓ → seeders ↓). On equal scores the
// winner must be deterministic — candidates arrive from a HashMap, so a score-only
// sort previously left the tie to iteration order.

const TIE_SERIES: &str = "tie-break-show";
const TIE_NEWER: &str = "magnet:?xt=urn:btih:3333333333333333333333333333333333333333";
const TIE_OLDER: &str = "magnet:?xt=urn:btih:4444444444444444444444444444444444444444";
const TIE_SEEDED: &str = "magnet:?xt=urn:btih:5555555555555555555555555555555555555555";
const TIE_UNSEEDED: &str = "magnet:?xt=urn:btih:6666666666666666666666666666666666666666";

/// A mock source entry scoring 100 (via the mapping's `1080p` term). Other scoring
/// weights are zero, so entries tie unless the title differs.
fn tie_entry(title: &str, link: &str, published: &str, seeders: u32) -> serde_json::Value {
    serde_json::json!({
        "title": title,
        "source": "Mock",
        "link": link,
        "download_url": link,
        "download_id": title,
        "size": 1_000_000_000u64,
        "seeders": seeders,
        "published": published,
    })
}

async fn tie_break_env(
    entries: Vec<serde_json::Value>,
) -> (
    ContentOrganizer,
    Arc<jumbie::db::DbManager>,
    tempfile::TempDir,
) {
    let (organizer, db, _dl, _mock_state, tmp) = setup_with_controllable_mock().await;
    db.upsert_series_mapping(
        TIE_SERIES,
        &MappingRule {
            target_title: "Test Show".to_string(),
            name: "test_show".to_string(),
            series_id: TIE_SERIES.to_string(),
            scoring: Some(jumbie_shared::scoring::ReleaseProfile {
                terms: HashMap::from([("1080p".to_string(), 100)]),
                ..Default::default()
            }),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    insert_test_episode(&db, &format!("{TIE_SERIES}_S01E01"), TIE_SERIES).await;
    add_mock_source(&organizer, entries).await;
    (organizer, db, tmp)
}

#[tokio::test]
async fn test_auto_search_tie_break_prefers_newer_release() {
    // Equal score and seeders; the older release is listed first to show that
    // arrival order does not decide the winner.
    let (organizer, db, _tmp) = tie_break_env(vec![
        tie_entry(
            "Test Show S01E01 1080p A",
            TIE_OLDER,
            "2024-01-01T00:00:00Z",
            100,
        ),
        tie_entry(
            "Test Show S01E01 1080p B",
            TIE_NEWER,
            "2024-06-01T00:00:00Z",
            100,
        ),
    ])
    .await;

    organizer
        .auto_search_missing(TIE_SERIES, "01", &[1])
        .await
        .unwrap();

    let queue = db.get_download_queue().await.unwrap();
    assert_eq!(queue.len(), 1);
    assert_eq!(
        queue[0].media_link, TIE_NEWER,
        "the newer release must win a score tie"
    );
}

#[tokio::test]
async fn test_auto_search_tie_break_uses_seeders_when_dates_equal() {
    let same_date = "2024-06-01T00:00:00Z";
    let (organizer, db, _tmp) = tie_break_env(vec![
        tie_entry("Test Show S01E01 1080p A", TIE_UNSEEDED, same_date, 3),
        tie_entry("Test Show S01E01 1080p B", TIE_SEEDED, same_date, 900),
    ])
    .await;

    organizer
        .auto_search_missing(TIE_SERIES, "01", &[1])
        .await
        .unwrap();

    let queue = db.get_download_queue().await.unwrap();
    assert_eq!(queue.len(), 1);
    assert_eq!(
        queue[0].media_link, TIE_SEEDED,
        "the most-seeded release must win when scores and dates tie"
    );
}

/// Seed a `Downloading` season-pack item (covered episodes via intentions) with
/// frozen progress and an off-disk content path.
async fn seed_pack_downloading_item(
    db: &jumbie::db::DbManager,
    mock_state: &Arc<Mutex<MockState>>,
    series_id: &str,
    hash: &str,
    episode_nums: &[i32],
) -> i64 {
    db.upsert_series_mapping(
        series_id,
        &MappingRule {
            target_title: "Pack Show".to_string(),
            name: "pack_show".to_string(),
            series_id: series_id.to_string(),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let mut episode_ids = Vec::new();
    let mut intentions = Vec::new();
    for &n in episode_nums {
        let ep_id = format!("{series_id}_S01E{n:02}");
        db.insert_episode(jumbie::db::episodes::InsertEpisodeParams::dummy(
            &ep_id,
            series_id,
            1,
            n,
            &HashMap::new(),
        ))
        .await
        .unwrap();
        intentions.push(EpisodeIntention {
            episode_num: n,
            source_episode_num: n,
            episode_id: ep_id.clone(),
            score: 100,
            keep: true,
        });
        episode_ids.push(ep_id);
    }
    let intentions_json = serde_json::to_string(&intentions).unwrap();
    let link = magnet_with_hash(hash);
    let result = db
        .add_to_download_queue(AddToDownloadQueueParams {
            media_name: "Pack Show S01",
            media_link: &link,
            series_title: "Pack Show",
            series_id,
            seasons: &[1],
            episodes: episode_nums,
            episode_id: Some(&episode_ids[0]),
            score: 100,
            is_user_requested: false,
            is_season_pack: true,
            category: "",
            multi_targets: None,
            episode_intentions: Some(&intentions_json),
            source_pub_date: None,
            download_id: hash,
            scoring_size_bytes: None,
            scoring_seeders: None,
            scoring_episode_count: None,
            submitter: None,
            quality_profile_id: None,
            version: 1,
        })
        .await
        .expect("add_to_download_queue");
    assert!(matches!(
        result,
        jumbie_shared::types::AddQueueResult::Added { .. }
    ));
    let id = db
        .get_queued_items()
        .await
        .unwrap()
        .first()
        .map(|i| i.id)
        .expect("queued item");
    db.update_queue_item_status(id, "Downloading", Some(hash), Some("mock_dl"), None)
        .await
        .unwrap();
    {
        // 0.25 matches `backdate_no_progress` so the sample reads as unchanged.
        let mut state = mock_state.lock().unwrap();
        state.content_paths.insert(
            hash.to_string(),
            format!("/tmp/jumbie-nonexistent/{hash}.mkv"),
        );
        state.progress.insert(hash.to_string(), 0.25);
    }
    id
}

#[tokio::test]
async fn test_season_pack_no_progress_autoresolves_covered_range() {
    let (organizer, db, _dl, mock_state, _tmp) = setup_with_controllable_mock().await;
    let series_id = "pack-stall-show";
    let hash = "pack_stall_hash_0001";
    let id = seed_pack_downloading_item(&db, &mock_state, series_id, hash, &[1, 2, 3]).await;
    backdate_no_progress(&db, id, 31).await;

    organizer.process_download_queue().await.unwrap();

    assert!(
        db.get_download_queue()
            .await
            .unwrap()
            .iter()
            .all(|i| i.id != id),
        "a stalled season pack must be removed for re-search"
    );
    // The pack's anchor episode consumes one attempt; the covered range is what the
    // re-search targets (scoping unit-tested in `no_progress_tests`).
    let target = RejectionTarget {
        series_id: Some(series_id),
        season: Some(1),
        episode: Some(1),
    };
    assert_eq!(
        db.get_autoresolve_attempts(target, 12, 2).await.unwrap(),
        1,
        "a pack stall consumes one attempt for its anchor episode"
    );
    let link = magnet_with_hash(hash);
    assert!(
        db.get_rejected_downloads()
            .await
            .unwrap()
            .contains(Some(link.as_str()), Some(hash)),
        "the stalled pack release must be rejected"
    );
}

// ── Episode-level manual search downloading a season pack ────────────────────
//
// These exercise the queue context an episode manual search produces: one target
// episode (`episodes: &[n]`), the pack flag, and the manual/user-requested flags.
// The release is a whole season (or range) pack, so smart-link must fill the
// other episodes as well while still honouring the single target.

struct ManualEpisodeDownload<'a> {
    series_id: &'a str,
    series_title: &'a str,
    media_name: &'a str,
    season: i32,
    episode: i32,
    episode_id: &'a str,
    is_season_pack: bool,
    download_hash: &'a str,
}

/// Queue the exact `enrich_and_enqueue` call `add_download` Path A makes for an
/// episode-level manual search: the episode's own season/episode context, the
/// pack flag from the search result, and `is_manual`/`is_user_requested` set.
async fn queue_manual_episode_download(
    db: &jumbie::db::DbManager,
    spec: ManualEpisodeDownload<'_>,
) {
    let seasons = [spec.season];
    let episodes = [spec.episode];
    ContentOrganizer::enrich_and_enqueue(EnrichAndEnqueueParams {
        db,
        notifications: None,
        media_name: spec.media_name,
        media_link: &magnet_with_hash(spec.download_hash),
        source: "",
        series_title: spec.series_title,
        series_id: spec.series_id,
        seasons: &seasons,
        episodes: &episodes,
        episode_id: Some(spec.episode_id),
        score: 100,
        is_user_requested: true,
        is_manual: true,
        is_season_pack: spec.is_season_pack,
        category: "Series",
        multi_targets: None,
        episode_intentions: None,
        quality_profile_id: None,
        meta_date: None,
        source_pub_date: None,
        metadata_ids: None,
        description: None,
        runtime: None,
        image_url: None,
        download_id: spec.download_hash,
        title_override: None,
        submitter: Some("MockFansub"),
        version: None,
        scoring_size_bytes: None,
        scoring_seeders: None,
        scoring_episode_count: None,
    })
    .await
    .expect("enrich_and_enqueue");

    let queue_id = db
        .get_queued_items()
        .await
        .unwrap()
        .first()
        .map(|i| i.id)
        .expect("queue item");
    db.update_queue_item_status(
        queue_id,
        "Downloading",
        Some(spec.download_hash),
        None,
        None,
    )
    .await
    .expect("update status");
}

/// Queue the exact shape `add_download` Path B (series-level search) produces: no
/// season/episode context, so smart-link resolves every file from disk.
async fn queue_manual_series_level_pack(
    db: &jumbie::db::DbManager,
    series_id: &str,
    series_title: &str,
    media_name: &str,
    download_hash: &str,
) {
    db.add_manual_download_to_queue(AddToDownloadQueueParams {
        media_name,
        media_link: &magnet_with_hash(download_hash),
        series_title,
        series_id,
        seasons: &[],
        episodes: &[],
        episode_id: None,
        score: 100,
        is_user_requested: true,
        is_season_pack: true,
        category: "Series",
        multi_targets: None,
        episode_intentions: None,
        source_pub_date: None,
        download_id: download_hash,
        scoring_size_bytes: None,
        scoring_seeders: None,
        scoring_episode_count: None,
        submitter: Some("MockFansub"),
        quality_profile_id: None,
        version: 1,
    })
    .await
    .expect("add_manual_download_to_queue");
    let queue_id = db
        .get_queued_items()
        .await
        .unwrap()
        .first()
        .map(|i| i.id)
        .expect("queue item");
    db.update_queue_item_status(queue_id, "Downloading", Some(download_hash), None, None)
        .await
        .expect("update status");
}

/// Insert an episode that already owns an on-disk file, as if it was organized
/// by an earlier download.
async fn insert_organized_episode(
    db: &jumbie::db::DbManager,
    episode_id: &str,
    series_id: &str,
    season: i32,
    episode: i32,
    file_path: &str,
) {
    db.insert_episode(jumbie::db::episodes::InsertEpisodeParams {
        episode_id,
        series_id,
        season,
        episode,
        file_path: Some(file_path),
        status: "organized",
        ..jumbie::db::episodes::InsertEpisodeParams::dummy(
            episode_id,
            series_id,
            season,
            episode,
            &HashMap::new(),
        )
    })
    .await
    .expect("insert organized episode");
}

async fn episode_file(db: &jumbie::db::DbManager, episode_id: &str) -> Option<String> {
    db.get_episode_by_id(episode_id)
        .await
        .expect("get episode")
        .and_then(|ep| ep.file_path)
}

/// Queue the exact `enrich_and_enqueue` call an *automatic* episode search makes:
/// the same single target intention as a manual search, but `is_manual = false`.
async fn queue_auto_episode_download(db: &jumbie::db::DbManager, spec: ManualEpisodeDownload<'_>) {
    let seasons = [spec.season];
    let episodes = [spec.episode];
    ContentOrganizer::enrich_and_enqueue(EnrichAndEnqueueParams {
        db,
        notifications: None,
        media_name: spec.media_name,
        media_link: &magnet_with_hash(spec.download_hash),
        source: "Nyaa",
        series_title: spec.series_title,
        series_id: spec.series_id,
        seasons: &seasons,
        episodes: &episodes,
        episode_id: Some(spec.episode_id),
        score: 100,
        is_user_requested: true,
        is_manual: false,
        is_season_pack: spec.is_season_pack,
        category: "Series",
        multi_targets: None,
        episode_intentions: None,
        quality_profile_id: None,
        meta_date: None,
        source_pub_date: None,
        metadata_ids: None,
        description: None,
        runtime: None,
        image_url: None,
        download_id: spec.download_hash,
        title_override: None,
        submitter: Some("MockFansub"),
        version: None,
        scoring_size_bytes: None,
        scoring_seeders: None,
        scoring_episode_count: None,
    })
    .await
    .expect("enrich_and_enqueue");

    let queue_id = db
        .get_queued_items()
        .await
        .unwrap()
        .first()
        .map(|i| i.id)
        .expect("queue item");
    db.update_queue_item_status(
        queue_id,
        "Downloading",
        Some(spec.download_hash),
        None,
        None,
    )
    .await
    .expect("update status");
}

/// An episode search is scoped to the episode the user picked: a pack's other
/// episodes are extras, not filled. This matches an episode auto-search.
#[tokio::test]
async fn test_manual_episode_search_pack_fills_only_the_searched_episode() {
    let (organizer, db, _dl, mock_state, tmp) = setup_with_controllable_mock().await;
    let series_id = "manual-pack-season";
    let library = tmp.path().join("library-manual-pack");
    let content_dir = tmp.path().join("Star.Voyage.S01.COMPLETE");
    let content_path_str = content_dir.to_string_lossy().to_string();
    let download_hash = "manual_pack_season_hash";

    save_delete_unexpected_config(&db).await;
    insert_offset_mapping(&db, series_id, &library, "1", 0).await;

    let target = format!("{}_S01E05", series_id);
    queue_manual_episode_download(
        &db,
        ManualEpisodeDownload {
            series_id,
            series_title: "Star Voyage",
            media_name: "Star Voyage S01 COMPLETE [1080p]",
            season: 1,
            episode: 5,
            episode_id: &target,
            is_season_pack: true,
            download_hash,
        },
    )
    .await;

    tokio::fs::create_dir_all(&content_dir).await.unwrap();
    for n in [1, 2, 5] {
        tokio::fs::write(
            content_dir.join(format!("Star.Voyage.S01E{n:02}.mkv")),
            b"episode",
        )
        .await
        .unwrap();
    }
    mark_mock_download_complete(&mock_state, download_hash, &content_path_str);

    organizer
        .process_downloads()
        .await
        .expect("process_downloads");

    // Only the searched episode is filled.
    let assigned = episode_file(&db, &target)
        .await
        .expect("the searched episode must be filled");
    assert!(std::path::Path::new(&assigned).exists());

    // The other pack episodes are extras: not filled, and (under the default
    // unexpected_files_handling=delete) removed from disk.
    for n in [1, 2] {
        let ep_id = format!("{}_S01E{n:02}", series_id);
        assert!(
            episode_file(&db, &ep_id).await.is_none(),
            "S01E{n:02} is a non-target pack episode and must not be filled"
        );
        assert!(
            !content_dir
                .join(format!("Star.Voyage.S01E{n:02}.mkv"))
                .exists(),
            "S01E{n:02} extra file must follow unexpected_files_handling=delete"
        );
    }
}

// A single multi-episode range file in the pack (S01E01-E03) has no way to be
// split, so it must be assigned to every episode it covers.
#[tokio::test]
async fn test_manual_episode_search_multirange_file_assigns_all_covered_episodes() {
    let (organizer, db, _dl, mock_state, tmp) = setup_with_controllable_mock().await;
    let series_id = "manual-pack-range";
    let library = tmp.path().join("library-manual-range");
    let content_dir = tmp.path().join("Star.Voyage.S01.RANGE");
    let content_path_str = content_dir.to_string_lossy().to_string();
    let download_hash = "manual_pack_range_hash";

    save_delete_unexpected_config(&db).await;
    insert_offset_mapping(&db, series_id, &library, "1", 0).await;

    let target = format!("{}_S01E02", series_id);
    queue_manual_episode_download(
        &db,
        ManualEpisodeDownload {
            series_id,
            series_title: "Star Voyage",
            media_name: "Star Voyage S01E01-E03 [1080p]",
            season: 1,
            episode: 2,
            episode_id: &target,
            is_season_pack: false,
            download_hash,
        },
    )
    .await;

    tokio::fs::create_dir_all(&content_dir).await.unwrap();
    tokio::fs::write(content_dir.join("Star.Voyage.S01E01-E03.mkv"), b"range")
        .await
        .unwrap();
    mark_mock_download_complete(&mock_state, download_hash, &content_path_str);

    organizer
        .process_downloads()
        .await
        .expect("process_downloads");

    let mut paths = Vec::new();
    for n in [1, 2, 3] {
        let ep_id = format!("{}_S01E{n:02}", series_id);
        let assigned = episode_file(&db, &ep_id)
            .await
            .unwrap_or_else(|| panic!("S01E{n:02} must be covered by the range file"));
        assert!(
            std::path::Path::new(&assigned).exists(),
            "S01E{n:02} file must exist: {assigned}"
        );
        paths.push(assigned);
    }
    assert_eq!(
        paths[0], paths[1],
        "a multi-episode file must be shared by every covered episode"
    );
    assert_eq!(paths[1], paths[2]);
}

// A pack file for an episode that isn't the searched one is an extra. The episode's
// existing library file is untouched, and the extra incoming file follows the
// automatic policy (delete by default) rather than being kept for review.
#[tokio::test]
async fn test_manual_episode_search_pack_conflict_keeps_existing_drops_extra() {
    let (organizer, db, _dl, mock_state, tmp) = setup_with_controllable_mock().await;
    let series_id = "manual-pack-conflict";
    let library = tmp.path().join("library-manual-conflict");
    let content_dir = tmp.path().join("Star.Voyage.S01.CONFLICT");
    let content_path_str = content_dir.to_string_lossy().to_string();
    let download_hash = "manual_pack_conflict_hash";

    save_delete_unexpected_config(&db).await;
    insert_offset_mapping(&db, series_id, &library, "1", 0).await;

    tokio::fs::create_dir_all(&library).await.unwrap();
    let existing = library.join("Star.Voyage.S01E03.mkv");
    tokio::fs::write(&existing, b"existing library file")
        .await
        .unwrap();
    let existing_str = existing.to_string_lossy().to_string();
    let ep3 = format!("{}_S01E03", series_id);
    insert_organized_episode(&db, &ep3, series_id, 1, 3, &existing_str).await;

    let target = format!("{}_S01E02", series_id);
    queue_manual_episode_download(
        &db,
        ManualEpisodeDownload {
            series_id,
            series_title: "Star Voyage",
            media_name: "Star Voyage S01 COMPLETE [1080p]",
            season: 1,
            episode: 2,
            episode_id: &target,
            is_season_pack: true,
            download_hash,
        },
    )
    .await;

    tokio::fs::create_dir_all(&content_dir).await.unwrap();
    for n in [2, 3] {
        tokio::fs::write(
            content_dir.join(format!("Star.Voyage.S01E{n:02}.mkv")),
            b"incoming",
        )
        .await
        .unwrap();
    }
    let incoming_conflict = content_dir.join("Star.Voyage.S01E03.mkv");
    mark_mock_download_complete(&mock_state, download_hash, &content_path_str);

    organizer
        .process_downloads()
        .await
        .expect("process_downloads");

    // Target episode E02 is filled by the pack.
    let ep2 = format!("{}_S01E02", series_id);
    assert!(
        episode_file(&db, &ep2).await.is_some(),
        "the target episode must still be filled"
    );

    // The existing E03 library file is untouched and still owns the episode.
    assert!(
        existing.exists(),
        "the pre-existing library file must remain"
    );
    assert_eq!(
        episode_file(&db, &ep3).await.as_deref(),
        Some(existing_str.as_str()),
        "E03 must keep its existing file"
    );

    // The extra incoming file is not needed (no matching target) and follows the
    // automatic policy: unexpected_files_handling=delete removes it, and nothing is
    // recorded for review.
    assert!(
        !incoming_conflict.exists(),
        "a non-target pack file follows unexpected_files_handling=delete"
    );
    let reviewed: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM unmatched_files WHERE series_id = ?")
            .bind(series_id)
            .fetch_one(db.get_pool())
            .await
            .unwrap();
    assert_eq!(reviewed, 0, "nothing should be held for review");
}

// A manually chosen pack that targets an episode already on disk must upgrade it
// regardless of score: the incoming file replaces the existing one in place.
#[tokio::test]
async fn test_manual_episode_search_pack_replaces_current_episode() {
    let (organizer, db, _dl, mock_state, tmp) = setup_with_controllable_mock().await;
    let series_id = "manual-pack-replace";
    let library = tmp.path().join("library-manual-replace");
    let content_dir = tmp.path().join("Star.Voyage.S01.REPLACE");
    let content_path_str = content_dir.to_string_lossy().to_string();
    let download_hash = "manual_pack_replace_hash";

    save_delete_unexpected_config(&db).await;
    insert_offset_mapping(&db, series_id, &library, "1", 0).await;

    tokio::fs::create_dir_all(&library).await.unwrap();
    let existing = library.join("Star.Voyage.S01E05.mkv");
    tokio::fs::write(&existing, b"old library content")
        .await
        .unwrap();
    let existing_str = existing.to_string_lossy().to_string();
    let target = format!("{}_S01E05", series_id);
    insert_organized_episode(&db, &target, series_id, 1, 5, &existing_str).await;

    queue_manual_episode_download(
        &db,
        ManualEpisodeDownload {
            series_id,
            series_title: "Star Voyage",
            media_name: "Star Voyage S01E05 [1080p]",
            season: 1,
            episode: 5,
            episode_id: &target,
            is_season_pack: false,
            download_hash,
        },
    )
    .await;

    tokio::fs::create_dir_all(&content_dir).await.unwrap();
    tokio::fs::write(
        content_dir.join("Star.Voyage.S01E05.mkv"),
        b"new download content",
    )
    .await
    .unwrap();
    mark_mock_download_complete(&mock_state, download_hash, &content_path_str);

    organizer
        .process_downloads()
        .await
        .expect("process_downloads");

    let assigned = episode_file(&db, &target)
        .await
        .expect("E05 must have a file");
    assert!(
        std::path::Path::new(&assigned).exists(),
        "the replacement file must exist: {assigned}"
    );
    let content = tokio::fs::read(&assigned)
        .await
        .expect("read replaced file");
    assert_eq!(
        content, b"new download content",
        "a manually chosen release must replace the existing episode file"
    );
}

// Two pack files from a *different* season than the searched episode: the file
// whose number matches the target is mapped directly onto the target episode
// (regardless of its season), while the other is an extra.
#[tokio::test]
async fn test_manual_episode_search_pack_other_season_maps_target_by_number() {
    let (organizer, db, _dl, mock_state, tmp) = setup_with_controllable_mock().await;
    let series_id = "manual-pack-other-season";
    let library = tmp.path().join("library-manual-other-season");
    let content_dir = tmp.path().join("Star.Voyage.S02.PACK");
    let content_path_str = content_dir.to_string_lossy().to_string();
    let download_hash = "manual_pack_other_season_hash";

    save_delete_unexpected_config(&db).await;
    insert_offset_mapping(&db, series_id, &library, "1", 0).await;

    let target = format!("{}_S01E05", series_id);
    queue_manual_episode_download(
        &db,
        ManualEpisodeDownload {
            series_id,
            series_title: "Star Voyage",
            media_name: "Star Voyage S02 COMPLETE [1080p]",
            season: 1,
            episode: 5,
            episode_id: &target,
            is_season_pack: true,
            download_hash,
        },
    )
    .await;

    tokio::fs::create_dir_all(&content_dir).await.unwrap();
    for (season, episode) in [(2, 1), (2, 5)] {
        tokio::fs::write(
            content_dir.join(format!("Star.Voyage.S{season:02}E{episode:02}.mkv")),
            b"episode",
        )
        .await
        .unwrap();
    }
    mark_mock_download_complete(&mock_state, download_hash, &content_path_str);

    organizer
        .process_downloads()
        .await
        .expect("process_downloads");

    // S02E05 maps by number onto the searched S01E05.
    assert!(
        episode_file(&db, &target).await.is_some(),
        "the searched episode must be filled by the number-matching file"
    );
    // S02E01 does not match the target and is an extra: not created, deleted.
    let s02e01 = format!("{}_S02E01", series_id);
    assert!(
        episode_file(&db, &s02e01).await.is_none(),
        "a non-matching pack file is an extra and must not be linked"
    );
    assert!(!content_dir.join("Star.Voyage.S02E01.mkv").exists());
}

// Series-level search (no episode context) does NOT force files onto a target:
// every file is resolved by its own parsed season/episode.
#[tokio::test]
async fn test_series_level_search_pack_resolves_each_file_by_its_own_season() {
    let (organizer, db, _dl, mock_state, tmp) = setup_with_controllable_mock().await;
    let series_id = "series-level-pack";
    let library = tmp.path().join("library-series-level");
    let content_dir = tmp.path().join("Star.Voyage.S02.PACK");
    let content_path_str = content_dir.to_string_lossy().to_string();
    let download_hash = "series_level_pack_hash";

    save_delete_unexpected_config(&db).await;
    insert_offset_mapping(&db, series_id, &library, "1", 0).await;

    queue_manual_series_level_pack(
        &db,
        series_id,
        "Star Voyage",
        "Star Voyage S02 COMPLETE [1080p]",
        download_hash,
    )
    .await;

    tokio::fs::create_dir_all(&content_dir).await.unwrap();
    tokio::fs::write(content_dir.join("Star.Voyage.S02E05.mkv"), b"episode")
        .await
        .unwrap();
    mark_mock_download_complete(&mock_state, download_hash, &content_path_str);

    organizer
        .process_downloads()
        .await
        .expect("process_downloads");

    let s02e05 = format!("{}_S02E05", series_id);
    assert!(
        episode_file(&db, &s02e05).await.is_some(),
        "a series-level pack keeps the file in its own season"
    );
    let s01e05 = format!("{}_S01E05", series_id);
    assert!(
        episode_file(&db, &s01e05).await.is_none(),
        "series-level search must not force a file onto S01E05"
    );
}

// Seasonless release numbers in an episode-level pack: the file whose number
// matches the searched episode is mapped onto it; the others are extras.
#[tokio::test]
async fn test_manual_episode_search_seasonless_number_matching_target() {
    let (organizer, db, _dl, mock_state, tmp) = setup_with_controllable_mock().await;
    let series_id = "manual-pack-seasonless";
    let library = tmp.path().join("library-manual-seasonless");
    let content_dir = tmp.path().join("Star.Voyage.BARE");
    let content_path_str = content_dir.to_string_lossy().to_string();
    let download_hash = "manual_pack_seasonless_hash";

    save_delete_unexpected_config(&db).await;
    insert_offset_mapping(&db, series_id, &library, "1", 0).await;

    let target = format!("{}_S01E05", series_id);
    queue_manual_episode_download(
        &db,
        ManualEpisodeDownload {
            series_id,
            series_title: "Star Voyage",
            media_name: "Star Voyage S01 [1080p]",
            season: 1,
            episode: 5,
            episode_id: &target,
            is_season_pack: true,
            download_hash,
        },
    )
    .await;

    tokio::fs::create_dir_all(&content_dir).await.unwrap();
    for n in [3, 5] {
        tokio::fs::write(
            content_dir.join(format!("Star Voyage - {n:02}.mkv")),
            b"episode",
        )
        .await
        .unwrap();
    }
    mark_mock_download_complete(&mock_state, download_hash, &content_path_str);

    organizer
        .process_downloads()
        .await
        .expect("process_downloads");

    assert!(
        episode_file(&db, &target).await.is_some(),
        "the target episode must be filled by the number-matching file"
    );
    // The -03 file is an extra (no matching target) and follows the automatic policy.
    let other = format!("{}_S01E03", series_id);
    assert!(
        episode_file(&db, &other).await.is_none(),
        "a seasonless non-target file is an extra and must not be linked"
    );
    assert!(!content_dir.join("Star Voyage - 03.mkv").exists());
}

// A single unparseable video file in an episode-level download cannot be routed
// by name, so it is linked directly to the searched episode.
#[tokio::test]
async fn test_manual_episode_search_single_unparseable_file_maps_to_target() {
    let (organizer, db, _dl, mock_state, tmp) = setup_with_controllable_mock().await;
    let series_id = "manual-single-unparseable";
    let library = tmp.path().join("library-manual-unparseable");
    let content_dir = tmp.path().join("Star.Voyage.UNPARSEABLE");
    let content_path_str = content_dir.to_string_lossy().to_string();
    let download_hash = "manual_single_unparseable_hash";

    save_delete_unexpected_config(&db).await;
    insert_offset_mapping(&db, series_id, &library, "1", 0).await;

    let target = format!("{}_S01E05", series_id);
    queue_manual_episode_download(
        &db,
        ManualEpisodeDownload {
            series_id,
            series_title: "Star Voyage",
            media_name: "Star Voyage release [1080p]",
            season: 1,
            episode: 5,
            episode_id: &target,
            is_season_pack: false,
            download_hash,
        },
    )
    .await;

    tokio::fs::create_dir_all(&content_dir).await.unwrap();
    tokio::fs::write(content_dir.join("release.mkv"), b"unparseable")
        .await
        .unwrap();
    mark_mock_download_complete(&mock_state, download_hash, &content_path_str);

    organizer
        .process_downloads()
        .await
        .expect("process_downloads");

    let assigned = episode_file(&db, &target)
        .await
        .expect("the searched episode must receive the unparseable file");
    assert!(
        std::path::Path::new(&assigned).exists(),
        "assigned file must exist: {assigned}"
    );
}

// A multi-range pack through an episode search: only the range that covers the
// searched episode is placed (as a unit). A range that does not cover it is an
// extra and follows the automatic policy.
#[tokio::test]
async fn test_manual_episode_search_multirange_pack_covers_target_range() {
    let (organizer, db, _dl, mock_state, tmp) = setup_with_controllable_mock().await;
    let series_id = "manual-multirange-pack";
    let library = tmp.path().join("library-manual-multirange");
    let content_dir = tmp.path().join("Star.Voyage.S01.RANGES");
    let content_path_str = content_dir.to_string_lossy().to_string();
    let download_hash = "manual_multirange_pack_hash";

    save_delete_unexpected_config(&db).await;
    insert_offset_mapping(&db, series_id, &library, "1", 0).await;

    let target = format!("{}_S01E02", series_id);
    queue_manual_episode_download(
        &db,
        ManualEpisodeDownload {
            series_id,
            series_title: "Star Voyage",
            media_name: "Star Voyage S01E01-E04 [1080p]",
            season: 1,
            episode: 2,
            episode_id: &target,
            is_season_pack: true,
            download_hash,
        },
    )
    .await;

    tokio::fs::create_dir_all(&content_dir).await.unwrap();
    for name in ["Star.Voyage.S01E01-E02.mkv", "Star.Voyage.S01E03-E04.mkv"] {
        tokio::fs::write(content_dir.join(name), b"range")
            .await
            .unwrap();
    }
    mark_mock_download_complete(&mock_state, download_hash, &content_path_str);

    organizer
        .process_downloads()
        .await
        .expect("process_downloads");

    let ep = |n: i32| format!("{}_S01E{n:02}", series_id);

    // The range covering the target is placed as a unit: E01 and E02 share it.
    let e01 = episode_file(&db, &ep(1))
        .await
        .expect("E01 is covered by the target's range");
    let e02 = episode_file(&db, &ep(2))
        .await
        .expect("E02 is the target episode");
    assert_eq!(e01, e02, "the covering range is placed as one file");
    assert!(std::path::Path::new(&e01).exists());

    // The range not covering the target is an extra: not filled, deleted.
    assert!(episode_file(&db, &ep(3)).await.is_none());
    assert!(episode_file(&db, &ep(4)).await.is_none());
    assert!(
        !content_dir.join("Star.Voyage.S01E03-E04.mkv").exists(),
        "the non-target range follows unexpected_files_handling=delete"
    );
}

// A different-season extra whose episode already exists: the episode's library file
// keeps ownership, and the extra incoming file follows the automatic policy.
#[tokio::test]
async fn test_manual_episode_search_pack_other_season_conflict_keeps_existing() {
    let (organizer, db, _dl, mock_state, tmp) = setup_with_controllable_mock().await;
    let series_id = "manual-other-season-conflict";
    let library = tmp.path().join("library-manual-other-season-conflict");
    let content_dir = tmp.path().join("Star.Voyage.S02.CONFLICT");
    let content_path_str = content_dir.to_string_lossy().to_string();
    let download_hash = "manual_other_season_conflict_hash";

    save_delete_unexpected_config(&db).await;
    insert_offset_mapping(&db, series_id, &library, "1", 0).await;

    tokio::fs::create_dir_all(&library).await.unwrap();
    let existing = library.join("Star.Voyage.S02E03.mkv");
    tokio::fs::write(&existing, b"existing s02 file")
        .await
        .unwrap();
    let existing_str = existing.to_string_lossy().to_string();
    let s02e03 = format!("{}_S02E03", series_id);
    insert_organized_episode(&db, &s02e03, series_id, 2, 3, &existing_str).await;

    let target = format!("{}_S01E05", series_id);
    queue_manual_episode_download(
        &db,
        ManualEpisodeDownload {
            series_id,
            series_title: "Star Voyage",
            media_name: "Star Voyage S02 COMPLETE [1080p]",
            season: 1,
            episode: 5,
            episode_id: &target,
            is_season_pack: true,
            download_hash,
        },
    )
    .await;

    tokio::fs::create_dir_all(&content_dir).await.unwrap();
    // Two files so the single-target fast path (which would map any lone file onto
    // the searched episode) does not fire.
    let incoming = content_dir.join("Star.Voyage.S02E03.mkv");
    tokio::fs::write(&incoming, b"incoming s02 file")
        .await
        .unwrap();
    tokio::fs::write(content_dir.join("Star.Voyage.S02E05.mkv"), b"target")
        .await
        .unwrap();
    mark_mock_download_complete(&mock_state, download_hash, &content_path_str);

    organizer
        .process_downloads()
        .await
        .expect("process_downloads");

    assert!(existing.exists(), "the pre-existing S02 file must remain");
    assert_eq!(
        episode_file(&db, &s02e03).await.as_deref(),
        Some(existing_str.as_str()),
        "S02E03 must keep its library file"
    );
    // The S02E03 incoming file is a non-target extra and follows the automatic
    // policy: deleted, nothing held for review.
    assert!(
        !incoming.exists(),
        "a non-target different-season file follows unexpected_files_handling=delete"
    );
    let reviewed: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM unmatched_files WHERE series_id = ?")
            .bind(series_id)
            .fetch_one(db.get_pool())
            .await
            .unwrap();
    assert_eq!(reviewed, 0);
}

// Multiple range files, none covering the searched episode: they are extras. No
// episode is filled; because nothing matched, the download is an unplaced
// assignment and both files are kept for review (matching an auto-search).
#[tokio::test]
async fn test_manual_episode_search_multiple_ranges_partial_conflicts_drop_extras() {
    let (organizer, db, _dl, mock_state, tmp) = setup_with_controllable_mock().await;
    let series_id = "manual-multi-range-conflict";
    let library = tmp.path().join("library-manual-multi-range-conflict");
    let content_dir = tmp.path().join("Star.Voyage.S01.CONFLICTS");
    let content_path_str = content_dir.to_string_lossy().to_string();
    let download_hash = "manual_multi_range_conflict_hash";

    save_delete_unexpected_config(&db).await;
    insert_offset_mapping(&db, series_id, &library, "1", 0).await;

    // E02 and E08 already own library files.
    tokio::fs::create_dir_all(&library).await.unwrap();
    let mut existing_paths = Vec::new();
    for n in [2, 8] {
        let p = library.join(format!("Star.Voyage.S01E{n:02}.mkv"));
        tokio::fs::write(&p, b"existing").await.unwrap();
        let path_str = p.to_string_lossy().to_string();
        insert_organized_episode(
            &db,
            &format!("{}_S01E{n:02}", series_id),
            series_id,
            1,
            n,
            &path_str,
        )
        .await;
        existing_paths.push((n, path_str));
    }

    let target = format!("{}_S01E05", series_id);
    queue_manual_episode_download(
        &db,
        ManualEpisodeDownload {
            series_id,
            series_title: "Star Voyage",
            media_name: "Star Voyage S01E01-E09 [1080p]",
            season: 1,
            episode: 5,
            episode_id: &target,
            is_season_pack: true,
            download_hash,
        },
    )
    .await;

    tokio::fs::create_dir_all(&content_dir).await.unwrap();
    tokio::fs::write(content_dir.join("Star.Voyage.S01E01-E03.mkv"), b"range1")
        .await
        .unwrap();
    tokio::fs::write(content_dir.join("Star.Voyage.S01E07-E09.mkv"), b"range2")
        .await
        .unwrap();
    mark_mock_download_complete(&mock_state, download_hash, &content_path_str);

    organizer
        .process_downloads()
        .await
        .expect("process_downloads");

    let ep = |n: i32| format!("{}_S01E{n:02}", series_id);

    // The pre-existing episodes keep their own files.
    for (n, path) in &existing_paths {
        assert!(
            std::path::Path::new(path).exists(),
            "existing E{n:02} file must remain"
        );
        assert_eq!(
            episode_file(&db, &ep(*n)).await.as_deref(),
            Some(path.as_str()),
            "E{n:02} must keep its existing file"
        );
    }

    // Neither range covers the searched episode, so both are extras: no new episode
    // is filled.
    for n in [1, 3, 7, 9] {
        assert!(
            episode_file(&db, &ep(n)).await.is_none(),
            "E{n:02} is in a non-target range and must not be filled"
        );
    }

    // Nothing matched the searched episode, so the download is an unplaced
    // assignment: both range files are kept for review (matching an auto-search).
    assert!(content_dir.join("Star.Voyage.S01E01-E03.mkv").exists());
    assert!(content_dir.join("Star.Voyage.S01E07-E09.mkv").exists());

    // Episodes outside every range stay untouched.
    for n in [4, 5, 6] {
        assert!(
            episode_file(&db, &ep(n)).await.is_none(),
            "E{n:02} is outside both ranges and must stay unassigned"
        );
    }

    let reviewed: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM unmatched_files WHERE series_id = ?")
            .bind(series_id)
            .fetch_one(db.get_pool())
            .await
            .expect("unmatched count");
    assert_eq!(reviewed, 2, "both unplaced range files are held for review");
}

// A different-season *single* file in an episode-level download is mapped directly
// onto the searched episode by the single-target fast path (one file cannot be
// ambiguous), so no episode of the file's own season is created.
#[tokio::test]
async fn test_manual_episode_search_single_file_other_season_maps_to_target() {
    let (organizer, db, _dl, mock_state, tmp) = setup_with_controllable_mock().await;
    let series_id = "manual-single-other-season";
    let library = tmp.path().join("library-manual-single-other-season");
    let content_dir = tmp.path().join("Star.Voyage.S02.SINGLE");
    let content_path_str = content_dir.to_string_lossy().to_string();
    let download_hash = "manual_single_other_season_hash";

    save_delete_unexpected_config(&db).await;
    insert_offset_mapping(&db, series_id, &library, "1", 0).await;

    let target = format!("{}_S01E05", series_id);
    queue_manual_episode_download(
        &db,
        ManualEpisodeDownload {
            series_id,
            series_title: "Star Voyage",
            media_name: "Star Voyage S02E03 [1080p]",
            season: 1,
            episode: 5,
            episode_id: &target,
            is_season_pack: false,
            download_hash,
        },
    )
    .await;

    tokio::fs::create_dir_all(&content_dir).await.unwrap();
    tokio::fs::write(content_dir.join("Star.Voyage.S02E03.mkv"), b"single")
        .await
        .unwrap();
    mark_mock_download_complete(&mock_state, download_hash, &content_path_str);

    organizer
        .process_downloads()
        .await
        .expect("process_downloads");

    let assigned = episode_file(&db, &target)
        .await
        .expect("the lone file must map directly onto the searched episode");
    assert!(std::path::Path::new(&assigned).exists());
    assert!(
        episode_file(&db, &format!("{}_S02E03", series_id))
            .await
            .is_none(),
        "the file must not create an episode in its own season"
    );
}

// A single indivisible range that *covers* the searched episode is placed as a
// unit: every covered episode receives the range file and an overlapping existing
// file is replaced. Contrast with individual per-episode files, where only the
// searched episode is replaced (see `..._individual_files_replace_only_target`).
#[tokio::test]
async fn test_manual_episode_search_single_range_covering_target_replaces_whole_range() {
    let (organizer, db, _dl, mock_state, tmp) = setup_with_controllable_mock().await;
    let series_id = "manual-target-range-authority";
    let library = tmp.path().join("library-manual-target-range");
    let content_dir = tmp.path().join("Star.Voyage.S01.TARGETRANGE");
    let content_path_str = content_dir.to_string_lossy().to_string();
    let download_hash = "manual_target_range_authority_hash";

    save_delete_unexpected_config(&db).await;
    insert_offset_mapping(&db, series_id, &library, "1", 0).await;

    tokio::fs::create_dir_all(&library).await.unwrap();

    // E01 (non-target) and E02 (target) already own library files.
    let e01_old = library.join("Star.Voyage.S01E01.mkv");
    tokio::fs::write(&e01_old, b"e01 existing").await.unwrap();
    let e01_old_str = e01_old.to_string_lossy().to_string();
    insert_organized_episode(
        &db,
        &format!("{}_S01E01", series_id),
        series_id,
        1,
        1,
        &e01_old_str,
    )
    .await;

    let e02_old = library.join("Star.Voyage.S01E02.mkv");
    tokio::fs::write(&e02_old, b"e02 old content")
        .await
        .unwrap();
    let e02_old_str = e02_old.to_string_lossy().to_string();
    let target = format!("{}_S01E02", series_id);
    insert_organized_episode(&db, &target, series_id, 1, 2, &e02_old_str).await;

    queue_manual_episode_download(
        &db,
        ManualEpisodeDownload {
            series_id,
            series_title: "Star Voyage",
            media_name: "Star Voyage S01E01-E03 [1080p]",
            season: 1,
            episode: 2,
            episode_id: &target,
            is_season_pack: false,
            download_hash,
        },
    )
    .await;

    tokio::fs::create_dir_all(&content_dir).await.unwrap();
    tokio::fs::write(
        content_dir.join("Star.Voyage.S01E01-E03.mkv"),
        b"target range content",
    )
    .await
    .unwrap();
    mark_mock_download_complete(&mock_state, download_hash, &content_path_str);

    organizer
        .process_downloads()
        .await
        .expect("process_downloads");

    let ep = |n: i32| format!("{}_S01E{n:02}", series_id);
    let e01 = episode_file(&db, &ep(1))
        .await
        .expect("E01 must be replaced by the range");
    let e02 = episode_file(&db, &target)
        .await
        .expect("E02 must be replaced by the range");
    let e03 = episode_file(&db, &ep(3))
        .await
        .expect("E03 must be filled by the range");
    assert_eq!(e01, e02, "the whole range is placed as one file");
    assert_eq!(e02, e03);
    assert_eq!(
        tokio::fs::read(&e02).await.unwrap(),
        b"target range content",
        "the covering range replaces the target episode's old file"
    );
}

// A single range that does NOT cover the searched episode is an extra: it is not
// filled and follows the automatic policy, while an existing library file stays.
#[tokio::test]
async fn test_manual_episode_search_range_not_covering_target_is_extra() {
    let (organizer, db, _dl, mock_state, tmp) = setup_with_controllable_mock().await;
    let series_id = "manual-range-outside-target";
    let library = tmp.path().join("library-manual-range-outside");
    let content_dir = tmp.path().join("Star.Voyage.S01.OUTSIDE");
    let content_path_str = content_dir.to_string_lossy().to_string();
    let download_hash = "manual_range_outside_target_hash";

    save_delete_unexpected_config(&db).await;
    insert_offset_mapping(&db, series_id, &library, "1", 0).await;

    // E01 already owns a file; the searched episode is E05 (outside the range).
    tokio::fs::create_dir_all(&library).await.unwrap();
    let e01_old = library.join("Star.Voyage.S01E01.mkv");
    tokio::fs::write(&e01_old, b"e01 existing").await.unwrap();
    let e01_old_str = e01_old.to_string_lossy().to_string();
    insert_organized_episode(
        &db,
        &format!("{}_S01E01", series_id),
        series_id,
        1,
        1,
        &e01_old_str,
    )
    .await;

    let target = format!("{}_S01E05", series_id);
    queue_manual_episode_download(
        &db,
        ManualEpisodeDownload {
            series_id,
            series_title: "Star Voyage",
            media_name: "Star Voyage S01E01-E03 [1080p]",
            season: 1,
            episode: 5,
            episode_id: &target,
            is_season_pack: false,
            download_hash,
        },
    )
    .await;

    tokio::fs::create_dir_all(&content_dir).await.unwrap();
    tokio::fs::write(content_dir.join("Star.Voyage.S01E01-E03.mkv"), b"range")
        .await
        .unwrap();
    mark_mock_download_complete(&mock_state, download_hash, &content_path_str);

    organizer
        .process_downloads()
        .await
        .expect("process_downloads");

    let ep = |n: i32| format!("{}_S01E{n:02}", series_id);

    // E01 is occupied by a non-target episode: it stays.
    assert!(e01_old.exists());
    assert_eq!(
        episode_file(&db, &ep(1)).await.as_deref(),
        Some(e01_old_str.as_str()),
        "an occupied non-target episode must keep its file"
    );

    // The covered non-target episodes are NOT filled: the range is an extra.
    assert!(
        episode_file(&db, &ep(2)).await.is_none(),
        "a non-target range must not fill its episodes"
    );
    assert!(episode_file(&db, &ep(3)).await.is_none());

    // Nothing matched the searched episode, so the download is an unplaced
    // assignment: the extra file is kept for review (matching an auto-search).
    assert!(
        content_dir.join("Star.Voyage.S01E01-E03.mkv").exists(),
        "an unmatched download keeps its files for review"
    );
    let reviewed: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM unmatched_files WHERE series_id = ?")
            .bind(series_id)
            .fetch_one(db.get_pool())
            .await
            .unwrap();
    assert_eq!(reviewed, 1);

    // The searched episode (outside the range) is untouched.
    assert!(
        episode_file(&db, &target).await.is_none(),
        "the searched episode is not covered by the range and must stay empty"
    );
}

// Individual per-episode files replace only the searched episode; other occupied
// episodes keep their files.
#[tokio::test]
async fn test_manual_episode_search_individual_files_replace_only_target() {
    let (organizer, db, _dl, mock_state, tmp) = setup_with_controllable_mock().await;
    let series_id = "manual-individual-target-only";
    let library = tmp.path().join("library-manual-individual");
    let content_dir = tmp.path().join("Star.Voyage.S01.INDIVIDUAL");
    let content_path_str = content_dir.to_string_lossy().to_string();
    let download_hash = "manual_individual_target_only_hash";

    save_delete_unexpected_config(&db).await;
    insert_offset_mapping(&db, series_id, &library, "1", 0).await;

    tokio::fs::create_dir_all(&library).await.unwrap();
    let mut old = Vec::new();
    for n in [1, 2, 3] {
        let p = library.join(format!("Star.Voyage.S01E{n:02}.mkv"));
        tokio::fs::write(&p, format!("e{n:02} old")).await.unwrap();
        let path_str = p.to_string_lossy().to_string();
        insert_organized_episode(
            &db,
            &format!("{}_S01E{n:02}", series_id),
            series_id,
            1,
            n,
            &path_str,
        )
        .await;
        old.push((n, path_str));
    }

    let target = format!("{}_S01E02", series_id);
    queue_manual_episode_download(
        &db,
        ManualEpisodeDownload {
            series_id,
            series_title: "Star Voyage",
            media_name: "Star Voyage S01E01-E03 [1080p]",
            season: 1,
            episode: 2,
            episode_id: &target,
            is_season_pack: true,
            download_hash,
        },
    )
    .await;

    tokio::fs::create_dir_all(&content_dir).await.unwrap();
    for n in [1, 2, 3] {
        tokio::fs::write(
            content_dir.join(format!("Star.Voyage.S01E{n:02}.mkv")),
            format!("e{n:02} new"),
        )
        .await
        .unwrap();
    }
    mark_mock_download_complete(&mock_state, download_hash, &content_path_str);

    organizer
        .process_downloads()
        .await
        .expect("process_downloads");

    let ep = |n: i32| format!("{}_S01E{n:02}", series_id);

    // The searched episode is replaced.
    let e02 = episode_file(&db, &target)
        .await
        .expect("E02 must have a file");
    assert_eq!(
        tokio::fs::read(&e02).await.unwrap(),
        b"e02 new",
        "the searched episode is replaced by the manually chosen file"
    );

    // The other occupied episodes keep their files.
    for (n, path) in &old {
        if *n == 2 {
            continue;
        }
        assert!(std::path::Path::new(path).exists());
        assert_eq!(
            episode_file(&db, &ep(*n)).await.as_deref(),
            Some(path.as_str()),
            "E{n:02} is not the searched episode and must keep its file"
        );
    }
}

// Characterization: how an *automatic* episode search handles the extra episodes of
// a pack it selected. It has the same single-target intention shape as a manual
// search, but `is_manual = false`.
//
// KNOWN DIFFERENCE (flagged): the non-target files are routed as *unexpected* (not
// permissively filled like a manual search, and not `unneeded` like a season-pack
// auto-search), so the default `unexpected_files_handling = delete` removes them and
// records nothing for review. A manual search keeps them for review instead.
#[tokio::test]
async fn test_auto_episode_search_pack_extras_follow_unexpected_handling() {
    let (organizer, db, _dl, mock_state, tmp) = setup_with_controllable_mock().await;
    let series_id = "auto-pack-extras";
    let library = tmp.path().join("library-auto-pack-extras");
    let content_dir = tmp.path().join("Star.Voyage.S01.AUTOPACK");
    let content_path_str = content_dir.to_string_lossy().to_string();
    let download_hash = "auto_pack_extras_hash";

    // unexpected_files_handling = delete (the default)
    save_delete_unexpected_config(&db).await;
    insert_offset_mapping(&db, series_id, &library, "1", 0).await;

    let target = format!("{}_S01E05", series_id);
    queue_auto_episode_download(
        &db,
        ManualEpisodeDownload {
            series_id,
            series_title: "Star Voyage",
            media_name: "Star Voyage S01 COMPLETE [1080p]",
            season: 1,
            episode: 5,
            episode_id: &target,
            is_season_pack: true,
            download_hash,
        },
    )
    .await;

    tokio::fs::create_dir_all(&content_dir).await.unwrap();
    for n in [1, 2, 5] {
        tokio::fs::write(
            content_dir.join(format!("Star.Voyage.S01E{n:02}.mkv")),
            b"episode",
        )
        .await
        .unwrap();
    }
    mark_mock_download_complete(&mock_state, download_hash, &content_path_str);

    organizer
        .process_downloads()
        .await
        .expect("process_downloads");

    let ep = |n: i32| format!("{}_S01E{n:02}", series_id);

    // The searched episode is filled.
    assert!(episode_file(&db, &target).await.is_some());

    // The extra pack files are neither assigned nor filled from the pack.
    assert!(episode_file(&db, &ep(1)).await.is_none());
    assert!(episode_file(&db, &ep(2)).await.is_none());

    // Under the default delete policy they are removed from disk, and nothing is
    // recorded for review (contrast with the manual search, which keeps them).
    assert!(
        !content_dir.join("Star.Voyage.S01E01.mkv").exists()
            && !content_dir.join("Star.Voyage.S01E02.mkv").exists(),
        "auto non-target pack files follow unexpected_files_handling=delete"
    );
    let reviewed: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM unmatched_files WHERE series_id = ?")
            .bind(series_id)
            .fetch_one(db.get_pool())
            .await
            .unwrap();
    assert_eq!(reviewed, 0, "auto extras are not held for review");
}
