use leptos::prelude::*;
use leptos_router::hooks::use_location;

#[component]
pub fn ViewShell(
    #[prop(into)] id: String,
    #[prop(optional, into)] class: String,
    children: Children,
) -> impl IntoView {
    let location = use_location();
    let div_ref = NodeRef::<leptos::html::Div>::new();

    // Track the previous path so we only re-trigger on actual navigation,
    // not on every render.
    let prev_path = RwSignal::new(String::new());

    Effect::new(move |_| {
        let new_path = location.pathname.get();
        let old = prev_path.get_untracked();
        if new_path != old {
            prev_path.set(new_path.clone());

            // Re-trigger the CSS `fadeIn` animation on the existing DOM
            // element by briefly removing the "active" class and re-adding
            // it after forcing a browser reflow. This is the equivalent of
            // `el.classList.remove("active"); void el.offsetWidth; el.classList.add("active");`
            // in vanilla JS.
            //
            // We cannot use `class_list()` / `DomTokenList` here because that
            // web-sys feature is not enabled. Instead we read and set the
            // full `className` string.
            if let Some(el) = div_ref.get() {
                let full_class = el.class_name();
                let without_active = full_class
                    .split_ascii_whitespace()
                    .filter(|c| *c != "active")
                    .collect::<Vec<_>>()
                    .join(" ");
                el.set_class_name(&without_active);
                // Force browser reflow so the browser sees the removal
                // before we add "active" back.
                let _ = el.offset_height();
                el.set_class_name(&full_class);
            }
        }
    });

    view! {
        <div class=format!("view active {} ", class) id=id node_ref=div_ref>
            {children()}
        </div>
    }
}
