use crate::hooks::use_ui_config::{UIConfigContext, update_ui_config, use_ui_config};
use jumbie_shared::config::TableSortState;
use leptos::prelude::*;

/// Sort state for a paginated table that persists to the user's server-side
/// preferences.
///
/// The persisted sort lives in `ui_preferences` on the backend (not a browser
/// concern), and the list endpoints apply it when the request omits `sort`/`order`
/// (see `resolve_table_sort` on the backend). The frontend therefore drives the
/// request from an explicit override:
///
/// * `override_state == None` → the request **omits** the sort and the backend
///   applies the stored preference. The UI still *displays* the stored value
///   (via `column`/`ascending`) so the header arrow is right.
/// * `override_state == Some(..)` → the user clicked a header, so the request
///   carries an explicit sort.
///
/// Driving the request from the override (not the display value) avoids a
/// redundant re-fetch: when `ui_config` arrives after a hard refresh and the
/// display value changes from the default to the stored sort, the override is
/// still `None`, so the query key is unchanged and nothing refetches.
pub struct PaginatedSortState {
    /// Column shown in the header (override → stored preference → default).
    pub column: Signal<String>,
    /// Direction shown in the header (override → stored preference → default).
    pub ascending: Signal<bool>,
    /// Explicit user override. `None` means "let the backend apply the stored
    /// preference" — pass this straight through to the fetcher.
    pub override_state: ReadSignal<Option<TableSortState>>,
    /// Callback to be passed to `TableBuilder::on_sort`.
    pub on_sort: Callback<String>,
}

/// Local sort state for paginated tables with **immediate reactivity**: the local
/// override signal updates synchronously, whereas `ui_config` updates
/// asynchronously (effects run as microtasks in Leptos) — without the local signal,
/// a fetcher reading the override via `get_untracked()` right after a change would
/// get the stale value.
///
/// # Usage
///
/// ```ignore
/// let sort = use_paginated_sort_state("my_table".to_string(), "name".to_string(), false);
///
/// // Fetcher: pass the override straight through (None → omit the param).
/// let pagination = use_pagination("fetch_my_table", move |page| {
///     let sort = sort.override_state.get_untracked();
///     async move { fetch_my_table(page, sort).await }
/// }, 100, true, None, query_key);
///
/// // Query key reflects only the override (stable across the ui_config load).
/// // Don't write an override that is already set — `Signal::derive` has no
/// // equality check, so that would re-run the pagination effect.
/// let query_key = Signal::derive(move || {
///     sort.override_state.get().map(|s| format!("{}|{}", s.column, s.ascending)).unwrap_or_default()
/// });
/// ```
pub fn use_paginated_sort_state(
    persistence_id: String,
    default_col: String,
    default_asc: bool,
) -> PaginatedSortState {
    let UIConfigContext {
        ui_config,
        set_ui_config,
    } = use_ui_config();

    // Explicit override (None until the user sorts)
    let (override_state, set_override_state) = signal::<Option<TableSortState>>(None);

    // Display values (override → stored preference → default); these drive the
    // header arrow only and never affect the request.
    let pid_display_col = persistence_id.clone();
    let default_col_display = default_col.clone();
    let column = Signal::derive(move || {
        override_state
            .get()
            .map(|s| s.column)
            .or_else(|| {
                ui_config.get().and_then(|ui| {
                    ui.table_sorts
                        .get(&pid_display_col)
                        .map(|s| s.column.clone())
                })
            })
            .unwrap_or_else(|| default_col_display.clone())
    });
    let pid_display_asc = persistence_id.clone();
    let ascending = Signal::derive(move || {
        override_state
            .get()
            .map(|s| s.ascending)
            .or_else(|| {
                ui_config
                    .get()
                    .and_then(|ui| ui.table_sorts.get(&pid_display_asc).map(|s| s.ascending))
            })
            .unwrap_or(default_asc)
    });

    // Sort callback: set the override + persist
    let ctx = UIConfigContext {
        ui_config,
        set_ui_config,
    };
    let on_sort = Callback::new(move |col: String| {
        // Toggle direction when re-clicking the active column, otherwise sort
        // the new column ascending.
        let current_col = column.get_untracked();
        let current_asc = ascending.get_untracked();
        let (new_col, new_asc) = if current_col == col {
            (col, !current_asc)
        } else {
            (col, true)
        };
        let state = TableSortState {
            column: new_col,
            ascending: new_asc,
        };
        set_override_state.set(Some(state.clone()));

        // Persist to ui_config (session + cache) and the backend, so the
        // server-side default matches on the next request and stays in sync
        // for other clients.
        let pid = persistence_id.clone();
        update_ui_config(&ctx, move |ui| {
            ui.table_sorts.insert(pid, state);
        });
    });

    PaginatedSortState {
        column,
        ascending,
        override_state,
        on_sort,
    }
}
