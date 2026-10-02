//! System-tray integration behind the `desktop` feature.
//!
//! `linux` speaks the freedesktop StatusNotifierItem protocol over the session
//! D-Bus (pure Rust via `ksni`), avoiding the GTK/appindicator stack that links
//! X11 libraries. `windows_macos` uses `tray-icon` + `tao` (Win32 / Cocoa).
//!
//! Shared labels, the "Open in Web" URL, and icon decoding live here so the two
//! backends cannot drift apart.
#![cfg(feature = "desktop")]

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::TrayApp;

#[cfg(not(target_os = "linux"))]
mod windows_macos;
#[cfg(not(target_os = "linux"))]
pub use windows_macos::{AppEvent, TrayApp};

// Shared tray content (SSoT for both backends)

/// Display name used as the item title and tooltip.
pub(crate) const APP_TITLE: &str = "Jumbie";
/// "Open in Web" menu item label.
pub(crate) const OPEN_WEB_LABEL: &str = "Open in Web";
/// "Quit" menu item label.
pub(crate) const QUIT_LABEL: &str = "Quit";

/// URL the "Open in Web" menu item opens. Uses the same port the API server
/// binds (see `crate::cli::server::api_port`).
pub(crate) fn open_web_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}")
}

/// Single tray icon PNG for the Win32/Cocoa backends (they show one image).
#[cfg(not(target_os = "linux"))]
pub(crate) const TRAY_ICON_PNG: &[u8] = include_bytes!("../../../frontend/icons/icon-32.png");

/// Tray icon PNGs for the SNI backend, ascending by size, so HiDPI panels can
/// pick a crisp source.
#[cfg(target_os = "linux")]
pub(crate) const ICON_PNGS: [&[u8]; 3] = [
    include_bytes!("../../../frontend/icons/icon-32.png").as_slice(),
    include_bytes!("../../../frontend/icons/icon-48.png").as_slice(),
    include_bytes!("../../../frontend/icons/icon-128.png").as_slice(),
];

/// Decode a PNG into `(width, height, RGBA8)`.
pub(crate) fn decode_rgba(png: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    let rgba = image::load_from_memory(png).ok()?.into_rgba8();
    let (width, height) = rgba.dimensions();
    Some((width, height, rgba.into_raw()))
}

/// Convert RGBA8 to ARGB32 (network byte order) in place, as required by the
/// StatusNotifierItem spec.
#[cfg(target_os = "linux")]
pub(crate) fn rgba_to_argb(rgba: &mut [u8]) {
    for pixel in rgba.as_chunks_mut::<4>().0 {
        pixel.rotate_right(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_web_url_uses_the_given_port() {
        assert_eq!(open_web_url(3000), "http://127.0.0.1:3000");
        assert_eq!(open_web_url(8080), "http://127.0.0.1:8080");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn rgba_to_argb_rotates_each_pixel() {
        let mut data = vec![1, 2, 3, 4, 5, 6, 7, 8];
        rgba_to_argb(&mut data);
        // [r, g, b, a] -> [a, r, g, b]
        assert_eq!(data, vec![4, 1, 2, 3, 8, 5, 6, 7]);
    }
}
