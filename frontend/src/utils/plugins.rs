use jumbie_shared::config::{PluginsConfig, instance_is_enabled};
use jumbie_shared::plugin::{Capability, PluginInstanceInfo, plugin_type_key};

/// Resolve active (enabled) metadata plugin instances from the backend-served
/// instance list plus the plugins config.
///
/// Instance identity is backend-generated (`/api/plugins` stamps `plugin_id` +
/// `instance_id`) and the list arrives in priority order; this preserves that order —
/// loaded instances first (priority DESC, instance_id ASC), then enabled-but-unloaded
/// config instances (e.g. an external plugin not installed) so their ID field renders.
///
/// Multiple metadata providers are supported; callers must not assume a single active
/// provider.
pub fn resolve_active_metadata_plugins(
    instance_plugins: Vec<PluginInstanceInfo>,
    cfg: &PluginsConfig,
) -> Vec<PluginInstanceInfo> {
    let is_metadata = |p: &PluginInstanceInfo| {
        p.capabilities.contains(&Capability::MetadataProviderNormal)
            || p.capabilities
                .contains(&Capability::MetadataProviderAbsolute)
    };
    let cfg_enabled = |instance_cfg: &serde_json::Value| -> bool {
        instance_cfg
            .as_object()
            .and_then(|t| t.get("enabled"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    };

    // Enabled metadata instance ids from config.
    let enabled: std::collections::HashSet<&str> = cfg
        .metadata
        .values()
        .flat_map(|instances| instances.iter())
        .filter(|(_, instance_cfg)| cfg_enabled(instance_cfg))
        .map(|(instance_id, _)| instance_id.as_str())
        .collect();

    let mut result: Vec<PluginInstanceInfo> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    // Loaded instances, in backend (priority) order.
    for p in instance_plugins.into_iter().filter(is_metadata) {
        if let Some(iid) = p.instance_id.as_deref()
            && enabled.contains(iid)
        {
            seen.insert(iid.to_string());
            result.push(p);
        }
    }

    // Enabled-but-unloaded config instances (fallback entries), deterministic by id.
    let mut fallbacks: Vec<(String, String)> = Vec::new();
    for (plugin_key, instances) in &cfg.metadata {
        for (instance_id, instance_cfg) in instances {
            if seen.contains(instance_id) || !cfg_enabled(instance_cfg) {
                continue;
            }
            let instance_name = instance_cfg
                .as_object()
                .and_then(|t| t.get("name"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| plugin_type_key(plugin_key).to_string());
            fallbacks.push((instance_id.clone(), instance_name));
        }
    }
    fallbacks.sort_by(|a, b| a.0.cmp(&b.0));
    for (instance_id, display_name) in fallbacks {
        result.push(PluginInstanceInfo {
            plugin_id: None,
            instance_id: Some(instance_id),
            display_name,
            version: String::new(),
            author: String::new(),
            description: String::new(),
            capabilities: Vec::new(),
            supported_protocols: None,
            series_identifier_label: None,
            series_identifier_placeholder: None,
            rate_limit: None,
            supports_test: false,
        });
    }
    result
}

/// Display name for a metadata instance, falling back to the plugin type key.
fn metadata_instance_name(cfg: &PluginsConfig, plugin_key: &str, instance_id: &str) -> String {
    cfg.metadata
        .get(plugin_key)
        .and_then(|instances| instances.get(instance_id))
        .and_then(|value| value.get("name"))
        .and_then(|name| name.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| plugin_type_key(plugin_key).to_string())
}

/// Enforce the single-active metadata policy on a local config before saving it.
/// Returns the display names of the instances that were disabled (empty when the config
/// already satisfies the policy).
///
/// Delegates to `jumbie_shared::config::ensure_single_metadata_plugin` so frontend and
/// backend agree on which provider survives. Disabling happens client-side so the PUT
/// payload is already valid and the UI can explain the change.
pub fn enforce_single_metadata(
    cfg: &mut PluginsConfig,
    preferred: Option<(&str, &str)>,
) -> Vec<String> {
    jumbie_shared::config::ensure_single_metadata_plugin(cfg, preferred)
        .into_iter()
        .map(|(plugin_key, instance_id)| metadata_instance_name(cfg, &plugin_key, &instance_id))
        .collect()
}

/// Display names of metadata providers enabled in `before` but disabled in
/// `after`.
///
/// Used after a create call (where the backend assigns the instance id and does
/// the enforcement) so the UI can explain which provider was switched off.
pub fn newly_disabled_metadata_providers(
    before: &PluginsConfig,
    after: &PluginsConfig,
) -> Vec<String> {
    let mut names: Vec<String> = before
        .metadata
        .iter()
        .flat_map(|(plugin_key, instances)| {
            instances
                .iter()
                .filter(|(_, value)| instance_is_enabled(value))
                .filter(|(instance_id, _)| {
                    let still_enabled = after
                        .metadata
                        .get(plugin_key)
                        .and_then(|m| m.get(*instance_id))
                        .is_some_and(instance_is_enabled);
                    !still_enabled
                })
                .map(move |(instance_id, _)| {
                    metadata_instance_name(before, plugin_key, instance_id)
                })
        })
        .collect();
    names.sort();
    names
}
