use leptos::prelude::*;

pub struct NestedCheckboxItem {
    pub id: String,
    pub label: String,
    pub checked: Signal<bool>,
    pub set_checked: WriteSignal<bool>,
}

impl Clone for NestedCheckboxItem {
    fn clone(&self) -> Self {
        Self {
            id: self.id.clone(),
            label: self.label.clone(),
            checked: self.checked,
            set_checked: self.set_checked,
        }
    }
}

/// Renders a parent checkbox with nested children that are forced on and
/// disabled when the parent is checked. Children can be toggled independently
/// when the parent is unchecked.
///
/// When the parent is checked, the child's `set_checked` is **not** invoked — the
/// visual state derives from the parent alone, so no per-child reactive side
/// effects (e.g. API calls) fire. Consumers must derive the effective child state
/// as `child_checked || parent_checked`.
#[component]
pub fn NestedCheckboxGroup(
    parent_id: &'static str,
    parent_label: &'static str,
    parent_checked: Signal<bool>,
    set_parent_checked: WriteSignal<bool>,
    children: Vec<NestedCheckboxItem>,
) -> impl IntoView {
    view! {
        <div class="flex flex-col gap-sm mt-lg">
            <div class="form-check">
                <input
                    type="checkbox"
                    class="form-check-input"
                    id=parent_id
                    prop:checked=parent_checked
                    on:change=move |ev| {
                        let checked = event_target_checked(&ev);
                        set_parent_checked.set(checked);
                    }
                />
                <label class="form-check-label" for=parent_id>
                    {parent_label}
                </label>
            </div>

            <div class="nested-checkboxes">
                {children.into_iter().map(|child| {
                    let child_id = child.id.clone();
                    let child_label = child.label;
                    let child_checked = child.checked;
                    view! {
                        <div class="form-check checkbox-group-sub">
                            <input
                                type="checkbox"
                                class=move || {
                                    let base = "form-check-input".to_string();
                                    if parent_checked.get() {
                                        format!("{} forced-active", base)
                                    } else {
                                        base
                                    }
                                }
                                id=child_id.clone()
                                prop:checked=move || parent_checked.get() || child_checked.get()
                                disabled=move || parent_checked.get()
                                on:change=move |ev| {
                                    child.set_checked.set(event_target_checked(&ev));
                                }
                            />
                            <label class="form-check-label" for=child_id>
                                {child_label}
                            </label>
                        </div>
                    }
                }).collect::<Vec<_>>()}
            </div>
        </div>
    }
}
