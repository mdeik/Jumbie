use std::collections::HashMap;
use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State},
};
use jumbie_shared::{
    config::{PluginsConfig, ensure_single_metadata_plugin, instance_is_enabled},
    plugin::{
        Capability, PluginDisplaySortKey, PluginInstanceInfo, is_jumbie_plugin_id, plugin_type_key,
    },
    types::{PluginStatusEntry, SavePluginsSectionResponse, TestPluginPayload},
};
use serde_json;

use crate::api::AppState;
use crate::error::{AppError, IntoApiResponse};
use crate::plugins::PluginInstance;

pub async fn get_available_plugins() -> Json<Vec<jumbie_shared::plugin::PluginTypeListing>> {
    tracing::debug!("get_available_plugins called");
    let plugins = crate::plugins::registry::InternalPluginRegistry::get_available_plugins().await;
    let count = plugins.len();

    let mut listings: Vec<jumbie_shared::plugin::PluginTypeListing> = plugins
        .into_iter()
        .map(|(key, info)| {
            // Backend-derived type id (`author.plugin_name` format, SSoT
            // helper) — types are immutable metadata + backend identity.
            let short_name = plugin_type_key(&key);
            let mut listing: jumbie_shared::plugin::PluginTypeListing = info.into();
            listing.plugin_id = Some(crate::plugins::derive_plugin_id(
                &listing.author.clone(),
                short_name,
            ));
            listing
        })
        .collect();

    // Deterministic order: built-in plugins first, then the shared display order
    // (case-insensitive display name, `plugin_id` as tie-break — see
    // `PluginDisplaySortKey`, reused verbatim by the plugin status sort). The
    // registry iterates a HashMap, so without this the order is arbitrary.
    // Sorting here (API boundary) rather than in the registry keeps runtime
    // `plugin_order` — populated from registry iteration — unaffected. Cached
    // keys keep the lowercasing O(n) rather than O(n log n).
    listings.sort_by_cached_key(|p| {
        (
            std::cmp::Reverse(p.plugin_id.as_deref().is_some_and(is_jumbie_plugin_id)),
            PluginDisplaySortKey::new(&p.display_name, p.plugin_id.as_deref().unwrap_or("")),
        )
    });

    tracing::debug!("get_available_plugins completed: {} plugins", count);
    Json(listings)
}

/// Shared helper: inject reserved fields (name, enabled, priority, refresh_interval)
/// into a plugin's JSON schema. Used by both the single and batch schema endpoints
/// to keep the injection logic in one place.
fn inject_schema_fields(
    schema: &mut serde_json::Value,
    category: &str,
    plugin_type: &str,
    capabilities: &[Capability],
) {
    if let Some(obj) = schema.as_object_mut() {
        let props = obj
            .entry("properties")
            .or_insert_with(|| serde_json::json!({}));
        if let Some(props_obj) = props.as_object_mut() {
            let original_fields: HashMap<String, serde_json::Value> = [
                "name",
                "enabled",
                "priority",
                "refresh_interval",
                "enable_polling",
                "enable_manual_search",
                "enable_automatic_search",
                "enable_seeding",
            ]
            .iter()
            .filter_map(|&field| props_obj.get(field).map(|v| (field.to_string(), v.clone())))
            .collect();

            let had_illegal = props_obj.contains_key("name")
                || props_obj.contains_key("enabled")
                || props_obj.contains_key("priority")
                || props_obj.contains_key("refresh_interval")
                || props_obj.contains_key("enable_polling")
                || props_obj.contains_key("enable_manual_search")
                || props_obj.contains_key("enable_automatic_search")
                || props_obj.contains_key("enable_seeding");

            props_obj.remove("name");
            props_obj.remove("enabled");
            props_obj.remove("priority");
            props_obj.remove("refresh_interval");
            props_obj.remove("enable_polling");
            props_obj.remove("enable_manual_search");
            props_obj.remove("enable_automatic_search");
            props_obj.remove("enable_seeding");

            // Inject name — preserve plugin's default if it defined one
            let mut name_schema = serde_json::json!({
                "type": "string",
                "title": "Name",
                "description": "User-defined name for this plugin instance",
                "order": -20
            });
            if let Some(default) = original_fields.get("name").and_then(|v| v.get("default"))
                && let Some(obj) = name_schema.as_object_mut()
            {
                obj.insert("default".to_string(), default.clone());
            }
            props_obj.insert("name".to_string(), name_schema);

            // Inject enabled — preserve plugin's default if it defined one, else default to true
            let mut enabled_schema = serde_json::json!({
                "type": "boolean",
                "title": "Enabled",
                "description": "Whether this plugin instance is active",
                "default": true,
                "order": -10
            });
            if let Some(default) = original_fields
                .get("enabled")
                .and_then(|v| v.get("default"))
                && let Some(obj) = enabled_schema.as_object_mut()
            {
                obj.insert("default".to_string(), default.clone());
            }
            props_obj.insert("enabled".to_string(), enabled_schema);

            // Capability toggle schemas — injected only when the plugin TYPE declares
            // the matching capability (a toggle can disable a supported capability,
            // never grant one). The field→capability mapping comes from the SSoT in
            // `plugins::capabilities`, so the UI can never drift from dispatch.
            type ToggleDef = (
                &'static str,
                &'static str,
                &'static str,
                Option<&'static str>,
                i32,
            );
            const TOGGLES: &[ToggleDef] = &[
                (
                    "enable_manual_search",
                    "Enable Manual Search",
                    "Allow this source to be used for manual searches from the UI",
                    None,
                    -5,
                ),
                (
                    "enable_automatic_search",
                    "Enable Automatic Search",
                    "Allow this source to be used for automatic missing episode searches",
                    None,
                    -4,
                ),
                (
                    "enable_polling",
                    "Enable Polling",
                    "Periodically fetch new entries from this source",
                    Some("Periodically check for metadata updates"),
                    -3,
                ),
                (
                    "enable_seeding",
                    "Enable Seeding",
                    "Keep files seeding after organize instead of removing torrent immediately",
                    None,
                    -2,
                ),
            ];

            let is_metadata = category == "metadata";

            for &(field, title, desc_source, desc_metadata, order) in TOGGLES {
                let Some(capability) = crate::plugins::capabilities::capability_for_toggle(field)
                else {
                    continue;
                };
                if !capabilities.contains(&capability) {
                    continue;
                }

                let description = if is_metadata {
                    desc_metadata.unwrap_or(desc_source)
                } else {
                    desc_source
                };

                let mut schema = serde_json::json!({
                    "type": "boolean",
                    "title": title,
                    "description": description,
                    "default": true,
                    "order": order,
                    "parent": "enabled"
                });

                if let Some(default) = original_fields.get(field).and_then(|v| v.get("default"))
                    && let Some(obj) = schema.as_object_mut()
                {
                    obj.insert("default".to_string(), default.clone());
                }

                props_obj.insert(field.to_string(), schema);
            }

            if category == "downloader" {
                props_obj.insert("priority".to_string(), serde_json::json!({
                    "type": "integer",
                    "title": "Priority",
                    "description": "Affects the order in which downloaders are used. Higher values are attempted first.",
                    "default": 0,
                    "order": 1000
                }));
            }

            if category == "source" || category == "metadata" {
                let (title, description) = if category == "metadata" {
                    (
                        "Refresh Interval",
                        "How often to check for updated metadata (e.g. 12h, 1w)",
                    )
                } else {
                    (
                        "Refresh Interval",
                        "How often to check for new entries (e.g. 30m, 2h)",
                    )
                };

                // SSoT: the storage unit and default live in `plugins` so the
                // schema and the background loops can never disagree.
                let default = crate::plugins::refresh_interval_default(category);
                let time_unit = crate::plugins::refresh_interval_unit(category);

                let mut refresh_schema = serde_json::json!({
                    "type": "integer",
                    "format": "duration",
                    "x-time-unit": time_unit,
                    "title": title,
                    "description": description,
                    "default": default,
                    "order": 1001
                });

                if category == "metadata"
                    && let Some(obj) = refresh_schema.as_object_mut()
                {
                    obj.insert("minimum".to_string(), serde_json::json!(1));
                }

                if let Some(orig) = original_fields.get("refresh_interval")
                    && let Some(orig_obj) = orig.as_object()
                    && let Some(schema_obj) = refresh_schema.as_object_mut()
                {
                    for (key, value) in orig_obj {
                        if key != "type" && key != "title" {
                            schema_obj.insert(key.clone(), value.clone());
                        }
                    }
                }

                props_obj.insert("refresh_interval".to_string(), refresh_schema);
            }

            if had_illegal {
                tracing::debug!(
                    "Plugin '{}' defined reserved core fields (defaults will be preserved).",
                    plugin_type
                );
            }
        }
    }
}

pub async fn get_plugin_schema(
    State(_state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    tracing::debug!("get_plugin_schema called: plugin_id={}", id);
    // Handle both short ("qbittorrent") and full ("downloader.qbittorrent") plugin IDs
    let short_name = plugin_type_key(&id).to_lowercase();
    match crate::plugins::InternalPluginRegistry::get_schema(&short_name).await {
        Ok((mut schema, category, info)) => {
            inject_schema_fields(&mut schema, &category, &short_name, &info.capabilities);
            Ok(Json(schema))
        }
        Err(e) => {
            tracing::warn!("Schema lookup failed for '{}': {}", id, e);
            Err(AppError::NotFound(format!(
                "No schema available for plugin type '{}'",
                id
            )))
        }
    }
}

/// Fetch schemas for all registered plugin types in a single request.
pub async fn get_all_plugin_schemas() -> Result<Json<HashMap<String, serde_json::Value>>, AppError>
{
    tracing::debug!("get_all_plugin_schemas called");
    let schemas = crate::plugins::InternalPluginRegistry::get_all_schemas().await;
    let mut result = HashMap::new();
    for (plugin_type, mut schema, category, info) in schemas {
        inject_schema_fields(&mut schema, &category, &plugin_type, &info.capabilities);
        result.insert(plugin_type, schema);
    }
    Ok(Json(result))
}

pub async fn validate_plugin_config(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(payload): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, AppError> {
    tracing::debug!("validate_plugin_config called: plugin_id={}", id);
    let pm = state.plugin_manager.read().await;
    if let Some(host) = pm.get_plugin(&id) {
        match host.call("validate_config", Some(payload)).await {
            Ok(result) => Ok(Json(result)),
            Err(e) => Err(AppError::Internal(anyhow::anyhow!(format!(
                "Plugin error: {}",
                e
            )))),
        }
    } else {
        Err(AppError::NotFound("Plugin not found".to_string()))
    }
}

pub async fn get_plugins(State(state): State<Arc<AppState>>) -> Json<Vec<PluginInstanceInfo>> {
    tracing::debug!("get_plugins called");
    let pm = state.plugin_manager.read().await;

    let mut plugins = Vec::new();
    // ORDER (SSoT): priority DESC, then instance_id ASC — the same order the
    // manager uses for capability lookups, so the frontend's "first instance" is
    // the highest-priority one. Covers every loaded instance: downloaders,
    // notifiers, sources, metadata, and external hosts.
    let mut instances: Vec<Arc<dyn PluginInstance>> = pm
        .plugin_order
        .iter()
        .filter_map(|id| pm.plugins.get(id).cloned())
        .collect();
    instances.sort_by(|a, b| {
        b.priority()
            .cmp(&a.priority())
            .then_with(|| a.instance_id().cmp(b.instance_id()))
    });
    for plugin in instances {
        // Immutable TYPE metadata (SSoT: identical for every instance of a type)
        // plus the backend-owned instance identity, built into the flat DTO the
        // frontend consumes. Plugins never provide identity — it is derived here.
        let mut info: PluginInstanceInfo = plugin.plugin_info().into();
        let instance_id = plugin.instance_id().to_string();
        if let Some(type_id) = pm.get_instance_type_id(&instance_id) {
            info.plugin_id = Some(type_id.to_string());
            info.instance_id = Some(instance_id.clone());
        }

        // Backend-owned display name (user-configured instance name).
        let name = pm.get_instance_name(&instance_id);
        if !name.is_empty() {
            info.display_name = name;
        }
        plugins.push(info);
    }

    tracing::debug!("get_plugins completed: {} plugins", plugins.len());
    Json(plugins)
}

pub async fn test_plugin(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<TestPluginPayload>,
) -> Result<Json<String>, AppError> {
    tracing::info!(
        "Testing plugin: {}/{}",
        payload.category,
        payload.plugin_type
    );
    tracing::debug!(
        "test_plugin called: category={}, plugin_type={}",
        payload.category,
        payload.plugin_type
    );

    let key = format!(
        "{}.{}",
        payload.category,
        payload.plugin_type.to_lowercase()
    );

    // Force enabled: true so tests work even when the plugin is currently disabled
    let mut test_config = payload.config.clone();
    if let Some(obj) = test_config.as_object_mut() {
        obj.insert("enabled".to_string(), serde_json::json!(true));
    }

    let global_config = state.cfg.read().await;
    let shutdown_token = state.plugin_manager.read().await.shutdown_token();
    let plugin = match crate::plugins::registry::InternalPluginRegistry::create(
        &key,
        test_config,
        "test_instance".to_string(),
        0,    // priority
        None, // refresh_interval
        Arc::new(global_config.clone()),
        shutdown_token,
    )
    .await
    {
        Ok(p) => p,
        Err(e) => {
            tracing::error!("Failed to create plugin for test (key: {}): {}", key, e);
            return Err(AppError::BadRequest(format!(
                "Failed to create plugin: {}",
                e
            )));
        }
    };

    // The unified "test" method runs an operational connectivity check (API
    // credentials, webhook URLs, reachability), separate from "health_check"
    // which verifies plugin-backend IPC.
    let method = "test";

    let timeout = std::time::Duration::from_secs(10);
    tracing::info!(
        "Running test on plugin '{}' ({})",
        plugin.instance_id(),
        key
    );
    match tokio::time::timeout(timeout, plugin.call(method, None)).await {
        Ok(Ok(result_val)) => {
            let message = result_val
                .as_str()
                .map(|s| s.to_string())
                .unwrap_or_else(|| format!("Test passed for {}", payload.plugin_type));
            tracing::info!("Test successful for plugin {}", key);
            Ok(Json(message))
        }
        Ok(Err(e)) => {
            tracing::error!("Test failed for plugin {}: {}", key, e);
            Err(AppError::BadRequest(format!("Test failed: {}", e)))
        }
        Err(_) => {
            tracing::error!("Test timed out for plugin {}", key);
            Err(AppError::RequestTimeout("Test timed out".to_string()))
        }
    }
}

pub async fn get_plugin_status(State(state): State<Arc<AppState>>) -> Json<Vec<PluginStatusEntry>> {
    tracing::debug!("get_plugin_status called");
    let mut entries: Vec<PluginStatusEntry> = Vec::new();

    // Reads IN-PROCESS state only, with no plugin RPCs: external type health comes
    // from the type host's last probe (an atomic) and per-instance failure state
    // from the wrapper's cooldown. Any additional plugin metadata should be
    // populated on `RuntimePluginTypeInfo` at registration time, not queried here.
    let pm = state.plugin_manager.read().await;
    let mut statuses = pm.get_plugin_statuses();

    // Deterministic display order (see `sort_plugin_statuses`). Resolved via the
    // backend-owned instance→type map so the tie-break matches the type id the
    // frontend sees, without the client having to rank anything itself.
    crate::plugins::sort_plugin_statuses(&mut statuses, |id| {
        pm.get_instance_type_id(id).map(str::to_string)
    });

    for info in statuses {
        match info.state {
            crate::plugins::RuntimePluginState::Loaded(plugin) => {
                // IN-PROCESS health: external instances report the type host's last
                // probe result; failure cooldowns / 429 pauses surface cached reasons.
                let (ok, message) = plugin.health_status().await;
                entries.push(PluginStatusEntry {
                    name: info.name,
                    category: info.category,
                    ok,
                    message,
                });
            }
            crate::plugins::RuntimePluginState::Failed(err) => {
                entries.push(PluginStatusEntry {
                    name: info.name,
                    category: info.category,
                    ok: false,
                    message: Some(err),
                });
            }
        }
    }

    Json(entries)
}

// Dedicated endpoints for DB-stored config sections

pub async fn get_plugins_config_endpoint(
    State(state): State<Arc<AppState>>,
) -> Result<Json<PluginsConfig>, AppError> {
    tracing::debug!("get_plugins_config called");
    state.db.get_plugins_config().await.into_json_response()
}

/// Create a new plugin instance with a server-generated ID.
///
/// The frontend sends the plugin config body and the backend assigns a 12-char
/// hex instance ID (48 bits of randomness). This ensures the backend is the
/// single source of truth for instance IDs — users can't inject arbitrary IDs.
///
/// The generated ID is short enough for alias/pattern prefixes (`@id:value`)
/// while being practically unique for any realistic number of plugin instances.
pub async fn create_plugin_instance_endpoint(
    State(state): State<Arc<AppState>>,
    Path((section, plugin_id)): Path<(String, String)>,
    Json(config): Json<serde_json::Value>,
) -> Result<Json<PluginsConfig>, AppError> {
    tracing::debug!("create_plugin_instance called: {}/{}", section, plugin_id);

    if !["downloader", "notifier", "source", "metadata"].contains(&&section[..]) {
        return Err(AppError::BadRequest(format!(
            "Invalid plugin section: {}",
            section
        )));
    }

    // Generate the new instance ID (12 hex chars from a UUID v4).
    let instance_id = &uuid::Uuid::new_v4().simple().to_string()[..12];

    let mut full_cfg = state.db.get_plugins_config().await.unwrap_or_default();

    let target_map = match section.as_str() {
        "downloader" => &mut full_cfg.downloader,
        "notifier" => &mut full_cfg.notifier,
        "source" => &mut full_cfg.source,
        "metadata" => &mut full_cfg.metadata,
        _ => unreachable!(),
    };

    let instances = target_map
        .entry(plugin_id.clone())
        .or_insert_with(HashMap::new);
    instances.insert(instance_id.to_string(), config);

    // Only one metadata plugin may be enabled at a time: enabling the new instance
    // auto-disables any other enabled metadata plugins.
    if section == "metadata" {
        ensure_single_metadata_plugin(&mut full_cfg, Some((&plugin_id, instance_id)));
    }

    state
        .db
        .save_plugins_config(&full_cfg)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    // Hot-reload via apply_config: saves only touch instances whose config
    // changed (external reconfigured in place, internal rebuilt from factory).
    let global_cfg = Arc::new(state.cfg.read().await.clone());
    let mut pm = state.plugin_manager.write().await;
    pm.apply_config(&full_cfg, global_cfg).await;

    Ok(Json(full_cfg))
}

/// Check all instances in a section payload for missing required fields.
pub async fn put_plugins_section_endpoint(
    State(state): State<Arc<AppState>>,
    Path(section): Path<String>,
    Json(mut payload): Json<HashMap<String, HashMap<String, serde_json::Value>>>,
) -> Result<Json<SavePluginsSectionResponse>, AppError> {
    tracing::debug!("put_plugins_section called: {}", section);

    // Validate section name
    if !["downloader", "notifier", "source", "metadata"].contains(&&section[..]) {
        return Err(AppError::BadRequest(format!(
            "Invalid plugin section: {}",
            section
        )));
    }

    // Only one metadata plugin may be enabled at a time. Pick the instance the
    // user just enabled as the preferred winner so the shared enforcement
    // preserves user intent.
    let mut preferred_winner: Option<(String, String)> = None;
    if section == "metadata" {
        let current_cfg = state.db.get_plugins_config().await.unwrap_or_default();

        // Previously-enabled instances (normally 0 or 1; sorted for determinism if
        // a hand-edited config ever contains more than one).
        let mut previously_enabled: Vec<(String, String)> = current_cfg
            .metadata
            .iter()
            .flat_map(|(plugin_id, instances)| {
                instances
                    .iter()
                    .filter(|(_, config)| instance_is_enabled(config))
                    .map(move |(instance_id, _)| (plugin_id.clone(), instance_id.clone()))
            })
            .collect();
        previously_enabled.sort();

        // Enabled instances in the incoming payload (sorted → deterministic winner).
        let mut payload_enabled: Vec<(String, String)> = payload
            .iter()
            .flat_map(|(plugin_id, instances)| {
                instances
                    .iter()
                    .filter(|(_, config)| instance_is_enabled(config))
                    .map(move |(instance_id, _)| (plugin_id.clone(), instance_id.clone()))
            })
            .collect();
        payload_enabled.sort();

        if payload_enabled.len() > 1 {
            // Prefer the instance the user just turned on (not enabled before),
            // falling back to the deterministic first.
            preferred_winner = payload_enabled
                .iter()
                .find(|p| !previously_enabled.contains(p))
                .or_else(|| payload_enabled.first())
                .cloned();
        }
    }

    // Names must match `[a-zA-Z0-9 ]` as they are used as slug prefixes.
    let section_label = match section.as_str() {
        "downloader" => "Downloader",
        "notifier" => "Notifier",
        "source" => "Source",
        "metadata" => "Metadata",
        _ => unreachable!(), // validated above
    };
    super::validate_plugin_instance_names(&payload, section_label)?;

    // Enabled plugins missing required fields are auto-disabled, preventing the
    // confusing state of a green "Enabled" badge on a non-functional plugin.
    let plugins_dir = state.cfg.read().await.plugins_dir.clone();
    let warnings = validate_section_required_fields(&mut payload, &plugins_dir).await;

    // Save only this section, with any auto-corrections applied.
    state
        .db
        .save_plugins_section(&section, &payload)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    let mut full_cfg = state
        .db
        .get_plugins_config()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    // Enforce single metadata plugin on the full config (SSoT: shared logic —
    // `jumbie_shared::config::ensure_single_metadata_plugin`). Re-save if
    // enforcement changed anything.
    if section == "metadata" {
        let disabled = ensure_single_metadata_plugin(
            &mut full_cfg,
            preferred_winner
                .as_ref()
                .map(|(a, b)| (a.as_str(), b.as_str())),
        );
        if !disabled.is_empty() {
            state
                .db
                .save_plugins_config(&full_cfg)
                .await
                .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
        }
    }

    // Hot-reload via apply_config: saves only touch instances whose config
    // changed (external reconfigured in place, internal rebuilt from factory).
    let global_cfg = Arc::new(state.cfg.read().await.clone());
    let mut pm = state.plugin_manager.write().await;
    pm.apply_config(&full_cfg, global_cfg).await;

    Ok(Json(SavePluginsSectionResponse {
        config: full_cfg,
        warnings,
    }))
}

/// Check all instances in a section payload for missing required fields.
/// Any enabled plugin with empty required fields is auto-disabled,
/// and a human-readable warning is returned.
///
/// Uses the plugin's JSON Schema `required` array (standard JSON Schema)
/// to determine which fields are mandatory.  Works for:
///   • Internal plugins — schemas are fetched from the registry.
///   • External plugins — if a `schema.json` exists in the plugin's
///     directory (`plugins_dir/<plugin_type>/schema.json`).
///   • Third-party plugins without a schema — silently skipped.
async fn validate_section_required_fields(
    payload: &mut HashMap<String, HashMap<String, serde_json::Value>>,
    plugins_dir: &std::path::Path,
) -> Vec<String> {
    let mut warnings: Vec<String> = Vec::new();

    for (plugin_id, instances) in payload.iter_mut() {
        // Try the internal schema registry first, then fall back to
        // reading a schema.json from the external plugin's directory.
        let schema = match crate::plugins::InternalPluginRegistry::get_schema(plugin_id).await {
            Ok((schema, _, _)) => schema,
            Err(_) => {
                // Check for external plugin schema.json on disk
                let schema_path = plugins_dir.join(plugin_id).join("schema.json");
                match tokio::fs::read_to_string(&schema_path).await {
                    Ok(text) => match serde_json::from_str::<serde_json::Value>(&text) {
                        Ok(s) => s,
                        Err(_) => continue,
                    },
                    Err(_) => continue,
                }
            }
        };

        // Fill in schema defaults for properties entirely missing from the config
        // JSON (e.g. a fresh instance created as `{}` still gets `enabled: true` and
        // a default name), mirroring the frontend form editor. Empty strings are NOT
        // replaced — an explicitly cleared field stays cleared.
        if let Some(props) = schema.get("properties").and_then(|p| p.as_object()) {
            for value in instances.values_mut() {
                if let Some(obj) = value.as_object_mut() {
                    for (prop_name, prop_schema) in props {
                        if !obj.contains_key(prop_name)
                            && let Some(default) = prop_schema.get("default")
                        {
                            obj.insert(prop_name.clone(), default.clone());
                        }
                    }
                }
            }
        }

        let Some(fields) = schema.get("required").and_then(|r| r.as_array()) else {
            continue;
        };

        if fields.is_empty() {
            continue;
        }

        for (instance_id, value) in instances.iter_mut() {
            let is_enabled = value
                .get("enabled")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            if !is_enabled {
                continue;
            }

            let mut missing: Vec<String> = Vec::new();
            for field in fields {
                let field_name = match field.as_str() {
                    Some(name) => name,
                    None => continue,
                };
                // Field is still missing/empty even after defaults were applied.
                // That means the schema has no default and the user didn't fill it in.
                match value.get(field_name) {
                    Some(serde_json::Value::String(s)) if s.is_empty() => {
                        missing.push(field_name.to_string());
                    }
                    None => {
                        missing.push(field_name.to_string());
                    }
                    _ => {}
                }
            }

            if !missing.is_empty() {
                // Auto-disable the plugin
                if let Some(obj) = value.as_object_mut() {
                    obj.insert("enabled".to_string(), serde_json::Value::Bool(false));
                }
                // Use the user-defined name if available, falling back to the instance ID.
                let instance_label = value
                    .get("name")
                    .and_then(|n| n.as_str())
                    .filter(|s| !s.is_empty())
                    .unwrap_or(instance_id);
                warnings.push(format!(
                    "Disabled '{}' instance '{}': missing required field(s): {}",
                    plugin_id,
                    instance_label,
                    missing.join(", ")
                ));
            }
        }
    }

    warnings
}

pub async fn delete_plugin_instance_endpoint(
    State(state): State<Arc<AppState>>,
    Path((section, plugin_id, instance_id)): Path<(String, String, String)>,
) -> Result<Json<PluginsConfig>, AppError> {
    tracing::debug!(
        "delete_plugin_instance: {}/{}/{}",
        section,
        plugin_id,
        instance_id
    );

    if !["downloader", "notifier", "source", "metadata"].contains(&&section[..]) {
        return Err(AppError::BadRequest(format!(
            "Invalid plugin section: {}",
            section
        )));
    }

    state
        .db
        .delete_plugin_instance(&section, &plugin_id, &instance_id)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    // Deleting a metadata instance must remove all cached data keyed by its
    // instance_id so stale entries can't linger.
    if section == "metadata" {
        let instance_str = &instance_id;
        let _ = state
            .db
            .delete_metadata_episodes_cache_for_instance(instance_str)
            .await;
        let _ = state
            .db
            .delete_metadata_season_cache_for_instance(instance_str)
            .await;
        let _ = state
            .db
            .delete_metadata_series_cache_for_instance(instance_str)
            .await;
        let _ = state
            .db
            .delete_metadata_fetch_log_for_instance(instance_str)
            .await;
    }

    let full_cfg = state
        .db
        .get_plugins_config()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    // Hot-reload via apply_config: only instances whose config changed are touched
    // (external reconfigured in place, internal rebuilt from factory).
    let global_cfg = Arc::new(state.cfg.read().await.clone());
    let mut pm = state.plugin_manager.write().await;
    pm.apply_config(&full_cfg, global_cfg).await;

    Ok(Json(full_cfg))
}
