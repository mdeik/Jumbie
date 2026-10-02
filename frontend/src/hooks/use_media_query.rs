// use_media_query — reactive CSS media query tracking.
//
// Every table renders both a desktop and mobile layout; without a reactive media
// query Leptos evaluates both views even though one is hidden via CSS. This hook
// lets consumers wrap expensive views in conditional closures so only the matching
// view's reactive closures run. 946px is the project-wide mobile boundary.

use leptos::prelude::*;
use wasm_bindgen::prelude::*;

/// The project-wide mobile breakpoint (matches the CSS `@media (max-width: 946px)`).
pub const MOBILE_BREAKPOINT: &str = "(max-width: 946px)";

/// Numeric width (px) matching `MOBILE_BREAKPOINT` — SSoT for Rust code that needs
/// a numeric comparison (e.g. initial view-mode selection in the calendar).
pub const MOBILE_BREAKPOINT_PX: f64 = 946.0;

/// Reactively tracks whether a CSS media query matches.
///
/// Uses `window.matchMedia` under the hood and listens for `change` events to
/// update the signal without polling.
///
/// # Example
/// ```ignore
/// let is_mobile = use_media_query(MOBILE_BREAKPOINT);
/// ```
pub fn use_media_query(query: &str) -> Signal<bool> {
    // Initial value
    let matched = RwSignal::new(
        web_sys::window()
            .and_then(|w| w.match_media(query).ok().flatten())
            .map(|mql| mql.matches())
            .unwrap_or(false),
    );

    // Listen for changes — avoid the MediaQueryListEvent type (fiddly across
    // web-sys versions) by using a zero-arg closure that re-reads matches()
    // on every event.
    if let Some(window) = web_sys::window()
        && let Ok(Some(mql)) = window.match_media(query)
    {
        let query_owned = query.to_owned();
        let handler = Closure::<dyn FnMut()>::new(move || {
            matched.set(
                web_sys::window()
                    .and_then(|w| w.match_media(&query_owned).ok().flatten())
                    .map(|mql| mql.matches())
                    .unwrap_or(false),
            );
        });
        let _ = mql.add_event_listener_with_callback("change", handler.as_ref().unchecked_ref());
        handler.forget();
    }

    matched.into()
}

/// Convenience wrapper pre-configured with the project's mobile breakpoint.
pub fn use_is_mobile() -> Signal<bool> {
    use_media_query(MOBILE_BREAKPOINT)
}
