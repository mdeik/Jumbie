use crate::plugins::PluginInstance;
use crate::plugins::bridge;
use crate::plugins::bridge::BridgeError;
use crate::plugins::metadata::{
    EpisodeMetadata, SeasonMetadata, SeriesMetadata, SeriesMetadataInfo,
};
use crate::plugins::notifiers::{NotifierContext, NotifierEvent};
use async_trait::async_trait;
use jumbie_shared::validation::Validate;
use jumbie_shared::validation::fields::MAX_FAILURE_REASON_LENGTH;
use jumbie_shared::validation::plugin_data::MAX_PLUGIN_ITEMS;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

// Mock Plugin
// A simple mock that returns pre-configured JSON responses.

struct MockPlugin {
    id: String,
    responses: Arc<Mutex<HashMap<String, Result<Value, String>>>>,
}

impl MockPlugin {
    fn new(id: &str) -> Self {
        Self {
            id: id.to_string(),
            responses: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn add_response(&self, method: &str, response: Result<Value, String>) {
        self.responses
            .lock()
            .unwrap()
            .insert(method.to_string(), response);
    }

    fn info() -> jumbie_shared::plugin::PluginTypeInfo {
        jumbie_shared::plugin::PluginTypeInfo {
            display_name: "mock".to_string(),
            version: "1.0".to_string(),
            author: "test".to_string(),
            description: "".to_string(),
            capabilities: vec![],
            supported_protocols: None,
            series_identifier_label: None,
            series_identifier_placeholder: None,
            rate_limit: None,
            supports_test: false,
        }
    }
}

#[async_trait]
impl PluginInstance for MockPlugin {
    fn instance_id(&self) -> &str {
        &self.id
    }
    fn plugin_info(&self) -> jumbie_shared::plugin::PluginTypeInfo {
        Self::info()
    }
    fn supported_protocols(&self) -> Option<&[String]> {
        None
    }
    fn priority(&self) -> i32 {
        0
    }

    async fn call(&self, method: &str, _params: Option<Value>) -> anyhow::Result<Value> {
        let responses = self.responses.lock().unwrap();
        match responses.get(method) {
            Some(Ok(val)) => Ok(val.clone()),
            Some(Err(msg)) => Err(anyhow::anyhow!("{}", msg)),
            None => Err(anyhow::anyhow!("MethodNotSupported: {}", method)),
        }
    }
}

// EpisodeMetadata validation tests

#[test]
fn test_episode_metadata_valid() {
    let ep = EpisodeMetadata {
        unique_id: "ep-1".to_string(),
        season: 1,
        episode: 1,
        title: "Test Episode".to_string(),
        description: Some("A test".to_string()),
        runtime: Some(30),
        image_url: Some("https://example.com/img.jpg".to_string()),
        meta_date: None,
    };
    assert!(ep.validate().is_ok());
}

#[test]
fn test_episode_metadata_empty_unique_id() {
    let ep = EpisodeMetadata {
        unique_id: "".to_string(),
        season: 1,
        episode: 1,
        title: "Test".to_string(),
        description: None,
        runtime: None,
        image_url: None,
        meta_date: None,
    };
    let result = ep.validate();
    assert!(result.is_err());
    let errors = result.unwrap_err();
    assert!(errors.iter().any(|e| e.0.contains("unique_id")));
}

#[test]
fn test_episode_metadata_negative_season() {
    let ep = EpisodeMetadata {
        unique_id: "ep-1".to_string(),
        season: -1,
        episode: 1,
        title: "Test".to_string(),
        description: None,
        runtime: None,
        image_url: None,
        meta_date: None,
    };
    let result = ep.validate();
    assert!(result.is_err());
}

#[test]
fn test_episode_metadata_zero_episode() {
    let ep = EpisodeMetadata {
        unique_id: "ep-1".to_string(),
        season: 1,
        episode: 0,
        title: "Test".to_string(),
        description: None,
        runtime: None,
        image_url: None,
        meta_date: None,
    };
    let result = ep.validate();
    assert!(result.is_err());
}

#[test]
fn test_episode_metadata_season_zero_allowed() {
    // Season 0 (specials) is valid after the fix
    let ep = EpisodeMetadata {
        unique_id: "ep-special".to_string(),
        season: 0,
        episode: 1,
        title: "Special Episode".to_string(),
        description: None,
        runtime: None,
        image_url: None,
        meta_date: None,
    };
    assert!(ep.validate().is_ok());
}

#[test]
fn test_episode_metadata_negative_runtime() {
    let ep = EpisodeMetadata {
        unique_id: "ep-1".to_string(),
        season: 1,
        episode: 1,
        title: "Test".to_string(),
        description: None,
        runtime: Some(-5),
        image_url: None,
        meta_date: None,
    };
    let result = ep.validate();
    assert!(result.is_err());
}

#[test]
fn test_episode_metadata_runtime_too_large() {
    let ep = EpisodeMetadata {
        unique_id: "ep-1".to_string(),
        season: 1,
        episode: 1,
        title: "Test".to_string(),
        description: None,
        runtime: Some(999_999),
        image_url: None,
        meta_date: None,
    };
    let result = ep.validate();
    assert!(result.is_err());
}

#[test]
fn test_episode_metadata_invalid_image_url() {
    let ep = EpisodeMetadata {
        unique_id: "ep-1".to_string(),
        season: 1,
        episode: 1,
        title: "Test".to_string(),
        description: None,
        runtime: None,
        image_url: Some("ftp://bad-scheme.com/img.jpg".to_string()),
        meta_date: None,
    };
    let result = ep.validate();
    assert!(result.is_err());
}

#[test]
fn test_episode_metadata_relative_image_url_rejected() {
    // TVDB now prepends the CDN base URL in its implementation, so bare
    // relative paths are rejected at validation time.
    let ep = EpisodeMetadata {
        unique_id: "ep-1".to_string(),
        season: 1,
        episode: 1,
        title: "Test".to_string(),
        description: None,
        runtime: None,
        image_url: Some("/banners/v4/episode/123/screencap/abc.jpg".to_string()),
        meta_date: None,
    };
    assert!(ep.validate().is_err());
}

#[test]
fn test_episode_metadata_empty_title() {
    // Empty titles are allowed — the proactive enrichment system fills
    // them in asynchronously.
    let ep = EpisodeMetadata {
        unique_id: "ep-1".to_string(),
        season: 1,
        episode: 1,
        title: "".to_string(),
        description: None,
        runtime: None,
        image_url: None,
        meta_date: None,
    };
    assert!(ep.validate().is_ok());
}

// SeasonMetadata validation tests

#[test]
fn test_season_metadata_valid() {
    let s = SeasonMetadata {
        season: 1,
        episode_count: 24,
    };
    assert!(s.validate().is_ok());
}

#[test]
fn test_season_metadata_negative_season() {
    let s = SeasonMetadata {
        season: -1,
        episode_count: 10,
    };
    let result = s.validate();
    assert!(result.is_err());
}

#[test]
fn test_season_metadata_season_zero_allowed() {
    let s = SeasonMetadata {
        season: 0,
        episode_count: 5,
    };
    assert!(s.validate().is_ok());
}

#[test]
fn test_season_metadata_negative_episode_count() {
    let s = SeasonMetadata {
        season: 1,
        episode_count: -1,
    };
    let result = s.validate();
    assert!(result.is_err());
}

#[test]
fn test_season_metadata_episode_count_too_large() {
    let s = SeasonMetadata {
        season: 1,
        episode_count: 20_000,
    };
    let result = s.validate();
    assert!(result.is_err());
}

// SeriesMetadata validation tests

#[test]
fn test_series_metadata_valid() {
    let metadata = SeriesMetadata {
        episodes: vec![
            EpisodeMetadata {
                unique_id: "ep-1".to_string(),
                season: 1,
                episode: 1,
                title: "E1".to_string(),
                description: None,
                runtime: None,
                image_url: None,
                meta_date: None,
            },
            EpisodeMetadata {
                unique_id: "ep-2".to_string(),
                season: 1,
                episode: 2,
                title: "E2".to_string(),
                description: None,
                runtime: None,
                image_url: None,
                meta_date: None,
            },
        ],
        seasons: vec![SeasonMetadata {
            season: 1,
            episode_count: 2,
        }],
    };
    assert!(metadata.validate().is_ok());
}

#[test]
fn test_series_metadata_duplicate_seasons() {
    let metadata = SeriesMetadata {
        episodes: vec![],
        seasons: vec![
            SeasonMetadata {
                season: 1,
                episode_count: 10,
            },
            SeasonMetadata {
                season: 1,
                episode_count: 20,
            },
        ],
    };
    let result = metadata.validate();
    assert!(result.is_err());
    let errors = result.unwrap_err();
    assert!(errors.iter().any(|e| e.0.contains("duplicate")));
}

#[test]
fn test_series_metadata_invalid_episode() {
    let metadata = SeriesMetadata {
        episodes: vec![EpisodeMetadata {
            unique_id: "ep-bad".to_string(),
            season: -5,
            episode: 1,
            title: "Bad".to_string(),
            description: None,
            runtime: None,
            image_url: None,
            meta_date: None,
        }],
        seasons: vec![SeasonMetadata {
            season: -5,
            episode_count: 1,
        }],
    };
    let result = metadata.validate();
    assert!(result.is_err());
}

// SeriesMetadataInfo validation tests

#[test]
fn test_series_metadata_info_valid() {
    let info = SeriesMetadataInfo {
        name: "Test Series".to_string(),
        overview: Some("A great show".to_string()),
        original_country: Some("usa".to_string()),
        aliases: HashMap::new(),
        image_url: Some("https://example.com/poster.jpg".to_string()),
    };
    assert!(info.validate().is_ok());
}

#[test]
fn test_series_metadata_info_empty_name() {
    // Empty names are allowed — a provider may not always return a series title.
    let info = SeriesMetadataInfo {
        name: "".to_string(),
        overview: None,
        original_country: None,
        aliases: HashMap::new(),
        image_url: None,
    };
    assert!(info.validate().is_ok());
}

#[test]
fn test_series_metadata_info_invalid_image_url() {
    let info = SeriesMetadataInfo {
        name: "Test".to_string(),
        overview: None,
        original_country: None,
        aliases: HashMap::new(),
        image_url: Some("javascript:alert(1)".to_string()),
    };
    let result = info.validate();
    assert!(result.is_err());
}

#[test]
fn test_series_metadata_info_with_aliases() {
    let mut aliases = HashMap::new();
    aliases.insert("eng".to_string(), vec!["Alt Title".to_string()]);
    let info = SeriesMetadataInfo {
        name: "Test".to_string(),
        overview: None,
        original_country: None,
        aliases,
        image_url: None,
    };
    assert!(info.validate().is_ok());
}

#[test]
fn test_series_metadata_info_empty_alias() {
    // Empty aliases are skipped during validation, not rejected.
    let mut aliases = HashMap::new();
    aliases.insert("eng".to_string(), vec!["".to_string()]);
    let info = SeriesMetadataInfo {
        name: "Test".to_string(),
        overview: None,
        original_country: None,
        aliases,
        image_url: None,
    };
    assert!(info.validate().is_ok());
}

// Bridge: metadata::fetch_episodes_and_seasons tests

#[tokio::test]
async fn test_fetch_episodes_and_seasons_valid() {
    let plugin = MockPlugin::new("test.tvdb");
    plugin.add_response(
        "fetch_series_metadata",
        Ok(serde_json::json!({
            "episodes_and_seasons": {
                "episodes": [
                    {
                        "unique_id": "ep-1",
                        "season": 1,
                        "episode": 1,
                        "title": "Episode 1"
                    }
                ],
                "seasons": [
                    { "season": 1, "episode_count": 1 }
                ]
            }
        })),
    );

    let result = bridge::metadata::fetch_episodes_and_seasons(&plugin, "123", false).await;
    assert!(result.is_ok());
    let metadata = result.unwrap();
    assert_eq!(metadata.episodes.len(), 1);
    assert_eq!(metadata.seasons.len(), 1);
}

#[tokio::test]
async fn test_fetch_episodes_and_seasons_invalid_episode() {
    let plugin = MockPlugin::new("test.tvdb");
    plugin.add_response(
        "fetch_series_metadata",
        Ok(serde_json::json!({
            "episodes_and_seasons": {
                "episodes": [
                    {
                        "unique_id": "ep-bad",
                        "season": -99,
                        "episode": 0,
                        "title": "Bad Episode"
                    }
                ],
                "seasons": [
                    { "season": 1, "episode_count": 10 }
                ]
            }
        })),
    );

    let result = bridge::metadata::fetch_episodes_and_seasons(&plugin, "123", false).await;
    assert!(result.is_err());
    match result.unwrap_err() {
        BridgeError::Validation(msg) => {
            assert!(msg.contains("Episode") || msg.contains("season") || msg.contains("episode"));
        }
        _ => panic!("Expected Validation error"),
    }
}

#[tokio::test]
async fn test_fetch_episodes_and_seasons_missing_field() {
    let plugin = MockPlugin::new("test.tvdb");
    plugin.add_response(
        "fetch_series_metadata",
        Ok(serde_json::json!({ "not_episodes": [] })),
    );

    let result = bridge::metadata::fetch_episodes_and_seasons(&plugin, "123", false).await;
    assert!(result.is_err());
    match result.unwrap_err() {
        BridgeError::Deserialization(msg) => {
            assert!(msg.contains("episodes_and_seasons"));
        }
        _ => panic!("Expected Deserialization error"),
    }
}

#[tokio::test]
async fn test_fetch_episodes_and_seasons_network_error() {
    let plugin = MockPlugin::new("test.tvdb");
    plugin.add_response("fetch_series_metadata", Err("Network timeout".to_string()));

    let result = bridge::metadata::fetch_episodes_and_seasons(&plugin, "123", false).await;
    assert!(result.is_err());
    match result.unwrap_err() {
        BridgeError::Call(_) => {} // expected
        _ => panic!("Expected Call error"),
    }
}

// Bridge: metadata::fetch_series_info tests

#[tokio::test]
async fn test_fetch_series_info_valid() {
    let plugin = MockPlugin::new("test.tvdb");
    plugin.add_response(
        "fetch_series_info",
        Ok(serde_json::json!({
            "name": "Test Series",
            "overview": "A test series",
            "original_country": "usa",
            "aliases": {},
            "image_url": "https://example.com/poster.jpg"
        })),
    );

    let result = bridge::metadata::fetch_series_info(&plugin, "123").await;
    assert!(result.is_ok());
    let info = result.unwrap();
    assert_eq!(info.name, "Test Series");
}

#[tokio::test]
async fn test_fetch_series_info_empty_name() {
    let plugin = MockPlugin::new("test.tvdb");
    plugin.add_response(
        "fetch_series_info",
        Ok(serde_json::json!({
            "name": ""
        })),
    );

    // Empty names are now accepted at validation time.
    let result = bridge::metadata::fetch_series_info(&plugin, "123").await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap().name, "");
}

// Bridge: metadata::fetch_series_aliases tests

#[tokio::test]
async fn test_fetch_series_aliases_valid() {
    let plugin = MockPlugin::new("test.tvdb");
    plugin.add_response(
        "fetch_series_aliases",
        Ok(serde_json::json!(["Alias One", "Alias Two"])),
    );

    let result = bridge::metadata::fetch_series_aliases(&plugin, "123").await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap().len(), 2);
}

#[tokio::test]
async fn test_fetch_series_aliases_empty_alias_skipped() {
    let plugin = MockPlugin::new("test.tvdb");
    plugin.add_response(
        "fetch_series_aliases",
        Ok(serde_json::json!(["Good Alias", ""])),
    );

    // Empty aliases are skipped (log-and-skip), not rejected.
    let result = bridge::metadata::fetch_series_aliases(&plugin, "123").await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap().len(), 1);
}

// Bridge: sources::search tests

fn test_search_log() -> crate::search::SearchLog {
    crate::search::SearchLog {
        kind: crate::search::SearchKind::Manual,
        source: "test".to_string(),
        series_id: None,
        series_title: None,
        season: None,
        source_episodes: Vec::new(),
        aliases: Vec::new(),
    }
}

#[tokio::test]
async fn test_sources_search_valid() {
    let plugin = MockPlugin::new("test.source");
    plugin.add_response(
        "search",
        Ok(serde_json::json!({
            "entries": [
                {
                    "title": "Test Release",
                    "source": "test",
                    "link": "https://example.com/torrent",
                    "size": 500000000,
                    "seeders": 10,
                    "guid": "guid-1"
                }
            ],
            "queries": ["test query"]
        })),
    );

    let result = bridge::sources::search(&plugin, "test query", &test_search_log()).await;
    assert!(result.is_ok());
    let entries = result.unwrap();
    assert_eq!(entries.len(), 1);
}

#[tokio::test]
async fn test_sources_search_legacy_bare_array_is_rejected() {
    // The envelope is the enforced standard: a bare `Vec<MediaEntry>` no longer
    // deserializes, so a plugin can't silently skip reporting its queries.
    let plugin = MockPlugin::new("test.source");
    plugin.add_response("search", Ok(serde_json::json!([{ "title": "x" }])));

    let result = bridge::sources::search(&plugin, "test query", &test_search_log()).await;
    assert!(matches!(result, Err(BridgeError::Deserialization(_))));
}

#[tokio::test]
async fn test_sources_search_invalid_entry_is_filtered() {
    let plugin = MockPlugin::new("test.source");
    plugin.add_response(
        "search",
        Ok(serde_json::json!({
            "entries": [
                {
                    "title": "Valid Release",
                    "source": "test",
                    "link": "https://example.com/valid",
                    "size": 500000000,
                    "guid": "guid-1"
                },
                {
                    "title": "",  // empty title — invalid
                    "source": "test",
                    "link": "https://example.com/bad",
                    "guid": "guid-2"
                }
            ],
            "queries": ["test query"]
        })),
    );

    let result = bridge::sources::search(&plugin, "test query", &test_search_log()).await;
    assert!(result.is_ok());
    // Invalid entries should be filtered out, not rejected
    let entries = result.unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].title, "Valid Release");
}

// Bridge: sources::auto_search tests

#[tokio::test]
async fn test_sources_auto_search_valid() {
    let plugin = MockPlugin::new("test.source");
    plugin.add_response(
        "auto_search",
        Ok(serde_json::json!({
            "entries": [
                {
                    "title": "Auto Found",
                    "source": "test",
                    "link": "https://example.com/auto",
                    "size": 1000000,
                    "guid": "guid-auto-1"
                }
            ],
            "queries": ["Test S01(\"E01\")"]
        })),
    );

    let result = bridge::sources::auto_search(
        &plugin,
        serde_json::json!({ "series_title": "Test", "season": 1, "episodes": [1] }),
        &test_search_log(),
    )
    .await;
    assert!(result.is_ok());
}

// Bridge: sources::fetch_entries tests

#[tokio::test]
async fn test_sources_fetch_entries_valid() {
    let plugin = MockPlugin::new("test.source");
    plugin.add_response(
        "fetch_entries",
        Ok(serde_json::json!([
            {
                "title": "Feed Entry",
                "source": "rss",
                "link": "https://example.com/feed",
                "size": 2000000,
                "guid": "guid-feed-1"
            }
        ])),
    );

    let result = bridge::sources::fetch_entries(&plugin).await;
    assert!(result.is_ok());
}

// Bridge: downloader tests

#[tokio::test]
async fn test_downloader_add_download_empty_url() {
    let plugin = MockPlugin::new("test.dl");
    let result = bridge::downloaders::add_download(&plugin, "", "tv", None, None).await;
    assert!(result.is_err());
    match result.unwrap_err() {
        BridgeError::Validation(msg) => assert!(msg.contains("URL") || msg.contains("empty")),
        _ => panic!("Expected Validation error"),
    }
}

#[tokio::test]
async fn test_downloader_add_download_invalid_link() {
    // The bridge is the URL-validation boundary: a non-magnet, scheme-less
    // string is rejected before it reaches the client.
    let plugin = MockPlugin::new("test.dl");
    let result = bridge::downloaders::add_download(&plugin, "not a link", "tv", None, None).await;
    assert!(matches!(result, Err(BridgeError::Validation(_))));
}

#[tokio::test]
async fn test_downloader_add_download_magnet_accepted() {
    let plugin = MockPlugin::new("test.dl");
    plugin.add_response("add_download", Ok(serde_json::json!(null)));

    let result =
        bridge::downloaders::add_download(&plugin, "magnet:?xt=urn:btih:abcdef", "tv", None, None)
            .await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_downloader_get_completed_downloads() {
    let plugin = MockPlugin::new("test.dl");
    plugin.add_response(
        "get_completed_downloads",
        Ok(serde_json::json!(["hash1", "hash2"])),
    );

    let result = bridge::downloaders::get_completed_downloads(&plugin).await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap().len(), 2);
}

#[tokio::test]
async fn test_downloader_get_completed_downloads_capped() {
    // A runaway plugin returning more than MAX_PLUGIN_ITEMS ids is truncated.
    let plugin = MockPlugin::new("test.dl");
    let ids: Vec<String> = (0..MAX_PLUGIN_ITEMS + 1).map(|i| format!("h{i}")).collect();
    plugin.add_response("get_completed_downloads", Ok(serde_json::json!(ids)));

    let result = bridge::downloaders::get_completed_downloads(&plugin).await;
    assert_eq!(result.unwrap().len(), MAX_PLUGIN_ITEMS);
}

#[tokio::test]
async fn test_downloader_get_download_progress_invalid_range() {
    let plugin = MockPlugin::new("test.dl");
    plugin.add_response(
        "get_download_progress",
        Ok(serde_json::json!(2.5)), // > 1.0 — invalid
    );

    let result = bridge::downloaders::get_download_progress(&plugin, "hash").await;
    assert!(result.is_err());
    match result.unwrap_err() {
        BridgeError::Validation(msg) => assert!(msg.contains("progress") || msg.contains("range")),
        _ => panic!("Expected Validation error"),
    }
}

#[tokio::test]
async fn test_downloader_get_download_path_valid() {
    let plugin = MockPlugin::new("test.dl");
    plugin.add_response(
        "get_download_path",
        Ok(serde_json::json!("/downloads/complete")),
    );

    let result = bridge::downloaders::get_download_path(&plugin).await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), Some("/downloads/complete".to_string()));
}

#[tokio::test]
async fn test_downloader_get_download_path_empty() {
    let plugin = MockPlugin::new("test.dl");
    plugin.add_response(
        "get_download_path",
        Ok(serde_json::json!("")), // empty path — invalid
    );

    let result = bridge::downloaders::get_download_path(&plugin).await;
    assert!(result.is_err());
    match result.unwrap_err() {
        BridgeError::Validation(msg) => assert!(msg.contains("empty")),
        _ => panic!("Expected Validation error"),
    }
}

#[tokio::test]
async fn test_downloader_test_connection_valid() {
    let plugin = MockPlugin::new("test.dl");
    plugin.add_response(
        "test_connection",
        Ok(serde_json::json!("Connected successfully")),
    );

    let result = bridge::downloaders::test_connection(&plugin).await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), "Connected successfully");
}

#[tokio::test]
async fn test_downloader_test_connection_empty() {
    let plugin = MockPlugin::new("test.dl");
    plugin.add_response(
        "test_connection",
        Ok(serde_json::json!("")), // empty — invalid
    );

    let result = bridge::downloaders::test_connection(&plugin).await;
    assert!(result.is_err());
    match result.unwrap_err() {
        BridgeError::Validation(_) => {} // expected
        _ => panic!("Expected Validation error"),
    }
}

// Bridge: notifier tests

#[tokio::test]
async fn test_notifier_notify_with_valid_context() {
    let plugin = MockPlugin::new("test.notifier");
    plugin.add_response("notify", Ok(serde_json::json!(null)));

    let ctx = NotifierContext::new()
        .with_series("Test Series")
        .with_release_title("Test.Release.720p")
        .with_episode(1, 1);

    let result = bridge::notifiers::notify(&plugin, &NotifierEvent::DownloadCompleted, &ctx).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_notifier_notify_missing_required_fields() {
    let plugin = MockPlugin::new("test.notifier");

    // Missing series_title for DownloadStarted event
    let ctx = NotifierContext::new().with_release_title("Test");

    let result = bridge::notifiers::notify(&plugin, &NotifierEvent::DownloadStarted, &ctx).await;
    assert!(result.is_err());
    match result.unwrap_err() {
        BridgeError::Validation(msg) => assert!(msg.contains("series_title")),
        _ => panic!("Expected Validation error"),
    }
}

#[tokio::test]
async fn test_notifier_notify_invalid_season() {
    let plugin = MockPlugin::new("test.notifier");
    plugin.add_response("notify", Ok(serde_json::json!(null)));

    let ctx = NotifierContext::new()
        .with_series("Test")
        .with_release_title("Test.Release")
        .with_episode(-5, 1); // negative season — invalid

    let result = bridge::notifiers::notify(&plugin, &NotifierEvent::DownloadCompleted, &ctx).await;
    // Should succeed (validation is at warn level, not reject)
    if let Err(ref e) = result {
        eprintln!("notify returned error: {:?}", e);
    }
    assert!(result.is_ok());
}

// SSoT alignment & edge case tests

#[test]
fn test_episode_metadata_season_above_max() {
    // SSoT: validate_season_number caps at 10000 — verify EpisodeMetadata delegates
    let ep = EpisodeMetadata {
        unique_id: "ep-1".to_string(),
        season: 10001,
        episode: 1,
        title: "Test".to_string(),
        description: None,
        runtime: None,
        image_url: None,
        meta_date: None,
    };
    let result = ep.validate();
    assert!(result.is_err(), "season > 10000 should be rejected");
}

#[test]
fn test_episode_metadata_episode_above_max() {
    // SSoT: validate_episode_number caps at MAX_EPISODE (10000)
    let ep = EpisodeMetadata {
        unique_id: "ep-1".to_string(),
        season: 1,
        episode: 10001,
        title: "Test".to_string(),
        description: None,
        runtime: None,
        image_url: None,
        meta_date: None,
    };
    let result = ep.validate();
    assert!(result.is_err(), "episode > 10000 should be rejected");
}

#[test]
fn test_episode_metadata_runtime_seconds_inline() {
    // Runtime is in seconds (TVDB format), not minutes — 500s ≈ 8m is valid
    let ep = EpisodeMetadata {
        unique_id: "ep-1".to_string(),
        season: 1,
        episode: 1,
        title: "Test".to_string(),
        description: None,
        runtime: Some(500),
        image_url: None,
        meta_date: None,
    };
    assert!(ep.validate().is_ok(), "500s runtime should be valid");
}

#[test]
fn test_season_metadata_season_above_max() {
    // SSoT: validate_season_number caps at 10000 — verify SeasonMetadata delegates
    let s = SeasonMetadata {
        season: 10001,
        episode_count: 1,
    };
    let result = s.validate();
    assert!(result.is_err(), "season > 10000 should be rejected");
}

#[test]
fn test_series_metadata_info_http_url_allowed() {
    // http:// URLs are also valid (not just https://)
    let info = SeriesMetadataInfo {
        name: "Test".to_string(),
        overview: None,
        original_country: None,
        aliases: HashMap::new(),
        image_url: Some("http://example.com/poster.jpg".to_string()),
    };
    assert!(info.validate().is_ok());
}

#[test]
fn test_series_metadata_info_name_too_long() {
    let info = SeriesMetadataInfo {
        name: "x".repeat(10001),
        overview: None,
        original_country: None,
        aliases: HashMap::new(),
        image_url: None,
    };
    let result = info.validate();
    assert!(result.is_err(), "10001-char name should be rejected");
}

#[test]
fn test_series_metadata_info_control_chars_rejected() {
    let info = SeriesMetadataInfo {
        name: "Test\u{0000}Series".to_string(),
        overview: None,
        original_country: None,
        aliases: HashMap::new(),
        image_url: None,
    };
    let result = info.validate();
    assert!(result.is_err(), "name with null byte should be rejected");
}

#[test]
fn test_episode_metadata_control_chars_rejected() {
    let ep = EpisodeMetadata {
        unique_id: "ep-1".to_string(),
        season: 1,
        episode: 1,
        title: "Normal\u{001b}Title".to_string(), // ESC control char
        description: None,
        runtime: None,
        image_url: None,
        meta_date: None,
    };
    let result = ep.validate();
    assert!(
        result.is_err(),
        "title with control char should be rejected"
    );
}

// Additional downloader bridge tests

#[tokio::test]
async fn test_downloader_add_download_valid() {
    let plugin = MockPlugin::new("test.dl");
    plugin.add_response("add_download", Ok(serde_json::json!(null)));

    let result = bridge::downloaders::add_download(
        &plugin,
        "https://example.com/test.torrent",
        "tv",
        Some("test-tag"),
        Some("Test Release"),
    )
    .await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_downloader_get_download_progress_valid() {
    let plugin = MockPlugin::new("test.dl");
    plugin.add_response(
        "get_download_progress",
        Ok(serde_json::json!(0.5)), // 50% — valid
    );

    let result = bridge::downloaders::get_download_progress(&plugin, "hash").await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), Some(0.5));
}

#[tokio::test]
async fn test_downloader_get_download_progress_null() {
    // Null response should map to None
    let plugin = MockPlugin::new("test.dl");
    plugin.add_response("get_download_progress", Ok(serde_json::json!(null)));

    let result = bridge::downloaders::get_download_progress(&plugin, "hash").await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), None);
}

#[tokio::test]
async fn test_downloader_get_download_status_null() {
    let plugin = MockPlugin::new("test.dl");
    plugin.add_response("get_download_status", Ok(serde_json::json!(null)));

    let result = bridge::downloaders::get_download_status(&plugin, "hash").await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), None);
}

#[tokio::test]
async fn test_downloader_get_download_failure_null_is_none() {
    let plugin = MockPlugin::new("test.dl");
    plugin.add_response("get_download_failure", Ok(serde_json::json!(null)));

    let result = bridge::downloaders::get_download_failure(&plugin, "hash").await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), None);
}

#[tokio::test]
async fn test_downloader_get_download_failure_returns_reason() {
    let plugin = MockPlugin::new("test.dl");
    plugin.add_response(
        "get_download_failure",
        Ok(serde_json::json!("torrent errored")),
    );

    let result = bridge::downloaders::get_download_failure(&plugin, "hash").await;
    assert_eq!(result.unwrap(), Some("torrent errored".to_string()));
}

#[tokio::test]
async fn test_downloader_get_download_failure_blank_is_none() {
    let plugin = MockPlugin::new("test.dl");
    plugin.add_response("get_download_failure", Ok(serde_json::json!("   ")));

    let result = bridge::downloaders::get_download_failure(&plugin, "hash").await;
    assert_eq!(result.unwrap(), None);
}

#[tokio::test]
async fn test_downloader_get_download_failure_is_trimmed() {
    let plugin = MockPlugin::new("test.dl");
    plugin.add_response(
        "get_download_failure",
        Ok(serde_json::json!("  torrent errored  ")),
    );

    let result = bridge::downloaders::get_download_failure(&plugin, "hash").await;
    assert_eq!(result.unwrap(), Some("torrent errored".to_string()));
}

#[tokio::test]
async fn test_downloader_get_download_failure_control_chars_replaced_not_dropped() {
    // A terminal failure must still be honored even if the plugin's reason is
    // cosmetically malformed — control chars are replaced, not rejected.
    let plugin = MockPlugin::new("test.dl");
    plugin.add_response(
        "get_download_failure",
        Ok(serde_json::json!("bad\u{001b}state\nhere")),
    );

    let result = bridge::downloaders::get_download_failure(&plugin, "hash").await;
    let reason = result.unwrap().expect("failure must be preserved");
    assert!(!reason.chars().any(char::is_control));
    assert!(reason.contains("bad state here") || reason.contains("bad state"));
}

#[tokio::test]
async fn test_downloader_get_download_failure_truncates_oversized_reason() {
    let plugin = MockPlugin::new("test.dl");
    plugin.add_response(
        "get_download_failure",
        Ok(serde_json::json!("x".repeat(MAX_FAILURE_REASON_LENGTH * 4))),
    );

    let result = bridge::downloaders::get_download_failure(&plugin, "hash").await;
    let reason = result.unwrap().expect("failure must be preserved");
    // Bounded to the max plus the ellipsis marker.
    assert!(reason.len() <= MAX_FAILURE_REASON_LENGTH + 3);
    assert!(reason.starts_with('x'));
}

#[tokio::test]
async fn test_downloader_get_download_status_too_long_is_rejected() {
    let plugin = MockPlugin::new("test.dl");
    plugin.add_response(
        "get_download_status",
        Ok(serde_json::json!("s".repeat(1000))),
    );

    let result = bridge::downloaders::get_download_status(&plugin, "hash").await;
    assert!(matches!(result, Err(BridgeError::Validation(_))));
}

#[tokio::test]
async fn test_downloader_get_download_status_control_chars_rejected() {
    let plugin = MockPlugin::new("test.dl");
    plugin.add_response(
        "get_download_status",
        Ok(serde_json::json!("down\u{0000}loading")),
    );

    let result = bridge::downloaders::get_download_status(&plugin, "hash").await;
    assert!(matches!(result, Err(BridgeError::Validation(_))));
}

#[tokio::test]
async fn test_downloader_test_connection_too_long_is_rejected() {
    let plugin = MockPlugin::new("test.dl");
    plugin.add_response("test_connection", Ok(serde_json::json!("m".repeat(10_000))));

    let result = bridge::downloaders::test_connection(&plugin).await;
    assert!(matches!(result, Err(BridgeError::Validation(_))));
}

#[tokio::test]
async fn test_downloader_test_connection_control_chars_rejected() {
    let plugin = MockPlugin::new("test.dl");
    plugin.add_response("test_connection", Ok(serde_json::json!("ok\u{001b}[31m")));

    let result = bridge::downloaders::test_connection(&plugin).await;
    assert!(matches!(result, Err(BridgeError::Validation(_))));
}

#[tokio::test]
async fn test_downloader_get_completed_downloads_empty() {
    let plugin = MockPlugin::new("test.dl");
    plugin.add_response("get_completed_downloads", Ok(serde_json::json!([])));

    let result = bridge::downloaders::get_completed_downloads(&plugin).await;
    assert!(result.is_ok());
    assert!(result.unwrap().is_empty());
}

#[tokio::test]
async fn test_downloader_get_download_path_null() {
    let plugin = MockPlugin::new("test.dl");
    plugin.add_response("get_download_path", Ok(serde_json::json!(null)));

    let result = bridge::downloaders::get_download_path(&plugin).await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), None);
}

#[tokio::test]
async fn test_downloader_get_organizer_path_valid() {
    let plugin = MockPlugin::new("test.dl");
    plugin.add_response("get_organizer_path", Ok(serde_json::json!("/media/tv")));

    let result = bridge::downloaders::get_organizer_path(&plugin).await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), Some("/media/tv".to_string()));
}

#[tokio::test]
async fn test_downloader_pause_resume_delete_retry() {
    let plugin = MockPlugin::new("test.dl");
    plugin.add_response("pause_download", Ok(serde_json::json!(null)));
    plugin.add_response("resume_download", Ok(serde_json::json!(null)));
    plugin.add_response("delete_download", Ok(serde_json::json!(null)));
    plugin.add_response("complete_download", Ok(serde_json::json!(null)));
    plugin.add_response("retry", Ok(serde_json::json!(null)));

    assert!(
        bridge::downloaders::pause_download(&plugin, "hash")
            .await
            .is_ok()
    );
    assert!(
        bridge::downloaders::resume_download(&plugin, "hash")
            .await
            .is_ok()
    );
    assert!(
        bridge::downloaders::delete_download(&plugin, "hash", false)
            .await
            .is_ok()
    );
    assert!(
        bridge::downloaders::complete_download(&plugin, "hash")
            .await
            .is_ok()
    );
    assert!(bridge::downloaders::retry(&plugin, "hash").await.is_ok());
}

#[tokio::test]
async fn test_downloader_get_download_id_by_name_null() {
    let plugin = MockPlugin::new("test.dl");
    plugin.add_response("get_download_id_by_name", Ok(serde_json::json!(null)));

    let result = bridge::downloaders::get_download_id_by_name(&plugin, "Some Torrent").await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), None);
}

#[tokio::test]
async fn test_downloader_get_download_content_path_valid() {
    let plugin = MockPlugin::new("test.dl");
    plugin.add_response(
        "get_download_content_path",
        Ok(serde_json::json!("/downloads/complete/Some.Show.S01E01")),
    );

    let result = bridge::downloaders::get_download_content_path(&plugin, "hash").await;
    assert!(result.is_ok());
    assert_eq!(
        result.unwrap(),
        "/downloads/complete/Some.Show.S01E01".to_string()
    );
}
