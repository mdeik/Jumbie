use leptos::task::spawn_local;

/// Manage application theme fetching and application to the DOM.
pub fn use_theme() {
    spawn_local(async move {
        if let Ok(theme_res) = crate::api::fetch_public_theme().await {
            apply_theme(&theme_res.theme);
        } else if let Some(window) = web_sys::window() {
            // Fall back to the cached theme when the fetch fails
            if let Ok(Some(storage)) = window.local_storage()
                && let Ok(Some(cached_theme)) = storage.get_item("jb_active_theme")
            {
                apply_theme(&cached_theme);
            }
        }
    });
}

pub fn apply_theme(theme: &str) {
    if let Some(window) = web_sys::window() {
        if let Some(doc) = window.document()
            && let Some(root) = doc.document_element()
        {
            let _ = root.set_attribute("data-theme", theme);
        }
        if let Ok(Some(storage)) = window.local_storage() {
            let _ = storage.set_item("jb_active_theme", theme);
        }
    }
}
