use crate::components::common::form_fields::FormGroup;
use leptos::prelude::*;

#[derive(Clone, Debug, PartialEq)]
pub struct SelectOptionItem {
    pub value: String,
    pub label: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SelectOption {
    Single(SelectOptionItem),
    Group {
        label: String,
        items: Vec<SelectOptionItem>,
    },
}

impl SelectOption {
    pub fn first_value(&self) -> Option<String> {
        match self {
            SelectOption::Single(item) => Some(item.value.clone()),
            SelectOption::Group { items, .. } => items.first().map(|i| i.value.clone()),
        }
    }
}

impl From<(String, String)> for SelectOption {
    fn from((value, label): (String, String)) -> Self {
        SelectOption::Single(SelectOptionItem { value, label })
    }
}

#[component]
pub fn Select(
    #[prop(into)] value: Signal<String>,
    #[prop(into)] set_value: Callback<String>,
    #[prop(into)] options: Signal<Vec<SelectOption>>,
    #[prop(optional, into)] id: String,
    #[prop(optional, into)] name: Option<String>,
    #[prop(optional, into)] class: Signal<String>,
    #[prop(optional, into)] disabled: Signal<bool>,
    #[prop(optional, into)] label: Signal<String>,
    #[prop(optional, into)] help_text: Signal<String>,
    #[prop(optional, into)] help_signal: Signal<String>,
    #[prop(optional, into)] error: MaybeProp<String>,
    #[prop(optional)] autofocus: bool,
) -> impl IntoView {
    let id_val = if id.is_empty() {
        format!("f-{}", uuid::Uuid::new_v4())
    } else {
        id.clone()
    };
    let id_for_label = id_val.clone();

    view! {
        <FormGroup label=label label_for=id_for_label help_text=help_text help_signal=help_signal error=error>
            <select
                class=move || format!("form-select {}", class.get())
                id=id_val
                name=name
                prop:disabled=disabled
                prop:autofocus=autofocus
                on:change=move |ev| set_value.run(event_target_value(&ev))
            >
            {move || options.get().into_iter().map(|opt| {
                match opt {
                    SelectOption::Single(item) => {
                        let val = item.value;
                        let label = item.label;
                        let val_clone1 = val.clone();
                        let val_clone2 = val.clone();
                        view! {
                            <option
                                value=val.clone()
                                selected=move || value.get() == val_clone1
                                prop:selected=move || value.get() == val_clone2
                            >
                                {label}
                            </option>
                        }.into_any()
                    },
                    SelectOption::Group { label, items } => {
                        view! {
                            <optgroup label=label>
                                {items.into_iter().map(|item| {
                                    let val = item.value;
                                    let label = item.label;
                                    let val_clone1 = val.clone();
                                    let val_clone2 = val.clone();
                                    view! {
                                        <option
                                            value=val.clone()
                                            selected=move || value.get() == val_clone1
                                            prop:selected=move || value.get() == val_clone2
                                        >
                                            {label}
                                        </option>
                                    }
                                }).collect_view()}
                            </optgroup>
                        }.into_any()
                    }
                }
            }).collect_view()}
        </select>
        </FormGroup>
    }
}
