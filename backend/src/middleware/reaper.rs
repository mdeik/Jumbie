// Bounds the memory held by the IP-keyed maps (`ban_list` and the rate limiter) and
// prunes long-expired rows from the `banned_ips` table.
//
// Without eviction, a client with many IPs (or a spoofed XFF behind a misconfigured
// proxy) could grow these maps without bound. Two defences: time-based retention
// (keeps active/permanent bans and recently-seen entries, drops elapsed rate-limit
// windows) plus a hard entry cap that evicts oldest non-active entries first. The DB
// prune is a write, so it runs less often than the in-memory eviction
// (`PRUNE_EVERY_TICKS`).

use std::net::IpAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::api::AppState;

/// How often the in-memory reaper runs.
const REAP_INTERVAL: Duration = Duration::from_secs(60);
/// The DB prune is a write; run it every Nth pass (~10 minutes) rather than every
/// minute, since expired rows accumulate slowly.
const PRUNE_EVERY_TICKS: u64 = 10;
/// Hard cap on ban entries (backstop against a flood of distinct IPs).
const MAX_BAN_ENTRIES: usize = 50_000;

/// Spawn the periodic reaper. Call once during router construction.
pub fn start(state: Arc<AppState>) {
    tokio::spawn(async move {
        // Sleep first so the first pass is delayed by REAP_INTERVAL; a bare
        // `interval` fires immediately, issuing a DB write during startup.
        let mut tick: u64 = 0;
        loop {
            tokio::time::sleep(REAP_INTERVAL).await;
            tick += 1;
            reap_once_inner(&state, tick.is_multiple_of(PRUNE_EVERY_TICKS)).await;
        }
    });
}

/// Run one full reaper pass (in-memory eviction + DB prune). Exposed for tests.
pub async fn reap_once(state: &AppState) -> (usize, usize) {
    reap_once_inner(state, true).await
}

async fn reap_once_inner(state: &AppState, prune: bool) -> (usize, usize) {
    let reset_days = state.cfg.read().await.auth.ban_count_reset_days;
    let now = Instant::now();

    // Minimum 1 day so an entry is never evicted between two quick attempts.
    let retention = Duration::from_secs((reset_days as u64).max(1) * 86_400);

    let bans_evicted = {
        let mut bans = state.ban_list.lock().await;
        let before = bans.len();

        bans.retain(|_, info| {
            // Active temporary bans and permanent bans (None) are always kept;
            // everything else is kept only within the escalation window.
            info.is_banned(now) || now.duration_since(info.last_seen) < retention
        });

        // Hard-cap backstop: evict the oldest entries, preferring non-active ones.
        if bans.len() > MAX_BAN_ENTRIES {
            let mut entries: Vec<(IpAddr, bool, Instant)> = bans
                .iter()
                .map(|(ip, info)| (*ip, info.is_banned(now), info.last_seen))
                .collect();
            // Non-active first (false < true), then oldest `last_seen` first.
            entries.sort_by(|a, b| a.1.cmp(&b.1).then(a.2.cmp(&b.2)));
            let mut over = bans.len() - MAX_BAN_ENTRIES;
            for (ip, _, _) in entries {
                if over == 0 {
                    break;
                }
                bans.remove(&ip);
                over -= 1;
            }
        }

        before - bans.len()
    };

    let rate_evicted = crate::middleware::rate_limit::reap(&state.rate_limiter, now).await;

    let pruned = if prune {
        prune_db(state, retention).await
    } else {
        0
    };

    if bans_evicted > 0 || rate_evicted > 0 || pruned > 0 {
        tracing::debug!(
            "auth state reaper: evicted {} ban entries, {} rate-limit windows, pruned {} DB rows",
            bans_evicted,
            rate_evicted,
            pruned
        );
    }

    (bans_evicted, rate_evicted)
}

/// Delete temporary bans that expired before the escalation window, plus expired
/// download rejections and autoresolve attempt rows.
async fn prune_db(state: &AppState, retention: Duration) -> u64 {
    let cutoff = chrono::Utc::now() - chrono::Duration::seconds(retention.as_secs() as i64);
    let pruned = match state.db.prune_expired_bans(&cutoff.to_rfc3339()).await {
        Ok(n) => n,
        Err(e) => {
            tracing::warn!("Failed to prune expired bans: {}", e);
            0
        }
    };
    if let Err(e) = state.db.prune_expired_rejections().await {
        tracing::warn!("Failed to prune expired download rejections: {}", e);
    }
    if let Err(e) = state
        .db
        .prune_expired_autoresolve_attempts(
            crate::organizer::AUTORESOLVE_BUDGET_HOURS,
            crate::organizer::AUTORESOLVE_HIT_COOLDOWN_HOURS,
        )
        .await
    {
        tracing::warn!("Failed to prune expired autoresolve attempts: {}", e);
    }
    pruned
}
