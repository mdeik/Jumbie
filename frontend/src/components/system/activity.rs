use crate::components::common::icons::{CheckIcon, SquareIcon, TagsIcon};
use crate::components::common::standard_modal::StandardModal;
use crate::components::common::table_builder::{ManagedColumn, TableBuilder, TableVariant};
use crate::components::common::toast::show_error;
use crate::components::common::{SearchInput, ViewShell, pagination::PaginationControl};
use crate::hooks::use_filter_override::{FilterOverride, use_filter_override};
use crate::hooks::use_pagination::use_pagination;
use crate::utils::ListQueryParams;
use crate::utils::format_age;
use jumbie_shared::config::UIConfig;
use jumbie_shared::types::ActivityItem;
use leptos::prelude::*;

/// Activity types as (lowercase value, display label) pairs.
const ALL_TYPES: &[(&str, &str)] = &[
    ("download", "Download"),
    ("import", "Import"),
    ("metadata", "Metadata"),
    ("reassign", "Reassign"),
    ("assign", "Assign"),
    ("analyze", "Analyze"),
    ("delete", "Delete"),
    ("unassign", "Unassign"),
];

/// Label for the filter button: "All types", a single type's label, or "N types".
fn filter_button_label(selected: &[String]) -> String {
    let all_count = ALL_TYPES.len();
    if selected.is_empty() || selected.len() == all_count {
        "All types".to_string()
    } else if selected.len() == 1 {
        // Find the display label for the single selected type
        ALL_TYPES
            .iter()
            .find(|(val, _)| val == &selected[0])
            .map(|(_, label)| *label)
            .unwrap_or(&selected[0])
            .to_string()
    } else {
        format!("{} types", selected.len())
    }
}

#[component]
pub fn Activity() -> impl IntoView {
    // No override until the user applies a filter, so the first request omits
    // `types` and the backend applies the stored preference
    // (`resolve_activity_types`).  A committed empty selection ("show all") is an
    // explicit override, distinct from "no override" (see `filter_segment`).
    let FilterOverride {
        state: type_override,
        display: type_display,
        set: apply_filter,
    } = use_filter_override(
        |ui: &UIConfig| ui.activity.filter_types.clone(),
        Vec::<String>::new(),
        |ui: &mut UIConfig, types: Vec<String>| ui.activity.filter_types = types,
    );
    // Free-text filter over title/details/status. No stored default.
    let (search_override, set_search_override) = signal::<Option<String>>(None);

    let (show_modal, set_show_modal) = signal(false);
    // Draft types within the modal — local copy that only commits on Apply
    let initial_draft = type_display.get_untracked();
    let (draft_types, set_draft_types) = signal(initial_draft);

    let sort = crate::hooks::use_paginated_sort_state::use_paginated_sort_state(
        "activity".to_string(),
        "timestamp".to_string(),
        false,
    );
    let sort_col = sort.column;
    let sort_asc = sort.ascending;
    let sort_override = sort.override_state;
    let on_sort = sort.on_sort;

    // `None` (backend applies the stored filter) and an explicit empty selection
    // ("show all types") are distinct here, so clearing the filter refetches.
    let params = Signal::derive(move || {
        ListQueryParams::new()
            .sort(sort_override.get())
            .search(search_override.get())
            .filter("types", type_override.get().map(|v| v.join(",")))
    });
    let query_key = Signal::derive(move || params.get().key());

    let pagination = use_pagination(
        "fetch_activity",
        move |page| {
            let p = params.get_untracked();
            async move {
                crate::api::fetch_activity(
                    page,
                    50,
                    p.search_value(),
                    p.sort_state(),
                    p.filter_value("types"),
                )
                .await
                .map_err(|e| {
                    show_error(format!("Failed to fetch activity: {}", e));
                    e
                })
            }
        },
        50,
        true,
        Some(30_000),
        query_key,
    );

    let open_modal = Callback::new({
        let set_draft_types = set_draft_types.clone();
        move |()| {
            set_draft_types.set(type_display.get_untracked());
            set_show_modal.set(true);
        }
    });

    let close_modal = Callback::new({
        let set_show_modal = set_show_modal.clone();
        move |()| set_show_modal.set(false)
    });

    let toggle_type = Callback::new({
        let set_draft_types = set_draft_types.clone();
        move |val: String| {
            set_draft_types.update(|draft| {
                if let Some(pos) = draft.iter().position(|t| t == &val) {
                    draft.remove(pos);
                } else {
                    draft.push(val);
                }
            });
        }
    });

    let select_all = Callback::new({
        let set_draft_types = set_draft_types.clone();
        move |()| {
            set_draft_types.set(
                ALL_TYPES
                    .iter()
                    .map(|(val, _)| (*val).to_string())
                    .collect(),
            );
        }
    });

    let clear_all = Callback::new({
        let set_draft_types = set_draft_types.clone();
        move |()| set_draft_types.set(Vec::new())
    });

    let columns: Vec<ManagedColumn<ActivityItem>> = vec![
        ManagedColumn {
            id: "timestamp".into(),
            label: "Age".into(),
            sortable: true,
            class: "font-mono".into(),
            cell_render: Callback::new(move |item: ActivityItem| {
                view! { <span>{format_age(&item.timestamp)}</span> }.into_any()
            }),
        },
        ManagedColumn {
            id: "type".into(),
            label: "Type".into(),
            sortable: true,
            class: "w-15".into(),
            cell_render: Callback::new(move |item: ActivityItem| {
                let (type_class, type_label) = item.activity_type.display_info();
                view! { <span class={type_class}>{type_label}</span> }.into_any()
            }),
        },
        ManagedColumn {
            id: "title".into(),
            label: "Title".into(),
            sortable: true,
            class: "activity-title".into(),
            cell_render: Callback::new(move |item: ActivityItem| {
                view! { <span>{item.title}</span> }.into_any()
            }),
        },
        ManagedColumn {
            id: "details".into(),
            label: "Details".into(),
            sortable: false,
            class: "activity-details".into(),
            cell_render: Callback::new(move |item: ActivityItem| {
                view! { <span>{item.details.unwrap_or_default()}</span> }.into_any()
            }),
        },
        ManagedColumn {
            id: "status".into(),
            label: "Status".into(),
            sortable: true,
            class: "w-10".into(),
            cell_render: Callback::new(move |item: ActivityItem| {
                view! { <span>{item.status}</span> }.into_any()
            }),
        },
    ];

    // Discriminant ordering — avoids allocating two Strings per comparison.
    let type_order = |t: &jumbie_shared::types::ActivityType| -> u8 {
        use jumbie_shared::types::ActivityType::*;
        match t {
            Download => 0,
            Import => 1,
            Metadata => 2,
            Reassign => 3,
            Assign => 4,
            Analyze => 5,
            Delete => 6,
            Unassign => 7,
        }
    };

    let compare =
        Callback::new(move |(a, b, col): (ActivityItem, ActivityItem, String)| {
            match col.as_str() {
                "timestamp" => a.timestamp.cmp(&b.timestamp),
                "type" => type_order(&a.activity_type).cmp(&type_order(&b.activity_type)),
                "title" => jumbie_shared::formatting::natural_cmp(&a.title, &b.title),
                "status" => jumbie_shared::formatting::natural_cmp(&a.status, &b.status),
                _ => std::cmp::Ordering::Equal,
            }
        });

    view! {
        <ViewShell id="activity">
            <div class="table-header pt-0">
                <div class="flex items-center gap-md w-full">
                    <button
                        class="btn btn-ghost flex items-center gap-xs"
                        on:click=move |_| open_modal.run(())
                        type="button"
                    >
                        <span class="icon text-base"><TagsIcon /></span>
                        <span>{move || filter_button_label(&type_display.get())}</span>
                    </button>
                    <SearchInput
                        id="activitySearchInput"
                        placeholder="Search activity…"
                        class="form-input flex-1"
                        on_search=Callback::new(move |v: String| {
                            set_search_override.set(if v.is_empty() { None } else { Some(v) });
                        })
                    />
                </div>
            </div>

            <StandardModal
                show=show_modal
                on_close=close_modal
                title=Signal::derive(move || "Filter Activity Types".to_string())
                size=Signal::derive(move || "modal-sm".to_string())
                footer={view! {
                    <div class="modal-footer flex items-center justify-end gap-md">
                        <button
                            class="btn btn-primary"
                            on:click=move |_| {
                                apply_filter.run(draft_types.get_untracked());
                                close_modal.run(());
                            }
                            type="button"
                        >"Apply"</button>
                    </div>
                }.into_any()}
            >
                <div class="flex flex-col gap-sm">
                    <div class="activity-filter-actions">
                        <button
                            class="btn btn-ghost"
                            on:click=move |_| select_all.run(())
                            type="button"
                        >"Select All"</button>
                        <button
                            class="btn btn-ghost"
                            on:click=move |_| clear_all.run(())
                            type="button"
                        >"Clear"</button>
                    </div>
                    <div class="grid grid-cols-2 gap-sm">
                    {ALL_TYPES.iter().map(|(val, label)| {
                        let id_for_class = val.to_string();
                        let id_for_click = val.to_string();
                        let id_for_inner = id_for_class.clone();
                        let name_for_text = label;
                        view! {
                            <button
                                type="button"
                                class=move || {
                                    if draft_types.get().contains(&id_for_class) {
                                        "btn activity-type-btn activity-type-btn-on"
                                    } else {
                                        "btn activity-type-btn activity-type-btn-off"
                                    }
                                }
                                on:click=move |_| toggle_type.run(id_for_click.clone())
                            >
                                {move || {
                                    if draft_types.get().contains(&id_for_inner) {
                                        view! { <span class="flex items-center gap-xs"><CheckIcon /> {*name_for_text}</span> }.into_any()
                                    } else {
                                        view! { <span class="flex items-center gap-xs"><SquareIcon /> {*name_for_text}</span> }.into_any()
                                    }
                                }}
                            </button>
                        }
                    }).collect::<Vec<_>>()}
                    </div>
                </div>
            </StandardModal>

            {
                TableBuilder::new(sort_col, sort_asc)
                    .container_class(Signal::derive(move || "table-container".to_string()))
                    .table_variant(TableVariant::Hover)
                    .loading(pagination.loading)
                    .empty_message("No activity recorded")
                    .on_sort(on_sort)
                    .build_managed(
                        pagination.items,
                        columns,
                        compare,
                        None,
                        None,
                        Some(Callback::new(move |item: ActivityItem| {
                            let (type_class, type_label) = item.activity_type.display_info();
                            let time_display = format_age(&item.timestamp);
                            view! {
                                <div class="card p-md bg-secondary border border-radius flex flex-col gap-sm">
                                    <div class="card-row-with-badge">
                                        <div class="flex flex-col min-w-0">
                                            <span class="text-xs text-muted mb-xs">{time_display}</span>
                                            <strong class="truncate card-row-title text-base">{item.title}</strong>
                                        </div>
                                        <span class={format!("{} p-xs text-xs border-radius flex-shrink-0", type_class)}>{type_label}</span>
                                    </div>
                                    <div class="text-sm activity-details">{item.details.unwrap_or_default()}</div>
                                    <div class="text-xs text-muted mt-xs">{format!("Status: {}", item.status)}</div>
                                </div>
                            }.into_any()
                        })),
                    )
            }
        </ViewShell>
        <PaginationControl
            page=pagination.page
            total_pages=pagination.total_pages
            on_page_change=pagination.set_page
        />
    }
}
