//! Backend facade over the shared series-path policy engine (`jumbie_shared::paths`).
//!
//! A zero-logic facade that supplies `PlatformOs::Host` — the backend always applies
//! the illegal-char policy for the OS the server runs on, while the frontend calls the
//! shared engine directly with the server's OS. See the shared module for the
//! illegal-character, `${series}` template, collision-handling, and mapping-path
//! policies.

use jumbie_shared::config::organization::{InvalidCharPolicy, OrganizationConfig};
use jumbie_shared::patterns::PlatformOs;
use jumbie_shared::types::MappingRule;
use std::path::{Path, PathBuf};

/// Re-exported unchanged: these functions need no OS parameter.
pub use jumbie_shared::paths::{
    next_free_suffixed_path, resolve_folder_collision, strip_collision_suffix,
    strip_collision_suffixes_of, unicode_lookalike,
};

/// Sanitize a name according to the configured illegal character policy.
/// Backend wrapper for [`jumbie_shared::paths::sanitize_with_policy`] with
/// `PlatformOs::Host` (the server's own OS).
pub fn sanitize_with_policy(
    filename: &str,
    policy: &InvalidCharPolicy,
    allow_platform_specific: bool,
) -> String {
    jumbie_shared::paths::sanitize_with_policy(
        filename,
        policy,
        allow_platform_specific,
        PlatformOs::Host,
    )
}

/// Sanitize a series title per the org illegal-char policy.
pub fn sanitize_title(title: &str, org: &OrganizationConfig) -> String {
    jumbie_shared::paths::sanitize_title(title, org, PlatformOs::Host)
}

/// Resolve a stored series path template, sanitizing the `${series}` value.
pub fn resolve_template(path_str: &str, title: &str, org: &OrganizationConfig) -> PathBuf {
    jumbie_shared::paths::resolve_template(path_str, title, org, PlatformOs::Host)
}

/// Sanitize the last path component (the series folder name) per the org policy.
pub fn sanitize_folder_name(path: &Path, org: &OrganizationConfig) -> PathBuf {
    jumbie_shared::paths::sanitize_folder_name(path, org, PlatformOs::Host)
}

/// Reject a VERBATIM (custom) path whose components contain characters the
/// host filesystem cannot store.
///
/// Root-derived folder names are sanitized by [`sanitize_folder_name`] before
/// touching disk, but explicit custom paths are honored verbatim — so they
/// must already be legal on the host. On Windows/NTFS this rejects `<>:"\|?*`
/// in any component with a clear error instead of a raw mkdir failure that
/// would surface as an opaque 500; on Unix it is effectively a no-op. SSoT
/// shared by `create_series` (via `resolve_series_folder_path`) and the
/// validate-path endpoint, so the preview matches what creation will do.
pub fn check_host_legal_components(path: &Path) -> Result<(), String> {
    if let Some((component, ch)) =
        jumbie_shared::paths::first_host_illegal_component(path, PlatformOs::Host)
    {
        return Err(format!(
            "Path contains a character not allowed on this filesystem ('{}' in '{}')",
            ch, component
        ));
    }
    Ok(())
}

/// Resolve the concrete filesystem path for a mapping (template + policy).
pub fn mapping_path(mapping: &MappingRule, org: &OrganizationConfig) -> PathBuf {
    jumbie_shared::paths::mapping_path(mapping, org, PlatformOs::Host)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn org_with(policy: InvalidCharPolicy) -> OrganizationConfig {
        OrganizationConfig {
            illegal_char_policy: policy,
            ..Default::default()
        }
    }

    /// Facade == shared engine with `PlatformOs::Host` (guards signature drift).
    #[test]
    fn facade_delegates_to_shared_with_host_os() {
        let org = org_with(InvalidCharPolicy::Underscore);
        let title = "Show: The Best?";
        assert_eq!(
            sanitize_title(title, &org),
            jumbie_shared::paths::sanitize_title(title, &org, PlatformOs::Host)
        );
        assert_eq!(
            sanitize_with_policy(title, &InvalidCharPolicy::Underscore, false),
            jumbie_shared::paths::sanitize_with_policy(
                title,
                &InvalidCharPolicy::Underscore,
                false,
                PlatformOs::Host
            )
        );
        assert_eq!(
            resolve_template("/media/${series}", title, &org),
            jumbie_shared::paths::resolve_template(
                "/media/${series}",
                title,
                &org,
                PlatformOs::Host
            )
        );
    }

    #[test]
    fn check_host_legal_components_is_host_gated() {
        // `:` is legal on Unix but ALWAYS illegal on Windows (NTFS). The check
        // is asserted at RUNTIME so it runs on every platform.
        let custom = Path::new("organized/Custom: Path");
        if cfg!(target_os = "windows") {
            assert!(check_host_legal_components(custom).is_err());
        } else {
            assert!(check_host_legal_components(custom).is_ok());
        }
    }

    #[test]
    fn facade_mapping_path_matches_shared() {
        let org = org_with(InvalidCharPolicy::Underscore);
        let mapping = MappingRule {
            target_title: "Show: The Best?".to_string(),
            settings: jumbie_shared::types::SeriesSettings {
                path: Some("/media/tv/${series}".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(
            mapping_path(&mapping, &org),
            jumbie_shared::paths::mapping_path(&mapping, &org, PlatformOs::Host)
        );
    }
}
