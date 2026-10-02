use crate::api::AppState;
use crate::api::modifying_series::{try_lock_series, unlock_series};
use crate::error::{AppError, AppResultExt};
use crate::file_manager::move_series_directory;
use axum::{
    Json,
    extract::{Path, State},
};
use std::sync::Arc;

use super::helpers::get_series_mapping_or_404;

/// Returned from `update_series` so the frontend can show a mode-switch toast.
#[derive(Debug, serde::Serialize)]
pub struct UpdateSeriesResponse {
    #[serde(default)]
    pub mode_switch_warning: bool,
    #[serde(default)]
    pub mode_switch_message: Option<String>,
}

/// Called after `absolute_numbering` changes on a series.
///
/// Strategy:
/// 1. Auto-fetch metadata for the new mode if never attempted.
/// 2. Try to re-link files previously moved to `_unmatched/` for this series by matching
///    their stored `episode_id` (in `file_paths`) to the new mode's episode IDs.
/// 3. For remaining downloaded episodes: run title+description matching to detect conflicts.
/// 4. Conflicting/unresolvable episodes are moved to `_unmatched/` — but `file_paths`
///    retains the `episode_id` so that switching back will find and re-link them.
pub async fn handle_mode_switch(
    state: &Arc<AppState>,
    series_id: &str,
    series_title: &str,
    series_path: Option<&str>,
    new_absolute: bool,
) -> (bool, Option<String>) {
    tracing::debug!(
        "handle_mode_switch called: series_id={}, series_title={}, new_absolute={}",
        series_id,
        series_title,
        new_absolute
    );
    let _ = series_title; // used only for display below
    let new_ordering_mode = if new_absolute { "absolute" } else { "normal" };

    if !try_lock_series(state, series_id).await {
        tracing::warn!(
            "handle_mode_switch: series {} is locked, skipping unmatched file relocation",
            series_id
        );
        return (false, None);
    }

    let mapping = match get_series_mapping_or_404(state, series_id).await {
        Ok(m) => m,
        Err(_) => {
            unlock_series(state, series_id).await;
            return (false, None);
        }
    };
    // SSoT: resolve the provider instance + external ID for the NEW mode
    // (priority-ordered, mode-filtered) instead of taking an arbitrary entry.
    let required = Some(if new_absolute {
        jumbie_shared::plugin::Capability::MetadataProviderAbsolute
    } else {
        jumbie_shared::plugin::Capability::MetadataProviderNormal
    });
    let provider = match crate::api_routes::series::metadata::fetch::resolve_provider_for_series(
        state, &mapping, None, required,
    )
    .await
    {
        Ok(p) => p,
        Err(_) => {
            unlock_series(state, series_id).await;
            return (false, None);
        }
    };
    // Cache key is (metadata_id, plugin_id, instance_id): `plugin_id` is the backend
    // TYPE id, `instance_id` the instance.
    let instance_id = provider.instance_id;
    let plugin_id = provider.plugin_id;
    let metadata_id = provider.metadata_id;

    // The stored path may contain a ${series} placeholder. It is resolved here for
    // filesystem ops (sanitized per the illegal-char policy, SSoT:
    // `paths::resolve_template`), while the DB keeps the template form so the path
    // stays valid if the title changes later.
    let org_config = state.org_config().await;
    let resolved_series_path = series_path
        .map(|p| crate::paths::resolve_template(p, series_title, &org_config))
        .filter(|p| p.exists());

    // Step 1: auto-fetch metadata if the new mode was never attempted. Switching
    // modes changes episode IDs (S01E01 → E001), so new-format metadata is needed;
    // fetching automatically avoids a manual step, and
    // `has_fetch_been_attempted` prevents retrying a failed fetch on every toggle.
    let has_new_mode_data = !state
        .db
        .get_metadata_season_cache(&metadata_id, &plugin_id, &instance_id, new_ordering_mode)
        .await
        .unwrap_or_default()
        .is_empty();

    if !has_new_mode_data
        && !state
            .db
            .has_fetch_been_attempted(&metadata_id, &plugin_id, &instance_id, new_ordering_mode)
            .await
    {
        tracing::info!(
            "Triggering metadata fetch for {} in {} mode after mode switch, submitting to queue (P1)",
            series_title,
            new_ordering_mode
        );
        let state_for_queue = state.clone();
        let sid = series_id.to_string();
        state
            .metadata_queue
            .submit(
                sid.clone(),
                crate::metadata_queue::Priority::Normal,
                move || {
                    let state = state_for_queue.clone();
                    let sid = sid.clone();
                    async move {
                        if let Err(e) =
                            crate::api_routes::series::metadata::fetch_metadata_for_series(
                                &state, &sid, None,
                            )
                            .await
                        {
                            tracing::warn!(
                                "Mode-switch metadata fetch failed for {}: {:?}",
                                sid,
                                e
                            );
                        }
                    }
                },
            )
            .await;
    }

    // Step 2: re-link files previously quarantined in _unmatched/. When modes
    // switch, previously-matched episodes may get different IDs; rather than
    // deleting the file it is moved to _unmatched/ while file_paths keeps
    // the original episode_id. This makes the switch reversible — switching back
    // re-links from _unmatched/.
    if let Some(ref sp) = resolved_series_path {
        let unmatched_dir = sp.join("_unmatched");
        if unmatched_dir.exists() {
            // Season folders are rendered from the series' active format — the same
            // format the organizer uses — rather than assumed to be `S01`. The
            // series' highest season sizes any `:auto` spec.
            let folder_config = state.cfg.read().await.clone();
            let folder_episodes = state
                .db
                .get_series_episodes_details(series_id, new_absolute)
                .await
                .unwrap_or_default();
            let folder_context =
                crate::file_manager::get_mapping_context(&folder_episodes, new_absolute);
            let season_pad_options = crate::utils::TemplatePadOptions {
                max_season: folder_context.max_season as u32,
                max_episode: 0,
                max_length: 0,
            };

            // Episode IDs are `{series_id}_...` (series_id is the UUID), so the
            // prefix must be the id — not the display title, which never appears
            // in an episode ID.
            let prefix = format!("{}_", series_id);
            if let Ok(rows) = state
                .db
                .get_unmatched_fingerprints_for_series(&prefix)
                .await
            {
                for (fp_path, fp_episode_id) in rows {
                    let src = std::path::Path::new(&fp_path);
                    if !src.exists() {
                        continue;
                    }

                    // Absolute numbering has a single canonical season; a normal ID
                    // encodes its own (`{series}_S02E05`).
                    let season_num = if new_absolute {
                        jumbie_shared::mapping::ABSOLUTE_SEASON_NUM
                    } else {
                        jumbie_shared::formatting::parse_season_from_episode_id(&fp_episode_id)
                            .unwrap_or(jumbie_shared::mapping::ABSOLUTE_SEASON_NUM)
                    };
                    let season_folder = crate::file_manager::season_folder_name(
                        &folder_config,
                        &mapping,
                        season_num,
                        &season_pad_options,
                    );
                    let filename = match src.file_name() {
                        Some(n) => n.to_string_lossy().to_string(),
                        None => continue,
                    };
                    let dest = sp.join(&season_folder).join(&filename);

                    if let Some(parent) = dest.parent() {
                        let _ = tokio::fs::create_dir_all(parent).await;
                    }
                    match tokio::fs::rename(src, &dest).await {
                        Ok(_) => {
                            let dest_str = dest.to_string_lossy().to_string();
                            let _ = state.db.move_fingerprint_path(&fp_path, &dest_str).await;
                            let _ = state
                                .db
                                .relink_episode_file(&fp_episode_id, &dest_str)
                                .await;
                            tracing::debug!(
                                "Re-linked {} back from _unmatched to '{}'",
                                fp_episode_id,
                                dest.display()
                            );
                        }
                        Err(e) => {
                            tracing::warn!("Failed to restore {} from _unmatched: {}", fp_path, e)
                        }
                    }
                }
            }
        }
    }

    // Step 3: mode switch notification. Episodes are kept separate per mode
    // (numbering_mode column), so no conflict detection or quarantining is needed —
    // all data from the previous mode survives untouched.

    // Warn only if the old mode actually has episode data worth preserving.
    let old_mode_has_data = state
        .db
        .get_series_episodes_details(series_id, !new_absolute)
        .await
        .map(|eps| !eps.is_empty())
        .unwrap_or(false);

    if !old_mode_has_data {
        unlock_series(state, series_id).await;
        return (false, None);
    }

    unlock_series(state, series_id).await;
    (
        true,
        Some(format!(
            "Switched to {} mode. Episode data from the other mode has been preserved and will reappear if you switch back.",
            new_ordering_mode
        )),
    )
}

pub async fn update_series(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(payload): Json<jumbie_shared::types::UpdateSeriesPayload>,
) -> Result<Json<UpdateSeriesResponse>, AppError> {
    crate::validation::validate_quality_profile(&payload.quality_profile)
        .or_bad_request("Invalid quality_profile")?;

    if let Some(ref title) = payload.title {
        crate::validation::validate_title(title).or_bad_request("Invalid title")?;
    }

    payload.settings.validate().map_err(AppError::BadRequest)?;

    // Extra validation for aliases in overrides
    for override_rule in payload.settings.season.values() {
        for alias in &override_rule.aliases {
            crate::validation::validate_alias(alias).or_bad_request("Invalid season alias")?;
        }
    }
    for override_rule in payload.settings.season_absolute.values() {
        for alias in &override_rule.aliases {
            crate::validation::validate_alias(alias).or_bad_request("Invalid season alias")?;
        }
    }

    let mut file_operation: Option<(
        jumbie_shared::types::PathOperation,
        std::path::PathBuf,
        std::path::PathBuf,
        String,
    )> = None;
    let mut monitor_mode_update = None;
    let old_absolute_numbering: bool;
    let new_absolute_numbering: bool;
    let series_title_for_mode_switch: String;
    let series_path_for_mode_switch: Option<String>;
    // The org policy is needed for path resolution here and the file-op collision
    // check below; the read lock must be released before any async I/O. The global
    // numbering default is snapshotted once for the mode resolution and the
    // post-move monitor refresh.
    let org_config = state.org_config().await;
    let global_absolute = {
        let c = state.cfg.read().await;
        c.general.absolute_numbering
    };

    {
        let mut mapping = get_series_mapping_or_404(&state, &id).await?;

        old_absolute_numbering = mapping
            .settings
            .effective_absolute_numbering(global_absolute);
        series_title_for_mode_switch = mapping.target_title.clone();
        series_path_for_mode_switch = mapping.settings.path.clone();

        tracing::debug!(
            "Received update for {} ({}): quality_profile={}, title={:?}, absolute_numbering={:?}",
            mapping.target_title,
            id,
            payload.quality_profile,
            payload.title,
            payload.settings.absolute_numbering
        );

        // Capture old release profile so we can detect changes below
        let old_release_profile = mapping.release_profile.clone();

        // SSoT: config.organization.primary_root
        let root = {
            let config = state.cfg.read().await;
            config.organization.primary_root().to_path_buf()
        };

        if let Some(new_title) = payload.title
            && !new_title.trim().is_empty()
        {
            mapping.target_title = new_title;
        }

        mapping.quality_profile = Some(payload.quality_profile.clone());
        mapping.release_profile = Some(payload.release_profile.clone());
        // Deduplicate aliases on save — prevents duplicates from manual form entry.
        // Skips empty strings (blank lines from textarea) and uses a HashSet for
        // O(1) lookups, preserving insertion order.
        {
            let mut seen = std::collections::HashSet::new();
            mapping.settings.aliases = payload
                .settings
                .aliases
                .into_iter()
                .filter(|a| !a.is_empty() && seen.insert(a.clone()))
                .collect();
        }
        mapping.settings.reg_patterns = payload.settings.reg_patterns.clone();
        mapping.settings.season = payload.settings.season.clone();
        mapping.settings.season_absolute = payload.settings.season_absolute.clone();
        mapping.settings.season_folder_format = payload.settings.season_folder_format.clone();
        mapping.settings.episode_file_format = payload.settings.episode_file_format.clone();
        mapping.settings.season_folder_format_absolute =
            payload.settings.season_folder_format_absolute.clone();
        mapping.settings.episode_file_format_absolute =
            payload.settings.episode_file_format_absolute.clone();
        mapping.settings.flatten_season_folders = payload.settings.flatten_season_folders;
        mapping.settings.absolute_numbering = payload.settings.absolute_numbering;
        // These search/rename controls are plain per-series settings — copy them
        // too, otherwise a save silently drops the change (storage SSoT).
        mapping.settings.rename_episodes = payload.settings.rename_episodes;
        mapping.settings.search_format = payload.settings.search_format;
        mapping.settings.search_format_absolute = payload.settings.search_format_absolute;
        // Resolve the NEW mode through the same SSoT helper as `old`, so the two
        // sides can't diverge. `payload.settings.absolute_numbering` is a tristate:
        // `None` means "inherit global" and must resolve against
        // `config.general.absolute_numbering`, not default to `false`. Defaulting
        // to `false` made a no-op update (series `None` + globally absolute) look
        // like a switch to normal mode, triggering a spurious file move.
        new_absolute_numbering = mapping
            .settings
            .effective_absolute_numbering(global_absolute);

        if let Some(mode) = payload.settings.monitor_mode {
            let old_mode = mapping.settings.monitor_mode;
            mapping.settings.monitor_mode = Some(mode);
            if old_mode != Some(mode) {
                monitor_mode_update = Some(mode);
            }
        }

        // Prune `metadata_last_synced_at` entries for instance ids whose
        // metadata ID changed or were removed entirely.  This ensures the
        // background refresh loop re-fetches metadata for the new ID on its
        // next tick instead of skipping based on a stale timestamp.
        let old_ids = &mapping.settings.metadata_ids;
        let new_ids = &payload.settings.metadata_ids;
        mapping
            .settings
            .metadata_last_synced_at
            .retain(|k, _| old_ids.get(k) == new_ids.get(k));

        mapping.settings.metadata_ids = payload.settings.metadata_ids.clone();
        let old_path_str = mapping.settings.path.clone();

        if let Some(ref dir_name) = payload.settings.path {
            if !dir_name.trim().is_empty() {
                let series_title = mapping.target_title.clone();
                // SSoT: resolve ${series} with the org illegal-char policy
                // (shared `paths::resolve_template`) so the validated path
                // matches the folder that will actually be created — a raw
                // replace could inject illegal chars (e.g. a title with `/`).
                let new_resolved_str =
                    crate::paths::resolve_template(dir_name, &series_title, &org_config)
                        .to_string_lossy()
                        .to_string();

                // validate_path is the SSoT for path resolution (traversal,
                // length, permissions); the returned PathBuf is the authoritative
                // resolved path used for all subsequent checks.
                let validated_path = crate::validation::validate_path(&new_resolved_str, &root)
                    .map_err(|e| {
                        tracing::error!("Path validation failed: {}", e);
                        AppError::BadRequest(format!("Invalid path or lacking permissions: {}", e))
                    })?;

                let stored_path = if old_path_str.as_deref() != Some(dir_name.as_str()) {
                    // Path changed — compute the effective destination (SSoT shared
                    // with create_series and the validate-path preview): sanitize
                    // the folder name per the illegal-char policy, reclaim hidden
                    // series, reject visible-series claims, then resolve folder
                    // collisions per the collision config.
                    let effective = super::helpers::resolve_series_folder_path(
                        &state,
                        &validated_path,
                        &org_config,
                        Some(&id),
                        true,
                    )
                    .await
                    .map_err(|e| AppError::BadRequest(e.to_string()))?;

                    if effective == validated_path {
                        // Nothing changed — keep the original (possibly templated)
                        // form so future title changes still resolve.
                        // Separators are normalized because a template like
                        // "C:\path/${series}" (mixed / and \) makes PathBuf::eq report
                        // two semantically identical Windows paths as different,
                        // triggering unwanted moves.
                        dir_name.replace('/', std::path::MAIN_SEPARATOR_STR)
                    } else {
                        // Sanitization or collision handling changed the folder
                        // name — store the concrete effective path so the mapping
                        // matches the folder that actually exists on disk.
                        effective.to_string_lossy().to_string()
                    }
                } else {
                    // Path unchanged — keep the original stored form.
                    dir_name.replace('/', std::path::MAIN_SEPARATOR_STR)
                };
                mapping.settings.path = Some(stored_path);
            } else {
                mapping.settings.path = None;
            }
        }
        let new_path_str = mapping.settings.path.clone();

        if let (Some(old), Some(new), Some(op)) =
            (&old_path_str, &new_path_str, &payload.path_operation)
            && old != new
            && *op != jumbie_shared::types::PathOperation::DoNothing
        {
            let new_series_title = mapping.target_title.clone();
            // Paths are stored with ${series} templates for portability across title
            // changes, but filesystem ops need real paths. Both sides resolve via
            // `paths::resolve_template` (policy sanitization). The OLD path uses the
            // PRE-update title (the on-disk folder was built from it), the NEW path
            // the post-update title.
            let old_resolved =
                crate::paths::resolve_template(old, &series_title_for_mode_switch, &org_config);
            let new_resolved = crate::paths::resolve_template(new, &new_series_title, &org_config);
            file_operation = Some((op.clone(), old_resolved, new_resolved, new_series_title));
        }

        state
            .db
            .upsert_series_mapping(&id, &mapping)
            .await
            .map_err(|e| {
                tracing::error!(
                    "Failed to save mapping to DB for series {} ({}): {}",
                    mapping.target_title,
                    id,
                    e
                );
                AppError::Internal(anyhow::anyhow!(format!("Failed to save mapping: {}", e)))
            })?;
        tracing::debug!(
            "Successfully updated configuration for series {} ({})",
            mapping.target_title,
            id
        );

        // Invalidate the rename plan cache — the mapping changed (title,
        // template, settings, etc.), so the cached plan is stale.

        // Wake the auto-apply renames scheduler so it picks up title,
        // naming-template, or path changes immediately instead of waiting
        // for the 30-second fallback timeout.
        state.rename_queue_trigger.notify_one();

        // Retroactive rescore: if the release profile changed, recalculate scores
        // for episodes with stored release titles. Always safe — rescore is
        // idempotent and skips episodes with NULL release_title.
        let profile_changed = old_release_profile.as_deref() != Some(&payload.release_profile)
            || (old_release_profile.is_some() != payload.release_profile.is_empty());
        if profile_changed {
            let sid = id.clone();
            let state_clone = state.clone();
            // Run rescore asynchronously — the user doesn't need to wait
            tokio::spawn(async move {
                if let Err(e) = state_clone.db.rescore_episodes_for_profiles(&[sid]).await {
                    tracing::error!("Rescore after profile change failed: {}", e);
                }
            });
        }
    }

    // Path operations (Move/Copy/Delete) are deferred until AFTER the mapping
    // save: if the filesystem operation crashes partway, the mapping already points
    // at the new location and the scanner re-discovers files on restart. Doing it
    // the other way would leave files at the new location with the old path in the
    // DB — an invisible orphan. Order is always: filesystem first, DB second.
    //
    // `update_series_paths` updates paths in-place for only the files that were
    // physically inside the old directory; files outside it (e.g. in a downloads
    // folder) keep their existing DB records and stay tracked.
    if let Some((op, old_resolved, new_resolved, _series_title)) = file_operation {
        // Both sides are normalized before comparison: on Windows PathBuf::eq
        // compares OsStr byte-by-byte, so "C:\path/file" != "C:\path\file" and
        // "C:\Path" != "c:\path" despite being the same location.
        // `normalize_path` (dunce::canonicalize) is the SSoT for cross-platform
        // normalization, shared with validate_path and get_organized_series.
        let exists = old_resolved.exists();
        let same_path = exists
            && crate::validation::normalize_path(&old_resolved)
                == crate::validation::normalize_path(&new_resolved);

        // Check that the new path isn't already claimed by a different series
        if !same_path {
            let all_mappings = state.db.get_all_series_mappings().await.unwrap_or_default();
            if let Err(msg) = crate::validation::check_series_path_not_taken(
                &new_resolved,
                Some(&id),
                &all_mappings,
                &org_config,
            ) {
                tracing::error!("Cannot move series '{}': {}", id, msg);
                // Don't abort — just log and proceed. The directory-level
                // collision will be handled by the filesystem operations below.
                // The DB mapping path is updated regardless.
            }
        }

        if exists && !same_path {
            if !try_lock_series(&state, &id).await {
                return Err(AppError::Conflict(
                    "Series is currently being modified by another operation (e.g. batch move)"
                        .to_string(),
                ));
            }

            // `move_series_directory` handles scan queue cancellation, parent dir
            // creation, EXDEV fallback, and empty dir cleanup.
            let result = move_series_directory(
                &state,
                &old_resolved,
                &new_resolved,
                op.clone(),
                &state.shutdown_token,
            )
            .await;

            // Post-file DB updates
            let scan_dest = match &op {
                jumbie_shared::types::PathOperation::Move
                | jumbie_shared::types::PathOperation::Copy => {
                    // Files moved to new location — update DB paths to match.
                    let _ = state
                        .db
                        .update_series_paths(&old_resolved, &new_resolved)
                        .await;
                    Some(new_resolved.clone())
                }
                jumbie_shared::types::PathOperation::Delete
                | jumbie_shared::types::PathOperation::DoNothing => {
                    // Old files gone or untouched — clear stale DB paths.
                    let _ = state.db.clear_series_paths(&old_resolved).await;
                    // Re-evaluate monitor status — cleared file_paths change has_file.
                    // Uses series-wide refresh (Option B) since path changes are rare
                    // and overrides are respected.
                    crate::source_processor::reapply_monitor_for_series(
                        &state.db,
                        &id,
                        false,
                        global_absolute,
                    )
                    .await;
                    Some(new_resolved.clone())
                }
            };

            if result.is_err() {
                tracing::error!(
                    "Failed to perform path operation {:?} for series {}: {:?}",
                    op,
                    id,
                    result
                );
            } else if let Some(ref dest) = scan_dest
                && let Ok(Some(m)) = state.db.get_series_mapping(&id).await
            {
                let _ = crate::scanner::scan_series_directory(dest, &m, &state).await;
            }

            unlock_series(&state, &id).await;
        } else if !old_resolved.exists() {
            tracing::info!(
                "Old path '{}' didn't exist to perform path operation",
                old_resolved.display()
            );
        }
    }

    if monitor_mode_update.is_some() {
        crate::api_routes::series::apply_monitor_mode(&state, &id)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    }

    let (mode_switch_warning, mode_switch_message) =
        if old_absolute_numbering != new_absolute_numbering {
            handle_mode_switch(
                &state,
                &id,
                &series_title_for_mode_switch,
                series_path_for_mode_switch.as_deref(),
                new_absolute_numbering,
            )
            .await
        } else {
            (false, None)
        };

    tracing::debug!(
        "update_series completed for series {}: mode_switch_warning={}",
        id,
        mode_switch_warning
    );
    Ok(Json(UpdateSeriesResponse {
        mode_switch_warning,
        mode_switch_message,
    }))
}
