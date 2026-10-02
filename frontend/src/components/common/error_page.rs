use crate::routes::path;
use leptos::prelude::*;
use leptos_router::hooks::*;

#[component]
pub fn ErrorPage(
    #[prop(into, default = "Error".to_string())] title: String,
    #[prop(into, default = "An error occurred".to_string())] message: String,
    #[prop(into, default = "⚠️".to_string())] icon: String,
    #[prop(optional)] hide_back_btn: bool,
    #[prop(into, default = format!("/{}", path::SERIES))] back_path: String,
    #[prop(into, default = "Back to Library".to_string())] back_label: String,
) -> impl IntoView {
    let navigate = use_navigate();

    view! {
        <div class="view active">
            <div class="error-container">
                <span class="error-container-icon">{icon}</span>
                <h3 class="mb-sm">{title}</h3>
                <p class="mb-xl">{message}</p>
                <Show when=move || !hide_back_btn>
                    <button class="btn btn-primary" on:click={
                        let nav = navigate.clone();
                        let path = back_path.clone();
                        move |_| nav(&path, Default::default())
                    }>{back_label.clone()}</button>
                </Show>
            </div>
        </div>
    }
}
