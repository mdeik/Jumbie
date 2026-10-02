use crate::api::fetch_config;
use crate::hooks::use_config::{ConfigContext, use_config};
use leptos::prelude::*;

/// A standardized wrapper for settings pages that ensures config is loaded
/// and provides a consistent layout.
#[component]
pub fn SettingsPage(
    #[prop(into)] id: String,
    #[prop(optional, into)] header: String,
    children: Children,
) -> impl IntoView {
    let ConfigContext {
        config: _,
        set_config,
    } = use_config();

    // Ensure config is loaded and synchronized with the global context.
    // use_api_cache handles deduplication and caching.
    crate::utils::use_api_cache(
        "fetch_config".to_string(),
        || fetch_config(),
        move |c| {
            let _ = set_config.try_update(|s| *s = Some(c));
        },
    );

    view! {
        <div class="view active" id=if id.is_empty() { None } else { Some(id.clone()) }>
            <div class="settings-content">
                {if !header.is_empty() {
                    view! { <h1 class="page-header-title">{header}</h1> }.into_any()
                } else {
                    view! { <></> }.into_any()
                }}
                <div class="settings-body">
                    {children()}
                </div>
            </div>
        </div>
    }
}
