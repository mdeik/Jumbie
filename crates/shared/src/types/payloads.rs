use crate::config::Config;
use crate::config::plugins::PluginsConfig;
use crate::config::ui::UIConfig;
use crate::plugin::{PluginInstanceInfo, PluginTypeListing};
use crate::types::ReleaseDates;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Default for boolean serde fields that should be `true` when absent.
/// Shared across payloads so every `#[serde(default = "default_true")]`
/// references the same function — no duplication.
pub const fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateConfigPayload {
    /// Startup-path fields (database, plugins_dir, etc.) are NOT in this payload.
    /// They live in config.toml and only take effect after a restart.
    /// Only DB-managed sections can be updated through the API.
    pub organization: Option<crate::config::OrganizationConfig>,
    pub sources: Option<crate::config::SourcesConfig>,
    pub general: Option<crate::config::GeneralConfig>,
    pub proxy: Option<crate::config::ProxyConfig>,
    pub auth: Option<crate::config::AuthConfig>,
    pub security: Option<crate::config::SecurityConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestPluginPayload {
    pub category: String,
    pub plugin_type: String,
    pub config: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemediatePayload {
    pub action: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchPayload {
    pub query: String,
    pub mode: Option<String>,
    pub series_id: Option<String>,
    pub episode_id: Option<String>,
    /// Season string (e.g. "01", "02") — required for mode="season"
    pub season: Option<String>,
    /// Episode numbers to search for — required for mode="season"
    pub episode_numbers: Option<Vec<i32>>,
    /// When true (default), the search came from a user action (UI "Auto Search" /
    /// "Download"); at organize time it temporarily enables upgrade evaluation. The
    /// background wanted-episodes task sets it false via the `download_winner` path.
    #[serde(default = "default_true")]
    pub is_user_requested: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoSearchSeasonPayload {
    pub series_id: String,
    pub season: String,
    pub episode_numbers: Vec<i32>,
    /// When true (default), the season search came from a user action (the season
    /// accordion's "Auto Search"). Passed through to queue items so organize time
    /// treats them as user-requested.
    #[serde(default = "default_true")]
    pub is_user_requested: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteTorrentPayload {
    pub delete_files: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadMediaPayload {
    pub link: String,
    /// Unique download identifier from the source plugin (e.g. Nyaa's infoHash).
    /// Required — the system cannot track the download without it.
    pub download_id: String,
    pub category: Option<String>,
    pub episode_id: Option<String>,
    pub tag: Option<String>,
    pub title: Option<String>,
    pub score: Option<i32>,
    /// UUID FK — from modal context. Used to link manual downloads to their series.
    #[serde(default)]
    pub series_id: Option<String>,
    /// Explicit pack flag — from SearchResult. Never re-derived downstream.
    #[serde(default)]
    pub is_season_pack: Option<bool>,
    /// When true (default), the download was explicitly requested by the user; at
    /// organize time it temporarily enables upgrade evaluation for a fair comparison.
    /// The background wanted-episodes task sets it false via the `download_winner` path.
    #[serde(default = "default_true")]
    pub is_user_requested: bool,
    /// Scoring inputs from the search result, carried through so the
    /// age-based, size-based, and peers-based scoring components can be
    /// reconstructed on retroactive rescore.
    #[serde(default)]
    pub size: Option<u64>,
    #[serde(default)]
    pub seeders: Option<u32>,
    /// Source feed publish/upload date (ISO 8601).
    #[serde(default)]
    pub upload_date: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BanEntry {
    pub ip: String,
    pub fail_count: u32,
    pub ban_count: u32,
    pub banned_at: String,
    pub banned_until: Option<String>,
    pub is_permanent: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddBanPayload {
    pub ip: String,
    pub duration_seconds: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublicThemeResponse {
    pub theme: String,
}

/// Aggregated payload returned by GET /api/bootstrap.
/// Contains every read-only settings datum the frontend needs to render
/// the settings UI in a single round-trip.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootstrapData {
    pub config: Config,
    pub qualities: HashMap<String, crate::scoring::Quality>,
    pub quality_profiles: HashMap<String, crate::scoring::QualityProfile>,
    pub release_profiles: HashMap<String, crate::scoring::ReleaseProfile>,
    pub ui_preferences: UIConfig,
    pub plugins_cfg: PluginsConfig,
    /// Loaded INSTANCES (type metadata + backend-owned identity).
    pub plugins: Vec<PluginInstanceInfo>,
    /// Available TYPES (immutable type metadata + plugin_id).
    pub available_plugins: Vec<PluginTypeListing>,
    pub plugin_status: Vec<PluginStatusEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginStatusEntry {
    pub name: String,
    pub category: String,
    pub ok: bool,
    pub message: Option<String>,
}

/// Per-TYPE plugin process metrics (`/api/system/plugins-metrics`) — in-process
/// reads, no RPC to plugin processes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginTypeMetrics {
    pub type_id: String,
    pub display_name: String,
    pub healthy: bool,
    pub calls: u64,
    pub errors: u64,
    pub restarts: u64,
    pub queue_depth: u64,
}

/// Response wrapper for the section-level plugin save endpoint.
/// Carries the updated full config together with any warnings the backend
/// wants to surface (e.g. "Disabled Discord because webhook_url is empty").
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavePluginsSectionResponse {
    pub config: PluginsConfig,
    #[serde(default)]
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WantedEpisode {
    pub series_id: String,
    pub series_title: String,
    pub episode_id: String,
    pub season: Option<String>,
    pub episode: i32,
    pub title: Option<String>,
    pub eff_date: String, // server-resolved effective date (for age display)
    pub dates: ReleaseDates<String>,
    pub status: String,
}
