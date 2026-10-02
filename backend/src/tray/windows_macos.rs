//! System tray for Windows and macOS via the `tray-icon` + `tao` crates
//! (Win32 / Cocoa).
//!
//! Not compiled on Linux — see `super::linux`.

use anyhow::Result;
use tao::event_loop::{ControlFlow, EventLoop, EventLoopBuilder, EventLoopProxy};
use tray_icon::{
    TrayIcon, TrayIconBuilder,
    menu::{Menu, MenuItem, PredefinedMenuItem},
};

use super::{APP_TITLE, OPEN_WEB_LABEL, QUIT_LABEL, TRAY_ICON_PNG, decode_rgba, open_web_url};

#[derive(Debug)]
pub enum AppEvent {
    MenuEvent(tray_icon::menu::MenuEvent),
    AppFinished,
}

pub struct TrayApp {
    event_loop: EventLoop<AppEvent>,
    proxy: EventLoopProxy<AppEvent>,
    port: u16,
    tray_cancel: tokio_util::sync::CancellationToken,
    open_web_id: tray_icon::menu::MenuId,
    quit_app_id: tray_icon::menu::MenuId,
    _tray_icon: TrayIcon,
}

impl TrayApp {
    pub fn new(port: u16, tray_cancel: tokio_util::sync::CancellationToken) -> Result<Self> {
        let event_loop = EventLoopBuilder::<AppEvent>::with_user_event().build();
        let proxy = event_loop.create_proxy();
        let proxy_clone = proxy.clone();

        tray_icon::menu::MenuEvent::set_event_handler(Some(move |event| {
            let _ = proxy_clone.send_event(AppEvent::MenuEvent(event));
        }));

        let tray_menu = Menu::new();
        let open_web = MenuItem::new(OPEN_WEB_LABEL, true, None);
        let quit_app = MenuItem::new(QUIT_LABEL, true, None);
        tray_menu.append_items(&[&open_web, &PredefinedMenuItem::separator(), &quit_app])?;

        let (width, height, rgba) =
            decode_rgba(TRAY_ICON_PNG).expect("Failed to load icon from memory");
        let icon =
            tray_icon::Icon::from_rgba(rgba, width, height).expect("Failed to create tray icon");

        let tray_icon = TrayIconBuilder::new()
            .with_menu(Box::new(tray_menu))
            .with_tooltip(APP_TITLE)
            .with_icon(icon)
            .build()?;

        Ok(Self {
            event_loop,
            proxy,
            port,
            tray_cancel,
            open_web_id: open_web.id().clone(),
            quit_app_id: quit_app.id().clone(),
            _tray_icon: tray_icon,
        })
    }

    pub fn proxy(&self) -> EventLoopProxy<AppEvent> {
        self.proxy.clone()
    }

    pub fn run(self) {
        let port = self.port;
        let tray_cancel = self.tray_cancel;
        let open_web_id = self.open_web_id;
        let quit_app_id = self.quit_app_id;

        self.event_loop.run(move |event, _, control_flow| {
            *control_flow = ControlFlow::Wait;
            if let tao::event::Event::UserEvent(app_event) = event {
                match app_event {
                    AppEvent::MenuEvent(menu_event) => {
                        if menu_event.id == open_web_id {
                            let _ = open::that(open_web_url(port));
                        } else if menu_event.id == quit_app_id {
                            tray_cancel.cancel();
                        }
                    }
                    AppEvent::AppFinished => {
                        *control_flow = ControlFlow::Exit;
                    }
                }
            }
        });
    }
}
