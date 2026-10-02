use crate::components::common::form_fields::FormGroup;
use leptos::prelude::*;

#[component]
pub fn CheckboxInput(
    #[prop(into)] checked: Signal<bool>,
    #[prop(into)] set_checked: Callback<bool>,
    #[prop(optional, into)] label: Signal<String>,
    #[prop(optional)] children: Option<Children>,
    #[prop(optional, into)] id: String,
    #[prop(optional, into)] class: Signal<String>,
    #[prop(optional, into)] disabled: Signal<bool>,
    #[prop(optional, into)] forced: Signal<bool>,
    #[prop(optional, into)] help_text: Signal<String>,
) -> impl IntoView {
    let id_val = if id.is_empty() {
        uuid::Uuid::new_v4().to_string()
    } else {
        id
    };
    let id_for_label = id_val.clone();

    view! {
        <FormGroup label_for=id_for_label help_text=help_text class=class.get()>
            <div class="form-check".to_string()>
                <input
                    type="checkbox"
                    class=move || {
                        let base = "form-check-input".to_string();
                        if forced.get() {
                            format!("{} forced-active", base)
                        } else {
                            base
                        }
                    }
                    id=id_val.clone()
                    prop:checked=checked
                    prop:disabled=disabled
                    on:change=move |ev| set_checked.run(event_target_checked(&ev))
                />
                <label class="form-check-label" for=id_val.clone()>
                    {move || label.get()}
                    {children.map(|c| c())}
                </label>
            </div>
        </FormGroup>
    }
}

/// Shared helper: returns the next value in a three-element cycle.
/// When `current` is not found in `states`, returns `states[0]`.
pub fn next_in_cycle<'a, T: PartialEq>(current: &'a T, states: &'a [T; 3]) -> &'a T {
    let pos = states.iter().position(|s| s == current).unwrap_or(2);
    &states[(pos + 1) % 3]
}

#[component]
pub fn CheckboxField(
    #[prop(into)] label: Signal<String>,
    #[prop(into)] checked: Signal<bool>,
    #[prop(into)] set_checked: Callback<bool>,
    #[prop(optional, into)] help_text: MaybeProp<String>,
    #[prop(optional, into)] disabled: MaybeProp<bool>,
    #[prop(into, optional)] class: Signal<String>,
) -> impl IntoView {
    let disabled_reactive = move || disabled.get().unwrap_or(false);
    let class_val = move || class.get();

    view! {
        <div class=move || format!("settings-checkbox-container {}", class_val())>
            <CheckboxInput
                label=label
                checked=checked
                set_checked=set_checked
                disabled=Signal::derive(disabled_reactive)
                help_text=Signal::derive(move || help_text.get().unwrap_or_default())
            />
        </div>
    }
}
