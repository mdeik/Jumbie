use gloo_timers::future::TimeoutFuture;
use leptos::prelude::*;
use leptos::task::spawn_local;

/// Free-text search box with debounce + Enter/blur.
///
/// Commits the trimmed text through `on_search`:
///   * immediately on **Enter**,
///   * immediately on **blur** (the `change` event), and
///   * after `debounce_ms` of no typing.
///
/// The value is read straight from the DOM event (`event_target_value`), not from
/// component state, so Enter/blur work even while a debounce timer is pending.
/// Identical commits are dropped: a repeated write would otherwise re-notify the
/// parent's `Signal::derive` query key (which has no equality check) and could
/// fire a redundant request.
///
/// The input is uncontrolled, so the caret never jumps.
#[component]
pub fn SearchInput(
    #[prop(into)] id: String,
    #[prop(into)] placeholder: String,
    #[prop(into)] on_search: Callback<String>,
    #[prop(default = 300)] debounce_ms: u32,
    #[prop(optional, into)] class: Option<String>,
) -> impl IntoView {
    let input_class = class.unwrap_or_else(|| "form-input".to_string());

    // Bumped on every keystroke/commit so a stale debounce is discarded (and on
    // unmount so a pending timer can't fire against a disposed owner).
    let generation = RwSignal::new(0u64);
    // Last value handed to the parent — drops redundant commits.
    let last_committed = RwSignal::new(None::<String>);

    let commit = move |value: String| {
        let value = value.trim().to_string();
        if last_committed.get_untracked().as_deref() == Some(value.as_str()) {
            return;
        }
        last_committed.set(Some(value.clone()));
        on_search.run(value);
    };

    // Cancel any pending debounce when this component is unmounted.
    on_cleanup(move || generation.update(|g| *g += 1));

    view! {
        <input
            type="text"
            id=id
            class=input_class
            placeholder=placeholder
            autocomplete="off"
            on:input=move |ev| {
                let value = event_target_value(&ev);
                let g = generation.get_untracked() + 1;
                generation.set(g);
                spawn_local(async move {
                    TimeoutFuture::new(debounce_ms).await;
                    if generation.get_untracked() == g {
                        commit(value);
                    }
                });
            }
            on:keydown=move |ev| {
                if ev.key() == "Enter" {
                    let value = event_target_value(&ev);
                    generation.update(|g| *g += 1);
                    commit(value);
                }
            }
            on:change=move |ev| {
                let value = event_target_value(&ev);
                generation.update(|g| *g += 1);
                commit(value);
            }
        />
    }
}
