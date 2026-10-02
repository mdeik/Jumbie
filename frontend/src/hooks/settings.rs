use leptos::prelude::*;

/// A hook to manage a draft of a settings object.
///
/// - `original`: The backend-synchronized settings signal.
/// - `saver`: A callback to save the modified settings.
///
/// Returns `(draft, set_draft, save, is_dirty)`.
pub fn use_settings_draft<T, S>(
    original: Signal<Option<T>>,
    saver: S,
) -> (Signal<Option<T>>, Callback<T>, Callback<()>, Signal<bool>)
where
    T: Clone + serde::Serialize + serde::de::DeserializeOwned + PartialEq + Send + Sync + 'static,
    S: Fn(T) + Send + Sync + 'static,
{
    let (draft, set_draft) = signal(None::<T>);
    let (is_dirty, set_is_dirty) = signal(false);

    // Sync draft with original initially or when original changes but we aren't dirty
    Effect::new(move |_| {
        if let Some(orig) = original.get()
            && (draft.get_untracked().is_none() || !is_dirty.get_untracked())
        {
            set_draft.set(Some(orig));
        }
    });

    let update_draft = Callback::new(move |new_val: T| {
        set_draft.set(Some(new_val.clone()));

        let dirty = if let Some(orig) = original.get_untracked() {
            orig != new_val
        } else {
            true
        };
        set_is_dirty.set(dirty);
    });

    let save = Callback::new({
        let saver = std::sync::Arc::new(saver);
        move |_| {
            if let Some(d) = draft.get_untracked() {
                saver(d);
                set_is_dirty.set(false);
            }
        }
    });

    (draft.into(), update_draft, save, is_dirty.into())
}

/// A hook to sync a local state signal with a JSON-serialized draft callback.
pub fn use_serialization_effect<T>(
    settings: ReadSignal<T>,
    update_draft: Callback<serde_json::Value>,
) where
    T: Clone + serde::Serialize + Send + Sync + 'static,
{
    Effect::new(move |_| {
        if let Ok(val) = settings.with(|s| serde_json::to_value(s)) {
            update_draft.run(val);
        }
    });
}
