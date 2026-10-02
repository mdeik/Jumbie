use anyhow::Result;
use async_trait::async_trait;
use plugin_sdk::query::SearchResponse;
use plugin_sdk::{CallContext, PluginError, PluginHandler, PluginServer, PluginTypeInfo};
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Configuration schema for the dummy plugin.
/// schemars will auto-generate a JSON Schema from this struct.
///
/// This is the plugin's "specified fields/keys": every instance of this type
/// is just a config blob matching this shape. The handler receives it on every
/// call via `CallContext::config` — there is no per-instance plugin object and
/// no `reconfigure` logic.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DummyConfig {
    /// A human-readable name for this plugin instance
    pub name: String,

    /// Whether this plugin instance is active
    pub enabled: bool,

    /// An example API key field (will be rendered as a password input)
    pub api_key: String,

    /// Base URL for the dummy service
    pub base_url: String,

    /// Request timeout in seconds
    pub timeout_secs: u32,
}

impl Default for DummyConfig {
    fn default() -> Self {
        Self {
            name: "Dummy Plugin".to_string(),
            enabled: true,
            api_key: String::new(),
            base_url: "https://api.example.com".to_string(),
            timeout_secs: 30,
        }
    }
}

/// ONE handler object per process. Instances are just configs — the handler
/// reads `ctx.config` on every call and memoizes derived state in `ctx.cache`
/// (cleared automatically when the host pushes a new config).
pub struct DummyPlugin;

impl DummyPlugin {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl PluginHandler for DummyPlugin {
    async fn get_info(&self) -> PluginTypeInfo {
        PluginTypeInfo {
            display_name: "Dummy Rust".to_string(),
            version: "0.1.0".to_string(),
            author: "Jumbie".to_string(),
            description: "A Rust-based dummy plugin for testing the JSON-RPC plugin system"
                .to_string(),
            // Declare as a metadata provider so the host routes it correctly.
            // Also demonstrates how to declare multiple capabilities.
            capabilities: vec![
                plugin_sdk::traits::Capability::MetadataProviderNormal,
                plugin_sdk::traits::Capability::FetchSeriesTitle,
            ],
            supported_protocols: None,
            series_identifier_label: Some("Series ID".to_string()),
            series_identifier_placeholder: Some("e.g. 121361".to_string()),
            rate_limit: Some(plugin_sdk::traits::RateLimit {
                requests_per_minute: 60,
                burst: 5,
            }),
            supports_test: true,
        }
    }

    async fn get_config_schema(&self) -> Value {
        let schema = schema_for!(DummyConfig);
        serde_json::to_value(schema).unwrap_or(Value::Null)
    }

    async fn validate_config(&self, config: Value) -> Result<(), Vec<String>> {
        match serde_json::from_value::<DummyConfig>(config) {
            Ok(cfg) => {
                let mut errors = Vec::new();
                if cfg.name.trim().is_empty() {
                    errors.push("'name' must not be empty".to_string());
                }
                if cfg.api_key.len() < 5 {
                    errors.push("'api_key' must be at least 5 characters".to_string());
                }
                if !cfg.base_url.starts_with("http") {
                    errors.push("'base_url' must start with http or https".to_string());
                }
                if errors.is_empty() {
                    Ok(())
                } else {
                    Err(errors)
                }
            }
            Err(e) => Err(vec![format!("Invalid config structure: {}", e)]),
        }
    }

    /// Type-level health: called once per process by the host's probe.
    async fn health_check(&self) -> Result<()> {
        Ok(())
    }

    async fn handle(&self, ctx: CallContext<'_>) -> Result<Value, anyhow::Error> {
        // The instance's config is INPUT — parse it per call. Derived state
        // (tokens, clients) would be memoized in ctx.cache keyed by the config
        // fields that affect it; the cache clears itself on config change.
        let _cfg: DummyConfig = serde_json::from_value(ctx.config.clone())?;

        match ctx.method {
            // ── MetadataProvider methods ────────────────────────────────────
            //
            // Each returns a JSON value matching the shapes documented in the
            // plugin specification (SeriesMetadata, SeriesMetadataInfo, etc.).
            "fetch_series_metadata" => {
                // Params: { "id": string, "language"?: string }
                let _series_id = ctx
                    .params
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                Ok(serde_json::json!({
                    "episodes": [{
                        "unique_id": "1",
                        "season": 1,
                        "episode": 1,
                        "title": "Episode 1",
                        "description": null,
                        "runtime": null,
                        "image_url": null,
                        "meta_date": null
                    }],
                    "seasons": [{ "season": 1, "episode_count": 12 }]
                }))
            }
            "fetch_series_info" => {
                Ok(serde_json::json!({
                    "name": "Dummy Series",
                    "overview": "A test series",
                    "original_country": null,
                    "aliases": {}
                }))
            }
            "fetch_series_aliases" => Ok(serde_json::json!({})),
            "get_updated_series" => Ok(serde_json::json!([])),
            "uses_absolute_episode_numbering" => Ok(serde_json::json!(false)),

            // ── Also handle auto_search (FeedProvider-style) for
            //    cross-reference — the server dispatches any method
            //    name, so a plugin can support methods from multiple
            //    capability categories.
            "auto_search" => {
                // Params: { series_title, season, episodes, aliases, keys }
                // (`keys` is one rendered search key per episode, aligned with `episodes`.)
                let title = ctx
                    .params
                    .get("series_title")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let season = ctx.params.get("season").and_then(|v| v.as_i64()).unwrap_or(1);
                let query = format!("Dummy {} S{:02}", title, season);
                let now = chrono::Utc::now().to_rfc3339();

                // Return the `{ entries, queries }` envelope; the backend logs the
                // reported `queries`.
                let entries = serde_json::json!([{
                    "title": query,
                    "source": "dummy",
                    "seeders": 0,
                    "size": 0,
                    "published": now,
                    "link": "",
                    "magnet": "",
                    "info_hash": null
                }]);
                Ok(serde_json::to_value(SearchResponse::new(entries, vec![query]))?)
            }
            // Signal "unsupported method" structurally so the host treats it as a
            // deliberate non-implementation (skipped quietly) rather than a
            // transient failure it retries with backoff.
            _ => Err(PluginError::MethodNotSupported(format!(
                "Method '{}' not implemented by dummy plugin",
                ctx.method
            ))
            .into()),
        }
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .init();

    tracing::info!("Dummy Rust plugin starting...");
    let plugin = DummyPlugin::new();
    let info = plugin.get_info().await;
    let server = PluginServer::new(info, Box::new(plugin));
    server.run().await;
}
