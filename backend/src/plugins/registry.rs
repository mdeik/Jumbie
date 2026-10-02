use super::PluginInstance;
use anyhow::{Result, bail};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

// Internal Plugin Registry
//
// A static registry of plugin factories using LazyLock + RwLock — the central
// directory where all built-in plugins (downloaders, notifiers, sources,
// metadata providers) are registered at startup. Static globals avoid threading
// a Registry reference through every configuration layer that creates plugins.
//
// The three separate registries exist because factories create instances at
// runtime, schemas define the JSON config format for the UI's editor, and info
// provides static metadata without instantiating the plugin — so the UI can list
// plugins and show their config forms without loading them.

pub type PluginFactory = fn(
    Value,
    String,
    i32,
    Option<u64>,
    Arc<jumbie_shared::config::Config>,
    CancellationToken,
) -> Result<Arc<dyn PluginInstance>>;
/// A pure function that returns a plugin's JSON Schema with no side-effects.
pub type SchemaFn = fn() -> Value;
/// A pure function that returns a plugin's PluginTypeInfo.
pub type InfoFn = fn() -> jumbie_shared::plugin::PluginTypeInfo;

// LazyLock ensures the registries are initialized exactly once, on first access.
// This is safe because registration happens during the single-threaded startup
// phase, and all subsequent access is read-only.
static REGISTRY: LazyLock<RwLock<HashMap<String, PluginFactory>>> =
    LazyLock::new(|| RwLock::new(HashMap::<String, PluginFactory>::new()));
static SCHEMA_REGISTRY: LazyLock<RwLock<HashMap<String, SchemaFn>>> =
    LazyLock::new(|| RwLock::new(HashMap::<String, SchemaFn>::new()));
static INFO_REGISTRY: LazyLock<RwLock<HashMap<String, InfoFn>>> =
    LazyLock::new(|| RwLock::new(HashMap::<String, InfoFn>::new()));

/// Default `PluginTypeInfo` for plugins that don't have a custom info function.
fn default_plugin_info(plugin_type: &str) -> jumbie_shared::plugin::PluginTypeInfo {
    jumbie_shared::plugin::PluginTypeInfo {
        display_name: plugin_type.to_string(),
        version: "unknown".to_string(),
        author: jumbie_shared::plugin::JUMBIE_AUTHOR.to_string(),
        description: String::new(),
        capabilities: vec![],
        supported_protocols: None,
        series_identifier_label: None,
        series_identifier_placeholder: None,
        rate_limit: None,
        supports_test: false,
    }
}

pub struct InternalPluginRegistry;

impl InternalPluginRegistry {
    /// Register a factory, a static schema function, and a plugin info function
    /// together.
    ///
    /// The three registries are written independently (not under a single lock)
    /// to avoid readers blocking on writers when only one is needed. Registration
    /// happens at single-threaded startup, so no reader can observe a partially-
    /// registered plugin.
    pub async fn register_full(
        key: &str,
        factory: PluginFactory,
        schema_fn: SchemaFn,
        info_fn: InfoFn,
    ) {
        {
            let mut reg = REGISTRY.write().await;
            reg.insert(key.to_string(), factory);
        }
        {
            let mut sreg = SCHEMA_REGISTRY.write().await;
            sreg.insert(key.to_string(), schema_fn);
        }
        {
            let mut ireg = INFO_REGISTRY.write().await;
            ireg.insert(key.to_string(), info_fn);
        }
    }

    /// Instantiate a plugin using the registered factory.
    ///
    /// The factory receives the raw config JSON, the instance ID (which allows
    /// multiple instances of the same plugin type, e.g., "primary" and "secondary"
    /// qBittorrent connections), the global config for HTTP client settings, and
    /// a CancellationToken so the plugin can respond to app-level shutdown signals.
    pub async fn create(
        key: &str,
        config: Value,
        instance_id: String,
        priority: i32,
        refresh_interval: Option<u64>,
        global_config: Arc<jumbie_shared::config::Config>,
        shutdown_token: CancellationToken,
    ) -> Result<Arc<dyn PluginInstance>> {
        let reg = REGISTRY.read().await;
        if let Some(factory) = reg.get(key) {
            factory(
                config,
                instance_id,
                priority,
                refresh_interval,
                global_config,
                shutdown_token,
            )
        } else {
            bail!("No internal plugin factory registered for key: {}", key)
        }
    }

    /// Return the JSON Schema for a plugin **by type name only** (e.g. `"qbittorrent"`).
    ///
    /// Tries all known category prefixes (`downloader`, `notifier`, `source`, `metadata`).
    /// Only works for plugins registered via `register_with_schema`.
    ///
    /// The category-agnostic lookup works because schema registration uses the
    /// full key (e.g., "downloader.qbittorrent"), but the UI only knows the
    /// short plugin name ("qbittorrent"). We brute-force through all four known
    /// categories to find it — cheap enough since the registry has < 20 entries.
    pub async fn get_schema(
        plugin_type: &str,
    ) -> Result<(Value, String, jumbie_shared::plugin::PluginTypeInfo)> {
        let categories = ["downloader", "notifier", "source", "metadata"];
        let sreg = SCHEMA_REGISTRY.read().await;
        let ireg = INFO_REGISTRY.read().await;

        for cat in &categories {
            let key = format!("{}.{}", cat, plugin_type);
            if let Some(schema_fn) = sreg.get(&key) {
                let info = ireg
                    .get(&key)
                    .map(|info_fn| info_fn())
                    .unwrap_or_else(|| default_plugin_info(plugin_type));
                return Ok((schema_fn(), cat.to_string(), info));
            }
        }

        bail!("No schema registered for plugin type '{}'", plugin_type)
    }

    /// Return all registered plugin infos along with their registry keys.
    ///
    /// Used by the PluginManager to enumerate available plugins during startup
    /// and by the UI status endpoint to show which plugin types are supported.
    pub async fn get_available_plugins() -> Vec<(String, jumbie_shared::plugin::PluginTypeInfo)> {
        let ireg = INFO_REGISTRY.read().await;
        ireg.iter()
            .map(|(k, info_fn)| (k.clone(), info_fn()))
            .collect()
    }

    /// Return schemas for all registered plugins.
    ///
    /// Used by the batch schema endpoint so the frontend can fetch all plugin
    /// schemas in a single round-trip instead of N individual requests.
    pub async fn get_all_schemas() -> Vec<(
        String,
        serde_json::Value,
        String,
        jumbie_shared::plugin::PluginTypeInfo,
    )> {
        let sreg = SCHEMA_REGISTRY.read().await;
        let ireg = INFO_REGISTRY.read().await;
        sreg.iter()
            .map(|(key, schema_fn)| {
                let schema = schema_fn();
                // key format is "category.plugin_type"
                let plugin_type = key.split('.').nth(1).unwrap_or(key).to_string();
                let category = key.split('.').next().unwrap_or("").to_string();
                let info = ireg
                    .get(key)
                    .map(|info_fn| info_fn())
                    .unwrap_or_else(|| default_plugin_info(&plugin_type));
                (plugin_type, schema, category, info)
            })
            .collect()
    }
}
