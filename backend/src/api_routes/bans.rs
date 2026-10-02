use crate::error::AppError;
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use chrono::Utc;
use jumbie_shared::config::BannedIp;

use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::api::{AppState, BanInfo};

/// GET /api/auth/bans
///
/// Returns the current in-memory ban list merged with persisted bans.
///
/// Dual storage (in-memory + DB): auth checks run on every request, so bans live
/// in an in-memory HashMap for O(1) lookup while the DB is the durable source of
/// truth that survives restarts. In-memory bans use `Instant` for cheap expiry
/// checks; the remaining duration is projected onto the wall clock for display
/// (may be off by seconds if the system clock moved).
pub async fn list_bans(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<jumbie_shared::types::BanEntry>>, AppError> {
    tracing::debug!("list_bans called");
    let persisted = state.db.get_banned_ips().await.unwrap_or_default();
    let ban_list = state.ban_list.lock().await;

    let mut entries: Vec<jumbie_shared::types::BanEntry> = persisted
        .iter()
        .map(|b| {
            let is_permanent = b.banned_until.is_none();
            jumbie_shared::types::BanEntry {
                ip: b.ip.clone(),
                fail_count: b.fail_count,
                ban_count: b.ban_count,
                banned_at: b.banned_at.clone(),
                banned_until: b.banned_until.clone(),
                is_permanent,
            }
        })
        .collect();

    // Merge runtime-only bans not yet persisted to DB: an API-added ban lands in
    // memory immediately but the DB write is async, so listing right after adding
    // would otherwise show stale data.
    for (ip, info) in ban_list.iter() {
        let ip_str = ip.to_string();
        if persisted.iter().any(|b| b.ip == ip_str) || !info.is_banned(Instant::now()) {
            continue;
        }
        entries.push(jumbie_shared::types::BanEntry {
            ip: ip_str,
            fail_count: info.fail_count,
            ban_count: info.ban_count,
            banned_at: info
                .banned_at
                .map(|t| crate::datetime::UtcDateTime::from_chrono_utc(t).to_rfc3339_utc())
                .unwrap_or_else(|| crate::datetime::UtcDateTime::now().to_rfc3339_utc()),
            // In-memory `banned_until` is an Instant (monotonic); project the
            // remaining seconds forward from the wall clock for a stable display
            // timestamp.
            banned_until: info.banned_until.map(|_| {
                let remaining = info
                    .banned_until
                    .and_then(|t| t.checked_duration_since(Instant::now()))
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                Utc::now()
                    .checked_add_signed(chrono::Duration::seconds(remaining))
                    .map(|t| crate::datetime::UtcDateTime::from_chrono_utc(t).to_rfc3339_utc())
                    .unwrap_or_default()
            }),
            is_permanent: info.is_permanent(),
        });
    }

    tracing::debug!("list_bans completed: {} entries", entries.len());
    Ok(Json(entries))
}

/// POST /api/auth/bans
///
/// Manually add a ban (admin action).
///
/// In-memory is updated before the DB so the ban is effective immediately for
/// subsequent requests; the DB write makes it survive a restart.
pub async fn add_ban(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<jumbie_shared::types::AddBanPayload>,
) -> Result<StatusCode, AppError> {
    let ip: std::net::IpAddr = payload
        .ip
        .trim()
        .parse()
        .map_err(|_| AppError::BadRequest(format!("Invalid IP address: {}", payload.ip)))?;

    // Reject unreasonably long durations (> 10 years = 31536000 seconds)
    if let Some(s) = payload.duration_seconds
        && s > 31_536_000
    {
        return Err(AppError::BadRequest(
            "Ban duration cannot exceed 10 years".to_string(),
        ));
    }

    let now = Instant::now();
    let banned_until_instant = payload
        .duration_seconds
        .map(|s| now + Duration::from_secs(s));
    let banned_until_iso = payload.duration_seconds.and_then(|s| {
        let secs = i64::try_from(s).ok()?;
        Utc::now()
            .checked_add_signed(chrono::Duration::seconds(secs))
            .map(|t| crate::datetime::UtcDateTime::from_chrono_utc(t).to_rfc3339_utc())
    });

    {
        let mut ban_list = state.ban_list.lock().await;
        let entry = ban_list
            .entry(ip)
            .or_insert_with(|| BanInfo::tracked(Instant::now()));
        entry.banned_until = banned_until_instant;
        entry.banned_at = Some(Utc::now());
        entry.last_seen = Instant::now();
        entry.ban_count += 1;
    }

    // The in-memory list is volatile, so the DB write is required for the ban to
    // survive a restart.
    state
        .db
        .insert_banned_ip(&BannedIp {
            ip: ip.to_string(),
            fail_count: 0,
            ban_count: 1,
            banned_at: crate::datetime::UtcDateTime::now().to_rfc3339_utc(),
            banned_until: banned_until_iso,
        })
        .await?;

    tracing::info!("Manually banned IP: {}", payload.ip);
    Ok(StatusCode::CREATED)
}

/// DELETE /api/auth/bans/:ip
///
/// Unban an IP address. Removed from memory first (same reasoning as `add_ban`)
/// so the unban takes effect immediately.
pub async fn remove_ban(
    State(state): State<Arc<AppState>>,
    Path(ip_str): Path<String>,
) -> Result<StatusCode, AppError> {
    let ip: std::net::IpAddr = ip_str
        .trim()
        .parse()
        .map_err(|_| AppError::BadRequest(format!("Invalid IP address: {}", ip_str)))?;

    {
        let mut ban_list = state.ban_list.lock().await;
        ban_list.remove(&ip);
    }

    state.db.delete_banned_ip(&ip.to_string()).await?;

    tracing::info!("Unbanned IP: {}", ip_str);
    Ok(StatusCode::NO_CONTENT)
}
