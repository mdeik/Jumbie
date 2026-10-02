use jumbie_shared::types::MappingRule;
use std::collections::HashMap;
use std::sync::Arc;

/// One metadata provider instance linked to a series, resolved to the full
/// metadata-cache key `(metadata_id, plugin_id, instance_id)`.
#[derive(Debug, Clone)]
pub struct ResolvedMetadataProvider {
    /// Provider INSTANCE id (UUID). Keys `metadata_ids`, the episode
    /// `metadata_ids` column, `metadata_source`, and `metadata_last_synced_at`.
    pub instance_id: String,
    /// Backend plugin TYPE id (e.g. `"jumbie.tvdb"`) — the cache key's provider column.
    pub plugin_id: String,
    /// The provider's external series id (the cache key's `metadata_id`).
    pub metadata_id: String,
}

/// Build an instance id → plugin id (backend **type** id) map from the plugins
/// config.
///
/// The config is keyed by `plugin_id` — the same value `/api/plugins` stamps as
/// `PluginInstanceInfo.plugin_id` (e.g. `"jumbie.tvdb"`) — so a config key IS the
/// type id. Metadata-cache lookups key on `(metadata_id, plugin_id, instance_id)`.
///
/// Call once per request and pass the map to all lookups rather than re-fetching
/// the plugins config.
pub async fn instance_plugin_id_map(db: &crate::db::DbManager) -> HashMap<String, String> {
    let plugins_cfg = db.get_plugins_config().await.unwrap_or_default();
    let mut map = HashMap::new();
    for (plugin_id, instances) in &plugins_cfg.metadata {
        for uuid in instances.keys() {
            map.insert(uuid.clone(), plugin_id.clone());
        }
    }
    map
}

/// Resolve a single instance id to its plugin id (backend **type** id).
///
/// Falls back to the instance id itself when the instance is not configured — a
/// stable, non-empty value so cache keys stay well-formed.
pub async fn instance_plugin_id(db: &crate::db::DbManager, instance_id: &str) -> String {
    instance_plugin_id_map(db)
        .await
        .get(instance_id)
        .cloned()
        .unwrap_or_else(|| instance_id.to_string())
}

/// SSoT: a series' metadata providers **in canonical priority order**, each
/// resolved to the full metadata-cache key `(metadata_id, plugin_id, instance_id)`.
///
/// `ordered` MUST come from the plugin manager (`ordered_metadata_providers` /
/// `get_plugins_by_capability`) so the order can never drift from the order
/// `fetch_metadata_for_series` writes in. Instances without a non-empty external
/// id for this series are skipped.
///
/// Cache readers iterate this list and take the first provider with stored data,
/// so the read order always matches the write order — never `HashMap` iteration
/// order.
pub fn ordered_series_providers(
    mapping: &MappingRule,
    ordered: &[Arc<dyn crate::plugins::PluginInstance>],
    instance_plugins: &HashMap<String, String>,
) -> Vec<ResolvedMetadataProvider> {
    ordered
        .iter()
        .filter_map(|p| {
            let instance_id = p.instance_id();
            let metadata_id = mapping.settings.metadata_ids.get(instance_id)?;
            if metadata_id.is_empty() {
                return None;
            }
            Some(ResolvedMetadataProvider {
                instance_id: instance_id.to_string(),
                plugin_id: instance_plugins
                    .get(instance_id)
                    .cloned()
                    .unwrap_or_else(|| instance_id.to_string()),
                metadata_id: metadata_id.clone(),
            })
        })
        .collect()
}
