use crate::components::common::form_fields::text_input::TextInput;
use crate::components::common::form_fields::tooltip::TooltipBuilder;
use leptos::prelude::*;

#[component]
pub fn FormatFieldBuilder(
    #[prop(into)] id: String,
    #[prop(into)] label: String,
    #[prop(into)] value: Signal<String>,
    #[prop(into)] on_change: Callback<String>,
    #[prop(into)] tooltip: TooltipBuilder,
    #[prop(optional)] on_blur: Option<Callback<()>>,
    #[prop(optional, into)] placeholder: Signal<String>,
    #[prop(optional, into)] disabled: Signal<bool>,
    #[prop(optional, into)] error: MaybeProp<String>,
    #[prop(optional, into)] help_text: Signal<String>,
) -> impl IntoView {
    view! {
        <div class="form-group">
            <div class="flex">
            <label class="form-label flex items-center" for=id.clone()>
                {label}

            </label>
            {tooltip.build()}
            </div>
            <TextInput
                id=id
                value=value
                set_value=on_change
                placeholder=placeholder
                disabled=disabled
                error=error
                help_text=help_text
                on_blur=Callback::new(move |_| {
                    if let Some(cb) = on_blur {
                        cb.run(());
                    }
                })
            />
        </div>
    }
}
