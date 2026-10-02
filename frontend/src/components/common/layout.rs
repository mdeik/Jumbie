use leptos::prelude::*;

fn base_class(
    base: &str,
    gap: Option<String>,
    class: Signal<String>,
    extra: Vec<Option<String>>,
) -> String {
    let mut classes = vec![base.to_string()];
    if let Some(g) = gap {
        classes.push(format!("gap-{}", g));
    }
    for val in extra.into_iter().flatten() {
        classes.push(val);
    }

    let extra_class = class.get();
    if !extra_class.is_empty() {
        classes.push(extra_class);
    }

    classes.join(" ")
}

/// A vertical stack layout component.
#[component]
pub fn Stack(
    #[prop(optional, into)] gap: Option<String>,
    #[prop(optional, into)] align: Option<String>,
    #[prop(optional, into)] class: Signal<String>,
    children: Children,
) -> impl IntoView {
    view! {
        <div class=move || base_class("flex flex-col", gap.clone(), class.clone(), vec![
            align.as_ref().map(|a| format!("items-{}", a))
        ])>
            {children()}
        </div>
    }
}

/// A horizontal group layout component.
#[component]
pub fn Group(
    #[prop(optional, into)] gap: Option<String>,
    #[prop(optional, into)] align: Option<String>,
    #[prop(optional, into)] justify: Option<String>,
    #[prop(optional)] wrap: bool,
    #[prop(optional, into)] class: Signal<String>,
    children: Children,
) -> impl IntoView {
    view! {
        <div class=move || base_class("flex flex-row", gap.clone(), class.clone(), vec![
            Some(align.as_ref().map(|a| format!("items-{}", a)).unwrap_or_else(|| "items-center".to_string())),
            justify.as_ref().map(|j| format!("justify-{}", j)),
            if wrap { Some("flex-wrap".to_string()) } else { None }
        ])>
            {children()}
        </div>
    }
}

/// A responsive grid layout component.
#[component]
pub fn Grid(
    #[prop(optional)] cols: Option<u32>,
    #[prop(optional)] md_cols: Option<u32>,
    #[prop(optional)] lg_cols: Option<u32>,
    #[prop(optional, into)] gap: Option<String>,
    #[prop(optional, into)] class: Signal<String>,
    children: Children,
) -> impl IntoView {
    view! {
        <div class=move || base_class("grid", gap.clone(), class.clone(), vec![
            Some(cols.map(|c| format!("grid-cols-{}", c)).unwrap_or_else(|| "grid-cols-1".to_string())),
            md_cols.map(|c| format!("grid-md-cols-{}", c)),
            lg_cols.map(|c| format!("grid-lg-cols-{}", c))
        ])>
            {children()}
        </div>
    }
}

/// A standardized card container.
#[component]
pub fn Card(#[prop(optional, into)] class: Signal<String>, children: Children) -> impl IntoView {
    view! {
        <div class=move || format!("card {}", class.get())>
            {children()}
        </div>
    }
}
