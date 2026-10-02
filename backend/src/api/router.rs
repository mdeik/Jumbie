use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, RwLock};

use tower_http::trace::TraceLayer;

use axum::Router;
use axum::middleware::from_fn;
use axum::routing::{delete, get, post, put};

use super::{AppState, BanInfo, ProgressTracker};
use crate::api_routes::{
    calendar::{get_calendar, get_calendar_ical},
    downloads::{add_download, refresh_series},
    series::files::{
        assign_series_file, batch_assign_series_files, delete_series_files, get_series_files,
        unassign_series_files,
    },
};
use crate::middleware::scope;
use crate::middleware::{auth, security};

pub struct RouterConfig {
    pub cfg: Arc<crate::config_manager::ConfigManager>,
    pub db: Arc<crate::db::DbManager>,
    pub downloader: Option<Arc<RwLock<crate::plugins::downloaders::DownloadManager>>>,
    pub notifications: Option<Arc<RwLock<crate::plugins::notifiers::NotifierManager>>>,
    pub organizer: Option<crate::organizer::ContentOrganizer>,
    pub plugin_manager: Arc<RwLock<crate::plugins::PluginManager>>,
    pub logs_dir: String,
    pub log_level: String,
    pub log_buffer: jumbie_shared::types::LogBuffer,
    pub shutdown_token: tokio_util::sync::CancellationToken,
}

/// Rehydrate the in-memory ban list from the durable DB record.
///
/// Entries still active or permanent are restored as-is. A temporarily expired
/// ban is ALSO restored when it falls within `reset_period_days` so its
/// `ban_count` survives for escalation — but with a *past* `banned_until`
/// sentinel, NOT `None`.
///
/// The auth middleware treats `banned_until == None` as a PERMANENT ban, so
/// rehydrating an expired temporary ban as `None` would silently upgrade it to a
/// lifetime ban after a restart; a past instant means "not currently banned"
/// while keeping `ban_count`/`fail_count` for the next escalation.
pub async fn rehydrate_bans(
    db: &crate::db::DbManager,
    reset_period_days: i64,
) -> HashMap<IpAddr, BanInfo> {
    let banned_ips = db.get_banned_ips().await.unwrap_or_default();

    banned_ips
        .iter()
        .filter_map(|b| {
            b.ip.parse::<IpAddr>().ok().and_then(|ip| {
                let banned_at = crate::datetime::parse_utc(&b.banned_at)
                    .ok()
                    .map(|d| d.to_chrono_utc());
                let banned_until = b.banned_until.as_ref().and_then(|ts| {
                    let expiry = crate::datetime::parse_utc(ts).ok()?.to_chrono_utc();
                    let secs_remaining = expiry
                        .signed_duration_since(chrono::Utc::now())
                        .num_seconds();
                    if secs_remaining > 0 {
                        Some(Instant::now() + Duration::from_secs(secs_remaining as u64))
                    } else {
                        None // expired
                    }
                });

                // Keep expired bans in memory within the reset period so
                // ban_count is remembered: otherwise a user could trigger N
                // fails, wait out the ban, then N-1 more and be re-banned at
                // the same low duration, bypassing the escalation.
                let is_within_reset = banned_at
                    .map(|at| {
                        let age = chrono::Utc::now()
                            .signed_duration_since(at.with_timezone(&chrono::Utc));
                        age.num_days() < reset_period_days
                    })
                    .unwrap_or(false);

                let is_permanent = b.banned_until.is_none();
                let still_active = is_permanent || banned_until.is_some();

                if still_active || is_within_reset {
                    Some((
                        ip,
                        BanInfo {
                            fail_count: b.fail_count,
                            ban_count: b.ban_count,
                            // Permanent bans stay `None`. A temporary ban keeps its
                            // future instant if still active; if only kept for the
                            // reset window, use a past instant so it is NOT treated
                            // as permanent.
                            banned_until: if is_permanent {
                                None
                            } else {
                                Some(banned_until.unwrap_or_else(Instant::now))
                            },
                            banned_at: banned_at.map(|at| at.with_timezone(&chrono::Utc)),
                            last_seen: Instant::now(),
                        },
                    ))
                } else {
                    None
                }
            })
        })
        .collect()
}

/// Map the configured scan concurrency to a semaphore permit count.
///
/// The `UNLIMITED` sentinel maps to the semaphore's maximum (effectively no cap);
/// any other value is clamped to `[1, MAX_PERMITS]` so an out-of-range setting
/// can never panic `Semaphore::new`.
fn scan_concurrency_permits(configured: usize) -> usize {
    if configured == jumbie_shared::config::GeneralConfig::MEDIA_INFO_SCAN_CONCURRENCY_UNLIMITED {
        tokio::sync::Semaphore::MAX_PERMITS
    } else {
        configured.clamp(1, tokio::sync::Semaphore::MAX_PERMITS)
    }
}

pub async fn create_router(config: RouterConfig) -> (Router, Arc<AppState>) {
    // Pre-populate ban_list from the durable DB record so bans survive
    // restarts (active IPs stay banned without new failures) and ban_count is
    // preserved for exponential backoff.
    let initial_bans = {
        let guard = config.cfg.read().await;
        let reset_period = guard.auth.ban_count_reset_days as i64;
        drop(guard);

        rehydrate_bans(&config.db, reset_period).await
    };
    // Media-info scan concurrency from config.
    let concurrency = {
        let config = config.cfg.read().await;
        scan_concurrency_permits(config.general.media_info_scan_concurrency)
    };
    let scan_queue = Arc::new(crate::scan_queue::ScanQueue::with_max_concurrency(
        concurrency,
    ));
    // Share the organizer's modifying_series set so background organize
    // and API route lock checks see the same in-memory state.
    let modifying_series = config
        .organizer
        .as_ref()
        .map(|o| o.modifying_series.clone())
        .unwrap_or_else(|| Arc::new(RwLock::new(std::collections::HashSet::new())));

    let rename_plan_cache = crate::file_manager::RenamePlanCache::new();

    let state = Arc::new(AppState {
        cfg: config.cfg,
        db: config.db,
        downloader: config.downloader,
        notifications: config.notifications,
        organizer: config.organizer,
        ban_list: Arc::new(Mutex::new(initial_bans)),
        is_reorganizing: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        failed_renames: Arc::new(RwLock::new(HashMap::new())),
        processing_renames: Arc::new(RwLock::new(std::collections::HashSet::new())),
        rate_limiter: crate::middleware::rate_limit::new_rate_limiter(),
        auth_cache: Arc::new(crate::middleware::auth_cache::AuthCache::new()),
        plugin_manager: config.plugin_manager,
        scan_queue,
        metadata_queue: Arc::new(crate::metadata_queue::MetadataQueue::new()),
        rename_queue_has_pending: Arc::new(RwLock::new(false)),
        rename_queue_trigger: Arc::new(tokio::sync::Notify::new()),
        search_queue: Arc::new(crate::search_queue::SearchQueue::new()),
        shutdown_token: config.shutdown_token,
        logs_dir: config.logs_dir,
        log_level: config.log_level,
        log_buffer: config.log_buffer,
        monitor_sweep_cancel: Arc::new(Mutex::new(None)),
        recalc_cancel: Arc::new(Mutex::new(None)),
        progress_tracker: ProgressTracker::new(),
        rename_plan_cache,
        modifying_series,
    });

    // Route construction: grouped by scope. One sub-router per scope applies
    // its middleware once via `.route_layer()`, then sub-routers are merged —
    // cheaper per request than per-route middleware. Public routes are added
    // directly to the main router to skip scope-check overhead entirely.

    // Public
    // These must be reachable without credentials: theme is called before login
    // to render the login page, and ping is used by health checks.
    let public = Router::new()
        .route(
            "/api/public/theme",
            get(crate::api_routes::settings::get_public_theme),
        )
        .route("/api/public/ping", get(crate::api_routes::system::ping));

    // Series Read / Activity Read
    let series_read = Router::new()
        .route("/api/series", get(crate::api_routes::series::get_series))
        .route(
            "/api/series/details/batch",
            post(crate::api_routes::series::get_series_details_batch),
        )
        .route(
            "/api/series/{id}",
            get(crate::api_routes::series::get_series_details),
        )
        .route("/api/series/{id}/files", get(get_series_files))
        .route("/api/calendar", get(get_calendar))
        .route(
            "/api/episodes/batch-monitor",
            post(crate::api_routes::series::batch_monitor_episodes),
        )
        .route_layer(from_fn(scope::series_read));

    let activity_read = Router::new()
        .route(
            "/api/activity",
            get(crate::api_routes::system::get_activity),
        )
        .route_layer(from_fn(scope::activity_read));

    let wanted_read = Router::new()
        .route("/api/wanted", get(crate::api_routes::system::get_wanted))
        .route_layer(from_fn(scope::wanted_read));

    // Public: iCal
    // Consumed by external calendar apps that authenticate via a token query
    // parameter, not HTTP Basic/Bearer headers; validated inside the handler.
    let ical = Router::new().route("/api/calendar/ical", get(get_calendar_ical));

    // Series Write
    let series_write = Router::new()
        .route(
            "/api/series",
            post(crate::api_routes::series::create_series),
        )
        .route(
            "/api/series/batch-edit",
            post(crate::api_routes::series::batch_edit_series),
        )
        .route(
            "/api/series/batch-delete",
            post(crate::api_routes::series::batch_remove_series),
        )
        .route(
            "/api/series/batch-fetch-metadata",
            post(crate::api_routes::series::batch_fetch_metadata),
        )
        .route(
            "/api/series/{id}",
            put(crate::api_routes::series::update_series)
                .delete(crate::api_routes::series::remove_series),
        )
        .route(
            "/api/series/{id}/season/{season}",
            delete(crate::api_routes::series::delete_season),
        )
        .route("/api/series/{id}/refresh", post(refresh_series))
        .route(
            "/api/series/{id}/sync_metadata",
            post(crate::api_routes::series::sync_metadata),
        )
        .route(
            "/api/series/{id}/fetch_metadata",
            post(crate::api_routes::series::fetch_metadata),
        )
        .route(
            "/api/series/{id}/fetch_series_info",
            post(crate::api_routes::series::fetch_series_info),
        )
        .route(
            "/api/series/{id}/fetch_series_aliases",
            post(crate::api_routes::series::fetch_series_aliases),
        )
        .route("/api/series/{id}/files", delete(delete_series_files))
        .route("/api/series/{id}/files/assign", post(assign_series_file))
        .route(
            "/api/series/{id}/files/unassign",
            post(unassign_series_files),
        )
        .route(
            "/api/series/{id}/files/batch-assign",
            post(batch_assign_series_files),
        )
        .route(
            "/api/series/{id}/clear-not-found-files",
            post(crate::api_routes::series::files::clear_not_found_files),
        )
        .route(
            "/api/series/{id}/actions/monitor_episodes",
            put(crate::api_routes::series::monitor_episodes),
        )
        .route(
            "/api/series/{id}/actions/reorganize",
            post(crate::api_routes::series::reorganize_series),
        )
        .route(
            "/api/series/actions/reorganize_all",
            post(crate::api_routes::series::reorganize_all_series),
        )
        .route(
            "/api/series/actions/reorganize_all_async",
            post(crate::api_routes::series::reorganize_all_series_async),
        )
        .route(
            "/api/series/actions/reorganize_all/{task_id}/status",
            get(crate::api_routes::series::reorganize_all_status),
        )
        .route(
            "/api/episodes/{id}/scan_media",
            post(crate::api_routes::series::scan_episode_media),
        )
        .route(
            "/api/episodes/{id}/est_date",
            post(crate::api_routes::series::update_est_date),
        )
        .route(
            "/api/episodes/{id}/monitor",
            put(crate::api_routes::series::toggle_episode_monitor),
        )
        .route(
            "/api/series/{id}/actions/delete_episode_data",
            post(crate::api_routes::series::delete_episode_data),
        )
        .route(
            "/api/series/{id}/actions/reset_configuration",
            post(crate::api_routes::series::reset_configuration),
        )
        .route(
            "/api/series/{id}/season/{season}/actions/delete_episode_data",
            post(crate::api_routes::series::delete_season_episode_data),
        )
        .route(
            "/api/series/{id}/season/{season}/actions/reset_configuration",
            post(crate::api_routes::series::reset_season_configuration),
        )
        .route(
            "/api/series/{id}/season/{season}/restore_metadata",
            post(crate::api_routes::series::restore::restore_season_metadata),
        )
        .route(
            "/api/series/{id}/seasons",
            post(crate::api_routes::series::batch_upsert_seasons),
        )
        .route(
            "/api/series/{id}/metadata-cache",
            delete(crate::api_routes::series::clear_metadata_cache),
        )
        .route(
            "/api/series/{id}/episodes/{episode_id}/metadata",
            put(crate::api_routes::series::save_custom_metadata)
                .delete(crate::api_routes::series::clear_episode_metadata),
        )
        .route(
            "/api/series/{id}/episodes/{episode_id}/restore_metadata",
            post(crate::api_routes::series::restore_episode_metadata),
        )
        .route(
            "/api/series/{id}/season/{season}/match",
            post(crate::api_routes::series::match_season_to_provider),
        )
        .route(
            "/api/series/{id}/metadata/match",
            post(crate::api_routes::series::match_series_to_provider),
        )
        .route_layer(from_fn(scope::series_write));

    // Config Read
    let config_read = Router::new()
        .route("/api/config", get(crate::api_routes::settings::get_config))
        .route(
            "/api/bootstrap",
            get(crate::api_routes::settings::get_bootstrap),
        )
        .route(
            "/api/config/qualities",
            get(crate::api_routes::settings::get_quality_definitions),
        )
        .route(
            "/api/config/quality_profiles",
            get(crate::api_routes::settings::get_quality_profiles),
        )
        .route(
            "/api/config/release_profiles",
            get(crate::api_routes::settings::get_release_profiles),
        )
        .route(
            "/api/config/ui_preferences",
            get(crate::api_routes::settings::get_ui_preferences_endpoint),
        )
        .route(
            "/api/automatic-profiles",
            get(crate::api_routes::settings::get_automatic_profiles),
        )
        .route(
            "/api/automatic-profiles/{submitter}/records",
            get(crate::api_routes::settings::get_automatic_profile_records),
        )
        .route_layer(from_fn(scope::config_read));

    // Config Write
    let config_write = Router::new()
        .route(
            "/api/config",
            put(crate::api_routes::settings::update_config),
        )
        .route(
            "/api/config/qualities",
            put(crate::api_routes::settings::put_quality_definitions),
        )
        .route(
            "/api/config/quality_profiles",
            put(crate::api_routes::settings::put_quality_profiles),
        )
        .route(
            "/api/config/release_profiles",
            put(crate::api_routes::settings::put_release_profiles),
        )
        .route(
            "/api/config/ui_preferences",
            put(crate::api_routes::settings::put_ui_preferences_endpoint),
        )
        .route(
            "/api/automatic-profiles/{submitter}",
            delete(crate::api_routes::settings::delete_automatic_profile),
        )
        .route(
            "/api/automatic-profiles/recalculate",
            post(crate::api_routes::settings::recalculate_automatic_scores),
        )
        .route_layer(from_fn(scope::config_write));

    // Plugins Read
    let plugins_read = Router::new()
        .route(
            "/api/config/plugins_cfg",
            get(crate::api_routes::settings::get_plugins_config_endpoint),
        )
        .route(
            "/api/plugins",
            get(crate::api_routes::settings::get_plugins),
        )
        .route(
            "/api/plugins/available",
            get(crate::api_routes::settings::get_available_plugins),
        )
        .route(
            "/api/plugins/schemas",
            get(crate::api_routes::settings::get_all_plugin_schemas),
        )
        .route(
            "/api/plugins/{name}/schema",
            get(crate::api_routes::settings::get_plugin_schema),
        )
        .route(
            "/api/plugins/status",
            get(crate::api_routes::settings::get_plugin_status),
        )
        .route_layer(from_fn(scope::plugins_read));

    // Plugins Write
    let plugins_write = Router::new()
        .route(
            "/api/config/plugins_cfg/{section}/{plugin_id}/instances",
            post(crate::api_routes::settings::create_plugin_instance_endpoint),
        )
        .route(
            "/api/config/plugins_cfg/{section}/{plugin_id}/{instance_id}",
            delete(crate::api_routes::settings::delete_plugin_instance_endpoint),
        )
        .route(
            "/api/config/plugins_cfg/{section}",
            put(crate::api_routes::settings::put_plugins_section_endpoint),
        )
        .route(
            "/api/plugins/{name}/validate",
            post(crate::api_routes::settings::validate_plugin_config),
        )
        .route(
            "/api/plugins/test",
            post(crate::api_routes::settings::test_plugin),
        )
        .route_layer(from_fn(scope::plugins_write));

    // Auth Write
    let auth_write = Router::new()
        .route(
            "/api/config/auth/api_keys/generate",
            post(crate::api_routes::auth::generate_api_key),
        )
        .route(
            "/api/config/auth/calendar_tokens/generate",
            post(crate::api_routes::auth::generate_calendar_token),
        )
        .route_layer(from_fn(scope::auth_write));

    // Files Read
    let files_read = Router::new()
        .route(
            "/api/system/organized_series",
            get(crate::api_routes::system::get_organized_series),
        )
        .route_layer(from_fn(scope::files_read));

    // Files Write
    let files_write = Router::new()
        .route(
            "/api/system/validate-path",
            post(crate::api_routes::system::validate_path_endpoint),
        )
        .route(
            "/api/system/organized_series/{series_id}/toggle",
            put(crate::api_routes::system::toggle_series_monitor),
        )
        .route(
            "/api/system/organized_series/{series_id}/visibility",
            put(crate::api_routes::system::toggle_library_visibility),
        )
        .route(
            "/api/system/organized_series/bulk",
            post(crate::api_routes::series::bulk_create_series),
        )
        .route(
            "/api/system/organized_series/batch_edit",
            post(crate::api_routes::system::batch_edit_organized_series),
        )
        .route(
            "/api/system/organized_series/preview",
            post(crate::api_routes::series::preview_import),
        )
        .route(
            "/api/system/organized_series/batch_move_preview",
            post(crate::api_routes::system::batch_move_series_preview),
        )
        .route(
            "/api/system/organized_series/batch_move",
            post(crate::api_routes::system::batch_move_series),
        )
        .route(
            "/api/system/organized_series/batch_move/{task_id}/status",
            get(crate::api_routes::system::batch_move_status),
        )
        .route(
            "/api/system/active_operations",
            get(crate::api_routes::system::get_active_operations),
        )
        .route_layer(from_fn(scope::files_write));

    // Rename Write
    let rename_write = Router::new()
        .route(
            "/api/system/remediate",
            post(crate::api_routes::system::remediate),
        )
        .route_layer(from_fn(scope::rename_write));

    // Queue Read
    let queue_read = Router::new()
        .route("/api/queue", get(crate::api_routes::system::get_queue))
        .route_layer(from_fn(scope::queue_read));

    // Queue Write
    let queue_write = Router::new()
        .route("/api/downloads", post(add_download))
        .route(
            "/api/queue/{id}",
            delete(crate::api_routes::system::remove_from_queue),
        )
        .route(
            "/api/queue/{id}/pause",
            post(crate::api_routes::system::pause_download_queue),
        )
        .route(
            "/api/queue/{id}/resume",
            post(crate::api_routes::system::resume_download_queue),
        )
        .route(
            "/api/queue/{id}/delete",
            post(crate::api_routes::system::delete_download_queue),
        )
        .route(
            "/api/queue/{id}/retry",
            post(crate::api_routes::system::retry_download_queue),
        )
        .route_layer(from_fn(scope::queue_write));

    // Search
    // Two sub-routers: /api/search (manual) needs only the `search` scope, while
    // /api/search/auto-season triggers downloads and requires BOTH `search` and
    // `queue:write` — separate routers make the permission boundaries explicit.
    let search = Router::new()
        .route("/api/search", post(crate::api_routes::system::search_media))
        .route_layer(from_fn(scope::search));

    let search_auto_season = Router::new()
        .route(
            "/api/search/auto-season",
            post(crate::api_routes::system::auto_season_search),
        )
        .route(
            "/api/search/auto-season/{series_id}/{season}/status",
            get(crate::api_routes::system::get_auto_season_status),
        )
        .route_layer(from_fn(scope::search_and_queue_write));

    // System Read
    let system_read = Router::new()
        .route(
            "/api/system/health",
            get(crate::api_routes::system::get_health),
        )
        .route(
            "/api/system/plugins-metrics",
            get(crate::api_routes::system::get_plugin_metrics),
        )
        .route(
            "/api/system/about",
            get(crate::api_routes::system::get_about),
        )
        .route(
            "/api/system/memory",
            get(crate::api_routes::system::get_memory_stats),
        )
        .route(
            "/api/media-info-scan/counts",
            get(crate::api_routes::system::get_media_info_scan_counts),
        )
        .route(
            "/api/media-info-scan/counts/{series_id}",
            get(crate::api_routes::system::get_media_info_scan_count_for_series),
        )
        .route(
            "/api/status",
            get(crate::api_routes::system::get_system_status),
        );

    let system_read = system_read.route_layer(from_fn(scope::system_read));

    // Logs Read
    let logs_read = Router::new()
        .route("/api/system/logs", get(crate::api_routes::system::get_logs))
        .route(
            "/api/system/logs/file",
            get(crate::api_routes::system::get_logs_file),
        )
        .route(
            "/api/system/log-level",
            get(crate::api_routes::system::get_log_level),
        )
        .route_layer(from_fn(scope::logs_read));

    // Rename Read
    let rename_read = Router::new()
        .route(
            "/api/system/rename_queue",
            get(crate::api_routes::system::get_rename_queue),
        )
        .route(
            "/api/system/rename_queue/{series_id}",
            get(crate::api_routes::system::get_rename_queue_detail),
        )
        .route_layer(from_fn(scope::rename_read));

    // Auth Read
    let auth_read = Router::new()
        .route("/api/auth/bans", get(crate::api_routes::bans::list_bans))
        .route_layer(from_fn(scope::auth_read));

    // Auth Write (bans)
    let auth_write_bans = Router::new()
        .route("/api/auth/bans", post(crate::api_routes::bans::add_ban))
        .route(
            "/api/auth/bans/{ip}",
            delete(crate::api_routes::bans::remove_ban),
        )
        .route_layer(from_fn(scope::auth_write));

    // Merge all sub-routers into the main router. Each carries its own scope
    // middleware; merging preserves those layers. Order doesn't matter since
    // Axum merges method routers per-path.
    let router = Router::new()
        .merge(public)
        .merge(series_read)
        .merge(activity_read)
        .merge(wanted_read)
        .merge(ical)
        .merge(series_write)
        .merge(config_read)
        .merge(config_write)
        .merge(plugins_read)
        .merge(plugins_write)
        .merge(auth_write)
        .merge(files_read)
        .merge(files_write)
        .merge(rename_write)
        .merge(queue_read)
        .merge(queue_write)
        .merge(search)
        .merge(search_auto_season)
        .merge(system_read)
        .merge(logs_read)
        .merge(rename_read)
        .merge(auth_read)
        .merge(auth_write_bans);

    // Global middleware. Layering order (innermost → outermost):
    //
    // 1. Per-route `from_fn(scope::*)` checks — decide whether the caller has
    //    the required ApiScope.
    // 2. `auth::auth_interceptor` — credential validation, ban checks, and
    //    localhost/subnet bypass; injects ApiScopes into request extensions for
    //    the scope middleware to consume.
    // 3. `rate_limit::rate_limit_middleware` — applies to ALL requests,
    //    including authenticated ones, so it must be outside auth; inside
    //    security_headers because CORS preflight OPTIONS must never be
    //    rate-limited.
    // 4. `security::security_headers` — must run for unauthenticated requests
    //    (CORS preflight must never hit auth); validates Host, applies CORS and
    //    security headers.
    // 5. `client_ip::resolve_client_ip_middleware` — outermost application
    //    layer; resolves the client IP once so the rate limiter and auth
    //    middleware key on the same identity.
    let router = router
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::auth_interceptor,
        ))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            crate::middleware::rate_limit::rate_limit_middleware,
        ))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            security::security_headers,
        ))
        // Client IP resolution
        // Done once per request; both the rate limiter and auth middleware read
        // the shared `ClientIp`, so they cannot diverge.
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            crate::middleware::client_ip::resolve_client_ip_middleware,
        ))
        // Request/Response Tracing
        // TraceLayer is outermost so it observes every request entering the
        // server, including those rejected by auth or security middleware.
        // Log levels let operators pick verbosity: info = operational, debug =
        // per-request spans, trace = every URL + response code + latency.
        .layer(
            TraceLayer::new_for_http()
                .on_request(
                    |request: &axum::http::Request<axum::body::Body>, _span: &tracing::Span| {
                        tracing::trace!("→ {} {}", request.method(), request.uri());
                    },
                )
                .on_response(
                    |response: &axum::http::Response<axum::body::Body>,
                     latency: std::time::Duration,
                     _span: &tracing::Span| {
                        tracing::trace!("← {} ({}ms)", response.status(), latency.as_millis());
                    },
                ),
        )
        .with_state(state.clone());

    // Frontend Serving Strategy
    // The frontend is embedded via `rust-embed` (build.rs); if available serve
    // it, otherwise run in API-only mode.
    let router = if crate::embedded_frontend::is_available() {
        tracing::info!("Serving embedded frontend (built into binary)");
        add_embedded_fallback(router)
    } else {
        tracing::warn!("No frontend available — not compiled into binary.");
        router
    };

    // Progress-tracker reaper: periodically removes stale entries auto-finalised
    // by the TaskHandle Drop safety net (panic path where async cleanup can't run).
    state.progress_tracker.start_reaper();

    // Auth-state reaper: bounds the in-memory ban_list and rate_limiter (evicts
    // stale non-banned entries and old rate-limit windows) and prunes
    // long-expired bans from the DB.
    crate::middleware::reaper::start(state.clone());

    (router, state)
}

/// Add an Axum fallback handler that serves the frontend from embedded assets.
///
/// Hashed assets are served with `immutable` cache headers (the URL changes
/// when content changes); SPA routes fall back to `index.html`.
fn add_embedded_fallback(router: Router) -> Router {
    use axum::{
        body::Body,
        http::{Response, StatusCode, header},
        response::IntoResponse,
    };

    router.fallback(move |req: axum::http::Request<Body>| async move {
        // Unmatched /api/* paths must return 404, not the SPA index.html.
        if req.uri().path().starts_with("/api/") {
            return StatusCode::NOT_FOUND.into_response();
        }

        match crate::embedded_frontend::serve(req.uri().path()) {
            Some(asset) => {
                let resp = Response::builder()
                    .header(header::CONTENT_TYPE, asset.mime)
                    .header(header::CACHE_CONTROL, asset.cache_control)
                    .body(Body::from(asset.body))
                    .unwrap();
                resp.into_response()
            }
            None => StatusCode::NOT_FOUND.into_response(),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use jumbie_shared::config::GeneralConfig;

    #[test]
    fn test_unlimited_scan_concurrency_maps_to_semaphore_max() {
        assert_eq!(
            scan_concurrency_permits(GeneralConfig::MEDIA_INFO_SCAN_CONCURRENCY_UNLIMITED),
            tokio::sync::Semaphore::MAX_PERMITS,
        );
    }

    #[test]
    fn test_scan_concurrency_is_clamped_to_valid_range() {
        assert_eq!(scan_concurrency_permits(4), 4);
        // Zero (other than the unlimited sentinel can't occur, but guard anyway)
        // and absurd values must not exceed the semaphore limit.
        assert_eq!(
            scan_concurrency_permits(usize::MAX),
            tokio::sync::Semaphore::MAX_PERMITS
        );
    }
}
