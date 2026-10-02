pub mod defaults;

use config::ConfigError;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

pub mod auth;
pub mod automatic;
pub mod general;
pub mod organization;
pub mod plugins;
pub mod security;
pub mod ui;

pub use auth::{ApiKey, AuthConfig, BannedIp, CalendarToken};
pub use automatic::{
    AutomaticProfileCategory, AutomaticProfileRule, AutomaticProfilesConfig, RuleCondition,
    ThresholdCondition, TrackType, UnexpectedFilesMode,
};
pub use general::{DynamicProfilesConfig, GeneralConfig, ProxyConfig, SourcesConfig};
pub use organization::{CollisionStrategy, DestinationRoot, OrganizationConfig};
pub use plugins::{
    BasicRssConfig, DiscordConfig, NyaaConfig, PluginsConfig, SeasonPackStrategy,
    ensure_single_metadata_plugin, instance_is_enabled,
};
pub use security::SecurityConfig;
pub use ui::{
    ActivityUIConfig, LogsUIConfig, ReleaseDateDisplayConfig, TableSortState, Theme, TimeFormat,
    UIConfig,
};

// ConfigFile holds the 4 fields that live in config.toml. It exists only for TOML
// (de)serialization of the optional override file: the Rust default functions below
// are the SSoT for startup paths (platform-aware, including portable.txt detection).
// All other sections (organization, sources, general, proxy, auth, security) are
// DB-managed and never touch disk.
#[derive(Debug, Default, Deserialize, Serialize, Clone)]
pub(crate) struct ConfigFile {
    #[serde(default = "default_database")]
    pub database: String,

    #[serde(default = "default_unknown_files_tmp_dir")]
    pub unknown_files_tmp_dir: PathBuf,

    #[serde(default = "default_plugins_dir")]
    pub plugins_dir: PathBuf,

    #[serde(default = "default_logs_dir")]
    pub logs_dir: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_database")]
    pub database: String,

    #[serde(default = "default_unknown_files_tmp_dir")]
    pub unknown_files_tmp_dir: PathBuf,

    #[serde(default = "default_plugins_dir")]
    pub plugins_dir: PathBuf,

    #[serde(default = "default_logs_dir")]
    pub logs_dir: PathBuf,

    #[serde(default)]
    pub organization: OrganizationConfig,
    #[serde(default)]
    pub sources: SourcesConfig,
    #[serde(default)]
    pub general: GeneralConfig,
    #[serde(default)]
    pub proxy: ProxyConfig,
    #[serde(default)]
    pub auth: AuthConfig,
    #[serde(default)]
    pub security: SecurityConfig,

    /// Original TOML-sourced values, unaffected by env var overrides.
    /// Used exclusively by `save()` so that env-overridden paths never
    /// leak back to config.toml.
    #[serde(skip)]
    pub(crate) toml_values: ConfigFile,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            database: default_database(),
            unknown_files_tmp_dir: default_unknown_files_tmp_dir(),
            plugins_dir: default_plugins_dir(),
            logs_dir: default_logs_dir(),
            organization: OrganizationConfig::default(),
            sources: SourcesConfig::default(),
            general: GeneralConfig::default(),
            proxy: ProxyConfig::default(),
            auth: AuthConfig::default(),
            security: SecurityConfig::default(),
            toml_values: ConfigFile {
                database: default_database(),
                unknown_files_tmp_dir: default_unknown_files_tmp_dir(),
                plugins_dir: default_plugins_dir(),
                logs_dir: default_logs_dir(),
            },
        }
    }
}

impl Config {
    /// Load config from Rust defaults, optionally overlaying values from a TOML file.
    ///
    /// The config.toml file is optional and never auto-generated. If present, its
    /// values override the Rust defaults, and relative paths are resolved against
    /// the executable's directory on every platform (portable configs). Env vars
    /// (JUMBIE_DATABASE, JUMBIE_PLUGINS_DIR, JUMBIE_TMP_DIR, JUMBIE_LOGS_DIR)
    /// override both at runtime only — they are never written back by `save()`.
    pub fn new(config_path: &str) -> Result<Self, ConfigError> {
        let path = Path::new(config_path);

        // Rust platform defaults are the SSoT; in portable mode these are relative.
        let mut config = Config::default();

        // Overlay the TOML file's values on top of the Rust defaults, if it exists.
        if path.exists() {
            let content = fs::read_to_string(path).map_err(|e| {
                ConfigError::Message(format!(
                    "Failed to read config file '{}': {}",
                    config_path, e
                ))
            })?;
            let config_file: ConfigFile = toml::from_str(&content).map_err(|e| {
                ConfigError::Message(format!("TOML parse error at '{}': {}", config_path, e))
            })?;

            config.database = config_file.database.clone();
            config.unknown_files_tmp_dir = config_file.unknown_files_tmp_dir.clone();
            config.plugins_dir = config_file.plugins_dir.clone();
            config.logs_dir = config_file.logs_dir.clone();
            config.toml_values = config_file;
        }

        // Resolve relative paths (portable defaults and user overrides) against the
        // executable's directory on all platforms.
        #[cfg(not(feature = "docker-defaults"))]
        {
            let exe_dir = win_exe_dir();
            let resolve = |p: &Path| -> PathBuf {
                if p.is_relative() {
                    exe_dir.join(p)
                } else {
                    p.to_path_buf()
                }
            };
            config.database = resolve(Path::new(&config.database))
                .to_string_lossy()
                .to_string();
            config.unknown_files_tmp_dir = resolve(&config.unknown_files_tmp_dir);
            config.plugins_dir = resolve(&config.plugins_dir);
            config.logs_dir = resolve(&config.logs_dir);
        }

        // Env var overrides apply at runtime only (not to `toml_values`), so save()
        // always writes the original TOML-sourced paths.
        if let Ok(val) = std::env::var("JUMBIE_DATABASE") {
            config.database = val;
        }
        if let Ok(val) = std::env::var("JUMBIE_PLUGINS_DIR") {
            config.plugins_dir = PathBuf::from(val);
        }
        if let Ok(val) = std::env::var("JUMBIE_TMP_DIR") {
            config.unknown_files_tmp_dir = PathBuf::from(val);
        }
        if let Ok(val) = std::env::var("JUMBIE_LOGS_DIR") {
            config.logs_dir = PathBuf::from(val);
        }

        Ok(config)
    }

    /// Round-trips only the 5 TOML-managed fields back to disk; DB-managed sections
    /// (organization, sources, general, proxy, auth, security) are excluded so the
    /// file stays a minimal startup-path config that survives binary reinstallation.
    ///
    /// CRITICAL: writes `self.toml_values` (the original TOML-sourced paths), not the
    /// runtime fields, so env var overrides never leak back into config.toml.
    pub fn save(&self, config_path: &str) -> Result<(), ConfigError> {
        let toml_string = toml::to_string_pretty(&self.toml_values)
            .map_err(|e| ConfigError::Message(e.to_string()))?;

        fs::write(config_path, toml_string).map_err(|e| ConfigError::Message(e.to_string()))?;

        Ok(())
    }
}

/// Centralized UUID generation for the whole application — the single point of
/// control for changing the ID scheme (e.g. to ULID).
pub fn generate_uuid() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Shared by structs using `#[serde(default = "crate::config::default_true")]`,
/// avoiding a duplicate `fn default_true()` per struct.
fn default_true() -> bool {
    true
}

/// Default config.toml path.
///
/// Docker: `/config/config.toml` — matches the `./config:/config` volume mount.
///
/// Portable (`portable.txt` next to the binary): `<exe_dir>/config/config.toml`
///
/// Windows: `<exe_dir>/config/config.toml`
///
/// macOS: `~/Library/Application Support/jumbie/config.toml`
///
/// Linux: `$XDG_CONFIG_HOME/jumbie/config.toml` or `~/.config/jumbie/config.toml`.
pub fn default_config_path() -> PathBuf {
    #[cfg(feature = "docker-defaults")]
    {
        PathBuf::from("/config/config.toml")
    }
    #[cfg(not(feature = "docker-defaults"))]
    {
        if is_portable() {
            return win_exe_dir().join("config").join("config.toml");
        }
        #[cfg(windows)]
        {
            win_exe_dir().join("config").join("config.toml")
        }
        #[cfg(target_os = "macos")]
        {
            mac_app_support_dir().join("jumbie").join("config.toml")
        }
        #[cfg(not(any(windows, target_os = "macos")))]
        {
            let xdg_config = std::env::var("XDG_CONFIG_HOME")
                .ok()
                .map(PathBuf::from)
                .or_else(|| {
                    std::env::var("HOME")
                        .ok()
                        .map(|h| PathBuf::from(h).join(".config"))
                })
                .unwrap_or_else(|| PathBuf::from(".config"));
            xdg_config.join("jumbie").join("config.toml")
        }
    }
}

/// Default database path used when `database` is omitted from config.toml.
///
/// Docker: `/data/jumbie.db` — matches the `./data:/data` volume mount.
///
/// Portable (`portable.txt` next to the binary): `jumbie/jumbie.db`
/// (relative — resolved against `<exe_dir>` at load time).
///
/// Linux (default): `~/.local/share/jumbie/jumbie.db` — follows the
/// XDG Base Directory Specification (`$XDG_DATA_HOME/jumbie/jumbie.db`).
///
/// macOS: `~/Library/Application Support/jumbie/jumbie.db`.
///
/// Windows (native): `%APPDATA%/jumbie/jumbie.db`.
pub fn default_database() -> String {
    #[cfg(feature = "docker-defaults")]
    {
        String::from("/data/jumbie.db")
    }
    #[cfg(not(feature = "docker-defaults"))]
    {
        if is_portable() {
            return "jumbie/jumbie.db".to_string();
        }
        xdg_data_dir()
            .join("jumbie")
            .join("jumbie.db")
            .to_string_lossy()
            .to_string()
    }
}

/// Default plugins directory.
///
/// Docker: `/plugins`.
///
/// Portable (`portable.txt` next to the binary): `jumbie/plugins`
/// (relative — resolved against `<exe_dir>` at load time).
///
/// Linux: `~/.local/share/jumbie/plugins`.
///
/// macOS: `~/Library/Application Support/jumbie/plugins`.
///
/// Windows (native): `%APPDATA%/jumbie/plugins`.
pub fn default_plugins_dir() -> PathBuf {
    #[cfg(feature = "docker-defaults")]
    {
        PathBuf::from("/plugins")
    }
    #[cfg(not(feature = "docker-defaults"))]
    {
        if is_portable() {
            return PathBuf::from("jumbie/plugins");
        }
        xdg_data_dir().join("jumbie").join("plugins")
    }
}

/// Default temporary directory for unknown/unmatched files.
///
/// Portable (`portable.txt` next to the binary): `tmp/jumbie` (relative — resolved
/// against `<exe_dir>` at load time). Otherwise `std::env::temp_dir()/jumbie`
/// (Linux/Docker `/tmp/jumbie`, macOS `$TMPDIR/jumbie`, Windows `%TEMP%\jumbie`).
pub fn default_unknown_files_tmp_dir() -> PathBuf {
    #[cfg(target_arch = "wasm32")]
    {
        // The browser has no filesystem and this path is a backend-only concern
        // that is never used on wasm. `std::env::temp_dir()` *panics* on
        // wasm32-unknown-unknown, which would abort `Config::default()`; return a
        // portable relative path instead.
        PathBuf::from("tmp/jumbie")
    }
    #[cfg(all(not(target_arch = "wasm32"), feature = "docker-defaults"))]
    {
        PathBuf::from("/tmp/jumbie")
    }
    #[cfg(all(not(target_arch = "wasm32"), not(feature = "docker-defaults")))]
    {
        if is_portable() {
            PathBuf::from("tmp/jumbie")
        } else {
            std::env::temp_dir().join("jumbie")
        }
    }
}

/// Default log directory.
///
/// Portable (`portable.txt` next to the binary): `jumbie/logs`
/// (relative — resolved against `<exe_dir>` at load time).
///
/// Docker: `/logs` — matches the `./logs:/logs` volume mount.
///
/// macOS: `~/Library/Logs/jumbie` — Apple's official log directory.
///
/// Linux: `$XDG_STATE_HOME/jumbie/logs` or `~/.local/state/jumbie/logs`.
///
/// Windows (native): `%LOCALAPPDATA%/jumbie/logs`.
pub fn default_logs_dir() -> PathBuf {
    #[cfg(feature = "docker-defaults")]
    {
        PathBuf::from("/logs")
    }
    #[cfg(not(feature = "docker-defaults"))]
    {
        if is_portable() {
            return PathBuf::from("jumbie/logs");
        }
        #[cfg(target_os = "macos")]
        {
            // macOS uses ~/Library/Logs for logs, not Application Support.
            // This matches Apple's convention and `setup.rs`'s runtime fallback.
            let home = std::env::var("HOME")
                .ok()
                .unwrap_or_else(|| String::from("."));
            PathBuf::from(home)
                .join("Library")
                .join("Logs")
                .join("jumbie")
        }
        #[cfg(not(any(target_os = "macos")))]
        {
            xdg_state_dir().join("jumbie").join("logs")
        }
    }
}

/// Check for a `portable.txt` marker file next to the executable.
/// When present, all data paths default to relative paths resolved against
/// `<exe_dir>`, making the installation relocatable across machines or drives.
///
/// Create an empty file called `portable.txt` in the same directory as the
/// `jumbie` binary to enable portable mode on any platform.
#[cfg(not(feature = "docker-defaults"))]
pub fn is_portable() -> bool {
    win_exe_dir().join("portable.txt").exists()
}

// XDG path helpers (Unix, non-Docker): resolve data/config/state paths at runtime
// without extra deps. Linux follows the XDG Base Directory Spec; macOS uses
// `~/Library/Application Support`; Docker uses container absolute paths.

/// Parent directory of the running executable (used on Windows so defaults
/// are relative to where the program lives, not the working directory).
pub fn win_exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// `~/Library/Application Support` — the macOS convention for app data.
pub fn mac_app_support_dir() -> PathBuf {
    let home = std::env::var("HOME")
        .ok()
        .unwrap_or_else(|| String::from("."));
    PathBuf::from(home)
        .join("Library")
        .join("Application Support")
}

/// `%APPDATA%`  |  macOS: `~/Library/Application Support`
///
/// Windows native uses `%APPDATA%` (typically
/// `C:\Users\<user>\AppData\Roaming`). Users who want data next to the
/// binary should create `portable.txt` to enable portable mode.
#[cfg(not(feature = "docker-defaults"))]
pub(crate) fn xdg_data_dir() -> PathBuf {
    #[cfg(windows)]
    {
        std::env::var("APPDATA")
            .ok()
            .map(PathBuf::from)
            .unwrap_or_else(win_exe_dir)
    }
    #[cfg(target_os = "macos")]
    {
        mac_app_support_dir()
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        std::env::var("XDG_DATA_HOME")
            .ok()
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var("HOME")
                    .ok()
                    .map(|h| PathBuf::from(h).join(".local").join("share"))
            })
            .unwrap_or_else(|| PathBuf::from(".local/share"))
    }
}

/// `%LOCALAPPDATA%`  |  macOS: `~/Library/Application Support` (Apple
/// does not distinguish data and state; both live under `Application Support`.)
///
/// Windows native uses `%LOCALAPPDATA%` (typically
/// `C:\Users\<user>\AppData\Local`) for state data like logs. Users who
/// want data next to the binary should create `portable.txt`.
#[cfg(not(feature = "docker-defaults"))]
#[cfg_attr(target_os = "macos", expect(dead_code))]
pub(crate) fn xdg_state_dir() -> PathBuf {
    #[cfg(windows)]
    {
        std::env::var("LOCALAPPDATA")
            .ok()
            .map(PathBuf::from)
            .unwrap_or_else(win_exe_dir)
    }
    #[cfg(target_os = "macos")]
    {
        mac_app_support_dir()
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        std::env::var("XDG_STATE_HOME")
            .ok()
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var("HOME")
                    .ok()
                    .map(|h| PathBuf::from(h).join(".local").join("state"))
            })
            .unwrap_or_else(|| PathBuf::from(".local/state"))
    }
}

crate::test_module! {

    use tempfile::NamedTempFile;

    /// Resolve a TOML path the same way `Config::new()` does (cross-platform).
    /// On Unix, `/data/...` is absolute and kept as-is.
    /// On Windows, it's relative (no drive letter) and gets joined with `win_exe_dir()`.
    fn resolve_toml_path(p: &str) -> PathBuf {
        #[cfg(not(feature = "docker-defaults"))]
        {
            let path = Path::new(p);
            let exe_dir = win_exe_dir();
            if path.is_relative() {
                return exe_dir.join(path);
            }
        }
        PathBuf::from(p)
    }

    // ConfigFile TOML serialization: config.toml must contain only startup-path
    // fields; DB-managed sections must never leak into it.

    #[test]
    fn test_config_file_toml_only_contains_toml_fields() {
        // Arrange: a ConfigFile with all fields populated
        let config_file = ConfigFile {
            database: "/data/test.db".to_string(),
            unknown_files_tmp_dir: PathBuf::from("/tmp/test"),
            plugins_dir: PathBuf::from("/test/plugins"),
            logs_dir: PathBuf::from("/test/logs"),
        };

        // Act: serialize to TOML
        let toml_str = toml::to_string_pretty(&config_file).unwrap();

        // Assert: DB-managed section names must NOT appear
        assert!(!toml_str.contains("[organization]"), "TOML must not contain [organization]");
        assert!(!toml_str.contains("[sources]"), "TOML must not contain [sources]");
        assert!(!toml_str.contains("[general]"), "TOML must not contain [general]");
        assert!(!toml_str.contains("[proxy]"), "TOML must not contain [proxy]");
        assert!(!toml_str.contains("[auth]"), "TOML must not contain [auth]");
        assert!(!toml_str.contains("[security]"), "TOML must not contain [security]");

        // Assert: TOML fields must be present
        assert!(toml_str.contains("database"), "TOML must contain database");
        assert!(toml_str.contains("unknown_files_tmp_dir"), "TOML must contain unknown_files_tmp_dir");
        assert!(toml_str.contains("plugins_dir"), "TOML must contain plugins_dir");
        assert!(toml_str.contains("logs_dir"), "TOML must contain logs_dir");
    }

    #[test]
    fn test_config_json_includes_all_sections() {
        // Arrange: a full Config with all sections populated
        let config = Config {
            database: "/data/test.db".to_string(),
            unknown_files_tmp_dir: PathBuf::from("/tmp/test"),
            plugins_dir: PathBuf::from("/test/plugins"),
            logs_dir: PathBuf::from("/test/logs"),
            organization: OrganizationConfig::default(),
            sources: SourcesConfig::default(),
            general: GeneralConfig::default(),
            proxy: ProxyConfig::default(),
            auth: AuthConfig::default(),
            security: SecurityConfig::default(),
            toml_values: ConfigFile {
                database: "/data/test.db".to_string(),
                unknown_files_tmp_dir: PathBuf::from("/tmp/test"),
                plugins_dir: PathBuf::from("/test/plugins"),
                logs_dir: PathBuf::from("/test/logs"),
            },
        };

        // Act: serialize to JSON (the API format)
        let json_str = serde_json::to_string_pretty(&config).unwrap();

        // Assert: all DB-managed sections must be present in JSON
        assert!(json_str.contains("\"organization\""), "JSON must contain organization");
        assert!(json_str.contains("\"sources\""), "JSON must contain sources");
        assert!(json_str.contains("\"general\""), "JSON must contain general");
        assert!(json_str.contains("\"proxy\""), "JSON must contain proxy");
        assert!(json_str.contains("\"security\""), "JSON must contain security");
        // auth may be partially stripped in API responses, but the key must exist
        assert!(json_str.contains("\"auth\""), "JSON must contain auth");
    }

    #[test]
    fn test_config_load_from_toml_uses_defaults_for_db_sections() {
        // Arrange: write a minimal TOML file with only the 4 allowed fields.
        // Must use a .toml extension for the `config` crate to recognize the format.
        let dir = tempfile::tempdir().unwrap();
        let toml_path = dir.path().join("jumbie_test_config.toml");
        std::fs::write(
            &toml_path,
            r#"
database = "/data/custom.db"
unknown_files_tmp_dir = "/tmp/custom"
plugins_dir = "/custom/plugins"
logs_dir = "/custom/logs"
"#,
        )
        .unwrap();
        let path = toml_path.to_string_lossy().to_string();

        // Act: load via Config::new()
        let config = Config::new(&path).unwrap();

        // Assert: TOML fields match what we wrote
        assert_eq!(config.database, resolve_toml_path("/data/custom.db").to_string_lossy().to_string());
        assert_eq!(config.unknown_files_tmp_dir, resolve_toml_path("/tmp/custom"));
        assert_eq!(config.plugins_dir, resolve_toml_path("/custom/plugins"));
        assert_eq!(config.logs_dir, resolve_toml_path("/custom/logs"));

        // Assert: DB-managed sections use their Rust Default impl
        assert_eq!(
            config.organization.destination_roots,
            OrganizationConfig::default().destination_roots,
            "organization must fall back to Default"
        );
        assert_eq!(
            config.general.season_pack_strategy,
            GeneralConfig::default().season_pack_strategy,
            "general must fall back to Default"
        );
        assert_eq!(
            config.proxy.enabled,
            ProxyConfig::default().enabled,
            "proxy must fall back to Default"
        );
        assert_eq!(
            config.security.clickjacking_protection,
            SecurityConfig::default().clickjacking_protection,
            "security must fall back to Default"
        );
    }

    #[test]
    fn test_config_save_only_writes_toml_fields() {
        // Arrange: a full Config with values in both TOML and DB sections
        let config = Config {
            database: "/data/save_test.db".to_string(),
            unknown_files_tmp_dir: PathBuf::from("/tmp/save_test"),
            plugins_dir: PathBuf::from("/save/plugins"),
            logs_dir: PathBuf::from("/save/logs"),
            organization: OrganizationConfig {
                destination_roots: vec!["/should/not/appear".into()],
                ..OrganizationConfig::default()
            },
            sources: SourcesConfig::default(),
            general: GeneralConfig::default(),
            proxy: ProxyConfig::default(),
            auth: AuthConfig::default(),
            security: SecurityConfig::default(),
            toml_values: ConfigFile {
                database: "/data/save_test.db".to_string(),
                unknown_files_tmp_dir: PathBuf::from("/tmp/save_test"),
                plugins_dir: PathBuf::from("/save/plugins"),
                logs_dir: PathBuf::from("/save/logs"),
            },
        };

        // Act: save to a temp path, then read back
        let tmp = NamedTempFile::new().unwrap();
        let path = tmp.path().to_string_lossy().to_string();
        config.save(&path).unwrap();

        let saved = std::fs::read_to_string(&path).unwrap();

        // Assert: DB-managed sections must NOT appear in the saved TOML
        assert!(!saved.contains("organization"), "Saved TOML must not contain [organization]");
        assert!(!saved.contains("sources"), "Saved TOML must not contain [sources]");
        assert!(!saved.contains("general"), "Saved TOML must not contain [general]");
        assert!(!saved.contains("proxy"), "Saved TOML must not contain [proxy]");
        assert!(!saved.contains("auth"), "Saved TOML must not contain [auth]");
        assert!(!saved.contains("security"), "Saved TOML must not contain [security]");
        // The value from organization.destination_roots should also be absent
        assert!(!saved.contains("should/not/appear"), "Saved TOML must not contain DB-only values");

        // Assert: TOML fields must be present
        assert!(saved.contains("database"), "Saved TOML must contain database");
        assert!(saved.contains("unknown_files_tmp_dir"), "Saved TOML must contain unknown_files_tmp_dir");
        assert!(saved.contains("plugins_dir"), "Saved TOML must contain plugins_dir");
        assert!(saved.contains("logs_dir"), "Saved TOML must contain logs_dir");
    }

    // ── Existing tests below ─────────────────────────────────────────────────

    #[test]
    fn test_proxy_config_deserialization() {
        let toml_str = r#"
            enabled = true
            http = "http://proxy.example.com:8080"
            https = "http://proxy.example.com:8081"
        "#;
        let proxy: ProxyConfig = toml::from_str(toml_str).unwrap();
        assert!(proxy.enabled);
        assert_eq!(proxy.http, "http://proxy.example.com:8080");
        assert_eq!(proxy.https, "http://proxy.example.com:8081");
    }

    #[test]
    fn test_proxy_config_defaults() {
        let proxy = ProxyConfig::default();
        assert!(!proxy.enabled);
        assert!(proxy.http.is_empty());
        assert!(proxy.https.is_empty());
    }

    #[test]
    fn test_auth_config_deserialization() {
        let toml_str = r#"
            password = "secret_password"
            bypass_local_auth = true
            max_auth_fail_count = 10
            subnet_whitelist = ["192.168.1.0/24"]
        "#;
        let auth: AuthConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(auth.password, Some("secret_password".to_string()));
        assert!(auth.bypass_local_auth);
        assert_eq!(auth.max_auth_fail_count, 10);
        assert_eq!(auth.subnet_whitelist, vec!["192.168.1.0/24".to_string()]);
    }

    #[test]
    fn test_auth_config_defaults() {
        let auth = AuthConfig::default();
        assert_eq!(auth.password, None);
        assert!(!auth.bypass_local_auth);
        assert_eq!(auth.max_auth_fail_count, 0); // 0 = disabled
        assert_eq!(auth.ban_duration_seconds, 300); // From default_ban_duration
    }

    #[test]
    fn test_auth_config_serde_defaults() {
        // Test that toml deserialization also picks up these defaults
        let auth: AuthConfig = toml::from_str("").unwrap();
        assert_eq!(auth.max_auth_fail_count, 0); // 0 = disabled
        assert_eq!(auth.ban_duration_seconds, 300);
    }

    #[test]
    fn test_security_config_deserialization() {
        let toml_str = r#"
            clickjacking_protection = true
            csrf_protection = true
            host_header_validation = true
            allowed_domains = ["example.com", "*.example.com"]
            use_custom_headers = true
            custom_headers = ["X-Custom: Value"]
            restrict_cors = true
        "#;
        let security: SecurityConfig = toml::from_str(toml_str).unwrap();
        assert!(security.clickjacking_protection);
        assert!(security.csrf_protection);
        assert!(security.host_header_validation);
        assert_eq!(security.allowed_domains, vec!["example.com".to_string(), "*.example.com".to_string()]);
        assert!(security.use_custom_headers);
        assert_eq!(security.custom_headers, vec!["X-Custom: Value".to_string()]);
        assert!(security.restrict_cors);
    }

    #[test]
    fn test_security_config_defaults() {
        let security = SecurityConfig::default();
        assert!(!security.clickjacking_protection);
        assert!(!security.csrf_protection);
        assert!(!security.host_header_validation);
        assert!(security.allowed_domains.is_empty());
        assert!(!security.use_custom_headers);
        assert!(security.custom_headers.is_empty());
        assert!(!security.restrict_cors);
    }
}
