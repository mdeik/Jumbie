// Client IP resolution: SSoT for "what is the client's IP?". Every IP-keyed
// protection (ban list, failure-counter reset, rate limiting) must agree, or bans
// and rate-limit buckets land on different identities behind a proxy.
//
// `resolve_client_ip_middleware` runs once (outermost) and stashes the resolved IP
// in request extensions; downstream middleware reads it instead of re-deriving it.
//
// XFF is trusted only when the TCP peer is a configured trusted proxy — otherwise an
// attacker could spoof XFF to bypass a subnet whitelist or impersonate localhost.
// Keep `trusted_proxies` narrow: trust is CIDR-based, so any peer inside a trusted
// range is treated as a proxy, and only XFF entries outside those ranges are used.

use axum::{
    extract::{ConnectInfo, Request, State},
    middleware::Next,
    response::Response,
};
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use crate::api::AppState;

/// The resolved client IP, inserted into request extensions by
/// [`resolve_client_ip_middleware`].
///
/// `None` means the IP could not be determined — e.g. no `ConnectInfo` was
/// injected (as in some test harnesses) and no trusted XFF was available.
#[derive(Clone, Copy, Debug)]
pub struct ClientIp(pub Option<IpAddr>);

/// Middleware: resolve the client IP once and store it in request extensions.
pub async fn resolve_client_ip_middleware(
    State(state): State<Arc<AppState>>,
    mut req: Request,
    next: Next,
) -> Response {
    let connect_ip = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ci| ci.0.ip());

    // Hold the config read guard only for the synchronous resolution; it must be
    // released before `next.run` so a concurrent config hot-reload can write.
    let client_ip = {
        let config = state.cfg.read().await;
        resolve_client_ip(&req, connect_ip, &config.security.trusted_proxies)
    };

    req.extensions_mut().insert(ClientIp(client_ip));
    next.run(req).await
}

/// Resolve the real client IP, guarding against X-Forwarded-For spoofing.
///
/// Two-tier trust model:
///   • If `trusted_proxies` is non-empty: only trust XFF when the direct
///     connection IP is in that list. This is the "production" mode where the
///     operator explicitly declares which proxies are theirs (e.g., their
///     nginx container's subnet).
///   • If `trusted_proxies` is empty: only trust XFF from loopback addresses.
///     This is the "dev" mode — it assumes any reverse proxy is running on
///     the same machine as the app.
///
/// When XFF is NOT trusted, `connect_ip` is returned directly, so the app sees
/// the true origin IP for rate-limiting and ban purposes even behind a proxy.
///
/// When XFF IS trusted, the chain is walked from the RIGHT and the first entry
/// that is not itself a trusted proxy is returned (see the inline rationale).
pub fn resolve_client_ip(
    req: &Request,
    connect_ip: Option<IpAddr>,
    trusted_proxies: &[String],
) -> Option<IpAddr> {
    if !is_trusted_proxy(connect_ip, trusted_proxies) {
        return connect_ip;
    }

    // The peer is a trusted proxy, so X-Forwarded-For is trustworthy. Walk the
    // chain from the RIGHT and return the first entry that is not itself a trusted
    // proxy.
    //
    // Rightmost (not leftmost): if the proxy APPENDS to XFF (nginx's
    // `$proxy_add_x_forwarded_for`), a client can inject a value at the LEFT end.
    // Choosing leftmost would trust attacker-controlled input — enough to spoof
    // localhost (bypassing `bypass_local_auth`), dodge rate limits, or frame another
    // IP for a ban. The rightmost value is what the edge proxy appended.
    if let Some(xff) = req.headers().get("x-forwarded-for")
        && let Ok(val) = xff.to_str()
    {
        let parsed: Vec<IpAddr> = val
            .split(',')
            .filter_map(|s| s.trim().parse::<IpAddr>().ok())
            .collect();
        for ip in parsed.iter().rev() {
            if !is_trusted_proxy(Some(*ip), trusted_proxies) {
                return Some(*ip);
            }
        }
    }

    connect_ip
}

/// Whether `ip` is a configured/trusted reverse proxy.
///
/// With an empty `trusted_proxies` list, only loopback peers are trusted (the
/// "dev" mode where the proxy runs on the same host).
fn is_trusted_proxy(ip: Option<IpAddr>, trusted_proxies: &[String]) -> bool {
    let Some(ip) = ip else {
        return false;
    };
    if trusted_proxies.is_empty() {
        return ip.is_loopback();
    }
    trusted_proxies.iter().any(|entry| {
        entry
            .trim()
            .parse::<ipnet::IpNet>()
            .map(|net| net.contains(&ip))
            .unwrap_or_else(|_| {
                // If the entry isn't a valid CIDR, treat it as a plain IP.
                // This allows entries like "192.168.1.1" that match a single
                // proxy rather than an entire subnet.
                entry
                    .trim()
                    .parse::<IpAddr>()
                    .map(|a| a == ip)
                    .unwrap_or(false)
            })
    })
}
