//! `paths` — the single source of truth for ALL series path logic.
//!
//! Every site — backend or frontend — that derives, sanitizes, or
//! collision-resolves a series path funnels through this module:
//!
//! - **Illegal-character policy** — [`sanitize_with_policy`], plus
//!   [`sanitize_title`] / [`sanitize_folder_name`] for series names.
//! - **`${series}` template resolution** — [`resolve_template`].
//! - **Folder-collision handling** — [`resolve_folder_collision`], driven by
//!   `collision_handling` + `collision_rename_suffix` and backed by the unified
//!   suffix engine [`next_free_suffixed_path`].
//! - **Mapping → concrete path** — [`mapping_path`].
//!
//! Synchronous and config-parameterized: callers pass `&OrganizationConfig`
//! (fetched fresh), and the module never touches `AppState` or the DB. Async
//! orchestration (hidden-series reclaim, visible-claim rejection) composes these
//! primitives in `api_routes::series::helpers`.
//!
//! ## OS awareness
//!
//! The illegal-char policy depends on the OS the FILES live on. The backend
//! passes [`PlatformOs::Host`]; the frontend (WASM) must pass the SERVER's OS
//! explicitly (`PlatformOs::Windows`/`PlatformOs::Unix`) so the preview matches
//! the backend. `patterns.rs` remains the low-level regex/character table.

use crate::config::organization::{CollisionRenameSuffix, InvalidCharPolicy, OrganizationConfig};
use crate::patterns::{ILLEGAL_CHARS, PlatformOs, platform_safe_chars};
use crate::types::MappingRule;
use std::path::{Path, PathBuf};

// Illegal-character policy engine

/// Return the Unicode look-alike for a printable filesystem-illegal character.
/// Control characters (`\x00`–`\x1f`) return `None` — they have no visual
/// counterpart and are always removed.
///
/// Derived from `patterns::ILLEGAL_CHARS` (the SSoT).
pub fn unicode_lookalike(c: char) -> Option<&'static str> {
    ILLEGAL_CHARS
        .iter()
        .find(|&&(ch, _, _)| ch == c)
        .map(|&(_, lookalike, _)| lookalike)
}

/// Sanitize a name according to the configured illegal character policy.
///
/// This is the SSoT for applying `InvalidCharPolicy` — every code path that
/// preserves original filenames or builds a folder name from user input calls
/// this (via [`sanitize_title`]) instead of using `patterns::INVALID_CHARS`
/// directly.
///
/// `os` decides which characters are "platform-specific" when
/// `allow_platform_specific` is enabled. The backend passes
/// [`PlatformOs::Host`]; callers that preview paths for another machine
/// (the frontend previewing the server's filesystem) pass that OS explicitly.
pub fn sanitize_with_policy(
    filename: &str,
    policy: &InvalidCharPolicy,
    allow_platform_specific: bool,
    os: PlatformOs,
) -> String {
    let re: &regex::Regex = if allow_platform_specific {
        platform_safe_chars(os)
    } else {
        &crate::patterns::INVALID_CHARS
    };

    let result = re.replace_all(filename, |caps: &regex::Captures| {
        let c = caps[0].chars().next().unwrap_or('\0');
        match policy {
            InvalidCharPolicy::Unicode => unicode_lookalike(c).unwrap_or("").to_string(),
            InvalidCharPolicy::Underscore => "_".to_string(),
            InvalidCharPolicy::Hyphen => "-".to_string(),
            InvalidCharPolicy::Space => " ".to_string(),
            InvalidCharPolicy::Remove => String::new(),
        }
    });

    let result = result.to_string();

    if result.is_empty() {
        "_invalid_".to_string()
    } else {
        result
    }
}

/// First path component containing a character the given OS cannot store in a
/// filename, if any.
///
/// Verbatim custom paths are stored as-is (no sanitization), so they must
/// already be legal on the filesystem they will be created on. Returns the
/// offending `(component, character)` pair. Effectively never matches on Unix:
/// `/` cannot appear inside a single path component, and control characters
/// are rejected consistently with [`sanitize_with_policy`]'s universal
/// control-char removal.
pub fn first_host_illegal_component(path: &Path, os: PlatformOs) -> Option<(String, char)> {
    let re = platform_safe_chars(os);
    for component in path.components() {
        if let std::path::Component::Normal(name) = component {
            let name_str = name.to_string_lossy();
            if let Some(m) = re.find(&name_str) {
                return Some((
                    name_str.to_string(),
                    m.as_str().chars().next().unwrap_or('\0'),
                ));
            }
        }
    }
    None
}

/// Sanitize a series title per the org illegal-char policy, for use when
/// building a folder path from the raw title.
pub fn sanitize_title(title: &str, org: &OrganizationConfig, os: PlatformOs) -> String {
    sanitize_with_policy(
        title,
        &org.illegal_char_policy,
        org.allow_platform_specific_chars,
        os,
    )
}

/// Resolve a stored series path template and sanitize the `${series}` value.
/// Sanitizing the substituted title (not the whole path) keeps static path
/// components untouched and handles `${series}` appearing mid-path.
pub fn resolve_template(
    path_str: &str,
    title: &str,
    org: &OrganizationConfig,
    os: PlatformOs,
) -> PathBuf {
    let safe_title = sanitize_title(title, org, os);
    PathBuf::from(path_str.replace("${series}", &safe_title))
}

/// Sanitize the last path component (the series folder name) per the org
/// policy. Path separators are NOT handled here — callers that build the path
/// from user input must flatten those first (a `/` cannot exist inside a
/// single path component).
pub fn sanitize_folder_name(path: &Path, org: &OrganizationConfig, os: PlatformOs) -> PathBuf {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return path.to_path_buf();
    };
    let sanitized = sanitize_title(name, org, os);
    if sanitized == name {
        path.to_path_buf()
    } else {
        path.parent()
            .unwrap_or_else(|| Path::new("."))
            .join(sanitized)
    }
}

/// Split a filename into `(stem, extension)` for collision purposes.
///
/// A language tag is folded into the extension for **video and subtitle** files
/// (`Show.en.mkv` / `Show.en.srt` → `Show` + `.en.mkv` / `.en.srt`), so a counter
/// inserted there lands *before* the meaningful tag (`Show.001.en.mkv`) instead of
/// orphaning it (`Show.en.001.mkv`). `.nfo` is deliberately excluded (its name is
/// metadata, not an episode artifact).
///
/// The tag is validated against [`crate::languages::is_language_tag`], not merely
/// "2–3 letters", so release tokens (`HDR`, `DV`, `AAC`, `WEB`) and numeric
/// segments (`001`, `2019`) still split at the last dot.
fn split_stem_ext(file_name: &str) -> (&str, Option<&str>) {
    let Some(dot) = file_name.rfind('.') else {
        return (file_name, None);
    };
    let ext = &file_name[dot + 1..];
    let foldable =
        crate::media_format::is_video_ext(ext) || crate::media_format::is_subtitle_ext(ext);
    if foldable && let Some(prev_dot) = file_name[..dot].rfind('.') {
        let tag = &file_name[prev_dot + 1..dot];
        if crate::languages::is_language_tag(tag) {
            return (&file_name[..prev_dot], Some(&file_name[prev_dot..]));
        }
    }
    (&file_name[..dot], Some(&file_name[dot..]))
}

/// Detect and strip a [`CollisionRenameSuffix`] from a path, returning the path
/// without the suffix and which variant was detected.
///
/// Reverses the naming logic in [`next_free_suffixed_path`]. Returns `None` if
/// no collision suffix is detected. Only a single trailing suffix is removed —
/// callers that need to collapse a runaway chain (`Episode.001.001`) call this
/// repeatedly (see [`next_free_suffixed_path`]).
pub fn strip_collision_suffix(path: &Path) -> Option<(PathBuf, CollisionRenameSuffix)> {
    let filename = path.file_name()?.to_str()?;
    let parent = path.parent()?;

    // Split into stem and extension. Video/subtitle language tags are kept in
    // the extension (`.en.srt`), so a counter before the tag is stripped correctly.
    let (stem, ext) = split_stem_ext(filename);

    // Helper: rebuild a PathBuf from a stripped stem plus the original extension.
    let rebuild = |base_stem: &str| -> PathBuf {
        match ext {
            Some(ext_str) => parent.join(format!("{}{}", base_stem, ext_str)),
            None => parent.join(base_stem),
        }
    };

    // ParenNumeric: stem ends with ` (digits)`. The parentheses make the
    // pattern unambiguous, so no digit-adjacency guard is needed.
    if let Some(rest) = stem.strip_suffix(')')
        && let Some(open_idx) = rest.rfind('(')
        && open_idx > 0
        && rest.as_bytes()[open_idx - 1] == b' '
        && !rest[open_idx + 1..].is_empty()
        && rest[open_idx + 1..].bytes().all(|b| b.is_ascii_digit())
    {
        let base = &rest[..open_idx - 1];
        return Some((rebuild(base), CollisionRenameSuffix::ParenNumeric));
    }

    // UnderscoreNumeric: stem ends with `_` + exactly 3 digits.
    if let Some(base) = strip_sep_3_digits(stem, '_') {
        return Some((rebuild(base), CollisionRenameSuffix::UnderscoreNumeric));
    }

    // DashNumeric: stem ends with `-` + 1+ digits.
    if let Some(dash_idx) = stem.rfind('-') {
        let after = &stem[dash_idx + 1..];
        if !after.is_empty() && after.bytes().all(|b| b.is_ascii_digit()) {
            let base = &stem[..dash_idx];
            return Some((rebuild(base), CollisionRenameSuffix::DashNumeric));
        }
    }

    // DotNumeric. Two sub-cases because `.000` is ambiguous with an extension:
    //
    //   a) With a real extension → the suffix lives inside the stem
    //      (e.g. "video.001.mkv" → stem="video.001", ext=".mkv").
    //   b) Without one → the suffix IS the "extension"
    //      (e.g. "video.001" → stem="video", ext=".001").
    if let Some(base) = strip_sep_3_digits(stem, '.') {
        return Some((rebuild(base), CollisionRenameSuffix::DotNumeric));
    }
    if let Some(ext_str) = ext
        && let Some(suffix) = ext_str.strip_prefix('.')
        && suffix.len() == 3
        && suffix.bytes().all(|b| b.is_ascii_digit())
    {
        return Some((parent.join(stem), CollisionRenameSuffix::DotNumeric));
    }

    None
}

/// If `s` ends with `separator` followed by exactly 3 ASCII digits, returns the
/// portion before that suffix. Otherwise returns `None`.
fn strip_sep_3_digits(s: &str, separator: char) -> Option<&str> {
    let bytes = s.as_bytes();
    let len = bytes.len();
    if len < 4 {
        return None;
    }
    let start = len - 4;
    if bytes[start] as char == separator
        && bytes[start + 1].is_ascii_digit()
        && bytes[start + 2].is_ascii_digit()
        && bytes[start + 3].is_ascii_digit()
    {
        Some(&s[..start])
    } else {
        None
    }
}

/// Strip every trailing collision suffix that matches `variant`, collapsing a
/// runaway chain (`Episode.001.001.mkv` → `Episode.mkv`). Bounded to guard
/// against a pathological input; returns the path unchanged when nothing matches.
///
/// Matching only `variant` matters: under `DotNumeric` a legitimately named
/// `-1` part or `(2019)` year is left alone, and vice versa.
pub fn strip_collision_suffixes_of(path: &Path, variant: CollisionRenameSuffix) -> PathBuf {
    let mut current = path.to_path_buf();
    for _ in 0..64 {
        match strip_collision_suffix(&current) {
            Some((stripped, detected)) if detected == variant => current = stripped,
            _ => break,
        }
    }
    current
}

/// Unified collision-suffix engine: append a numeric suffix until a free name
/// is found.
///
/// `target` is the **intended** (template-derived) name and the counter is always
/// appended to it — the engine never reinterprets a trailing `001`/` (1)` in the
/// target as a counter of its own. That keeps a template that legitimately renders
/// such a name (`... OVA (1)`) intact and prevents nesting. Callers must therefore
/// pass the clean intended name; a name already carrying a counter is rotated
/// back to its clean form upstream (see `auxiliary_suffix`, which strips the
/// counter with [`strip_collision_suffixes_of`] before the engine runs).
///
/// `split_ext = true` treats the extension as the split point (files:
/// `Episode.001.mkv`, and `.en.srt` keeps its language tag in the extension so the
/// counter reads `Episode.001.en.srt`); `false` treats the whole name as the stem
/// (directories: `Dr. Jekyll` → `Dr. Jekyll (1)`, never `Dr (1).Jekyll`).
///
/// `is_occupied` decides whether a candidate is taken — callers pass disk
/// existence and/or in-memory claim sets (series mappings, batch destinations).
///
/// Capped at 999: past this, something is fundamentally wrong (runaway loop /
/// collapsed naming template), and variable-width counters would break
/// lexicographic sorting.
pub fn next_free_suffixed_path<F>(
    target: &Path,
    suffix: CollisionRenameSuffix,
    split_ext: bool,
    is_occupied: F,
) -> Option<PathBuf>
where
    F: Fn(&Path) -> bool,
{
    let file_name = target
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("name");

    let (stem, ext) = if split_ext {
        split_stem_ext(file_name)
    } else {
        (file_name, None)
    };
    let parent = target.parent().unwrap_or_else(|| Path::new("."));

    let mut counter = 1;
    loop {
        if counter > 999 {
            return None;
        }
        let new_name = match suffix {
            CollisionRenameSuffix::DotNumeric => match ext {
                Some(e) => format!("{}.{:03}{}", stem, counter, e),
                None => format!("{}.{:03}", stem, counter),
            },
            CollisionRenameSuffix::ParenNumeric => match ext {
                Some(e) => format!("{} ({}){}", stem, counter, e),
                None => format!("{} ({})", stem, counter),
            },
            CollisionRenameSuffix::UnderscoreNumeric => match ext {
                Some(e) => format!("{}_{:03}{}", stem, counter, e),
                None => format!("{}_{:03}", stem, counter),
            },
            CollisionRenameSuffix::DashNumeric => match ext {
                Some(e) => format!("{}-{}{}", stem, counter, e),
                None => format!("{}-{}", stem, counter),
            },
        };
        let new_path = parent.join(&new_name);
        if !is_occupied(&new_path) {
            return Some(new_path);
        }
        counter += 1;
    }
}

/// Resolve a series *folder* collision at `path` according to the org config.
///
/// This is the SSoT for "which folder will a new series actually land in" —
/// `create_series`, `update_series`, the `validate-path` endpoint, and batch
/// move all delegate here, so previews and execution always agree.
///
/// Semantics (apply when `path` already exists on disk and is NOT claimed by
/// another series — visible-series claims are rejected before this is called):
/// - `collision_handling = "skip"` → `Err` (creation should be refused).
/// - `collision_handling = "overwrite"` → used as-is (existing folder claimed).
/// - anything else ("rename", the default) → next free suffixed folder name.
///
/// `is_claimed` reports whether a candidate path is claimed by another series
/// mapping — the rename loop must not pick a name that collides with an
/// existing series even if that folder doesn't exist on disk yet.
///
/// `path` must already be sanitized via [`sanitize_folder_name`] when the policy
/// applies. Callers that need the sanitized name for claim checks sanitize first,
/// then resolve.
pub fn resolve_folder_collision<F>(
    path: &Path,
    org: &OrganizationConfig,
    is_claimed: F,
) -> Result<PathBuf, String>
where
    F: Fn(&Path) -> bool,
{
    if !path.exists() {
        return Ok(path.to_path_buf());
    }
    match org.collision_handling.as_str() {
        "skip" => Err(format!(
            "A folder already exists at '{}' and collision handling is set to 'skip'",
            path.display()
        )),
        "overwrite" => Ok(path.to_path_buf()),
        _ => next_free_suffixed_path(path, org.collision_rename_suffix, false, |p| {
            p.exists() || is_claimed(p)
        })
        .ok_or_else(|| {
            format!(
                "No free folder name available near '{}' (more than 999 collisions)",
                path.display()
            )
        }),
    }
}

/// Resolve the concrete filesystem path for a mapping: `${series}` template
/// resolution with policy sanitization. Returns an empty path when
/// `settings.path` is unset (should not happen post-migration).
///
/// SSoT: every site that needs a series' real folder uses this, so template
/// resolution can never bypass the illegal-char policy.
pub fn mapping_path(mapping: &MappingRule, org: &OrganizationConfig, os: PlatformOs) -> PathBuf {
    match &mapping.settings.path {
        Some(p) if !p.is_empty() => resolve_template(p, &mapping.target_title, org, os),
        _ => PathBuf::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::organization::InvalidCharPolicy;

    fn org_with(policy: InvalidCharPolicy) -> OrganizationConfig {
        OrganizationConfig {
            illegal_char_policy: policy,
            ..Default::default()
        }
    }

    // sanitize_with_policy

    #[test]
    fn test_sanitize_with_policy_underscore() {
        assert_eq!(
            sanitize_with_policy(
                "file:name?<bad>",
                &InvalidCharPolicy::Underscore,
                false,
                PlatformOs::Host
            ),
            "file_name__bad_"
        );
    }

    #[test]
    fn test_sanitize_with_policy_unicode() {
        // ? becomes U+FF1F (？), : becomes U+FF1A (：), < becomes U+FF1C (＜), > becomes U+FF1E (＞)
        let result =
            sanitize_with_policy("?", &InvalidCharPolicy::Unicode, false, PlatformOs::Host);
        assert_eq!(
            result, "\u{FF1F}",
            "question mark should become fullwidth question mark"
        );

        let result =
            sanitize_with_policy(":", &InvalidCharPolicy::Unicode, false, PlatformOs::Host);
        assert_eq!(result, "\u{FF1A}", "colon should become fullwidth colon");

        let result =
            sanitize_with_policy("<", &InvalidCharPolicy::Unicode, false, PlatformOs::Host);
        assert_eq!(
            result, "\u{FF1C}",
            "less-than should become fullwidth less-than"
        );

        let result =
            sanitize_with_policy(">", &InvalidCharPolicy::Unicode, false, PlatformOs::Host);
        assert_eq!(
            result, "\u{FF1E}",
            "greater-than should become fullwidth greater-than"
        );

        let result =
            sanitize_with_policy("*", &InvalidCharPolicy::Unicode, false, PlatformOs::Host);
        assert_eq!(
            result, "\u{FF0A}",
            "asterisk should become fullwidth asterisk"
        );
    }

    #[test]
    fn test_sanitize_with_policy_hyphen() {
        assert_eq!(
            sanitize_with_policy(
                "file:name?",
                &InvalidCharPolicy::Hyphen,
                false,
                PlatformOs::Host
            ),
            "file-name-"
        );
    }

    #[test]
    fn test_sanitize_with_policy_space() {
        assert_eq!(
            sanitize_with_policy(
                "file:name?",
                &InvalidCharPolicy::Space,
                false,
                PlatformOs::Host
            ),
            "file name "
        );
    }

    #[test]
    fn test_sanitize_with_policy_remove() {
        assert_eq!(
            sanitize_with_policy(
                "file:name?",
                &InvalidCharPolicy::Remove,
                false,
                PlatformOs::Host
            ),
            "filename"
        );
    }

    #[test]
    fn test_sanitize_with_policy_control_chars_always_removed() {
        // Control chars are always invalid regardless of policy — Unicode
        // lookalikes have no visual counterpart.
        assert_eq!(
            sanitize_with_policy(
                "a\x00b",
                &InvalidCharPolicy::Unicode,
                true,
                PlatformOs::Host
            ),
            "ab"
        );
    }

    #[test]
    fn test_sanitize_with_policy_empty_result_fallback() {
        let result =
            sanitize_with_policy("***", &InvalidCharPolicy::Remove, false, PlatformOs::Host);
        assert_eq!(result, "_invalid_");
    }

    #[test]
    fn test_sanitize_with_policy_platform_specific_host_dependent() {
        // When allow_platform_specific=true, chars valid on the HOST OS are preserved.
        // PlatformOs::Host resolves at runtime via cfg!, so this test runs on
        // every platform and asserts per the current target.
        let on_windows = cfg!(target_os = "windows");

        let r = sanitize_with_policy("?", &InvalidCharPolicy::Underscore, true, PlatformOs::Host);
        if on_windows {
            assert_eq!(r, "_", "? should be sanitized on Windows");
        } else {
            assert_eq!(r, "?", "? should be preserved on Linux/macOS");
        }

        let r = sanitize_with_policy("*", &InvalidCharPolicy::Underscore, true, PlatformOs::Host);
        if on_windows {
            assert_eq!(r, "_", "* should be sanitized on Windows");
        } else {
            assert_eq!(r, "*", "* should be preserved on Linux/macOS");
        }

        let r = sanitize_with_policy(":", &InvalidCharPolicy::Underscore, true, PlatformOs::Host);
        if on_windows {
            assert_eq!(r, "_", ": should be sanitized on Windows");
        } else {
            assert_eq!(r, ":", ": should be preserved on Linux/macOS");
        }

        // '/' is a path separator on ALL platforms — always filtered
        assert_eq!(
            sanitize_with_policy(
                "a/b",
                &InvalidCharPolicy::Underscore,
                true,
                PlatformOs::Host
            ),
            "a_b",
            "/ should be filtered on all platforms"
        );

        let r = sanitize_with_policy(
            "a\\b",
            &InvalidCharPolicy::Underscore,
            true,
            PlatformOs::Host,
        );
        if on_windows {
            assert_eq!(r, "a_b", "backslash should be filtered on Windows");
        } else {
            assert_eq!(r, "a\\b", "backslash should be preserved on Linux/macOS");
        }
    }

    // Explicit-OS gating (runs on every platform, runtime-branched)

    #[test]
    fn test_sanitize_with_policy_windows_preserves_nothing() {
        // On Windows, the platform-safe set == the strictest set: every
        // ILLEGAL_CHARS entry is filtered even with allow_platform_specific.
        for c in &['<', '>', ':', '\"', '/', '\\', '|', '?', '*'] {
            let input = c.to_string();
            let r = sanitize_with_policy(
                &input,
                &InvalidCharPolicy::Underscore,
                true,
                PlatformOs::Windows,
            );
            assert_eq!(r, "_", "Windows must sanitize '{}'", c);
        }
        // Control chars always removed under Unicode policy (no lookalike).
        assert_eq!(
            sanitize_with_policy(
                "a\x00b",
                &InvalidCharPolicy::Unicode,
                true,
                PlatformOs::Windows
            ),
            "ab"
        );
    }

    #[test]
    fn test_sanitize_with_policy_unix_preserves_windows_only_chars() {
        for c in &['<', '>', ':', '\"', '\\', '|', '?', '*'] {
            let input = c.to_string();
            let r = sanitize_with_policy(
                &input,
                &InvalidCharPolicy::Underscore,
                true,
                PlatformOs::Unix,
            );
            assert_eq!(r, input, "Unix must preserve '{}'", c);
        }
        // '/' is a path separator on Unix too — always filtered.
        assert_eq!(
            sanitize_with_policy("/", &InvalidCharPolicy::Underscore, true, PlatformOs::Unix),
            "_"
        );
        assert_eq!(
            sanitize_with_policy(
                "a\x00b",
                &InvalidCharPolicy::Unicode,
                true,
                PlatformOs::Unix
            ),
            "ab"
        );
    }

    #[test]
    fn first_host_illegal_component_is_os_aware() {
        // `:` is illegal on Windows (NTFS) but legal on Unix — the same path
        // must be rejected for Windows and accepted for Unix.
        let custom = Path::new("organized/Custom: Path");
        assert_eq!(
            first_host_illegal_component(custom, PlatformOs::Windows),
            Some(("Custom: Path".to_string(), ':'))
        );
        assert_eq!(first_host_illegal_component(custom, PlatformOs::Unix), None);

        // Path separators never count — components cannot contain them.
        assert_eq!(
            first_host_illegal_component(Path::new("/media/TV/Brand New"), PlatformOs::Windows),
            None
        );
    }

    #[test]
    fn test_platform_safe_chars_windows_equals_strict_set() {
        // Optimization guard: on Windows the platform-safe regex is the same
        // object as INVALID_CHARS — the strictest set.
        assert!(std::ptr::eq(
            crate::patterns::platform_safe_chars(PlatformOs::Windows),
            &*crate::patterns::INVALID_CHARS
        ));
    }

    #[test]
    fn test_platform_safe_chars_runtime_selection_matches_os() {
        // Behavioral check of runtime selection (not compile-time cfg!).
        let unix_re = crate::patterns::platform_safe_chars(PlatformOs::Unix);
        assert!(unix_re.is_match("/"));
        assert!(!unix_re.is_match("?"));
        assert!(!unix_re.is_match("\\"));

        let win_re = crate::patterns::platform_safe_chars(PlatformOs::Windows);
        assert!(win_re.is_match("/"));
        assert!(win_re.is_match("?"));
        assert!(win_re.is_match("\\"));

        // Host resolves to the current target — assert the per-target contract.
        let host_re = crate::patterns::platform_safe_chars(PlatformOs::Host);
        if cfg!(target_os = "windows") {
            assert!(host_re.is_match("?"));
            assert!(host_re.is_match("\\"));
        } else {
            assert!(!host_re.is_match("?"));
            assert!(!host_re.is_match("\\"));
        }
    }

    #[test]
    fn test_sanitize_with_policy_unicode_lookalike_mapping() {
        // Verify all printable invalid chars have a look-alike
        for c in &['<', '>', ':', '\"', '/', '\\', '|', '?', '*'] {
            let input = c.to_string();
            let result =
                sanitize_with_policy(&input, &InvalidCharPolicy::Unicode, false, PlatformOs::Host);
            assert!(
                !result.is_empty(),
                "Unicode look-alike for '{}' should not be empty",
                c
            );
            assert_ne!(
                result, input,
                "Unicode look-alike for '{}' should differ from input",
                c
            );
        }
    }

    #[test]
    fn test_add_series_preview_contract_matches_server_os() {
        // The Add Series default-path preview routes the trimmed series name
        // through this exact call. `/` is a path separator everywhere, so it is
        // always filtered per policy; `\\` is preserved on a Unix server when
        // allow_platform_specific_chars=true.
        let org = org_with(InvalidCharPolicy::Underscore);
        let mut org = org;
        org.allow_platform_specific_chars = true;

        let name = "My/Show\\Name";
        assert_eq!(
            sanitize_with_policy(
                name,
                &org.illegal_char_policy,
                org.allow_platform_specific_chars,
                PlatformOs::Unix
            ),
            "My_Show\\Name"
        );
        assert_eq!(
            sanitize_with_policy(
                name,
                &org.illegal_char_policy,
                org.allow_platform_specific_chars,
                PlatformOs::Windows
            ),
            "My_Show_Name"
        );
        // Default config (cross-platform safe set): both separators filtered.
        let default_org = OrganizationConfig::default();
        assert_eq!(
            sanitize_with_policy(
                name,
                &default_org.illegal_char_policy,
                default_org.allow_platform_specific_chars,
                PlatformOs::Unix
            ),
            "My_Show_Name"
        );
    }

    // sanitize_title / resolve_template

    #[test]
    fn test_sanitize_title_follows_policy() {
        let org = org_with(InvalidCharPolicy::Underscore);
        assert_eq!(
            sanitize_title("Show: The Best?", &org, PlatformOs::Host),
            "Show_ The Best_"
        );

        let org = org_with(InvalidCharPolicy::Space);
        assert_eq!(
            sanitize_title("Show: The Best?", &org, PlatformOs::Host),
            "Show  The Best "
        );
    }

    #[test]
    fn test_sanitize_title_respects_server_os() {
        // The same title sanitizes differently depending on the filesystem's OS.
        let org = org_with(InvalidCharPolicy::Underscore);
        let mut org = org;
        org.allow_platform_specific_chars = true;

        assert_eq!(
            sanitize_title("Show: The Best?", &org, PlatformOs::Unix),
            "Show: The Best?"
        );
        assert_eq!(
            sanitize_title("Show: The Best?", &org, PlatformOs::Windows),
            "Show_ The Best_"
        );
    }

    #[test]
    fn test_resolve_template_sanitizes_series_value() {
        let org = org_with(InvalidCharPolicy::Underscore);
        let resolved = resolve_template(
            "/media/tv/${series}",
            "Show: The Best?",
            &org,
            PlatformOs::Host,
        );
        assert_eq!(resolved, PathBuf::from("/media/tv/Show_ The Best_"));
    }

    #[test]
    fn test_resolve_template_handles_mid_path_series() {
        let org = org_with(InvalidCharPolicy::Underscore);
        let resolved = resolve_template("/media/${series}/shows", "A: B", &org, PlatformOs::Host);
        assert_eq!(resolved, PathBuf::from("/media/A_ B/shows"));
    }

    #[test]
    fn test_resolve_template_leaves_static_components_untouched() {
        let org = org_with(InvalidCharPolicy::Underscore);
        // Static components with legal-but-exotic chars are NOT sanitized —
        // only the ${series} substitution value is.
        let resolved = resolve_template(
            "/media/TV: Shows/${series}",
            "Clean",
            &org,
            PlatformOs::Host,
        );
        assert_eq!(resolved, PathBuf::from("/media/TV: Shows/Clean"));
    }

    // next_free_suffixed_path

    #[test]
    fn test_next_free_file_preserves_extension() {
        let path = Path::new("/media/Episode.mkv");
        let free =
            next_free_suffixed_path(path, CollisionRenameSuffix::DotNumeric, true, |_| false)
                .unwrap();
        assert_eq!(free, PathBuf::from("/media/Episode.001.mkv"));

        let free =
            next_free_suffixed_path(path, CollisionRenameSuffix::ParenNumeric, true, |_| false)
                .unwrap();
        assert_eq!(free, PathBuf::from("/media/Episode (1).mkv"));
    }

    #[test]
    fn test_next_free_file_walks_past_taken_names() {
        let path = Path::new("/media/Episode.mkv");
        let taken: std::collections::HashSet<PathBuf> = [
            PathBuf::from("/media/Episode.001.mkv"),
            PathBuf::from("/media/Episode.002.mkv"),
        ]
        .into_iter()
        .collect();
        let free = next_free_suffixed_path(path, CollisionRenameSuffix::DotNumeric, true, |p| {
            taken.contains(p)
        })
        .unwrap();
        assert_eq!(free, PathBuf::from("/media/Episode.003.mkv"));
    }

    #[test]
    fn test_next_free_supports_all_suffix_formats() {
        let path = Path::new("/media/Episode.mkv");
        let cases = [
            (CollisionRenameSuffix::DotNumeric, "/media/Episode.001.mkv"),
            (
                CollisionRenameSuffix::ParenNumeric,
                "/media/Episode (1).mkv",
            ),
            (
                CollisionRenameSuffix::UnderscoreNumeric,
                "/media/Episode_001.mkv",
            ),
            (CollisionRenameSuffix::DashNumeric, "/media/Episode-1.mkv"),
        ];
        for (suffix, expected) in cases {
            let free = next_free_suffixed_path(path, suffix, true, |_| false).unwrap();
            assert_eq!(free, PathBuf::from(expected));
        }

        // Directory mode (no extension split) for the same formats.
        let dir = Path::new("/media/Dr. Jekyll");
        let dir_cases = [
            (CollisionRenameSuffix::DotNumeric, "/media/Dr. Jekyll.001"),
            (CollisionRenameSuffix::ParenNumeric, "/media/Dr. Jekyll (1)"),
            (
                CollisionRenameSuffix::UnderscoreNumeric,
                "/media/Dr. Jekyll_001",
            ),
            (CollisionRenameSuffix::DashNumeric, "/media/Dr. Jekyll-1"),
        ];
        for (suffix, expected) in dir_cases {
            let free = next_free_suffixed_path(dir, suffix, false, |_| false).unwrap();
            assert_eq!(free, PathBuf::from(expected));
        }
    }

    #[test]
    fn test_next_free_folder_whole_name_is_stem() {
        // A directory named "Dr. Jekyll" must not have its last dot treated as
        // an extension separator — the whole name is the stem.
        let path = Path::new("/media/Dr. Jekyll");
        let free =
            next_free_suffixed_path(path, CollisionRenameSuffix::ParenNumeric, false, |_| false)
                .unwrap();
        assert_eq!(free, PathBuf::from("/media/Dr. Jekyll (1)"));

        let free =
            next_free_suffixed_path(path, CollisionRenameSuffix::DotNumeric, false, |_| false)
                .unwrap();
        assert_eq!(free, PathBuf::from("/media/Dr. Jekyll.001"));
    }

    #[test]
    fn test_next_free_folder_walks_past_taken_suffixes() {
        let path = Path::new("/media/Show");
        let taken: std::collections::HashSet<PathBuf> = [
            PathBuf::from("/media/Show (1)"),
            PathBuf::from("/media/Show (2)"),
        ]
        .into_iter()
        .collect();
        let free = next_free_suffixed_path(path, CollisionRenameSuffix::ParenNumeric, false, |p| {
            taken.contains(p)
        })
        .unwrap();
        assert_eq!(free, PathBuf::from("/media/Show (3)"));
    }

    #[test]
    fn test_next_free_suffixed_path_returns_none_after_999() {
        let path = Path::new("/media/Show");
        // Every candidate is occupied → the loop must terminate at the cap.
        let result =
            next_free_suffixed_path(path, CollisionRenameSuffix::ParenNumeric, false, |_| true);
        assert!(result.is_none());
    }

    #[test]
    fn test_next_free_appends_to_intended_name_with_suffix_like_shape() {
        // The engine treats `target` as the intended name and never reinterprets a
        // trailing `(2019)` as a counter of its own — so a template that renders
        // such a name is preserved as the base.
        let path = Path::new("/media/Movie (2019).mkv");
        let occupied = PathBuf::from("/media/Movie (2019).mkv");
        let free = next_free_suffixed_path(path, CollisionRenameSuffix::DotNumeric, true, |p| {
            p == occupied
        })
        .unwrap();
        assert_eq!(free, PathBuf::from("/media/Movie (2019).001.mkv"));
    }

    #[test]
    fn test_strip_collision_suffix_detects_variants() {
        assert_eq!(
            strip_collision_suffix(Path::new("/media/Episode.001.mkv")),
            Some((
                PathBuf::from("/media/Episode.mkv"),
                CollisionRenameSuffix::DotNumeric
            ))
        );
        assert!(strip_collision_suffix(Path::new("/media/Episode.mkv")).is_none());
    }

    #[test]
    fn test_strip_collision_suffixes_collapses_chain() {
        assert_eq!(
            strip_collision_suffixes_of(
                Path::new("/media/Episode.001.001.001.mkv"),
                CollisionRenameSuffix::DotNumeric
            ),
            PathBuf::from("/media/Episode.mkv")
        );
        // The language tag survives the collapse.
        assert_eq!(
            strip_collision_suffixes_of(
                Path::new("/media/Show.001.001.en.srt"),
                CollisionRenameSuffix::DotNumeric
            ),
            PathBuf::from("/media/Show.en.srt")
        );
    }

    #[test]
    fn test_strip_collision_suffixes_of_respects_variant() {
        // `Movie (2).mkv` is a ParenNumeric counter, not a DotNumeric one.
        assert_eq!(
            strip_collision_suffixes_of(
                Path::new("/media/Movie (2).mkv"),
                CollisionRenameSuffix::DotNumeric
            ),
            PathBuf::from("/media/Movie (2).mkv")
        );
        assert_eq!(
            strip_collision_suffixes_of(
                Path::new("/media/Movie (2).mkv"),
                CollisionRenameSuffix::ParenNumeric
            ),
            PathBuf::from("/media/Movie.mkv")
        );
    }

    #[test]
    fn test_next_free_places_counter_before_language_tag() {
        // Video and subtitle language tags stay at the end of the name.
        let cases = [
            (
                "/media/Show - S01E01.en.srt",
                "/media/Show - S01E01.001.en.srt",
            ),
            (
                "/media/Show - S01E01.en.mkv",
                "/media/Show - S01E01.001.en.mkv",
            ),
        ];
        for (target, expected) in cases {
            let occupied = PathBuf::from(target);
            let free = next_free_suffixed_path(
                Path::new(target),
                CollisionRenameSuffix::DotNumeric,
                true,
                |p| p == occupied,
            )
            .unwrap();
            assert_eq!(free, PathBuf::from(expected), "{target}");
        }

        // `.nfo` is not an episode artifact: the counter stays after the tag.
        let path = Path::new("/media/Show - S01E01.en.nfo");
        let occupied = PathBuf::from("/media/Show - S01E01.en.nfo");
        let free = next_free_suffixed_path(path, CollisionRenameSuffix::DotNumeric, true, |p| {
            p == occupied
        })
        .unwrap();
        assert_eq!(free, PathBuf::from("/media/Show - S01E01.en.001.nfo"));
    }

    #[test]
    fn test_next_free_does_not_fold_release_tokens() {
        // Short release tokens are not language tags → the counter lands after them.
        for name in [
            "Movie.HDR.mkv",
            "Movie.DV.mkv",
            "Movie.AAC.mkv",
            "Movie.WEB.mkv",
        ] {
            let path = Path::new("/media").join(name);
            let occupied = path.clone();
            let free =
                next_free_suffixed_path(&path, CollisionRenameSuffix::DotNumeric, true, |p| {
                    p == occupied
                })
                .unwrap();
            let stem = name.strip_suffix(".mkv").unwrap();
            assert_eq!(free, PathBuf::from(format!("/media/{stem}.001.mkv")));
        }
    }

    #[test]
    fn test_next_free_increments_past_taken_counters() {
        // Callers pass the clean intended name; the engine walks the counters so
        // repeated collisions increment (`.001` …) instead of nesting.
        let cases = [
            (
                CollisionRenameSuffix::DotNumeric,
                "/media/Show.mkv",
                "/media/Show.001.mkv",
                "/media/Show.002.mkv",
            ),
            (
                CollisionRenameSuffix::ParenNumeric,
                "/media/Show.mkv",
                "/media/Show (1).mkv",
                "/media/Show (2).mkv",
            ),
            (
                CollisionRenameSuffix::UnderscoreNumeric,
                "/media/Show.mkv",
                "/media/Show_001.mkv",
                "/media/Show_002.mkv",
            ),
            (
                CollisionRenameSuffix::DashNumeric,
                "/media/Show.mkv",
                "/media/Show-1.mkv",
                "/media/Show-2.mkv",
            ),
        ];
        for (suffix, clean, taken_counter, expected) in cases {
            let taken: std::collections::HashSet<PathBuf> =
                [PathBuf::from(clean), PathBuf::from(taken_counter)]
                    .into_iter()
                    .collect();
            let free =
                next_free_suffixed_path(Path::new(clean), suffix, true, |p| taken.contains(p))
                    .unwrap();
            assert_eq!(free, PathBuf::from(expected), "variant {suffix:?}");
        }
    }

    #[test]
    fn test_strip_collision_suffix_language_tag_orders() {
        // New order (counter before the tag), for video and subtitle.
        assert_eq!(
            strip_collision_suffix(Path::new("/media/Show.001.en.srt")),
            Some((
                PathBuf::from("/media/Show.en.srt"),
                CollisionRenameSuffix::DotNumeric
            ))
        );
        assert_eq!(
            strip_collision_suffix(Path::new("/media/Show.001.en.mkv")),
            Some((
                PathBuf::from("/media/Show.en.mkv"),
                CollisionRenameSuffix::DotNumeric
            ))
        );
        // Legacy order (counter after the tag) is still recognised.
        assert_eq!(
            strip_collision_suffix(Path::new("/media/Show.en.001.srt")),
            Some((
                PathBuf::from("/media/Show.en.srt"),
                CollisionRenameSuffix::DotNumeric
            ))
        );
    }

    // resolve_folder_collision

    #[test]
    fn test_folder_collision_free_path_unchanged() {
        let org = OrganizationConfig::default();
        let path = std::env::temp_dir().join("jumbie_paths_free");
        let resolved = resolve_folder_collision(&path, &org, |_| false).unwrap();
        assert_eq!(resolved, path);
    }

    #[test]
    fn test_folder_collision_skip_rejects_existing() {
        let org = OrganizationConfig::default();
        let dir = std::env::temp_dir().join("jumbie_paths_skip");
        std::fs::create_dir_all(&dir).unwrap();

        let mut org = org;
        org.collision_handling = "skip".to_string();
        let err = resolve_folder_collision(&dir, &org, |_| false).unwrap_err();
        assert!(err.to_lowercase().contains("skip"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_folder_collision_overwrite_keeps_existing() {
        let dir = std::env::temp_dir().join("jumbie_paths_overwrite");
        std::fs::create_dir_all(&dir).unwrap();

        let org = OrganizationConfig {
            collision_handling: "overwrite".to_string(),
            ..Default::default()
        };
        let resolved = resolve_folder_collision(&dir, &org, |_| false).unwrap();
        assert_eq!(resolved, dir);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_folder_collision_rename_uses_configured_suffix() {
        let dir = std::env::temp_dir().join("jumbie_paths_rename");
        std::fs::create_dir_all(&dir).unwrap();

        let org = OrganizationConfig {
            collision_rename_suffix: CollisionRenameSuffix::ParenNumeric,
            ..Default::default()
        };
        let resolved = resolve_folder_collision(&dir, &org, |_| false).unwrap();
        assert_eq!(
            resolved.file_name().unwrap().to_string_lossy(),
            format!("{} (1)", dir.file_name().unwrap().to_string_lossy())
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    // mapping_path

    #[test]
    fn test_mapping_path_resolves_and_sanitizes_template() {
        let org = org_with(InvalidCharPolicy::Underscore);
        let mapping = MappingRule {
            target_title: "Show: The Best?".to_string(),
            settings: crate::mapping::SeriesSettings {
                path: Some("/media/tv/${series}".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };

        let path = mapping_path(&mapping, &org, PlatformOs::Host);
        assert_eq!(path, PathBuf::from("/media/tv/Show_ The Best_"));
    }

    #[test]
    fn test_mapping_path_concrete_path_unchanged() {
        let org = org_with(InvalidCharPolicy::Underscore);
        let mapping = MappingRule {
            target_title: "Show: The Best?".to_string(),
            settings: crate::mapping::SeriesSettings {
                path: Some("/media/tv/Show_ The Best_".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };

        let path = mapping_path(&mapping, &org, PlatformOs::Host);
        assert_eq!(path, PathBuf::from("/media/tv/Show_ The Best_"));
    }

    #[test]
    fn test_mapping_path_empty_path_returns_empty() {
        let org = OrganizationConfig::default();
        let mapping = MappingRule::default();
        assert!(
            mapping_path(&mapping, &org, PlatformOs::Host)
                .as_os_str()
                .is_empty()
        );
    }
}
