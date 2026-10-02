use leptos::prelude::*;

#[component]
pub fn EmptyState(
    #[prop(into)] message: String,
    #[prop(default = "empty-state".to_string(), into)] class_name: String,
) -> impl IntoView {
    view! {
        <div class=class_name>
            {message}
        </div>
    }
}
