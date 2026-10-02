use super::{EpisodeMetadata, SeasonMetadata, SeriesMetadata, SeriesMetadataInfo};
use crate::datetime::{UtcDateTime, country_to_tz};
use crate::plugins::extract::{extract_bool_opt, extract_str};
use crate::plugins::registry::InternalPluginRegistry;
use crate::plugins::{PluginCallError, PluginInstance};
use crate::utils::describe_error_chain;
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use chrono::TimeZone as ChronoTzTimeZone;
use chrono::{DateTime, Duration, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use tokio::sync::RwLock;

/// TVDB V4 API metadata provider settings.
///
/// TVDB V4 replaced the old "user key" model with an API key; an optional PIN
/// grants subscriber-level metadata and higher rate limits, so one plugin covers
/// both free and subscriber tiers.
///
/// The API token is cached in memory only: it expires after ~30 days, so
/// persisting it would still require re-login once expired. `RwLock<Option<>>`
/// allows concurrent reads without blocking on every API call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TvDbSettings {
    #[serde(default = "default_tvdb_name")]
    pub name: String,
    pub api_key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pin: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lang: Option<String>,
}

fn default_tvdb_name() -> String {
    "TVDB".to_string()
}

pub struct TvDbPlugin {
    instance_id: String,
    client: reqwest::Client,
    // Mutable config (in-placeset_config). Reads go through `cfg()` (clone
    // accessor) so the std lock is never held across an await.
    config: std::sync::RwLock<TvDbSettings>,
    token: RwLock<Option<(String, DateTime<Utc>)>>,
    plugin_priority: i32,
}

#[derive(Debug, Serialize, Deserialize)]
struct LoginRequest {
    apikey: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pin: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct LoginResponse {
    data: LoginResponseData,
    status: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct LoginResponseData {
    token: String,
}

/// Independent version of the built-in TVDB plugin.
/// Bump when this plugin's behavior changes — it versions the plugin, not the app.
pub const PLUGIN_VERSION: &str = "1.0.0";

impl TvDbPlugin {
    /// SSoT: Static metadata for this provider. Also registered as the
    /// registry-level info so the two can never diverge.
    pub fn plugin_info() -> jumbie_shared::plugin::PluginTypeInfo {
        jumbie_shared::plugin::PluginTypeInfo {
            display_name: "tvdb".to_string(),
            version: PLUGIN_VERSION.to_string(),
            author: jumbie_shared::plugin::JUMBIE_AUTHOR.to_string(),
            description: "TheTVDB Metadata Provider (V4 API)".to_string(),
            capabilities: vec![
                jumbie_shared::plugin::Capability::MetadataProviderNormal,
                jumbie_shared::plugin::Capability::MetadataProviderAbsolute,
                // Supports periodic background refresh (gated by the instance's
                // `enable_polling` toggle). See `plugins::capabilities`.
                jumbie_shared::plugin::Capability::Polling,
                jumbie_shared::plugin::Capability::FetchSeriesTitle,
                jumbie_shared::plugin::Capability::FetchSeriesAliases,
            ],
            series_identifier_label: Some("TVDB ID".to_string()),
            series_identifier_placeholder: Some("79549".to_string()),
            supported_protocols: None,
            rate_limit: None,
            supports_test: true,
        }
    }

    pub fn config_schema() -> serde_json::Value {
        serde_json::json!({
            "$schema": "http://json-schema.org/draft-07/schema#",
            "title": "TVDBSettings",
            "type": "object",
            "properties": {
                "name": { "type": "string", "title": "Name", "default": "TVDB", "order": -25 },
                "api_key": {
                    "type": "string",
                    "title": "API Key*",
                    "description": "Your TVDB V4 API Key",
                    "order": 1
                },
                "pin": {
                    "type": "integer",
                    "title": "PIN",
                    "description": "Optional subscriber PIN",
                    "order": 2
                },
                "lang": {
                    "type": "string",
                    "title": "Language",
                    "description": "Preferred language for metadata. If None, the default will be used.",
                    "enum": ["", "eng", "spa", "zho", "fra", "deu", "por", "jpn", "kor", "rus", "ita", "nld", "pol", "swe", "ara"],
                    "enumNames": ["None", "English", "Spanish", "Chinese", "French", "German", "Portuguese", "Japanese", "Korean", "Russian", "Italian", "Dutch", "Polish", "Swedish", "Arabic"],
                    "order": 3
                }
            },
            "required": ["api_key"]
        })
    }

    pub fn new(
        config: &jumbie_shared::config::Config,
        settings: TvDbSettings,
        instance_id: String,
        priority: i32,
    ) -> Self {
        let client = crate::utils::http::create_client(config);

        Self {
            instance_id,
            client,
            config: std::sync::RwLock::new(settings),
            token: RwLock::new(None),
            plugin_priority: priority,
        }
    }

    /// Snapshot of the current settings (SSoT: all config reads go through
    /// here so the std lock is never held across an await).
    fn cfg(&self) -> TvDbSettings {
        self.config.read().unwrap().clone()
    }

    /// Fetch JSON from `path`, parse it into `SeriesMetadataInfo`.
    /// Shared by both the translations and base endpoints.
    async fn fetch_series_info_from_api(
        &self,
        path: &str,
        not_found_msg: String,
        parse_aliases: impl FnOnce(&serde_json::Value) -> std::collections::HashMap<String, Vec<String>>,
    ) -> Result<super::SeriesMetadataInfo> {
        tracing::debug!(target: "metadata::tvdb", "Fetching TVDB series info from {}", path);
        let json = self
            .api_get(path)
            .await?
            .ok_or_else(|| anyhow!("{}", not_found_msg))?;
        Self::parse_series_info(json, parse_aliases)
    }

    /// Shared parser: extracts name, overview and calls `parse_aliases` on the
    /// `data` object, then logs and returns the assembled `SeriesMetadataInfo`.
    fn parse_series_info(
        json: serde_json::Value,
        parse_aliases: impl FnOnce(&serde_json::Value) -> std::collections::HashMap<String, Vec<String>>,
    ) -> Result<super::SeriesMetadataInfo> {
        let data = json
            .get("data")
            .ok_or_else(|| anyhow!("Missing data in TVDB series response"))?;

        let name = data
            .get("name")
            .and_then(|n| n.as_str())
            .unwrap_or("Unknown")
            .to_string();

        let overview = data
            .get("overview")
            .and_then(|n| n.as_str())
            .map(String::from);

        let aliases = parse_aliases(data);

        let alias_count: usize = aliases.values().map(|v| v.len()).sum();
        tracing::debug!(
            target: "metadata::tvdb",
            "Fetched TVDB series info: name={}, has_overview={}, alias_count={}",
            name,
            overview.is_some(),
            alias_count
        );

        let original_country = data
            .get("originalCountry")
            .and_then(|n| n.as_str())
            .map(String::from);

        let image_url = data.get("image").and_then(|n| n.as_str()).map(String::from);

        Ok(super::SeriesMetadataInfo {
            name,
            overview,
            original_country,
            aliases,
            image_url,
        })
    }

    /// Get a valid API token, reusing a cached one or logging in fresh.
    ///
    /// Double-checked locking: acquisition is an expensive HTTP POST and concurrent
    /// tasks may need the token at once. The read guard is released before taking
    /// the write guard (a tokio RwLock cannot upgrade, which would deadlock), and
    /// re-checking under the write guard ensures only one racing task logs in.
    ///
    /// Expiry uses a 1-day safety margin rather than exact expiry, so network delay
    /// or clock skew can't expire a token mid-request.
    async fn get_token(&self) -> Result<String> {
        {
            let read_guard = self.token.read().await;
            if let Some((token, expiry)) = &*read_guard
                && *expiry > Utc::now() + Duration::days(1)
            {
                return Ok(token.clone());
            }
        }

        let mut write_guard = self.token.write().await;
        // Re-check after acquiring the write lock.
        if let Some((token, expiry)) = &*write_guard
            && *expiry > Utc::now() + Duration::days(1)
        {
            return Ok(token.clone());
        }

        tracing::debug!(target: "metadata::tvdb", "Authenticating with TVDB...");
        let cfg = self.cfg();
        let login_req = LoginRequest {
            apikey: cfg.api_key,
            pin: cfg.pin,
        };

        // A `send()` error means NO handshake (connect/timeout/DNS/TLS) — a
        // transient network problem, safe to retry with backoff.
        let resp = self
            .client
            .post("https://api4.thetvdb.com/v4/login")
            .json(&login_req)
            .send()
            .await
            .map_err(|e| {
                anyhow::Error::new(PluginCallError::Transient(format!(
                    "Failed to login to TVDB: {}",
                    describe_error_chain(&e)
                )))
            })?;

        if !resp.status().is_success() {
            let status = resp.status();
            let retry_after = Self::retry_after_header(&resp);
            let body = resp.text().await.unwrap_or_default();
            // Handshake received but login failed — classify so the backend's
            // failure policy (plugin spec §8) applies the right behavior:
            // 4xx → auth failure (no retry, cooldown); 429 → pause; 5xx → transient.
            return Err(Self::classify_login_failure(status, &body, retry_after));
        }

        let login_resp: LoginResponse = resp.json().await.map_err(|e| {
            anyhow!(
                "Failed to parse TVDB login response: {}",
                describe_error_chain(&e)
            )
        })?;

        let token = login_resp.data.token;
        // TVDB tokens last ~1 month; use 25 days for safety.
        let expiry = Utc::now() + Duration::days(25);
        *write_guard = Some((token.clone(), expiry));

        Ok(token)
    }

    /// Issue an authenticated GET to the TVDB V4 API.
    ///
    /// No proactive rate limiting: TVDB's limits are generous and plan-dependent,
    /// so throttling would slow bulk imports. A 429 is classified as `RetryAfter`
    /// so the backend's failure policy pauses for the advertised duration instead.
    ///
    /// Returns `Ok(None)` on 404 — a missing series/episode is normal (e.g. a series
    /// with no absolute numbering) and callers fall back gracefully.
    async fn api_get(&self, path: &str) -> Result<Option<serde_json::Value>> {
        let token = self.get_token().await?;
        let url = format!("https://api4.thetvdb.com/v4{}", path);

        let resp = self
            .client
            .get(&url)
            .bearer_auth(token)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| {
                anyhow::Error::new(PluginCallError::Transient(format!(
                    "TVDB API request failed ({}): {}",
                    path,
                    describe_error_chain(&e)
                )))
            })?;

        let retry_after = Self::retry_after_header(&resp);
        match Self::classify_api_response_status(resp.status(), path, retry_after) {
            ApiResponseStatus::OkContinue => {}
            ApiResponseStatus::NotFound => return Ok(None),
            ApiResponseStatus::TokenRejected => {
                // Revoked-token recovery: drop the cached token so the next
                // attempt re-authenticates fresh. A stale token recovers
                // immediately (the wrapper's retry re-runs the call); genuinely
                // bad credentials surface as AuthFailed (→ auth cooldown)
                // instead of failing forever on a dead token.
                *self.token.write().await = None;
                tracing::warn!(
                    target: "metadata::tvdb",
                    "TVDB token rejected for {} (HTTP 401) — re-authenticating on next call",
                    path
                );
                return Err(anyhow::Error::new(PluginCallError::Transient(format!(
                    "TVDB token rejected for {} (HTTP 401) — will re-authenticate",
                    path
                ))));
            }
            ApiResponseStatus::RateLimited(secs) => {
                return Err(anyhow::Error::new(PluginCallError::RetryAfter(secs)));
            }
            ApiResponseStatus::Transient(msg) => {
                return Err(anyhow::Error::new(PluginCallError::Transient(msg)));
            }
            ApiResponseStatus::Permanent(msg) => {
                return Err(anyhow::Error::new(PluginCallError::Permanent(msg)));
            }
        }

        let json: serde_json::Value = resp.json().await.map_err(|e| {
            anyhow!(
                "Failed to parse TVDB response from {}: {}",
                path,
                describe_error_chain(&e)
            )
        })?;

        if json.get("status").and_then(|s| s.as_str()) != Some("success") {
            return Err(anyhow!(
                "TVDB API error for {}: {}",
                path,
                json.get("status")
                    .and_then(|s| s.as_str())
                    .unwrap_or("unknown")
            ));
        }

        Ok(Some(json))
    }

    /// Classify a failed TVDB login response into the backend's shared error
    /// kinds (SSoT: see [`PluginCallError`] and the plugin spec §8).
    ///
    /// - 429        → `RetryAfter` (rate limited — respect the pause).
    /// - other 4xx  → `AuthFailed` (the remote answered but rejected the
    ///   credentials — never retried, enters auth cooldown).
    /// - 5xx        → plain error (transient — retried with backoff).
    fn classify_login_failure(
        status: reqwest::StatusCode,
        body: &str,
        retry_after: Option<u64>,
    ) -> anyhow::Error {
        match status.as_u16() {
            429 => anyhow::Error::new(PluginCallError::RetryAfter(retry_after.unwrap_or(30))),
            code if (400..=499).contains(&code) => {
                tracing::warn!(
                    target: "metadata::tvdb",
                    "TVDB login rejected (HTTP {}): {}",
                    status,
                    body
                );
                anyhow::Error::new(PluginCallError::AuthFailed(format!(
                    "TVDB login rejected (HTTP {}): {}",
                    status, body
                )))
            }
            _ => anyhow::Error::new(PluginCallError::Transient(format!(
                "TVDB login failed with status {}: {}",
                status, body
            ))),
        }
    }

    /// Classify an authenticated request's response status (SSoT for the
    /// revoked-token / 404 / 429 / 5xx / 4xx decision in `api_get`).
    fn classify_api_response_status(
        status: reqwest::StatusCode,
        path: &str,
        retry_after: Option<u64>,
    ) -> ApiResponseStatus {
        match status.as_u16() {
            401 => ApiResponseStatus::TokenRejected,
            404 => ApiResponseStatus::NotFound,
            429 => ApiResponseStatus::RateLimited(retry_after.unwrap_or(30)),
            code if (500..=599).contains(&code) => {
                ApiResponseStatus::Transient(format!("TVDB returned HTTP {} for {}", status, path))
            }
            code if (400..=499).contains(&code) => {
                ApiResponseStatus::Permanent(format!("TVDB returned HTTP {} for {}", status, path))
            }
            _ => ApiResponseStatus::OkContinue,
        }
    }

    /// Parse the `Retry-After` header (integer seconds) if present.
    fn retry_after_header(resp: &reqwest::Response) -> Option<u64> {
        resp.headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok())
    }
}

/// The outcome of inspecting an authenticated TVDB response status — a pure
/// decision so the revoked-token / 404 / 429 / 5xx / 4xx branches are
/// unit-testable without an HTTP mock (see `tests` below).
#[derive(Debug, Clone, PartialEq, Eq)]
enum ApiResponseStatus {
    /// 2xx — proceed to parse the body.
    OkContinue,
    /// 404 — resource missing (a normal state; callers fall back gracefully).
    NotFound,
    /// 401 — token rejected mid-session; the caller must clear the cached token.
    TokenRejected,
    /// 429 — rate limited; pause for `retry_after` seconds.
    RateLimited(u64),
    /// 5xx — transient server error, safe to retry.
    Transient(String),
    /// Other 4xx — deterministic request problem, retrying won't help.
    Permanent(String),
}

#[async_trait]
impl PluginInstance for TvDbPlugin {
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
                let updated = self.get_updated_series_impl(Duration::days(1)).await?;
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
                let absolute = extract_bool_opt(&params, "absolute_numbering", false);
                // When fetching in absolute mode, TVDB returns a single "absolute season"
                // (season=1) with the absolute episode number as the episode field.
                // The backend must key those episodes using the _E{:03} format.
                Ok(serde_json::json!(absolute))
            }
            _ => PluginInstance::call(self, method, params).await,
        }
    }

    async fn test_impl(&self) -> Result<()> {
        tracing::info!(target: "metadata::tvdb", "Testing TVDB connection");
        self.get_token().await?;
        Ok(())
    }

    async fn set_config(&self, config: Value) -> Result<Value> {
        // Parse + validate FIRST — on error return Err with zero mutation; the
        // manager falls back to factory-replacement (plugin spec §7).
        let new: TvDbSettings = serde_json::from_value(config)
            .map_err(|e| anyhow!("Invalid TVDB configuration: {}", e))?;
        if new.api_key.is_empty() {
            return Err(anyhow!("TVDB api_key cannot be empty"));
        }
        // Credential change → the cached token is invalid; clear it so the next
        // call re-authenticates. Name/lang changes don't affect the token.
        // (Config write guard is dropped before the async token write.)
        let creds_changed = {
            let mut guard = self.config.write().unwrap();
            let changed = new.api_key != guard.api_key || new.pin != guard.pin;
            *guard = new;
            changed
        };
        if creds_changed {
            *self.token.write().await = None;
        }
        tracing::info!(
            target: "metadata::tvdb",
            "Reconfigured TVDB plugin '{}' in place",
            self.cfg().name
        );
        Ok(Value::Bool(true))
    }
}

// Internal implementation methods
impl TvDbPlugin {
    /// Fetch all episodes for a series from TVDB, paginating through results.
    ///
    /// TVDB V4 assigns stable, immutable numeric IDs (unlike tvmaze slugs, which
    /// can change on rename), making the ID a reliable foreign key across library
    /// restores. Parsing the ID to `i64` up front validates the format at the plugin
    /// boundary before any API call.
    async fn fetch_series_metadata_impl(
        &self,
        plugin_series_id: &str,
        absolute_numbering: bool,
    ) -> Result<(SeriesMetadata, Option<SeriesMetadataInfo>)> {
        let _id: i64 = plugin_series_id
            .parse()
            .map_err(|_| anyhow!("Invalid TVDB ID: {}", plugin_series_id))?;

        let mut episodes = Vec::new();
        let mut season_counts: HashMap<i32, i32> = HashMap::new();
        let series_info: Option<SeriesMetadataInfo> = None;
        let mut page = 0;
        // Country code cached from the first page's series-level data.
        let mut original_country: Option<String> = None;

        let season_type = if absolute_numbering {
            "absolute"
        } else {
            "default"
        };

        loop {
            let lang_part = match &self.cfg().lang {
                Some(l) if !l.is_empty() => format!("/{}", l),
                _ => "".to_string(),
            };
            let path = format!(
                "/series/{}/episodes/{}{}?page={}",
                plugin_series_id, season_type, lang_part, page
            );
            let json = match self.api_get(&path).await? {
                Some(j) => j,
                None => {
                    if !lang_part.is_empty() {
                        let fallback_path = format!(
                            "/series/{}/episodes/{}?page={}",
                            plugin_series_id, season_type, page
                        );
                        match self.api_get(&fallback_path).await? {
                            Some(j) => j,
                            None => break,
                        }
                    } else {
                        break;
                    }
                }
            };

            let data = json
                .get("data")
                .ok_or_else(|| anyhow!("Missing data in TVDB response"))?;

            // The series-level name is NOT taken from here: TVDB's episodes
            // endpoint always returns the ORIGINAL (untranslated) name even with a
            // language param; the translated title only comes from the dedicated
            // translations endpoint (see `fetch_series_info`). `originalCountry` is
            // language-independent, so we cache it from page 0 to interpret episode
            // `aired` dates in the show's home timezone.
            if page == 0 {
                original_country = data
                    .get("originalCountry")
                    .and_then(|n| n.as_str())
                    .map(String::from);
            }

            let episodes_val = data.get("episodes").and_then(|e| e.as_array());

            let episodes_batch = match episodes_val {
                Some(eps) if !eps.is_empty() => eps,
                _ => break, // No more episodes or empty page
            };

            for ep in episodes_batch {
                let season_num =
                    ep.get("seasonNumber").and_then(|n| n.as_i64()).unwrap_or(0) as i32;

                let ep_num = ep.get("number").and_then(|n| n.as_i64()).unwrap_or(0) as i32;
                if ep_num == 0 {
                    continue;
                }

                let unique_id = ep
                    .get("id")
                    .and_then(|id| id.as_i64())
                    .unwrap_or(0)
                    .to_string();
                let title = ep
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or("")
                    .to_string();
                let description = ep
                    .get("overview")
                    .and_then(|n| n.as_str())
                    .map(String::from);
                let runtime = ep.get("runtime").and_then(|n| n.as_i64()).map(|n| n as i32);
                // TVDB stores episode images as relative paths like
                // `/banners/v4/episode/11371286/screencap/69b81485d76a4.jpg`.
                // Prepend the CDN base URL to get the full URL.
                let image_url = ep.get("image").and_then(|i| i.as_str()).map(|p| {
                    if p.starts_with("http") {
                        p.to_string()
                    } else {
                        format!("https://artworks.thetvdb.com{}", p)
                    }
                });

                // TVDB's `aired` is date-only ("YYYY-MM-DD"). With the series'
                // original country we interpret midnight in that timezone and convert
                // to UTC; otherwise store as midnight UTC.
                let tz: Option<Tz> = original_country.as_deref().and_then(country_to_tz);
                let meta_date = ep
                    .get("aired")
                    .and_then(|n| n.as_str())
                    .filter(|s| !s.is_empty())
                    .and_then(|d| {
                        let naive_date = chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok()?;
                        if let Some(tz) = tz {
                            // Midnight in the show's home timezone, then to UTC.
                            let local_dt = naive_date.and_hms_opt(0, 0, 0).unwrap();
                            tz.from_local_datetime(&local_dt)
                                .single()
                                .map(UtcDateTime::from_chrono_with_offset)
                        } else {
                            // No timezone info — store as midnight UTC.
                            Some(UtcDateTime::from_naive_date(naive_date))
                        }
                    });

                episodes.push(EpisodeMetadata {
                    unique_id,
                    season: season_num,
                    episode: ep_num,
                    title,
                    description,
                    runtime,
                    image_url,
                    meta_date,
                });

                *season_counts.entry(season_num).or_insert(0) += 1;
            }

            // TVDB V4 paginates (typically 100 items/page); `links.next` holds the
            // next page number or null on the last page. We follow it rather than a
            // plain `page++` because a series may have fewer pages than expected.
            if let Some(links) = json.get("links") {
                if links.get("next").is_none()
                    || links
                        .get("next")
                        .and_then(|n| n.as_str())
                        .map(|s| s.is_empty())
                        .unwrap_or(true)
                {
                    break;
                }
            } else {
                // No links field at all: fall back to breaking on a small batch.
                if episodes_batch.len() < 10 {
                    break;
                }
            }

            page += 1;
        }

        let mut seasons: Vec<SeasonMetadata> = season_counts
            .into_iter()
            .map(|(season, episode_count)| SeasonMetadata {
                season,
                episode_count,
            })
            .collect();
        seasons.sort_by_key(|s| s.season);

        Ok((SeriesMetadata { episodes, seasons }, series_info))
    }

    async fn get_updated_series_impl(&self, since: Duration) -> Result<Vec<String>> {
        // TVDB v4 /updates requires a `since` query param (Unix timestamp in seconds).
        // Calculate the timestamp by subtracting the caller-provided duration from now.
        let since_ts = (Utc::now() - since).timestamp();
        let path = format!("/updates?since={}&type=series", since_ts);
        let json_opt = self.api_get(&path).await?;
        let json = match json_opt {
            Some(j) => j,
            None => return Ok(Vec::new()),
        };
        let data = json
            .get("data")
            .and_then(|d| d.as_array())
            .ok_or_else(|| anyhow!("Missing data in TVDB updates response"))?;

        let mut series_ids = Vec::new();
        for item in data {
            if item.get("recordType").and_then(|t| t.as_str()) == Some("series")
                && let Some(id) = item.get("recordId").and_then(|id| id.as_i64())
            {
                series_ids.push(id.to_string());
            }
        }

        series_ids.sort();
        series_ids.dedup();

        Ok(series_ids)
    }

    // Fetches show-level metadata from TVDB. With a configured language, hits
    // /series/{id}/translations/{lang}; otherwise /series/{id} for original-language
    // data.
    async fn fetch_series_info_impl(&self, plugin_series_id: &str) -> Result<SeriesMetadataInfo> {
        let _id: i64 = plugin_series_id
            .parse()
            .map_err(|_| anyhow!("Invalid TVDB ID: {}", plugin_series_id))?;

        // No in-memory cache — series info is now persisted to the DB cache by the
        // API layer when `fetch_series_metadata` returns it. The DB cache is checked
        // first by the caller (fetch_series_info route handler) before reaching here.
        match &self.cfg().lang {
            Some(l) if !l.is_empty() => {
                let path = format!("/series/{}/translations/{}", plugin_series_id, l);
                self.fetch_series_info_from_api(
                    &path,
                    format!(
                        "TVDB series {} translation not found for language {}",
                        plugin_series_id, l
                    ),
                    |data| {
                        // TVDB /translations endpoint returns "aliases" as a flat array of strings.
                        // The language is inferred from the "language" field in the response.
                        let response_lang = data
                            .get("language")
                            .and_then(|l| l.as_str())
                            .unwrap_or("unknown")
                            .to_string();

                        let mut aliases: std::collections::HashMap<String, Vec<String>> =
                            std::collections::HashMap::new();
                        if let Some(alias_arr) = data.get("aliases").and_then(|a| a.as_array()) {
                            let list: Vec<String> = alias_arr
                                .iter()
                                .filter_map(|v| v.as_str().map(String::from))
                                .collect();
                            if !list.is_empty() {
                                aliases.insert(response_lang, list);
                            }
                        }
                        aliases
                    },
                )
                .await
            }
            _ => {
                let path = format!("/series/{}", plugin_series_id);
                self.fetch_series_info_from_api(
                    &path,
                    format!("TVDB series {} not found", plugin_series_id),
                    |data| {
                        // The base /series/{id} endpoint returns aliases as an array of
                        // { "language": "...", "name": "..." } objects.
                        let mut aliases: std::collections::HashMap<String, Vec<String>> =
                            std::collections::HashMap::new();
                        if let Some(alias_arr) = data.get("aliases").and_then(|a| a.as_array()) {
                            for alias_obj in alias_arr {
                                if let Some(name) = alias_obj.get("name").and_then(|n| n.as_str()) {
                                    let lang = alias_obj
                                        .get("language")
                                        .and_then(|l| l.as_str())
                                        .unwrap_or("unknown")
                                        .to_string();
                                    aliases.entry(lang).or_default().push(name.to_string());
                                }
                            }
                        }
                        aliases
                    },
                )
                .await
            }
        }
    }

    // Fetches alternative titles via `fetch_series_info` and returns a flat list
    // of alias strings across all languages.
    async fn fetch_series_aliases_impl(&self, plugin_series_id: &str) -> Result<Vec<String>> {
        let info = self.fetch_series_info_impl(plugin_series_id).await?;

        let aliases: Vec<String> = info
            .aliases
            .into_values()
            .flat_map(|lang_aliases| lang_aliases.into_iter())
            .collect();

        tracing::debug!(
            target: "metadata::tvdb",
            "Fetched {} TVDB aliases for series {}",
            aliases.len(),
            plugin_series_id
        );

        Ok(aliases)
    }
}

// Registration
impl TvDbPlugin {
    pub async fn register() {
        use std::sync::Arc;

        InternalPluginRegistry::register_full(
            "metadata.tvdb",
            |config, id, priority, _refresh_interval, global_cfg, _shutdown_token| {
                let settings: crate::plugins::metadata::tvdb::TvDbSettings =
                    serde_json::from_value(config.clone())
                        .map_err(|e| anyhow::anyhow!("Invalid TVDB configuration: {}", e))?;
                if settings.api_key.is_empty() {
                    anyhow::bail!("TVDB API Key is required");
                }
                Ok(
                    Arc::new(TvDbPlugin::new(&global_cfg, settings, id.clone(), priority))
                        as Arc<dyn PluginInstance>,
                )
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

    #[test]
    fn test_classify_login_failure_401_is_auth() {
        let err = TvDbPlugin::classify_login_failure(
            reqwest::StatusCode::UNAUTHORIZED,
            "{\"Error\":\"invalid key\"}",
            None,
        );
        assert!(matches!(
            err.downcast_ref::<PluginCallError>(),
            Some(PluginCallError::AuthFailed(msg)) if msg.contains("401")
        ));
    }

    #[test]
    fn test_classify_login_failure_403_is_auth() {
        let err =
            TvDbPlugin::classify_login_failure(reqwest::StatusCode::FORBIDDEN, "forbidden", None);
        assert!(matches!(
            err.downcast_ref::<PluginCallError>(),
            Some(PluginCallError::AuthFailed(_))
        ));
    }

    #[test]
    fn test_classify_login_failure_429_is_rate_limit() {
        let err = TvDbPlugin::classify_login_failure(
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            "",
            Some(45),
        );
        assert!(matches!(
            err.downcast_ref::<PluginCallError>(),
            Some(PluginCallError::RetryAfter(45))
        ));
        // Without a Retry-After header, fall back to the 30s default.
        let err =
            TvDbPlugin::classify_login_failure(reqwest::StatusCode::TOO_MANY_REQUESTS, "", None);
        assert!(matches!(
            err.downcast_ref::<PluginCallError>(),
            Some(PluginCallError::RetryAfter(30))
        ));
    }

    #[test]
    fn test_classify_login_failure_5xx_is_transient() {
        // A handshake that completes with a server error is NOT an auth
        // failure — it is an explicit transient error so the retry-with-backoff
        // path applies.
        let err = TvDbPlugin::classify_login_failure(
            reqwest::StatusCode::INTERNAL_SERVER_ERROR,
            "boom",
            None,
        );
        assert!(matches!(
            err.downcast_ref::<PluginCallError>(),
            Some(PluginCallError::Transient(_))
        ));
    }

    fn test_plugin() -> TvDbPlugin {
        TvDbPlugin::new(
            &jumbie_shared::config::Config::default(),
            TvDbSettings {
                name: "old".to_string(),
                api_key: "key".to_string(),
                pin: None,
                lang: None,
            },
            "inst".to_string(),
            0,
        )
    }

    #[tokio::test]
    async fn test_set_config_applies_fields_and_keeps_token() {
        let plugin = test_plugin();
        *plugin.token.write().await = Some(("tok".to_string(), Utc::now() + Duration::days(20)));

        // Non-credential change (name/lang) — applies and the token survives.
        plugin
            .set_config(serde_json::json!({"name": "new", "api_key": "key", "lang": "de"}))
            .await
            .unwrap();
        let cfg = plugin.cfg();
        assert_eq!(cfg.name, "new");
        assert_eq!(cfg.lang.as_deref(), Some("de"));
        assert!(
            plugin.token.read().await.is_some(),
            "token should survive non-credential changes"
        );
    }

    #[tokio::test]
    async fn test_set_config_credential_change_clears_token() {
        let plugin = test_plugin();
        *plugin.token.write().await = Some(("tok".to_string(), Utc::now() + Duration::days(20)));

        plugin
            .set_config(serde_json::json!({"api_key": "newkey", "pin": null}))
            .await
            .unwrap();
        assert_eq!(plugin.cfg().api_key, "newkey");
        assert!(
            plugin.token.read().await.is_none(),
            "credential change must drop the cached token"
        );
    }

    #[tokio::test]
    async fn test_set_config_invalid_config_errors_without_mutation() {
        let plugin = test_plugin();

        // Wrong type → parse error, zero mutation.
        assert!(
            plugin
                .set_config(serde_json::json!({ "api_key": 42 }))
                .await
                .is_err()
        );
        assert_eq!(plugin.cfg().api_key, "key");

        // Empty api_key → rejected (matches the factory's validation).
        assert!(
            plugin
                .set_config(serde_json::json!({ "api_key": "" }))
                .await
                .is_err()
        );
        assert_eq!(plugin.cfg().api_key, "key");
    }

    #[test]
    fn test_classify_api_response_status() {
        let s = |code: u16| reqwest::StatusCode::from_u16(code).unwrap();

        assert_eq!(
            TvDbPlugin::classify_api_response_status(s(200), "/x", None),
            ApiResponseStatus::OkContinue
        );
        assert_eq!(
            TvDbPlugin::classify_api_response_status(s(401), "/x", None),
            ApiResponseStatus::TokenRejected
        );
        assert_eq!(
            TvDbPlugin::classify_api_response_status(s(404), "/x", None),
            ApiResponseStatus::NotFound
        );
        assert_eq!(
            TvDbPlugin::classify_api_response_status(s(429), "/x", Some(45)),
            ApiResponseStatus::RateLimited(45)
        );
        assert_eq!(
            TvDbPlugin::classify_api_response_status(s(429), "/x", None),
            ApiResponseStatus::RateLimited(30)
        );
        assert!(matches!(
            TvDbPlugin::classify_api_response_status(s(500), "/x", None),
            ApiResponseStatus::Transient(_)
        ));
        assert!(matches!(
            TvDbPlugin::classify_api_response_status(s(403), "/x", None),
            ApiResponseStatus::Permanent(_)
        ));
    }
}
