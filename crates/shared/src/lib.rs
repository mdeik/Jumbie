// Core infrastructure

pub mod auth; // API authorization scopes (ApiScope, permits hierarchy)
pub mod config; // Application configuration (Config, sub-configs)
pub mod datetime; // Canonical UTC timestamps (UtcDateTime, parse_utc) — SSoT
pub mod macros; // Convenience macros (test_module!)
pub mod serde_utc; // Serde adapters: naive-UTC DB values ↔ RFC 3339 on the wire
pub mod types; // API payload / view-model types (re-exports from other modules)

// Media parsing & pattern matching

pub mod parsing; // Filename → EpisodeInfo conversion
pub mod paths; // Series path policy engine (illegal chars, ${series}, collisions)
pub mod patterns; // Compiled regex patterns (LazyLock singletons)
pub mod protocol; // URI wildcard matching (matches_protocol, is_likely_link)

// Series mapping & formatting

pub mod filtering; // Search filter rules (FilterRule, MatchMode)
pub mod formatting; // Episode/season label formatting, search query building
pub mod mapping; // MappingRule, SeasonOverride, SeriesSettings + alias/pattern helpers
pub mod scoring; // Release profiling & quality scoring (ReleaseProfile, Quality, …)
pub mod template; // `${variable}` template rendering (apply_template)

// Media metadata

pub mod languages; // Language mapping and normalisation
pub mod media_format; // File extension validation (video/subtitle)
pub mod quality; // Quality detection from release title strings

// Plugin system

pub mod plugin; // Plugin configuration contract (PluginConfig trait)

// Validation & templating

pub mod validation; // Field-level validators (names, ports, templates, …)
pub mod variables; // Template variable definitions and context helpers

pub use mapping::{
    NumberingMode, ParsedAlias, ParsedPattern, parse_alias, parse_source_pattern,
    plugin_name_to_slug,
};

pub const APP_USER_AGENT: &str = concat!("Jumbie/", env!("CARGO_PKG_VERSION"));
