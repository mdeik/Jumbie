// Authentication middleware — the gatekeeper for all API requests. Determines whether
// the client is banned, has valid credentials (Basic auth or Bearer API key), and
// what scopes those grant, then injects the scopes into `req.extensions` for scope.rs
// and route handlers.
//
// Checks run cheapest-first: localhost bypass → subnet whitelist bypass →
// calendar/public path bypass → auth-disabled check (one DB read) → in-memory ban
// list → credential validation (the slow DB + hashing path).
//
// The client IP is resolved once by `client_ip::resolve_client_ip_middleware` and
// shared via extensions, so the ban list, fail-counter reset, and rate limiter all
// key on the SAME identity.

use axum::{
    Json,
    body::Body,
    extract::{Request, State},
    http::{StatusCode, header, header::AUTHORIZATION},
    middleware::Next,
    response::{IntoResponse, Response},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use chrono::Utc;
use jumbie_shared::auth::ApiScope;
use jumbie_shared::types::ApiErrorEnvelope;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::api::{AppState, BanInfo};
use crate::middleware::client_ip::ClientIp;

enum AuthOutcome {
    Allow,
    Deny,
    MissingCredentials,
}

impl AuthOutcome {
    fn into_response(self) -> Response {
        match self {
            // A 401 must carry a challenge so clients know how to authenticate.
            AuthOutcome::Deny | AuthOutcome::MissingCredentials => Response::builder()
                .status(StatusCode::UNAUTHORIZED)
                .header(header::WWW_AUTHENTICATE, "Basic realm=\"jumbie\"")
                .body(Body::empty())
                .unwrap(),
            AuthOutcome::Allow => unreachable!(),
        }
    }
}

/// 403 for a banned client. Built directly rather than via `AuthOutcome`
/// because the ban check short-circuits inside `auth_interceptor`
/// before an outcome is computed.
fn banned_response() -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(ApiErrorEnvelope {
            error: "IP is banned".to_string(),
            ..Default::default()
        }),
    )
        .into_response()
}

pub async fn auth_interceptor(
    State(state): State<Arc<AppState>>,
    mut req: Request,
    next: Next,
) -> Response {
    // Resolved once by `client_ip::resolve_client_ip_middleware` and shared via
    // extensions, so every IP-keyed decision here keys on the same identity the rate
    // limiter uses.
    let client_ip = req.extensions().get::<ClientIp>().and_then(|c| c.0);

    // Phase 1: credential/ban decision. Config reads are scoped so the RwLock read
    // guard is dropped before the DB awaits / `next.run()` — holding it across an
    // await would stall a concurrent config hot-reload.
    let outcome: AuthOutcome = {
        // Snapshot the bypass flags, then release the guard before any I/O.
        let (bypass_local, bypass_subnet, subnet_whitelist) = {
            let config = state.cfg.read().await;
            (
                config.auth.bypass_local_auth,
                config.auth.bypass_subnet_whitelist,
                config.auth.subnet_whitelist.clone(),
            )
        };

        // Check 1: localhost (loopback) bypass, for local-first workflows.
        if bypass_local
            && let Some(ip) = client_ip
            && ip.is_loopback()
        {
            req.extensions_mut().insert(ApiScope::all().to_vec());
            return next.run(req).await;
        }

        // Check 2: subnet whitelist bypass (CIDR, e.g. "10.0.0.0/8" for a VPN or
        // Docker bridge network).
        if bypass_subnet && let Some(ip) = client_ip {
            for cidr in &subnet_whitelist {
                if let Ok(network) = cidr.trim().parse::<ipnet::IpNet>()
                    && network.contains(&ip)
                {
                    req.extensions_mut().insert(ApiScope::all().to_vec());
                    return next.run(req).await;
                }
            }
        }

        // Check 3: calendar/public path bypass. Calendar tokens are embedded in the
        // URL rather than a header; the path set is shared with the rate limiter.
        if super::is_public_path(req.uri().path()) {
            return next.run(req).await;
        }

        // Check 4: fetch the admin password hash. An empty result means auth is
        // intentionally disabled (fresh install); a query FAILURE must fail closed —
        // treating it as "no password" would open all scopes.
        let db_password_hash = match state.auth_cache.password_hash(&state.db).await {
            Ok(hash) => hash,
            Err(e) => {
                tracing::error!("[SECURITY] Failed to read admin password hash: {}", e);
                return AuthOutcome::Deny.into_response();
            }
        };

        // Fresh install: open all scopes so the setup wizard can run unauthenticated.
        if db_password_hash.is_none() || db_password_hash.as_deref().unwrap_or("").is_empty() {
            req.extensions_mut().insert(ApiScope::all().to_vec());
            AuthOutcome::Allow
        } else {
            // Check 5: in-memory ban list, before credential validation so banned
            // clients cost no crypto work.
            if let Some(ip) = client_ip {
                let ban_list = state.ban_list.lock().await;
                if let Some(info) = ban_list.get(&ip) {
                    let now = Instant::now();
                    if info.is_banned(now) {
                        tracing::warn!(
                            "[SECURITY] BANNED_IP_ACCESS | IP: {} | Held for {} more seconds",
                            ip,
                            info.banned_until
                                .map(|t| t.saturating_duration_since(Instant::now()).as_secs())
                                .unwrap_or(u64::MAX),
                        );
                        return banned_response();
                    }
                }
            }

            // Check 6: credential validation. Two schemes: Basic auth (the admin
            // password, all scopes, for interactive UI) and Bearer (`jb_…` API key
            // with key-creation-time scopes and optional expiry). Bearer is checked
            // first as it is more common for programmatic access.
            if let Some(hv) = req.headers().get(AUTHORIZATION) {
                let mut is_authorized = false;
                let mut scopes = Vec::new();

                if let Ok(hs) = hv.to_str() {
                    if let Some(stripped) = hs.strip_prefix("Basic ") {
                        if let Some(stored_hash) = db_password_hash.as_deref()
                            && let Ok(decoded) = STANDARD.decode(stripped)
                            && let Ok(decoded_str) = String::from_utf8(decoded)
                        {
                            let parts: Vec<&str> = decoded_str.splitn(2, ':').collect();
                            let request_password =
                                if parts.len() == 2 { parts[1] } else { parts[0] };
                            if state
                                .auth_cache
                                .verify_password(request_password, stored_hash)
                            {
                                is_authorized = true;
                                scopes = ApiScope::all().to_vec();
                            }
                        }
                    } else if let Some(token) = hs.strip_prefix("Bearer ") {
                        let token_hash = crate::auth_utils::hash_api_key(token);
                        if let Ok(Some(key)) =
                            state.auth_cache.api_key(&state.db, &token_hash).await
                        {
                            let mut valid = true;
                            if let Some(expires_at) = &key.expires_at
                                && let Ok(expiry_dt) = crate::datetime::parse_utc(expires_at)
                                && Utc::now() > expiry_dt.to_chrono_utc()
                            {
                                valid = false;
                                tracing::warn!(
                                    "[SECURITY] API Key {} expired at {} | IP: {}",
                                    key.name,
                                    expires_at,
                                    client_ip
                                        .map(|ip| ip.to_string())
                                        .unwrap_or_else(|| "Unknown".to_string())
                                );
                            }
                            if valid {
                                is_authorized = true;
                                scopes = key.scopes.clone();
                            }
                        }
                    }
                }

                if is_authorized {
                    req.extensions_mut().insert(scopes);
                    AuthOutcome::Allow
                } else {
                    AuthOutcome::Deny
                }
            } else {
                AuthOutcome::MissingCredentials
            }
        }
    };

    // Phase 2: process the outcome.
    match outcome {
        AuthOutcome::Allow => {
            // Reset the fail counter but keep the BanInfo entry so `ban_count`
            // persists and bans escalate (each longer than the last). `client_ip` is
            // the shared resolved identity, so we never reset the wrong (proxy) IP.
            let max_fails = state.cfg.read().await.auth.max_auth_fail_count;

            if max_fails > 0
                && let Some(ip) = client_ip
            {
                let mut ban_list = state.ban_list.lock().await;
                if let Some(entry) = ban_list.get_mut(&ip) {
                    entry.last_seen = Instant::now();
                    if entry.fail_count > 0 {
                        tracing::info!(
                            "[SECURITY] AUTH_SUCCESS | IP: {} | Cleared {} failed auth attempts",
                            ip,
                            entry.fail_count
                        );
                        entry.fail_count = 0;
                    }
                }
            }
            next.run(req).await
        }
        AuthOutcome::Deny => {
            // Escalating ban logic: each failure increments the counter; hitting
            // max_auth_fail_count bans the IP. With increment enabled the duration is
            // base * factor^(ban_count-1), capped at ban_increment_max_seconds and
            // reset to 0 after ban_count_reset_days clean days. The ban is persisted
            // synchronously so it survives a restart.
            let (max_fails, base_ban_secs, increment_enabled, factor, max_ban_secs, reset_days) = {
                let config = state.cfg.read().await;
                (
                    config.auth.max_auth_fail_count,
                    config.auth.ban_duration_seconds,
                    config.auth.ban_increment_enabled,
                    config.auth.ban_increment_factor,
                    config.auth.ban_increment_max_seconds,
                    config.auth.ban_count_reset_days,
                )
            };

            // Capture the current fail count for the AUTH_FAILURE log; read it before
            // the ban_list lock is dropped.
            let auth_fail_attempt: Option<u32> = if max_fails > 0 {
                if let Some(ip) = client_ip {
                    let mut ban_list = state.ban_list.lock().await;
                    let now = Instant::now();
                    let entry = ban_list.entry(ip).or_insert_with(|| BanInfo::tracked(now));
                    entry.fail_count += 1;
                    entry.last_seen = now;
                    let current_fail = entry.fail_count;

                    // `client_ip` is the shared resolved identity, so counts target the
                    // real client even behind a proxy.
                    //
                    // Auto-banning must gate on "not currently banned" (sentinel or
                    // expired ban), not on `banned_until.is_none()` — tracked-but-
                    // not-banned entries use the sentinel `Some(now)`, and actively or
                    // permanently banned IPs were already rejected by Check 5.
                    if entry.fail_count >= max_fails && !entry.is_banned(now) {
                        let mut ban_secs = base_ban_secs;

                        if increment_enabled {
                            // Forgiveness window: start over from count 1 after `reset_days`.
                            if let Some(at) = entry.banned_at
                                && Utc::now().signed_duration_since(at).num_days()
                                    >= reset_days as i64
                            {
                                entry.ban_count = 0;
                            }

                            entry.ban_count += 1;

                            if entry.ban_count > 1 {
                                let multiplier = factor.powf((entry.ban_count - 1) as f32);
                                ban_secs = (base_ban_secs as f32 * multiplier) as u64;
                                if ban_secs > max_ban_secs {
                                    ban_secs = max_ban_secs;
                                }
                            }
                        } else {
                            entry.ban_count = 1;
                        }

                        let until = Instant::now() + Duration::from_secs(ban_secs);
                        entry.banned_until = Some(until);
                        entry.banned_at = Some(Utc::now());

                        let until_iso = Utc::now()
                            .checked_add_signed(chrono::Duration::seconds(ban_secs as i64))
                            .map(|t| {
                                crate::datetime::UtcDateTime::from_chrono_utc(t).to_rfc3339_utc()
                            })
                            .unwrap_or_default();

                        tracing::warn!(
                            "[SECURITY] BAN_ADDED | IP: {} | Duration: {}s | Ban Count: {} | \
                             Reason: {} failed auth attempts",
                            ip,
                            ban_secs,
                            entry.ban_count,
                            max_fails
                        );

                        // Persist the ban synchronously so it survives a crash/restart;
                        // the in-memory ban already applies while this runs.
                        let ip_str = ip.to_string();
                        let fail_count = entry.fail_count;
                        let ban_count = entry.ban_count;
                        let banned_at = crate::datetime::UtcDateTime::now().to_rfc3339_utc();
                        drop(ban_list);
                        if let Err(e) = state
                            .db
                            .insert_banned_ip(&jumbie_shared::config::BannedIp {
                                ip: ip_str,
                                fail_count,
                                ban_count,
                                banned_at,
                                banned_until: Some(until_iso),
                            })
                            .await
                        {
                            tracing::error!("Failed to persist ban to DB: {}", e);
                        }
                    }
                    Some(current_fail)
                } else {
                    None
                }
            } else {
                None
            };

            tracing::warn!(
                "[SECURITY] AUTH_FAILURE | Type: Basic/Bearer | \
                 Reason: Invalid Credentials | IP: {} | Attempt: {}/{}",
                client_ip
                    .map(|ip| ip.to_string())
                    .unwrap_or_else(|| "Unknown".to_string()),
                auth_fail_attempt.unwrap_or(0),
                max_fails
            );

            AuthOutcome::Deny.into_response()
        }
        AuthOutcome::MissingCredentials => {
            // No Authorization header: 401 without incrementing the fail counter, so
            // clients that simply haven't logged in yet aren't banned.
            AuthOutcome::MissingCredentials.into_response()
        }
    }
}
