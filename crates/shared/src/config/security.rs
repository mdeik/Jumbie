use serde::{Deserialize, Serialize};

// Every field is #[serde(default)] so all protections are opt-in: security headers
// that are correct behind a reverse proxy are WRONG when the app runs directly,
// and there's no way to auto-detect a proxy at config load. Defaulting to "safe"
// (no headers) beats defaulting to "broken" (conflicting headers). Hence
// default() is all false/empty and is derived, not hand-written.
#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct SecurityConfig {
    /// Add X-Frame-Options: SAMEORIGIN to prevent clickjacking
    #[serde(default)]
    pub clickjacking_protection: bool,
    /// Add CSRF-relevant security headers (X-Content-Type-Options, Referrer-Policy)
    #[serde(default)]
    pub csrf_protection: bool,
    /// Reject requests whose Host header is not in allowed_domains
    #[serde(default)]
    pub host_header_validation: bool,
    /// Allowed Host header values; supports wildcard '*' and ';'-separated lists
    #[serde(default)]
    pub allowed_domains: Vec<String>,
    /// Append arbitrary headers to every response when enabled
    #[serde(default)]
    pub use_custom_headers: bool,
    /// Raw "Header: value" lines to inject into every response
    #[serde(default)]
    pub custom_headers: Vec<String>,
    /// IPs/CIDRs of trusted reverse proxies whose X-Forwarded-For is trusted
    ///
    /// Keep this list NARROW — enumerate the proxies themselves, not broad client
    /// subnets. Trust is CIDR-based, so any peer inside the range is treated as a
    /// proxy (indistinguishable from a real one). The resolver walks
    /// X-Forwarded-For from the right and returns the first entry not in this
    /// list; if real clients share the range, the result is ambiguous.
    ///
    /// Prefer proxies configured to OVERWRITE `X-Forwarded-For` rather than append
    /// to it (`proxy_set_header X-Forwarded-For $remote_addr` in nginx, not
    /// `$proxy_add_x_forwarded_for`).
    #[serde(default)]
    pub trusted_proxies: Vec<String>,
    /// When true, restrict CORS to allowed_origins instead of wildcard
    #[serde(default)]
    pub restrict_cors: bool,
    /// Origins allowed for CORS requests, e.g. ["https://myapp.example.com"].
    /// Only used when restrict_cors is true. An empty list means no cross-origin requests allowed.
    #[serde(default)]
    pub allowed_origins: Vec<String>,
    // Rate limiting — all default to disabled/0 for zero overhead on fresh installs.
    /// Enable per-IP rate limiting for all API routes
    #[serde(default)]
    pub rate_limit_enabled: bool,
    /// Maximum requests per IP per 60-second sliding window
    #[serde(default = "default_rate_limit_per_minute")]
    pub rate_limit_per_minute: u32,
    /// Short burst of requests allowed before rate limiting kicks in.
    /// Set to `rate_limit_per_minute` (or higher) to disable burst gating.
    #[serde(default = "default_rate_limit_burst")]
    pub rate_limit_burst: u32,
    // Plugin security
    /// When true, external plugins MUST ship a valid Ed25519 signature in their
    /// manifest (`public_key` + `signature` over the executable's BLAKE3 hash)
    /// or discovery fails. Opt-in (consistent with the other security toggles):
    /// defaulting on would break unsigned plugins on fresh installs.
    #[serde(default)]
    pub require_plugin_signatures: bool,
}

fn default_rate_limit_per_minute() -> u32 {
    60
}
fn default_rate_limit_burst() -> u32 {
    10
}
