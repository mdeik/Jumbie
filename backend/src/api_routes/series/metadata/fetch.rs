use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, Query, State},
};
use jumbie_shared::formatting::{
    LabelStyle, fmt_absolute_episode_id, fmt_episode_id_num, fmt_season_episode,
};
use serde::Deserialize;

use crate::api::AppState;
use crate::error::{AppError, IntoApiResponse};
use crate::models::activity::{ActivityEvent, ActivityType};

#[derive(Debug, Deserialize)]
pub struct FetchSeriesInfoPayload {
    /// Whether to update the series title with the canonical name from the provider.
    #[serde(default)]
    pub update_title: bool,
    /// Whether to merge provider aliases into the series alias list.
    #[serde(default)]
    pub merge_aliases: bool,
}

#[derive(Debug, Deserialize, Default)]
pub struct ProviderQuery {
    /// Optional provider instance id. When set, the action targets that specific
    /// provider instance (its config — language, credentials — is used). When
    /// absent, the highest-priority enabled provider mapped to the series is used.
    #[serde(default)]
    pub provider_instance_id: Option<String>,
}

/// Shared helper: fetch episodes from the active metadata plugin for a specific series and upsert them into the DB.
/// Updates the per-provider `metadata_last_synced_at` in the config on success.
/// Returns the ISO-8601 timestamp of the sync, or an error string.
pub async fn fetch_metadata_for_series(
    state: &Arc<AppState>,
    series_id: &str,
    metadata_id_override: Option<String>,
) -> Result<String, AppError> {
    // Resolve the provider instance + metadata ID for this series. The instance
    // decides which provider config (language, credentials) is used, so this is
    // never an arbitrary `HashMap` pick.
    let (instance_id, metadata_id, series_title, absolute_numbering) = {
        let mapping = state
            .db
            .get_series_mapping(series_id)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e.to_string())))?
            .ok_or_else(|| AppError::NotFound(format!("Series {} not found", series_id)))?;

        let absolute_numbering = state.effective_absolute_numbering(&mapping).await;
        let required = Some(if absolute_numbering {
            jumbie_shared::plugin::Capability::MetadataProviderAbsolute
        } else {
            jumbie_shared::plugin::Capability::MetadataProviderNormal
        });

        let (instance_id, metadata_id) = match metadata_id_override {
            Some(override_id) => {
                // The override is a metadata ID; find the instance that owns it so
                // the fetch uses that provider instance's config.
                match mapping
                    .settings
                    .metadata_ids
                    .iter()
                    .find(|(_, v)| **v == override_id)
                    .map(|(k, _)| k.clone())
                {
                    Some(iid) => (iid, override_id),
                    None => {
                        // Unknown ID (e.g. first sync) — resolve the provider normally.
                        let p =
                            resolve_provider_for_series(state, &mapping, None, required).await?;
                        (p.instance_id, override_id)
                    }
                }
            }
            None => {
                let p = resolve_provider_for_series(state, &mapping, None, required).await?;
                (p.instance_id, p.metadata_id)
            }
        };

        (
            instance_id,
            metadata_id,
            mapping.target_title.clone(),
            absolute_numbering,
        )
    };

    let plugin = state
        .plugin_manager
        .read()
        .await
        .get_plugin(&instance_id)
        .ok_or_else(|| {
            AppError::BadRequest(format!(
                "Metadata provider '{instance_id}' is not available"
            ))
        })?;

    // `instance_id` identifies the instance and keys the `metadata_ids` map, the
    // episode `metadata_source`, and `metadata_last_synced_at`. `plugin_id` is the
    // backend TYPE id (e.g. "jumbie.tvdb") that keys the cache.
    let instance_id = plugin.instance_id().to_string();
    let plugin_id = crate::utils::metadata::instance_plugin_id(&state.db, &instance_id).await;
    let plugin_id = &plugin_id;
    let instance_id = &instance_id;
    let ordering_mode = if absolute_numbering {
        "absolute"
    } else {
        "normal"
    };

    // Some providers have their own absolute numbering (e.g. TheTVDB absolute
    // episode IDs) while others return normal season/episode numbers; the plugin
    // reports which format it uses so episode_id keys are constructed correctly.
    let plugin_uses_absolute_ids = if absolute_numbering {
        plugin
            .call(
                "uses_absolute_episode_numbering",
                Some(serde_json::json!({
                    "absolute_numbering": true
                })),
            )
            .await
            .ok()
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    } else {
        false
    };

    // Marked attempted BEFORE the fetch so a down/429 provider is not hammered on
    // every mode switch or refresh; retrying requires an explicit user re-fetch.
    // Marking after success would retry forever on a dead provider.
    let _ = state
        .db
        .mark_fetch_attempted(&metadata_id, plugin_id, instance_id, ordering_mode)
        .await;

    let metadata = crate::plugins::bridge::metadata::fetch_episodes_and_seasons(
        plugin.as_ref(),
        &metadata_id,
        absolute_numbering,
    )
    .await
    .map_err(|e| {
        let msg = e.to_string();
        if msg.contains("429") {
            AppError::TooManyRequests(msg)
        } else {
            AppError::Internal(anyhow::anyhow!(msg))
        }
    })?;

    let mut inserts = Vec::new();
    for ep in &metadata.episodes {
        let episode_id = if plugin_uses_absolute_ids {
            fmt_absolute_episode_id(ep.episode, series_id)
        } else {
            fmt_episode_id_num(ep.season, ep.episode, series_id)
        };
        inserts.push((episode_id, ep.clone()));
    }

    // A valid 200 OK with no episodes usually means the metadata ID is invalid
    // (typo or nonexistent on this provider) — bail with a clear message rather
    // than silently succeeding with nothing.
    if metadata.episodes.is_empty() {
        let provider_name = &plugin.plugin_info().display_name;
        return Err(AppError::BadRequest(format!(
            "No episodes found for metadata ID '{}' on {}. Check that the ID is correct.",
            metadata_id, provider_name,
        )));
    }

    tracing::debug!(
        "Upserting {} metadata episodes for series {} (provider={}, mode={}, absolute_ids={})",
        inserts.len(),
        series_title,
        plugin_id,
        ordering_mode,
        plugin_uses_absolute_ids
    );

    state
        .db
        .merge_metadata_episodes(
            instance_id,
            series_id,
            inserts,
            plugin_uses_absolute_ids as i32,
        )
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e.to_string())))?;

    // Re-estimate release dates after metadata fetch — new episode dates
    // from metadata may fill gaps that improve estimation accuracy.
    if let Err(e) = crate::release_estimator::run_release_date_estimation(
        &state.db,
        series_id,
        absolute_numbering,
    )
    .await
    {
        tracing::warn!(
            "Failed to run release date estimator after metadata fetch for {} ({}): {}",
            series_title,
            series_id,
            e
        );
    }

    // Per-provider episode cache — enables "restore season metadata" without a
    // re-fetch from the API.
    if let Err(e) = state
        .db
        .batch_upsert_metadata_episodes_cache(
            &metadata_id,
            plugin_id,
            instance_id,
            ordering_mode,
            &metadata
                .episodes
                .iter()
                .map(|ep| crate::db::metadata_cache::EpisodeMetadataForCache {
                    season_number: ep.season,
                    episode_number: ep.episode,
                    unique_id: ep.unique_id.clone(),
                    title: ep.title.clone(),
                    description: ep.description.clone(),
                    runtime: ep.runtime,
                    image_url: ep.image_url.clone(),
                    meta_date: ep.meta_date,
                })
                .collect::<Vec<_>>(),
        )
        .await
    {
        tracing::warn!(
            "Failed to persist episode cache for series {} (provider={}): {}",
            series_title,
            plugin_id,
            e
        );
    }

    // Lazily populate series-level metadata cache: when there is no entry for this
    // (metadata_id, provider) it is fetched here, so the series poster is available
    // for notifier embeds on the very first episode sync — no background scheduler,
    // uniform across all metadata plugins.
    let cache_missing = state
        .db
        .get_metadata_series_cache(&metadata_id, plugin_id, instance_id)
        .await
        .ok()
        .flatten()
        .is_none();
    if cache_missing {
        match crate::plugins::bridge::metadata::fetch_series_info(plugin.as_ref(), &metadata_id)
            .await
            .map_err(|e| anyhow::anyhow!(e))
        {
            Ok(info) => {
                if let Err(e) = state
                    .db
                    .merge_metadata_series_cache(
                        crate::db::metadata_cache::MergeMetadataCacheParams {
                            metadata_id: &metadata_id,
                            plugin_id,
                            instance_id,
                            title: &series_title,
                            overview: info.overview.as_deref(),
                            language: None,
                            aliases_json: None,
                            image_url: info.image_url.as_deref(),
                        },
                    )
                    .await
                {
                    tracing::warn!(
                        "Failed to persist series metadata cache after lazy fetch: {}",
                        e
                    );
                }
            }
            Err(e) => {
                tracing::warn!(
                    "Lazy series info fetch failed for {} ({}): {}",
                    series_title,
                    metadata_id,
                    e
                );
                // Write a tombstone row so we don't retry on every sync.
                let _ = state
                    .db
                    .merge_metadata_series_cache(
                        crate::db::metadata_cache::MergeMetadataCacheParams {
                            metadata_id: &metadata_id,
                            plugin_id,
                            instance_id,
                            title: &series_title,
                            overview: None,
                            language: None,
                            aliases_json: None,
                            image_url: None,
                        },
                    )
                    .await;
            }
        }
    }

    // Season counts are isolated by provider + ordering mode. Storing
    // (provider, mode, season) → count answers "how many episodes should season 2
    // have?" without re-parsing all metadata rows, which `fill_missing_episodes`
    // needs. Counts differ per mode, so they cannot live in the series config.
    state
        .db
        .upsert_metadata_season_cache(
            &metadata_id,
            plugin_id,
            instance_id,
            ordering_mode,
            &metadata.seasons,
        )
        .await
        .map_err(|e| {
            AppError::Internal(anyhow::anyhow!(format!(
                "Failed to save metadata seasons: {}",
                e
            )))
        })?;

    // Update the per-provider last_synced timestamp so the background refresh
    // loop knows this series was recently synced for this provider.
    // Canonical DB format (naive UTC); converted to RFC 3339 at the API boundary.
    let synced_at = crate::datetime::UtcDateTime::now().to_db_string();
    if let Ok(Some(mut mapping)) = state.db.get_series_mapping(series_id).await {
        mapping
            .settings
            .metadata_last_synced_at
            .insert(instance_id.to_string(), synced_at.clone());
        if let Err(e) = state.db.upsert_series_mapping(series_id, &mapping).await {
            tracing::error!(
                "Failed to persist mapping after metadata sync for {} ({}): {}",
                series_title,
                series_id,
                e
            );
        }

        // Refresh monitor status — newly populated meta_date / upload_date
        // may now give the episode an effective date, which affects `Future` mode.
        crate::source_processor::reapply_monitor_for_series(&state.db, series_id, false, {
            let c = state.cfg.read().await;
            c.general.absolute_numbering
        })
        .await;
    }

    // Record one activity event per season with the consolidated episode range;
    // multiple fetches within 5 minutes for the same series are merged by
    // `merge_activity_rows`.
    {
        use std::collections::BTreeMap;
        let mut by_season: BTreeMap<i32, Vec<i32>> = BTreeMap::new();
        for ep in &metadata.episodes {
            by_season.entry(ep.season).or_default().push(ep.episode);
        }
        for (season, mut episodes) in by_season {
            episodes.sort();
            let first = episodes[0];
            let last = *episodes.last().unwrap_or(&first);
            let ep_label = fmt_season_episode(season, first, Some(last), LabelStyle::Short);
            let _ = state
                .db
                .record_activity(ActivityEvent {
                    event_type: ActivityType::Metadata,
                    series_title: series_title.clone(),
                    season: Some(season.to_string()),
                    episode: Some(first),
                    episode_end: if last > first { Some(last) } else { None },
                    title: None,
                    details: Some(ep_label),
                    status: "Success".to_string(),
                })
                .await;
        }
    }

    tracing::debug!(
        "Metadata sync complete for series {} at {} (provider={}, mode={})",
        series_title,
        synced_at,
        plugin_id,
        ordering_mode
    );
    Ok(synced_at)
}

/// Guard: the resolved provider must actually support the requested operation.
///
/// The UI hides the "Fetch Title"/"Fetch Aliases" buttons when the capability
/// is absent, but this is the authoritative server-side check — a direct API
/// call must never reach a provider that cannot perform the action. Uses
/// effective capabilities, so a per-instance toggle (if one exists) is honored.
async fn ensure_metadata_capability(
    state: &Arc<AppState>,
    plugin: &Arc<dyn crate::plugins::PluginInstance>,
    capability: jumbie_shared::plugin::Capability,
    action: &str,
) -> Result<(), AppError> {
    let supported = state
        .plugin_manager
        .read()
        .await
        .effective_capabilities_for(plugin)
        .contains(&capability);
    if supported {
        Ok(())
    } else {
        Err(AppError::BadRequest(format!(
            "The {} provider does not support {}",
            plugin.instance_id(),
            action
        )))
    }
}

/// Resolve the metadata provider instance + ID for a series.
///
/// SSoT for provider selection:
/// - `forced` (a provider instance id, e.g. from a per-provider Fetch button)
///   targets that instance, whose config (language, credentials) is used.
/// - `required` narrows to providers supporting that capability (e.g. the
///   active numbering mode); `None` accepts any enabled metadata provider.
/// - Selection is priority-ordered and only ENABLED providers that declare the
///   capability and have a non-empty ID for the series are eligible — never an
///   arbitrary `HashMap` value.
///
/// Returns the [`ResolvedMetadataProvider`] (instance + type id + external id).
pub(crate) async fn resolve_provider_for_series(
    state: &Arc<AppState>,
    mapping: &jumbie_shared::mapping::MappingRule,
    forced: Option<&str>,
    required: Option<jumbie_shared::plugin::Capability>,
) -> Result<crate::utils::metadata::ResolvedMetadataProvider, AppError> {
    use crate::utils::metadata::{
        ResolvedMetadataProvider, instance_plugin_id_map, ordered_series_providers,
    };

    let instance_plugins = instance_plugin_id_map(&state.db).await;

    if let Some(pid) = forced {
        let metadata_id = mapping
            .settings
            .metadata_ids
            .get(pid)
            .filter(|s| !s.is_empty())
            .cloned()
            .ok_or_else(|| {
                AppError::BadRequest(format!("No metadata ID set for provider '{pid}'"))
            })?;
        if state.plugin_manager.read().await.get_plugin(pid).is_none() {
            return Err(AppError::BadRequest(format!(
                "Metadata provider '{pid}' is not available"
            )));
        }
        return Ok(ResolvedMetadataProvider {
            instance_id: pid.to_string(),
            plugin_id: instance_plugins
                .get(pid)
                .cloned()
                .unwrap_or_else(|| pid.to_string()),
            metadata_id,
        });
    }

    let pm = state.plugin_manager.read().await;
    // `get_plugins_by_capability`/`ordered_metadata_providers` are already
    // priority-ordered (SSoT), so the first provider with an ID wins.
    // Preferred: providers supporting the required capability, priority-ordered.
    if let Some(cap) = required
        && let Some(p) = ordered_series_providers(
            mapping,
            &pm.get_plugins_by_capability(cap),
            &instance_plugins,
        )
        .into_iter()
        .next()
    {
        return Ok(p);
    }
    // Fallback / mode-agnostic: any enabled metadata provider with an ID.
    if let Some(p) =
        ordered_series_providers(mapping, &pm.ordered_metadata_providers(), &instance_plugins)
            .into_iter()
            .next()
    {
        return Ok(p);
    }
    Err(AppError::BadRequest(
        "No metadata ID set for this series".to_string(),
    ))
}

/// Load the metadata ID and the resolved metadata plugin for a series.
async fn resolve_metadata_plugin(
    state: &Arc<AppState>,
    id: &str,
    forced: Option<&str>,
) -> Result<(String, Arc<dyn crate::plugins::PluginInstance>), AppError> {
    let mapping = state
        .db
        .get_series_mapping(id)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e.to_string())))?
        .ok_or_else(|| AppError::NotFound(format!("Series {} not found", id)))?;
    let resolved = resolve_provider_for_series(state, &mapping, forced, None).await?;
    let plugin = state
        .plugin_manager
        .read()
        .await
        .get_plugin(&resolved.instance_id)
        .ok_or_else(|| {
            AppError::BadRequest(format!(
                "Metadata provider '{}' is not available",
                resolved.instance_id
            ))
        })?;
    Ok((resolved.metadata_id, plugin))
}

/// HTTP handler: `POST /api/series/:id/fetch_series_info`
/// Fetches the series canonical title from the active metadata provider and updates
/// the series title in the mapping.  Opt-in: only runs when the payload has
/// `update_title: true`.  If `merge_aliases: true`, the fetched aliases are merged
/// into the existing series alias list (deduplicated).
///
/// # DB-first optimization
/// Series titles and aliases rarely change after initial metadata sync. If the
/// series has been synced before (`last_synced_at` is set), this handler skips
/// the API call and returns the existing data from the mapping.  The user can
/// still force a fresh fetch by re-syncing metadata episodes first.
pub async fn fetch_series_info(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(provider): Query<ProviderQuery>,
    Json(payload): Json<FetchSeriesInfoPayload>,
) -> Result<Json<serde_json::Value>, AppError> {
    tracing::debug!("fetch_series_info called: series_id={}", id);

    let (metadata_id, plugin) =
        resolve_metadata_plugin(&state, &id, provider.provider_instance_id.as_deref()).await?;
    ensure_metadata_capability(
        &state,
        &plugin,
        jumbie_shared::plugin::Capability::FetchSeriesTitle,
        "fetching series info",
    )
    .await?;

    // Cache key is (metadata_id, plugin_id, instance_id): `plugin_id` is the
    // backend TYPE id (e.g. "jumbie.tvdb"), `instance_id` the instance.
    let instance_id = plugin.instance_id().to_string();
    let plugin_id = crate::utils::metadata::instance_plugin_id(&state.db, &instance_id).await;
    let plugin_id = &plugin_id;
    let instance_id = &instance_id;

    // The DB cache is intentionally skipped: the episodes endpoint populated it with
    // the untranslated original-language name, but this user-initiated "Fetch Title"
    // handler wants fresh, correctly-localised data from the translation endpoint.
    let info = crate::plugins::bridge::metadata::fetch_series_info(plugin.as_ref(), &metadata_id)
        .await
        .map_err(|e| {
            let msg = e.to_string();
            if msg.contains("not implemented") || msg.contains("MethodNotSupported") {
                AppError::BadRequest(format!(
                    "The {} provider does not support fetching series info",
                    plugin.instance_id()
                ))
            } else if msg.contains("429") {
                AppError::TooManyRequests(msg)
            } else {
                AppError::Internal(anyhow::anyhow!(msg))
            }
        })?;

    let fetched_name = info.name.clone();
    let fetched_overview = info.overview.clone();

    if let Ok(Some(mut mapping)) = state.db.get_series_mapping(&id).await {
        if payload.update_title {
            mapping.target_title = info.name;
            tracing::info!(
                "Updated series {} title to '{}' from provider",
                id,
                mapping.target_title
            );
        }

        if payload.merge_aliases {
            // Aliases override the series title for search (not an addition): when
            // aliases are set, the title is no longer a search term. To keep the
            // series findable by its canonical name, that title is injected as the
            // first alias, followed by provider aliases.
            let title = fetched_name.clone();
            let new_aliases: Vec<String> = info.aliases.values().flat_map(|v| v.clone()).collect();

            let mut seen = std::collections::HashSet::new();
            let mut merged: Vec<String> = Vec::new();

            merged.push(title.clone());
            seen.insert(title);

            for alias in new_aliases {
                if seen.insert(alias.clone()) {
                    merged.push(alias);
                }
            }

            for alias in &mapping.settings.aliases {
                if !alias.is_empty() && seen.insert(alias.clone()) {
                    merged.push(alias.clone());
                }
            }

            mapping.settings.aliases = merged;
            tracing::info!(
                "Merged series aliases for {} ({}) from provider",
                mapping.target_title,
                id
            );
        }

        if let Err(e) = state.db.upsert_series_mapping(&id, &mapping).await {
            tracing::error!("Failed to persist mapping after fetch_series_info: {}", e);
        }
    }

    // Write to cache via merge so subsequent fast-path lookups find fresh data.
    // Using merge because the series-info endpoint may not include aliases — we
    // don't want to wipe aliases that were cached separately by
    // `fetch_series_aliases`. Aliases are only written when the provider
    // actually returned them.
    let merged_aliases: Vec<String> = info.aliases.values().flatten().cloned().collect();
    let aliases_json = if merged_aliases.is_empty() {
        None
    } else {
        Some(serde_json::to_string(&merged_aliases).unwrap_or_else(|_| "[]".to_string()))
    };
    if let Err(e) = state
        .db
        .merge_metadata_series_cache(crate::db::metadata_cache::MergeMetadataCacheParams {
            metadata_id: &metadata_id,
            plugin_id,
            instance_id,
            title: &fetched_name,
            overview: fetched_overview.as_deref(),
            language: None, // don't touch language
            aliases_json: aliases_json.as_deref(),
            image_url: info.image_url.as_deref(),
        })
        .await
    {
        tracing::warn!("Failed to cache series info for {}: {}", id, e);
    }

    Ok(Json(serde_json::json!({
        "name": fetched_name,
        "overview": fetched_overview,
        "cached": false,
    })))
}

/// HTTP handler: `POST /api/series/:id/fetch_series_aliases`
/// Fetches series aliases from the active metadata provider and merges them
/// into the series settings (deduplicated).
///
/// # DB-first optimization
/// Aliases rarely change after initial metadata sync. If the series has been
/// synced before (`last_synced_at` is set) AND already has aliases stored,
/// this skips the API call and returns the existing aliases from the mapping.
pub async fn fetch_series_aliases(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(provider): Query<ProviderQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    tracing::debug!("fetch_series_aliases called: series_id={}", id);

    let (metadata_id, plugin) =
        resolve_metadata_plugin(&state, &id, provider.provider_instance_id.as_deref()).await?;
    ensure_metadata_capability(
        &state,
        &plugin,
        jumbie_shared::plugin::Capability::FetchSeriesAliases,
        "fetching aliases",
    )
    .await?;

    // Cache key is (metadata_id, plugin_id, instance_id): `plugin_id` is the
    // backend TYPE id (e.g. "jumbie.tvdb"), `instance_id` the instance.
    let instance_id = plugin.instance_id().to_string();
    let plugin_id = crate::utils::metadata::instance_plugin_id(&state.db, &instance_id).await;
    let plugin_id = &plugin_id;
    let instance_id = &instance_id;

    // Resolve aliases: prefer cache, fall back to the provider API.
    let (new_aliases, from_cache) = match state
        .db
        .get_metadata_series_cache(&metadata_id, plugin_id, instance_id)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e.to_string())))?
    {
        Some(cached) if !cached.aliases.is_empty() => {
            tracing::debug!(
                "fetch_series_aliases: {} aliases cached for series {} ({}) (provider={}), skipping API",
                cached.aliases.len(),
                cached.title,
                id,
                plugin_id
            );
            (cached.aliases, true)
        }
        _ => {
            let aliases = crate::plugins::bridge::metadata::fetch_series_aliases(
                plugin.as_ref(),
                &metadata_id,
            )
            .await
            .map_err(|e| {
                let msg = e.to_string();
                if msg.contains("not implemented") || msg.contains("MethodNotSupported") {
                    AppError::BadRequest(format!(
                        "The {} provider does not support fetching aliases",
                        plugin.instance_id()
                    ))
                } else if msg.contains("429") {
                    AppError::TooManyRequests(msg)
                } else {
                    AppError::Internal(anyhow::anyhow!(msg))
                }
            })?;
            (aliases, false)
        }
    };

    // Resolve the provider's canonical title (cache first, then fetch_series_info).
    // It is injected as the first alias so the series stays findable by its real
    // name when aliases override the title as a search term.
    let provider_title = match state
        .db
        .get_metadata_series_cache(&metadata_id, plugin_id, instance_id)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e.to_string())))?
    {
        Some(cached) if !cached.title.is_empty() => Some(cached.title),
        _ => None,
    };

    let provider_title = match provider_title {
        Some(t) => t,
        None => {
            // Providers that support aliases (TVDB, TVMaze) also support series info.
            match crate::plugins::bridge::metadata::fetch_series_info(plugin.as_ref(), &metadata_id)
                .await
            {
                Ok(info) => info.name,
                Err(e) => {
                    tracing::warn!("fetch_series_aliases: failed to get provider title: {}", e);
                    String::new()
                }
            }
        }
    };

    // Merge with existing aliases
    let mut merged: Vec<String> = Vec::new();
    if let Ok(Some(mut mapping)) = state.db.get_series_mapping(&id).await {
        merged = merge_series_aliases(&mapping.settings.aliases, &new_aliases, &provider_title);
        mapping.settings.aliases = merged.clone();

        if let Err(e) = state.db.upsert_series_mapping(&id, &mapping).await {
            tracing::error!(
                "Failed to persist mapping after fetch_series_aliases: {}",
                e
            );
        }

        // Invalidate rename plan cache — aliases changed

        // Aliases are merged into the cache (not upserted) so the overview and
        // language fields keep their existing values. The title is always overwritten
        // by the merge SQL (not COALESCE-protected), so we pass the provider's
        // canonical title — using `mapping.target_title` (possibly user-custom) would
        // pollute the cache and later inject the wrong title as an alias. When the
        // provider title is empty (unresolvable) the write is skipped entirely.
        if !provider_title.is_empty() {
            let aliases_json = serde_json::to_string(&merged).unwrap_or_else(|_| "[]".to_string());
            if let Err(e) = state
                .db
                .merge_metadata_series_cache(crate::db::metadata_cache::MergeMetadataCacheParams {
                    metadata_id: &metadata_id,
                    plugin_id,
                    instance_id,
                    title: &provider_title,
                    overview: None, // don't touch overview
                    language: None, // don't touch language
                    aliases_json: Some(&aliases_json),
                    image_url: None, // don't touch image_url
                })
                .await
            {
                tracing::warn!(
                    "Failed to cache series aliases for {} ({}): {}",
                    mapping.target_title,
                    id,
                    e
                );
            }
        }
    }

    let response_title = if !provider_title.is_empty() && !merged.is_empty() {
        provider_title.to_string()
    } else {
        String::new()
    };

    tracing::info!(
        "fetch_series_aliases: {} aliases for series {} ({}) (cached={})",
        merged.len(),
        provider_title,
        id,
        from_cache
    );

    Ok(Json(serde_json::json!({
        "title": response_title,
        "aliases": merged,
        "cached": from_cache,
    })))
}

// ---------------------------------------------------------------------------
// Pure helpers for alias merge and response construction
// (extracted for testability)
// ---------------------------------------------------------------------------

/// Merge existing DB aliases with new provider aliases, deduplicating.
/// Order: existing_aliases → provider_title (if not already present) → provider_aliases.
/// The provider title is only injected when there are other aliases (existing or new).
pub fn merge_series_aliases(
    existing_db_aliases: &[String],
    new_aliases: &[String],
    provider_title: &str,
) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut merged: Vec<String> = Vec::new();

    // 1. Existing aliases first, in order, skipping empty strings (legacy data).
    for alias in existing_db_aliases {
        if !alias.is_empty() && seen.insert(alias.clone()) {
            merged.push(alias.clone());
        }
    }

    // 2. Provider title injected after existing aliases, if non-empty, not already
    //    present in existing, and there will be at least one alias total (existing
    //    or new) — avoids polluting the alias list with a lone title.
    if !provider_title.is_empty()
        && (!merged.is_empty() || !new_aliases.is_empty())
        && seen.insert(provider_title.to_string())
    {
        merged.push(provider_title.to_string());
    }

    // 3. New provider aliases, filtering out any already seen (from existing or title).
    for alias in new_aliases {
        if seen.insert(alias.clone()) {
            merged.push(alias.clone());
        }
    }

    merged
}

/// `POST /api/series/:id/fetch_metadata`
///
/// Fetch episode data from metadata server-side and upsert it into the DB.
/// Accepts an optional `metadata_id` body field to avoid races with unsaved form data.
///
/// SSoT: the single path for fetching metadata through the metadata queue and
/// notifying dependent caches. Used by both the edit-page sync endpoint and
/// `create_series` (add-series auto-sync + search-on-add), so the fetch path and
/// rename-queue wake-up live in one place.
///
/// The rename-queue wake-up fires regardless of fetch outcome, which matters for
/// add-series where a failed fetch may still have partially mutated episode state.
pub async fn sync_series_metadata(
    state: &Arc<AppState>,
    series_id: &str,
    metadata_id_override: Option<String>,
) -> Result<String, AppError> {
    let state_clone = state.clone();
    let id_for_closure = series_id.to_string();
    let mid = metadata_id_override.clone();
    let result = state
        .metadata_queue
        .submit_and_wait(series_id.to_string(), move || {
            let state = state_clone.clone();
            let sid = id_for_closure.clone();
            let override_id = mid.clone();
            async move { fetch_metadata_for_series(&state, &sid, override_id).await }
        })
        .await;
    if let Ok(synced_at) = &result {
        tracing::debug!(
            "sync_series_metadata completed for series {}: synced at {}",
            series_id,
            synced_at
        )
    }
    // Episode titles may have updated — wake auto-apply to re-evaluate rename plans.
    state.rename_queue_trigger.notify_one();
    result
}

/// `POST /api/series/{id}/fetch_metadata`
///
/// Trigger a metadata fetch for a series.  Optionally overrides the metadata
/// ID (the value stored on the series mapping is used when absent).
pub async fn fetch_metadata(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    body: Option<Json<serde_json::Value>>,
) -> Result<Json<jumbie_shared::types::MetadataSyncStatus>, AppError> {
    tracing::debug!("fetch_metadata called: series_id={}", id);
    let mut metadata_id_override = None;
    if let Some(Json(ref payload)) = body
        && let Some(mid) = payload.get("metadata_id").and_then(|v| v.as_str())
    {
        metadata_id_override = Some(mid.to_string());
    }

    sync_series_metadata(&state, &id, metadata_id_override)
        .await
        .map(|synced_at| jumbie_shared::types::MetadataSyncStatus {
            // `synced_at` is the DB-canonical (naive) string; emit RFC 3339 on the
            // wire like every other timestamp field.
            synced_at: Some(crate::datetime::naive_utc_str_to_rfc3339(&synced_at)),
        })
        .into_json_response()
}
