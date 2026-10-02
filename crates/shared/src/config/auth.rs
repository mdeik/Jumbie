use crate::auth::ApiScope;
use serde::{Deserialize, Serialize};

/// A single entry in the persistent ban list.
///
/// Banned IPs live in the database (not `config.toml`) so they survive restarts
/// and are managed via the UI. This struct is kept to deserialize legacy config
/// files that may define `banned_ips`; `skip_serializing_if` ensures runtime bans
/// are never written back to the config file.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct BannedIp {
    pub ip: String,
    pub fail_count: u32,
    // serde(default) so older/newly migrated bans without this field default to 0
    // ("first offense").
    #[serde(default)]
    pub ban_count: u32,
    /// ISO 8601 timestamp of when the ban was applied
    pub banned_at: String,
    /// ISO 8601 timestamp of expiry; None = permanent ban
    pub banned_until: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(default)]
pub struct AuthConfig {
    // These are `Option`/`Vec` without serde(default): if present in the config
    // file they must be valid; missing is fine (outer Config has serde(default)).
    pub password: Option<String>,
    // skip_serializing_if keeps empty Vecs out of config.toml.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub api_keys: Vec<ApiKey>,
    pub calendar_token: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub calendar_tokens: Vec<CalendarToken>,
    /// Skip auth for requests from 127.0.0.1 / ::1
    #[serde(default)]
    pub bypass_local_auth: bool,
    /// Skip auth for IPs matching any entry in subnet_whitelist
    #[serde(default)]
    pub bypass_subnet_whitelist: bool,
    /// CIDR subnets to whitelist, e.g. ["192.168.1.0/24"]
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subnet_whitelist: Vec<String>,
    /// Ban a client after this many consecutive auth failures (0 = disabled)
    #[serde(default = "default_max_auth_fail_count")]
    pub max_auth_fail_count: u32,
    /// How long (seconds) a banned client stays banned.
    ///
    /// 300s balances slowing brute-force attempts against locking out a legitimate
    /// user who mistyped their password.
    #[serde(default = "default_ban_duration")]
    pub ban_duration_seconds: u64,
    /// Enable incrementing ban durations for reoccurring bans
    #[serde(default)]
    pub ban_increment_enabled: bool,
    /// Multiplier factor for incrementing bans (e.g. 2.0 = double each time).
    ///
    /// Exponential backoff: each repeat offense doubles the penalty.
    #[serde(default = "default_ban_increment_factor")]
    pub ban_increment_factor: f32,
    /// Maximum ban duration in seconds (1 year).
    ///
    /// Caps the doubling so durations stay finite rather than growing unbounded.
    #[serde(default = "default_ban_increment_max_seconds")]
    pub ban_increment_max_seconds: u64,
    /// Reset the ban counter after this many days of no offenses.
    ///
    /// 30 days: a month of good behavior suggests a one-time mistake rather than a
    /// persistent attacker.
    #[serde(default = "default_ban_count_reset_days")]
    pub ban_count_reset_days: u32,
    /// Ban list — stored in the database, not serialized to the config file.
    ///
    /// Runtime bans are managed through the DB and UI, so writing them back to
    /// `config.toml` would conflict with DB state.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub banned_ips: Vec<BannedIp>,
}

impl AuthConfig {
    // Default values, exposed so the UI resolves blank inputs to the same value
    // the schema (serde/Default) uses. These are the SSoT for the `default_*`
    // functions below and the security settings fields.
    pub const MAX_AUTH_FAIL_COUNT_DEFAULT: u32 = 0; // 0 = disabled
    pub const BAN_DURATION_SECONDS_DEFAULT: u64 = 300; // 5 minutes
    pub const BAN_INCREMENT_FACTOR_DEFAULT: f32 = 2.0; // double each time
    pub const BAN_INCREMENT_MAX_SECONDS_DEFAULT: u64 = 31_536_000; // 1 year
    pub const BAN_COUNT_RESET_DAYS_DEFAULT: u32 = 30; // one month

    /// Initial `max_auth_fail_count` seeded on a fresh database (OWASP-recommended
    /// threshold). Distinct from [`Self::MAX_AUTH_FAIL_COUNT_DEFAULT`], which is
    /// the "unset" fallback.
    pub const MAX_AUTH_FAIL_COUNT_SEEDED: u32 = 5;
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            password: None,
            api_keys: Vec::new(),
            calendar_token: None,
            calendar_tokens: Vec::new(),
            bypass_local_auth: false,
            bypass_subnet_whitelist: false,
            subnet_whitelist: Vec::new(),
            max_auth_fail_count: default_max_auth_fail_count(),
            ban_duration_seconds: default_ban_duration(),
            ban_increment_enabled: false,
            ban_increment_factor: default_ban_increment_factor(),
            ban_increment_max_seconds: default_ban_increment_max_seconds(),
            ban_count_reset_days: default_ban_count_reset_days(),
            banned_ips: Vec::new(),
        }
    }
}

fn default_ban_duration() -> u64 {
    AuthConfig::BAN_DURATION_SECONDS_DEFAULT
}

fn default_max_auth_fail_count() -> u32 {
    AuthConfig::MAX_AUTH_FAIL_COUNT_DEFAULT
}

fn default_ban_increment_factor() -> f32 {
    AuthConfig::BAN_INCREMENT_FACTOR_DEFAULT
}

fn default_ban_increment_max_seconds() -> u64 {
    AuthConfig::BAN_INCREMENT_MAX_SECONDS_DEFAULT
}

fn default_ban_count_reset_days() -> u32 {
    AuthConfig::BAN_COUNT_RESET_DAYS_DEFAULT
}

/// An API key with scoped permissions.
///
/// A separate struct so keys can carry lifecycle metadata (name, scopes, expiry);
/// inlining them as raw strings would lose that.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ApiKey {
    pub id: String,
    pub name: String,
    pub key: String,
    pub prefix: String,
    pub scopes: Vec<ApiScope>,
    // Option + default: not all API keys expire; permanent keys are common for
    // server-to-server integrations.
    #[serde(default)]
    pub expires_at: Option<String>,
}

/// Calendar tokens are separate from API keys: calendar integration (
/// e.g. iCal feed access) needs read-only, unscoped access, whereas API keys have
/// granular permission scopes.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct CalendarToken {
    pub id: String,
    pub name: String,
    pub token: String,
    #[serde(default)]
    pub hide_unmonitored: bool,
    #[serde(default)]
    pub show_as_all_day: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exposed_defaults_match_constructed_defaults() {
        let cfg = AuthConfig::default();
        assert_eq!(
            cfg.max_auth_fail_count,
            AuthConfig::MAX_AUTH_FAIL_COUNT_DEFAULT
        );
        assert_eq!(
            cfg.ban_duration_seconds,
            AuthConfig::BAN_DURATION_SECONDS_DEFAULT
        );
        assert_eq!(
            cfg.ban_increment_factor,
            AuthConfig::BAN_INCREMENT_FACTOR_DEFAULT
        );
        assert_eq!(
            cfg.ban_increment_max_seconds,
            AuthConfig::BAN_INCREMENT_MAX_SECONDS_DEFAULT
        );
        assert_eq!(
            cfg.ban_count_reset_days,
            AuthConfig::BAN_COUNT_RESET_DAYS_DEFAULT
        );
    }

    #[test]
    fn test_serde_missing_fields_use_exposed_defaults() {
        let cfg: AuthConfig = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(
            cfg.max_auth_fail_count,
            AuthConfig::MAX_AUTH_FAIL_COUNT_DEFAULT
        );
        assert_eq!(
            cfg.ban_duration_seconds,
            AuthConfig::BAN_DURATION_SECONDS_DEFAULT
        );
        assert_eq!(
            cfg.ban_increment_factor,
            AuthConfig::BAN_INCREMENT_FACTOR_DEFAULT
        );
        assert_eq!(
            cfg.ban_increment_max_seconds,
            AuthConfig::BAN_INCREMENT_MAX_SECONDS_DEFAULT
        );
        assert_eq!(
            cfg.ban_count_reset_days,
            AuthConfig::BAN_COUNT_RESET_DAYS_DEFAULT
        );
    }
}
