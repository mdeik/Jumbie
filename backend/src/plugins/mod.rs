pub mod capabilities;
pub mod extract;
pub mod host;
pub mod internal;
pub mod manager;
pub mod methods;
pub mod policy;
pub mod registry;
pub mod sandbox;
pub mod signing;

// Internal (built-in) plugin categories: data models, concrete implementations,
// and orchestrators for each plugin category.
pub mod bridge;
pub mod downloaders;
pub mod metadata;
pub mod notifiers;
pub mod sources;

pub use host::{InstanceHandle, PluginTypeHost};
pub use manager::{PluginBuild, PluginManager, RuntimePluginState, RuntimePluginTypeInfo};
pub use policy::PolicyPlugin;
pub use registry::{InternalPluginRegistry, PluginFactory, SchemaFn};

use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;

#[derive(thiserror::Error, Debug)]
pub enum PluginCallError {
    #[error("Plugin call timed out")]
    Timeout,
    #[error("Plugin is overloaded with requests")]
    Overload,
    #[error("Plugin process died or is unreachable")]
    ProcessDead,
    #[error("Plugin returned an invalid response: {0}")]
    InvalidResponse(String),
    #[error("Plugin requested a retry after {0} seconds")]
    RetryAfter(u64),
    /// The plugin reached the remote service but authentication/credentials were
    /// rejected (e.g. HTTP 401/403 on a login endpoint).
    ///
    /// SSOT: this is the single classification plugins use to signal "do NOT
    /// retry — the same credentials will fail again". The backend's failure
    /// policy (`PolicyPlugin`) short-circuits these errors into an
    /// escalating cooldown instead of hammering the upstream service.
    #[error("Authentication failed: {0}")]
    AuthFailed(String),
    /// A transient failure — no handshake (connect/timeout/DNS/TLS) or a 5xx
    /// server error. Safe to retry with exponential backoff; if retries are
    /// exhausted the backend enters a transient cooldown.
    #[error("Transient error: {0}")]
    Transient(String),
    /// A deterministic failure that retrying will never fix — bad config,
    /// missing resource, unsupported operation. Surfaced immediately without
    /// retry or cooldown.
    #[error("Permanent error: {0}")]
    Permanent(String),
    #[error("Method {0} is not supported")]
    MethodNotSupported(String),
    #[error("Internal plugin error: {0}")]
    Internal(String),
    #[error("Plugin panicked: {0}")]
    PluginPanicked(String),
}

/// Central trait for all plugins — both internal (in-process Rust) and
/// external (subprocess over JSON-RPC).
///
/// Every metadata field has a getter AND a setter so the hot-reload path
/// (`reconfigure_all_plugins`) can update any field without restarting.
/// The defaults are safe no-ops — only `PluginTypeHost` stores mutable state.
#[async_trait]
pub trait PluginInstance: Send + Sync {
    // Identity (immutable after construction)
    /// Backend-owned per-instance identifier (config instance key for internal
    /// plugins; canonical type id for external type-singletons).
    ///
    /// This is NOT the type identifier — type identity (`plugin_id`) is
    /// backend-derived and attached at the API boundary (see
    /// `jumbie_shared::plugin::PluginInstanceInfo`). Plugins never choose this
    /// value.
    fn instance_id(&self) -> &str;
    fn supported_protocols(&self) -> Option<&[String]>;

    // Priority (mutable via set_priority)
    fn priority(&self) -> i32;
    fn set_priority(&self, _new_priority: i32) {}

    // Enabled state (mutable via set_enabled)
    fn is_enabled(&self) -> bool {
        true
    }
    fn set_enabled(&self, _enabled: bool) {}

    // Refresh interval for polling sources (mutable via set_refresh_interval)
    fn refresh_interval(&self) -> Option<u64> {
        None
    }
    fn set_refresh_interval(&self, _minutes: Option<u64>) {}

    // Plugin metadata
    /// Returns the plugin's static metadata (name, version, capabilities, etc.).
    /// This replaces calling `call("get_info", None)` for the common case.
    fn plugin_info(&self) -> plugin_sdk::traits::PluginTypeInfo;

    // RPC dispatch
    /// Universal dispatch with default handling for standard methods.
    ///
    /// Default implementation handles:
    /// - `get_info` — returns `serde_json::to_value(self.plugin_info())`
    /// - `health_check` — calls `health_check_impl()`, returns `json!("ok")`
    /// - `test` — calls `test_impl()`, returns success message
    ///
    /// Plugins override `handle_custom_method()` for their specific methods.
    async fn call(&self, method: &str, params: Option<Value>) -> Result<Value> {
        match method {
            "get_info" => Ok(serde_json::to_value(self.plugin_info())?),
            "health_check" => {
                self.health_check_impl().await?;
                Ok(serde_json::json!("ok"))
            }
            "test" => {
                self.test_impl().await?;
                Ok(serde_json::json!(format!(
                    "Successfully connected to {}",
                    self.plugin_info().display_name
                )))
            }
            _ => self.handle_custom_method(method, params).await,
        }
    }

    /// Override this to handle plugin-specific methods.
    /// Default returns `MethodNotSupported`.
    async fn handle_custom_method(&self, method: &str, _params: Option<Value>) -> Result<Value> {
        Err(anyhow::anyhow!(PluginCallError::MethodNotSupported(
            method.to_string()
        )))
    }

    /// Override for real health-check logic (e.g., ping a subprocess).
    async fn health_check_impl(&self) -> Result<()> {
        Ok(())
    }

    /// Override for real connectivity test logic.
    async fn test_impl(&self) -> Result<()> {
        Ok(())
    }

    /// In-process process/type health (no RPC). External instances report the
    /// type host's last probe result; internal instances are healthy unless a
    /// failure policy state says otherwise.
    fn is_healthy(&self) -> bool {
        true
    }

    /// Human-readable health status for the status page: `(ok, message)`.
    ///
    /// Default: in-process health only. `PolicyPlugin` overrides this to also
    /// surface failure-cooldown/429-pause state — the status page reads it
    /// directly (zero RPCs to plugin processes).
    async fn health_status(&self) -> (bool, Option<String>) {
        (self.is_healthy(), None)
    }

    /// Apply new configuration to a running plugin without restarting.
    ///
    /// Config is INPUT: this replaces the instance's config in place. It is
    /// infallible-by-contract for schema-validated configs (the API boundary
    /// rejects invalid ones first); an `Err` here marks the instance Failed
    /// rather than triggering a factory rebuild (a rebuild would fail with the
    /// same config anyway). Plugins that cannot apply in place (the default
    /// returns `MethodNotSupported`) surface as Failed on config change.
    async fn set_config(&self, _config: Value) -> Result<Value> {
        Err(anyhow::anyhow!(PluginCallError::MethodNotSupported(
            "set_config".to_string()
        )))
    }
}

#[cfg(test)]
#[path = "tests/host.rs"]
mod host_tests;

#[cfg(test)]
#[path = "tests/manager.rs"]
mod manager_tests;

#[cfg(test)]
#[path = "tests/health.rs"]
mod health_tests;

#[cfg(test)]
#[path = "tests/status.rs"]
mod status_tests;

/// Derive a unique plugin instance key in `author.plugin_name` format.
///
/// SSoT: Both `get_available_plugins` (sets the `plugin_id` exposed to the
/// frontend) and `build_internal_plugins` (looks up config entries by that
/// key) call this function so the format lives in exactly one place.
///
/// Built-in (first-party) plugins are namespaced under
/// `jumbie.{name}` — their author is [`jumbie_shared::plugin::JUMBIE_AUTHOR`].
/// External plugins use their declared author (lowercased, spaces → underscores).
/// Legacy internal authors (`"system"`, `"internal"`) also map to the Jumbie
/// namespace so old config keys keep resolving to the same plugin type.
pub fn derive_plugin_id(author: &str, short_name: &str) -> String {
    let is_builtin_author = author.is_empty()
        || author.eq_ignore_ascii_case("system")
        || author.eq_ignore_ascii_case("internal")
        || author.eq_ignore_ascii_case(jumbie_shared::plugin::JUMBIE_AUTHOR);

    if is_builtin_author {
        format!(
            "{}.{}",
            jumbie_shared::plugin::JUMBIE_PLUGIN_NAMESPACE,
            short_name
        )
    } else {
        format!("{}.{}", author.to_lowercase().replace(' ', "_"), short_name)
    }
}

/// Sort plugin status entries into their canonical display order, in place.
///
/// Primary key is [`jumbie_shared::plugin::plugin_category_rank`] (Sources,
/// Metadata, Downloaders, Notifiers); ties fall back to the same display order
/// `/api/plugins/available` gives the "Add New Plugin" modal
/// ([`jumbie_shared::plugin::PluginDisplaySortKey`]: case-insensitive name, then
/// type id), and finally the instance id so multiple instances of one type stay
/// stable. The modal's built-in-first tier is omitted: it is constant within
/// every category here (external types are always reported under the `"plugin"`
/// category, never a built-in one), so it cannot reorder anything.
///
/// Keys are built once per entry (`sort_by_cached_key`), so the lowercasing and
/// type-id lookups are O(n) rather than O(n log n).
///
/// `type_id_of` resolves an instance id to its canonical type id. It is a
/// callback so the ordering can be unit-tested without a live plugin manager.
pub fn sort_plugin_statuses(
    statuses: &mut [RuntimePluginTypeInfo],
    type_id_of: impl Fn(&str) -> Option<String>,
) {
    statuses.sort_by_cached_key(|info| {
        (
            jumbie_shared::plugin::plugin_category_rank(&info.category),
            jumbie_shared::plugin::PluginDisplaySortKey::new(
                &info.name,
                type_id_of(&info.id).as_deref().unwrap_or(""),
            ),
            info.id.clone(),
        )
    });
}

// refresh_interval unit SSoT
//
// `refresh_interval` is a plain integer whose UNIT depends on the plugin
// category. Metadata providers store HOURS; every other category stores
// MINUTES. The settings schema advertises the same unit via `x-time-unit`, so
// the schema injector and the background loops must agree on it — this pair of
// helpers is that single source of truth. Change the unit here, not in one of
// the call sites.

/// Storage unit for `refresh_interval` on `category` plugins (schema `x-time-unit`).
pub fn refresh_interval_unit(category: &str) -> &'static str {
    if category == "metadata" {
        "hours"
    } else {
        "minutes"
    }
}

/// Default `refresh_interval` for `category` plugins, expressed in that
/// category's native unit (see [`refresh_interval_unit`]).
pub fn refresh_interval_default(category: &str) -> u64 {
    if category == "metadata" { 12 } else { 10 }
}
