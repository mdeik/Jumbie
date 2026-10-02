mod common;

use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use jumbie::api::AppState;
use jumbie::organizer::ContentOrganizer;
use jumbie::plugins::PluginInstance;
use jumbie::plugins::notifiers::{NotifierContext, NotifierEvent};
use jumbie_shared::plugin::Capability;
use tokio::sync::Mutex;

/// Recorded RPC invocation: (method name, optional JSON params).
type RecordedCall = (String, Option<serde_json::Value>);

/// A simple mock plugin that records incoming RPC calls for asserting tests.
struct MockNotifierPlugin {
    pub calls: Arc<Mutex<Vec<RecordedCall>>>,
}

impl MockNotifierPlugin {
    fn new() -> Self {
        Self {
            calls: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

#[async_trait]
impl PluginInstance for MockNotifierPlugin {
    fn priority(&self) -> i32 {
        0
    }
    fn instance_id(&self) -> &str {
        "mock_notifier"
    }

    fn plugin_info(&self) -> plugin_sdk::traits::PluginTypeInfo {
        plugin_sdk::traits::PluginTypeInfo {
            display_name: "MockNotifier".to_string(),
            version: "1.0.0".to_string(),
            author: jumbie_shared::plugin::JUMBIE_AUTHOR.to_string(),
            description: "Mock notifier for integration tests".to_string(),
            capabilities: vec![Capability::Notifier],
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

    async fn call(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<serde_json::Value> {
        self.calls.lock().await.push((method.to_string(), params));
        Ok(serde_json::json!({"status": "success"}))
    }
}

/// Helper: register a mock notifier and return its call recorder.
async fn register_mock_notifier(
    state: &Arc<AppState>,
) -> Arc<tokio::sync::Mutex<Vec<RecordedCall>>> {
    let mock_plugin = Arc::new(MockNotifierPlugin::new());
    let calls_ref = mock_plugin.calls.clone();
    {
        let mut pm: tokio::sync::RwLockWriteGuard<'_, jumbie::plugins::PluginManager> =
            state.plugin_manager.write().await;
        pm.add_internal_plugin(mock_plugin);
    }
    calls_ref
}

#[tokio::test]
async fn test_enqueue_download_fires_download_started_when_automated() {
    let (_app, state, _tmp) = common::setup_test_app().await;
    let calls = register_mock_notifier(&state).await;

    let notif_mgr = state
        .notifications
        .as_ref()
        .expect("NotifierManager should be initialized");
    let guard = notif_mgr.read().await;

    ContentOrganizer::enqueue_download(
        jumbie::download_orchestrator::download::EnqueueDownloadParams {
            db: &state.db,
            notifications: Some(&*guard),
            media_name: "Test.Show.S01E01",
            media_link: "magnet:?xt=urn:btih:test123",
            source: "",
            series_title: "Test Show",
            series_id: "series-1",
            episodes: &[1],
            seasons: &[1],
            episode_id: None,
            score: 100,
            is_user_requested: false,
            is_manual: false,
            is_season_pack: false,

            category: "Series",
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
        },
    )
    .await
    .expect("enqueue_download should succeed");

    drop(guard);

    let recorded = calls.lock().await;
    assert_eq!(recorded.len(), 1, "expected exactly one notification");
    let (method, params) = &recorded[0];
    assert_eq!(method, "notify");
    let payload = params.as_ref().expect("expected params");
    assert_eq!(payload["event"], "DownloadStarted");
    assert_eq!(payload["context"]["series_title"], "Test Show");
    assert_eq!(payload["context"]["release_title"], "Test.Show.S01E01");
}

#[tokio::test]
async fn test_enqueue_download_skips_notification_when_user_requested() {
    let (_app, state, _tmp) = common::setup_test_app().await;
    let calls = register_mock_notifier(&state).await;

    let notif_mgr = state
        .notifications
        .as_ref()
        .expect("NotifierManager should be initialized");
    let guard = notif_mgr.read().await;

    ContentOrganizer::enqueue_download(
        jumbie::download_orchestrator::download::EnqueueDownloadParams {
            db: &state.db,
            notifications: Some(&*guard),
            media_name: "Test.Show.S01E01",
            media_link: "magnet:?xt=urn:btih:test456",
            source: "",
            series_title: "Test Show",
            series_id: "series-2",
            episodes: &[1],
            seasons: &[1],
            episode_id: None,
            score: 100,
            is_user_requested: true,
            is_manual: false,
            is_season_pack: false,

            category: "Series",
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
        },
    )
    .await
    .expect("enqueue_download should succeed");

    drop(guard);

    let recorded = calls.lock().await;
    assert_eq!(
        recorded.len(),
        0,
        "expected no notification when is_user_requested=true"
    );
}

#[tokio::test]
async fn test_notifier_events_propagate() {
    let (_app, state, _tmp) = common::setup_test_app().await;
    let calls = register_mock_notifier(&state).await;

    // Dispatch through the NotifierManager's notify().
    if let Some(ref notifications) = state.notifications {
        let notifier = notifications.read().await;

        let err_ctx = NotifierContext::new().with_error("test_context", "test_error_message");
        notifier.notify(NotifierEvent::Error, &err_ctx).await;
    } else {
        panic!("NotifierManager should be initialized in setup_test_app");
    }

    let recorded_calls = calls.lock().await;
    assert_eq!(
        recorded_calls.len(),
        1,
        "Expected exactly one recorded method call"
    );

    let (ref method1, ref params1) = recorded_calls[0];
    assert_eq!(method1, "notify");
    let payload1 = params1.as_ref().expect("Expected valid json params");
    assert_eq!(payload1["event"], "Error");
    assert_eq!(payload1["context"]["error_context"], "test_context");
    assert_eq!(payload1["context"]["error_message"], "test_error_message");
}
