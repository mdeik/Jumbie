use crate::components::common::form_fields::{CheckboxInput, FormatFieldBuilder, TooltipBuilder};
use leptos::prelude::*;

/// A template field with an override checkbox.
///
/// Unchecked means "inherit": the field is disabled and shows the effective
/// inherited value. Checking seeds the override with that inherited value;
/// clearing the checkbox drops the override. The field itself may be emptied to
/// a deliberate blank.
#[component]
pub fn OverrideFormatField(
    #[prop(into)] id: String,
    #[prop(into)] label: String,
    #[prop(into)] override_label: String,
    #[prop(into)] help_text: String,
    /// The override value: `None` = inherit, `Some(_)` = override (blank allowed).
    value: Signal<Option<String>>,
    set_value: Callback<Option<String>>,
    /// The effective inherited value shown while not overriding.
    inherited: Signal<String>,
    #[prop(into)] tooltip: TooltipBuilder,
    #[prop(optional)] on_blur: Option<Callback<()>>,
    /// Help text rendered under the template input (distinct from the override
    /// checkbox's `help_text`).
    #[prop(optional, into)]
    field_help_text: Signal<String>,
    /// Fired after the override checkbox toggles (not on text edits), so callers
    /// can persist the inherit/override change immediately.
    #[prop(optional)]
    on_toggle: Option<Callback<()>>,
    #[prop(optional, into)] error: MaybeProp<String>,
) -> impl IntoView {
    let overridden = Signal::derive(move || value.get().is_some());
    let inherited_for_check = inherited;
    let inherited_for_value = inherited;
    let value_for_field = value;
    let value_for_disabled = value;
    let set_value = StoredValue::new_local(set_value);
    let on_toggle = StoredValue::new_local(on_toggle);

    view! {
        <div class="form-group">
            <CheckboxInput
                id=format!("{}-override", id)
                checked=overridden
                set_checked=Callback::new(move |checked| {
                    if checked {
                        set_value.with_value(|cb| cb.run(Some(inherited_for_check.get())));
                    } else {
                        set_value.with_value(|cb| cb.run(None));
                    }
                    on_toggle.with_value(|cb| {
                        if let Some(cb) = cb {
                            cb.run(());
                        }
                    });
                })
                label=override_label
                help_text=help_text
            />
            <FormatFieldBuilder
                id=id
                label=label
                value=Signal::derive(move || {
                    value_for_field
                        .get()
                        .unwrap_or_else(|| inherited_for_value.get())
                })
                on_change=Callback::new(move |v| set_value.with_value(|cb| cb.run(Some(v))))
                tooltip=tooltip
                disabled=Signal::derive(move || value_for_disabled.get().is_none())
                on_blur=on_blur.unwrap_or_else(|| Callback::new(|_: ()| {}))
                error=error
                help_text=field_help_text
            />
        </div>
    }
}
