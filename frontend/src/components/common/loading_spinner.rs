use leptos::prelude::*;

#[component]
pub fn LoadingSpinner(
    #[prop(default = "Loading...".to_string(), into)] label: String,
    #[prop(default = false)] full_height: bool,
) -> impl IntoView {
    let classes = if full_height {
        "loader-container full-height"
    } else {
        "loader-container"
    };

    let label_clone = label.clone();
    view! {
        <div class=classes title=label_clone>
            <div class="loader"></div>
            <span class="sr-only">{label}</span>
        </div>
    }
}
