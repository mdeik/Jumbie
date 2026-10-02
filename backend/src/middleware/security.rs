// Security headers + CORS at the Axum middleware layer (not the reverse proxy), so
// the app is portable across proxies, headers are runtime-configurable, and Host
// validation prevents DNS rebinding even when exposed directly. Runs on every
// request; CORS preflights are handled early (before auth) so browsers get a 204,
// not a 401.

use axum::{
    extract::{Request, State},
    http::{HeaderName, HeaderValue, Method, StatusCode, header},
    middleware::Next,
    response::Response,
};
use std::str::FromStr;
use std::sync::Arc;

use crate::api::AppState;

pub async fn security_headers(
    State(state): State<Arc<AppState>>,
    req: Request,
    next: Next,
) -> Response {
    let (
        host_validation,
        host_allowed,
        clickjacking,
        csrf,
        use_custom,
        custom_headers,
        restrict_cors,
        allowed_origins,
    ) = {
        // Scoped read block: release the RwLock read guard before awaiting
        // `next.run()`. Holding it across an await would block the config write lock
        // (hot-reload) until the request completes — a multi-second stall for uploads.
        let config = state.cfg.read().await;
        let s = &config.security;
        (
            s.host_header_validation,
            s.allowed_domains.clone(),
            s.clickjacking_protection,
            s.csrf_protection,
            s.use_custom_headers,
            s.custom_headers.clone(),
            s.restrict_cors,
            s.allowed_origins.clone(),
        )
    };

    // CORS preflight: handle before auth so browsers don't get 401 for OPTIONS
    let is_preflight = req.method() == Method::OPTIONS;
    let request_origin = req
        .headers()
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    // Host header validation, before passing to the next handler. Supports "*"
    // (any host), "*.example.com" (wildcard subdomain), and "example.com" (exact,
    // case-insensitive). The port is stripped first because DNS rebinding works
    // regardless of port.
    if host_validation
        && !host_allowed.is_empty()
        && let Some(host_hv) = req.headers().get(header::HOST)
        && let Ok(host_str) = host_hv.to_str()
    {
        let host_bare = host_str.split(':').next().unwrap_or(host_str);
        let allowed = host_allowed.iter().any(|pattern| {
            let pattern = pattern.trim();
            if pattern == "*" {
                true
            } else if let Some(stripped) = pattern.strip_prefix('*') {
                host_bare.ends_with(stripped)
            } else {
                host_bare.eq_ignore_ascii_case(pattern)
            }
        });
        if !allowed {
            return axum::response::Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .body(axum::body::Body::from("Invalid Host header"))
                .unwrap();
        }
    }

    // For CORS preflight, respond immediately with 204 + CORS headers
    if is_preflight {
        let mut builder = axum::response::Response::builder().status(StatusCode::NO_CONTENT);
        builder = apply_cors_headers(builder, &request_origin, restrict_cors, &allowed_origins);
        return builder.body(axum::body::Body::empty()).unwrap();
    }

    let mut response = next.run(req).await;
    let headers = response.headers_mut();

    // Clickjacking protection
    if clickjacking {
        headers.insert(
            header::X_FRAME_OPTIONS,
            HeaderValue::from_static("SAMEORIGIN"),
        );
    }

    // CSRF-related headers
    if csrf {
        headers.insert(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        );
        headers.insert(
            HeaderName::from_static("referrer-policy"),
            HeaderValue::from_static("strict-origin-when-cross-origin"),
        );
        headers.insert(
            HeaderName::from_static("x-permitted-cross-domain-policies"),
            HeaderValue::from_static("none"),
        );
    }

    // CORS response headers
    apply_cors_headers_to_map(headers, &request_origin, restrict_cors, &allowed_origins);

    // Custom HTTP headers
    if use_custom {
        for line in &custom_headers {
            if let Some((name, value)) = line.split_once(':') {
                let name = name.trim();
                let value = value.trim();
                if let (Ok(hn), Ok(hv)) = (HeaderName::from_str(name), HeaderValue::from_str(value))
                {
                    headers.insert(hn, hv);
                }
            }
        }
    }

    response
}

// CORS origin resolution: three modes, controlled by `restrict_cors` and
// `allowed_origins`:
//
//   restrict=false           → echo the request Origin back (permissive), plus
//                              Allow-Credentials: true which the `*` form forbids.
//   restrict=true, empty list → deny all cross-origin (no ACAO header).
//   restrict=true, populated  → echo the Origin only if it is in the allowlist.

fn cors_origin_value(
    request_origin: &Option<String>,
    restrict_cors: bool,
    allowed_origins: &[String],
) -> Option<HeaderValue> {
    if !restrict_cors {
        // Permissive: echo origin back (same as CorsLayer::permissive)
        request_origin
            .as_ref()
            .and_then(|o| HeaderValue::from_str(o).ok())
            .or_else(|| Some(HeaderValue::from_static("*")))
    } else if allowed_origins.is_empty() {
        // Restricted with no list: deny all cross-origin
        None
    } else {
        // Restricted: only echo the origin if it's in the allowlist
        request_origin.as_ref().and_then(|origin| {
            if allowed_origins.iter().any(|a| a.trim() == origin) {
                HeaderValue::from_str(origin).ok()
            } else {
                None
            }
        })
    }
}

// Split into builder-based (preflight responses are constructed from scratch) and
// map-based (regular responses already have a HeaderMap) variants; both apply the
// same CORS headers so behavior is consistent.

fn apply_cors_headers(
    mut builder: axum::http::response::Builder,
    request_origin: &Option<String>,
    restrict_cors: bool,
    allowed_origins: &[String],
) -> axum::http::response::Builder {
    if let Some(origin) = cors_origin_value(request_origin, restrict_cors, allowed_origins) {
        builder = builder.header(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
    }
    builder = builder
        .header(
            header::ACCESS_CONTROL_ALLOW_METHODS,
            "GET, POST, PUT, DELETE, OPTIONS",
        )
        .header(
            header::ACCESS_CONTROL_ALLOW_HEADERS,
            "Content-Type, Authorization",
        )
        .header(header::ACCESS_CONTROL_MAX_AGE, "3600");
    if !restrict_cors {
        builder = builder.header(header::ACCESS_CONTROL_ALLOW_CREDENTIALS, "true");
    }
    builder
}

fn apply_cors_headers_to_map(
    headers: &mut axum::http::HeaderMap,
    request_origin: &Option<String>,
    restrict_cors: bool,
    allowed_origins: &[String],
) {
    if let Some(origin) = cors_origin_value(request_origin, restrict_cors, allowed_origins) {
        headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
    }
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static("GET, POST, PUT, DELETE, OPTIONS"),
    );
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_HEADERS,
        HeaderValue::from_static("Content-Type, Authorization"),
    );
    if !restrict_cors {
        headers.insert(
            header::ACCESS_CONTROL_ALLOW_CREDENTIALS,
            HeaderValue::from_static("true"),
        );
    }
}
