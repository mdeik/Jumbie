use leptos::prelude::*;

/// Resolve a unique element id, generating one if empty.
pub fn resolve_field_id(id: &str) -> String {
    if id.is_empty() {
        format!("f-{}", uuid::Uuid::new_v4())
    } else {
        id.to_string()
    }
}

/// Validation display for ad-hoc form fields (outside `FormGroup`).
///
/// Renders a `.form-error` paragraph when the error is non-empty, or an
/// empty `<span>` placeholder to preserve layout spacing.
///
/// Use [`field_validation_with`] when you need `span_id` or `span_class`.
pub fn field_validation(error: impl Into<Signal<String>>) -> impl IntoView {
    field_validation_with(error, None, String::new())
}

/// Same as [`field_validation`] with `id` and `class` applied to both the
/// error `<p>` and the empty `<span>` placeholder.
pub fn field_validation_with(
    error: impl Into<Signal<String>>,
    id: Option<String>,
    class: impl Into<Signal<String>>,
) -> impl IntoView {
    let error = error.into();
    let class = class.into();
    move || {
        let msg = error.get();
        if msg.is_empty() {
            view! { <span id=id.clone() class=class.get()></span> }.into_any()
        } else {
            let cls = {
                let base = class.get();
                if base.is_empty() {
                    "form-error".to_string()
                } else {
                    format!("form-error {}", base)
                }
            };
            view! { <p id=id.clone() class=cls>{msg}</p> }.into_any()
        }
    }
}
