// Middleware layer architecture (outermost → innermost):
//
//   client_ip — resolves the client IP once (honoring trusted proxies) and shares
//               it via request extensions for all IP-keyed protections.
//   auth      — credential validation, ban detection, localhost/subnet bypass;
//               injects ApiScopes into request extensions.
//   security  — CORS, CSP, clickjacking, Host header validation. Runs before auth
//               on preflight OPTIONS (browsers need CORS even for 401 responses).
//   scope::*  — per-route check that the caller's scopes include the endpoint's.

pub mod auth;
pub mod auth_cache;
pub mod client_ip;
pub mod rate_limit;
pub mod reaper;
pub mod scope;
pub mod security;

/// Paths that are public: no authentication is required and they are exempt from
/// rate limiting.
///
/// SSoT: both the auth middleware (auto-bypass) and the rate limiter (exemption)
/// call this, so the two sets of paths cannot drift apart.
pub fn is_public_path(path: &str) -> bool {
    path.starts_with("/api/public/") || path.starts_with("/api/calendar/ical")
}
