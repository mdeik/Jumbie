use crate::components::common::form_fields::FormGroup;
use crate::components::common::form_fields::helpers::resolve_field_id;
use crate::components::common::icons::{EyeIcon, EyeOffIcon};
use leptos::html;
use leptos::prelude::*;

#[component]
pub fn TextInput(
    #[prop(into)] value: Signal<String>,
    #[prop(into)] set_value: Callback<String>,
    #[prop(optional, into)] placeholder: Signal<String>,
    #[prop(optional, into)] id: String,
    #[prop(optional, into)] class: Signal<String>,
    #[prop(optional, into)] type_: Signal<String>,
    #[prop(optional, into)] disabled: Signal<bool>,
    #[prop(optional)] readonly: bool,
    #[prop(optional)] required: bool,
    #[prop(optional)] show_toggle: bool,
    #[prop(optional, into)] on_blur: Option<Callback<()>>,
    #[prop(optional, into)] name: Option<String>,
    #[prop(optional, into)] autocomplete: Option<String>,
    #[prop(optional)] password_placeholder_behaviour: bool,
    #[prop(optional, into)] label: Signal<String>,
    #[prop(optional, into)] help_text: Signal<String>,
    #[prop(optional, into)] error: MaybeProp<String>,
    #[prop(optional)] autofocus: bool,
) -> impl IntoView {
    let id_val = resolve_field_id(&id);
    let id_for_label = id_val.clone();

    let type_for_memo = type_.clone();
    let is_password_field = Memo::new(move |_| type_for_memo.get() == "password");
    let (visible, set_visible) = signal(false);

    let is_pw_for_type = is_password_field;
    let type_for_type = type_.clone();
    let effective_type = move || {
        let t = type_for_type.get();
        let base = if t.is_empty() { "text".to_string() } else { t };
        if is_pw_for_type.get() && show_toggle {
            if visible.get() {
                "text".to_string()
            } else {
                "password".to_string()
            }
        } else {
            base
        }
    };

    let id_for_name = id_val.clone();
    let name_attr = name.or({
        if id_for_name.is_empty() {
            None
        } else {
            Some(id_for_name)
        }
    });
    let autocomplete_attr = autocomplete.or_else(|| Some("off".to_string()));

    let is_pw_for_placeholder = is_password_field;
    let placeholder_for_placeholder = placeholder.clone();
    let effective_placeholder = move || {
        if password_placeholder_behaviour
            && is_pw_for_placeholder.get()
            && show_toggle
            && !visible.get()
        {
            "********".to_string()
        } else {
            placeholder_for_placeholder.get()
        }
    };

    let input_ref = NodeRef::<html::Input>::new();
    if autofocus {
        Effect::new(move |_| {
            if let Some(el) = input_ref.get() {
                let _ = el.focus();
            }
        });
    }

    let input_view = {
        let id_val = id_val.clone();
        let class = class.clone();
        let name_attr = name_attr.clone();
        let autocomplete_attr = autocomplete_attr.clone();
        let on_blur = on_blur.clone();

        if show_toggle && is_password_field.get_untracked() {
            view! {
                <div class="form-field-wrapper">
                    <input
                        node_ref=input_ref
                        type=effective_type
                        class="form-input form-field-input-flex"
                        id=id_val.clone()
                        name=name_attr.clone()
                        autocomplete=autocomplete_attr.clone()
                        placeholder=move || {
                            let p = effective_placeholder();
                            if p.is_empty() { None } else { Some(p) }
                        }
                        prop:value=value
                        prop:disabled=disabled
                        readonly=readonly
                        required=required
                        autofocus=autofocus
                        on:input=move |ev| set_value.run(event_target_value(&ev))
                        on:blur=move |_| { if let Some(cb) = &on_blur { cb.run(()); } }
                    />
                    <button
                        type="button"
                        class="btn btn-ghost btn-icon form-field-icon-right"
                        on:click=move |_| set_visible.update(|v| *v = !*v)
                        title=move || if visible.get() { "Hide password" } else { "Show password" }
                    >
                        {move || if visible.get() { view!{ <EyeOffIcon/> }.into_any() } else { view!{ <EyeIcon/> }.into_any() }}
                    </button>
                </div>
            }.into_any()
        } else {
            view! {
                <input
                    node_ref=input_ref
                    type=effective_type
                    class=move || format!("form-input {}", class.get())
                    id=id_val
                    name=name_attr
                    autocomplete=autocomplete_attr
                    placeholder=move || {
                        let p = effective_placeholder();
                        if p.is_empty() { None } else { Some(p) }
                    }
                    prop:value=value
                    prop:disabled=disabled
                    readonly=readonly
                    required=required
                    autofocus=autofocus
                    on:input=move |ev| set_value.run(event_target_value(&ev))
                    on:blur=move |_| { if let Some(cb) = &on_blur { cb.run(()); } }
                />
            }
            .into_any()
        }
    };

    view! {
        <FormGroup label=label label_for=id_for_label help_text=help_text error=error class=class>
            {input_view}
        </FormGroup>
    }
}
