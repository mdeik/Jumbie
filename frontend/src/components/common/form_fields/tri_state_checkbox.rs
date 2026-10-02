use crate::components::common::form_fields::checkbox_input::next_in_cycle;
use leptos::prelude::*;
use uuid::Uuid;

/// A tri-state checkbox that cycles through None (inherit) → Some(true) → Some(false) → None.
/// Displays as an indeterminate checkbox when inheriting. The inherit source
/// label is configurable (`inherit from global` by default, or `inherit from series`).
#[component]
pub fn TriStateCheckbox(
    label: &'static str,
    #[prop(into)] help_text: Signal<String>,
    value: Signal<Option<bool>>,
    set_value: Callback<Option<bool>>,
    #[prop(default = "inherit from global")] inherit_label: &'static str,
) -> impl IntoView {
    let input_ref = NodeRef::<leptos::html::Input>::new();
    let id = Uuid::new_v4().to_string();

    // One-shot initialisation: set indeterminate on the DOM element once
    // the node is available. The browser handles On/Off visuals naturally
    // through clicks — we only override the DOM for the None (inherit) state.
    let init_done = StoredValue::new_local(false);
    Effect::new(move |_| {
        if !init_done.get_value()
            && let Some(input) = input_ref.get()
        {
            input.set_indeterminate(value.get_untracked().is_none());
            init_done.set_value(true);
        }
    });

    view! {
        <div class="form-label checkbox-group">
            <div class="form-check">
                <input
                    type="checkbox"
                    id=id.clone()
                    node_ref=input_ref
                    class=move || if value.get().is_none() { "form-check-input tri-state-default" } else { "form-check-input" }
                    prop:checked=move || value.get().unwrap_or(false)
                    on:click=move |ev| {
                        let states = [None, Some(true), Some(false)];
                        let next = *next_in_cycle(&value.get_untracked(), &states);
                        set_value.run(next);
                        // When cycling to None we need the indeterminate dash, but
                        // the browser's native toggle runs *after* this handler and
                        // would set checked=true and clear indeterminate, undoing
                        // our DOM changes.  prevent_default stops that.
                        // On/Off transitions are left to the browser — no override.
                        if next.is_none() {
                            ev.prevent_default();
                            if let Some(input) = input_ref.get() {
                                input.set_checked(false);
                                input.set_indeterminate(true);
                            }
                        }
                    }
                />
                <label for=id.clone() class="form-check-label">
                    {move || match value.get() {
                        None => format!("{} \u{2014} ({})", label, inherit_label),
                        Some(true) => format!("{} \u{2014} On", label),
                        Some(false) => format!("{} \u{2014} Off", label),
                    }}
                </label>
            </div>
            <div class="form-hint">{move || help_text.get()}</div>
        </div>
    }
}
