use leptos::prelude::*;

/// A convenient wrapper to render the standard dynamic schema form in the editor closure.
/// Many plugin forms are entirely dynamic and just need to wire up `update_draft`.
#[component]
pub fn DynamicPluginFormWrapper(
    plugin_type: String,
    initial_value: serde_json::Value,
    update_draft: Callback<serde_json::Value>,
) -> impl IntoView {
    let (draft_sig, set_draft_sig) = signal(initial_value);
    Effect::new(move |_| update_draft.run(draft_sig.get()));
    view! {
        <crate::components::common::json_schema_form::DynamicPluginForm
            plugin_id=plugin_type
            value=Signal::derive(move || draft_sig.get())
            set_value=Callback::new(move |v| set_draft_sig.set(v))
        />
    }
}
