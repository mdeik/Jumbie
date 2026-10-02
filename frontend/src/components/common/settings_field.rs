use crate::components::common::form_fields::{FormGroup, Select, SelectOption, TextInput};
use crate::hooks::use_config_binding::ConfigBinding;
use leptos::prelude::*;

#[component]
pub fn SettingsTextInput(
    #[prop(into)] label: Signal<String>,
    binding: ConfigBinding<String>,
    #[prop(optional, into)] help_text: Signal<String>,
    #[prop(optional, into)] placeholder: Signal<String>,
    #[prop(optional, into)] type_: Signal<String>,
    #[prop(optional, into)] class: Signal<String>,
) -> impl IntoView {
    let id = format!("f-{}", uuid::Uuid::new_v4());
    view! {
        <FormGroup label=label label_for=id.clone() help_text=help_text>
            <TextInput
                id=id
                value=binding.value
                set_value=binding.set_value
                placeholder=placeholder
                type_=type_
                class=class
            />
        </FormGroup>
    }
}

#[component]
pub fn SettingsSelect(
    #[prop(into)] label: Signal<String>,
    binding: ConfigBinding<String>,
    #[prop(into)] options: Signal<Vec<(String, String)>>,
    #[prop(optional, into)] help_text: Signal<String>,
) -> impl IntoView {
    let id = format!("f-{}", uuid::Uuid::new_v4());
    let options_converted = Signal::derive(move || {
        options
            .get()
            .into_iter()
            .map(SelectOption::from)
            .collect::<Vec<_>>()
    });
    view! {
        <FormGroup label=label label_for=id.clone()>
            <Select
                id=id
                value=binding.value
                set_value=binding.set_value
                options=options_converted
                help_text=help_text
            />
        </FormGroup>
    }
}

#[component]
pub fn SettingsCheckbox(
    #[prop(into)] label: Signal<String>,
    binding: ConfigBinding<bool>,
    #[prop(optional, into)] help_text: Signal<String>,
    #[prop(optional, into)] disabled: MaybeProp<bool>,
    #[prop(optional, into)] class: Signal<String>,
) -> impl IntoView {
    view! {
        <crate::components::common::form_fields::CheckboxField
            label=label
            checked=binding.value
            set_checked=binding.set_value
            help_text=Signal::derive(move || {
                let text = help_text.get();
                if text.is_empty() { None } else { Some(text) }
            })
            disabled=disabled
            class=class
        />
    }
}
