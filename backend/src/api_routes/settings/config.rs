use std::collections::HashSet;
use std::sync::Arc;

use axum::{Json, extract::State, http::StatusCode};
use jumbie_shared::{
    config::{Config, Theme, UIConfig},
    types::{BootstrapData, PublicThemeResponse, UpdateConfigPayload},
};
use tokio_util::sync::CancellationToken;

use crate::api::AppState;
use crate::error::{AppError, IntoApiResponse};

/// SSoT: strips secrets from config before it is sent to the client. Every
/// config-response path must call this — add new secret fields here and only here.
/// Kept as a pure function so it is testable without a running server and no
/// handler can skip masking.
fn sanitize_config_for_response(mut config: Config) -> Config {
    // Sentinel value if a password is set, None if auth is disabled.
    config.auth.password = config
        .auth
        .password
        .filter(|p| !p.is_empty())
        .map(|_| "********".to_string());

    // API keys: mask the raw key value, keep metadata for display
    for key in &mut config.auth.api_keys {
        key.key = "********".to_string();
    }

    config
}

pub async fn update_config(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<UpdateConfigPayload>,
) -> Result<StatusCode, AppError> {
    tracing::info!("Received request to update global config");

    // Only sections present in the payload are validated. config.toml fields
    // (database, plugins_dir, etc.) are startup-only and edited in the file.

    if let Some(ref org) = payload.organization {
        tracing::debug!("Validating organization paths...");

        for root in &org.destination_roots {
            crate::validation::validate_path(&root.path.to_string_lossy(), &root.path).map_err(
                |e| {
                    tracing::error!("Invalid destination root '{}': {}", root.path.display(), e);
                    AppError::BadRequest(format!(
                        "Invalid destination root '{}': {}",
                        root.path.display(),
                        e
                    ))
                },
            )?;
        }
        let mut seen_canonical = HashSet::new();
        for root in &org.destination_roots {
            let canonical = crate::validation::normalize_path(&root.path);
            if !seen_canonical.insert(canonical) {
                tracing::error!(
                    "Duplicate destination root detected: '{}' resolves to the same location as another root",
                    root.path.display()
                );
                return Err(AppError::BadRequest(format!(
                    "Duplicate destination root detected: '{}' resolves to the same location as another root",
                    root.path.display()
                )));
            }
        }

        // Search-key templates share the season/episode restriction with the
        // per-series and per-season templates (SSoT: `validate_search_template`),
        // so an API write can't slip in an unusable template.
        crate::validation::validate_search_template(&org.search_format, true)
            .map_err(|e| AppError::BadRequest(format!("Invalid search_format: {}", e)))?;
        crate::validation::validate_search_template(&org.search_format_absolute, true)
            .map_err(|e| AppError::BadRequest(format!("Invalid search_format_absolute: {}", e)))?;
    }

    if let Some(ref proxy) = payload.proxy {
        tracing::debug!("Validating proxy configuration...");
        if proxy.enabled {
            if !proxy.http.is_empty() {
                crate::validation::validate_url(&proxy.http).map_err(|e| {
                    tracing::error!(
                        "Failed to update global config: invalid HTTP proxy -> {}",
                        e
                    );
                    AppError::BadRequest(format!("Invalid HTTP proxy URL: {}", e))
                })?;
            }
            if !proxy.https.is_empty() {
                crate::validation::validate_url(&proxy.https).map_err(|e| {
                    tracing::error!(
                        "Failed to update global config: invalid HTTPS proxy -> {}",
                        e
                    );
                    AppError::BadRequest(format!("Invalid HTTPS proxy URL: {}", e))
                })?;
            }
        }
    }

    if let Some(ref general_cfg) = payload.general {
        general_cfg.automatic_profiles.validate().map_err(|e| {
            tracing::error!("Invalid automatic profiles configuration: {}", e);
            AppError::BadRequest(e)
        })?;

        // Shared validators (SSoT for config field constraints)
        crate::validation::validate_media_info_scan_interval(general_cfg.media_info_scan_interval)
            .map_err(|e| AppError::BadRequest(e.0))?;
        crate::validation::validate_season_pack_replace_threshold(
            general_cfg.season_pack_replace_threshold,
        )
        .map_err(|e| AppError::BadRequest(e.0))?;
        crate::validation::validate_unexpected_files_handling(
            &general_cfg.unexpected_files_handling,
        )
        .map_err(|e| AppError::BadRequest(e.0))?;
        crate::validation::validate_unneeded_episodes_handling(
            &general_cfg.unneeded_episodes_handling,
        )
        .map_err(|e| AppError::BadRequest(e.0))?;

        crate::validation::validate_auto_search_wanted_interval(
            general_cfg.auto_search_wanted_interval,
        )
        .map_err(|e| AppError::BadRequest(e.0))?;
        crate::validation::validate_auto_search_wanted_min_wait(
            general_cfg.auto_search_wanted_min_wait,
        )
        .map_err(|e| AppError::BadRequest(e.0))?;
        crate::validation::validate_auto_search_wanted_max_age_days(
            general_cfg.auto_search_wanted_max_age_days,
        )
        .map_err(|e| AppError::BadRequest(e.0))?;
    }

    if let Some(ref auth) = payload.auth {
        for key in &auth.api_keys {
            crate::validation::validate_api_key_scopes(key.scopes.len())
                .map_err(|e| AppError::BadRequest(format!("API Key '{}': {}", key.name, e.0)))?;
            if let Some(expires_at) = key.expires_at.as_deref()
                && !expires_at.is_empty()
            {
                crate::validation::validate_rfc3339_timestamp(
                    expires_at,
                    &format!("API Key '{}' expires_at", key.name),
                )
                .map_err(|e| AppError::BadRequest(e.0))?;
            }
        }
        for entry in &auth.banned_ips {
            if !entry.banned_at.is_empty() {
                crate::validation::validate_rfc3339_timestamp(
                    &entry.banned_at,
                    &format!("Banned IP '{}' banned_at", entry.ip),
                )
                .map_err(|e| AppError::BadRequest(e.0))?;
            }
            if let Some(until) = entry.banned_until.as_deref()
                && !until.is_empty()
            {
                crate::validation::validate_rfc3339_timestamp(
                    until,
                    &format!("Banned IP '{}' banned_until", entry.ip),
                )
                .map_err(|e| AppError::BadRequest(e.0))?;
            }
        }
    }

    // Auth field processing (direct DB writes).
    if let Some(auth) = &payload.auth {
        // Password
        match &auth.password {
            Some(pwd) if pwd != "********" && !pwd.is_empty() => {
                let hash = crate::auth_utils::hash_password(pwd).map_err(|e| {
                    AppError::Internal(anyhow::anyhow!("Failed to hash password: {}", e))
                })?;
                state
                    .db
                    .set_user_password_hash("admin", &hash)
                    .await
                    .map_err(|e| {
                        AppError::Internal(anyhow::anyhow!(
                            "Failed to persist password hash: {}",
                            e
                        ))
                    })?;
            }
            Some(pwd) if pwd.is_empty() => {
                state
                    .db
                    .set_user_password_hash("admin", "")
                    .await
                    .map_err(|e| {
                        AppError::Internal(anyhow::anyhow!("Failed to clear password hash: {}", e))
                    })?;
            }
            None => {
                state
                    .db
                    .set_user_password_hash("admin", "")
                    .await
                    .map_err(|e| {
                        AppError::Internal(anyhow::anyhow!("Failed to clear password hash: {}", e))
                    })?;
            }
            _ => {}
        }

        // Any password change must drop the auth cache (password hash + verified
        // credentials) so the new credential takes effect immediately.
        state.auth_cache.invalidate().await;

        // API keys
        let current_keys = state.db.get_api_keys().await.unwrap_or_default();
        let current_ids: std::collections::HashSet<_> =
            current_keys.iter().map(|k| k.id.clone()).collect();
        let payload_ids: std::collections::HashSet<_> =
            auth.api_keys.iter().map(|k| k.id.clone()).collect();
        for id in current_ids.difference(&payload_ids) {
            let _ = state.db.delete_api_key(id).await;
        }
        for key in &auth.api_keys {
            if key.key != "********" {
                let new_hash = crate::auth_utils::hash_api_key(&key.key);
                let prefix = if key.key.len() > 11 {
                    format!("jb_{}", &key.key[3..11])
                } else {
                    key.prefix.clone()
                };
                let _ = state
                    .db
                    .insert_api_key(
                        &key.id,
                        &key.name,
                        &new_hash,
                        &prefix,
                        &key.scopes,
                        key.expires_at.as_deref(),
                    )
                    .await;
            } else if let Some(existing) = current_keys.iter().find(|k| k.id == key.id) {
                let _ = state
                    .db
                    .insert_api_key(
                        &key.id,
                        &key.name,
                        &existing.key,
                        &existing.prefix,
                        &key.scopes,
                        key.expires_at.as_deref(),
                    )
                    .await;
            }
        }

        // Any API-key change must drop cached lookups so edits/removals take
        // effect immediately.
        state.auth_cache.invalidate_api_keys().await;

        // Calendar tokens
        let current_cal_tokens = state.db.get_calendar_tokens().await.unwrap_or_default();
        let current_cal_ids: std::collections::HashSet<_> =
            current_cal_tokens.iter().map(|k| k.id.clone()).collect();
        let payload_cal_ids: std::collections::HashSet<_> =
            auth.calendar_tokens.iter().map(|k| k.id.clone()).collect();
        for id in current_cal_ids.difference(&payload_cal_ids) {
            let _ = state.db.delete_calendar_token(id).await;
        }
        for cal in &auth.calendar_tokens {
            let _ = state
                .db
                .insert_calendar_token(
                    &cal.id,
                    &cal.name,
                    &cal.token,
                    !cal.hide_unmonitored,
                    cal.show_as_all_day,
                )
                .await;
        }

        // Banned IPs — replace all
        {
            let current_banned = state.db.get_banned_ips().await.unwrap_or_default();
            for entry in &current_banned {
                let _ = state.db.delete_banned_ip(&entry.ip).await;
            }
            for entry in &auth.banned_ips {
                let _ = state.db.insert_banned_ip(entry).await;
            }
        }

        // Subnet whitelist
        let _ = state.db.save_subnet_whitelist(&auth.subnet_whitelist).await;
    }

    // Capture the pre-save global numbering default so a flip can be detected
    // below and "use global" series re-evaluated.
    let old_global_absolute = {
        let c = state.cfg.read().await;
        c.general.absolute_numbering
    };
    {
        let mut config = state.cfg.write().await;

        // Mask auth secrets before storing in memory so the in-memory Config
        // never carries plaintext credentials.
        if let Some(mut auth) = payload.auth.clone() {
            auth.password = auth.password.map(|p| {
                if !p.is_empty() && p != "********" {
                    "********".to_string()
                } else {
                    p
                }
            });
            for key in &mut auth.api_keys {
                if key.key != "********" {
                    key.key = "********".to_string();
                }
            }
            config.auth = auth;
        }
        if let Some(org) = payload.organization.clone() {
            config.organization = org;
        }
        if let Some(proxy) = payload.proxy.clone() {
            config.proxy = proxy;
        }
        if let Some(general_cfg) = payload.general.clone() {
            config.general = general_cfg;
        }
        if let Some(sources) = payload.sources.clone() {
            config.sources = sources;
        }
        if let Some(security) = payload.security.clone() {
            config.security = security;
        }
    }

    // Only changed sections are persisted. config.toml is NEVER written at
    // runtime — it is a startup-only file.
    state.cfg.persist_db().await.map_err(|e| {
        tracing::error!("Failed to persist config to database: {}", e);
        AppError::Internal(anyhow::anyhow!("Failed to persist config: {}", e))
    })?;

    // Rename plans are keyed on a fingerprint of their inputs, which includes the
    // `organization` and `general` sections, so a config save is picked up on the
    // next poll without any explicit cache invalidation.

    // Global numbering-mode flip → renumber "use global" series. A change to
    // general.absolute_numbering flips the effective mode of every series without
    // an explicit override; since episodes are stored per-mode, they must be
    // renumbered via the same mode-switch path a manual toggle uses, or queries
    // filter a mode with no rows and those series vanish. Runs in the background.
    if let Some(general_cfg) = payload.general.as_ref()
        && general_cfg.absolute_numbering != old_global_absolute
    {
        let new_global_absolute = general_cfg.absolute_numbering;
        let state_for_sweep = state.clone();
        tokio::spawn(async move {
            let mappings = state_for_sweep
                .db
                .get_all_series_mappings()
                .await
                .unwrap_or_default();
            for (id, mapping) in mappings {
                // Explicit overrides are unaffected by the global change.
                if mapping.settings.absolute_numbering.is_some() {
                    continue;
                }
                // Only renumber when episodes are actually stored in the old
                // mode — a series with no rows there has nothing to convert.
                let db_eps = state_for_sweep
                    .db
                    .get_series_episodes_details(&id, !new_global_absolute)
                    .await
                    .unwrap_or_default();
                if db_eps.is_empty() {
                    continue;
                }
                tracing::debug!(
                    "Global absolute_numbering changed: renumbering '{}' (use global)",
                    mapping.target_title
                );
                crate::api_routes::series::handle_mode_switch(
                    &state_for_sweep,
                    &id,
                    &mapping.target_title,
                    mapping.settings.path.as_deref(),
                    new_global_absolute,
                )
                .await;
                state_for_sweep.rename_plan_cache.invalidate(&id).await;
            }
        });
    }

    tracing::info!("Successfully updated global config");

    // Hot-reload plugins.
    let plugins_cfg = state.db.get_plugins_config().await.unwrap_or_default();
    let global_cfg = {
        let cfg = state.cfg.read().await;
        Arc::new(cfg.clone())
    };

    // Config saves queue a deferred recalculation (2s debounce): each new save
    // resets the timer so rapid edits don't trigger N recalculations, and manual
    // "Recalculate" cancels any pending debounce and runs instantly.
    if global_cfg.general.automatic_profiles.enabled {
        // Cancel any previously scheduled debounced recalculation
        if let Some(token) = state.recalc_cancel.lock().await.take() {
            token.cancel();
        }

        let cancel = CancellationToken::new();
        *state.recalc_cancel.lock().await = Some(cancel.clone());

        if let Some(organizer) = state.organizer.clone() {
            let debounce_ms = 2_000u64;
            tokio::spawn(async move {
                tokio::select! {
                    _ = cancel.cancelled() => {
                        // Cancelled — a newer save or manual recalc will handle it
                    }
                    _ = tokio::time::sleep(std::time::Duration::from_millis(debounce_ms)) => {
                        if let Err(e) = organizer.reapply_automatic_profiles_to_library().await {
                            tracing::error!("Recalculation failed: {}", e);
                        }
                    }
                }
            });
        }
    }

    // SSoT: single reconfigure path for ALL plugins — in-place reconfigure where
    // supported, fresh instances otherwise.
    let mut pm = state.plugin_manager.write().await;
    pm.apply_config(&plugins_cfg, global_cfg).await;

    Ok(StatusCode::OK)
}

pub async fn get_config(State(state): State<Arc<AppState>>) -> Json<Config> {
    tracing::debug!("get_config called");
    let mut config = state.cfg.read().await.clone();

    // Inject API keys from DB (SSoT: DB is the canonical store)
    if let Ok(keys) = state.db.get_api_keys().await {
        config.auth.api_keys = keys;
    }

    // Inject calendar tokens from DB
    if let Ok(cal_tokens) = state.db.get_calendar_tokens().await {
        config.auth.calendar_tokens = cal_tokens;
    }

    // Inject banned IPs from DB
    if let Ok(banned_ips) = state.db.get_banned_ips().await {
        config.auth.banned_ips = banned_ips;
    }

    // Inject subnet whitelist from DB
    if let Ok(subnet_whitelist) = state.db.get_subnet_whitelist().await {
        config.auth.subnet_whitelist = subnet_whitelist;
    }

    // Inject password signal (SSoT: DB password hash)
    if let Ok(Some(hash)) = state.db.get_user_password_hash("admin").await {
        config.auth.password = if hash.is_empty() {
            None
        } else {
            Some(String::new())
        };
    } else {
        config.auth.password = None;
    }

    // SSoT: strip secrets before sending — one call guarantees consistency.
    Json(sanitize_config_for_response(config))
}

pub async fn get_public_theme(State(state): State<Arc<AppState>>) -> Json<PublicThemeResponse> {
    tracing::debug!("get_public_theme called");
    // Try to load the theme from DB; fall back to "auto".
    let theme_str = match state.db.get_ui_preferences().await {
        Ok(ui_config) => match ui_config.theme {
            Theme::Auto => "auto",
            Theme::Light => "light",
            Theme::Dark => "dark",
        },
        Err(_) => "auto",
    };
    Json(PublicThemeResponse {
        theme: theme_str.to_string(),
    })
}

pub async fn get_ui_preferences_endpoint(
    State(state): State<Arc<AppState>>,
) -> Result<Json<UIConfig>, AppError> {
    tracing::debug!("get_ui_preferences called");
    state.db.get_ui_preferences().await.into_json_response()
}

pub async fn put_ui_preferences_endpoint(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<UIConfig>,
) -> Result<StatusCode, AppError> {
    tracing::debug!("put_ui_preferences called");
    state
        .db
        .save_ui_preferences(&payload)
        .await
        .into_status_response()?;

    // Debounced monitor-mode sweep: each save cancels the previous pending sweep
    // and starts a fresh 5-minute timer, so N rapid saves yield one sweep (only the
    // last timer reaches zero).
    {
        let cancel = tokio_util::sync::CancellationToken::new();
        {
            let mut guard = state.monitor_sweep_cancel.lock().await;
            if let Some(old) = guard.take() {
                old.cancel();
            }
            *guard = Some(cancel.clone());
        }
        let state_clone = state.clone();
        let shutdown_token = state_clone.shutdown_token.clone();
        tokio::spawn(async move {
            tokio::select! {
                _ = cancel.cancelled() => {
                    // Cancelled — a newer preference save reset the timer.
                }
                _ = shutdown_token.cancelled() => {
                    // App is shutting down — don't start a sweep.
                }
                _ = tokio::time::sleep(std::time::Duration::from_secs(300)) => {
                    crate::api_routes::series::sweep_monitor_status(&state_clone).await;
                }
            }
        });
    }

    Ok(StatusCode::OK)
}

pub async fn get_bootstrap(
    State(state): State<Arc<AppState>>,
) -> Result<Json<BootstrapData>, AppError> {
    tracing::debug!("get_bootstrap called");

    let config = get_config(State(state.clone())).await.0;
    let qualities = super::get_quality_definitions(State(state.clone()))
        .await?
        .0;
    let quality_profiles = super::get_quality_profiles(State(state.clone())).await?.0;
    let release_profiles = super::get_release_profiles(State(state.clone())).await?.0;
    let ui_preferences = get_ui_preferences_endpoint(State(state.clone())).await?.0;
    let plugins_cfg = super::get_plugins_config_endpoint(State(state.clone()))
        .await?
        .0;
    let plugins = super::get_plugins(State(state.clone())).await.0;
    let available_plugins = super::get_available_plugins().await.0;
    let plugin_status = super::get_plugin_status(State(state.clone())).await.0;

    Ok(Json(BootstrapData {
        config,
        qualities,
        quality_profiles,
        release_profiles,
        ui_preferences,
        plugins_cfg,
        plugins,
        available_plugins,
        plugin_status,
    }))
}
