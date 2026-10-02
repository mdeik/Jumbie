use axum::{Json, extract::State, http::StatusCode};
use std::sync::Arc;

use crate::api::AppState;
use crate::error::AppError;

#[derive(serde::Serialize)]
pub struct StorageHealth {
    pub path: String,
    pub free_bytes: u64,
    pub total_bytes: u64,
}

#[derive(serde::Serialize)]
pub struct PermissionCheck {
    pub path: String,
    pub can_read: bool,
    pub can_write: bool,
}

#[derive(serde::Serialize)]
pub struct SystemHealth {
    pub status: String,
    pub uptime_seconds: u64,
    pub memory_used: u64,
    pub memory_total: u64,
    pub cpu_usage: f32,
    pub db_integrity_status: String,
    pub storage_health: Vec<StorageHealth>,
    pub dir_permissions: Vec<PermissionCheck>,
    pub ffmpeg_installed: bool,
    pub ffmpeg_version: Option<String>,
    pub downloader_is_disabled: bool,
}

#[derive(serde::Serialize)]
pub struct AboutInfo {
    pub version: String,
    pub os: String,
    pub git_commit: String,
    pub build_date: String,
    pub license: String,
    pub description: String,
}

/// Per-TYPE plugin process metrics — in-process reads only (no RPC to plugin
/// processes): call counts, error counts, restart counts, queue depth, and
/// process health. External types only (internal plugins have no process).
pub async fn get_plugin_metrics(
    State(state): State<Arc<AppState>>,
) -> Json<Vec<jumbie_shared::types::payloads::PluginTypeMetrics>> {
    let pm = state.plugin_manager.read().await;
    Json(pm.external_plugin_metrics())
}

pub async fn get_health(State(state): State<Arc<AppState>>) -> Json<SystemHealth> {
    tracing::debug!("get_health called");
    let mut sys = sysinfo::System::new_all();
    sys.refresh_all();
    let disks = sysinfo::Disks::new_with_refreshed_list();

    let config = state.cfg.read().await;
    let dest_roots = config.organization.destination_roots.clone();
    drop(config);

    // quick_check rather than integrity_check: it runs ~1% of the checks but is
    // far faster on large databases, which matters for a frequently-polled health
    // endpoint. Full integrity_check is for offline maintenance.
    let db_integrity_status = match sqlx::query_scalar::<_, String>("PRAGMA quick_check;")
        .fetch_one(state.db.get_pool())
        .await
    {
        Ok(s) => {
            if s.to_lowercase() == "ok" {
                "Healthy".to_string()
            } else {
                format!("Error: {}", s)
            }
        }
        Err(e) => format!("Error: {}", e),
    };

    // All paths are normalized through the SSoT `normalize_path` so that
    // differently-spelled equivalents (e.g. /media/./TV vs /media/TV) dedup into
    // one entry, avoiding redundant I/O checks below.
    let mut paths_to_check: std::collections::HashSet<std::path::PathBuf> = dest_roots
        .into_iter()
        .map(|root| crate::validation::normalize_path(&root.path))
        .collect();
    if let Some(downloader_lock) = &state.downloader {
        let downloader = downloader_lock.read().await;
        // Downloader paths may be on a different filesystem than the destination
        // roots; a full drive there also fails downloads, so check it too.
        paths_to_check.extend(
            downloader
                .get_organizer_paths()
                .await
                .into_iter()
                .map(|p| crate::validation::normalize_path(&p)),
        );
    }
    // Series paths may live outside any configured destination root (custom
    // locations, or no roots configured at all), so their parent directories are
    // checked too. The HashSet dedups, keeping the I/O loop bounded even for many
    // series under few roots.
    if let Ok(mappings) = state.db.get_all_series_mappings().await {
        for mapping in mappings.values() {
            if let Some(path) = &mapping.settings.path
                && let Some(parent) = std::path::Path::new(path).parent()
                && !parent.as_os_str().is_empty()
            {
                paths_to_check.insert(crate::validation::normalize_path(parent));
            }
        }
    }

    // The set above is unordered, and its iteration order is not stable across
    // requests. Sort once into a Vec so both the storage and permission lists are
    // deterministic for clients that render them in the order received.
    let mut paths_to_check: Vec<std::path::PathBuf> = paths_to_check.into_iter().collect();
    paths_to_check.sort();

    let mut storage_health = Vec::new();
    for p in &paths_to_check {
        if let Some(disk) = disks.list().iter().find(|d| p.starts_with(d.mount_point())) {
            storage_health.push(StorageHealth {
                path: p.to_string_lossy().to_string(),
                free_bytes: disk.available_space(),
                total_bytes: disk.total_space(),
            });
        }
    }

    // Write a temp file rather than just stat(): a directory can be readable but
    // not writable, and access(2)/W_OK checks the process EUID, not actual
    // writeability (read-only mounts still pass access()).
    let mut dir_permissions = Vec::new();
    for p in &paths_to_check {
        let mut can_read = false;
        let mut can_write = false;

        if p.exists() {
            if std::fs::read_dir(p).is_ok() {
                can_read = true;
            }
            let test_file = p.join(".sys_health_check.tmp");
            if std::fs::write(&test_file, b"test").is_ok() {
                can_write = true;
                let _ = std::fs::remove_file(test_file);
            }
        }
        dir_permissions.push(PermissionCheck {
            path: p.to_string_lossy().to_string(),
            can_read,
            can_write,
        });
    }

    // Check ffprobe, not ffmpeg: ffprobe is the tool media-info extraction
    // actually invokes, so it is the one that must be on PATH.
    let ffmpeg_version = crate::utils::media_info::ffprobe_version();
    let ffmpeg_installed = ffmpeg_version.is_some();

    // "Disabled" means every client is down, not just one: a partial failure can
    // still fall back to remaining clients, but with all down the downloader is
    // unusable and the user should check credentials.
    let downloader_is_disabled = if let Some(dl) = &state.downloader {
        let lock = dl.read().await;
        let disabled_count =
            lock.permanently_disabled_clients.len() + lock.temporarily_disabled_clients.len();
        let total_count = lock.client_count().await;
        total_count > 0 && disabled_count >= total_count
    } else {
        false
    };

    Json(SystemHealth {
        status: "Healthy".to_string(),
        uptime_seconds: sysinfo::System::uptime(),
        memory_used: sys.used_memory(),
        memory_total: sys.total_memory(),
        cpu_usage: sys.global_cpu_usage(),
        db_integrity_status,
        storage_health,
        dir_permissions,
        ffmpeg_installed,
        ffmpeg_version,
        downloader_is_disabled,
    })
}

pub async fn ping() -> Json<serde_json::Value> {
    tracing::trace!("ping called");
    // Ping is a lightweight "is the server running?" check with zero I/O, so load
    // balancers and monitoring should hit /ping instead of the expensive /health.
    Json(serde_json::json!({ "status": "ok" }))
}

pub async fn remediate(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<jumbie_shared::types::RemediatePayload>,
) -> Result<StatusCode, AppError> {
    tracing::debug!("remediate called: action={}", payload.action);
    if payload.action == "cleanup_logs" {
        let logs_dir = std::path::Path::new(&state.logs_dir);
        // The SizeRoller culls by disk budget on rotation — this is just a
        // manual "free disk now" action. The current file is never deleted;
        // unrelated files in the logs directory are ignored by the scheme.
        if logs_dir.exists()
            && let Ok(entries) = std::fs::read_dir(logs_dir)
        {
            let mut archives = Vec::new();
            for entry in entries.flatten() {
                if let Some(name) = entry.file_name().to_str()
                    && crate::logging::is_log_file_name(name)
                    && name != crate::logging::CURRENT_FILE
                {
                    archives.push(entry.path());
                }
            }
            archives.sort();
            if archives.len() > crate::logging::MAX_ARCHIVES {
                for path in archives
                    .iter()
                    .take(archives.len() - crate::logging::MAX_ARCHIVES)
                {
                    let _ = std::fs::remove_file(path);
                }
            }
        }
        Ok(StatusCode::OK)
    } else {
        Err(AppError::BadRequest(
            "Unknown remediation action".to_string(),
        ))
    }
}

/// Read memory usage from /proc/self/status as a cross-platform fallback,
/// jemalloc's mallctl for the allocator breakdown, and the log subsystem's
/// footprint (byte-bounded buffer + size-rolled files on disk).
///
/// The `logs` category exists so a recurrence of the ~1.5 GiB log-buffer
/// incident is identifiable from the API in one call, instead of requiring
/// heap profiling: `buffer_bytes` must stay ≤ `buffer_budget_bytes`.
pub async fn get_memory_stats(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    // Read process-level RSS from /proc/self/status (always available on Linux).
    let rss_kb = std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines().find_map(|line| {
                line.strip_prefix("VmRSS:")
                    .and_then(|v| v.trim().strip_suffix(" kB"))
                    .and_then(|v| v.trim().parse::<u64>().ok())
            })
        })
        .unwrap_or(0);

    let peak_kb = std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines().find_map(|line| {
                line.strip_prefix("VmPeak:")
                    .and_then(|v| v.trim().strip_suffix(" kB"))
                    .and_then(|v| v.trim().parse::<u64>().ok())
            })
        })
        .unwrap_or(0);

    // Try jemalloc mallctl stats (compiled-in when jemalloc is the global allocator).
    let (allocated, active, mapped, resident) = crate::alloc::memory_stats();

    // Log subsystem footprint. SSoT: `LogRing::bytes` is the estimated
    // in-memory size of the parsed buffer; the disk side is the size-rolled
    // file set from `logging::log_files` (current + numeric archives).
    let (log_buffer_bytes, log_buffer_entries) = match state.log_buffer.read() {
        Ok(buf) => (buf.bytes(), buf.len()),
        Err(_) => (0, 0),
    };
    let log_disk_bytes: u64 = crate::logging::log_files(std::path::Path::new(&state.logs_dir))
        .iter()
        .filter_map(|p| std::fs::metadata(p).ok().map(|m| m.len()))
        .sum();

    Json(serde_json::json!({
        "rss_bytes": rss_kb * 1024,
        "peak_bytes": peak_kb * 1024,
        "jemalloc": {
            "allocated_bytes": allocated,
            "active_bytes": active,
            "mapped_bytes": mapped,
            "resident_bytes": resident,
            "overhead_bytes": mapped.saturating_sub(allocated),
            "decay_goal_ms": 5000,
        },
        "logs": {
            "buffer_bytes": log_buffer_bytes,
            "buffer_entries": log_buffer_entries,
            "buffer_budget_bytes": jumbie_shared::types::LOG_BUFFER_BYTES,
            "disk_bytes": log_disk_bytes,
        },
        "note": "jemalloc returns freed pages to the OS within ~5 seconds via background thread decay"
    }))
}

pub async fn get_about() -> Json<AboutInfo> {
    tracing::debug!("get_about called");
    // CARGO_PKG_VERSION is baked in at compile time (no runtime file read), and
    // JUMBIE_GIT_COMMIT / JUMBIE_BUILD_DATE are injected by build.rs.
    Json(AboutInfo {
        version: env!("CARGO_PKG_VERSION").to_string(),
        os: std::env::consts::OS.to_string(),
        git_commit: env!("JUMBIE_GIT_COMMIT").to_string(),
        build_date: env!("JUMBIE_BUILD_DATE").to_string(),
        license: env!("CARGO_PKG_LICENSE").to_string(),
        description: env!("CARGO_PKG_DESCRIPTION").to_string(),
    })
}
