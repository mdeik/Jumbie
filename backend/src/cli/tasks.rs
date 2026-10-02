use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use jumbie_shared::plugin::Capability;
use jumbie_shared::types::EpisodeStatus;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use tracing::{debug, trace};

use jumbie::api::AppState;
use jumbie::db::DbManager;
use jumbie::organizer::ContentOrganizer;
use jumbie::plugins::PluginManager;
use jumbie::task::TaskRegistry;

// Returns `true` when `state_key`'s last-run timestamp is still within `interval`
// (the caller should skip). Missing or unparseable keys return `false` so the
// caller runs. SSoT for the "get_system_state + parse + compare" pattern.
async fn should_skip_by_interval(
    db: &DbManager,
    state_key: &str,
    interval: std::time::Duration,
) -> bool {
    let Ok(Some(last_run_str)) = db.get_system_state(state_key).await else {
        return false;
    };
    let Ok(last_run) = jumbie::datetime::parse_utc(&last_run_str) else {
        return false;
    };
    let elapsed = chrono::Utc::now().signed_duration_since(last_run.to_chrono_utc());
    let Ok(elapsed_std) = elapsed.to_std() else {
        return false;
    };
    elapsed_std < interval
}

/// Register all background tasks and startup sweeps.
///
/// Called once after the router and AppState are created.
/// Each task is registered with a name and an optional stagger delay to spread
/// initial CPU/IO load across the startup window.
pub(crate) async fn register_all(
    task_registry: &mut TaskRegistry,
    state: &Arc<AppState>,
    db_manager: &Arc<DbManager>,
    app: &ContentOrganizer,
    _plugin_manager: &Arc<RwLock<PluginManager>>,
    shutdown_token: &CancellationToken,
) {
    // Background Source Sync Loop
    let sync_app = app.clone();
    let db_manager_sync = db_manager.clone();

    tracing::info!("Background source sync orchestrator started (checks every 1 minute)");

    task_registry.register_recurring("Source Sync Loop", None, move |shutdown_token| {
        let sync_app = sync_app.clone();
        let db_manager = db_manager_sync.clone();
        async move {
            let start_time = std::time::Instant::now();

            let plugins = {
                let pm_lock = sync_app.plugin_manager();
                let pm = pm_lock.read().await;
                pm.get_plugins_by_all_capabilities(&[
                    Capability::FeedProvider,
                    Capability::Polling,
                ])
            };

            for plugin in plugins {
                if jumbie::task::is_shutdown_requested(&shutdown_token, "source sync loop") {
                    break;
                }
                let instance_id = plugin.instance_id().to_string();

                let interval_mins = plugin
                    .refresh_interval()
                    .unwrap_or(jumbie::plugins::refresh_interval_default("source"));
                let interval_duration = std::time::Duration::from_secs(interval_mins * 60);
                let state_key = format!("source_last_synced_{}", instance_id);

                if should_skip_by_interval(&db_manager, &state_key, interval_duration).await {
                    let remaining = interval_duration
                        - db_manager
                            .get_system_state(&state_key)
                            .await
                            .ok()
                            .flatten()
                            .and_then(|s| {
                                jumbie::datetime::parse_utc(&s)
                                    .ok()
                                    .map(|dt| {
                                        chrono::Utc::now()
                                            .signed_duration_since(dt.to_chrono_utc())
                                            .to_std()
                                            .unwrap_or(std::time::Duration::ZERO)
                                    })
                                    .map(|elapsed| interval_duration.saturating_sub(elapsed))
                            })
                            .unwrap_or(interval_duration);
                    debug!(
                        "Skipping source sync for plugin {} ({}) — refresh interval not yet elapsed ({}s remaining)",
                        plugin.plugin_info().display_name,
                        instance_id,
                        remaining.as_secs(),
                    );
                    continue;
                }

                trace!("Source plugin check: instance_id={}, should_run=true", instance_id);

                tracing::debug!("Triggering background source sync for plugin {} ({})", plugin.plugin_info().display_name, instance_id);
                if let Err(e) = sync_app.process_sources(Some(&instance_id)).await {
                    tracing::error!(
                        "Error in background source sync for {} ({}): {}",
                        plugin.plugin_info().display_name,
                        instance_id,
                        e
                    );
                } else {
                    let _ = db_manager
                        .set_system_state(&state_key, &jumbie::datetime::UtcDateTime::now().to_db_string())
                        .await;
                }
            }

            let loop_interval = std::time::Duration::from_secs(60);
            jumbie::task::compute_sleep_duration(start_time, loop_interval)
        }
    });

    // Background Media Info Scanner. Reads config from state.cfg (ConfigManager)
    // rather than the TOML config so UI changes take effect at runtime.
    let scan_state = state.clone();
    let db_manager_scan = db_manager.clone();
    let scan_queue_for_scanner = state.scan_queue.clone();
    let organizer_for_monitor = state.organizer.clone();

    let initial_cfg = state.cfg.read().await;
    let status = if initial_cfg.general.media_info_scan_enabled {
        "Enabled"
    } else {
        "Disabled"
    };
    tracing::info!(
        "Background media info scanner is {} (Interval: {} minutes)",
        status,
        initial_cfg.general.media_info_scan_interval
    );
    drop(initial_cfg);

    task_registry.register_recurring("Media Info Scanner", None, move |shutdown_token| {
        let db = db_manager_scan.clone();
        let scan_queue = scan_queue_for_scanner.clone();
        let scan_state = scan_state.clone();
        let organizer = organizer_for_monitor.clone();
        async move {
            trace!("Media Info Scanner cycle executing");

            let (is_enabled, interval_mins) = {
                let c = scan_state.cfg.read().await;
                (
                    c.general.media_info_scan_enabled,
                    c.general.media_info_scan_interval.max(1),
                )
            };

            let interval_duration = std::time::Duration::from_secs(interval_mins * 60);

            if !is_enabled {
                return std::time::Duration::from_secs(60);
            }

            // Also skip when ffprobe is not available — submitting would just
            // mark every file as permanently failed for the wrong reason.
            if jumbie::utils::media_info::ffprobe_version().is_none() {
                return std::time::Duration::from_secs(60);
            }

            let start_time = std::time::Instant::now();

            let paths = match db.get_files_without_media_info(50).await {
                Ok(p) => p,
                Err(e) => {
                    tracing::error!("Failed to fetch files missing media info: {}", e);
                    return jumbie::task::compute_sleep_duration(start_time, interval_duration);
                }
            };

            if paths.is_empty() {
                return jumbie::task::compute_sleep_duration(start_time, interval_duration);
            }

            tracing::debug!(
                "Found {} files missing media info, submitting to scan queue...",
                paths.len()
            );

            let mut affected_series: HashSet<String> = HashSet::new();
            for path_str in &paths {
                if jumbie::task::is_shutdown_requested(&shutdown_token, "media info scanner") {
                    return jumbie::task::compute_sleep_duration(start_time, interval_duration);
                }
                if let Ok(Some(series_id)) = db.get_series_id_by_file_path(path_str).await {
                    affected_series.insert(series_id);
                }
            }

            for path_str in paths {
                if jumbie::task::is_shutdown_requested(&shutdown_token, "media info scanner") {
                    return jumbie::task::compute_sleep_duration(start_time, interval_duration);
                }
                let path_buf = std::path::PathBuf::from(&path_str);
                let db_clone = db.clone();
                let path_clone = path_str.clone();
                scan_queue
                    .submit(path_buf.clone(), move || {
                        let db = db_clone.clone();
                        let path = path_clone.clone();
                        async move {
                            db.scan_file_fingerprint(
                                &std::path::PathBuf::from(&path),
                                EpisodeStatus::Organized.as_str(),
                            )
                            .await;
                        }
                    })
                    .await;
            }

            if let Some(ref org) = organizer {
                for sid in &affected_series {
                    if jumbie::task::is_shutdown_requested(&shutdown_token, "media info scanner") {
                        return jumbie::task::compute_sleep_duration(start_time, interval_duration);
                    }
                    org.refresh_monitor_status_for_series(sid).await;
                }
            } else {
                for sid in &affected_series {
                    if jumbie::task::is_shutdown_requested(&shutdown_token, "media info scanner") {
                        return jumbie::task::compute_sleep_duration(start_time, interval_duration);
                    }
                    if db.get_series_mapping(sid).await.ok().flatten().is_some() {
                        jumbie::source_processor::reapply_monitor_for_series(
                            &db,
                            sid,
                            true, // fresh_apply: first-time registration
                            db.get_general_config()
                                .await
                                .unwrap_or_default()
                                .absolute_numbering,
                        )
                        .await;
                    }
                }
            }

            jumbie::task::compute_sleep_duration(start_time, interval_duration)
        }
    });

    // Auto-Apply Renames: waits on a `Notify` (fired when `update_series` saves a
    // title/naming-template change) with a 30s fallback so on-disk changes are
    // still caught. The user-facing notifier has its own 10s cooldown
    // (`RenameQueueNotifier`).
    let auto_apply_state = state.clone();
    tracing::info!("Starting background Auto-Apply Renames scheduler (Notify + 30s fallback)");
    // 60s initial delay: storage may not be ready on a fresh restart, and
    // transient move failures would be recorded as failed_renames (spurious
    // critical indicator in the UI).
    task_registry.register_recurring(
        "Auto-Apply Renames",
        Some(std::time::Duration::from_secs(60)),
        move |shutdown_token| {
            let auto_apply_state = auto_apply_state.clone();
            async move {
                // Wait for trigger or 30s fallback.
                tokio::select! {
                    _ = auto_apply_state.rename_queue_trigger.notified() => {
                        trace!("Auto-Apply Renames: triggered by series update");
                    }
                    _ = tokio::time::sleep(std::time::Duration::from_secs(30)) => {
                        trace!("Auto-Apply Renames: fallback timeout");
                    }
                }

                if jumbie::task::is_shutdown_requested(&shutdown_token, "auto-apply renames") {
                    return std::time::Duration::from_millis(100);
                }

                let is_enabled = {
                    let c = auto_apply_state.cfg.read().await;
                    c.organization.auto_apply_renames
                };

                trace!("Auto-Apply Renames check: enabled={}", is_enabled);

                if is_enabled
                    && let Err(e) =
                        jumbie::api_routes::series::reorganize_all_impl(&auto_apply_state).await
                {
                    tracing::error!("Error in Auto-Apply Renames: {}", e);
                }

                // Brief yield; the real wait happens in the `select!` above.
                std::time::Duration::from_millis(100)
            }
        },
    );

    // Metadata Refresh Loop (hourly check, per-plugin intervals)
    let metadata_app_state = state.clone();
    let db_manager_meta = db_manager.clone();

    tracing::info!(
        "Starting Metadata refresh loop (checking every 1 hour for per-plugin intervals)"
    );

    task_registry.register_recurring(
        "Metadata Refresh Loop",
        Some(std::time::Duration::from_secs(300)),
        move |shutdown_token| {
            let metadata_app_state = metadata_app_state.clone();
            let db_manager = db_manager_meta.clone();
            async move {
                let start_time = std::time::Instant::now();

                // Single-active metadata policy (SSoT:
                // `PluginManager::active_metadata_provider`): only the highest-priority
                // provider is polled, and only if it opted into `Polling` — two
                // providers polling would duplicate traffic and conflict writes.
                let plugins = {
                    let pm_lock = metadata_app_state.plugin_manager.read().await;
                    pm_lock
                        .active_metadata_provider()
                        .filter(|p| {
                            pm_lock
                                .effective_capabilities_for(p)
                                .contains(&Capability::Polling)
                        })
                        .into_iter()
                        .collect::<Vec<_>>()
                };

                for plugin in plugins {
                    if jumbie::task::is_shutdown_requested(&shutdown_token, "metadata refresh loop") {
                        break;
                    }
                    let instance_id = plugin.instance_id().to_string();

                    // Metadata `refresh_interval` is stored in HOURS (SSoT:
                    // `plugins::refresh_interval_unit`), unlike source plugins whose
                    // value is minutes.
                    let interval_hours = plugin
                        .refresh_interval()
                        .unwrap_or(jumbie::plugins::refresh_interval_default("metadata"))
                        .max(1);

                    let interval_duration = std::time::Duration::from_secs(interval_hours * 3600);
                    let state_key = format!("metadata_last_synced_{}", instance_id);

                    if should_skip_by_interval(&db_manager, &state_key, interval_duration).await {
                        tracing::debug!(
                            "Skipping metadata refresh for plugin {} ({}) — interval not yet elapsed",
                            plugin.plugin_info().display_name,
                            instance_id,
                        );
                        continue;
                    }

                    // Refresh every series mapped to THIS instance. The plugin-level
                    // interval gate is the throttle, so we deliberately do NOT filter
                    // by the provider's "recently updated" feed — that stranded series
                    // whose upstream data hadn't changed within the window.
                    let series_to_refresh: Vec<(String, String)> = {
                        let all_mappings = db_manager
                            .get_all_series_mappings()
                            .await
                            .unwrap_or_default();
                        all_mappings
                            .iter()
                            .filter_map(|(id, mapping)| {
                                // Key by THIS instance: `metadata_ids` is keyed by
                                // instance id, so `.values().next()` (arbitrary
                                // provider) would pick the wrong ID for a series
                                // mapped to more than one provider.
                                let metadata_id =
                                    mapping.settings.metadata_ids.get(&instance_id)?.clone();
                                Some((id.clone(), metadata_id))
                            })
                            .collect()
                    };

                    let total = series_to_refresh.len();

                    for (series_id, metadata_id) in series_to_refresh {
                        if jumbie::task::is_shutdown_requested(&shutdown_token, "metadata refresh loop") {
                            break;
                        }
                        let state_for_closure = metadata_app_state.clone();
                        let sid = series_id.clone();
                        metadata_app_state.metadata_queue.submit(
                            sid.clone(),
                            jumbie::metadata_queue::Priority::Background,
                            move || {
                                let state = state_for_closure.clone();
                                let sid = sid.clone();
                                let mid = metadata_id.clone();
                                async move {
                                    if let Err(e) = jumbie::api_routes::series::fetch_metadata_for_series(
                                        &state, &sid, Some(mid),
                                    ).await {
                                        tracing::warn!(
                                            "Metadata refresh: failed to refresh series {}: {:?}",
                                            sid, e
                                        );
                                    }
                                    // Episode titles may have changed — wake
                                    // auto-apply to re-evaluate rename plans.
                                    state.rename_queue_trigger.notify_one();
                                }
                            },
                        ).await;
                    }

                    tracing::debug!(
                        "Metadata refresh for {} ({}): submitted {} series to queue (P2)",
                        plugin.plugin_info().display_name,
                        instance_id,
                        total
                    );

                    let _ = db_manager
                        .set_system_state(&state_key, &jumbie::datetime::UtcDateTime::now().to_db_string())
                        .await;
                }

                let loop_interval = std::time::Duration::from_secs(3600);
                jumbie::task::compute_sleep_duration(start_time, loop_interval)
            }
        },
    );

    // Stale File Cleanup (every 30 minutes): files rarely disappear, so 30min
    // catches the same orphans with far fewer stat() sweeps than 10min would.
    let stale_file_state = state.clone();
    task_registry.register_recurring(
        "Stale File Cleanup",
        Some(std::time::Duration::from_secs(300)),
        move |shutdown_token| {
            let stale_file_state = stale_file_state.clone();
            async move {
                let start_time = std::time::Instant::now();
                let thirty_minutes = std::time::Duration::from_secs(30 * 60);

                if jumbie::task::is_shutdown_requested(&shutdown_token, "stale file cleanup") {
                    return jumbie::task::compute_sleep_duration(start_time, thirty_minutes);
                }

                match stale_file_state.db.cleanup_missing_files().await {
                    Ok((affected, count)) => {
                        if count > 0 {
                            tracing::info!("Cleaned up {} stale file references", count);
                            for (ep_id, series_id) in &affected {
                                jumbie::source_processor::reapply_monitor_for_episode(
                                    &stale_file_state.db,
                                    series_id,
                                    ep_id,
                                    false,
                                    stale_file_state
                                        .db
                                        .get_general_config()
                                        .await
                                        .unwrap_or_default()
                                        .absolute_numbering,
                                )
                                .await;
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!("Error cleaning up stale files: {}", e);
                    }
                }

                jumbie::task::compute_sleep_duration(start_time, thirty_minutes)
            }
        },
    );

    // Series Directory Scanner
    let series_scan_state = state.clone();

    const FORCE_SCAN_BUCKETS: u64 = 6;
    const FORCE_SCAN_PERIOD_CYCLES: u64 = 6;

    let mut cycle_counter: u64 = 0;

    tracing::info!(
        "Starting Series Directory Scanner (checking every configured interval, default 10 min)"
    );
    task_registry.register_recurring(
        "Series Directory Scanner",
        Some(std::time::Duration::from_secs(120)),
        move |shutdown_token| {
            cycle_counter += 1;
            let current_cycle = cycle_counter;
            let series_scan_state = series_scan_state.clone();
            async move {
                let start_time = std::time::Instant::now();

                let (is_enabled, interval_mins, max_concurrent) = {
                    let c = series_scan_state.cfg.read().await;
                    (
                        c.general.series_scan_enabled,
                        c.general.series_scan_interval.max(1),
                        3usize,
                    )
                };

                let interval_duration = std::time::Duration::from_secs(interval_mins * 60);

                if !is_enabled {
                    trace!("Series Directory Scanner disabled in config");
                    return jumbie::task::compute_sleep_duration(start_time, interval_duration);
                }

                let force_scan_counter = current_cycle % FORCE_SCAN_PERIOD_CYCLES;
                let is_force_cycle = force_scan_counter == 0;
                let force_bucket = if is_force_cycle {
                    let bucket_idx =
                        (current_cycle / FORCE_SCAN_PERIOD_CYCLES) % FORCE_SCAN_BUCKETS;
                    Some(bucket_idx)
                } else {
                    None
                };

                let all_mappings = match series_scan_state.db.get_all_series_mappings().await {
                    Ok(m) => m,
                    Err(e) => {
                        tracing::error!("Series scanner: failed to fetch mappings: {}", e);
                        return jumbie::task::compute_sleep_duration(start_time, interval_duration);
                    }
                };

                if is_force_cycle {
                    tracing::debug!(
                        "Series scanner: force-scan bucket {} of {} (cycle {})",
                        force_bucket.unwrap(),
                        FORCE_SCAN_BUCKETS,
                        current_cycle,
                    );
                }

                let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(max_concurrent));

                let mut scan_count = 0usize;
                let mut skip_count = 0usize;
                let mut error_count = 0usize;
                let mut force_scan_count = 0usize;

                let state_ref = series_scan_state.clone();
                let mut handles = Vec::new();

                for (series_id, mapping) in &all_mappings {
                    if jumbie::task::is_shutdown_requested(
                        &shutdown_token,
                        "series directory scanner",
                    ) {
                        break;
                    }
                    let series_path = match &mapping.settings.path {
                        Some(p) if !p.is_empty() => std::path::PathBuf::from(p),
                        _ => {
                            skip_count += 1;
                            continue;
                        }
                    };

                    if !series_path.exists() || !series_path.is_dir() {
                        skip_count += 1;
                        continue;
                    }

                    let mut should_scan = false;

                    // Force-bucket check first — no filesystem access needed.
                    if let Some(bucket) = force_bucket {
                        let series_bucket =
                            jumbie::scanner::stable_hash_series_id(series_id) % FORCE_SCAN_BUCKETS;
                        if series_bucket == bucket {
                            should_scan = true;
                            force_scan_count += 1;
                            tracing::trace!(
                                "Series scanner: force-scan '{}' (bucket {}/{})",
                                mapping.target_title,
                                bucket,
                                FORCE_SCAN_BUCKETS,
                            );
                        }
                    }

                    // Mtime check — requires stat syscalls on every subdirectory.
                    // Skip if the force-bucket already decided we're scanning.
                    if !should_scan {
                        let current_mtimes = jumbie::scanner::collect_dir_mtimes(&series_path, 5);
                        let last_mtimes = &mapping.settings.last_known_dir_mtimes;
                        should_scan =
                            jumbie::scanner::has_any_dir_changed(&current_mtimes, last_mtimes);
                    }

                    if !should_scan {
                        skip_count += 1;
                        continue;
                    }

                    if state_ref
                        .processing_renames
                        .read()
                        .await
                        .contains(series_id)
                    {
                        tracing::debug!(
                            "Series scanner: skipping '{}' ({}) — reorganization in progress",
                            mapping.target_title,
                            series_id,
                        );
                        skip_count += 1;
                        continue;
                    }

                    let state_for_scan = state_ref.clone();
                    let series_id_for_scan = series_id.clone();
                    let path_for_scan = series_path.clone();
                    let mapping_for_scan = mapping.clone();
                    let shutdown_for_scan = shutdown_token.clone();
                    let permit = semaphore.clone().acquire_owned().await;

                    match permit {
                        Ok(_permit) => {
                            let handle = tokio::spawn(async move {
                                if jumbie::task::is_shutdown_requested(
                                    &shutdown_for_scan,
                                    "series directory scanner sub-task",
                                ) {
                                    return;
                                }

                                tracing::debug!(
                                    "Series scanner: scanning '{}' ({}) at '{}' (mtime changed)",
                                    mapping_for_scan.target_title,
                                    series_id_for_scan,
                                    path_for_scan.display(),
                                );

                                match jumbie::scanner::scan_series_directory(
                                    &path_for_scan,
                                    &mapping_for_scan,
                                    &state_for_scan,
                                )
                                .await
                                {
                                    Ok(count) => {
                                        if count > 0 {
                                            tracing::debug!(
                                                "Series scanner: found {} episodes in '{}'",
                                                count,
                                                path_for_scan.display(),
                                            );

                                            jumbie::source_processor::reapply_monitor_for_series(
                                                &state_for_scan.db,
                                                &series_id_for_scan,
                                                false, // sweep: preserve existing monitored state
                                                state_for_scan
                                                    .db
                                                    .get_general_config()
                                                    .await
                                                    .unwrap_or_default()
                                                    .absolute_numbering,
                                            )
                                            .await;
                                        }
                                    }
                                    Err(e) => {
                                        tracing::warn!(
                                            "Series scanner: scan failed for '{}': {}",
                                            path_for_scan.display(),
                                            e,
                                        );
                                    }
                                }
                            });
                            handles.push(handle);
                            scan_count += 1;
                        }
                        Err(_) => {
                            error_count += 1;
                        }
                    }
                }

                for handle in handles {
                    if jumbie::task::is_shutdown_requested(
                        &shutdown_token,
                        "series directory scanner handle await",
                    ) {
                        // Shutdown requested — stop waiting for remaining scans.
                        // Orphaned handles will complete on their own; the outer
                        // loop will see the token on next iteration and exit.
                        break;
                    }
                    if let Err(e) = handle.await {
                        tracing::warn!("Series scanner: scan task failed: {}", e);
                    }
                }

                if scan_count > 0 || is_force_cycle {
                    let mut details = format!(
                        "scanned={}, skipped={}, errors={}",
                        scan_count, skip_count, error_count,
                    );
                    if is_force_cycle {
                        details.push_str(&format!(
                            ", force-scanned={} (bucket {}/{})",
                            force_scan_count,
                            force_bucket.unwrap(),
                            FORCE_SCAN_BUCKETS,
                        ));
                    }
                    tracing::debug!("Series scanner cycle complete: {}", details);
                }

                jumbie::task::compute_sleep_duration(start_time, interval_duration)
            }
        },
    );

    // Download Orchestrator (every 3 seconds). Must stay single-flight:
    // `register_recurring` awaits each pass before sleeping, and no-progress
    // detection relies on it (a stall must not be counted twice). SSoT: does NOT
    // touch `rename_queue_has_pending` — that flag is owned by get_rename_queue
    // (sets) and reorganize_all_core (clears), which can compute the full queue.
    let download_app = app.clone();
    task_registry.register_recurring("Download Orchestrator", None, move |shutdown_token| {
        let download_app = download_app.clone();
        async move {
            if jumbie::task::is_shutdown_requested(&shutdown_token, "download orchestrator") {
                return std::time::Duration::ZERO;
            }
            if let Err(e) = download_app.process_downloads().await {
                tracing::error!("Error in background download orchestrator: {}", e);
            }
            std::time::Duration::from_secs(3)
        }
    });

    // Wanted Episodes Auto-Search
    let auto_search_wanted_state = state.clone();
    tracing::info!("Starting Wanted Episodes auto-search background loop");
    task_registry.register_recurring(
        "Wanted Episodes Auto-Search",
        Some(std::time::Duration::from_secs(180)),
        move |shutdown_token| {
            let state = auto_search_wanted_state.clone();
            async move {
                let start_time = std::time::Instant::now();

                let (enabled, interval_mins, min_wait_mins) = {
                    let cfg = state.cfg.read().await;
                    (
                        cfg.general.auto_search_wanted_enabled,
                        cfg.general.auto_search_wanted_interval.max(1),
                        cfg.general.auto_search_wanted_min_wait.max(1),
                    )
                };

                let interval_duration = std::time::Duration::from_secs(interval_mins * 60);

                if !enabled {
                    trace!("Wanted Episodes Auto-Search: disabled in config");
                    return jumbie::task::compute_sleep_duration(start_time, interval_duration);
                }

                let now = chrono::Utc::now().naive_utc();
                let cutoff_youngest =
                    now - chrono::Duration::minutes(min_wait_mins as i64);
                let max_age_days = {
                    let cfg = state.cfg.read().await;
                    cfg.general.auto_search_wanted_max_age_days
                };
                let cutoff_oldest = if max_age_days > 0 {
                    Some(now - chrono::Duration::days(max_age_days as i64))
                } else {
                    None
                };

                let candidates = match state
                    .db
                    .get_auto_search_candidates(cutoff_youngest, cutoff_oldest)
                    .await
                {
                    Ok(items) => items,
                    Err(e) => {
                        tracing::error!(
                            "Wanted Episodes Auto-Search: failed to fetch candidates: {}",
                            e
                        );
                        return jumbie::task::compute_sleep_duration(
                            start_time,
                            interval_duration,
                        );
                    }
                };

                if candidates.is_empty() {
                    trace!("Wanted Episodes Auto-Search: no candidates");
                    return jumbie::task::compute_sleep_duration(start_time, interval_duration);
                }

                let mut groups: HashMap<(String, String), Vec<i32>> = HashMap::new();
                for (series_id, season, episode) in &candidates {
                    let season_str = season
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| "1".to_string());
                    groups
                        .entry((series_id.clone(), season_str))
                        .or_default()
                        .push(*episode);
                }

                let mut qualifying: Vec<((String, String), Vec<i32>)> = Vec::new();
                for (key, current_eps) in groups.drain() {
                    let (ref series_id, ref season) = key;
                    let state_key = format!(
                        "auto_search_last_attempt_{}_{}",
                        series_id, season
                    );

                    let current_set: std::collections::BTreeSet<i32> =
                        current_eps.iter().copied().collect();

                    if let Ok(Some(raw)) = state.db.get_system_state(&state_key).await
                        && let Ok(val) =
                            serde_json::from_str::<serde_json::Value>(&raw)
                        {
                            let recent_enough = val["timestamp"]
                                .as_str()
                                .and_then(|ts| {
                                    jumbie::datetime::parse_utc(ts).ok()
                                })
                                .map(|dt| {
                                    chrono::Utc::now()
                                        .signed_duration_since(dt.to_chrono_utc())
                                        .num_minutes()
                                        < interval_mins as i64
                                })
                                .unwrap_or(false);

                            if recent_enough {
                                let stored_eps: std::collections::BTreeSet<i32> = val["episodes"]
                                    .as_array()
                                    .map(|arr| {
                                        arr.iter()
                                            .filter_map(|v| v.as_i64())
                                            .map(|n| n as i32)
                                            .collect()
                                    })
                                    .unwrap_or_default();

                                if stored_eps == current_set {
                                    trace!(
                                        "Wanted Episodes Auto-Search: {} searched {} min ago, same {} episodes, skipping",
                                        state_key,
                                        val["timestamp"]
                                            .as_str()
                                            .and_then(|ts| {
                                                jumbie::datetime::parse_utc(ts).ok()
                                            })
                                            .map(|dt| {
                                                chrono::Utc::now()
                                                    .signed_duration_since(dt.to_chrono_utc())
                                                    .num_minutes()
                                            })
                                            .unwrap_or(0),
                                        current_set.len(),
                                    );
                                    continue;
                                }

                                trace!(
                                    "Wanted Episodes Auto-Search: {} episode set changed ({}→{}), searching despite cooldown",
                                    state_key,
                                    stored_eps.len(),
                                    current_set.len(),
                                );
                            }
                        }

                    qualifying.push((key, current_eps));
                }

                if qualifying.is_empty() {
                    trace!("Wanted Episodes Auto-Search: all groups recently searched");
                    return jumbie::task::compute_sleep_duration(start_time, interval_duration);
                }

                tracing::info!(
                    "Wanted Episodes Auto-Search: {} series/seasons to search",
                    qualifying.len(),
                );

                if let Some(ref organizer) = state.organizer {
                    for ((series_id, season), episode_numbers) in &qualifying {
                        if jumbie::task::is_shutdown_requested(&shutdown_token, "wanted episodes auto-search") {
                            break;
                        }

                        let key = format!("{}:{}", series_id, season);
                        let org = organizer.clone();
                        let sid = series_id.clone();
                        let s = season.clone();
                        let eps = episode_numbers.clone();
                        let db = state.db.clone();

                        // Use submit() — dedup is handled by SearchQueue internally.
                        state.search_queue.submit(key, move || {
                            let org = org;
                            let sid = sid;
                            let s = s;
                            let eps = eps;
                            let db = db;
                            async move {
                                tracing::debug!(
                                    "Wanted Episodes Auto-Search: searching series={} season={} episodes={:?}",
                                    sid,
                                    s,
                                    eps,
                                );

                                if let Err(e) = org.auto_search_missing(&sid, &s, &eps).await {
                                    tracing::error!(
                                        "Wanted Episodes Auto-Search: error searching series={} season={}: {}",
                                        sid,
                                        s,
                                        e,
                                    );
                                }

                                let state_key = format!("auto_search_last_attempt_{}_{}", sid, s);
                                let state_value = serde_json::json!({
                                    "timestamp": jumbie::datetime::UtcDateTime::now().to_db_string(),
                                    "episodes": &eps,
                                });
                                let _ = db
                                    .set_system_state(&state_key, &state_value.to_string())
                                    .await;
                            }
                        }).await;
                    }
                } else {
                    tracing::warn!("Wanted Episodes Auto-Search: organizer not available");
                }

                jumbie::task::compute_sleep_duration(start_time, interval_duration)
            }
        },
    );

    // Startup Estimation Sweep: re-estimate all series at boot so estimation-logic
    // changes apply immediately instead of waiting for the recurring sweep. The
    // estimator is idempotent (skips DB writes when estimates match).
    {
        let est_db = db_manager.clone();
        tracing::info!("Running startup release date estimation for all series...");
        let start = std::time::Instant::now();
        match est_db.get_all_series_ids().await {
            Ok(ids) => {
                let count = ids.len();
                for series_id in &ids {
                    if let Err(e) =
                        jumbie::release_estimator::trigger_estimation_for_series(&est_db, series_id)
                            .await
                    {
                        tracing::warn!("Startup estimation failed for series {}: {}", series_id, e);
                    }
                }
                tracing::info!(
                    "Startup estimation completed for {} series in {:?}",
                    count,
                    start.elapsed()
                );
            }
            Err(e) => {
                tracing::error!("Failed to fetch series for startup estimation: {}", e);
            }
        }
    }

    // Release Date Estimation (every 6 hours)
    let est_db = db_manager.clone();
    tracing::info!("Starting background Release Date Estimator (Interval: 360 minutes)");
    task_registry.register_recurring(
        "Release Date Estimator",
        Some(std::time::Duration::from_secs(1800)),
        move |shutdown_token| {
            let est_db = est_db.clone();
            async move {
                let start_time = std::time::Instant::now();
                let six_hours = std::time::Duration::from_secs(6 * 3600);

                match est_db.get_all_series_ids().await {
                    Ok(ids) => {
                        let count = ids.len();
                        for series_id in &ids {
                            if jumbie::task::is_shutdown_requested(
                                &shutdown_token,
                                "background estimation sweep",
                            ) {
                                return jumbie::task::compute_sleep_duration(start_time, six_hours);
                            }
                            if let Err(e) =
                                jumbie::release_estimator::trigger_estimation_for_series(
                                    &est_db, series_id,
                                )
                                .await
                            {
                                tracing::warn!(
                                    "Background estimation failed for series {}: {}",
                                    series_id,
                                    e
                                );
                            }
                        }
                        tracing::debug!(
                            "Background estimation refresh completed for {} series",
                            count
                        );
                    }
                    Err(e) => {
                        tracing::error!("Failed to fetch series for background estimation: {}", e);
                    }
                }

                jumbie::task::compute_sleep_duration(start_time, six_hours)
            }
        },
    );

    // Monitor Status Sweep (every 60 minutes)
    let monitor_sweep_state = state.clone();
    tracing::info!("Starting background Monitor Status Sweep (Interval: 60 minutes)");
    task_registry.register_recurring(
        "Monitor Status Sweep",
        Some(std::time::Duration::from_secs(300)),
        move |shutdown_token| {
            let state_for_sweep = monitor_sweep_state.clone();
            async move {
                let start_time = std::time::Instant::now();
                let sixty_mins = std::time::Duration::from_secs(60 * 60);

                if jumbie::task::is_shutdown_requested(&shutdown_token, "monitor status sweep") {
                    return jumbie::task::compute_sleep_duration(start_time, sixty_mins);
                }

                jumbie::api_routes::series::sweep_monitor_status(&state_for_sweep).await;

                jumbie::task::compute_sleep_duration(start_time, sixty_mins)
            }
        },
    );

    // Startup monitor status sweep (once, not recurring)
    let startup_sweep_token = shutdown_token.clone();
    {
        let state_for_sweep = state.clone();
        let token = startup_sweep_token;
        tokio::spawn(async move {
            if jumbie::task::is_shutdown_requested(&token, "startup monitor sweep") {
                return;
            }
            jumbie::api_routes::series::sweep_monitor_status(&state_for_sweep).await;
        });
    }

    // DB Record Cleanup (journal, retry queue, metadata orphans, activity log).
    // Quick tables run every 6h; orphaned metadata is gated to every 4th cycle
    // (~24h); the activity log and file_paths cache are pruned whenever they
    // exceed their byte budgets.
    let mut cleanup_cycle: u8 = 0;
    let cleanup_db = db_manager.clone();
    tracing::info!("Starting DB record cleanup sweep (Interval: 6 hours)");
    task_registry.register_recurring(
        "DB Record Cleanup",
        Some(std::time::Duration::from_secs(600)),
        move |shutdown_token| {
            cleanup_cycle += 1;
            let current_cycle = cleanup_cycle;
            let cleanup_db = cleanup_db.clone();
            async move {
                let start_time = std::time::Instant::now();
                let six_hours = std::time::Duration::from_secs(6 * 3600);

                // Phase 1: quick cleanup (every cycle).

                // Completed journal entries
                match cleanup_db.cleanup_completed_journal_entries().await {
                    Ok(n) if n > 0 => {
                        tracing::info!("Cleaned up {} completed journal entries", n);
                    }
                    Ok(_) => {}
                    Err(e) => tracing::error!("Failed to clean up journal entries: {}", e),
                }

                // Terminal retry entries
                match cleanup_db.cleanup_terminal_retry_entries().await {
                    Ok(n) if n > 0 => {
                        tracing::info!("Cleaned up {} terminal retry entries", n);
                    }
                    Ok(_) => {}
                    Err(e) => tracing::error!("Failed to clean up retry entries: {}", e),
                }

                // Stale blocked-file markers: a blocked file whose name has not
                // been seen for a week is gone, renamed, or replaced — drop the
                // marker so the list stays bounded.
                match cleanup_db.cleanup_stale_blocked_files(7).await {
                    Ok(n) if n > 0 => {
                        tracing::info!("Cleared {n} stale blocked-file markers");
                    }
                    Ok(_) => {}
                    Err(e) => tracing::error!("Failed to clean up blocked-file markers: {}", e),
                }

                if jumbie::task::is_shutdown_requested(&shutdown_token, "db record cleanup") {
                    return six_hours;
                }

                // Phase 2: orphaned metadata (every 4th cycle ≈ 24h).
                if current_cycle.is_multiple_of(4) {
                    trace!(
                        "DB cleanup cycle {}: running orphaned metadata cleanup",
                        current_cycle
                    );
                    match cleanup_db.cleanup_orphaned_metadata().await {
                        Ok(count) => {
                            if count > 0 {
                                tracing::info!("Cleaned up {} orphaned metadata records", count);
                            }
                        }
                        Err(e) => {
                            tracing::error!("Error cleaning up orphaned metadata: {}", e);
                        }
                    }

                    if jumbie::task::is_shutdown_requested(&shutdown_token, "db record cleanup") {
                        return six_hours;
                    }
                } else {
                    trace!(
                        "DB cleanup cycle {}: skipping orphaned metadata (runs every 4th cycle)",
                        current_cycle
                    );
                }

                // Phase 3: activity log pruning, bounded by bytes (never age):
                // oldest rows are pruned while the estimated size exceeds
                // `ACTIVITY_LOG_BUDGET_BYTES`. The size check is a cheap SUM over a
                // sub-MiB table, so it runs every cycle rather than being gated.
                match cleanup_db.activity_log_bytes().await {
                    Ok(bytes) if bytes > jumbie::db::activity::ACTIVITY_LOG_BUDGET_BYTES => {
                        trace!(
                            "DB cleanup cycle {}: pruning activity log ({} B over budget)",
                            current_cycle,
                            bytes - jumbie::db::activity::ACTIVITY_LOG_BUDGET_BYTES
                        );
                        match cleanup_db
                            .prune_activity_log_to_budget(
                                jumbie::db::activity::ACTIVITY_LOG_BUDGET_BYTES,
                            )
                            .await
                        {
                            Ok(n) if n > 0 => {
                                tracing::debug!(
                                    "Pruned {n} activity log entries (over the size budget)"
                                );
                            }
                            Ok(_) => {}
                            Err(e) => tracing::error!("Failed to prune activity log: {}", e),
                        }
                    }
                    Ok(_) => {}
                    Err(e) => tracing::error!("Failed to measure activity log size: {}", e),
                }

                // Phase 4: file_paths size backstop. The disk-existence sweep in
                // `cleanup_missing_files` is the primary bound; this caps the
                // pathological case where live, unassigned rows alone exceed the
                // budget. Assigned rows and auto-adoption candidates are never
                // evicted, so it warns when it fires.
                match cleanup_db.file_paths_bytes().await {
                    Ok(bytes) if bytes > jumbie::db::fingerprints::FILE_PATHS_BUDGET_BYTES => {
                        tracing::warn!(
                            "DB cleanup cycle {}: file_paths cache is {} B over its budget; evicting stale unassigned rows",
                            current_cycle,
                            bytes - jumbie::db::fingerprints::FILE_PATHS_BUDGET_BYTES
                        );
                        match cleanup_db
                            .prune_unassigned_paths_to_budget(
                                jumbie::db::fingerprints::FILE_PATHS_BUDGET_BYTES,
                            )
                            .await
                        {
                            Ok(n) if n > 0 => {
                                tracing::info!(
                                    "Evicted {n} stale unassigned file_paths rows (over the size budget)"
                                );
                            }
                            Ok(_) => {}
                            Err(e) => tracing::error!("Failed to prune file_paths: {}", e),
                        }
                    }
                    Ok(_) => {}
                    Err(e) => tracing::error!("Failed to measure file_paths size: {}", e),
                }

                jumbie::task::compute_sleep_duration(start_time, six_hours)
            }
        },
    );

    // Rename Queue Cache Warm (one-shot). Without it, `rename_queue_has_pending`
    // stays `false` until the UI visits the rename queue page, so the sidebar
    // indicator would be missing on first load.
    //
    // The 5s delay runs after the immediate-tick tasks' first cycle and before
    // Auto-Apply (60s). register_once (not a raw spawn) keeps lifecycle management
    // consistent through the TaskRegistry.
    let cache_warm_state = state.clone();
    task_registry.register_once(
        "Rename Queue Cache Warm",
        Some(std::time::Duration::from_secs(5)),
        move |shutdown_token| {
            let cache_warm_state = cache_warm_state.clone();
            async move {
                if shutdown_token.is_cancelled() {
                    return;
                }
                tracing::debug!("Pre-computing rename queue for cached state...");
                let _ = jumbie::api_routes::system::get_rename_queue(axum::extract::State(
                    cache_warm_state.clone(),
                ))
                .await;
                tracing::debug!("Rename queue cached state initialized.");

                // Wake the auto-apply scheduler so it checks for pending
                // renames as soon as its 60s startup delay elapses, instead
                // of waiting an additional 30s fallback timeout.
                cache_warm_state.rename_queue_trigger.notify_one();
            }
        },
    );
}
