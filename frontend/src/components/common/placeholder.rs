use leptos::prelude::*;

#[component]
pub fn Placeholder(view_name: &'static str) -> impl IntoView {
    view! {
        <div class="view active">
            <div class="empty-state">
                <div class="empty-state-icon">"🚧"</div>
                <h2>{format!("{} - Under Construction", view_name)}</h2>
                <p>"This view is not yet implemented."</p>
            </div>
        </div>
    }
}
