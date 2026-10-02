use leptos::prelude::*;

#[derive(Clone, Copy)]
pub struct LayoutContext {
    pub sidebar_open: ReadSignal<bool>,
    pub set_sidebar_open: WriteSignal<bool>,
    /// Optional title override pushed by individual views (e.g. series name on edit page).
    pub header_title_override: ReadSignal<Option<String>>,
    pub set_header_title_override: WriteSignal<Option<String>>,
}

pub fn use_layout() -> LayoutContext {
    let (sidebar_open, set_sidebar_open) = signal(false);
    let (header_title_override, set_header_title_override) = signal::<Option<String>>(None);

    let context = LayoutContext {
        sidebar_open,
        set_sidebar_open,
        header_title_override,
        set_header_title_override,
    };
    provide_context(context);
    context
}
