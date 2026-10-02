use crate::components::common::icons::MenuIcon;
use crate::routes::header_title;
use leptos::prelude::*;
use leptos_router::hooks::*;

/// Given a path that didn't match any nav item directly, try matching the first
/// path segment as a parent route. This avoids flashing "Jumbie" during navigation
/// to dynamic sub-routes (e.g. `series/{id}/edit` → falls back to `series` → "Series Library").
fn parent_header_title(path: &str) -> Option<&'static str> {
    let first_segment = path.split('/').next().filter(|s| !s.is_empty())?;
    let title = header_title(first_segment);
    if title.is_empty() { None } else { Some(title) }
}

#[component]
pub fn Header() -> impl IntoView {
    let location = use_location();

    let layout_ctx = use_context::<crate::hooks::LayoutContext>().expect("LayoutContext missing");
    let set_sidebar_open = layout_ctx.set_sidebar_open;
    let header_title_override = layout_ctx.header_title_override;

    let active_path =
        Memo::new(move |_| location.pathname.get().trim_start_matches('/').to_string());

    view! {
        <header class="header">
            <div class="header-left flex items-center">
                <button
                    class="menu-toggle"
                    on:click=move |_| set_sidebar_open.update(|o| *o = !*o)
                >
                    <MenuIcon/>
                </button>
                <div class="header-title">
                    {move || {
                        if let Some(override_title) = header_title_override.get() {
                            return override_title;
                        }
                        let view = active_path.get();
                        let title = header_title(&view);
                        if !title.is_empty() {
                            title.to_string()
                        } else if let Some(parent) = parent_header_title(&view) {
                            parent.to_string()
                        } else {
                            String::new()
                        }
                    }}
                </div>
            </div>
            <div class="header-actions">
            </div>
        </header>
    }
}
