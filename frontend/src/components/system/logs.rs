use crate::api::{fetch_log_level, fetch_logs};
use crate::components::common::pagination::PaginationControl;
use crate::components::common::table_builder::{ManagedColumn, TableBuilder, TableVariant};
use crate::components::common::{FormattedTimestamp, SearchInput, SettingsPage};
use crate::hooks::use_filter_override::{FilterOverride, use_filter_override};
use crate::hooks::use_paginated_sort_state::use_paginated_sort_state;
use crate::hooks::use_pagination::use_pagination;
use crate::hooks::use_ui_config::use_time_format;
use crate::utils::ListQueryParams;
use crate::utils::format_datetime_local;
use jumbie_shared::config::UIConfig;
use jumbie_shared::types::{LogEntry, level_priority};
use leptos::prelude::*;
use leptos::task::spawn_local;

/// Log levels as (value, display label) pairs.
const LEVEL_LABELS: &[(&str, &str)] = &[
    ("TRACE", "Trace"),
    ("DEBUG", "Debug"),
    ("INFO", "Info"),
    ("WARN", "Warn"),
    ("ERROR", "Error"),
];

/// Returns the CSS color variable for a log level.
fn log_level_color(level: &str) -> &'static str {
    match level {
        "ERROR" => "var(--accent-danger)",
        "WARN" => "var(--accent-warning)",
        "INFO" => "var(--accent-info)",
        "DEBUG" => "var(--accent-secondary)",
        _ => "var(--text-muted)",
    }
}

/// Returns the levels that are at or above the given backend level.
/// E.g. if backend_level is "INFO", returns INFO, WARN, ERROR.
fn available_levels(backend_level: Option<&str>) -> Vec<(&'static str, &'static str)> {
    let backend_priority = backend_level.map(level_priority).unwrap_or(1); // Default to TRACE so all levels show if unknown
    LEVEL_LABELS
        .iter()
        .copied()
        .filter(|(val, _)| level_priority(val) >= backend_priority)
        .collect()
}

#[component]
pub fn SystemLogs() -> impl IntoView {
    let time_format = use_time_format();

    // No override until the user changes it, so the first request omits
    // `min_level` and the backend applies the stored preference
    // (`resolve_log_min_level`).  Free-text search has no stored default, so it
    // is a plain local signal (committed by `SearchInput`).
    let FilterOverride {
        state: level_override,
        display: level_display,
        set: on_level_change,
    } = use_filter_override(
        |ui: &UIConfig| ui.logs.min_level.clone(),
        "INFO".to_string(),
        |ui: &mut UIConfig, val: String| ui.logs.min_level = val,
    );
    let (search_override, set_search_override) = signal::<Option<String>>(None);

    let sort = use_paginated_sort_state("system_logs".to_string(), "timestamp".to_string(), false);
    let sort_col = sort.column;
    let sort_asc = sort.ascending;
    let sort_override = sort.override_state;
    let on_sort = sort.on_sort;

    // Request and change-detection key are both built from `params`, so a
    // filter cannot be added to one without the other. A `None` override means
    // "the backend applies the stored preference"; it stays stable when
    // `ui_config` loads and the *display* value fills in, so it must not
    // trigger a re-fetch.
    //
    // `Signal::derive` has no equality check, so callers must not write an
    // override that is already set (see the guards in the setters/hook) — that
    // is the dedupe point, not the HTTP layer.
    let params = Signal::derive(move || {
        ListQueryParams::new()
            .sort(sort_override.get())
            .search(search_override.get())
            .filter("min_level", level_override.get())
    });
    let query_key = Signal::derive(move || params.get().key());

    let pagination = use_pagination(
        "fetch_logs",
        move |page| {
            let p = params.get_untracked();
            async move {
                fetch_logs(
                    page,
                    100,
                    p.search_value(),
                    p.sort_state(),
                    p.filter_value("min_level"),
                )
                .await
            }
        },
        100,
        true,
        Some(30_000),
        query_key,
    );

    // Fetch the backend's actual logging level so we only show options that
    // make sense — no point offering TRACE if the backend isn't capturing it.
    let (backend_level, set_backend_level) = signal::<Option<String>>(None);
    spawn_local(async move {
        if let Ok(level) = fetch_log_level().await {
            set_backend_level.set(Some(level));
        }
    });

    // Auto-upgrade the filter if it's set lower than what the backend logs.
    // E.g. if the user had min_level=TRACE saved but the backend is at INFO,
    // bump the filter to INFO so they're not requesting impossible data.
    Effect::new({
        let on_level_change = on_level_change.clone();
        move || {
            if let Some(ref bl) = backend_level.get() {
                let current = level_display.get();
                if level_priority(&current) < level_priority(bl) {
                    on_level_change.run(bl.clone());
                }
            }
        }
    });

    let dropdown_levels = move || available_levels(backend_level.get().as_deref());

    let columns: Vec<ManagedColumn<LogEntry>> = vec![
        ManagedColumn {
            id: "timestamp".into(),
            label: "Time".into(),
            sortable: true,
            class: "logs-timestamp".into(),
            cell_render: Callback::new({
                let tf = time_format.clone();
                move |log: LogEntry| {
                    let formatted = format_datetime_local(&log.timestamp, &tf.get());
                    view! { <FormattedTimestamp value=formatted /> }.into_any()
                }
            }),
        },
        ManagedColumn {
            id: "level".into(),
            label: "Level".into(),
            sortable: true,
            class: "font-bold".into(),
            cell_render: Callback::new(move |log: LogEntry| {
                let color = log_level_color(&log.level);
                view! { <span style=format!("color: {}", color)>{log.level}</span> }.into_any()
            }),
        },
        ManagedColumn {
            id: "message".into(),
            label: "Message".into(),
            sortable: false,
            class: "".into(),
            cell_render: Callback::new(|log: LogEntry| {
                view! { <span class="text-sm whitespace-pre-wrap">{log.message}</span> }.into_any()
            }),
        },
    ];

    // No-op compare — unused because `.preserve_order()` is set below.
    // Sorting is handled server-side via the sort parameter sent to the API.
    let compare = Callback::new(move |_: (LogEntry, LogEntry, String)| std::cmp::Ordering::Equal);

    view! {
        <SettingsPage id="logs">
            <div class="table-header pt-0">
                <div class="flex items-center gap-md w-full">
                    <div class="flex items-center gap-sm">
                        <label for="minLevelSelect" class="form-label mb-0">"Min Level:"</label>
                        <select
                            id="minLevelSelect"
                            class="strict-select w-auto"
                            prop:value=move || level_display.get()
                            on:change=move |ev| {
                                let val = event_target_value(&ev);
                                on_level_change.run(val);
                        }
                    >
                        {move || dropdown_levels().into_iter().map(|(val, label)| {
                            let is_selected = level_display.get() == val;
                            view! {
                                <option value=val selected=is_selected>{label}</option>
                            }
                        }).collect::<Vec<_>>()}
                    </select>
                    </div>
                    // Debounced free-text filter; also commits on Enter.
                    <SearchInput
                        id="logSearchInput"
                        placeholder="Filter messages…"
                        class="form-input flex-1"
                        on_search=Callback::new(move |v: String| {
                            set_search_override.set(if v.is_empty() { None } else { Some(v) });
                        })
                    />
                </div>
            </div>
            <div>
                {TableBuilder::new(sort_col, sort_asc)
                    .table_variant(TableVariant::Hover)
                    .container_class(Signal::derive(move || "table-container".to_string()))
                    .on_sort(on_sort)
                    .loading(pagination.loading)
                    .empty_message("No logs found.")
                    .preserve_order()
                    .build_managed(
                        pagination.items,
                        columns,
                        compare,
                        None::<Callback<LogEntry>>,
                        None::<Callback<LogEntry, bool>>,
                        Some(Callback::new(move |log: LogEntry| {
                            let color = log_level_color(&log.level);
                            view! {
                                <div class="card p-md bg-secondary border border-radius flex flex-col gap-xs">
                                    <div class="flex justify-between items-center mb-xs">
                                        <span class="text-xs font-bold" style=format!("color: {}", color)>{log.level}</span>
                                        <span class="text-xs text-muted font-mono">{
                                            format_datetime_local(&log.timestamp, &time_format.get())
                                        }</span>
                                    </div>
                                    <div class="text-sm whitespace-pre-wrap selectable">{log.message}</div>
                                </div>
                            }.into_any()
                        }))
                    )}
            </div>
            <PaginationControl
                page=pagination.page
                total_pages=pagination.total_pages
                on_page_change=pagination.set_page
            />
        </SettingsPage>
    }
}
