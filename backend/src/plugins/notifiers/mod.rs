use crate::models::media::ReleaseCandidate;

use crate::plugins::PluginManager;
use jumbie_shared::plugin::Capability;
use jumbie_shared::types::MediaInfo;
use jumbie_shared::variables::TemplateContext;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Notification event types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NotifierEvent {
    DownloadStarted,
    DownloadCompleted,
    Error,
    RenameQueue,
    Test,
}

impl NotifierEvent {
    /// Return the PascalCase string representation used in JSON-RPC dispatch.
    pub fn as_str(&self) -> &'static str {
        match self {
            NotifierEvent::DownloadStarted => "DownloadStarted",
            NotifierEvent::DownloadCompleted => "DownloadCompleted",
            NotifierEvent::Error => "Error",
            NotifierEvent::RenameQueue => "RenameQueue",
            NotifierEvent::Test => "Test",
        }
    }
}

use tracing::warn;

impl NotifierEvent {
    pub fn template_context(&self) -> TemplateContext {
        match self {
            NotifierEvent::DownloadStarted | NotifierEvent::DownloadCompleted => {
                TemplateContext::NotifierDownload
            }
            NotifierEvent::Error => TemplateContext::NotifierError,
            NotifierEvent::RenameQueue => TemplateContext::NotifierRenameQueue,
            NotifierEvent::Test => TemplateContext::Notifier,
        }
    }

    /// Check that the context contains fields expected for this event type.
    /// Returns descriptions of missing fields (empty = all good).
    /// Called by [`NotifierManager::notify`] so ALL notifier plugins benefit.
    pub fn validate_context(&self, ctx: &NotifierContext) -> Vec<&'static str> {
        match self {
            NotifierEvent::DownloadStarted | NotifierEvent::DownloadCompleted => {
                let mut missing = Vec::new();
                if ctx.series_title.is_none() {
                    missing.push("series_title");
                }
                if ctx.release_title.is_none() {
                    missing.push("release_title");
                }
                missing
            }
            NotifierEvent::Error => {
                let mut missing = Vec::new();
                if ctx.error_context.is_none() {
                    missing.push("error_context");
                }
                if ctx.error_message.is_none() {
                    missing.push("error_message");
                }
                missing
            }
            NotifierEvent::RenameQueue => {
                let mut missing = Vec::new();
                if ctx.rename_queue_entries.is_empty() {
                    if ctx.series_title.is_none() {
                        missing.push("series_title");
                    }
                    if ctx.affected_count.is_none() {
                        missing.push("affected_count");
                    }
                }
                missing
            }
            NotifierEvent::Test => vec![],
        }
    }
}

/// Helper to parse standard notification parameters from RPC payloads
pub fn parse_notify_params(
    params: Option<Value>,
) -> anyhow::Result<(NotifierEvent, NotifierContext)> {
    let params = params.unwrap_or_default();
    let event_str = params.get("event").and_then(|v| v.as_str()).unwrap_or("");
    let context_val = params.get("context").cloned().unwrap_or_default();
    let context: NotifierContext = serde_json::from_value(context_val).unwrap_or_default();

    let event = match event_str {
        "DownloadStarted" => NotifierEvent::DownloadStarted,
        "DownloadCompleted" => NotifierEvent::DownloadCompleted,
        "Error" => NotifierEvent::Error,
        "RenameQueue" => NotifierEvent::RenameQueue,
        "Test" => NotifierEvent::Test,
        _ => anyhow::bail!("Unknown event: {}", event_str),
    };

    Ok((event, context))
}

/// Context data for notification events
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotifierContext {
    pub series_title: Option<String>,
    pub season: Option<i32>,
    pub episode: Option<i32>,
    pub path: Option<String>,
    pub error_context: Option<String>,
    pub error_message: Option<String>,
    pub affected_count: Option<i32>,
    /// The raw release title (e.g. "Show.Name.S01E02.1080p.WEB-DL"),
    /// populated from `ReleaseCandidate.title`. Used by Discord embeds.
    pub release_title: Option<String>,
    /// The indexer/source name (e.g. "Nyaa"),
    /// populated from `ReleaseCandidate.indexer`. Used by Discord embeds.
    pub indexer: Option<String>,
    /// The release size in bytes,
    /// populated from `ReleaseCandidate.size_bytes`. Used by Discord embeds.
    pub size_bytes: Option<u64>,
    /// Episode identifier (e.g. "S01E02" or "ABS003"),
    /// used by organize-success notifications.
    pub episode_id: Option<String>,
    /// The series UUID, used to construct links to the series page in notifiers.
    pub series_id: Option<String>,
    /// Media info from ffprobe scan (resolution, codec, duration, languages),
    /// populated after fingerprinting. Used by Discord embeds.
    pub media_info: Option<MediaInfo>,
    /// End episode for multi-episode releases (e.g. S01E01-E03 → episode=1, episode_end=3).
    /// None means this is a single-episode release.
    /// Raw data — each notifier plugin formats it as it sees fit.
    #[serde(default)]
    pub episode_end: Option<i32>,
    /// True when the download is a season/range pack covering multiple episodes.
    /// Wired up for context completeness; not all notifiers will use it.
    #[serde(default)]
    pub is_season_pack: bool,
    /// When the pack spans multiple seasons (e.g. S01-S02), the end season.
    /// `season` holds the start, `season_end` holds the end (inclusive).
    /// `None` for single-season packs.
    #[serde(default)]
    pub season_end: Option<i32>,
    /// Batch entries for rename-queue notifications, used when combining
    /// multiple series into a single notification.
    #[serde(default)]
    pub rename_queue_entries: Vec<(String, i32)>,
    /// Semantic image URLs for notifications. Shared key convention:
    /// Any notifier plugin can read or write entries.
    /// Series images are reserved for future use and currently never populated.
    #[serde(default)]
    pub images: HashMap<String, String>,
}

impl NotifierContext {
    pub fn new() -> Self {
        Self {
            series_title: None,
            season: None,
            episode: None,
            episode_end: None,
            is_season_pack: false,
            season_end: None,
            path: None,
            error_context: None,
            error_message: None,
            affected_count: None,
            release_title: None,
            indexer: None,
            size_bytes: None,
            episode_id: None,
            series_id: None,
            media_info: None,
            rename_queue_entries: Vec::new(),
            images: HashMap::new(),
        }
    }

    pub fn with_series(mut self, title: impl Into<String>) -> Self {
        self.series_title = Some(title.into());
        self
    }

    pub fn with_episode(mut self, season: i32, episode: i32) -> Self {
        self.season = Some(season);
        self.episode = Some(episode);
        self
    }

    pub fn with_episode_end(mut self, end: i32) -> Self {
        self.episode_end = Some(end);
        self
    }

    pub fn with_season_pack(mut self) -> Self {
        self.is_season_pack = true;
        self
    }

    pub fn with_season_end(mut self, end: i32) -> Self {
        self.season_end = Some(end);
        self
    }

    pub fn with_path(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }

    pub fn with_error(mut self, context: impl Into<String>, message: impl Into<String>) -> Self {
        self.error_context = Some(context.into());
        self.error_message = Some(message.into());
        self
    }

    pub fn with_affected_count(mut self, count: i32) -> Self {
        self.affected_count = Some(count);
        self
    }

    pub fn with_episode_id(mut self, ep_id: impl Into<String>) -> Self {
        self.episode_id = Some(ep_id.into());
        self
    }

    pub fn with_series_id(mut self, id: impl Into<String>) -> Self {
        self.series_id = Some(id.into());
        self
    }

    pub fn with_release_title(mut self, title: impl Into<String>) -> Self {
        self.release_title = Some(title.into());
        self
    }

    pub fn with_indexer(mut self, indexer: impl Into<String>) -> Self {
        self.indexer = Some(indexer.into());
        self
    }

    pub fn with_size_bytes(mut self, bytes: u64) -> Self {
        self.size_bytes = Some(bytes);
        self
    }

    pub fn with_media_info(mut self, info: MediaInfo) -> Self {
        self.media_info = Some(info);
        self
    }

    pub fn with_rename_queue_entries(mut self, entries: Vec<(String, i32)>) -> Self {
        self.rename_queue_entries = entries;
        self
    }

    /// Set a semantic image URL (e.g. "series", "episode").
    /// Shared across all notifiers — keys are a convention, not an enum.
    pub fn with_image(mut self, key: impl Into<String>, url: impl Into<String>) -> Self {
        self.images.insert(key.into(), url.into());
        self
    }

    /// Build a context from a `ReleaseCandidate`, populating series info,
    /// release title, indexer, size, and episode range in one shot.
    pub fn from_release(r: &ReleaseCandidate) -> Self {
        let ep_end = if r.episode_info.episodes.len() > 1 {
            r.episode_info.episodes.last().copied()
        } else {
            None
        };
        Self {
            series_title: Some(r.mapping.name.clone()),
            season: r.episode_info.seasons.first().copied(),
            episode: r.episode_info.episodes.first().copied(),
            episode_end: ep_end,
            is_season_pack: ep_end.is_some(),
            season_end: r.episode_info.seasons.last().copied(),
            path: None,
            error_context: None,
            error_message: None,
            affected_count: None,
            release_title: Some(r.title.clone()),
            indexer: Some(r.indexer.clone()),
            size_bytes: Some(r.size_bytes),
            episode_id: None,
            series_id: None,
            media_info: None,
            rename_queue_entries: Vec::new(),
            images: HashMap::new(),
        }
    }
}

impl Default for NotifierContext {
    fn default() -> Self {
        Self::new()
    }
}

/// Orchestrates all registered notification plugins.
///
/// Holds no plugins directly — it delegates to the central PluginManager at
/// runtime, so notifier config changes are picked up without recreating the
/// manager. It is a thin fire-and-forget broker: every notifier plugin with the
/// Notifier capability is called, and failures are logged but never propagated
/// (a single failing webhook must not block other notifications).
pub struct NotifierManager {
    plugin_manager: Arc<RwLock<PluginManager>>,
}

impl NotifierManager {
    pub fn new(plugin_manager: Arc<RwLock<PluginManager>>) -> Self {
        Self { plugin_manager }
    }

    /// Fire-and-forget call to all notifier plugins for the given method.
    ///
    /// Failures are logged individually but never propagated — one broken notifier
    /// must not silence others.
    async fn notify_all(&self, method: &str, params: Option<Value>) {
        let plugins_guard = self.plugin_manager.read().await;
        let plugins = plugins_guard.get_plugins_by_capability(Capability::Notifier);

        for p in plugins {
            if let Err(e) = p.call(method, params.clone()).await {
                tracing::warn!(
                    "Notification plugin {} ({}) failed ({}): {}",
                    p.plugin_info().display_name,
                    p.instance_id(),
                    method,
                    e
                );
            }
        }
    }

    /// Send a notification to all registered plugins.
    ///
    /// Fire-and-forget: notifications are advisory, so a failed webhook must not
    /// crash the pipeline or retry the download. Individual failures are logged for
    /// diagnosis; the caller handles no notifier errors.
    pub async fn notify(&self, event: NotifierEvent, context: &NotifierContext) {
        // Context guard: warn if expected fields are missing. `validate_context` is
        // the SSoT for per-event expectations, so every notifier benefits without
        // per-plugin duplication.
        let missing = event.validate_context(context);
        if !missing.is_empty() {
            warn!(
                "{} notification fired with missing context fields: {}",
                format!("{:?}", event),
                missing.join(", ")
            );
        }

        // Content guard: warn on invalid field values, reusing the same shared
        // validators as `bridge::notifiers`.
        if let Some(s) = context.season
            && jumbie_shared::validation::validate_season_number(&s.to_string()).is_err()
        {
            warn!(
                "{} notification has invalid season {} (expected 0..10000)",
                format!("{:?}", event),
                s,
            );
        }
        if let Some(ref ep) = context.episode
            && jumbie_shared::validation::validate_episode_number(*ep).is_err()
        {
            warn!(
                "{} notification has invalid episode {} (expected 1..10000)",
                format!("{:?}", event),
                ep,
            );
        }
        if let Some(s) = context.size_bytes
            && s > 1_000_000_000_000
        {
            warn!(
                "{} notification has size_bytes {} exceeding 1TB",
                format!("{:?}", event),
                s,
            );
        }

        let params = serde_json::json!({
            "event": event,
            "context": context,
        });
        self.notify_all("notify", Some(params)).await;
    }
}

// Built-in notifier plugins: `discord` (embed messages to Discord webhook URLs)
// and `template` (the shared template rendering engine). Additional notifiers
// (email, Slack, Pushover, ...) can be added as modules here and registered in the
// plugin registry.
pub mod discord;
pub mod template;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::{PluginCallError, PluginInstance};
    use anyhow::Result;
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicI32, Ordering};

    // Mock Notifier Plugin
    // Records which events it received instead of sending anything, so tests can
    // assert that `call("notify", ...)` deserializes params and dispatches correctly.

    struct MockNotifier {
        name: String,
        last_event: Arc<RwLock<Option<NotifierEvent>>>,
        last_context: Arc<RwLock<Option<NotifierContext>>>,
        call_count: Arc<AtomicI32>,
    }

    impl MockNotifier {
        fn new(name: &str) -> (Self, Arc<RwLock<Option<NotifierEvent>>>, Arc<AtomicI32>) {
            let last_event = Arc::new(RwLock::new(None));
            let call_count = Arc::new(AtomicI32::new(0));
            let ctx = Arc::new(RwLock::new(None));
            (
                Self {
                    name: name.to_string(),
                    last_event: last_event.clone(),
                    last_context: ctx.clone(),
                    call_count: call_count.clone(),
                },
                last_event,
                call_count,
            )
        }

        async fn notify(&self, event: NotifierEvent, context: &NotifierContext) -> Result<()> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            *self.last_event.write().await = Some(event);
            *self.last_context.write().await = Some(context.clone());
            Ok(())
        }
    }

    #[async_trait]
    impl PluginInstance for MockNotifier {
        fn instance_id(&self) -> &str {
            &self.name
        }

        fn plugin_info(&self) -> plugin_sdk::traits::PluginTypeInfo {
            plugin_sdk::traits::PluginTypeInfo {
                display_name: self.name.clone(),
                version: "1.0.0".to_string(),
                author: jumbie_shared::plugin::JUMBIE_AUTHOR.to_string(),
                description: format!("{} notifier", self.name),
                capabilities: vec![Capability::Notifier],
                supported_protocols: None,
                series_identifier_label: None,
                series_identifier_placeholder: None,
                rate_limit: None,
                supports_test: true,
            }
        }

        fn priority(&self) -> i32 {
            0
        }

        fn supported_protocols(&self) -> Option<&[String]> {
            None
        }

        async fn handle_custom_method(&self, method: &str, params: Option<Value>) -> Result<Value> {
            match method {
                "notify" => {
                    let (event, context) = parse_notify_params(params)?;
                    self.notify(event, &context).await?;
                    Ok(Value::Null)
                }
                "on_test" => {
                    self.notify(NotifierEvent::Test, &NotifierContext::new())
                        .await?;
                    Ok(serde_json::json!(format!(
                        "Test message sent to {}",
                        self.name
                    )))
                }
                _ => Err(anyhow::anyhow!(PluginCallError::MethodNotSupported(
                    method.to_string()
                ))),
            }
        }

        async fn test_impl(&self) -> Result<()> {
            self.notify(NotifierEvent::Test, &NotifierContext::new())
                .await
        }
    }

    // Dispatch Tests
    // PluginInstance.call("notify", ...) must dispatch through handle_custom_method
    // to the internal notify logic.

    fn make_wrapper_with_id(_id: &str, mock: MockNotifier) -> Arc<dyn PluginInstance> {
        Arc::new(mock) as Arc<dyn PluginInstance>
    }

    fn make_wrapper(mock: MockNotifier) -> Arc<dyn PluginInstance> {
        make_wrapper_with_id("test_notifier", mock)
    }

    fn notify_params(event: NotifierEvent, context: &NotifierContext) -> Option<Value> {
        Some(serde_json::json!({
            "event": event,
            "context": context,
        }))
    }

    #[tokio::test]
    async fn test_wrapper_dispatches_download_started() {
        let (mock, last_event, count) = MockNotifier::new("test");
        let wrapper = make_wrapper(mock);
        let ctx = NotifierContext::new()
            .with_series("Test")
            .with_episode(1, 1);

        wrapper
            .call(
                "notify",
                notify_params(NotifierEvent::DownloadStarted, &ctx),
            )
            .await
            .unwrap();

        assert_eq!(count.load(Ordering::SeqCst), 1);
        assert_eq!(
            *last_event.read().await,
            Some(NotifierEvent::DownloadStarted)
        );
    }

    #[tokio::test]
    async fn test_wrapper_dispatches_download_completed() {
        let (mock, last_event, count) = MockNotifier::new("test");
        let wrapper = make_wrapper(mock);
        let ctx = NotifierContext::new().with_series("Test");

        wrapper
            .call(
                "notify",
                notify_params(NotifierEvent::DownloadCompleted, &ctx),
            )
            .await
            .unwrap();

        assert_eq!(count.load(Ordering::SeqCst), 1);
        assert_eq!(
            *last_event.read().await,
            Some(NotifierEvent::DownloadCompleted)
        );
    }

    #[tokio::test]
    async fn test_wrapper_dispatches_rename_queue() {
        let (mock, last_event, count) = MockNotifier::new("test");
        let wrapper = make_wrapper(mock);
        let ctx = NotifierContext::new()
            .with_series("Test")
            .with_affected_count(5);

        wrapper
            .call("notify", notify_params(NotifierEvent::RenameQueue, &ctx))
            .await
            .unwrap();

        assert_eq!(count.load(Ordering::SeqCst), 1);
        assert_eq!(*last_event.read().await, Some(NotifierEvent::RenameQueue));
    }

    #[tokio::test]
    async fn test_wrapper_dispatches_error() {
        let (mock, last_event, count) = MockNotifier::new("test");
        let wrapper = make_wrapper(mock);
        let ctx = NotifierContext::new().with_error("Test", "error message");

        wrapper
            .call("notify", notify_params(NotifierEvent::Error, &ctx))
            .await
            .unwrap();

        assert_eq!(count.load(Ordering::SeqCst), 1);
        assert_eq!(*last_event.read().await, Some(NotifierEvent::Error));
    }

    #[tokio::test]
    async fn test_wrapper_passes_context_fields() {
        let (mock, _last_event, _count) = MockNotifier::new("test");
        let wrapper = make_wrapper(mock);
        let ctx = NotifierContext::new()
            .with_series("Test Series")
            .with_episode(4, 28)
            .with_affected_count(12)
            .with_path("/media/Test Series/Season 4/S04E28.mkv");

        wrapper
            .call("notify", notify_params(NotifierEvent::RenameQueue, &ctx))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_wrapper_dispatches_test_event() {
        let (mock, last_event, count) = MockNotifier::new("test");
        let wrapper = make_wrapper(mock);
        let ctx = NotifierContext::new();

        wrapper
            .call("notify", notify_params(NotifierEvent::Test, &ctx))
            .await
            .unwrap();

        assert_eq!(count.load(Ordering::SeqCst), 1);
        assert_eq!(*last_event.read().await, Some(NotifierEvent::Test));
    }

    #[tokio::test]
    async fn test_wrapper_test_rpc_method_dispatch() {
        // The "on_test"/"test" RPC method should dispatch through notify(Test).
        let (mock, last_event, count) = MockNotifier::new("test");
        let wrapper = make_wrapper(mock);

        let result = wrapper.call("on_test", None).await;
        assert!(result.is_ok(), "on_test RPC should succeed");
        assert_eq!(count.load(Ordering::SeqCst), 1);
        assert_eq!(*last_event.read().await, Some(NotifierEvent::Test));

        let result = wrapper.call("test", None).await;
        assert!(result.is_ok(), "test RPC should succeed");
        assert_eq!(count.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn test_wrapper_unknown_event_returns_error() {
        let (mock, _, _) = MockNotifier::new("test");
        let wrapper = make_wrapper(mock);

        let result = wrapper
            .call(
                "notify",
                Some(serde_json::json!({
                    "event": "NonExistentEvent",
                    "context": {}
                })),
            )
            .await;

        assert!(result.is_err(), "Unknown event should produce an error");
    }

    // Context Validation Tests

    #[test]
    fn test_validate_context_download_requires_series_and_release() {
        let event = NotifierEvent::DownloadStarted;
        let ctx = NotifierContext::new();

        let missing = event.validate_context(&ctx);
        assert!(missing.contains(&"series_title"));
        assert!(missing.contains(&"release_title"));

        let ctx = NotifierContext::new()
            .with_series("Test")
            .with_release_title("Test.S01E01");
        let missing = event.validate_context(&ctx);
        assert!(
            missing.is_empty(),
            "expected no missing fields: {:?}",
            missing
        );
    }

    #[test]
    fn test_validate_context_download_completed_same_as_started() {
        // Both DownloadStarted and DownloadCompleted expect series + release
        let completed_missing =
            NotifierEvent::DownloadCompleted.validate_context(&NotifierContext::new());
        let started_missing =
            NotifierEvent::DownloadStarted.validate_context(&NotifierContext::new());
        assert_eq!(completed_missing, started_missing);
    }

    #[test]
    fn test_validate_context_error_requires_error_fields() {
        let event = NotifierEvent::Error;
        let ctx = NotifierContext::new();

        let missing = event.validate_context(&ctx);
        assert!(missing.contains(&"error_context"));
        assert!(missing.contains(&"error_message"));

        let ctx = NotifierContext::new().with_error("Test", "msg");
        let missing = event.validate_context(&ctx);
        assert!(
            missing.is_empty(),
            "expected no missing fields: {:?}",
            missing
        );
    }

    #[test]
    fn test_validate_context_rename_queue_requires_series_and_count() {
        let event = NotifierEvent::RenameQueue;
        let ctx = NotifierContext::new();

        let missing = event.validate_context(&ctx);
        assert!(missing.contains(&"series_title"));
        assert!(missing.contains(&"affected_count"));

        let ctx = NotifierContext::new()
            .with_series("Test")
            .with_affected_count(5);
        let missing = event.validate_context(&ctx);
        assert!(
            missing.is_empty(),
            "expected no missing fields: {:?}",
            missing
        );
    }

    #[test]
    fn test_validate_context_rename_queue_with_batched_entries() {
        let event = NotifierEvent::RenameQueue;
        let ctx = NotifierContext::new().with_rename_queue_entries(vec![
            ("Dracula".to_string(), 12),
            ("Frankenstein".to_string(), 3),
        ]);

        let missing = event.validate_context(&ctx);
        assert!(
            missing.is_empty(),
            "expected no missing fields with batched entries: {:?}",
            missing
        );
    }

    #[test]
    fn test_validate_context_test_always_valid() {
        let missing = NotifierEvent::Test.validate_context(&NotifierContext::new());
        assert!(missing.is_empty());
    }

    // NotifierManager integration tests: notify() must call through to all
    // registered plugins.

    #[tokio::test]
    async fn test_manager_notify_dispatches_to_plugins() {
        let (mock1, _e1, c1) = MockNotifier::new("notifier_a");
        let (mock2, _e2, c2) = MockNotifier::new("notifier_b");

        let plugin_manager = Arc::new(RwLock::new(crate::plugins::PluginManager::new(
            std::env::temp_dir().join("notifier_test_plugins"),
        )));
        {
            let mut pm = plugin_manager.write().await;
            pm.add_internal_plugin(make_wrapper_with_id("notifier_a", mock1));
            pm.add_internal_plugin(make_wrapper_with_id("notifier_b", mock2));
        }

        let manager = NotifierManager::new(plugin_manager);
        let ctx = NotifierContext::new().with_series("Test");

        manager.notify(NotifierEvent::DownloadStarted, &ctx).await;

        // Both plugins should have been called exactly once
        assert_eq!(c1.load(Ordering::SeqCst), 1);
        assert_eq!(c2.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_manager_notify_handles_missing_plugins_gracefully() {
        let plugin_manager = Arc::new(RwLock::new(crate::plugins::PluginManager::new(
            std::env::temp_dir().join("notifier_test_plugins"),
        )));
        let manager = NotifierManager::new(plugin_manager);
        let ctx = NotifierContext::new().with_series("Test");

        // Should not panic or error when there are no notifier plugins
        manager.notify(NotifierEvent::DownloadStarted, &ctx).await;
        manager.notify(NotifierEvent::RenameQueue, &ctx).await;
    }
}
