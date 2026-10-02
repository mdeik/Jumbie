// IP-based sliding-window rate limiter. Configuration lives in SecurityConfig in
// the DB and is read at request time, so it can be hot-reloaded.
//
// Each IP keeps the count of the current and previous 60s window; the rate is
// estimated as previous (weighted by how much of it still overlaps the trailing
// window) + current, removing the "2x burst at a window boundary" artefact of a
// naive fixed window. A burst allowance (`rate_limit_burst`) lets short legitimate
// spikes through before the limiter engages.
//
// In-memory state is an `Arc<Mutex<HashMap<IpAddr, RateLimitEntry>>>` (every request
// does a read-modify-write, which rules out tokio's RwLock — same as `ban_list`).
//
// Sits between security_headers and auth_interceptor, so it also protects
// unauthenticated requests (e.g. the login page) from brute-force.

use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::Response,
};

// Fallback IP used when the resolved client IP is unavailable (e.g. a test
// harness that doesn't inject ConnectInfo). Production always resolves an IP via
// `client_ip::resolve_client_ip_middleware`.
const FALLBACK_IP: std::net::IpAddr = std::net::IpAddr::V4(std::net::Ipv4Addr::new(0, 0, 0, 0));
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

use crate::api::AppState;
use crate::middleware::client_ip::ClientIp;

/// Fixed window length for rate limiting.
const WINDOW_SECS: u64 = 60;
/// Hard cap on tracked IPs (backstop against a flood of distinct IPs).
pub const MAX_ENTRIES: usize = 100_000;

/// Per-IP rate-limit tracking entry.
pub struct RateLimitEntry {
    /// Requests counted in the current window.
    current: u32,
    /// Requests counted in the immediately preceding window. Weighted by how much
    /// of that window still falls inside the trailing 60s to smooth the estimate.
    previous: u32,
    /// Start of the current window.
    window_start: Instant,
}

/// Shared rate-limiting state map.
///
/// `Arc<Mutex<HashMap>>` (same rationale as `ban_list`): every request does a
/// read-modify-write, which tokio's RwLock cannot do without an upgrade.
pub type RateLimiter = Arc<Mutex<HashMap<IpAddr, RateLimitEntry>>>;

/// Create an empty rate limiter map. Called from create_router.
pub fn new_rate_limiter() -> RateLimiter {
    Arc::new(Mutex::new(HashMap::new()))
}

/// Path prefixes that bypass rate limiting entirely.
///
/// Public endpoints (theme, ping) are exempt because they're lightweight and are
/// needed for health checks and login-page rendering. iCal feeds have their own
/// token auth. The path set is shared with the auth middleware's bypass via
/// [`crate::middleware::is_public_path`] so they cannot drift.
fn is_exempt_path(path: &str) -> bool {
    crate::middleware::is_public_path(path)
}

/// Rate-limiting middleware — returns 429 if the client exceeds the configured limit.
pub async fn rate_limit_middleware(
    State(state): State<Arc<AppState>>,
    req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    // Phase 1: read config (scoped to drop the read lock quickly).
    let (enabled, per_minute, burst) = {
        let config = state.cfg.read().await;
        let s = &config.security;
        (
            s.rate_limit_enabled,
            s.rate_limit_per_minute,
            s.rate_limit_burst,
        )
    };

    if !enabled {
        return Ok(next.run(req).await);
    }

    // Exempt paths bypass the limiter.
    if is_exempt_path(req.uri().path()) {
        return Ok(next.run(req).await);
    }

    // Phase 2: resolve client IP. Resolved once per request by
    // `client_ip::resolve_client_ip_middleware` and shared with auth, so both key on
    // the same identity behind a trusted proxy. The sentinel fallback is only for
    // harnesses without ConnectInfo.
    let client_ip = req
        .extensions()
        .get::<ClientIp>()
        .and_then(|c| c.0)
        .unwrap_or(FALLBACK_IP);

    // Phase 3: check and update the rate-limit entry.
    {
        let mut limiter = state.rate_limiter.lock().await;
        let now = Instant::now();

        let entry = limiter.entry(client_ip).or_insert(RateLimitEntry {
            current: 0,
            previous: 0,
            window_start: now,
        });

        // Rotate the window(s) if time has passed.
        let elapsed = now.duration_since(entry.window_start);
        if elapsed >= Duration::from_secs(2 * WINDOW_SECS) {
            // Both windows are fully in the past — start clean.
            entry.previous = 0;
            entry.current = 0;
            entry.window_start = now;
        } else if elapsed >= Duration::from_secs(WINDOW_SECS) {
            // Advance exactly one window: the current count becomes the previous.
            entry.previous = entry.current;
            entry.current = 0;
            entry.window_start += Duration::from_secs(WINDOW_SECS);
        }

        // Sliding estimate: weight the previous window by how much of it still
        // falls inside the trailing WINDOW_SECS.
        let elapsed = now.duration_since(entry.window_start).as_secs_f64();
        let remaining_fraction = (1.0 - elapsed / WINDOW_SECS as f64).clamp(0.0, 1.0);
        let estimated = entry.current as f64 + entry.previous as f64 * remaining_fraction;

        let effective_limit = (per_minute + burst) as f64;
        if estimated >= effective_limit {
            tracing::warn!(
                "[SECURITY] RATE_LIMITED | IP: {} | Estimate: {:.1}/{} (burst {})",
                client_ip,
                estimated,
                per_minute,
                burst,
            );
            return Err(StatusCode::TOO_MANY_REQUESTS);
        }

        entry.current += 1;
    }

    Ok(next.run(req).await)
}

/// Evict entries whose windows have fully elapsed. Called by the state reaper to
/// bound the map's memory; exposed separately so it can be unit-tested. If the
/// elapsed-window eviction is not enough, the oldest entries beyond
/// [`MAX_ENTRIES`] are also dropped.
pub async fn reap(limiter: &RateLimiter, now: Instant) -> usize {
    let mut map = limiter.lock().await;
    let before = map.len();
    // An entry contributes to the estimate only while its two windows overlap the
    // trailing WINDOW_SECS, i.e. until two windows have elapsed since it started.
    map.retain(|_, e| now.duration_since(e.window_start) < Duration::from_secs(2 * WINDOW_SECS));

    if map.len() > MAX_ENTRIES {
        let mut entries: Vec<(IpAddr, Instant)> =
            map.iter().map(|(ip, e)| (*ip, e.window_start)).collect();
        entries.sort_by_key(|(_, window_start)| *window_start);
        let mut over = map.len() - MAX_ENTRIES;
        for (ip, _) in entries {
            if over == 0 {
                break;
            }
            map.remove(&ip);
            over -= 1;
        }
    }

    before - map.len()
}
