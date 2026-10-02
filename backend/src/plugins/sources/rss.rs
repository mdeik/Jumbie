use crate::models::media::MediaEntry;
use crate::plugins::PluginInstance;
use crate::plugins::extract::extract_str;
use anyhow::{Context, Result};
use async_trait::async_trait;
use jumbie_shared::formatting::{build_search_queries, extract_search_params};
use jumbie_shared::validation::plugin_data::validate_and_filter;
use plugin_sdk::query::SearchResponse;
use quick_xml::de::from_str;
use serde::{Deserialize, Serialize};

// Minimal standard RSS 2.0 types — private to this module (each RSS source owns
// its fetching and parsing).
//
// quick_xml::de is used instead of a dedicated RSS parser (e.g. the `rss` crate)
// because it maps XML directly to serde structs, giving control over which fields
// we extract and letting us add non-standard extensions. We need only title, link,
// guid, pubDate, and enclosure (about 10% of RSS), and torrent feeds often embed a
// magnet URI in <link>, which we can test with `item.link.starts_with("magnet:")`
// rather than fighting a parser that assumes <link> is always a URL.

#[derive(Debug, Deserialize)]
pub(crate) struct BasicRss {
    pub(crate) channel: BasicChannel,
}

#[derive(Debug, Deserialize)]
pub(crate) struct BasicChannel {
    pub(crate) item: Option<Vec<BasicItem>>,
}

// `Option<Vec<BasicItem>>`: a missing <item> element (common in well-formed but
// empty feeds) deserializes to `None`, whereas an empty <channel/> yields
// `Some(vec![])` — letting us distinguish the two for debugging.
#[derive(Debug, Deserialize)]
pub(crate) struct BasicItem {
    pub title: String,
    pub link: String,
    pub guid: Option<BasicGuid>,
    #[serde(rename = "pubDate")]
    pub meta_date: Option<String>,
    /// Standard RSS enclosure — common in podcast/torrent feeds for size + URL.
    pub enclosure: Option<BasicEnclosure>,
}

// quick_xml::de uses `$value` for element text content (<guid>abc</guid> → "abc")
// and `@attr` for attributes (<enclosure url="..."/>), unlike JSON serde.
#[derive(Debug, Deserialize)]
pub(crate) struct BasicGuid {
    #[serde(rename = "$value")]
    pub value: String,
}

// Only the enclosure `length` is stored, not its URL: the download URL comes from
// <link>/<guid>, while `length` is the only RSS 2.0 field giving file size without
// downloading (used for scoring and storage estimation).
#[derive(Debug, Deserialize)]
pub(crate) struct BasicEnclosure {
    #[serde(rename = "@length")]
    pub length: Option<u64>,
}

// Config

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BasicRssConfig {
    #[serde(default = "default_name")]
    pub name: String,
    pub url: String,
    #[serde(default = "default_true")]
    pub enable_polling: bool,
    #[serde(default = "default_true")]
    pub enable_automatic_search: bool,
}

fn default_name() -> String {
    "RSS".to_string()
}
fn default_true() -> bool {
    true
}

// Source

pub struct BasicRssSource {
    instance_id: String,
    priority: i32,
    // Mutable config (in-placeset_config); reads go through `cfg()`.
    config: std::sync::RwLock<BasicRssConfig>,
    client: reqwest::Client,
}

/// Independent version of the built-in RSS plugin.
/// Bump when this plugin's behavior changes — it versions the plugin, not the app.
pub const PLUGIN_VERSION: &str = "1.0.0";

impl BasicRssSource {
    pub fn plugin_info() -> jumbie_shared::plugin::PluginTypeInfo {
        jumbie_shared::plugin::PluginTypeInfo {
            capabilities: vec![
                jumbie_shared::plugin::Capability::FeedProvider,
                jumbie_shared::plugin::Capability::Polling,
                jumbie_shared::plugin::Capability::AutomaticSearch,
            ],
            display_name: "basic_rss".to_string(),
            version: PLUGIN_VERSION.to_string(),
            author: jumbie_shared::plugin::JUMBIE_AUTHOR.to_string(),
            description: "Basic RSS Feed".to_string(),
            series_identifier_label: None,
            series_identifier_placeholder: None,
            supported_protocols: None,
            rate_limit: Some(jumbie_shared::plugin::RateLimit {
                requests_per_minute: 10,
                burst: 2,
            }),
            supports_test: false,
        }
    }

    pub fn config_schema() -> serde_json::Value {
        serde_json::json!({
            "$schema": "http://json-schema.org/draft-07/schema#",
            "title": "BasicRssConfig",
            "type": "object",
            "required": ["url"],
            "properties": {
                "name": { "type": "string", "title": "Name", "default": "RSS", "order": -25 },
                "url": {
                    "type": "string",
                    "title": "RSS URL*",
                    "placeholder": "https://example.com/feed.xml"
                },
                "refresh_interval": {
                    "type": "integer",
                    "title": "Refresh Interval (minutes)",
                    "description": "How often to check for new entries",
                    "default": 10,
                    "minimum": 5,
                    "order": 1001
                }
            }
        })
    }

    pub fn new(
        config: BasicRssConfig,
        client: reqwest::Client,
        instance_id: String,
        priority: i32,
    ) -> Self {
        Self {
            instance_id,
            priority,
            config: std::sync::RwLock::new(config),
            client,
        }
    }

    /// Snapshot of the current config (SSoT: all config reads go through here
    /// so the std lock is never held across an await).
    fn cfg(&self) -> BasicRssConfig {
        self.config.read().unwrap().clone()
    }

    fn parse_entries(&self, content: &str) -> Result<Vec<MediaEntry>> {
        parse_rss_entries(content)
    }

    /// Tag all entries with this source's name.
    fn tag_entries(&self, entries: &mut [MediaEntry]) {
        let name = self.name();
        for entry in entries {
            entry.source.clone_from(&name);
        }
    }

    pub fn name(&self) -> String {
        self.cfg().name
    }

    pub fn url(&self) -> String {
        self.cfg().url
    }
}

#[async_trait]
impl PluginInstance for BasicRssSource {
    fn instance_id(&self) -> &str {
        &self.instance_id
    }

    fn supported_protocols(&self) -> Option<&[String]> {
        None
    }

    fn priority(&self) -> i32 {
        self.priority
    }

    fn plugin_info(&self) -> jumbie_shared::plugin::PluginTypeInfo {
        Self::plugin_info()
    }

    async fn handle_custom_method(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<serde_json::Value> {
        match method {
            "search" => {
                let query = extract_str(&params, "query")?;
                let entries = self.search(query.clone()).await?;
                let (clean, rejected) = validate_and_filter(entries);
                if !rejected.is_empty() {
                    tracing::debug!(
                        "BasicRss search returned {} invalid entries",
                        rejected.len()
                    );
                }
                Ok(serde_json::to_value(SearchResponse::from_manual(
                    clean, query,
                ))?)
            }
            "fetch_entries" => {
                let entries = self.fetch_entries().await?;
                let (clean, rejected) = validate_and_filter(entries);
                if !rejected.is_empty() {
                    tracing::debug!(
                        "BasicRss fetch_entries returned {} invalid entries",
                        rejected.len()
                    );
                }
                Ok(serde_json::to_value(clean)?)
            }
            "auto_search" => {
                let payload = params.unwrap_or_default();
                let (entries, queries) = self.auto_search(&payload).await?;
                let (clean, rejected) = validate_and_filter(entries);
                if !rejected.is_empty() {
                    tracing::debug!(
                        "BasicRss auto_search returned {} invalid entries",
                        rejected.len()
                    );
                }
                Ok(serde_json::to_value(SearchResponse::new(clean, queries))?)
            }
            _ => PluginInstance::call(self, method, params).await,
        }
    }

    async fn set_config(&self, config: serde_json::Value) -> Result<serde_json::Value> {
        // Parse-first; on error return Err with zero mutation (the manager
        // falls back to factory-replacement).
        let new: BasicRssConfig = serde_json::from_value(config)
            .map_err(|e| anyhow::anyhow!("Invalid Basic RSS configuration: {}", e))?;
        if new.url.trim().is_empty() {
            return Err(anyhow::anyhow!("Basic RSS url cannot be empty"));
        }
        *self.config.write().unwrap() = new;
        tracing::info!(target: "source.basic_rss", "Reconfigured Basic RSS source in place");
        Ok(serde_json::Value::Bool(true))
    }
}

impl BasicRssSource {
    async fn search(&self, _query: String) -> Result<Vec<MediaEntry>> {
        // Standard RSS feeds don't support search queries
        Ok(vec![])
    }

    async fn fetch_entries(&self) -> Result<Vec<MediaEntry>> {
        let content = self
            .client
            .get(&self.cfg().url)
            .send()
            .await?
            .text()
            .await?;
        let mut entries = self.parse_entries(&content)?;
        self.tag_entries(&mut entries);
        Ok(entries)
    }

    async fn auto_search(
        &self,
        payload: &serde_json::Value,
    ) -> Result<(Vec<MediaEntry>, Vec<String>)> {
        let queries = build_search_queries(&extract_search_params(payload));
        let mut entries = Vec::new();
        for q in &queries {
            entries.extend(self.search(q.clone()).await?);
        }
        Ok((entries, queries))
    }
}

/// Parse RSS 2.0 XML content into a list of `MediaEntry` values.
fn parse_rss_entries(content: &str) -> Result<Vec<MediaEntry>> {
    let rss: BasicRss = from_str(content).context("Failed to parse RSS XML")?;
    let mut entries = Vec::new();

    if let Some(items) = rss.channel.item {
        for item in items {
            // The download URL is the link from the RSS item — it may be a magnet
            // URI, a .torrent URL, or any other protocol the downloader supports.
            // Source plugins are responsible for providing the download_id separately.
            let download_url: Option<String> = Some(item.link.clone());

            let size = item.enclosure.as_ref().and_then(|e| e.length);

            let mut entry = MediaEntry {
                title: item.title,
                // GUID is the canonical unique id per RSS spec; when a feed omits
                // it, fall back to <link> (usually unique enough for dedup).
                // `or_else` is lazy, so the link isn't cloned when guid exists.
                guid: item
                    .guid
                    .map(|g| g.value)
                    .or_else(|| Some(item.link.clone())),
                link: Some(item.link),
                // pubDate is always RFC 2822 (e.g. "Mon, 01 Jan 2024 12:00:00 GMT");
                // normalize to UTC so the rest of the system works in one timezone.
                published: item.meta_date.as_ref().and_then(|d| {
                    chrono::DateTime::parse_from_rfc2822(d)
                        .ok()
                        .map(|dt| dt.with_timezone(&chrono::Utc))
                }),
                download_url,
                download_id: None,
                size,
                description: None,
                seeders: None,
                leechers: None,
                ..Default::default()
            };
            // Standard RSS doesn't have a dedicated submitter field — the release
            // group is embedded in the title (e.g. "[Group] Show - 01" or "Show-GRP").
            entry.resolve_submitter();
            entries.push(entry);
        }
    }

    Ok(entries)
}

// Registration
impl BasicRssSource {
    pub async fn register() {
        use crate::plugins::registry::InternalPluginRegistry;
        use std::sync::Arc;

        InternalPluginRegistry::register_full(
            "source.basic_rss",
            |config, id, priority, _refresh_interval, global_cfg, _shutdown_token| {
                let cfg: crate::plugins::sources::rss::BasicRssConfig =
                    serde_json::from_value(config)?;
                let client = crate::utils::http::create_client(&global_cfg);
                Ok(Arc::new(BasicRssSource::new(cfg, client, id, priority))
                    as Arc<dyn PluginInstance>)
            },
            Self::config_schema,
            Self::plugin_info,
        )
        .await;
    }
}
