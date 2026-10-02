use crate::hooks::use_ui_config::{UIConfigContext, update_ui_config, use_ui_config};
use jumbie_shared::config::TableSortState;
use leptos::prelude::*;

/// A hook to manage table sorting state with persistent storage in user preferences.
///
/// Returns `(sort_column, sort_ascending, on_sort)` where `on_sort` is a callback
/// to be passed to `TableBuilder::on_sort`.
///
/// Uses **local signals** (not derived from `ui_config`) so sort updates are visible
/// **synchronously** — no microtask delay. New state is also pushed into `ui_config`
/// for session persistence and saved to the backend in the background.
pub fn use_persistent_table_state(
    persistence_id: String,
    default_col: String,
    default_asc: bool,
) -> (Signal<String>, Signal<bool>, Callback<String>) {
    let UIConfigContext {
        ui_config,
        set_ui_config,
    } = use_ui_config();

    // Local signals (synchronous reads, not derived from ui_config)
    let init = ui_config
        .get_untracked()
        .and_then(|ui| ui.table_sorts.get(&persistence_id).cloned())
        .unwrap_or(TableSortState {
            column: default_col.clone(),
            ascending: default_asc,
        });
    let (sort_col, set_sort_col) = signal(init.column);
    let (sort_asc, set_sort_asc) = signal(init.ascending);

    // Sync local signals with ui_config once the preloader populates it. On first
    // render ui_config may still be None; this Effect picks up persisted state once
    // it becomes available without overwriting a user-initiated sort, because the
    // sort callback pushes into ui_config at the same time as it updates the locals.
    let pid_for_effect = persistence_id.clone();
    Effect::new(move |_| {
        if let Some(ui) = ui_config.get()
            && let Some(state) = ui.table_sorts.get(&pid_for_effect)
        {
            // Only push if the value actually changed — avoids redundant
            // re-renders when on_sort has already pushed the same value.
            if sort_col.get_untracked() != state.column
                || sort_asc.get_untracked() != state.ascending
            {
                set_sort_col.set(state.column.clone());
                set_sort_asc.set(state.ascending);
            }
        }
    });

    // Sort callback
    let ctx = UIConfigContext {
        ui_config,
        set_ui_config,
    };
    let on_sort = Callback::new(move |col: String| {
        let current_col = sort_col.get_untracked();
        let current_asc = sort_asc.get_untracked();

        let (new_col, new_asc) = if current_col == col {
            (col.clone(), !current_asc)
        } else {
            (col.clone(), true)
        };

        // 1. Update local signals immediately (synchronous — consumers see the new value now)
        set_sort_col.set(new_col.clone());
        set_sort_asc.set(new_asc);

        // 2. Persist to ui_config (session + backend)
        let pid = persistence_id.clone();
        update_ui_config(&ctx, move |ui| {
            ui.table_sorts.insert(
                pid,
                TableSortState {
                    column: new_col,
                    ascending: new_asc,
                },
            );
        });
    });

    (sort_col.into(), sort_asc.into(), on_sort)
}
