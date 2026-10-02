pub mod config;
pub mod plugins;
pub mod profiles;

pub use config::*;
pub use plugins::*;
pub use profiles::*;

use anyhow::Result;
use jumbie_shared::config::Config;

use crate::error::AppError;

/// Validate all plugin instance "name" fields within a plugin-type map.
/// Each entry is validated against [`crate::validation::validate_plugin_name`]
/// and errors are tagged with the given `plugin_category_label` for clear messaging.
fn validate_plugin_instance_names(
    instances_map: &std::collections::HashMap<
        String,
        std::collections::HashMap<String, serde_json::Value>,
    >,
    plugin_category_label: &str,
) -> Result<(), AppError> {
    for instances in instances_map.values() {
        for config in instances.values() {
            if let Some(name_val) = config.get("name").and_then(|v| v.as_str()) {
                crate::validation::validate_plugin_name(name_val).map_err(|e| {
                    AppError::BadRequest(format!(
                        "{} plugin '{}': {}",
                        plugin_category_label, name_val, e.0
                    ))
                })?;
            }
        }
    }
    Ok(())
}

/// Save a config reference to disk.
///
/// Auth fields are cleared first: the DB is the source of truth for API keys,
/// calendar tokens, banned IPs, and subnet whitelists. Writing stale copies to
/// the config file would create a second, possibly out-of-sync copy that could
/// later be restored from disk and overwrite the DB — and could leak secrets if
/// the file is committed.
pub async fn persist_config(config: &mut Config, path: &str) -> Result<()> {
    config.auth.api_keys.clear();
    config.auth.calendar_tokens.clear();
    config.auth.banned_ips.clear();
    config.auth.subnet_whitelist.clear();
    config.save(path)?;
    Ok(())
}
