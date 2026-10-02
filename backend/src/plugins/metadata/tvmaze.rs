// TVMaze Metadata Provider
//
// Fetches episode metadata from https://api.tvmaze.com. Uses
// `/shows/{id}/episodes`, which returns ALL episodes in one JSON array (no
// pagination needed — even long-running shows have < 1000 episodes); the result
// is cached in the DB after the first fetch.
//
// Episode summaries contain HTML tags (<p>, <b>, ...). These are stripped by
// simple string replacement rather than a full HTML parser: TVMaze uses only a
// few known tags in predictable patterns, a parser would add a heavy dependency,
// and a format change degrades gracefully (tags appear in the description rather
// than breaking).
//
// Each episode's globally-unique TVMaze integer ID becomes our `unique_id`
// (string), allowing cross-referencing from other sources or future exports.
//
// The free API has no documented per-minute limit but returns 429 under load. We
// surface that as a descriptive error; the caller (metadata fetch loop) backs off
// and the episode is retried on the next sync cycle.

use super::{EpisodeMetadata, SeasonMetadata, SeriesMetadata};
use crate::datetime::UtcDateTime;
use crate::plugins::PluginInstance;
use crate::plugins::extract::{extract_bool_opt, extract_str};
use crate::plugins::registry::InternalPluginRegistry;
use crate::utils::describe_error_chain;
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TvMazeSettings {
    #[serde(default = "default_tvmaze_name")]
    pub name: String,
}

fn default_tvmaze_name() -> String {
    "TVmaze".to_string()
}

pub struct TvMazePlugin {
    instance_id: String,
    // Mutable config (in-placeset_config); reads go through `cfg()`.
    config: std::sync::RwLock<TvMazeSettings>,
    client: reqwest::Client,
    plugin_priority: i32,
}

/// Independent version of the built-in TVMaze plugin.
/// Bump when this plugin's behavior changes — it versions the plugin, not the app.
pub const PLUGIN_VERSION: &str = "1.0.0";

impl TvMazePlugin {
    /// SSoT: Static metadata for this provider. Also registered as the
    /// registry-level info so the two can never diverge.
    pub fn plugin_info() -> jumbie_shared::plugin::PluginTypeInfo {
        jumbie_shared::plugin::PluginTypeInfo {
            display_name: "tvmaze".to_string(),
            version: PLUGIN_VERSION.to_string(),
            author: jumbie_shared::plugin::JUMBIE_AUTHOR.to_string(),
            description: "TVMaze Metadata Provider".to_string(),
            capabilities: vec![
                jumbie_shared::plugin::Capability::MetadataProviderNormal,
                // Supports periodic background refresh (gated by the instance's
                // `enable_polling` toggle). See `plugins::capabilities`.
                jumbie_shared::plugin::Capability::Polling,
                jumbie_shared::plugin::Capability::FetchSeriesTitle,
                jumbie_shared::plugin::Capability::FetchSeriesAliases,
            ],
            series_identifier_label: Some("TVMaze ID".to_string()),
            series_identifier_placeholder: Some("34292".to_string()),
            supported_protocols: None,
            rate_limit: None,
            supports_test: true,
        }
    }

    pub fn config_schema() -> serde_json::Value {
        serde_json::json!({
            "$schema": "http://json-schema.org/draft-07/schema#",
            "title": "TVMazeSettings",
            "type": "object",
            "properties": {
                "name": { "type": "string", "title": "Name", "default": "TVmaze", "order": -25 }
            }
        })
    }

    pub fn new(
        config: &jumbie_shared::config::Config,
        settings: TvMazeSettings,
        instance_id: String,
        priority: i32,
    ) -> Self {
        let client = crate::utils::http::create_client(config);
        Self {
            instance_id,
            config: std::sync::RwLock::new(settings),
            client,
            plugin_priority: priority,
        }
    }

    /// Snapshot of the current settings (SSoT: all config reads go through
    /// here so the std lock is never held across an await).
    fn cfg(&self) -> TvMazeSettings {
        self.config.read().unwrap().clone()
    }
}

#[async_trait]
impl PluginInstance for TvMazePlugin {
    fn instance_id(&self) -> &str {
        &self.instance_id
    }

    fn plugin_info(&self) -> jumbie_shared::plugin::PluginTypeInfo {
        let mut info = Self::plugin_info();
        info.display_name = self.cfg().name;
        info
    }

    fn priority(&self) -> i32 {
        self.plugin_priority
    }

    fn supported_protocols(&self) -> Option<&[String]> {
        None
    }

    async fn handle_custom_method(&self, method: &str, params: Option<Value>) -> Result<Value> {
        match method {
            "fetch_series_metadata" => {
                let id = extract_str(&params, "id")?;
                let absolute = extract_bool_opt(&params, "absolute_numbering", false);
                let (episodes_and_seasons, series_info) =
                    self.fetch_series_metadata_impl(&id, absolute).await?;
                Ok(serde_json::json!({
                    "episodes_and_seasons": episodes_and_seasons,
                    "series_info": series_info,
                }))
            }
            "get_updated_series" => {
                let updated = self
                    .get_updated_series_impl(chrono::Duration::days(1))
                    .await?;
                Ok(serde_json::to_value(updated)?)
            }
            "fetch_series_info" => {
                let id = extract_str(&params, "id")?;
                let info = self.fetch_series_info_impl(&id).await?;
                Ok(serde_json::to_value(info)?)
            }
            "fetch_series_aliases" => {
                let id = extract_str(&params, "id")?;
                let aliases = self.fetch_series_aliases_impl(&id).await?;
                Ok(serde_json::to_value(aliases)?)
            }
            "uses_absolute_episode_numbering" => {
                Ok(serde_json::json!(false)) // TVMaze doesn't use absolute
            }
            _ => PluginInstance::call(self, method, params).await,
        }
    }

    async fn test_impl(&self) -> Result<()> {
        let url = "https://api.tvmaze.com/shows/1";
        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| anyhow!("Failed to reach TVMaze: {}", describe_error_chain(&e)))?;
        if !response.status().is_success() {
            anyhow::bail!("TVMaze returned HTTP {}", response.status());
        }
        Ok(())
    }

    async fn set_config(&self, config: Value) -> Result<Value> {
        // Parse-first; on error return Err with zero mutation (the manager
        // falls back to factory-replacement).
        let new: TvMazeSettings = serde_json::from_value(config)
            .map_err(|e| anyhow!("Invalid TVMaze configuration: {}", e))?;
        *self.config.write().unwrap() = new;
        tracing::info!(target: "metadata::tvmaze", "Reconfigured TVMaze plugin in place");
        Ok(Value::Bool(true))
    }
}

// Internal implementation methods
impl TvMazePlugin {
    async fn fetch_series_metadata_impl(
        &self,
        plugin_series_id: &str,
        _absolute_numbering: bool,
    ) -> Result<(SeriesMetadata, Option<super::SeriesMetadataInfo>)> {
        let url = format!("https://api.tvmaze.com/shows/{}/episodes", plugin_series_id);
        tracing::debug!("Fetching TVMaze data from {}", url);

        // Distinguish network errors (transient, retryable) from HTTP errors.
        let response = match self.client.get(&url).send().await {
            Ok(resp) => resp,
            Err(e) => {
                return Err(anyhow!(
                    "Failed to reach TVMaze: {}",
                    describe_error_chain(&e)
                ));
            }
        };

        // 429 gets a descriptive message the caller can recognize for backoff;
        // a generic "status 429" would be less actionable.
        if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(anyhow!(
                "TVMaze rate limit exceeded (429). Please wait a few seconds and try again."
            ));
        }

        if !response.status().is_success() {
            return Err(anyhow!("TVMaze returned HTTP {}", response.status()));
        }

        let json: serde_json::Value = response.json().await.map_err(|e| {
            anyhow!(
                "Failed to parse TVMaze response: {}",
                describe_error_chain(&e)
            )
        })?;

        let arr = json
            .as_array()
            .ok_or_else(|| anyhow!("TVMaze response was not an array"))?;

        let mut episodes = Vec::new();
        let mut season_counts: HashMap<i32, i32> = HashMap::new();

        // Extract the series title from the first episode's `_links.show.name`.
        // TVMaze includes this in every episode object, so we grab it once.
        let series_title: Option<String> = arr.first().and_then(|ep| {
            ep.get("_links")
                .and_then(|l| l.get("show"))
                .and_then(|s| s.get("name"))
                .and_then(|n| n.as_str())
                .map(String::from)
        });

        for ep in arr {
            let s_num = ep.get("season").and_then(|n| n.as_i64()).unwrap_or(0);
            let ep_num = ep.get("number").and_then(|n| n.as_i64()).unwrap_or(0);
            if ep_num == 0 {
                continue;
            }

            let season_val = s_num as i32;
            let episode_val = ep_num as i32;

            // TVMaze's internal episode ID — globally unique, used as our identifier.
            let ep_unique_id = ep
                .get("id")
                .and_then(|id| id.as_i64())
                .unwrap_or(0)
                .to_string();

            let title = ep
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or("")
                .to_string();

            // TVMaze's `summary` contains HTML; strip the known tags. Lossy for
            // nested tags/attributes but sufficient for UI display.
            let description = ep.get("summary").and_then(|n| n.as_str()).map(|s| {
                s.replace("<p>", "")
                    .replace("</p>", "")
                    .replace("<b>", "")
                    .replace("</b>", "")
            });

            let runtime = ep.get("runtime").and_then(|n| n.as_i64()).map(|n| n as i32);

            let image_url = ep
                .get("image")
                .and_then(|i| i.get("original"))
                .and_then(|o| o.as_str())
                .map(String::from);

            // Prefer `airstamp` (ISO 8601 with offset, precise time-of-day);
            // fall back to `airdate` (date-only "YYYY-MM-DD").
            let meta_date = ep
                .get("airstamp")
                .and_then(|n| n.as_str())
                .filter(|s| !s.is_empty())
                .and_then(|s| {
                    // Strict SSoT parser — TVMaze's `airstamp` is RFC 3339 with an offset.
                    crate::datetime::parse_rfc3339(s).ok()
                })
                .or_else(|| {
                    ep.get("airdate")
                        .and_then(|n| n.as_str())
                        .filter(|s| !s.is_empty())
                        .and_then(|d| {
                            chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d")
                                .ok()
                                .map(UtcDateTime::from_naive_date)
                        })
                });

            episodes.push(EpisodeMetadata {
                unique_id: ep_unique_id,
                season: season_val,
                episode: episode_val,
                title,
                description,
                runtime,
                image_url,
                meta_date,
            });

            *season_counts.entry(season_val).or_insert(0) += 1;
        }

        // Build a season-sorted list; sorting by season number ensures the UI
        // displays seasons in order regardless of TVMaze's return order.
        let mut seasons: Vec<SeasonMetadata> = season_counts
            .into_iter()
            .map(|(season, episode_count)| SeasonMetadata {
                season,
                episode_count,
            })
            .collect();
        seasons.sort_by_key(|s| s.season);

        let series_info = series_title.map(|name| super::SeriesMetadataInfo {
            name,
            overview: None,
            original_country: None,
            aliases: std::collections::HashMap::new(),
            image_url: None,
        });

        Ok((SeriesMetadata { episodes, seasons }, series_info))
    }

    // Fetches show-level metadata from /shows/{id}: canonical title, overview
    // (HTML-stripped, as with episode summaries), and image URL. Aliases are not
    // included here — TVMaze /shows/{id} omits them, so they're fetched separately.
    async fn fetch_series_info_impl(
        &self,
        plugin_series_id: &str,
    ) -> Result<super::SeriesMetadataInfo> {
        let url = format!("https://api.tvmaze.com/shows/{}", plugin_series_id);
        tracing::debug!("Fetching TVMaze series info from {}", url);

        let response = match self.client.get(&url).send().await {
            Ok(resp) => resp,
            Err(e) => {
                return Err(anyhow!(
                    "Failed to reach TVMaze: {}",
                    describe_error_chain(&e)
                ));
            }
        };

        if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(anyhow!(
                "TVMaze rate limit exceeded (429). Please wait a few seconds and try again."
            ));
        }

        if !response.status().is_success() {
            return Err(anyhow!("TVMaze returned HTTP {}", response.status()));
        }

        let json: serde_json::Value = response.json().await.map_err(|e| {
            anyhow!(
                "Failed to parse TVMaze series info: {}",
                describe_error_chain(&e)
            )
        })?;

        let name = json
            .get("name")
            .and_then(|n| n.as_str())
            .unwrap_or("Unknown")
            .to_string();

        let overview = json.get("summary").and_then(|n| n.as_str()).map(|s| {
            s.replace("<p>", "")
                .replace("</p>", "")
                .replace("<b>", "")
                .replace("</b>", "")
        });

        // Build an empty aliases map — TVMaze /shows/{id} does not include aliases.
        // Aliases are fetched separately via fetch_series_aliases.
        let aliases = std::collections::HashMap::new();

        // Extract image URL: prefer `original`, fall back to `medium`.
        let image_url = json
            .get("image")
            .and_then(|img| {
                img.get("original")
                    .and_then(|u| u.as_str())
                    .filter(|s| !s.is_empty())
                    .or_else(|| {
                        img.get("medium")
                            .and_then(|u| u.as_str())
                            .filter(|s| !s.is_empty())
                    })
            })
            .map(String::from);

        tracing::debug!(
            "Fetched TVMaze series info: name={}, has_overview={}, has_image={}",
            name,
            overview.is_some(),
            image_url.is_some()
        );

        Ok(super::SeriesMetadataInfo {
            name,
            overview,
            original_country: None,
            aliases,
            image_url,
        })
    }

    // Fetches alternative titles (AKAs) from /shows/{id}/akas: an array of
    // objects with "name"/"country" fields, flattened to a list of names.
    async fn fetch_series_aliases_impl(&self, plugin_series_id: &str) -> Result<Vec<String>> {
        let url = format!("https://api.tvmaze.com/shows/{}/akas", plugin_series_id);
        tracing::debug!("Fetching TVMaze aliases from {}", url);

        let response = match self.client.get(&url).send().await {
            Ok(resp) => resp,
            Err(e) => {
                return Err(anyhow!(
                    "Failed to reach TVMaze: {}",
                    describe_error_chain(&e)
                ));
            }
        };

        if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(anyhow!(
                "TVMaze rate limit exceeded (429). Please wait a few seconds and try again."
            ));
        }

        if !response.status().is_success() {
            return Err(anyhow!("TVMaze returned HTTP {}", response.status()));
        }

        let json: serde_json::Value = response.json().await.map_err(|e| {
            anyhow!(
                "Failed to parse TVMaze aliases: {}",
                describe_error_chain(&e)
            )
        })?;

        let arr = json
            .as_array()
            .ok_or_else(|| anyhow!("TVMaze AKAs response was not an array"))?;

        let aliases: Vec<String> = arr
            .iter()
            .filter_map(|item| {
                let name = item.get("name").and_then(|n| n.as_str())?;
                if name.is_empty() {
                    return None;
                }
                Some(name.to_string())
            })
            .collect();

        tracing::debug!("Fetched {} TVMaze aliases", aliases.len());

        Ok(aliases)
    }

    // /updates/shows?since=day returns show_id → timestamp for all shows updated
    // in the last day (the endpoint only supports ?since=day, not arbitrary
    // durations). We return all IDs; the caller filters the ones it cares about.
    async fn get_updated_series_impl(&self, _since: chrono::Duration) -> Result<Vec<String>> {
        let url = "https://api.tvmaze.com/updates/shows?since=day";

        let response = self.client.get(url).send().await.map_err(|e| {
            anyhow!(
                "Failed to fetch TVMaze updates: {}",
                describe_error_chain(&e)
            )
        })?;

        if !response.status().is_success() {
            return Err(anyhow!(
                "TVMaze updates endpoint returned HTTP {}",
                response.status()
            ));
        }

        // The response is a JSON object where keys are show IDs and values are
        // timestamps. We only need the keys.
        let map: HashMap<String, serde_json::Value> = response.json().await.map_err(|e| {
            anyhow!(
                "Failed to parse TVMaze updates response: {}",
                describe_error_chain(&e)
            )
        })?;

        Ok(map.into_keys().collect())
    }
}

// Registration
impl TvMazePlugin {
    pub async fn register() {
        use std::sync::Arc;

        InternalPluginRegistry::register_full(
            "metadata.tvmaze",
            |config, id, priority, _refresh_interval, global_cfg, _shutdown_token| {
                let settings: crate::plugins::metadata::tvmaze::TvMazeSettings =
                    serde_json::from_value(config)?;
                Ok(Arc::new(TvMazePlugin::new(
                    &global_cfg,
                    settings,
                    id.clone(),
                    priority,
                )) as Arc<dyn PluginInstance>)
            },
            Self::config_schema,
            Self::plugin_info,
        )
        .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jumbie_shared::config::Config;

    fn dummy_config() -> Config {
        Config::default()
    }

    #[tokio::test]
    async fn test_tvmaze_plugin_name() {
        let config = dummy_config();
        let settings = TvMazeSettings {
            name: "My TVMaze".to_string(),
        };
        let plugin = TvMazePlugin::new(&config, settings, "test-id".to_string(), 0);
        assert_eq!(plugin.cfg().name, "My TVMaze");

        // In-placeset_config updates the name without rebuilding.
        plugin
            .set_config(serde_json::json!({ "name": "Renamed" }))
            .await
            .unwrap();
        assert_eq!(plugin.cfg().name, "Renamed");
        // Invalid config → Err, no mutation.
        assert!(
            plugin
                .set_config(serde_json::json!({ "name": 42 }))
                .await
                .is_err()
        );
        assert_eq!(plugin.cfg().name, "Renamed");
    }

    #[test]
    fn test_tvmaze_identifier_label() {
        assert_eq!(
            TvMazePlugin::plugin_info()
                .series_identifier_label
                .as_deref(),
            Some("TVMaze ID")
        );
    }
}
