use crate::components::common::AboutInfo;
use jumbie_shared::patterns::PlatformOs;
use leptos::prelude::*;
use leptos::task::spawn_local;

// SSoT: OS-aware path placeholder examples. Every input that needs a
// platform-appropriate path example references these constants.

/// Series-level path (Add Series, Batch Move default, System Organized import,
/// Batch Move custom root).
pub const EXAMPLE_SERIES_PATH: &str = "path/to/series";

/// Parent-folder bulk-import path (System Organized).
pub const EXAMPLE_SERIES_PARENT_PATH: &str = "path/to/series_parent_folder";

/// Media-root path (Edit Series Path modal, System Organized new dest root).
pub const EXAMPLE_MEDIA_PATH: &str = "path/to/media";

/// Returns a reactive signal with the raw backend OS string (e.g. `"linux"`,
/// `"windows"`, `"macos"`). `None` until resolved. Resolution order:
///   1. Synchronous check of the `"server_os"` cache (fast path).
///   2. Synchronous check of the `"fetch_about"` cache (populated by background
///      preload in Wave 2).
///   3. Async fetch from `/api/system/about`.
pub fn use_server_os_name() -> Signal<Option<String>> {
    let (os, set_os) = signal(None);

    // Tier 1: synchronous cache hit
    if let Some(cached) = crate::utils::read_cache::<String>("server_os") {
        set_os.set(Some(cached));
    }
    // Tier 2: about info already preloaded
    else if let Some(about) = crate::utils::read_cache::<AboutInfo>("fetch_about") {
        set_os.set(Some(about.os.clone()));
        crate::utils::write_cache("server_os", &about.os);
    }
    // Tier 3: nothing cached yet — fetch once
    else {
        spawn_local(async move {
            if let Ok(info) = crate::api::fetch_about().await {
                crate::utils::write_cache("server_os", &info.os);
                set_os.set(Some(info.os));
            }
        });
    }

    os.into()
}

/// Returns a reactive signal that is `true` when the backend is running on Windows.
/// Caches the result in the shared API cache so subsequent components get it
/// without an extra network round-trip.
pub fn use_server_os() -> Signal<bool> {
    let os = use_server_os_name();
    Signal::derive(move || os.get().is_some_and(|o| o == "windows"))
}

/// Map the backend-reported OS string to a [`PlatformOs`] for the shared path
/// policy engine.
///
/// Anything that is not `"windows"` is treated as Unix — macOS, Linux and BSD
/// all share the same filename rules. `None` (not loaded yet) defaults to Unix,
/// which is correct for the default config (cross-platform safe set, where the
/// OS doesn't change the outcome); when `allow_platform_specific_chars` is on
/// and the server is Windows, the reactive [`use_platform_os`] signal corrects
/// the preview as soon as the OS arrives.
pub fn platform_os_from_server_name(os: Option<&str>) -> PlatformOs {
    match os {
        Some(o) if o.eq_ignore_ascii_case("windows") => PlatformOs::Windows,
        _ => PlatformOs::Unix,
    }
}

/// Returns a reactive signal with the [`PlatformOs`] of the BACKEND server.
///
/// The frontend must never use [`PlatformOs::Host`] — that would resolve to the
/// browser's OS (always Unix for WASM). Pass this signal into the shared path
/// policy engine so previews match what the backend will actually do.
pub fn use_platform_os() -> Signal<PlatformOs> {
    let os = use_server_os_name();
    Signal::derive(move || platform_os_from_server_name(os.get().as_deref()))
}

/// Returns the platform-appropriate example path for a single directory.
///
/// Used as `placeholder` text in path inputs. The distinction matters:
///   - Windows users expect `D:\Media\TV` — a Unix-style `/media/tv` is
///     confusing and implies they've misconfigured something.
///   - The example includes a realistic drive-letter prefix so first-time
///     Windows users immediately understand the expected format.
pub fn path_placeholder(is_windows: bool, example: &str) -> String {
    if is_windows {
        // Derive a Windows-style example: strip leading slashes and prepend D:\
        let stripped = example.trim_start_matches('/');
        let win_path = stripped.replace('/', "\\");
        format!("D:\\{}", win_path)
    } else {
        format!("/{}", example.trim_start_matches('/'))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unix_prepends_forward_slash() {
        assert_eq!(path_placeholder(false, "path/to/series"), "/path/to/series");
    }

    #[test]
    fn unix_strips_leading_slash() {
        assert_eq!(
            path_placeholder(false, "/path/to/series"),
            "/path/to/series"
        );
    }

    #[test]
    fn unix_handles_short_path() {
        assert_eq!(path_placeholder(false, "media/tv"), "/media/tv");
    }

    #[test]
    fn windows_converts_to_backslashes() {
        assert_eq!(
            path_placeholder(true, "path/to/series"),
            "D:\\path\\to\\series"
        );
    }

    #[test]
    fn windows_strips_leading_slash() {
        assert_eq!(
            path_placeholder(true, "/path/to/series"),
            "D:\\path\\to\\series"
        );
    }

    #[test]
    fn windows_handles_short_path() {
        assert_eq!(path_placeholder(true, "media/tv"), "D:\\media\\tv");
    }
}
