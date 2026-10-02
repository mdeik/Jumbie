use crate::components::common::form_fields::FormGroup;
use leptos::prelude::*;

/// A standardized component for placing an action (like a button) next to an input.
#[component]
pub fn ActionField(
    #[prop(optional, into)] label: Signal<String>,
    #[prop(optional, into)] help_text: Signal<String>,
    #[prop(optional, into)] class: Signal<String>,
    children: Children,
) -> impl IntoView {
    view! {
        <FormGroup label=label help_text=help_text class=class>
            <div class="flex gap-sm items-center">
                {children()}
            </div>
        </FormGroup>
    }
}

/// A row with a description paragraph and an action button.
/// Uses `.action-row-body` / `.action-row-desc` for the same flex-row layout as
/// the Danger Zone. SSoT for this pattern (also used in SeasonManageModal,
/// AdvancedTab, and GeneralTab).
#[component]
pub fn ActionRow(
    /// Description text shown alongside the action button.
    #[prop(into)]
    description: Signal<String>,
    /// The action button(s).
    children: Children,
) -> impl IntoView {
    view! {
        <div class="action-row-body">
            <p class="action-row-desc">{description}</p>
            {children()}
        </div>
    }
}
