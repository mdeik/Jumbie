//! Linux system tray (Wayland-native).
//!
//! Implements the freedesktop [StatusNotifierItem] specification over the
//! session D-Bus using `ksni`. Unlike the GTK/appindicator stack this pulls in
//! no X11, GTK or `libxdo` libraries, so it works on pure Wayland sessions and
//! doesn't constrain the build to any system C library. Because SNI is a D-Bus
//! protocol it also serves modern X11 desktops (KDE, Cinnamon, XFCE with the
//! statusnotifier plugin, ...).
//!
//! [StatusNotifierItem]: https://www.freedesktop.org/wiki/Specifications/StatusNotifierItem/

use std::sync::LazyLock;

use anyhow::Result;
use ksni::{Icon, MenuItem, OfflineReason, ToolTip, Tray, TrayMethods, menu::StandardItem};
use tokio_util::sync::CancellationToken;

use super::{
    APP_TITLE, ICON_PNGS, OPEN_WEB_LABEL, QUIT_LABEL, decode_rgba, open_web_url, rgba_to_argb,
};

/// Well-known, session-stable identifier for this application.
const APP_ID: &str = "jumbie";

/// Tray icon rendered at several sizes so HiDPI panels pick a crisp source.
///
/// Decoded once from the embedded PNGs and converted to the ARGB32 that the
/// StatusNotifierItem spec expects.
static ICONS: LazyLock<Vec<Icon>> = LazyLock::new(|| {
    ICON_PNGS
        .iter()
        .filter_map(|png| {
            let (width, height, mut data) = decode_rgba(png)?;
            rgba_to_argb(&mut data);
            Some(Icon {
                width: width as i32,
                height: height as i32,
                data,
            })
        })
        .collect()
});

struct JumbieTray {
    port: u16,
    tray_cancel: CancellationToken,
}

impl Tray for JumbieTray {
    /// A left click opens the menu.
    const MENU_ON_ACTIVATE: bool = true;

    fn id(&self) -> String {
        APP_ID.to_owned()
    }

    fn title(&self) -> String {
        APP_TITLE.to_owned()
    }

    fn icon_pixmap(&self) -> Vec<Icon> {
        ICONS.clone()
    }

    fn tool_tip(&self) -> ToolTip {
        ToolTip {
            title: APP_TITLE.to_owned(),
            ..Default::default()
        }
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        vec![
            StandardItem {
                label: OPEN_WEB_LABEL.to_owned(),
                activate: Box::new(|this: &mut Self| {
                    let _ = open::that(open_web_url(this.port));
                }),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: QUIT_LABEL.to_owned(),
                activate: Box::new(|this: &mut Self| this.tray_cancel.cancel()),
                ..Default::default()
            }
            .into(),
        ]
    }

    fn watcher_online(&self) {
        tracing::info!("System tray watcher is online");
    }

    fn watcher_offline(&self, reason: OfflineReason) -> bool {
        tracing::warn!("System tray watcher is offline: {reason:?}");
        true
    }
}

/// Handle to a running tray service.
pub struct TrayApp {
    handle: ksni::Handle<JumbieTray>,
}

impl TrayApp {
    /// Register the StatusNotifierItem and start serving its menu over D-Bus.
    ///
    /// # Errors
    ///
    /// Returns an error if the session D-Bus is unavailable (e.g. a headless
    /// container without a session bus), letting the caller fall back to
    /// headless operation.
    pub async fn new(port: u16, tray_cancel: CancellationToken) -> Result<Self> {
        let tray = JumbieTray { port, tray_cancel };

        // A missing/not-yet-ready StatusNotifierHost is treated as a soft error:
        // the service stays up and the icon appears once the desktop shell
        // exposes its watcher (e.g. the shell restarted after we started).
        let handle = tray.assume_sni_available(true).spawn().await?;
        Ok(Self { handle })
    }

    /// Remove the tray icon and stop the underlying D-Bus service.
    pub async fn shutdown(self) {
        self.handle.shutdown().await;
    }
}
