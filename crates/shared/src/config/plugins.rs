use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

// Individual notification plugin configs

// A typed struct rather than a flat HashMap: Discord webhook notifications have
// well-defined fields that benefit from compile-time checking.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct DiscordConfig {
    #[serde(default = "default_discord_name")]
    pub name: String,
    // serde default is true, but PluginConfig::default() below sets false:
    // notifiers need a webhook URL before they can work, so enabling by default
    // would cause confusing "missing webhook" errors.
    #[serde(default = "crate::config::default_true")]
    pub enabled: bool,
    pub webhook_url: String,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub avatar_url: Option<String>,
    #[serde(default)]
    pub events: NotifierEvents,
}

// A struct rather than a HashMap: the event set is fixed at compile time and each
// event carries its own default template.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct NotifierEvents {
    #[serde(default)]
    pub download_started: EventConfig,
    #[serde(default)]
    pub download_completed: EventConfig,
    #[serde(default)]
    pub organize_success: EventConfig,
    #[serde(default)]
    pub error: EventConfig,
}

impl Default for NotifierEvents {
    fn default() -> Self {
        Self {
            download_started: EventConfig {
                // Events start disabled to avoid notification spam on first install;
                // each has a default template the user can preview before enabling.
                enabled: false,
                template: "⏳ Started: ${series} - S${season:02}E${episode:02}".to_string(),
            },
            download_completed: EventConfig {
                enabled: false,
                template: "✅ Downloaded: ${series} - S${season:02}E${episode:02}".to_string(),
            },
            organize_success: EventConfig {
                enabled: false,
                template: "📁 Organized: ${series} - S${season:02}E${episode:02}".to_string(),
            },
            error: EventConfig {
                enabled: false,
                // ${context} tells the user what operation failed,
                // ${error} gives the actual error message — separating them
                // makes error notifications actionable.
                template: "❌ Error: ${context} - ${error}".to_string(),
            },
        }
    }
}

// Reused by every event type (template + enabled), so new event types can be added
// without modifying existing ones.
#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct EventConfig {
    #[serde(default = "crate::config::default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub template: String,
}

fn default_discord_name() -> String {
    "Discord".to_string()
}

impl Default for DiscordConfig {
    fn default() -> Self {
        // Delegates to PluginConfig::default() so defaults live in one place (SSoT).
        <Self as crate::plugin::PluginConfig>::default()
    }
}

impl crate::plugin::PluginConfig for DiscordConfig {
    fn plugin_id() -> &'static str {
        "discord"
    }

    fn default() -> Self {
        Self {
            name: default_discord_name(),
            enabled: false, // Disabled by default — requires user setup
            webhook_url: "".to_string(),
            username: Some("Jumbie".to_string()),
            avatar_url: None,
            events: NotifierEvents::default(),
        }
    }

    fn validate(&self) -> Result<(), String> {
        // This struct is dead code — the real Discord notifier lives in
        // backend/src/notifiers/discord.rs and validates inline.
        Ok(())
    }

    fn is_enabled(&self) -> bool {
        self.enabled
    }
}

// Nyaa.si is a specific indexer with its own category/filter conventions that
// don't generalize to other torrent sources, so it gets its own struct.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
pub struct NyaaConfig {
    #[serde(default = "default_nyaa_name")]
    pub name: String,
    #[serde(default = "crate::config::default_true")]
    pub enabled: bool,
    #[serde(default = "default_nyaa_base_url")]
    pub base_url: String,
    // Nyaa's category system is hierarchical (parent_child), e.g. "1_2" =
    // Anime - English translated; the raw format is preserved.
    #[serde(default = "default_nyaa_category")]
    pub category: String, // e.g. "1_2" for Anime - English
    // "0" = no filter, "1" = no remakes, "2" = trusted only. Default maximizes results.
    #[serde(default = "default_nyaa_filter")]
    pub filter: String, // "0", "1", "2"
    // All three default true: Nyaa is the primary anime source, so limiting it by
    // default would surprise users who expect it to work out of the box.
    #[serde(default = "crate::config::default_true")]
    pub enable_polling: bool,
    #[serde(default = "crate::config::default_true")]
    pub enable_automatic_search: bool,
    #[serde(default = "crate::config::default_true")]
    pub enable_manual_search: bool,
    // Per-plugin polling cadence in minutes.
    #[serde(default = "default_refresh_interval")]
    pub refresh_interval: u64,
}

fn default_nyaa_name() -> String {
    "Nyaa".to_string()
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

fn default_refresh_interval() -> u64 {
    10
}

impl Default for NyaaConfig {
    fn default() -> Self {
        Self {
            name: default_nyaa_name(),
            enabled: true,
            base_url: default_nyaa_base_url(),
            category: default_nyaa_category(),
            filter: default_nyaa_filter(),
            enable_polling: true,
            enable_automatic_search: true,
            enable_manual_search: true,
            refresh_interval: default_refresh_interval(),
        }
    }
}

impl crate::plugin::PluginConfig for NyaaConfig {
    fn plugin_id() -> &'static str {
        "nyaa"
    }
    fn default() -> Self {
        <Self as Default>::default()
    }
    fn is_enabled(&self) -> bool {
        self.enabled
    }
}

// The most generic source: just a URL and refresh interval, unlike Nyaa's
// categories/filters, so the two configs stay separate.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
pub struct BasicRssConfig {
    #[serde(default = "default_rss_name")]
    pub name: String,
    #[serde(default = "crate::config::default_true")]
    pub enabled: bool,
    // Defaults to "" — there's no universal RSS feed, so the user must set it.
    #[serde(default = "default_rss_url")]
    pub url: String,
    #[serde(default = "crate::config::default_true")]
    pub enable_polling: bool,
    #[serde(default = "crate::config::default_true")]
    pub enable_automatic_search: bool,
    // No enable_interactive_search: a raw RSS feed has no search API (push-only).
    #[serde(default = "default_refresh_interval")]
    pub refresh_interval: u64,
}

fn default_rss_name() -> String {
    "Basic RSS".to_string()
}
fn default_rss_url() -> String {
    "".to_string()
}

impl Default for BasicRssConfig {
    fn default() -> Self {
        Self {
            name: default_rss_name(),
            enabled: true,
            url: default_rss_url(),
            enable_polling: true,
            enable_automatic_search: true,
            refresh_interval: default_refresh_interval(),
        }
    }
}

impl crate::plugin::PluginConfig for BasicRssConfig {
    fn plugin_id() -> &'static str {
        "basic_rss"
    }
    fn default() -> Self {
        <Self as Default>::default()
    }
    fn is_enabled(&self) -> bool {
        self.enabled
    }
}

// Master plugin registry. Nested HashMaps (category → plugin_name → raw JSON
// config) because external plugins can't have their config types defined at
// compile time here; `serde_json::Value` forwards any shape to the plugin loader.
#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct PluginsConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub directory: PathBuf,

    // Keyed by plugin name. The four categories map to the four plugin traits;
    // keeping them separate prevents a downloader being treated as a notifier.
    #[serde(default)]
    pub downloader: HashMap<String, HashMap<String, serde_json::Value>>,

    #[serde(default)]
    pub notifier: HashMap<String, HashMap<String, serde_json::Value>>,

    #[serde(default)]
    pub source: HashMap<String, HashMap<String, serde_json::Value>>,

    #[serde(default)]
    pub metadata: HashMap<String, HashMap<String, serde_json::Value>>,
}

/// Whether a raw JSON plugin instance config is enabled.
///
/// A missing `enabled` field counts as disabled: the plugin schema fills the
/// real default (`true`) before a config reaches the loader, and the metadata
/// single-active policy must never treat an absent flag as "on".
pub fn instance_is_enabled(config: &serde_json::Value) -> bool {
    config
        .get("enabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

/// Ensure at most one metadata plugin instance is enabled.
///
/// SSoT for the "only one metadata provider active at a time" policy, shared by
/// the backend endpoints and the frontend settings UI so the rule (and which
/// instance survives) can't drift.
///
/// When more than one instance is enabled, all but `preferred` are disabled.
/// `preferred` is the instance the user just acted on; if it isn't enabled, or no
/// preference is given, the first in deterministic `(plugin_key, instance_id)`
/// order wins — independent of `HashMap` iteration order.
///
/// Returns the `(plugin_key, instance_id)` pairs that were disabled.
pub fn ensure_single_metadata_plugin(
    cfg: &mut PluginsConfig,
    preferred: Option<(&str, &str)>,
) -> Vec<(String, String)> {
    let mut enabled: Vec<(String, String)> = cfg
        .metadata
        .iter()
        .flat_map(|(plugin_key, instances)| {
            instances
                .iter()
                .filter(|(_, config)| instance_is_enabled(config))
                .map(move |(instance_id, _)| (plugin_key.clone(), instance_id.clone()))
        })
        .collect();
    enabled.sort();

    if enabled.len() <= 1 {
        return Vec::new();
    }

    let winner = preferred
        .filter(|(pk, iid)| enabled.iter().any(|(p, i)| p == pk && i == iid))
        .map(|(pk, iid)| (pk.to_string(), iid.to_string()))
        .unwrap_or_else(|| enabled[0].clone());

    let mut disabled = Vec::new();
    for (plugin_key, instance_id) in enabled {
        if plugin_key == winner.0 && instance_id == winner.1 {
            continue;
        }
        if let Some(instances) = cfg.metadata.get_mut(&plugin_key)
            && let Some(config) = instances.get_mut(&instance_id)
            && let Some(obj) = config.as_object_mut()
        {
            obj.insert("enabled".to_string(), serde_json::Value::Bool(false));
            disabled.push((plugin_key, instance_id));
        }
    }

    disabled
}

/// What happens when a season pack torrent is downloaded and matches existing
/// individual episodes (or vice versa). "Favor" means "prefer to keep", not
/// "delete the other".
#[derive(Debug, Default, Deserialize, Serialize, Clone, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum SeasonPackStrategy {
    #[default]
    FavorEpisodes,
    FavorSeasonPacks,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Build a metadata-only config from `(plugin_key, instance_id, enabled)`.
    fn metadata_cfg(instances: &[(&str, &str, bool)]) -> PluginsConfig {
        let mut cfg = PluginsConfig::default();
        for (plugin_key, instance_id, enabled) in instances {
            cfg.metadata
                .entry(plugin_key.to_string())
                .or_default()
                .insert(instance_id.to_string(), json!({ "enabled": enabled }));
        }
        cfg
    }

    /// Sorted `(plugin_key, instance_id)` pairs that are enabled.
    fn enabled_pairs(cfg: &PluginsConfig) -> Vec<(String, String)> {
        let mut pairs: Vec<(String, String)> = cfg
            .metadata
            .iter()
            .flat_map(|(plugin_key, instances)| {
                instances
                    .iter()
                    .filter(|(_, config)| instance_is_enabled(config))
                    .map(move |(instance_id, _)| (plugin_key.clone(), instance_id.clone()))
            })
            .collect();
        pairs.sort();
        pairs
    }

    #[test]
    fn single_enabled_instance_is_a_noop() {
        let mut cfg = metadata_cfg(&[
            ("metadata.tvmaze", "a", true),
            ("metadata.tvdb", "b", false),
        ]);
        let disabled = ensure_single_metadata_plugin(&mut cfg, None);
        assert!(disabled.is_empty());
        assert_eq!(
            enabled_pairs(&cfg),
            vec![("metadata.tvmaze".into(), "a".into())]
        );
    }

    #[test]
    fn no_instances_is_a_noop() {
        let mut cfg = PluginsConfig::default();
        assert!(ensure_single_metadata_plugin(&mut cfg, None).is_empty());
    }

    #[test]
    fn keeps_preferred_and_disables_the_rest() {
        let mut cfg = metadata_cfg(&[
            ("metadata.tvmaze", "a", true),
            ("metadata.tvdb", "b", true),
            ("metadata.tvdb", "c", true),
        ]);
        let disabled = ensure_single_metadata_plugin(&mut cfg, Some(("metadata.tvdb", "b")));
        assert_eq!(
            disabled,
            vec![
                ("metadata.tvdb".to_string(), "c".to_string()),
                ("metadata.tvmaze".to_string(), "a".to_string()),
            ]
        );
        assert_eq!(
            enabled_pairs(&cfg),
            vec![("metadata.tvdb".into(), "b".into())]
        );
    }

    #[test]
    fn preferred_that_is_disabled_falls_back_to_first() {
        let mut cfg = metadata_cfg(&[("metadata.tvmaze", "a", true), ("metadata.tvdb", "b", true)]);
        // "z" is not in the config; the deterministic first wins. `metadata.tvdb`
        // sorts before `metadata.tvmaze`, so instance b wins.
        let disabled = ensure_single_metadata_plugin(&mut cfg, Some(("metadata.tvdb", "z")));
        assert_eq!(
            disabled,
            vec![("metadata.tvmaze".to_string(), "a".to_string())]
        );
        assert_eq!(
            enabled_pairs(&cfg),
            vec![("metadata.tvdb".into(), "b".into())]
        );
    }

    #[test]
    fn missing_enabled_field_counts_as_disabled() {
        let mut cfg = PluginsConfig::default();
        cfg.metadata.insert(
            "metadata.tvmaze".into(),
            [("a".to_string(), json!({}))].into_iter().collect(),
        );
        cfg.metadata.insert(
            "metadata.tvdb".into(),
            [("b".to_string(), json!({ "enabled": true }))]
                .into_iter()
                .collect(),
        );
        let disabled = ensure_single_metadata_plugin(&mut cfg, None);
        assert!(disabled.is_empty(), "only one instance is enabled");
        assert_eq!(
            enabled_pairs(&cfg),
            vec![("metadata.tvdb".into(), "b".into())]
        );
    }

    #[test]
    fn deterministic_winner_without_preference_is_sorted_first() {
        // No preference: the lowest `(plugin_key, instance_id)` wins regardless of
        // insertion order (`metadata.tvdb` sorts before `metadata.tvmaze`).
        let mut cfg = PluginsConfig::default();
        cfg.metadata.insert(
            "metadata.tvmaze".into(),
            [("aaa".to_string(), json!({ "enabled": true }))]
                .into_iter()
                .collect(),
        );
        cfg.metadata.insert(
            "metadata.tvdb".into(),
            [("zzz".to_string(), json!({ "enabled": true }))]
                .into_iter()
                .collect(),
        );
        let disabled = ensure_single_metadata_plugin(&mut cfg, None);
        assert_eq!(
            disabled,
            vec![("metadata.tvmaze".to_string(), "aaa".to_string())]
        );
        assert_eq!(
            enabled_pairs(&cfg),
            vec![("metadata.tvdb".into(), "zzz".into())]
        );
    }

    #[test]
    fn disabled_instances_are_untouched() {
        let mut cfg = metadata_cfg(&[
            ("metadata.tvmaze", "a", false),
            ("metadata.tvdb", "b", true),
            ("metadata.tvdb", "c", true),
        ]);
        let disabled = ensure_single_metadata_plugin(&mut cfg, Some(("metadata.tvdb", "c")));
        assert_eq!(
            disabled,
            vec![("metadata.tvdb".to_string(), "b".to_string())]
        );
        // The already-disabled instance keeps its disabled state (and is not reported).
        assert!(!instance_is_enabled(&cfg.metadata["metadata.tvmaze"]["a"]));
        assert_eq!(
            enabled_pairs(&cfg),
            vec![("metadata.tvdb".into(), "c".into())]
        );
    }
}
