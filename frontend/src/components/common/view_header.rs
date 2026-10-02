use leptos::prelude::*;

#[component]
pub fn ViewHeader(
    #[prop(optional, into)] title: Option<String>,
    #[prop(optional, into)] search_bar: Option<AnyView>,
    #[prop(optional, into)] actions: Option<AnyView>,
    #[prop(optional, into)] class: Option<String>,
    #[prop(optional, into)] id: Option<String>,
) -> impl IntoView {
    view! {
        <div class=format!("table-header {}", class.unwrap_or_default()) id=id.unwrap_or_default()>
            {title.map(|t| view! { <span class="table-title">{t}</span> })}
            {search_bar.map(|s| view! { <div class="search-bar">{s}</div> })}
            {actions.map(|a| view! { <div class="table-actions flex gap-sm">{a}</div> })}
        </div>
    }
}
