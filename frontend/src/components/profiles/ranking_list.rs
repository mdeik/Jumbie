use crate::components::common::icons::PlusIcon;
use leptos::prelude::*;
use std::sync::Arc;

/// A reusable ranked list editor with text key, numeric score, add/remove controls,
/// and an optional case-insensitive toggle. Used by the Submitter and Terms
/// ranking sections of `ReleaseProfileEditor`.
#[component]
pub fn RankingList(
    title: &'static str,
    list_id: &'static str,
    placeholder: &'static str,
    items: ReadSignal<Vec<(usize, String, i32)>>,
    set_items: WriteSignal<Vec<(usize, String, i32)>>,
    next_id: ReadSignal<usize>,
    set_next_id: WriteSignal<usize>,
    case_insensitive: ReadSignal<bool>,
    set_case_insensitive: WriteSignal<bool>,
    /// Optional per-row render callback injected after the score input.
    /// Receives the row id. Use `Option::<Box<dyn Fn(usize) -> AnyView>>::None` when not needed.
    #[prop(optional)]
    extra: Option<Arc<dyn Fn(usize) -> AnyView + Send + Sync>>,
) -> impl IntoView {
    view! {
        <div class="mt-lg">
            <div class="ranking-list-header">
                <div class="flex items-center gap-md">
                    <h3 class="m-0">{title}</h3>
                    <label class="flex items-center cursor-pointer m-0 mr-sm" for=format!("cb-{}", list_id)>
                        <input
                            type="checkbox"
                            id=format!("cb-{}", list_id)
                            prop:checked=move || case_insensitive.get()
                            on:change=move |ev| {
                                set_case_insensitive.set(event_target_checked(&ev));
                            }
                        />
                        "Case insensitive"
                    </label>
                </div>
                <button class="btn btn-secondary btn-sm" on:click=move |_| {
                    set_items.update(|t| {
                        let id = next_id.get_untracked();
                        t.push((id, String::new(), 0));
                        set_next_id.set(id + 1);
                    });
                }>
                    <span class="icon"><PlusIcon /></span>
                    "Add Row"
                </button>
            </div>
            <div id=list_id class="list-group">
                <For
                    each=move || items.get()
                    key=|(id, _, _)| *id
                    children=move |(row_id, _, _score)| {
                        view! {
                            <div class="list-group-item">
                                <input
                                    type="text"
                                    id=format!("{}-term-{}", list_id, row_id)
                                    class="form-control term-input"
                                    prop:value=move || {
                                        items.with(|t| t.iter().find(|i| i.0 == row_id).map(|i| i.1.clone()).unwrap_or_default())
                                    }
                                    placeholder=placeholder
                                    on:input=move |ev| {
                                        let new_key = event_target_value(&ev);
                                        set_items.update(|t| {
                                            if let Some(item) = t.iter_mut().find(|i| i.0 == row_id) {
                                                item.1 = new_key;
                                            }
                                        });
                                    }
                                />
                                <input
                                    type="number"
                                    id=format!("{}-score-{}", list_id, row_id)
                                    class="form-control flex-1"
                                    placeholder="Score"
                                    prop:value=move || {
                                        items.with(|t| t.iter().find(|i| i.0 == row_id).map(|i| i.2.to_string()).unwrap_or_default())
                                    }
                                    on:input=move |ev| {
                                        if let Ok(new_score) = event_target_value(&ev).parse::<i32>() {
                                            set_items.update(|t| {
                                                if let Some(item) = t.iter_mut().find(|i| i.0 == row_id) {
                                                    item.2 = new_score;
                                                }
                                            });
                                        }
                                    }
                                />
                                {extra.as_ref().map(|f| f(row_id))}
                                <button class="btn-danger btn-sm" on:click=move |_| {
                                    set_items.update(|t| {
                                        t.retain(|i| i.0 != row_id);
                                    });
                                }>{"×"}</button>
                            </div>
                        }
                    }
                />
            </div>
        </div>
    }
}
