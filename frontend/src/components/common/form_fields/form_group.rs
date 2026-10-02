use leptos::prelude::*;

#[component]
pub fn FormGroup(
    #[prop(optional, into)] label: Signal<String>,
    #[prop(optional, into)] label_for: Option<String>,
    #[prop(optional, into)] help_text: Signal<String>,
    /// Reactive alternative to `help_text`. When provided, it takes precedence.
    #[prop(optional, into)]
    help_signal: Option<Signal<String>>,
    #[prop(optional, into)] error: MaybeProp<String>,
    #[prop(optional, into)] class: Signal<String>,
    children: Children,
) -> impl IntoView {
    let children_view = children();

    view! {
        <div class=move || format!("form-group {}", class.get())>
            {move || {
                let lbl = label.get();
                if lbl.is_empty() {
                    view! {}.into_any()
                } else if let Some(lbl_for) = &label_for {
                    view! { <label class="form-label" for=lbl_for.clone()>{lbl}</label> }.into_any()
                } else {
                    view! { <div class="form-label">{lbl}</div> }.into_any()
                }
            }}

            {children_view}

            {move || {
                let has_help_sig = help_signal.as_ref().map(|s| !s.get().is_empty()).unwrap_or(false);
                let hlp_txt = help_text.get();
                if has_help_sig {
                    let sig_val = help_signal.as_ref().unwrap().get();
                    view! { <div class="form-hint">{sig_val}</div> }.into_any()
                } else if !hlp_txt.is_empty() {
                    view! { <div class="form-hint">{hlp_txt}</div> }.into_any()
                } else {
                    view! {}.into_any()
                }
            }}

            {move || {
                if let Some(err) = error.get() {
                    if !err.is_empty() {
                        view! { <div class="form-error text-danger text-sm mt-xs">{err}</div> }.into_any()
                    } else {
                        view! {}.into_any()
                    }
                } else {
                    view! {}.into_any()
                }
            }}
        </div>
    }
}
