// Discord Notifier Plugin
//
// Sends rich embed notifications to a Discord channel via webhook. All events use
// embeds with color-coded sidebars — no customizable templates.
//
// Color Scheme (SSoT) — the single source of truth for Discord embed colors:
//   • Orange — in-progress events (download started)
//   • Green  — success events (download completed)
//   • Purple — pending/pool events (rename queue)
//   • Red    — error events
//   • White  — test notifications

use super::{NotifierContext, NotifierEvent};
use crate::plugins::{PluginCallError, PluginInstance};
use anyhow::Result;
use async_trait::async_trait;
use jumbie_shared::formatting::{LabelStyle, fmt_season_episode};
use jumbie_shared::parsing::format_bytes;
use jumbie_shared::plugin::Capability;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tracing::trace;

const COLOR_DOWNLOAD_STARTED: u32 = 0xFFA500;
const COLOR_DOWNLOAD_COMPLETED: u32 = 0x00FF00;
const COLOR_RENAME_QUEUE: u32 = 0x9B59B6;
const COLOR_ERROR: u32 = 0xFF0000;
const COLOR_TEST: u32 = 0xFFFFFF;

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct DiscordConfig {
    #[serde(default = "default_discord_name")]
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub webhook_url: String,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub avatar_url: Option<String>,
    #[serde(default)]
    pub events: NotifierEvents,
}

// Events are flat toggles.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct NotifierEvents {
    #[serde(default, rename = "Download Started")]
    pub download_started: bool,
    #[serde(default, rename = "Download Completed")]
    pub download_completed: bool,
    #[serde(default, rename = "Rename Queue")]
    pub rename_queue: bool,
    #[serde(default, rename = "Error")]
    pub error: bool,
}

impl Default for NotifierEvents {
    fn default() -> Self {
        Self {
            download_started: true,
            download_completed: true,
            rename_queue: true,
            error: true,
        }
    }
}

fn default_discord_name() -> String {
    "Discord".to_string()
}

fn default_true() -> bool {
    true
}

impl DiscordConfig {
    pub fn validate(&self) -> std::result::Result<(), String> {
        if self.enabled && self.webhook_url.is_empty() {
            return Err("webhook_url is required when enabled".to_string());
        }
        if self.enabled
            && !self.webhook_url.is_empty()
            && !self
                .webhook_url
                .starts_with("https://discord.com/api/webhooks/")
            && !self
                .webhook_url
                .starts_with("https://discordapp.com/api/webhooks/")
        {
            return Err(
                "webhook_url must be a Discord webhook URL (https://discord.com/api/webhooks/...)"
                    .to_string(),
            );
        }
        Ok(())
    }
}

pub struct DiscordNotifier {
    instance_id: String,
    priority: i32,
    // Mutable config (in-placeset_config); reads go through `cfg()`.
    config: std::sync::RwLock<DiscordConfig>,
    client: reqwest::Client,
}

impl DiscordNotifier {
    pub fn new(
        config: DiscordConfig,
        instance_id: String,
        priority: i32,
        global_config: &jumbie_shared::config::Config,
    ) -> Self {
        Self {
            instance_id,
            priority,
            config: std::sync::RwLock::new(config),
            client: crate::utils::http::create_client(global_config),
        }
    }

    /// Snapshot of the current config (SSoT: all config reads go through here
    /// so the std lock is never held across an await).
    fn cfg(&self) -> DiscordConfig {
        self.config.read().unwrap().clone()
    }

    // Sends a rich embed with a description, color-coded sidebar, and named
    // fields. Title is optional (None omits it); the timestamp lets Discord
    // auto-format relative time.
    async fn send_embed(
        &self,
        title: Option<&str>,
        description: &str,
        color: u32,
        fields: Vec<serde_json::Value>,
        thumbnail_url: Option<&str>,
        image_url: Option<&str>,
    ) -> Result<()> {
        let cfg = self.cfg();
        if cfg.webhook_url.is_empty() {
            return Ok(());
        }

        let mut embed = json!({
            "description": description,
            "color": color,
            "fields": fields,
            "timestamp": crate::datetime::UtcDateTime::now().to_rfc3339_utc()
        });
        if let Some(t) = title {
            embed["title"] = json!(t);
        }
        if let Some(url) = thumbnail_url {
            embed["thumbnail"] = json!({"url": url});
        }
        if let Some(url) = image_url {
            embed["image"] = json!({"url": url});
        }

        let mut payload = json!({
            "embeds": [embed]
        });

        if let Some(username) = &cfg.username {
            payload["username"] = json!(username);
        }
        if let Some(avatar_url) = &cfg.avatar_url {
            payload["avatar_url"] = json!(avatar_url);
        }

        let response = self
            .client
            .post(&cfg.webhook_url)
            .json(&payload)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Discord webhook returned HTTP {}: {}", status, body);
        }

        Ok(())
    }

    fn format_languages(langs: &[String]) -> String {
        if langs.is_empty() {
            return "—".to_string();
        }
        langs.join(", ")
    }

    /// Send a notification for a specific event.
    pub async fn notify(&self, event: NotifierEvent, context: &NotifierContext) -> Result<()> {
        trace!(?event, "Constructing Discord notification");

        // Test notifications always fire regardless of per-event config.
        if matches!(event, NotifierEvent::Test) {
            return self
                .send_embed(
                    None,
                    "**Test Notification**\nThis is a test notification from Jumbie.",
                    COLOR_TEST,
                    vec![],
                    None,
                    None,
                )
                .await;
        }

        // Check the user's per-event config toggle.
        let cfg = self.cfg();
        let enabled = match event {
            NotifierEvent::DownloadStarted => cfg.events.download_started,
            NotifierEvent::DownloadCompleted => cfg.events.download_completed,
            NotifierEvent::RenameQueue => cfg.events.rename_queue,
            NotifierEvent::Error => cfg.events.error,
            NotifierEvent::Test => unreachable!(),
        };
        if !enabled {
            return Ok(());
        }

        match event {
            NotifierEvent::DownloadStarted => {
                let series = context.series_title.as_deref().unwrap_or("Unknown");
                let release = context.release_title.as_deref().unwrap_or("");

                let mut desc = String::from("**");
                desc.push_str(series);
                if let (Some(season), Some(ep)) = (context.season, context.episode) {
                    let ep_label =
                        fmt_season_episode(season, ep, context.episode_end, LabelStyle::Short);
                    desc.push_str(&format!(" - {}", ep_label));
                }
                desc.push_str("**\nDownload started");
                if !release.is_empty() {
                    desc.push_str(&format!("\n`{}`", release));
                }

                let mut fields = Vec::new();
                if let Some(ref idx) = context.indexer
                    && !idx.is_empty()
                {
                    fields.push(json!({"name": "Source", "value": idx, "inline": true}));
                }
                if let Some(bytes) = context.size_bytes {
                    fields.push(
                        json!({"name": "Size", "value": format_bytes(bytes), "inline": true}),
                    );
                }
                self.send_embed(None, &desc, COLOR_DOWNLOAD_STARTED, fields, None, None)
                    .await
            }
            NotifierEvent::DownloadCompleted => {
                let series = context.series_title.as_deref().unwrap_or("Unknown");
                let release = context.release_title.as_deref().unwrap_or("");

                let mut desc = String::from("**");
                desc.push_str(series);
                if let (Some(season), Some(ep)) = (context.season, context.episode) {
                    let ep_label =
                        fmt_season_episode(season, ep, context.episode_end, LabelStyle::Short);
                    desc.push_str(&format!(" - {}", ep_label));
                }
                desc.push_str("**\nDownload completed");
                if !release.is_empty() {
                    desc.push_str(&format!("\n`{}`", release));
                }

                let mut fields = Vec::new();
                if let Some(mi) = &context.media_info {
                    if let Some(res) = &mi.resolution {
                        fields.push(json!({"name": "Resolution", "value": res, "inline": true}));
                    }
                    if let Some(codec) = &mi.codec {
                        fields.push(json!({"name": "Codec", "value": codec, "inline": true}));
                    }
                    if let Some(dur) = &mi.duration {
                        fields.push(json!({"name": "Duration", "value": dur, "inline": true}));
                    }
                    if !mi.audio_languages.is_empty() {
                        fields.push(json!({"name": "Audio", "value": Self::format_languages(&mi.audio_languages), "inline": true}));
                    }
                    if !mi.subtitle_languages.is_empty() {
                        fields.push(json!({"name": "Subtitles", "value": Self::format_languages(&mi.subtitle_languages), "inline": true}));
                    }
                }
                if let Some(ref idx) = context.indexer
                    && !idx.is_empty()
                {
                    fields.push(json!({"name": "Source", "value": idx, "inline": true}));
                }
                if let Some(bytes) = context.size_bytes {
                    fields.push(
                        json!({"name": "Size", "value": format_bytes(bytes), "inline": true}),
                    );
                }
                self.send_embed(None, &desc, COLOR_DOWNLOAD_COMPLETED, fields, None, None)
                    .await
            }
            NotifierEvent::RenameQueue => {
                let desc = if !context.rename_queue_entries.is_empty() {
                    // Batched entries: format as a code-block table
                    let max_len = context
                        .rename_queue_entries
                        .iter()
                        .map(|(s, _)| s.len())
                        .max()
                        .unwrap_or(10)
                        .max(10);
                    let mut d = String::from("**Pending Reorganization**\n\n```\n");
                    d.push_str(&format!("{:<width$}  Files\n", "Series", width = max_len));
                    d.push_str(&format!("{}\n", "─".repeat(max_len + 7)));
                    for (title, count) in &context.rename_queue_entries {
                        d.push_str(&format!("{:<width$}  {}\n", title, count, width = max_len));
                    }
                    d.push_str("\n```");
                    d
                } else {
                    // Single series
                    let series = context.series_title.as_deref().unwrap_or("Unknown");
                    let count = context.affected_count.unwrap_or(0);
                    format!("**Pending Reorganization**\n{} — {} file(s)", series, count)
                };
                self.send_embed(None, &desc, COLOR_RENAME_QUEUE, vec![], None, None)
                    .await
            }
            NotifierEvent::Error => {
                let ctx = context.error_context.as_deref().unwrap_or("Unknown");
                let err = context.error_message.as_deref().unwrap_or("Unknown error");
                let desc = format!("**{}**\n\n{}", ctx, err);
                self.send_embed(None, &desc, COLOR_ERROR, vec![], None, None)
                    .await
            }
            NotifierEvent::Test => unreachable!(), // handled above
        }
    }
}

#[async_trait]
impl PluginInstance for DiscordNotifier {
    fn instance_id(&self) -> &str {
        &self.instance_id
    }

    fn plugin_info(&self) -> plugin_sdk::traits::PluginTypeInfo {
        Self::plugin_info()
    }

    fn priority(&self) -> i32 {
        self.priority
    }

    fn supported_protocols(&self) -> Option<&[String]> {
        None
    }

    async fn handle_custom_method(&self, method: &str, params: Option<Value>) -> Result<Value> {
        match method {
            "notify" => {
                let (event, context) = crate::plugins::notifiers::parse_notify_params(params)?;
                self.notify(event, &context).await?;
                Ok(Value::Null)
            }
            "on_test" => {
                self.notify(NotifierEvent::Test, &NotifierContext::new())
                    .await?;
                Ok(serde_json::json!("Test message sent to Discord"))
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

    async fn set_config(&self, config: Value) -> Result<Value> {
        // Parse + validate FIRST; on error return Err with zero mutation (the
        // manager falls back to factory-replacement).
        let new: DiscordConfig = serde_json::from_value(config)
            .map_err(|e| anyhow::anyhow!("Invalid Discord configuration: {}", e))?;
        if let Err(e) = new.validate() {
            return Err(anyhow::anyhow!("Invalid Discord configuration: {}", e));
        }
        *self.config.write().unwrap() = new;
        tracing::info!("Reconfigured Discord notifier in place");
        Ok(Value::Bool(true))
    }
}

// Static Info & Config Schema

/// Independent version of the built-in Discord plugin.
/// Bump when this plugin's behavior changes — it versions the plugin, not the app.
pub const PLUGIN_VERSION: &str = "1.0.0";

impl DiscordNotifier {
    /// SSoT: Static metadata for this plugin. Also registered as the
    /// registry-level info so the two can never diverge.
    pub fn plugin_info() -> plugin_sdk::traits::PluginTypeInfo {
        plugin_sdk::traits::PluginTypeInfo {
            display_name: "Discord".to_string(),
            version: PLUGIN_VERSION.to_string(),
            author: jumbie_shared::plugin::JUMBIE_AUTHOR.to_string(),
            description: "Discord webhook notifications".to_string(),
            capabilities: vec![Capability::Notifier],
            supported_protocols: None,
            series_identifier_label: None,
            series_identifier_placeholder: None,
            rate_limit: None,
            supports_test: true,
        }
    }

    pub fn config_schema() -> serde_json::Value {
        serde_json::json!({
            "$schema": "http://json-schema.org/draft-07/schema#",
            "title": "DiscordConfig",
            "type": "object",
            "required": ["webhook_url"],
            "properties": {
                "name": { "type": "string", "title": "Name", "default": "Discord", "order": -25 },
                "enabled": { "type": "boolean", "title": "Enabled", "default": true, "order": -20 },
                "webhook_url": {
                    "type": "string",
                    "title": "Webhook URL*",
                    "placeholder": "https://discord.com/api/webhooks/...",
                    "order": 10
                },
                "username": { "type": "string", "title": "Username Override (Optional)", "default": "Jumbie", "order": 20 },
                "avatar_url": { "type": "string", "title": "Avatar URL (Optional)", "order": 30 },
                "events": {
                    "type": "object",
                    "title": "Event Configuration",
                    "description": "Configure which events trigger notifications.",
                    "order": 40,
                    "properties": {
                        "Download Started": {
                            "type": "boolean",
                            "title": "Download Started",
                            "description": "Fires when an automated download is submitted to the client.",
                            "default": true,
                            "order": 50
                        },
                        "Download Completed": {
                            "type": "boolean",
                            "title": "Download Completed",
                            "description": "Fires when a download finishes and is added to the library.",
                            "default": true,
                            "order": 60
                        },
                        "Rename Queue": {
                            "type": "boolean",
                            "title": "Rename Queue",
                            "description": "Fires when episodes need renaming.",
                            "default": true,
                            "order": 75
                        },
                        "Error": {
                            "type": "boolean",
                            "title": "Error",
                            "description": "Fires on pipeline failures.",
                            "default": true,
                            "order": 80
                        },
                    }
                }
            }
        })
    }
}

// Registration
impl DiscordNotifier {
    pub async fn register() {
        use std::sync::Arc;

        use crate::plugins::registry::InternalPluginRegistry;

        InternalPluginRegistry::register_full(
            "notifier.discord",
            |config, id, priority, _refresh_interval, global_cfg, _shutdown_token| {
                use crate::plugins::PluginInstance;

                let cfg: DiscordConfig = serde_json::from_value(config)?;
                if let Err(e) = cfg.validate() {
                    tracing::warn!("Discord config validation failed (loading anyway): {}", e);
                }
                if !cfg.enabled {
                    anyhow::bail!("Discord notifier is disabled");
                }
                Ok(
                    Arc::new(DiscordNotifier::new(cfg, id, priority, &global_cfg))
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
    use jumbie_shared::types::MediaInfo;

    fn default_cfg() -> DiscordConfig {
        serde_json::from_value(serde_json::json!({})).unwrap()
    }

    #[test]
    fn test_discord_config_defaults() {
        let cfg = default_cfg();
        assert!(cfg.enabled);
        assert!(cfg.webhook_url.is_empty());
    }

    #[test]
    fn test_discord_settings_validation() {
        let mut discord = default_cfg();
        discord.enabled = true;
        discord.webhook_url = "".to_string();
        discord.username = Some("Bot".to_string());

        let result = discord.validate();
        assert!(result.is_err(), "Empty webhook URL should fail validation");
        assert!(result.unwrap_err().contains("webhook_url"));

        discord.webhook_url = "https://discord.com/api/webhooks/123/abc".to_string();
        assert!(discord.validate().is_ok(), "Valid webhook URL should pass");

        discord.webhook_url = "http://example.com".to_string();
        assert!(discord.validate().is_err(), "Non-discord URL should fail");
    }

    #[test]
    fn test_discord_enabled_flag() {
        let mut cfg = default_cfg();
        assert!(cfg.enabled);
        cfg.enabled = false;
        assert!(!cfg.enabled);
        cfg.enabled = true;
        assert!(cfg.enabled);
    }

    fn notifier_with_all_events_enabled() -> DiscordNotifier {
        let config = DiscordConfig {
            name: "Discord".to_string(),
            enabled: true,
            webhook_url: String::new(),
            username: None,
            avatar_url: None,
            events: NotifierEvents {
                download_started: true,
                download_completed: true,
                rename_queue: true,
                error: true,
            },
        };
        let global = jumbie_shared::config::Config::default();
        DiscordNotifier::new(config, "test_discord".to_string(), 0, &global)
    }

    async fn assert_all_events_noop(notifier: &DiscordNotifier, ctx: &NotifierContext) {
        assert!(
            notifier
                .notify(NotifierEvent::DownloadStarted, ctx)
                .await
                .is_ok()
        );
        assert!(
            notifier
                .notify(NotifierEvent::DownloadCompleted, ctx)
                .await
                .is_ok()
        );
        assert!(
            notifier
                .notify(NotifierEvent::RenameQueue, ctx)
                .await
                .is_ok()
        );
        assert!(notifier.notify(NotifierEvent::Error, ctx).await.is_ok());
    }

    #[tokio::test]
    async fn test_notify_disabled_event_skips_silently() {
        let global = jumbie_shared::config::Config::default();
        let config = DiscordConfig {
            name: "Discord".to_string(),
            enabled: true,
            webhook_url: "https://discord.com/api/webhooks/123/abc".to_string(),
            username: None,
            avatar_url: None,
            events: NotifierEvents {
                download_started: false,
                download_completed: false,
                rename_queue: false,
                error: false,
            },
        };
        let notifier = DiscordNotifier::new(config, "test_discord".to_string(), 0, &global);
        let ctx = NotifierContext::new()
            .with_series("Test")
            .with_episode(1, 1);

        assert_all_events_noop(&notifier, &ctx).await;
    }

    #[tokio::test]
    async fn test_notify_enabled_event_with_empty_webhook_noops() {
        let notifier = notifier_with_all_events_enabled();
        let ctx = NotifierContext::new()
            .with_series("The Time Machine")
            .with_episode(4, 5);

        assert_all_events_noop(&notifier, &ctx).await;
    }

    #[tokio::test]
    async fn test_notify_rename_queue_with_affected_count() {
        let notifier = notifier_with_all_events_enabled();
        let ctx = NotifierContext::new()
            .with_series("Mock Series")
            .with_affected_count(12);

        let result = notifier.notify(NotifierEvent::RenameQueue, &ctx).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_notify_rename_queue_with_batched_entries() {
        let notifier = notifier_with_all_events_enabled();
        let ctx = NotifierContext::new().with_rename_queue_entries(vec![
            ("Sherlock Holmes".to_string(), 12),
            ("Dracula".to_string(), 5),
            ("Frankenstein".to_string(), 3),
        ]);

        let result = notifier.notify(NotifierEvent::RenameQueue, &ctx).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_notify_error_with_context() {
        let notifier = notifier_with_all_events_enabled();
        let ctx = NotifierContext::new().with_error("Organize Failure", "Permission denied");

        let result = notifier.notify(NotifierEvent::Error, &ctx).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_notify_download_completed_with_media_info() {
        let notifier = notifier_with_all_events_enabled();
        let ctx = NotifierContext::new()
            .with_series("Test Series")
            .with_episode_id("S01E05")
            .with_release_title("Test.Series.S01E05.1080p.WEB-DL.HEVC.mkv")
            .with_indexer("Nyaa")
            .with_size_bytes(1_400_000_000)
            .with_media_info(MediaInfo {
                resolution: Some("1920x1080".to_string()),
                codec: Some("HEVC".to_string()),
                duration: Some("24 min".to_string()),
                audio_languages: vec!["eng".to_string(), "jpn".to_string()],
                subtitle_languages: vec!["eng".to_string()],
                ..Default::default()
            });

        let result = notifier
            .notify(NotifierEvent::DownloadCompleted, &ctx)
            .await;
        assert!(result.is_ok());
    }

    #[test]
    fn test_format_languages() {
        assert_eq!(DiscordNotifier::format_languages(&[]), "—");
        assert_eq!(
            DiscordNotifier::format_languages(&["eng".to_string()]),
            "eng"
        );
        assert_eq!(
            DiscordNotifier::format_languages(&["eng".to_string(), "jpn".to_string()]),
            "eng, jpn"
        );
    }
}
