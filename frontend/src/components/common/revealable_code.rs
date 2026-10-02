use leptos::prelude::*;

/// Shared state for a revealable code block and its associated toggle button.
///
/// Create one with [`RevealState::new`], then pass it to both
/// [`RevealableCode`] and [`RevealButton`] so they stay in sync.
#[derive(Clone, Copy)]
pub struct RevealState {
    revealed: ReadSignal<bool>,
    set_revealed: WriteSignal<bool>,
}

impl Default for RevealState {
    fn default() -> Self {
        Self::new()
    }
}

impl RevealState {
    pub fn new() -> Self {
        let (revealed, set_revealed) = signal(false);
        Self {
            revealed,
            set_revealed,
        }
    }
}

/// A `<code>` element that either shows `content` or masks it with asterisks.
///
/// Clicking the text when revealed selects the entire contents.
#[component]
pub fn RevealableCode(
    content: String,
    state: RevealState,
    /// Extra CSS classes to forward to the `<code>` element (e.g. `overflow-wrap-anywhere`).
    #[prop(optional)]
    class: &'static str,
) -> impl IntoView {
    view! {
        <code class={format!("auth-code-bg text-xs break-all min-w-0 {class}")} style:word-break=move || if state.revealed.get() { "break-all" } else { "unset" } on:click=move |ev| {
            if state.revealed.get() {
                crate::utils::select_element_contents(&event_target(&ev));
            }
        }>
            {move || if state.revealed.get() { content.clone() } else { "*".repeat(content.len()) }}
        </code>
    }
}

/// A "Reveal" / "Hide" toggle button paired with a [`RevealState`].
///
/// Clears any active text selection when toggled so a stale selection on
/// now-hidden text doesn't persist.
#[component]
pub fn RevealButton(state: RevealState) -> impl IntoView {
    view! {
        <button class="btn btn-ghost btn-reveal" on:click=move |_| {
            state.set_revealed.update(|v| *v = !*v);
            crate::utils::clear_selection();
        }>
            {move || if state.revealed.get() { "Hide" } else { "Reveal" }}
        </button>
    }
}
