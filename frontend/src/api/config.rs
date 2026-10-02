use super::{delete_json, delete_unit, diff_sections, get, post, post_unit, put_json, put_unit};
use crate::api_client::ApiError as Error;
use jumbie_shared::config::Config;
use jumbie_shared::plugin::{PluginInstanceInfo, PluginTypeListing};

/// Fetches all settings data in a single round-trip via the /api/bootstrap endpoint.
/// Callers unpack the response into individual cache keys.
pub async fn fetch_bootstrap() -> Result<jumbie_shared::types::BootstrapData, Error> {
    crate::debug_log!("fetch_bootstrap()");
    let result: Result<jumbie_shared::types::BootstrapData, Error> = get("bootstrap").await;
    match &result {
        Ok(_) => crate::debug_log!("fetch_bootstrap succeeded"),
        Err(e) => crate::debug_error!("fetch_bootstrap failed: {}", e),
    }
    result
}

// Config Endpoints

pub async fn fetch_config() -> Result<Config, Error> {
    crate::debug_log!("fetch_config()");
    let config: Config = get("config").await?;

    // Sync LAST_SAVED_CONFIG so the diff cache reflects what the server actually has.
    // Without this, a page reload followed by re-saving the same password would be
    // suppressed by the diff mechanism (cache still holds the old raw password from
    // the previous session, while the server returns "********").
    let sections = jumbie_shared::types::UpdateConfigPayload {
        organization: Some(config.organization.clone()),
        sources: Some(config.sources.clone()),
        general: Some(config.general.clone()),
        proxy: Some(config.proxy.clone()),
        auth: Some(config.auth.clone()),
        security: Some(config.security.clone()),
    };
    if let Ok(json) = serde_json::to_value(&sections)
        && let Ok(mut cache) = super::LAST_SAVED_CONFIG.lock()
    {
        *cache = Some(json);
    }

    crate::debug_log!("fetch_config succeeded");
    Ok(config)
}

/// Saves config sections to the backend, sending only sections that changed since
/// the last successful save (partial update).
///
/// The backend's `UpdateConfigPayload` has all-Option fields and fully supports
/// partial updates: each section is serialized to JSON and compared against the
/// last saved state, with unchanged sections sent as `None`. Callers still pass a
/// full `Config`; the diffing is transparent.
pub async fn save_config(config: Config) -> Result<(), Error> {
    crate::debug_log!("save_config()");
    let sections = jumbie_shared::types::UpdateConfigPayload {
        organization: Some(config.organization),
        sources: Some(config.sources),
        general: Some(config.general),
        proxy: Some(config.proxy),
        auth: Some(config.auth),
        security: Some(config.security),
    };

    let body = diff_sections(&sections);
    put_unit("config", &body).await?;

    // Update the cache on success so the next save can diff against it
    if let Ok(json) = serde_json::to_value(&sections)
        && let Ok(mut cache) = super::LAST_SAVED_CONFIG.lock()
    {
        *cache = Some(json);
    }

    Ok(())
}

pub async fn fetch_qualities()
-> Result<std::collections::HashMap<String, jumbie_shared::types::Quality>, Error> {
    crate::debug_log!("fetch_qualities()");
    let result: Result<std::collections::HashMap<String, jumbie_shared::types::Quality>, Error> =
        get("config/qualities").await;
    match &result {
        Ok(q) => crate::debug_log!("fetch_qualities: got {} qualities", q.len()),
        Err(e) => crate::debug_error!("fetch_qualities failed: {}", e),
    }
    result
}

pub async fn save_qualities(
    map: &std::collections::HashMap<String, jumbie_shared::types::Quality>,
) -> Result<(), Error> {
    crate::debug_log!("save_qualities({} items)", map.len());
    let result = put_unit("config/qualities", map).await;
    match &result {
        Ok(_) => crate::debug_log!("save_qualities succeeded"),
        Err(e) => crate::debug_error!("save_qualities failed: {}", e),
    }
    result
}

pub async fn fetch_quality_profiles()
-> Result<std::collections::HashMap<String, jumbie_shared::types::QualityProfile>, Error> {
    crate::debug_log!("fetch_quality_profiles()");
    let result: Result<
        std::collections::HashMap<String, jumbie_shared::types::QualityProfile>,
        Error,
    > = get("config/quality_profiles").await;
    match &result {
        Ok(p) => crate::debug_log!("fetch_quality_profiles: got {} profiles", p.len()),
        Err(e) => crate::debug_error!("fetch_quality_profiles failed: {}", e),
    }
    result
}

pub async fn save_quality_profiles(
    map: &std::collections::HashMap<String, jumbie_shared::types::QualityProfile>,
) -> Result<(), Error> {
    crate::debug_log!("save_quality_profiles({} items)", map.len());
    let result = put_unit("config/quality_profiles", map).await;
    match &result {
        Ok(_) => crate::debug_log!("save_quality_profiles succeeded"),
        Err(e) => crate::debug_error!("save_quality_profiles failed: {}", e),
    }
    result
}

pub async fn fetch_release_profiles()
-> Result<std::collections::HashMap<String, jumbie_shared::scoring::ReleaseProfile>, Error> {
    crate::debug_log!("fetch_release_profiles()");
    let result: Result<
        std::collections::HashMap<String, jumbie_shared::scoring::ReleaseProfile>,
        Error,
    > = get("config/release_profiles").await;
    match &result {
        Ok(p) => crate::debug_log!("fetch_release_profiles: got {} profiles", p.len()),
        Err(e) => crate::debug_error!("fetch_release_profiles failed: {}", e),
    }
    result
}

pub async fn save_release_profiles(
    map: &std::collections::HashMap<String, jumbie_shared::scoring::ReleaseProfile>,
) -> Result<(), Error> {
    crate::debug_log!("save_release_profiles({} items)", map.len());
    let result = put_unit("config/release_profiles", map).await;
    match &result {
        Ok(_) => crate::debug_log!("save_release_profiles succeeded"),
        Err(e) => crate::debug_error!("save_release_profiles failed: {}", e),
    }
    result
}

pub async fn fetch_ui_preferences() -> Result<jumbie_shared::config::UIConfig, Error> {
    crate::debug_log!("fetch_ui_preferences()");
    let result = get("config/ui_preferences").await;
    match &result {
        Ok(_) => crate::debug_log!("fetch_ui_preferences succeeded"),
        Err(e) => crate::debug_error!("fetch_ui_preferences failed: {}", e),
    }
    result
}

pub async fn save_ui_preferences(ui: &jumbie_shared::config::UIConfig) -> Result<(), Error> {
    crate::debug_log!("save_ui_preferences()");
    let result = put_unit("config/ui_preferences", ui).await;
    match &result {
        Ok(_) => crate::debug_log!("save_ui_preferences succeeded"),
        Err(e) => crate::debug_error!("save_ui_preferences failed: {}", e),
    }
    result
}

pub async fn fetch_plugins_cfg() -> Result<jumbie_shared::config::PluginsConfig, Error> {
    crate::debug_log!("fetch_plugins_cfg()");
    let result = get("config/plugins_cfg").await;
    match &result {
        Ok(_) => crate::debug_log!("fetch_plugins_cfg succeeded"),
        Err(e) => crate::debug_error!("fetch_plugins_cfg failed: {}", e),
    }
    result
}

/// Create a new plugin instance on the backend — the backend generates the
/// instance ID as part of the save, ensuring the server is the single source
/// of truth for instance IDs.
pub async fn create_plugin_instance(
    section: &str,
    plugin_id: &str,
    config: &serde_json::Value,
) -> Result<jumbie_shared::config::PluginsConfig, Error> {
    let result: Result<jumbie_shared::config::PluginsConfig, Error> = post(
        &format!("config/plugins_cfg/{}/{}/instances", section, plugin_id),
        config,
    )
    .await;
    match &result {
        Ok(_) => crate::debug_log!("create_plugin_instance succeeded"),
        Err(e) => crate::debug_error!("create_plugin_instance failed: {}", e),
    }
    result
}

pub async fn save_plugins_section(
    section: &str,
    data: &std::collections::HashMap<String, std::collections::HashMap<String, serde_json::Value>>,
) -> Result<jumbie_shared::types::SavePluginsSectionResponse, Error> {
    crate::debug_log!("save_plugins_section({})", section);
    let result: Result<jumbie_shared::types::SavePluginsSectionResponse, Error> =
        put_json(&format!("config/plugins_cfg/{}", section), data).await;
    match &result {
        Ok(r) => crate::debug_log!(
            "save_plugins_section succeeded ({} warnings)",
            r.warnings.len()
        ),
        Err(e) => crate::debug_error!("save_plugins_section failed: {}", e),
    }
    result
}

pub async fn delete_plugin_instance(
    section: &str,
    plugin_id: &str,
    instance_id: &str,
) -> Result<jumbie_shared::config::PluginsConfig, Error> {
    crate::debug_log!(
        "delete_plugin_instance({}/{}/{})",
        section,
        plugin_id,
        instance_id
    );
    let result = delete_json(&format!(
        "config/plugins_cfg/{}/{}/{}",
        section, plugin_id, instance_id
    ))
    .await;
    match &result {
        Ok(_) => crate::debug_log!("delete_plugin_instance succeeded"),
        Err(e) => crate::debug_error!("delete_plugin_instance failed: {}", e),
    }
    result
}

// Plugin Endpoints

/// Returns installed plugins (with their current config values).
pub async fn fetch_plugins() -> Result<Vec<PluginInstanceInfo>, Error> {
    crate::debug_log!("fetch_plugins()");
    let result: Result<Vec<PluginInstanceInfo>, Error> = get("plugins").await;
    match &result {
        Ok(p) => crate::debug_log!("fetch_plugins: got {} plugins", p.len()),
        Err(e) => crate::debug_error!("fetch_plugins failed: {}", e),
    }
    result
}

/// Returns plugins available in the catalog that the user hasn't installed yet.
pub async fn fetch_available_plugins() -> Result<Vec<PluginTypeListing>, Error> {
    crate::debug_log!("fetch_available_plugins()");
    let result: Result<Vec<PluginTypeListing>, Error> = get("plugins/available").await;
    match &result {
        Ok(p) => crate::debug_log!("fetch_available_plugins: got {} plugins", p.len()),
        Err(e) => crate::debug_error!("fetch_available_plugins failed: {}", e),
    }
    result
}

/// Returns the JSON Schema definition for a plugin's config — used to dynamically
/// render the settings form for each plugin type.
pub async fn fetch_plugin_schema(plugin_id: &str) -> Result<serde_json::Value, Error> {
    crate::debug_log!("fetch_plugin_schema({})", plugin_id);
    let result = get(&format!("plugins/{}/schema", plugin_id)).await;
    match &result {
        Ok(_) => crate::debug_log!("fetch_plugin_schema({}) succeeded", plugin_id),
        Err(e) => crate::debug_error!("fetch_plugin_schema({}) failed: {}", plugin_id, e),
    }
    result
}

/// Fetches schemas for all registered plugins in a single request.
/// Returns a map of plugin_type → schema. Used by the preload system to
/// avoid N individual round-trips.
pub async fn fetch_all_plugin_schemas()
-> Result<std::collections::HashMap<String, serde_json::Value>, Error> {
    crate::debug_log!("fetch_all_plugin_schemas()");
    let result: Result<std::collections::HashMap<String, serde_json::Value>, Error> =
        get("plugins/schemas").await;
    match &result {
        Ok(s) => crate::debug_log!("fetch_all_plugin_schemas: got {} schemas", s.len()),
        Err(e) => crate::debug_error!("fetch_all_plugin_schemas failed: {}", e),
    }
    result
}

/// Validates a plugin's config against its schema without saving it.
/// Used for real-time feedback in the plugin settings form.
pub async fn validate_plugin_config(
    plugin_id: &str,
    config: &serde_json::Value,
) -> Result<(), Error> {
    crate::debug_log!("validate_plugin_config({})", plugin_id);
    let result = post_unit(&format!("plugins/{}/validate", plugin_id), config).await;
    match &result {
        Ok(_) => crate::debug_log!("validate_plugin_config({}) succeeded", plugin_id),
        Err(e) => crate::debug_error!("validate_plugin_config({}) failed: {}", plugin_id, e),
    }
    result
}

/// Returns the runtime status of all plugins (enabled/disabled, errors, etc.).
pub async fn fetch_plugin_status() -> Result<Vec<jumbie_shared::types::PluginStatusEntry>, Error> {
    crate::debug_log!("fetch_plugin_status()");
    let result: Result<Vec<jumbie_shared::types::PluginStatusEntry>, Error> =
        get("plugins/status").await;
    match &result {
        Ok(s) => crate::debug_log!("fetch_plugin_status: got {} entries", s.len()),
        Err(e) => crate::debug_error!("fetch_plugin_status failed: {}", e),
    }
    result
}

/// Tests a plugin configuration by running a live connection or operation.
/// The test is async and may take several seconds — the frontend shows a spinner.
/// Returns a human-readable message from the backend (e.g. "Successfully connected to qBittorrent").
pub async fn test_plugin_config(
    category: String,
    plugin_id: String,
    config: serde_json::Value,
) -> Result<String, Error> {
    crate::debug_log!("test_plugin_config({}, {})", category, plugin_id);
    let body = jumbie_shared::types::TestPluginPayload {
        category,
        plugin_type: plugin_id,
        config,
    };
    let result = post("plugins/test", &body).await;
    match &result {
        Ok(msg) => crate::debug_log!("test_plugin_config succeeded: {}", msg),
        Err(e) => crate::debug_error!("test_plugin_config failed: {}", e),
    }
    result
}

// Public Theme Endpoint (unauthenticated)

/// Fetches the public theme. This endpoint does NOT require authentication, so
/// the landing/login page can style itself before the user logs in.
pub async fn fetch_public_theme() -> Result<jumbie_shared::types::PublicThemeResponse, Error> {
    crate::debug_log!("fetch_public_theme()");
    let result = get("public/theme").await;
    match &result {
        Ok(_) => crate::debug_log!("fetch_public_theme succeeded"),
        Err(e) => crate::debug_error!("fetch_public_theme failed: {}", e),
    }
    result
}

// Automatic Profiles Endpoints

/// Returns all automatic profiles (submitter-defined scoring rules).
pub async fn fetch_automatic_profiles() -> Result<Vec<jumbie_shared::types::AutomaticProfile>, Error>
{
    crate::debug_log!("fetch_automatic_profiles()");
    let result: Result<Vec<jumbie_shared::types::AutomaticProfile>, Error> =
        get("automatic-profiles").await;
    match &result {
        Ok(p) => crate::debug_log!("fetch_automatic_profiles: got {} profiles", p.len()),
        Err(e) => crate::debug_error!("fetch_automatic_profiles failed: {}", e),
    }
    result
}

/// Deletes an automatic profile by submitter name.
pub async fn delete_automatic_profile(submitter: String) -> Result<(), Error> {
    crate::debug_log!("delete_automatic_profile({})", submitter);
    let result = delete_unit(&format!("automatic-profiles/{}", submitter)).await;
    match &result {
        Ok(_) => crate::debug_log!("delete_automatic_profile({}) succeeded", submitter),
        Err(e) => crate::debug_error!("delete_automatic_profile({}) failed: {}", submitter, e),
    }
    result
}

/// Returns the record (scoring event) history for a specific automatic profile.
pub async fn fetch_automatic_profile_records(
    submitter: String,
) -> Result<Vec<jumbie_shared::types::AutomaticProfileRecord>, Error> {
    crate::debug_log!("fetch_automatic_profile_records({})", submitter);
    let path = format!("automatic-profiles/{}/records", submitter);
    let result: Result<Vec<jumbie_shared::types::AutomaticProfileRecord>, Error> = get(&path).await;
    match &result {
        Ok(r) => crate::debug_log!(
            "fetch_automatic_profile_records({}): {} records",
            submitter,
            r.len()
        ),
        Err(e) => crate::debug_error!(
            "fetch_automatic_profile_records({}) failed: {}",
            submitter,
            e
        ),
    }
    result
}

/// Recalculate automatic scoring for all profiles.
pub async fn recalculate_automatic_scores() -> Result<(), Error> {
    crate::debug_log!("recalculate_automatic_scores()");
    let result = post_unit("automatic-profiles/recalculate", &()).await;
    match &result {
        Ok(_) => crate::debug_log!("recalculate_automatic_scores succeeded"),
        Err(e) => crate::debug_error!("recalculate_automatic_scores failed: {}", e),
    }
    result
}

// API Key Generation

/// The generate endpoints return the newly created secret once at creation time,
/// so the frontend must capture it from the response and display it.
/// Payload/response types are local to this module (see the note on the calendar
/// token types).
#[derive(serde::Serialize)]
pub struct GenerateApiKeyPayload {
    pub name: String,
    pub scopes: Vec<jumbie_shared::auth::ApiScope>,
    pub duration_days: Option<i64>,
}

#[derive(serde::Deserialize)]
pub struct GeneratedApiKeyResponse {
    pub id: String,
    pub key: String,
    pub prefix: String,
}

pub async fn generate_api_key(
    name: String,
    scopes: Vec<jumbie_shared::auth::ApiScope>,
    duration_days: Option<i64>,
) -> Result<GeneratedApiKeyResponse, Error> {
    crate::debug_log!("generate_api_key({}, {} scopes)", name, scopes.len());
    let body = GenerateApiKeyPayload {
        name,
        scopes,
        duration_days,
    };
    let result: Result<GeneratedApiKeyResponse, Error> =
        post("config/auth/api_keys/generate", &body).await;
    match &result {
        Ok(key) => {
            crate::debug_log!("generate_api_key: created key with prefix {}", key.prefix)
        }
        Err(e) => crate::debug_error!("generate_api_key failed: {}", e),
    }
    result
}
