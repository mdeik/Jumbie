use leptos::prelude::*;

/// localStorage key prefix used for view mode persistence.
const LS_KEY_PREFIX: &str = "jb_view_mode_";

fn read_local_storage(id: &str) -> Option<String> {
    let key = format!("{}{}", LS_KEY_PREFIX, id);
    web_sys::window()
        .and_then(|w| w.local_storage().ok().flatten())
        .and_then(|s| s.get_item(&key).ok().flatten())
}

fn write_local_storage(id: &str, mode: &str) {
    let key = format!("{}{}", LS_KEY_PREFIX, id);
    if let Some(window) = web_sys::window()
        && let Ok(Some(storage)) = window.local_storage()
    {
        let _ = storage.set_item(&key, mode);
    }
}

/// A hook to manage a generic view mode state (e.g. month vs list), stored in a
/// local `RwSignal` for reactivity and persisted to `localStorage` synchronously
/// so it survives page refresh. Returns `(view_mode, set_view_mode)`.
pub fn use_persistent_view_mode(
    persistence_id: String,
    default_mode: String,
) -> (Signal<String>, Callback<String>) {
    let def = default_mode.clone();

    // Initialise from localStorage (survives refresh), falling back to default.
    let initial = read_local_storage(&persistence_id).unwrap_or(def);
    let mode = RwSignal::new(initial);

    let view_mode = mode.into();

    let on_change = Callback::new(move |new_mode: String| {
        if mode.get_untracked() != new_mode {
            mode.set(new_mode.clone());
            write_local_storage(&persistence_id, &new_mode);
        }
    });

    (view_mode, on_change)
}
