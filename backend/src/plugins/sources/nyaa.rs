// Nyaa.si Torrent Source
//
// RSS-based torrent indexer for Nyaa.si, usable for both automatic RSS polling
// (fetch_entries) and interactive search.
//
// quick_xml is used directly rather than reqwest's XML support: Nyaa's feed uses
// custom XML namespaces (nyaa:seeders, nyaa:infoHash, ...) which quick_xml's serde
// integration maps to struct fields via #[serde(rename)] (reqwest wraps quick_xml
// anyway, so this skips a layer).
//
// Nyaa has no JSON API — everything goes through the RSS feed with query params:
//   • Periodic sync: GET /?page=rss&c=1_2&f=0
//   • Search:        GET /?page=rss&c=1_2&f=0&q=search+terms
// Category (`c=`) and filter (`f=`) are per-instance config.
//
// auto_search chains aliases with Nyaa's `|` OR operator in the `q` parameter,
// because release groups name episodes differently ("... - S01E05", "... - 05",
// or absolute numbers). We emit "E05", a zero-padded absolute number (e.g. "105")
// and an unpadded one (e.g. "5") so Nyaa matches ANY of them.

use crate::models::media::MediaEntry;
use crate::plugins::PluginCallError;
use crate::plugins::PluginInstance;
use crate::plugins::extract::extract_str;
use crate::utils::describe_error_chain;
use anyhow::{Context, Result};
use async_trait::async_trait;
use jumbie_shared::formatting::{build_search_queries, extract_search_params};
use jumbie_shared::validation::plugin_data::validate_and_filter;
use plugin_sdk::query::SearchResponse;
use quick_xml::de::from_str;
use serde::{Deserialize, Serialize};
use url::Url;

// Nyaa-specific RSS types — private to this module. Nyaa's feed extends RSS 2.0
// with a custom `nyaa:` XML namespace for torrent metadata.

#[derive(Debug, Deserialize)]
struct NyaaRss {
    channel: NyaaChannel,
}

#[derive(Debug, Deserialize)]
struct NyaaChannel {
    item: Option<Vec<NyaaItem>>,
}

#[derive(Debug, Deserialize)]
struct NyaaItem {
    pub title: String,
    pub link: String,
    pub guid: Option<NyaaGuid>,
    #[serde(rename = "pubDate")]
    pub meta_date: Option<String>,
    // Nyaa-namespaced extensions
    #[serde(rename = "seeders", default)]
    pub seeders: Option<u32>,
    #[serde(rename = "leechers", default)]
    pub leechers: Option<u32>,
    #[serde(rename = "infoHash", default)]
    pub info_hash: Option<String>,
    #[serde(rename = "categoryId", default)]
    pub category_id: Option<String>,
    #[serde(rename = "size", default)]
    pub size: Option<String>,
}

#[derive(Debug, Deserialize)]
struct NyaaGuid {
    #[serde(rename = "$value")]
    pub value: String,
}

// Configuration for a Nyaa source instance. `category`/`filter` map to Nyaa's URL
// params `c=` (e.g. "1_2" = Anime English-translated) and `f=` (e.g. "0" = no
// filter). The three boolean toggles let users enable polling, automatic search,
// and manual search independently.

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct NyaaConfig {
    #[serde(default = "default_nyaa_name")]
    pub name: String,
    #[serde(default = "default_nyaa_base_url")]
    pub base_url: String,
    #[serde(default = "default_nyaa_category")]
    pub category: String,
    #[serde(default = "default_nyaa_filter")]
    pub filter: String,
    #[serde(default = "default_true")]
    pub enable_polling: bool,
    #[serde(default = "default_true")]
    pub enable_automatic_search: bool,
    #[serde(default = "default_true")]
    pub enable_manual_search: bool,
}

fn default_nyaa_name() -> String {
    "Nyaa".to_string()
}
fn default_true() -> bool {
    true
}
fn default_nyaa_base_url() -> String {
    "https://nyaa.si".to_string()
}
fn default_nyaa_category() -> String {
    "1_2".to_string()
}
fn default_nyaa_filter() -> String {
    "0".to_string()
}

impl Default for NyaaConfig {
    fn default() -> Self {
        Self {
            name: default_nyaa_name(),
            base_url: default_nyaa_base_url(),
            category: default_nyaa_category(),
            filter: default_nyaa_filter(),
            enable_polling: default_true(),
            enable_automatic_search: default_true(),
            enable_manual_search: default_true(),
        }
    }
}

pub struct NyaaSource {
    instance_id: String,
    priority: i32,
    // Mutable config (in-placeset_config); reads go through `cfg()`.
    config: std::sync::RwLock<NyaaConfig>,
    client: reqwest::Client,
}

/// Independent version of the built-in Nyaa plugin.
/// Bump when this plugin's behavior changes — it versions the plugin, not the app.
pub const PLUGIN_VERSION: &str = "1.0.0";

impl NyaaSource {
    pub fn plugin_info() -> jumbie_shared::plugin::PluginTypeInfo {
        jumbie_shared::plugin::PluginTypeInfo {
            capabilities: vec![
                jumbie_shared::plugin::Capability::FeedProvider,
                jumbie_shared::plugin::Capability::Polling,
                jumbie_shared::plugin::Capability::ManualSearch,
                jumbie_shared::plugin::Capability::AutomaticSearch,
            ],
            display_name: "nyaa".to_string(),
            version: PLUGIN_VERSION.to_string(),
            author: jumbie_shared::plugin::JUMBIE_AUTHOR.to_string(),
            description: "Nyaa.si Torrent Indexer".to_string(),
            series_identifier_label: None,
            series_identifier_placeholder: None,
            supported_protocols: None,
            rate_limit: Some(jumbie_shared::plugin::RateLimit {
                requests_per_minute: 30,
                burst: 5,
            }),
            supports_test: true,
        }
    }

    pub fn config_schema() -> serde_json::Value {
        serde_json::json!({
            "$schema": "http://json-schema.org/draft-07/schema#",
            "title": "NyaaConfig",
            "type": "object",
            "required": ["base_url"],
            "properties": {
                "name": { "type": "string", "title": "Name", "default": "Nyaa", "order": -25 },
                "base_url": {
                    "type": "string",
                    "title": "Base URL*",
                    "default": "https://nyaa.si",
                    "placeholder": "https://nyaa.si"
                },
                "category": {
                    "type": "string",
                    "title": "Category",
                    "default": "1_2",
                    "x-enum-options": [
                        { "value": "0_0", "label": "All categories" },
                        { "group": "Anime", "items": [
                            { "value": "1_0", "label": "Anime (all)" },
                            { "value": "1_1", "label": "Anime Music Video" },
                            { "value": "1_2", "label": "English-translated" },
                            { "value": "1_3", "label": "Non-English-translated" },
                            { "value": "1_4", "label": "Raw" }
                        ]},
                        { "group": "Audio", "items": [
                            { "value": "2_0", "label": "Audio (all)" },
                            { "value": "2_1", "label": "Lossless" },
                            { "value": "2_2", "label": "Lossy" }
                        ]},
                        { "group": "Literature", "items": [
                            { "value": "3_0", "label": "Literature (all)" },
                            { "value": "3_1", "label": "English-translated" },
                            { "value": "3_2", "label": "Non-English-translated" },
                            { "value": "3_3", "label": "Raw" }
                        ]},
                        { "group": "Live Action", "items": [
                            { "value": "4_0", "label": "Live Action (all)" },
                            { "value": "4_1", "label": "Idol/Promotional Video" },
                            { "value": "4_2", "label": "Non-English-translated" },
                            { "value": "4_3", "label": "Raw" }
                        ]},
                        { "group": "Pictures", "items": [
                            { "value": "5_0", "label": "Pictures (all)" },
                            { "value": "5_1", "label": "Graphics" },
                            { "value": "5_2", "label": "Photos" }
                        ]},
                        { "group": "Software", "items": [
                            { "value": "6_0", "label": "Software (all)" },
                            { "value": "6_1", "label": "Applications" },
                            { "value": "6_2", "label": "Games" }
                        ]}
                    ]
                },
                "filter": {
                    "type": "string",
                    "title": "Filter",
                    "default": "0",
                    "enum": ["0", "1", "2"],
                    "enumNames": ["No filter", "No remakes", "Trusted only"]
                },
                "refresh_interval": {
                    "type": "integer",
                    "format": "duration",
                    "x-time-unit": "minutes",
                    "title": "Refresh Interval",
                    "description": "How often to check for new entries (e.g. 30m, 2h)",
                    "placeholder": "2hr 30min",
                    "default": 10,
                    "minimum": 5,
                    "order": 1001
                }
            }
        })
    }

    pub fn new(
        config: Option<NyaaConfig>,
        client: reqwest::Client,
        instance_id: String,
        priority: i32,
    ) -> Self {
        Self {
            instance_id,
            priority,
            config: std::sync::RwLock::new(config.unwrap_or_default()),
            client,
        }
    }

    /// Snapshot of the current config (SSoT: all config reads go through here
    /// so the std lock is never held across an await).
    fn cfg(&self) -> NyaaConfig {
        self.config.read().unwrap().clone()
    }

    // Build the search/RSS URL with the correct query parameters. `page=rss` is
    // required for XML output. The `url` crate handles query encoding (string
    // concatenation would break if base_url already contains params).
    fn build_search_url(&self, query: Option<&str>) -> Result<String> {
        let mut base_url = self.cfg().base_url;

        // Ensure page=rss is present.
        if !base_url.contains("page=rss") {
            if !base_url.ends_with('/') && !base_url.contains('?') {
                base_url.push('/');
            }
            if base_url.contains('?') {
                base_url.push_str("&page=rss");
            } else {
                base_url.push_str("?page=rss");
            }
        }

        let mut url = Url::parse(&base_url).context("Failed to parse Nyaa base URL")?;

        {
            let mut pairs = url.query_pairs_mut();
            let cfg = self.cfg();
            pairs.append_pair("c", &cfg.category);
            pairs.append_pair("f", &cfg.filter);

            if let Some(q) = query {
                // query_pairs_mut uses application/x-www-form-urlencoded encoding,
                // which replaces spaces with + (Nyaa's expected format).
                pairs.append_pair("q", q);
            }
        }

        Ok(url.to_string())
    }

    // Parse Nyaa's RSS XML into MediaEntry structs. Two link types are handled:
    // magnet: links become the magnet URI (no download_url); http(s) links become
    // download_url (a .torrent file), no magnet. Size comes from `parse_size_str`;
    // an unparseable size stays None (the system then asks before downloading).
    /// Tag all entries with this source's name (SSoT — the entry knows its origin).
    fn tag_entries(&self, entries: &mut [MediaEntry]) {
        let name = self.name();
        for entry in entries {
            entry.source.clone_from(&name);
        }
    }

    fn parse_entries(&self, content: &str) -> Result<Vec<MediaEntry>> {
        let rss: NyaaRss = from_str(content).context("Failed to parse Nyaa RSS XML")?;
        let mut entries = Vec::new();

        if let Some(items) = rss.channel.item {
            for item in items {
                let size_bytes = if let Some(s) = &item.size {
                    parse_size_str(s)
                } else {
                    None
                };

                let mut entry = MediaEntry {
                    title: item.title,
                    guid: item.guid.map(|g| g.value),
                    link: Some(item.link.clone()),
                    published: item.meta_date.as_ref().and_then(|d| {
                        chrono::DateTime::parse_from_rfc2822(d)
                            .ok()
                            .map(|dt| dt.with_timezone(&chrono::Utc))
                    }),
                    download_url: Some(item.link.clone()),
                    download_id: item.info_hash,
                    size: size_bytes,
                    seeders: item.seeders,
                    leechers: item.leechers,
                    category: item.category_id,
                    ..Default::default()
                };
                // Nyaa RSS doesn't have a dedicated submitter field — the release
                // group is embedded in the title (e.g. "[SubsGroup] Show - 01").
                entry.resolve_submitter();
                entries.push(entry);
            }
        }

        Ok(entries)
    }
}

// Nyaa reports sizes as "123.45 MiB" (space between value and unit). Only
// KiB/MiB/GiB/TiB are recognised; any other unit or malformed input returns None
// and callers handle the missing size gracefully.
fn parse_size_str(s: &str) -> Option<u64> {
    let parts: Vec<&str> = s.split_whitespace().collect();
    if parts.len() < 2 {
        return None;
    }

    let val: f64 = parts[0].parse().ok()?;
    let unit = parts[1];

    let multiplier = match unit {
        "KiB" => 1024.0,
        "MiB" => 1024.0 * 1024.0,
        "GiB" => 1024.0 * 1024.0 * 1024.0,
        "TiB" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        _ => 1.0,
    };

    Some((val * multiplier) as u64)
}

// Public accessors and the source operations (search / fetch_entries / auto_search).
impl NyaaSource {
    pub fn name(&self) -> String {
        self.cfg().name
    }

    pub fn url(&self) -> String {
        self.cfg().base_url
    }

    /// GET `url` and return the response body as text.
    ///
    /// Network failures (DNS, connect, TLS, timeout, truncated body) are
    /// classified as `PluginCallError::Transient` so the backend's failure
    /// policy retries them with backoff and reports them as transient rather
    /// than "unclassified" (plugin spec §8). The message carries the full
    /// `source()` chain so the root cause (e.g. a DNS failure hidden behind
    /// reqwest's generic "error sending request for url") reaches the logs.
    async fn get_text(&self, url: &str) -> Result<String> {
        let response = self.client.get(url).send().await.map_err(|e| {
            anyhow::Error::new(PluginCallError::Transient(format!(
                "Failed to send request to {} — {}",
                url,
                describe_error_chain(&e)
            )))
        })?;
        response.text().await.map_err(|e| {
            anyhow::Error::new(PluginCallError::Transient(format!(
                "Failed to read response body from {} — {}",
                url,
                describe_error_chain(&e)
            )))
        })
    }

    async fn search(&self, query: String) -> Result<Vec<MediaEntry>> {
        let url = self.build_search_url(Some(&query))?;
        tracing::debug!("Nyaa search URL: {}", url);
        let content = self.get_text(&url).await?;
        let mut entries = self.parse_entries(&content)?;
        self.tag_entries(&mut entries);
        Ok(entries)
    }

    async fn fetch_entries(&self) -> Result<Vec<MediaEntry>> {
        let url = self.build_search_url(None)?;
        let content = self.get_text(&url).await?;
        let mut entries = self.parse_entries(&content)?;
        self.tag_entries(&mut entries);
        Ok(entries)
    }

    async fn auto_search(
        &self,
        payload: &serde_json::Value,
    ) -> Result<(Vec<MediaEntry>, Vec<String>)> {
        let params = extract_search_params(payload);
        let queries = build_search_queries(&params);
        tracing::debug!(
            "Nyaa auto_search: series_title={}, aliases={:?}, queries={:?}",
            params.series_title,
            params.aliases,
            queries,
        );
        let mut entries = Vec::new();
        for q in &queries {
            entries.extend(self.search(q.clone()).await?);
        }
        self.tag_entries(&mut entries);
        Ok((entries, queries))
    }
}

#[async_trait]
impl PluginInstance for NyaaSource {
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
                    tracing::debug!("Nyaa search returned {} invalid entries", rejected.len());
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
                        "Nyaa fetch_entries returned {} invalid entries",
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
                        "Nyaa auto_search returned {} invalid entries",
                        rejected.len()
                    );
                }
                Ok(serde_json::to_value(SearchResponse::new(clean, queries))?)
            }
            _ => PluginInstance::call(self, method, params).await,
        }
    }

    async fn test_impl(&self) -> Result<()> {
        let base_url = self.cfg().base_url;
        let response =
            self.client.get(&base_url).send().await.map_err(|e| {
                anyhow::anyhow!("Failed to reach Nyaa source at {}: {}", base_url, e)
            })?;
        if !response.status().is_success() {
            anyhow::bail!(
                "Nyaa source at {} returned HTTP {}",
                base_url,
                response.status()
            );
        }
        Ok(())
    }

    async fn set_config(&self, config: serde_json::Value) -> Result<serde_json::Value> {
        // Parse-first; on error return Err with zero mutation (the manager
        // falls back to factory-replacement).
        let new: NyaaConfig = serde_json::from_value(config)
            .map_err(|e| anyhow::anyhow!("Invalid Nyaa configuration: {}", e))?;
        *self.config.write().unwrap() = new;
        tracing::info!(target: "source.nyaa", "Reconfigured Nyaa source in place");
        Ok(serde_json::Value::Bool(true))
    }
}

// Registration
impl NyaaSource {
    pub async fn register() {
        use crate::plugins::registry::InternalPluginRegistry;
        use std::sync::Arc;

        InternalPluginRegistry::register_full(
            "source.nyaa",
            |config, id, priority, _refresh_interval, global_cfg, _shutdown_token| {
                let cfg: crate::plugins::sources::nyaa::NyaaConfig =
                    serde_json::from_value(config)?;
                let client = crate::utils::http::create_client(&global_cfg);
                Ok(Arc::new(NyaaSource::new(Some(cfg), client, id, priority))
                    as Arc<dyn PluginInstance>)
            },
            Self::config_schema,
            Self::plugin_info,
        )
        .await;
    }
}
