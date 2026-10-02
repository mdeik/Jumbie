//! Plugin configuration management.
//!
//! The `PluginConfig` trait is the static-config layer (parsed and validated at
//! startup, before any plugin process spawns) alongside the runtime RPC system.
//! `default()` builds a full config when the plugin section is missing;
//! `merge_with_defaults()` fills gaps in a partial section (its default impl just
//! clones, so plugins without merge needs don't implement it).

use serde::{Deserialize, Serialize, de::DeserializeOwned};

pub use plugin_sdk::traits::{Capability, PluginTypeInfo, RateLimit};

// API boundary DTOs: the frontend consumes flat objects (type metadata +
// backend-owned identity), so these mirror every `PluginTypeInfo` field and are
// constructed from it via `From` (the SSoT).

/// The ten `PluginTypeInfo` fields as a tuple (used by the flat DTO `From`
/// impls — keeps the field list in one place).
type TypeInfoFields = (
    String,
    String,
    String,
    String,
    Vec<Capability>,
    Option<Vec<String>>,
    Option<String>,
    Option<String>,
    Option<RateLimit>,
    bool,
);

fn type_info_fields(info: &PluginTypeInfo) -> TypeInfoFields {
    (
        info.display_name.clone(),
        info.version.clone(),
        info.author.clone(),
        info.description.clone(),
        info.capabilities.clone(),
        info.supported_protocols.clone(),
        info.series_identifier_label.clone(),
        info.series_identifier_placeholder.clone(),
        info.rate_limit.clone(),
        info.supports_test,
    )
}

/// `/api/plugins` — one entry per INSTANCE: immutable type metadata plus the
/// backend-owned `plugin_id` (type id) and `instance_id`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginInstanceInfo {
    pub display_name: String,
    pub version: String,
    pub author: String,
    pub description: String,
    pub capabilities: Vec<Capability>,
    pub supported_protocols: Option<Vec<String>>,
    pub series_identifier_label: Option<String>,
    pub series_identifier_placeholder: Option<String>,
    pub rate_limit: Option<RateLimit>,
    pub supports_test: bool,
    /// Backend-derived type id, e.g. `"jumbie.tvdb"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugin_id: Option<String>,
    /// Backend-owned instance key (config instance id).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instance_id: Option<String>,
}

impl From<PluginTypeInfo> for PluginInstanceInfo {
    fn from(info: PluginTypeInfo) -> Self {
        let (
            display_name,
            version,
            author,
            description,
            capabilities,
            supported_protocols,
            series_identifier_label,
            series_identifier_placeholder,
            rate_limit,
            supports_test,
        ) = type_info_fields(&info);
        Self {
            display_name,
            version,
            author,
            description,
            capabilities,
            supported_protocols,
            series_identifier_label,
            series_identifier_placeholder,
            rate_limit,
            supports_test,
            plugin_id: None,
            instance_id: None,
        }
    }
}

/// `/api/plugins/available` — one entry per TYPE: immutable type metadata plus
/// the backend-derived `plugin_id`. Never carries an instance id.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginTypeListing {
    pub display_name: String,
    pub version: String,
    pub author: String,
    pub description: String,
    pub capabilities: Vec<Capability>,
    pub supported_protocols: Option<Vec<String>>,
    pub series_identifier_label: Option<String>,
    pub series_identifier_placeholder: Option<String>,
    pub rate_limit: Option<RateLimit>,
    pub supports_test: bool,
    /// Backend-derived type id, e.g. `"jumbie.tvdb"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugin_id: Option<String>,
}

impl From<PluginTypeInfo> for PluginTypeListing {
    fn from(info: PluginTypeInfo) -> Self {
        let (
            display_name,
            version,
            author,
            description,
            capabilities,
            supported_protocols,
            series_identifier_label,
            series_identifier_placeholder,
            rate_limit,
            supports_test,
        ) = type_info_fields(&info);
        Self {
            display_name,
            version,
            author,
            description,
            capabilities,
            supported_protocols,
            series_identifier_label,
            series_identifier_placeholder,
            rate_limit,
            supports_test,
            plugin_id: None,
        }
    }
}

/// Extract the short/type name from a plugin ID in `author.name` format.
///
/// SSoT: mirrors the format created by `derive_plugin_id` (backend).
/// The type key is the component after the last `.`.
/// Returns the full string when no `.` is present.
///
/// ```
/// # use jumbie_shared::plugin::plugin_type_key;
/// assert_eq!(plugin_type_key("jumbie.tvdb"), "tvdb");
/// assert_eq!(plugin_type_key("downloader.qbittorrent"), "qbittorrent");
/// assert_eq!(plugin_type_key("simple"), "simple");
/// ```
pub fn plugin_type_key(id: &str) -> &str {
    id.rsplit('.').next().unwrap_or(id)
}

/// Author declared by Jumbie's built-in (compiled-in) plugins.
///
/// This is the first-party author name — external plugin authors must not use
/// it (see `jumbie_shared::validation::fields::validate_plugin_info`).
/// SSoT: shared by the backend's plugin-identity derivation and the UI, which
/// uses it to distinguish built-in plugins from user-installed ones.
pub const JUMBIE_AUTHOR: &str = "Jumbie";

/// Namespace prefix of built-in plugin type ids (`jumbie.{short_name}`).
///
/// Built-in plugins are namespaced under `jumbie` so they are distinguishable
/// from external plugins (which use their declared author as the namespace).
pub const JUMBIE_PLUGIN_NAMESPACE: &str = "jumbie";

/// Returns `true` when `plugin_id` belongs to a built-in (first-party) plugin.
///
/// Built-in type ids are namespaced under [`JUMBIE_PLUGIN_NAMESPACE`], e.g.
/// `jumbie.tvdb`. Requires an exact `jumbie.` prefix so an external author such
/// as `jumbiex` is not mistaken for a built-in.
///
/// ```
/// # use jumbie_shared::plugin::is_jumbie_plugin_id;
/// assert!(is_jumbie_plugin_id("jumbie.tvdb"));
/// assert!(!is_jumbie_plugin_id("jumbiex.tvdb"));
/// assert!(!is_jumbie_plugin_id("someauthor.tvdb"));
/// ```
pub fn is_jumbie_plugin_id(plugin_id: &str) -> bool {
    plugin_id
        .strip_prefix(JUMBIE_PLUGIN_NAMESPACE)
        .is_some_and(|rest| rest.starts_with('.'))
}

/// Canonical display order for plugin categories: Sources, Metadata,
/// Downloaders, Notifiers.
///
/// SSoT for anything that lists plugin types or instances (the Plugins nav group
/// and the plugin status sort both follow this order). Entries are the raw
/// category ids the backend derives from registry keys (`category.plugin_type`).
pub const PLUGIN_CATEGORY_ORDER: &[&str] = &["source", "metadata", "downloader", "notifier"];

/// Rank of `category` within [`PLUGIN_CATEGORY_ORDER`], suitable as a sort key.
///
/// Unknown categories rank last (`u8::MAX`).
///
/// ```
/// # use jumbie_shared::plugin::plugin_category_rank;
/// assert_eq!(plugin_category_rank("source"), 0);
/// assert!(plugin_category_rank("metadata") < plugin_category_rank("notifier"));
/// assert_eq!(plugin_category_rank("something-else"), u8::MAX);
/// ```
pub fn plugin_category_rank(category: &str) -> u8 {
    PLUGIN_CATEGORY_ORDER
        .iter()
        .position(|c| *c == category)
        .map_or(u8::MAX, |i| i as u8)
}

/// Ordering key for listing plugins by user-facing identity: case-insensitive
/// display name, then canonical type id (`plugin_id`).
///
/// SSoT for the tie-break shared by `/api/plugins/available` (the "Add New
/// Plugin" modal) and `/api/plugins/status`, so the two listings cannot drift
/// apart. Building the key lowercases the name once, so callers can sort with
/// `sort_by_cached_key` and keep comparisons allocation-free.
///
/// ```
/// # use jumbie_shared::plugin::PluginDisplaySortKey;
/// let nyaa = PluginDisplaySortKey::new("nyaa", "jumbie.nyaa");
/// let discord = PluginDisplaySortKey::new("Discord", "jumbie.discord");
/// // Case-insensitive name: "discord" sorts before "nyaa" regardless of case.
/// assert!(discord < nyaa);
/// ```
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct PluginDisplaySortKey {
    pub name_lower: String,
    pub plugin_id: String,
}

impl PluginDisplaySortKey {
    pub fn new(display_name: &str, plugin_id: &str) -> Self {
        Self {
            name_lower: display_name.to_lowercase(),
            plugin_id: plugin_id.to_string(),
        }
    }
}

/// The Python interpreter to use for `.py` plugin executables and test
/// fixtures — SSoT for the interpreter name across the backend and its tests.
///
/// Tries `python3` first (POSIX convention), then `python` (Windows
/// convention, where `python3` is often absent). Returns `None` when neither
/// is on PATH — callers skip Python-based work in that case.
pub fn python_command() -> Option<&'static str> {
    use std::process::Stdio;
    // Probe with `-c "pass"` rather than `--version`: a launcher that reports
    // a version but cannot actually execute code (e.g. Python 3.14's
    // `pymanager` frontend with no runtime installed) must NOT be treated as
    // a working interpreter — spawning it would fail on the first real run.
    ["python3", "python"].into_iter().find(|name| {
        std::process::Command::new(name)
            .arg("-c")
            .arg("pass")
            // Silence stdout/stderr so a broken interpreter's error banner
            // doesn't pollute the host's output — this probe runs per spawn.
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    })
}

/// SSoT template for a Python plugin test script — every test spawning a Python
/// subprocess uses this rather than duplicating the script logic.
///
/// Tests call `PYTHON_PLUGIN_SCRIPT.replace("__LOGIC__", custom_logic)` and write
/// the result to a temp file. `{}` can't be the placeholder because Python dict
/// literals also use `{}`.
pub const PYTHON_PLUGIN_SCRIPT: &str = r##"#!/usr/bin/env python3
import os, sys, json, time

# One process serves ALL instances of a plugin type: `instances` maps
# instance_id → config (populated by `set_config`).
instances = {}

# IPC authentication (fail-closed, mirrors the Rust SDK server): when the host
# sets JUMBIE_PLUGIN_SECRET, every request must carry it and every response
# echoes it back. Without a secret, auth is skipped (test fixtures).
JUMBIE_SECRET = os.environ.get("JUMBIE_PLUGIN_SECRET")

def main():
    while True:
        line = sys.stdin.readline()
        if not line: break
        line = line.strip()
        if not line: continue

        # Defaults so error responses can echo them even when parsing failed.
        req_id = None
        req_auth = None
        try:
            req = json.loads(line)
            req_id = req.get("id")
            method = req.get("method")
            params = req.get("params", {})
            instance_id = req.get("instance_id")
            req_auth = req.get("auth")

            if JUMBIE_SECRET and req_auth != JUMBIE_SECRET:
                print(json.dumps({"jsonrpc": "2.0", "error": {"code": -32001, "message": "Authentication failed"}, "id": req_id, "auth": req_auth}), flush=True)
                continue

            # ── Startup handshake (must match plugin_sdk::rpc::PROTOCOL_VERSION) ──
            if method == "hello":
                print(json.dumps({"jsonrpc": "2.0", "result": {"protocol_version": 3}, "id": req_id, "auth": req_auth}), flush=True)
                continue

            # ── Instance lifecycle (generic: one process, many instances) ──
            # `set_config` is idempotent: it creates OR updates an instance's
            # config — config is just input, there is no "reconfigure" logic.
            if method == "set_config":
                instances[instance_id] = params
                print(json.dumps({"jsonrpc": "2.0", "result": True, "id": req_id, "auth": req_auth}), flush=True)
                continue
            if method == "shutdown_instance":
                instances.pop(instance_id, None)
                print(json.dumps({"jsonrpc": "2.0", "result": True, "id": req_id, "auth": req_auth}), flush=True)
                continue
            if method == "health_check":
                print(json.dumps({"jsonrpc": "2.0", "result": "ok", "id": req_id, "auth": req_auth}), flush=True)
                continue

__LOGIC__

        except json.JSONDecodeError as e:
            print(json.dumps({"jsonrpc": "2.0", "error": {"code": -32700, "message": f"Parse error: {e}"}, "id": req_id, "auth": req_auth}), flush=True)
        except Exception as e:
            # Echo the request id + auth so the host can route the error back to
            # the caller (a response with id None and no auth echo is dropped).
            # Without this, a raised exception in a method looks like a 30s
            # timeout instead of an immediate, diagnosable failure.
            print(json.dumps({"jsonrpc": "2.0", "error": {"code": -32603, "message": str(e)}, "id": req_id, "auth": req_auth}), flush=True)

if __name__ == "__main__":
    main()
"##;

/// Each plugin implements this to declare and validate its static config.
pub trait PluginConfig: Sized + Serialize + DeserializeOwned + Clone {
    /// Plugin identifier (matches the config section name).
    fn plugin_id() -> &'static str;

    /// Full config, used when the plugin section is missing from `config.toml`.
    fn default() -> Self;

    /// Validate configuration values.
    fn validate(&self) -> Result<(), String> {
        Ok(())
    }

    /// Whether this plugin is enabled.
    fn is_enabled(&self) -> bool;

    /// Fills missing fields with defaults when the config section is partial.
    fn merge_with_defaults(&self) -> Self {
        self.clone()
    }

    /// Label for the series identifier input field, if the plugin uses one.
    /// This allows dynamic generation of metadata input fields per plugin.
    fn series_identifier_label() -> Option<&'static str> {
        None
    }

    /// Human-readable description of the plugin shown in the UI.
    fn description() -> &'static str {
        ""
    }
}
