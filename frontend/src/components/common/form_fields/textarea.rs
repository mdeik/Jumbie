use crate::components::common::form_fields::FormGroup;
use crate::components::common::form_fields::helpers::resolve_field_id;
use leptos::prelude::*;

#[component]
pub fn Textarea(
    #[prop(into)] value: Signal<String>,
    #[prop(into)] set_value: Callback<String>,
    #[prop(optional, into)] placeholder: Signal<String>,
    #[prop(optional, into)] id: String,
    #[prop(optional, into)] class: Signal<String>,
    #[prop(optional, into)] rows: Option<u32>,
    #[prop(optional, into)] disabled: Signal<bool>,
    #[prop(optional, into)] label: Signal<String>,
    #[prop(optional, into)] help_text: Signal<String>,
    #[prop(optional, into)] error: MaybeProp<String>,
    #[prop(optional)] autofocus: bool,
) -> impl IntoView {
    let id_val = resolve_field_id(&id);
    let id_for_label = id_val.clone();

    view! {
        <FormGroup label=label label_for=id_for_label help_text=help_text error=error>
            <textarea
                class=move || format!("form-input {}", class.get())
                id=id_val
                placeholder=move || { let p = placeholder.get(); if p.is_empty() { None } else { Some(p) } }
                rows=rows
                prop:value=value
                prop:disabled=disabled
                prop:autofocus=autofocus
                on:input=move |ev| set_value.run(event_target_value(&ev))
            />
        </FormGroup>
    }
}
