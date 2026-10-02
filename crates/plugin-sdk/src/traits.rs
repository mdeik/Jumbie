use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    FeedProvider,
    Downloader,
    Notifier,
    MetadataProvider,
    /// Metadata provider that supports normal (season-relative) episode numbering.
    MetadataProviderNormal,
    /// Metadata provider that supports absolute episode numbering.
    MetadataProviderAbsolute,
    /// Source supports periodic feed polling (fetch_entries)
    Polling,
    /// Source supports manual/searching from the UI (search)
    ManualSearch,
    /// Source supports automatic missing episode search (auto_search)
    AutomaticSearch,
    /// Download client supports pause/resume operations for active downloads.
    CanPauseResume,
    /// Metadata provider can fetch the canonical series title from an online source.
    FetchSeriesTitle,
    /// Metadata provider can fetch alternative titles (aliases) for a series.
    FetchSeriesAliases,
    /// Download client supports seeding (i.e., can keep files seeded after download
    /// and report seeding stats like ratio and seed time).
    /// DDL (Direct Download) clients should NOT advertise this capability.
    CanSeed,
}

/// Convert a string into a plugin slug for use in identifiers and routes.
///
/// - Lowercases, replaces non-alphanumeric with `_`, collapses consecutive
///   underscores, strips leading/trailing ones.
///   Example: `"Jumbie"` → `"jumbie"`
pub fn to_plugin_slug(s: &str) -> String {
    let raw: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .fold(String::new(), |mut acc, c| {
            if c != '_' || !acc.ends_with('_') {
                acc.push(c);
            }
            acc
        });
    raw.trim_matches('_').to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RateLimit {
    pub requests_per_minute: u32,
    pub burst: u32,
}

/// Type-level metadata returned by a plugin's `get_info` RPC call.
///
/// IMMUTABLE and identical for every instance of a type: capabilities,
/// protocols, rate limit, identifier labels — none of it varies per instance.
/// Backend-owned identity (`plugin_id` / `instance_id`) is NOT part of this
/// struct; the API boundary attaches it as instance records (see
/// `jumbie_shared::plugin::PluginInstanceInfo`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginTypeInfo {
    pub display_name: String,
    pub version: String,
    pub author: String,
    pub description: String,
    pub capabilities: Vec<Capability>,
    pub supported_protocols: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub series_identifier_label: Option<String>,
    /// Human-readable placeholder text for the ID/slug input field shown in the UI.
    /// Example: "e.g. 121361", "e.g. 1".
    /// When None, no placeholder is shown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub series_identifier_placeholder: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate_limit: Option<RateLimit>,
    /// Whether the plugin implements an optional `test` method for verifying
    /// connectivity with the external service (e.g. checking API credentials,
    /// testing webhook URLs, verifying server reachability).
    /// When true, the UI will show a "Test" button in the plugin config editor.
    #[serde(default)]
    pub supports_test: bool,
}

impl PluginTypeInfo {
    /// Derives the canonical plugin ID from `author` and the user-facing
    /// `display_name`.
    ///
    /// Format: `{author_slug}.{display_name_slug}`
    ///
    /// If either field is missing, empty, whitespace-only, or yields an empty slug after
    /// cleaning, the following fallbacks apply:
    /// - `author`       → `"unknown"`   (semantically accurate — we genuinely don't know)
    /// - `display_name` → `"unnamed"`   (more informative than crashing or silently
    ///   producing a malformed ID like `"author."`)
    ///
    /// Example: author `"Jumbie"`, name `"Dummy Rust"` → `"jumbie.dummy_rust"`
    pub fn derived_id(&self) -> String {
        let author_slug = {
            let s = to_plugin_slug(self.author.trim());
            if s.is_empty() {
                "unknown".to_string()
            } else {
                s
            }
        };
        let name_slug = {
            let s = to_plugin_slug(self.display_name.trim());
            if s.is_empty() {
                "unnamed".to_string()
            } else {
                s
            }
        };
        format!("{}.{}", author_slug, name_slug)
    }
}

/// Per-call context passed to [`PluginHandler::handle`].
///
/// This is the "config as input" model: the handler is ONE object per process
/// (never per-instance) and receives the instance's current config on every
/// call, plus a per-instance derived-state cache. There is no `reconfigure`
/// logic — when the host pushes a new config the cache is cleared automatically,
/// so any memoized derived state (tokens, sessions, transports) simply
/// re-derives on the next call.
pub struct CallContext<'a> {
    /// Which instance this call targets (the host's config instance key).
    pub instance_id: &'a str,
    /// The instance's current config (schema-validated at the API boundary).
    pub config: &'a Value,
    /// The custom method being invoked (capability-specific, e.g. "search").
    pub method: &'a str,
    /// The method's request parameters.
    pub params: Value,
    /// Per-instance derived-state cache. Cleared automatically whenever
    /// `set_config` replaces the config — never write invalidation logic.
    pub cache: &'a mut std::collections::HashMap<String, Value>,
}

/// One handler object per plugin PROCESS — never per instance.
///
/// Instances are just configs: the handler receives each instance's config via
/// [`CallContext`] and processes it. Type-level metadata (`get_info`, schema,
/// validate) is declared once and never varies per instance.
#[async_trait]
pub trait PluginHandler: Send + Sync {
    /// Type-level metadata (same for every instance of this plugin).
    async fn get_info(&self) -> PluginTypeInfo;

    /// Returns a JSON Schema representing the required configuration
    async fn get_config_schema(&self) -> Value;

    /// Validates the provided configuration against the schema and any custom logic.
    /// Returns Ok(()) on success, or a list of specific error strings on failure.
    async fn validate_config(&self, config: Value) -> Result<(), Vec<String>>;

    /// Type-level health check (process liveness + plugin health). Called once
    /// per type by the host's probe — never per instance.
    async fn health_check(&self) -> Result<(), anyhow::Error>;

    /// Process one instance-level method call: `(config, params) → result`.
    ///
    /// Implementations are pure with respect to config: read `ctx.config`,
    /// use `ctx.cache` for memoized derived state, return the result.
    async fn handle(&self, ctx: CallContext<'_>) -> Result<Value, anyhow::Error>;
}
