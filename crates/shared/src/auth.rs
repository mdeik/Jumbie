//! API authorization scopes for the jumbie.
//!
//! Each scope is declared once in [`define_scopes!`], which generates the enum,
//! all lookup methods, and trait impls.
//!
//! # Scope hierarchy
//!
//! Each write scope implies its paired read scope via [`ApiScope::permits`] (never
//! the reverse). Scopes from unrelated domains are never compatible even when both
//! are "read" (e.g. `series:read` does not permit `config:read`), preventing
//! accidental privilege escalation. `search` is a single-action scope with no
//! read/write split; `activity:read` and `wanted:read` stay separate so consumers
//! can grant only what they need.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// Defines all API scopes in one place and generates:
///
///   - [`ApiScope`] enum with `#[serde(rename = "...")]` on every variant
///   - [`ApiScope::description`], [`ApiScope::all`], [`ApiScope::as_str`]
///   - [`ApiScope::SCOPE_PAIRS`], [`ApiScope::read_scope`], [`ApiScope::write_scope`], [`ApiScope::permits`]
///   - [`Display`](fmt::Display) and [`FromStr`] implementations
///
/// # Syntax
///
/// ```ignore
/// define_scopes! {
///     pair SeriesWrite "series:write" "Write desc", SeriesRead "series:read" "Read desc";
///     pair FilesWrite  "files:write"  "Write desc", FilesRead  "files:read"  "Read desc";
///     single WantedRead "wanted:read" "Desc";
///     single Search "search" { "Multi-line desc" };
/// }
/// ```
macro_rules! define_scopes {
    (
        $(pair $w_v:ident $w_s:literal $w_d:expr, $r_v:ident $r_s:literal $r_d:expr;)*
        $(single $v:ident $s:literal $d:expr;)*
    ) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        pub enum ApiScope {
            $(
                #[serde(rename = $w_s)]
                $w_v,
                #[serde(rename = $r_s)]
                $r_v,
            )*
            $(
                #[serde(rename = $s)]
                $v,
            )*
        }

        impl ApiScope {
            /// Pairs of (write_scope, read_scope). Derived from `pair` declarations.
            const SCOPE_PAIRS: &[(ApiScope, ApiScope)] = &[
                $((ApiScope::$w_v, ApiScope::$r_v),)*
            ];

            /// Returns true if this scope grants access to the required scope.
            ///
            /// `*:write` implies `*:read` — a write scope permits its paired read scope.
            /// Standalone scopes only permit themselves.
            pub fn permits(&self, required: ApiScope) -> bool {
                *self == required
                    || Self::SCOPE_PAIRS
                        .iter()
                        .any(|(w, r)| w == self && *r == required)
            }

            /// If this is a write scope, returns the corresponding read scope.
            pub fn read_scope(&self) -> Option<ApiScope> {
                Some(Self::SCOPE_PAIRS.iter().find(|(w, _)| w == self)?.1)
            }

            /// If this is a read scope, returns the corresponding write scope.
            pub fn write_scope(&self) -> Option<ApiScope> {
                Some(Self::SCOPE_PAIRS.iter().find(|(_, r)| r == self)?.0)
            }

            /// Returns a human-readable description of what this scope permits.
            pub fn description(&self) -> &'static str {
                match self {
                    $(ApiScope::$w_v => $w_d,)*
                    $(ApiScope::$r_v => $r_d,)*
                    $(ApiScope::$v => $d,)*
                }
            }

            /// Returns a list of all available scopes in declaration order.
            pub fn all() -> &'static [ApiScope] {
                &[$(ApiScope::$w_v, ApiScope::$r_v,)* $(ApiScope::$v,)*]
            }

            /// Returns the serde string for this scope (e.g. `"series:read"`).
            pub fn as_str(&self) -> &'static str {
                match self {
                    $(ApiScope::$w_v => $w_s,)*
                    $(ApiScope::$r_v => $r_s,)*
                    $(ApiScope::$v => $s,)*
                }
            }
        }

        impl fmt::Display for ApiScope {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.as_str())
            }
        }

        impl FromStr for ApiScope {
            type Err = String;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                match s {
                    $($w_s => Ok(ApiScope::$w_v),)*
                    $($r_s => Ok(ApiScope::$r_v),)*
                    $($s => Ok(ApiScope::$v),)*
                    _ => Err(format!("Unknown scope: {}", s)),
                }
            }
        }
    };
}

// SSoT: every scope is declared once below. Order matters — it determines the
// UI order in `all()`. Resource-owning scopes come first (write then read per
// domain), then standalone read-only scopes, then `Search` at the end.
define_scopes! {
    // Resource pairs (write, read)
    pair SeriesWrite "series:write" "Allows adding, editing, and deleting series and episodes.",
         SeriesRead  "series:read"  "Allows viewing series and episodes.";
    pair FilesWrite  "files:write"  "Allows manual file assignment, reorganization, and deletion.",
         FilesRead   "files:read"   "Allows viewing organized files and library visibility.";
    pair RenameWrite "rename:write" "Allows remediating renaming issues.",
         RenameRead  "rename:read"  "Allows viewing the rename queue.";
    pair QueueWrite  "queue:write"  "Allows adding downloads and managing the download queue.",
         QueueRead   "queue:read"   "Allows viewing the download queue.";
    pair PluginsWrite "plugins:write" "Allows configuring and validating plugins.",
         PluginsRead  "plugins:read"  "Allows viewing plugin status and schema.";
    pair ConfigWrite "config:write" "Allows updating application settings.",
         ConfigRead  "config:read"  "Allows viewing application settings.";
    pair AuthWrite   "auth:write"   "Allows managing bans and generating API tokens.",
         AuthRead    "auth:read"    "Allows viewing active bans.";
    // Standalone read-only scopes
    single SystemRead   "system:read"   "Allows viewing health and about information.";
    single WantedRead   "wanted:read"   "Allows viewing missing episodes.";
    single ActivityRead "activity:read" "Allows viewing recent activity.";
    single LogsRead     "logs:read"     "Allows viewing system logs.";
    // Standalone action scope
    single Search "search" {
        "Allows performing manual searches. Automatic and season searches also require \
         queue:write scope, since they queue downloads."
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scope_string_conversion() {
        for scope in ApiScope::all() {
            let s = scope.as_str();
            let parsed = ApiScope::from_str(s).expect("Failed to parse scope from string");
            assert_eq!(
                *scope, parsed,
                "Scope parsed from string does not match original"
            );
            assert_eq!(scope.to_string(), s, "Display impl does not match as_str");
        }
    }

    #[test]
    fn test_scope_permits_itself() {
        for scope in ApiScope::all() {
            assert!(scope.permits(*scope), "{:?} should permit itself", scope);
        }
    }

    #[test]
    fn test_write_permits_read() {
        for scope in ApiScope::all() {
            if let Some(read) = scope.read_scope() {
                assert!(scope.permits(read), "{scope:?} should permit {read:?}");
            }
        }
    }

    #[test]
    fn test_read_does_not_permit_write() {
        for scope in ApiScope::all() {
            if let Some(write) = scope.write_scope() {
                assert!(
                    !scope.permits(write),
                    "{scope:?} should NOT permit {write:?}"
                );
            }
        }
    }

    #[test]
    fn test_unrelated_scopes_do_not_permit() {
        assert!(!ApiScope::SeriesWrite.permits(ApiScope::ConfigWrite));
        assert!(!ApiScope::SystemRead.permits(ApiScope::LogsRead));
        assert!(!ApiScope::ActivityRead.permits(ApiScope::SeriesRead));
        assert!(!ApiScope::WantedRead.permits(ApiScope::SeriesRead));
        assert!(!ApiScope::ActivityRead.permits(ApiScope::WantedRead));
        assert!(!ApiScope::WantedRead.permits(ApiScope::ActivityRead));
        assert!(!ApiScope::Search.permits(ApiScope::SeriesRead));
    }
}
