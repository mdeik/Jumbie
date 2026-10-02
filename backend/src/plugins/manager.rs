// Plugin Manager
//
// Central orchestrator for both plugin categories:
//
//   Internal plugins: built-in Rust implementations (qBittorrent, TVMaze, Nyaa,
//     Discord), instantiated via `InternalPluginRegistry::create()` from the app
//     config and hot-reloadable via `swap_internal_plugins()`.
//
//   External plugins: subprocess plugins in the plugins/ directory, discovered
//     via manifest.json files, with ONE shared process per TYPE (all instances
//     live in it, routed by instance_id) spoken to over stdin/stdout JSON-RPC.
//     Instances sync from config via `sync_external_instances()`.
//
// Both categories are keyed by their backend-owned instance id (config instance
// key) and populate `instance_types`/`instance_names` so instance infos can be
// stamped with plugin_id + instance_id at the service boundary.
//
// Internal plugins implement category traits (NotifierPlugin, DownloaderClient,
// ProviderSource, MetadataPlugin) in addition to `PluginInstance`; the registry
// wraps them in `Arc<dyn PluginInstance>` for uniform handling, and callers use
// `get_plugin()` + downcast when they need the concrete type's methods.

use super::{InstanceHandle, PluginInstance, PluginTypeHost, policy::PolicyPlugin};
use anyhow::Result;
use jumbie_shared::config::PluginsConfig;
use jumbie_shared::plugin::Capability;
use jumbie_shared::types::payloads::PluginTypeMetrics;
use plugin_sdk::PluginTypeInfo;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::fs;
use tokio_util::sync::CancellationToken;

// Plugin Manifest (from disk)
// Parsed from each external plugin's manifest.json file:
//   {
//     "display_name": "My Notifier",
//     "executable": "my_notifier.py",
//     "public_key": "base64url(32-byte Ed25519 public key)",          // optional
//     "signature": "base64url(64-byte Ed25519 sig of BLAKE3 hash)"   // optional
//   }
// If both `public_key` and `signature` are present, the executable's BLAKE3 hash
// is verified against the Ed25519 signature BEFORE spawning; if either is absent
// the check is skipped.
/// Plugin manifest loaded from `manifest.json` in a plugin subdirectory.
///
/// # Security
///
/// `deny_unknown_fields` rejects any unrecognized JSON fields so that
/// typos or injection attempts in the manifest are caught at load time
/// rather than silently ignored.
///
/// The `executable` field MUST be a simple filename (no `/`, `\`, or `..`)
/// to prevent directory traversal. This is enforced during load in
/// [`PluginManager::load_plugin`].
#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct PluginManifest {
    pub display_name: String,
    /// Simple filename of the plugin executable (e.g. `"run.py"`, `"plugin.bin"`).
    /// Path separators and `..` are rejected to prevent directory traversal.
    pub executable: String,
    /// Optional base64url-encoded Ed25519 public key (32 bytes).
    /// If present, `signature` must also be present.
    #[serde(default)]
    pub public_key: Option<String>,
    /// Optional base64url-encoded Ed25519 signature (64 bytes) of the
    /// executable's BLAKE3 hash, signed by the corresponding private key.
    #[serde(default)]
    pub signature: Option<String>,
}

// Runtime State
// Tracked separately from the plugin HashMap so FAILED plugins (which never
// make it into the active set) are still reported on the UI status page.
#[derive(Clone)]
pub enum RuntimePluginState {
    Loaded(Arc<dyn PluginInstance>),
    Failed(String),
}

#[derive(Clone)]
pub struct RuntimePluginTypeInfo {
    pub id: String,
    pub name: String,
    pub category: String,
    pub state: RuntimePluginState,
}

/// The output of `build_internal_plugins` — a named bundle instead of an
/// anonymous 5-tuple so call sites can't mix up arguments.
#[derive(Default)]
pub struct PluginBuild {
    pub plugins: HashMap<String, Arc<dyn PluginInstance>>,
    pub plugin_order: Vec<String>,
    pub registered_plugins: Vec<RuntimePluginTypeInfo>,
    pub instance_names: HashMap<String, String>,
    pub instance_types: HashMap<String, String>,
}

pub struct PluginManager {
    pub(crate) plugins: HashMap<String, Arc<dyn PluginInstance>>,
    // plugin_order determines the priority order for capability lookups.
    // Earlier entries are preferred when searching for a plugin by capability.
    pub(crate) plugin_order: Vec<String>,
    pub(crate) plugins_dir: PathBuf,
    pub(crate) registered_plugins: Vec<RuntimePluginTypeInfo>,
    /// Maps instance_id → user-configured display name, populated from plugin
    /// config at load time. The backend owns this mapping so that display names
    /// are not dependent on what the plugin's self-reported `plugin_info()`
    /// returns (which external plugins could set arbitrarily).
    pub(crate) instance_names: HashMap<String, String>,
    /// Maps instance_id → type id (`derive_plugin_id` format, e.g. "jumbie.tvdb").
    ///
    /// Backend-owned identity map, populated when instances are loaded. Used by
    /// `get_plugins` to stamp `PluginTypeInfo.plugin_id` + `instance_id` so the
    /// frontend never has to reconstruct identity from config.
    pub(crate) instance_types: HashMap<String, String>,
    /// Cached discovery metadata for external plugin types (manifest + get_info),
    /// so config updates can sync instances without re-scanning the plugins dir.
    external_discoveries: Vec<ExternalDiscovery>,
    /// One shared process per external plugin TYPE (see `PluginTypeHost`). All
    /// instances of a type live inside that process, routed by instance_id.
    external_hosts: HashMap<String, Arc<PluginTypeHost>>,
    /// ONE shared token bucket per external TYPE, so the combined throughput of
    /// a type's instances never exceeds its declared rate_limit.
    external_limiters: HashMap<String, std::sync::Arc<super::policy::DefaultDirectRateLimiter>>,
    /// When true, external plugins must ship a valid Ed25519 signature in their
    /// manifest or discovery fails. Opt-in (see `SecurityConfig`).
    require_signatures: bool,
    /// The config each instance was last built/reconfigured with (SSoT for the
    /// targeted-rebuild diff: saves only touch instances whose config changed).
    applied_configs: HashMap<String, serde_json::Value>,
    // Shutdown token shared with all PluginTypeHost management tasks.
    // Calling shutdown() cancels it, which causes every plugin's
    // lifecycle loop to exit gracefully.
    shutdown_token: CancellationToken,
}

impl PluginManager {
    pub fn new(plugins_dir: impl Into<PathBuf>) -> Self {
        let plugins_dir = plugins_dir.into();

        // Clean up leftover plugin processes from a previous crash — but only in
        // production builds. The PID registry is process-global, so running this
        // under `cargo test` would SIGTERM plugin processes belonging to
        // CONCURRENTLY RUNNING tests — the classic source of flaky
        // "Plugin process crashed" failures under parallel test load.
        #[cfg(not(test))]
        {
            // Persistent PID registry: survives host crashes so the next startup
            // can sweep PIDs a crashed host left behind.
            super::sandbox::init_state_path(plugins_dir.join(".plugin_pids.json"));
            super::sandbox::cleanup_orphan_pids();
        }

        Self {
            plugins: HashMap::new(),
            plugin_order: Vec::new(),
            plugins_dir,
            registered_plugins: Vec::new(),
            instance_names: HashMap::new(),
            instance_types: HashMap::new(),
            external_discoveries: Vec::new(),
            external_hosts: HashMap::new(),
            external_limiters: HashMap::new(),
            require_signatures: false,
            applied_configs: HashMap::new(),
            shutdown_token: CancellationToken::new(),
        }
    }

    /// Expose the shutdown token for callers that need to pass it to
    /// factory methods or background tasks.
    pub fn shutdown_token(&self) -> CancellationToken {
        self.shutdown_token.clone()
    }

    /// Require Ed25519 signatures on external plugin manifests (opt-in; wired
    /// from `SecurityConfig::require_plugin_signatures` at startup).
    pub fn set_require_signatures(&mut self, require: bool) {
        self.require_signatures = require;
    }

    pub fn shutdown(&self) {
        self.shutdown_token.cancel();
    }

    // External Plugin Discovery
    // Two-phase:
    //   Phase 1 (discover): scan plugins_dir for manifest.json directories and
    //     probe each with get_info for its canonical TYPE id + metadata.
    //     Duplicate types (versioned directories) are resolved by version, higher
    //     wins. The probe process is transient.
    //   Phase 2 (sync): derive instances from the plugins config — one subprocess
    //     per ENABLED configured instance; a type with no config entries gets a
    //     single instance keyed by its canonical id (singleton fallback so
    //     installed-but-unconfigured plugins still show up).
    // A plugin directory that fails to load is logged and recorded in
    // registered_plugins so the UI shows a "Failed" badge.
    pub async fn discover_and_start(&mut self, plugins_cfg: &PluginsConfig) -> Result<()> {
        if !self.plugins_dir.exists() {
            tokio::fs::create_dir_all(&self.plugins_dir).await.ok();
            return Ok(());
        }

        let mut discoveries = Vec::new();
        let mut entries = fs::read_dir(&self.plugins_dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.is_dir() {
                let manifest_path = path.join("manifest.json");
                if manifest_path.exists() {
                    match self.discover_external(&manifest_path, &path).await {
                        Ok(d) => discoveries.push(d),
                        Err(e) => {
                            tracing::error!("Failed to load plugin from {}: {}", path.display(), e);
                            self.registered_plugins.push(RuntimePluginTypeInfo {
                                id: path
                                    .file_name()
                                    .unwrap_or_default()
                                    .to_string_lossy()
                                    .to_string(),
                                name: path
                                    .file_name()
                                    .unwrap_or_default()
                                    .to_string_lossy()
                                    .to_string(),
                                category: "plugin".to_string(),
                                state: RuntimePluginState::Failed(e.to_string()),
                            });
                        }
                    }
                }
            }
        }

        // Type-level collision resolution (versioned plugin directories).
        dedupe_external_discoveries(&mut discoveries, &mut self.registered_plugins);

        // Cache the discovery so config updates can sync instances cheaply.
        self.external_discoveries = discoveries;

        // Spawn one subprocess per configured instance (or a singleton fallback).
        self.sync_external_instances(plugins_cfg).await;
        Ok(())
    }

    // Discover a single external plugin type: verify the signature (if
    // required), spawn a transient probe process, call `get_info`, validate +
    // sanitize, then drop the probe — each configured instance spawns its own
    // process separately.
    async fn discover_external(
        &mut self,
        manifest_path: &Path,
        dir: &Path,
    ) -> Result<ExternalDiscovery> {
        let manifest_data = fs::read_to_string(manifest_path).await?;
        let manifest: PluginManifest = serde_json::from_str(&manifest_data)?;

        // Reject path separators and `..` so the manifest cannot load an
        // executable outside the plugin directory.
        if manifest.executable.contains('/')
            || manifest.executable.contains('\\')
            || manifest.executable.contains("..")
        {
            anyhow::bail!(
                "Plugin '{}' has an invalid executable path '{}': must be a simple filename without path separators or '..'",
                manifest.display_name,
                manifest.executable
            );
        }

        let executable_path = dir.join(&manifest.executable);
        if !executable_path.exists() {
            anyhow::bail!("Executable {} not found", executable_path.display());
        }

        tracing::info!(
            "Discovering plugin: {} (from {})",
            manifest.display_name,
            dir.display()
        );
        // Make sure it's executable (UNIX)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&executable_path).await?.permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&executable_path, perms).await.ok();
        }

        // Signature verification (identity control): when `require_signatures` is
        // on, a manifest without a valid signature is REJECTED at discovery
        // (unsigned plugins are refused). Otherwise, if both `public_key` and
        // `signature` are present, verify the executable's hash BEFORE spawning
        // (per-manifest opt-in verification).
        if self.require_signatures
            && (manifest.public_key.is_none() || manifest.signature.is_none())
        {
            anyhow::bail!(
                "Plugin '{}' is unsigned and plugin signatures are required (security.require_plugin_signatures)",
                manifest.display_name
            );
        }
        if let (Some(pk), Some(sig)) = (&manifest.public_key, &manifest.signature) {
            tracing::info!(
                "Verifying plugin signature: {} (key: {}...)",
                manifest.display_name,
                &pk[..pk.len().min(16)]
            );
            super::signing::verify_plugin_signature(&executable_path, pk, sig).map_err(|e| {
                anyhow::anyhow!(
                    "Signature verification failed for {}: {}",
                    manifest.display_name,
                    e
                )
            })?;
            tracing::info!("Plugin signature verified: {}", manifest.display_name);
        } else if manifest.public_key.is_some() != manifest.signature.is_some() {
            anyhow::bail!(
                "Plugin '{}' manifest has partial signature data: must provide BOTH public_key and signature, or neither.",
                manifest.display_name
            );
        }

        // Transient probe process: the probe only calls get_info; the discovery is
        // cached and the type's real process (one per type, serving all instances)
        // spawns separately.
        let host = PluginTypeHost::spawn(
            &manifest.display_name,
            &manifest.display_name,
            &executable_path,
            self.shutdown_token.clone(),
        )
        .await?;

        let mut cached_info: Option<PluginTypeInfo> = None;
        let canonical_id = match host.get_info().await {
            Ok(info_val) => {
                if let Ok(info) = serde_json::from_value::<PluginTypeInfo>(info_val) {
                    // Reject reserved author values and enforce semver format.
                    if let Err(e) = crate::validation::validate_plugin_info(&info) {
                        anyhow::bail!(
                            "Plugin '{}' failed validation: {}; skipping.",
                            manifest.display_name,
                            e.0
                        );
                    }

                    // PluginTypeInfo carries no identity fields — `plugin_id`/
                    // `instance_id` are backend-derived and attached only at
                    // the API boundary (never from plugin-supplied data).
                    let derived = info.derived_id();
                    tracing::info!(
                        "Plugin '{}' initialised — canonical ID: {}, capabilities: {:?}, protocols: {:?}, rate_limit: {:?}",
                        manifest.display_name,
                        derived,
                        info.capabilities,
                        info.supported_protocols,
                        info.rate_limit
                    );
                    cached_info = Some(info.clone());
                    derived
                } else {
                    anyhow::bail!(
                        "Plugin '{}' returned invalid get_info response; skipping.",
                        manifest.display_name
                    );
                }
            }
            Err(e) => {
                tracing::error!("Plugin '{}' failed get_info: {}", manifest.display_name, e);
                plugin_sdk::traits::to_plugin_slug(&manifest.display_name)
            }
        };

        let info =
            cached_info.unwrap_or_else(|| ExternalDiscovery::fallback_info(&manifest.display_name));

        Ok(ExternalDiscovery {
            executable_path,
            display_name: manifest.display_name,
            canonical_id,
            info,
        })
    }

    // Sync External Instances From Config
    //
    // Idempotent: safe to call at boot and on every config update. One shared
    // process per TYPE is spawned lazily; desired instances = enabled config
    // entries per discovered type (or a singleton fallback keyed by the
    // canonical id when the type has no config). Present instances are left
    // alone (apply_config pushed their config); stale instances are shut down
    // in the shared process; missing ones are initialized + registered.
    pub async fn sync_external_instances(&mut self, plugins_cfg: &PluginsConfig) {
        // Ensure one shared process per discovered type
        for disc in &self.external_discoveries {
            if self.external_hosts.contains_key(&disc.canonical_id) {
                continue;
            }
            match PluginTypeHost::spawn(
                &disc.canonical_id,
                &disc.display_name,
                &disc.executable_path,
                self.shutdown_token.clone(),
            )
            .await
            {
                Ok(host) => {
                    host.set_cached_info(disc.info.clone());
                    tracing::info!(
                        "Spawned shared process for external plugin type '{}' ({})",
                        disc.display_name,
                        disc.canonical_id
                    );
                    self.external_hosts.insert(disc.canonical_id.clone(), host);
                }
                Err(e) => {
                    tracing::error!(
                        "Failed to spawn process for external plugin type '{}' ({}): {}",
                        disc.display_name,
                        disc.canonical_id,
                        e
                    );
                    self.registered_plugins.push(RuntimePluginTypeInfo {
                        id: disc.canonical_id.clone(),
                        name: disc.display_name.clone(),
                        category: "plugin".to_string(),
                        state: RuntimePluginState::Failed(e.to_string()),
                    });
                }
            }
        }

        // Desired instances: (instance_id, discovery_index, config)
        let mut desired: Vec<(String, usize, serde_json::Value)> = Vec::new();
        for (idx, disc) in self.external_discoveries.iter().enumerate() {
            let configured = find_config_instances(plugins_cfg, &disc.canonical_id);
            if configured.is_empty() {
                desired.push((disc.canonical_id.clone(), idx, serde_json::json!({})));
            } else {
                for (instance_id, config) in configured {
                    desired.push((instance_id, idx, config));
                }
            }
        }
        let desired_ids: std::collections::HashSet<String> =
            desired.iter().map(|(id, _, _)| id.clone()).collect();

        // Remove instances that are no longer configured
        // Only REGISTERED INSTANCES are swept (authoritative marker:
        // `instance_types`). Discovery-FAILED type entries (category "plugin",
        // but never registered as instances) must survive so the UI shows the
        // failure badge instead of silently erasing it.
        let stale: Vec<String> = self
            .registered_plugins
            .iter()
            .filter(|p| {
                p.category == "plugin"
                    && self.instance_types.contains_key(&p.id)
                    && !desired_ids.contains(&p.id)
            })
            .map(|p| p.id.clone())
            .collect();
        for id in stale {
            tracing::info!(
                "External plugin instance '{}' no longer configured — removing",
                id
            );
            self.remove_plugin_instance(&id);
        }

        // Initialize + register missing instances in the shared process
        for (instance_id, idx, config) in desired {
            if self.plugins.contains_key(&instance_id) {
                continue; // present instances were reconfigured by apply_config
            }
            let disc = self.external_discoveries[idx].clone();
            let type_host = self.external_hosts.get(&disc.canonical_id).cloned();
            match type_host {
                Some(host) => {
                    self.register_external_instance(&disc, &host, &instance_id, &config)
                        .await
                }
                None => {
                    tracing::error!(
                        "No process for external plugin type '{}' ({}) — instance '{}' not registered",
                        disc.display_name,
                        disc.canonical_id,
                        instance_id
                    );
                    self.registered_plugins.push(RuntimePluginTypeInfo {
                        id: instance_id.clone(),
                        name: disc.display_name.clone(),
                        category: "plugin".to_string(),
                        state: RuntimePluginState::Failed("type process unavailable".to_string()),
                    });
                }
            }
        }
    }

    /// Register one external plugin instance inside its type's shared process,
    /// mirroring internal plugin registration so both categories are fully
    /// consistent: instance key, identity map, display name, priority, status.
    async fn register_external_instance(
        &mut self,
        discovery: &ExternalDiscovery,
        type_host: &Arc<PluginTypeHost>,
        instance_id: &str,
        config: &serde_json::Value,
    ) {
        // Push the config into the shared process (`set_config` is idempotent
        // — config is input, so creation and update are the same RPC). On
        // failure the instance is registered as Failed so the UI shows a badge;
        // it will be retried on the next config sync.
        if let Err(e) = type_host.set_config(instance_id, config.clone()).await {
            tracing::warn!(
                "Plugin '{}' instance '{}' failed to initialize: {}. It may need configuration.",
                discovery.display_name,
                instance_id,
                e
            );
            self.registered_plugins.push(RuntimePluginTypeInfo {
                id: instance_id.to_string(),
                name: discovery.display_name.clone(),
                category: "plugin".to_string(),
                state: RuntimePluginState::Failed(e.to_string()),
            });
            return;
        }

        let handle = InstanceHandle::new(instance_id.to_string(), type_host.clone());
        let inner: Arc<dyn PluginInstance> = Arc::new(handle);
        // ONE shared token bucket per TYPE (SSoT: the declared rate_limit
        // describes the upstream endpoint shared by all instances of a type).
        let limiter = match self.external_limiters.get(&discovery.canonical_id) {
            Some(l) => Some(l.clone()),
            None => discovery.info.rate_limit.as_ref().map(|rl| {
                let l = super::policy::PolicyPlugin::build_limiter(rl);
                self.external_limiters
                    .insert(discovery.canonical_id.clone(), l.clone());
                l
            }),
        };
        let wrapped: Arc<dyn PluginInstance> = Arc::new(PolicyPlugin::new(
            inner,
            limiter,
            discovery.canonical_id.clone(),
        ));
        self.plugins
            .insert(instance_id.to_string(), wrapped.clone());
        if !self.plugin_order.contains(&instance_id.to_string()) {
            self.plugin_order.push(instance_id.to_string());
        }
        self.instance_types
            .insert(instance_id.to_string(), discovery.canonical_id.clone());

        // Backend-owned display name from config (consistent with internal plugins).
        let instance_name = config
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if !instance_name.is_empty() {
            self.instance_names
                .insert(instance_id.to_string(), instance_name);
        }

        // Host-managed fields (priority / enabled / refresh_interval) live on
        // the wrapper — sync them from config (SSoT with build_internal_plugins).
        Self::sync_host_fields(&wrapped, instance_id, config);
        self.applied_configs
            .insert(instance_id.to_string(), config.clone());

        self.registered_plugins.push(RuntimePluginTypeInfo {
            id: instance_id.to_string(),
            name: discovery.display_name.clone(),
            category: "plugin".to_string(),
            state: RuntimePluginState::Loaded(wrapped),
        });
        tracing::info!(
            "Loaded external plugin {} ({}) instance '{}' in shared process",
            discovery.display_name,
            discovery.canonical_id,
            instance_id
        );
    }

    /// Remove a plugin instance from every manager structure. The shared type
    /// process stays alive — the instance is shut down in-process (best effort).
    fn remove_plugin_instance(&mut self, id: &str) {
        if let Some(type_id) = self.instance_types.get(id)
            && let Some(host) = self.external_hosts.get(type_id)
        {
            let id_owned = id.to_string();
            let host = host.clone();
            tokio::spawn(async move {
                let _ = host.shutdown_instance(&id_owned).await;
            });
        }
        self.plugins.remove(id);
        self.plugin_order.retain(|x| x != id);
        self.registered_plugins.retain(|p| p.id != id);
        self.instance_types.remove(id);
        self.instance_names.remove(id);
        self.applied_configs.remove(id);
    }

    /// Flip an instance's status entry to Failed (e.g. after a failed
    /// `set_config`) without removing it — the instance keeps running on its
    /// previous config and the next save retries.
    fn mark_instance_failed(&mut self, id: &str, message: &str) {
        for entry in &mut self.registered_plugins {
            if entry.id == id {
                entry.state = RuntimePluginState::Failed(message.to_string());
                return;
            }
        }
        // No entry yet (e.g. external instance still registering) — add one.
        self.registered_plugins.push(RuntimePluginTypeInfo {
            id: id.to_string(),
            name: String::new(),
            category: "plugin".to_string(),
            state: RuntimePluginState::Failed(message.to_string()),
        });
    }

    /// Push new configuration to ALL plugins (internal and external) without
    /// restart — the single entry point for config saves and hot-reload.
    ///
    /// Saves only touch what changed (config is INPUT): external instance
    /// lifecycle sync runs first, then `set_config` on changed instances;
    /// unchanged instances keep their exact Arc. A failed `set_config` keeps the
    /// instance running on its OLD config and marks it Failed — never
    /// factory-rebuilt (a rebuild would fail with the same config).
    /// `applied_configs` records what each instance was built with so the next
    /// save can diff against it.
    pub async fn apply_config(
        &mut self,
        plugins_cfg: &jumbie_shared::config::PluginsConfig,
        full_cfg: Arc<jumbie_shared::config::Config>,
    ) {
        // 1. External instance lifecycle
        self.sync_external_instances(plugins_cfg).await;

        let config_map: std::collections::HashMap<String, serde_json::Value> = {
            let mut map = std::collections::HashMap::new();
            for cat_map in [
                &plugins_cfg.downloader,
                &plugins_cfg.notifier,
                &plugins_cfg.source,
                &plugins_cfg.metadata,
            ] {
                for instances in cat_map.values() {
                    for (instance_id, config) in instances {
                        map.insert(instance_id.clone(), config.clone());
                    }
                }
            }
            map
        };
        let external_ids: std::collections::HashSet<String> = self
            .registered_plugins
            .iter()
            .filter(|p| p.category == "plugin")
            .map(|p| p.id.clone())
            .collect();

        // 2. External: set_config changed instances; failure ⇒ Failed
        for (instance_id, config) in &config_map {
            if !external_ids.contains(instance_id) {
                continue;
            }
            if self.applied_configs.get(instance_id) == Some(config) {
                continue;
            }
            let Some(plugin) = self.plugins.get(instance_id) else {
                continue;
            };
            match plugin.set_config(config.clone()).await {
                Ok(_) => {
                    tracing::info!(
                        "Applied config to external plugin '{}' ({}) in place",
                        plugin.plugin_info().display_name,
                        instance_id
                    );
                    Self::sync_host_fields(plugin, instance_id, config);
                    self.sync_instance_name(instance_id, config);
                    self.applied_configs
                        .insert(instance_id.clone(), config.clone());
                }
                Err(e) => {
                    // No fallback: a rebuild/re-init would fail with the same
                    // config. Keep the instance running on its OLD config and
                    // surface the failure; the next save retries.
                    tracing::warn!(
                        "External plugin '{}' ({}) failed set_config ({}); keeping previous config, marking Failed",
                        plugin.plugin_info().display_name,
                        instance_id,
                        e
                    );
                    self.mark_instance_failed(instance_id, &e.to_string());
                }
            }
        }

        // 3. Internal: set_config changed instances; failure ⇒ Failed
        // `keep` = external + unchanged internal + internal instances that
        // accepted the new config (or kept their old one on failure) — the
        // swap retains those exact Arcs, so there is exactly one copy of an
        // instance at any time. Disabled instances are NOT kept (the build
        // drops them, matching `build_internal_plugins` semantics).
        let mut keep: std::collections::HashSet<String> = external_ids.clone();
        for (instance_id, config) in &config_map {
            if external_ids.contains(instance_id) {
                continue;
            }
            if self.applied_configs.get(instance_id) == Some(config) {
                keep.insert(instance_id.clone());
                continue;
            }
            // Mirror build_internal_plugins: a disabled instance is dropped
            // (never kept in place), not set_config'd.
            let is_enabled = config
                .get("enabled")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if !is_enabled {
                continue;
            }
            let Some(plugin) = self.plugins.get(instance_id) else {
                continue;
            };
            match plugin.set_config(config.clone()).await {
                Ok(_) => {
                    tracing::info!(
                        "Applied config to internal plugin '{}' ({}) in place",
                        plugin.plugin_info().display_name,
                        instance_id
                    );
                    Self::sync_host_fields(plugin, instance_id, config);
                    self.sync_instance_name(instance_id, config);
                    keep.insert(instance_id.clone());
                }
                Err(e) => {
                    // No factory rebuild: a rebuild would fail with the same
                    // config. The instance keeps running on its OLD config and
                    // is marked Failed; the next save retries.
                    tracing::warn!(
                        "Internal plugin '{}' ({}) failed set_config ({}); keeping previous config, marking Failed",
                        plugin.plugin_info().display_name,
                        instance_id,
                        e
                    );
                    self.mark_instance_failed(instance_id, &e.to_string());
                    keep.insert(instance_id.clone());
                }
            }
        }
        let build =
            Self::build_internal_plugins(plugins_cfg, full_cfg, self.shutdown_token.clone(), &keep)
                .await;
        self.swap_internal_plugins(&keep, build);

        // 4. Record applied configs; prune removed instances
        // Only instances actually present after the swap are recorded (a failed
        // build stays unrecorded so the next save retries it).
        self.applied_configs
            .retain(|id, _| config_map.contains_key(id));
        for (instance_id, config) in &config_map {
            if !self.plugins.contains_key(instance_id) {
                continue;
            }
            self.applied_configs
                .insert(instance_id.clone(), config.clone());
        }
    }

    /// Sync backend-managed host fields (enabled / priority / refresh_interval)
    /// onto a plugin after a successful in-place reconfigure or at build time.
    ///
    /// These fields are stored on the wrapper (`PolicyPlugin`), so they stick
    /// for every plugin kind regardless of the inner plugin's setter support.
    /// Capability toggles (enable_polling / enable_manual_search / ...) are
    /// declared statically in `plugin_info()` and are not mutated here.
    pub(crate) fn sync_host_fields(
        plugin: &std::sync::Arc<dyn PluginInstance>,
        id: &str,
        config: &serde_json::Value,
    ) {
        // Enabled state
        if let Some(enabled) = config.get("enabled").and_then(|v| v.as_bool())
            && plugin.is_enabled() != enabled
        {
            tracing::info!(
                "Setting enabled={} for plugin '{}' ({})",
                enabled,
                plugin.plugin_info().display_name,
                id
            );
            plugin.set_enabled(enabled);
        }

        // Priority ordering
        if let Some(new_pri) = config.get("priority").and_then(|v| v.as_i64())
            && plugin.priority() != new_pri as i32
        {
            tracing::info!(
                "Updating priority for '{}' ({}): {} → {}",
                plugin.plugin_info().display_name,
                id,
                plugin.priority(),
                new_pri
            );
            plugin.set_priority(new_pri as i32);
        }

        // Refresh interval for polling sources
        if let Some(interval) = config.get("refresh_interval").and_then(|v| v.as_u64())
            && plugin.refresh_interval() != Some(interval)
        {
            tracing::info!(
                "Updating refresh_interval for '{}' ({}): {} min",
                plugin.plugin_info().display_name,
                id,
                interval
            );
            plugin.set_refresh_interval(Some(interval));
        }
    }

    /// Update the backend-owned display-name map after an in-place reconfigure
    /// or re-initialization (SSoT with `register_external_instance` and
    /// `build_internal_plugins`, which both read `config["name"]`).
    fn sync_instance_name(&mut self, instance_id: &str, config: &serde_json::Value) {
        match config.get("name").and_then(|v| v.as_str()) {
            Some(name) if !name.is_empty() => {
                self.instance_names
                    .insert(instance_id.to_string(), name.to_string());
            }
            _ => {
                self.instance_names.remove(instance_id);
            }
        }
    }

    pub fn get_plugin(&self, plugin_id: &str) -> Option<Arc<dyn PluginInstance>> {
        self.plugins.get(plugin_id).cloned()
    }

    /// Returns the user-configured display name for a plugin instance.
    ///
    /// The backend owns this mapping, populated from the plugin config's `name`
    /// field at load time. This avoids relying on the plugin's self-reported
    /// `plugin_info().display_name`, which external plugins could set arbitrarily.
    /// Returns an empty string if no name was configured (the caller can check
    /// `!is_empty()` to decide whether to display the source field).
    pub fn get_instance_name(&self, instance_id: &str) -> String {
        self.instance_names
            .get(instance_id)
            .cloned()
            .unwrap_or_default()
    }

    /// Returns the type id (`derive_plugin_id` format) for a plugin instance.
    ///
    /// SSoT: populated at load time from the same `derive_plugin_id` call that
    /// `/api/plugins/available` uses, so the type id can never drift between
    /// the type-level listing and instance-level infos.
    pub fn get_instance_type_id(&self, instance_id: &str) -> Option<&str> {
        self.instance_types.get(instance_id).map(|s| s.as_str())
    }

    pub fn get_all_plugins(&self) -> Vec<Arc<dyn PluginInstance>> {
        self.plugins.values().cloned().collect()
    }

    // Capability-based lookup
    /// Resolve a `@uuid` prefix to a concrete source plugin instance.
    ///
    /// Only scanned against plugins with [`Capability::FeedProvider`], since
    /// only source plugins produce entries that carry an `instance_id`.
    ///
    /// # Edge cases handled
    /// - **Downloader UUID**: downloader plugins lack `FeedProvider`, so None is
    ///   returned and the caller falls back (generic alias / all sources).
    /// - **Invalid/malformed string**: no `plugin_id()` will match, so None is
    ///   returned with the same graceful fallback.
    pub async fn get_source_by_id(&self, id: &str) -> Option<Arc<dyn PluginInstance>> {
        self.get_plugin_with_capability(id, Capability::FeedProvider)
    }

    /// Returns all ENABLED plugins with the given capability.
    ///
    /// ORDER (SSoT): priority DESC, then `instance_id` ASC. This is the `ALL`
    /// intent — see also [`Self::first_plugin_by_capability`] (FIRST available)
    /// and [`Self::get_plugin`] / [`Self::get_plugin_with_capability`] (SPECIFIC).
    pub fn get_plugins_by_capability(
        &self,
        capability: Capability,
    ) -> Vec<Arc<dyn PluginInstance>> {
        self.get_plugins_by_all_capabilities(&[capability])
    }

    /// Returns all ENABLED plugins that have ALL of the given capabilities, in
    /// priority order (priority DESC, then `instance_id` ASC).
    ///
    /// Disabled plugins are excluded. This is the SSoT dispatch method — every
    /// caller should use it to get the exact set of plugins that should receive a
    /// request; no per-plugin capability re-checking is needed.
    ///
    /// Capabilities are evaluated as EFFECTIVE capabilities: the plugin type
    /// must declare the capability (static `plugin_info`) AND the instance's
    /// config toggle for it must be enabled — see `plugins::capabilities` for the
    /// SSoT mapping.
    pub fn get_plugins_by_all_capabilities(
        &self,
        capabilities: &[Capability],
    ) -> Vec<Arc<dyn PluginInstance>> {
        let mut plugins: Vec<Arc<dyn PluginInstance>> = self
            .plugin_order
            .iter()
            .filter_map(|id| self.plugins.get(id))
            .filter(|p| {
                if !p.is_enabled() {
                    return false;
                }
                let effective = self.effective_capabilities_for(p);
                capabilities.iter().all(|c| effective.contains(c))
            })
            .cloned()
            .collect();
        // ORDER: priority DESC, then instance_id ASC — deterministic regardless
        // of load/registration sequence.
        plugins.sort_by(|a, b| {
            b.priority()
                .cmp(&a.priority())
                .then_with(|| a.instance_id().cmp(b.instance_id()))
        });
        plugins
    }

    /// FIRST-available intent: the single highest-ordered plugin with
    /// `capability` (priority DESC, then `instance_id` ASC), or `None`.
    ///
    /// Use this for "pick one and act" dispatch (e.g. a downloader) instead of
    /// reaching into [`Self::get_plugins_by_all_capabilities`] and calling
    /// `.first()`/`.next()` yourself.
    pub fn first_plugin_by_capability(
        &self,
        capability: Capability,
    ) -> Option<Arc<dyn PluginInstance>> {
        self.first_plugin_by_all_capabilities(&[capability])
    }

    /// FIRST-available intent across multiple capabilities (must have ALL).
    pub fn first_plugin_by_all_capabilities(
        &self,
        capabilities: &[Capability],
    ) -> Option<Arc<dyn PluginInstance>> {
        self.get_plugins_by_all_capabilities(capabilities)
            .into_iter()
            .next()
    }

    /// SPECIFIC intent: the enabled instance `instance_id` IF it currently
    /// declares `capability` (effective). Returns `None` otherwise.
    ///
    /// Use this when the caller already knows which instance it wants (e.g. a
    /// per-provider action) — never scan-all-and-match by id.
    pub fn get_plugin_with_capability(
        &self,
        instance_id: &str,
        capability: Capability,
    ) -> Option<Arc<dyn PluginInstance>> {
        let plugin = self.plugins.get(instance_id)?;
        if plugin.is_enabled()
            && self
                .effective_capabilities_for(plugin)
                .contains(&capability)
        {
            Some(plugin.clone())
        } else {
            None
        }
    }

    /// Effective capabilities for one loaded plugin instance: the type's
    /// declared capabilities intersected with the instance's per-instance
    /// config toggles (see `plugins::capabilities`).
    ///
    /// This is the SSoT for "can we actually use capability X on this instance".
    /// Callers that already hold a plugin handle (rather than querying by
    /// capability) should use this so a toggle is honored consistently — e.g.
    /// `CanPauseResume` for the download queue's pause/resume buttons.
    pub fn effective_capabilities_for(&self, plugin: &Arc<dyn PluginInstance>) -> Vec<Capability> {
        let declared = plugin.plugin_info().capabilities;
        let config = self.applied_configs.get(plugin.instance_id());
        super::capabilities::effective_capabilities(&declared, config)
    }

    /// Enabled metadata providers (`MetadataProviderNormal` ∪
    /// `MetadataProviderAbsolute`, deduped), ordered by priority (DESC).
    ///
    /// SSoT for "which metadata provider acts first". Provider-selection sites
    /// must use this instead of taking an arbitrary `metadata_ids` map entry.
    pub fn ordered_metadata_providers(&self) -> Vec<Arc<dyn PluginInstance>> {
        let mut ordered = self.get_plugins_by_capability(Capability::MetadataProviderNormal);
        for p in self.get_plugins_by_capability(Capability::MetadataProviderAbsolute) {
            if !ordered.iter().any(|e| e.instance_id() == p.instance_id()) {
                ordered.push(p);
            }
        }
        // Re-sort so an absolute-only high-priority provider isn't stranded last,
        // with the same SSoT order (priority DESC, instance_id ASC).
        ordered.sort_by(|a, b| {
            b.priority()
                .cmp(&a.priority())
                .then_with(|| a.instance_id().cmp(b.instance_id()))
        });
        ordered
    }

    /// The single active metadata provider.
    ///
    /// SSoT for the single-active metadata policy: the highest-priority enabled
    /// provider (`ordered_metadata_providers` is already priority-ordered). If
    /// more than one is enabled — which the config layer prevents, but a
    /// hand-edited/legacy config could still produce — the extras are ignored and
    /// an error is logged so the violation is visible.
    pub fn active_metadata_provider(&self) -> Option<Arc<dyn PluginInstance>> {
        let providers = self.ordered_metadata_providers();
        if providers.len() > 1 {
            tracing::error!(
                "Single-active metadata policy violated: {} providers enabled [{}]; using '{}'",
                providers.len(),
                providers
                    .iter()
                    .map(|p| p.instance_id())
                    .collect::<Vec<_>>()
                    .join(", "),
                providers[0].instance_id()
            );
        }
        providers.into_iter().next()
    }

    /// Add an internal (built-in) plugin instance.
    ///
    /// Called during startup for each internal plugin factory that succeeds.
    /// If a plugin with the same ID already exists (unlikely for internal plugins
    /// unless there's a bug), the old one is overwritten with a warning.
    pub fn add_internal_plugin(&mut self, plugin: Arc<dyn PluginInstance>) {
        let plugin_id = plugin.instance_id().to_string();
        if self.plugins.contains_key(&plugin_id) {
            tracing::warn!(
                "Internal plugin ID collision: '{}' ({}). Overwriting.",
                plugin.plugin_info().display_name,
                plugin_id
            );
        } else {
            self.plugin_order.push(plugin_id.clone());
        }
        self.plugins.insert(plugin_id, plugin);
    }

    // Build Internal Plugin Instances
    //
    // A standalone async fn (no &self) so the caller can construct the new plugin
    // set OUTSIDE any write lock on the manager, then atomically swap with
    // `swap_internal_plugins()`. It iterates registered internal plugin types
    // (from the InternalPluginRegistry) and their config instances; for each
    // enabled instance NOT in `skip_instances` it calls the factory and wraps the
    // result in `PolicyPlugin`.
    //
    // `skip_instances` are instances that reconfigured in place (see
    // `apply_config`) — they must NOT be rebuilt here; the swap retains the
    // existing instance and its per-instance registered/name/type entries, so
    // there is exactly one copy of an instance at any time.
    //
    // Registered entries are PER-INSTANCE (id = config instance id), mirroring
    // external plugins, so kept instances survive the swap with their status entry
    // intact. If no instances of a plugin type are configured, an
    // InternalMarkerPlugin is inserted instead — a no-op placeholder that reports
    // as "Loaded" but refuses all method calls except health_check and get_info,
    // ensuring the UI shows the plugin type as present even when unconfigured.
    pub async fn build_internal_plugins(
        plugins_cfg: &jumbie_shared::config::PluginsConfig,
        full_cfg: Arc<jumbie_shared::config::Config>,
        shutdown_token: tokio_util::sync::CancellationToken,
        skip_instances: &std::collections::HashSet<String>,
    ) -> PluginBuild {
        let mut plugins = HashMap::new();
        let mut plugin_order = Vec::new();
        let mut registered_plugins = Vec::new();
        let mut instance_names = HashMap::new();
        let mut instance_types = HashMap::new();
        // ONE shared token bucket per TYPE (the declared rate_limit describes
        // the upstream endpoint, which the type's instances share) — built lazily
        // from each type's declared limit and reused across its instances.
        let mut type_limiters: std::collections::HashMap<
            String,
            std::sync::Arc<super::policy::DefaultDirectRateLimiter>,
        > = std::collections::HashMap::new();

        let available_plugins = super::InternalPluginRegistry::get_available_plugins().await;

        let mut categories_map = std::collections::HashMap::new();
        categories_map.insert("downloader", &plugins_cfg.downloader);
        categories_map.insert("notifier", &plugins_cfg.notifier);
        categories_map.insert("source", &plugins_cfg.source);
        categories_map.insert("metadata", &plugins_cfg.metadata);

        for (registry_key, info) in available_plugins {
            let (category, _plugin_name) = {
                let parts: Vec<&str> = registry_key.split('.').collect();
                if parts.len() != 2 {
                    continue;
                }
                (parts[0].to_string(), parts[1].to_string())
            };

            let display_name = info.display_name.clone();

            // Tracks whether this type produced any real entry below (built or
            // kept-in-place). If not, a marker entry is emitted instead.
            let mut emitted_any = false;

            // Look up config by the derived plugin_id in `author.plugin_name` format
            // (SSoT helper), matching what the frontend sends via plugin_id.
            let short_name = jumbie_shared::plugin::plugin_type_key(&registry_key);
            let plugin_id = super::derive_plugin_id(&info.author, short_name);

            let instances = categories_map
                .get(category.as_str())
                .and_then(|map| map.get(&plugin_id));

            if let Some(instances) = instances {
                for (instance_id, config_value) in instances {
                    // Kept in place by `apply_config` — never rebuild it;
                    // the swap retains the existing instance + its entries.
                    if skip_instances.contains(instance_id) {
                        emitted_any = true;
                        continue;
                    }

                    let is_enabled = config_value
                        .get("enabled")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);

                    if !is_enabled {
                        tracing::debug!(
                            "Internal plugin {} ({}) / {} is disabled — skipping.",
                            registry_key,
                            display_name,
                            instance_id
                        );
                        continue;
                    }

                    emitted_any = true;

                    let priority = config_value
                        .get("priority")
                        .and_then(|v| v.as_i64())
                        .unwrap_or(0) as i32;

                    let refresh_interval = config_value
                        .get("refresh_interval")
                        .and_then(|v| v.as_u64());

                    match super::InternalPluginRegistry::create(
                        &registry_key,
                        config_value.clone(),
                        instance_id.clone(),
                        priority,
                        refresh_interval,
                        full_cfg.clone(),
                        shutdown_token.clone(),
                    )
                    .await
                    {
                        Ok(plugin) => {
                            let id = plugin.instance_id().to_string();
                            if !plugins.contains_key(&id) {
                                plugin_order.push(id.clone());
                            }

                            // Read the user-configured instance name from config.
                            // The backend owns this — plugins don't get to choose it.
                            let instance_name = config_value
                                .get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            if !instance_name.is_empty() {
                                instance_names.insert(id.clone(), instance_name);
                            }

                            // Backend-owned identity map: instance_id → type id
                            // (same derive_plugin_id value /api/plugins/available uses).
                            instance_types.insert(id.clone(), plugin_id.clone());

                            let info = plugin.plugin_info();

                            // Shared per-type token bucket (SSoT: one bucket per
                            // type, from the type's declared rate limit).
                            let limiter = match type_limiters.get(&plugin_id) {
                                Some(l) => Some(l.clone()),
                                None => info.rate_limit.as_ref().map(|rl| {
                                    let l = super::policy::PolicyPlugin::build_limiter(rl);
                                    type_limiters.insert(plugin_id.clone(), l.clone());
                                    l
                                }),
                            };
                            let wrapped: Arc<dyn PluginInstance> =
                                Arc::new(PolicyPlugin::new(plugin, limiter, plugin_id.clone()));
                            plugins.insert(id.clone(), wrapped.clone());
                            // Apply backend-managed host fields (priority / enabled /
                            // refresh_interval) onto the wrapper. Factories consume
                            // priority but ignore refresh_interval — syncing here makes
                            // cold start and in-place saves agree (SSoT with
                            // `apply_config`).
                            Self::sync_host_fields(&wrapped, &id, config_value);
                            tracing::info!(
                                "Loaded internal plugin {} ({}): {}",
                                registry_key,
                                display_name,
                                instance_id
                            );

                            // Per-instance status entry (id = instance id), mirroring
                            // external plugins so kept instances survive the swap.
                            registered_plugins.push(RuntimePluginTypeInfo {
                                id,
                                name: display_name.clone(),
                                category: category.clone(),
                                state: RuntimePluginState::Loaded(wrapped),
                            });
                        }
                        Err(e) => {
                            tracing::warn!(
                                "Internal plugin {} ({}) / {} failed to load: {}",
                                registry_key,
                                display_name,
                                instance_id,
                                e
                            );
                            registered_plugins.push(RuntimePluginTypeInfo {
                                id: instance_id.clone(),
                                name: display_name.clone(),
                                category: category.clone(),
                                state: RuntimePluginState::Failed(e.to_string()),
                            });
                        }
                    }
                }
            }

            // Marker plugin fallback: if this plugin type produced no entry at all
            // (no configured, enabled, or kept-in-place instances), insert a marker
            // so the UI status page shows the type exists but has no config.
            if !emitted_any {
                tracing::debug!(
                    "No enabled instances for {} ({}); providing marker status.",
                    registry_key,
                    display_name
                );
                registered_plugins.push(RuntimePluginTypeInfo {
                    id: registry_key.clone(),
                    name: display_name.clone(),
                    category: category.clone(),
                    state: RuntimePluginState::Loaded(Arc::new(InternalMarkerPlugin {
                        id: registry_key.clone(),
                        info: info.clone(),
                    })),
                });
            }
        }

        PluginBuild {
            plugins,
            plugin_order,
            registered_plugins,
            instance_names,
            instance_types,
        }
    }

    // Hot-Reload Support
    //
    // Merges a fresh `PluginBuild` into the live maps, preserving:
    //   • external instances (their shared type processes must survive),
    //   • `keep_ids` — internal instances whose config did not change (their
    //     exact Arc survives: identity and runtime state are untouched).
    //
    // `build_internal_plugins` already skips kept instances, so a kept id never
    // appears in the fresh build; the guard keeps that invariant if a future
    // call site passes one anyway (there is exactly one copy of an instance).
    pub fn swap_internal_plugins(
        &mut self,
        keep_ids: &std::collections::HashSet<String>,
        build: PluginBuild,
    ) {
        let PluginBuild {
            plugins: new_plugins,
            plugin_order: new_order,
            registered_plugins: new_registered,
            instance_names: new_instance_names,
            instance_types: new_instance_types,
        } = build;

        // External IDs that MUST survive the swap (their shared type-process
        // lifecycle tasks would terminate if their Arc ref is dropped).
        let external_ids: std::collections::HashSet<String> = self
            .registered_plugins
            .iter()
            .filter(|p| p.category == "plugin")
            .map(|p| p.id.clone())
            .collect();

        let replaced_count = new_plugins.len();

        // Retain: 1) external plugins (must survive), 2) instances that
        // successfully reconfigured in-place (both their plugin and their
        // per-instance status/name/type entries — so the status card always
        // points at the real surviving instance).
        self.plugins
            .retain(|id, _| external_ids.contains(id) || keep_ids.contains(id));
        self.plugin_order
            .retain(|id| external_ids.contains(id) || keep_ids.contains(id));
        self.registered_plugins
            .retain(|p| external_ids.contains(&p.id) || keep_ids.contains(&p.id));
        self.instance_names
            .retain(|id, _| external_ids.contains(id) || keep_ids.contains(id));
        self.instance_types
            .retain(|id, _| external_ids.contains(id) || keep_ids.contains(id));

        // Merge in fresh instances for plugins that need replacement.
        // `build_internal_plugins` skips kept instances, so a kept id can never
        // appear here; the guard keeps that invariant if a future call site
        // passes one anyway (there is exactly one copy of an instance at a time).
        for (id, plugin) in new_plugins {
            if !keep_ids.contains(&id) {
                self.plugins.insert(id, plugin);
            }
        }
        let plugin_order_additions: Vec<String> = new_order
            .into_iter()
            .filter(|id| !keep_ids.contains(id) && !self.plugin_order.contains(id))
            .collect();
        self.plugin_order.extend(plugin_order_additions);
        self.registered_plugins.extend(new_registered);

        // Merge fresh instance names/types (internal-only maps; external entries
        // were retained above). Kept instances keep their existing entries.
        self.instance_names.extend(new_instance_names);
        self.instance_types.extend(new_instance_types);

        tracing::info!(
            "Hot-reload complete: {} kept in-place, {} replaced",
            keep_ids.len(),
            replaced_count
        );
    }

    /// Load internal plugins from config using the InternalPluginRegistry.
    ///
    /// The manager is the single authoritative place that checks `enabled`:
    /// - `enabled: false`  → silently skipped; never appears on the status page
    /// - `enabled: true` + factory `Ok`  → Loaded; health-checked at status query time
    /// - `enabled: true` + factory `Err` → Failed; shown as "Error" (bad config / missing fields)
    ///
    /// `Loaded` always wins over `Failed` when a plugin type has multiple instances.
    pub async fn load_internal_plugins(
        &mut self,
        plugins_cfg: &jumbie_shared::config::PluginsConfig,
        full_cfg: Arc<jumbie_shared::config::Config>,
    ) {
        // During initial startup there are no running plugins to reconfigure.
        let keep = std::collections::HashSet::new();
        let build =
            Self::build_internal_plugins(plugins_cfg, full_cfg, self.shutdown_token.clone(), &keep)
                .await;
        self.swap_internal_plugins(&keep, build);

        // Record what each instance was built with so `apply_config` can diff
        // subsequent saves against it (targeted rebuild).
        for instance_id in self.plugins.keys() {
            if let Some(config) = find_any_instance_config(plugins_cfg, instance_id) {
                self.applied_configs.insert(instance_id.clone(), config);
            }
        }
    }

    pub fn get_plugin_statuses(&self) -> Vec<RuntimePluginTypeInfo> {
        self.registered_plugins.clone()
    }

    /// Per-TYPE process metrics for external plugins (in-process reads — no
    /// RPC to plugin processes).
    pub fn external_plugin_metrics(&self) -> Vec<PluginTypeMetrics> {
        self.external_hosts
            .iter()
            .map(|(type_id, host)| {
                let (calls, errors, restarts, queue_depth) = host.stats().snapshot();
                PluginTypeMetrics {
                    type_id: type_id.clone(),
                    display_name: host.display_name().to_string(),
                    healthy: host.is_healthy(),
                    calls,
                    errors,
                    restarts,
                    queue_depth,
                }
            })
            .collect()
    }
}

// Internal Marker Plugin
//
// A lightweight placeholder for plugin types that are built into the system but
// have no active user configuration. For example, "Nyaa" as a source type exists
// in the registry even if the user hasn't added any Nyaa source instances.
//
// The marker responds to `health_check` (always OK) and `get_info` (returns the
// plugin's static info), but rejects all other method calls. This prevents the
// rest of the system from accidentally trying to use an unconfigured plugin.
struct InternalMarkerPlugin {
    id: String,
    info: jumbie_shared::plugin::PluginTypeInfo,
}

#[async_trait::async_trait]
impl PluginInstance for InternalMarkerPlugin {
    fn instance_id(&self) -> &str {
        &self.id
    }

    fn plugin_info(&self) -> plugin_sdk::traits::PluginTypeInfo {
        self.info.clone()
    }

    fn supported_protocols(&self) -> Option<&[String]> {
        self.info.supported_protocols.as_deref()
    }

    fn priority(&self) -> i32 {
        0
    }
}

// External Plugin Discovery Types

/// Cached metadata for one external plugin type (manifest + get_info probe).
///
/// The probe process is transient — the type's real shared `PluginTypeHost`
/// (one per type, serving all instances) spawns separately on first sync.
#[derive(Clone)]
struct ExternalDiscovery {
    executable_path: PathBuf,
    display_name: String,
    /// Canonical TYPE id (derived_id format, or slug fallback when get_info fails).
    canonical_id: String,
    /// Sanitized PluginTypeInfo — backend-owned identity fields are cleared.
    info: PluginTypeInfo,
}

impl ExternalDiscovery {
    /// Minimal info used when the get_info probe fails (plugin still loads,
    /// reported as version 0.0.0 with no capabilities — the canonical id falls
    /// back to the slugified display name).
    fn fallback_info(display_name: &str) -> PluginTypeInfo {
        PluginTypeInfo {
            display_name: display_name.to_string(),
            version: "0.0.0".to_string(),
            author: "external".to_string(),
            description: String::new(),
            capabilities: vec![],
            supported_protocols: None,
            series_identifier_label: None,
            series_identifier_placeholder: None,
            rate_limit: None,
            supports_test: false,
        }
    }
}

/// Parse "1.2.3" → [1, 2, 3] for version comparison (numeric prefix per segment).
fn parse_version(v: &str) -> Vec<u32> {
    v.split('.')
        .map(|s| {
            let digits: String = s.chars().take_while(|c| c.is_ascii_digit()).collect();
            digits.parse::<u32>().unwrap_or(0)
        })
        .collect()
}

/// Type-level collision resolution: if two directories discover the same
/// canonical id, the higher version wins. The loser is dropped and recorded as
/// Failed.
fn dedupe_external_discoveries(
    discoveries: &mut Vec<ExternalDiscovery>,
    registered: &mut Vec<RuntimePluginTypeInfo>,
) {
    let mut i = 0;
    while i < discoveries.len() {
        let mut dup_idx = None;
        for j in 0..i {
            if discoveries[j].canonical_id == discoveries[i].canonical_id {
                dup_idx = Some(j);
                break;
            }
        }
        let Some(j) = dup_idx else {
            i += 1;
            continue;
        };

        let new_ver = parse_version(&discoveries[i].info.version);
        let existing_ver = parse_version(&discoveries[j].info.version);
        if new_ver > existing_ver {
            tracing::info!(
                "Plugin ID collision: '{}' ({}) exists but new version ({}) > existing version ({}). Replacing.",
                discoveries[i].display_name,
                discoveries[i].canonical_id,
                discoveries[i].info.version,
                discoveries[j].info.version
            );
            discoveries.remove(j);
            i -= 1;
        } else {
            tracing::error!(
                "Plugin ID collision: '{}' ({}) is already loaded with version {} >= new version {}.",
                discoveries[i].display_name,
                discoveries[i].canonical_id,
                discoveries[j].info.version,
                discoveries[i].info.version
            );
            let loser = discoveries.remove(i);
            registered.push(RuntimePluginTypeInfo {
                id: loser.canonical_id.clone(),
                name: loser.display_name.clone(),
                category: "plugin".to_string(),
                state: RuntimePluginState::Failed(format!(
                    "Plugin ID collision: existing version {} >= new version {}",
                    discoveries[j].info.version, loser.info.version
                )),
            });
        }
    }
}

/// Enabled config instances for an external plugin type, across all categories.
///
/// Consistent with internal plugin loading: disabled instances are skipped.
fn find_config_instances(
    plugins_cfg: &PluginsConfig,
    type_id: &str,
) -> Vec<(String, serde_json::Value)> {
    let mut out = Vec::new();
    for cat_map in [
        &plugins_cfg.downloader,
        &plugins_cfg.notifier,
        &plugins_cfg.source,
        &plugins_cfg.metadata,
    ] {
        if let Some(instances) = cat_map.get(type_id) {
            for (instance_id, config) in instances {
                let enabled = config
                    .get("enabled")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                if enabled {
                    out.push((instance_id.clone(), config.clone()));
                }
            }
        }
    }
    out
}

/// Find an instance's config by its instance id, across all categories.
fn find_any_instance_config(
    plugins_cfg: &PluginsConfig,
    instance_id: &str,
) -> Option<serde_json::Value> {
    for cat_map in [
        &plugins_cfg.downloader,
        &plugins_cfg.notifier,
        &plugins_cfg.source,
        &plugins_cfg.metadata,
    ] {
        for instances in cat_map.values() {
            if let Some(config) = instances.get(instance_id) {
                return Some(config.clone());
            }
        }
    }
    None
}
