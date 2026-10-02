use leptos::prelude::*;
use std::collections::HashSet;
use std::hash::Hash;

/// Select-all state for a filtered managed table — the single source of truth
/// for the header checkbox and the selection set.
///
/// Enforced rules (identical for every table, so they can't drift):
/// - The header select-all box is **user-driven**: only an explicit user click
///   can check it. Selecting rows individually never auto-checks it.
/// - It auto-**unchecks** as soon as the filtered list contains an unselected
///   row (e.g. after the search changes).
/// - Checking selects the filtered rows; unchecking removes them. Selections
///   outside the current filter are **remembered**, so an apply targets every
///   selected row and the selected-count reflects them.
pub struct TableSelection<K: 'static> {
    /// Selected keys. Persists across filter changes.
    pub selected: ReadSignal<HashSet<K>>,
    /// Whether the user has checked the header select-all box.
    pub is_select_all: Signal<bool>,
    /// True when the filtered list is empty — the select-all box is disabled and
    /// forced off in that state.
    pub disabled: Signal<bool>,
    /// Feed to `TableBuilder::selection_mode` as `on_select_all`.
    pub select_all: Callback<bool>,
    /// Toggle a single row (row/card click or row checkbox).
    pub toggle: Callback<K>,
    /// Clear the selection and uncheck the box (edit-mode exit, post-op cleanup).
    pub clear: Callback<()>,
}

pub fn use_table_selection<T, K, F>(filtered: Signal<Vec<T>>, key: F) -> TableSelection<K>
where
    T: Clone + Send + Sync + 'static,
    K: Clone + Eq + Hash + Send + Sync + 'static,
    F: Fn(&T) -> K + Copy + Send + Sync + 'static,
{
    let (selected, set_selected) = signal(HashSet::<K>::new());
    let (is_select_all, set_is_select_all) = signal(false);

    // No rows in the filter -> nothing to select, so the box is disabled/off.
    let disabled = Signal::derive(move || filtered.get().is_empty());

    // True when there is at least one filtered row and all of them are selected.
    // Drives auto-uncheck only — it never checks the box on its own.
    let all_visible_selected = Signal::derive(move || {
        let visible = filtered.get();
        let selected = selected.get();
        !visible.is_empty() && visible.iter().all(|item| selected.contains(&key(item)))
    });

    Effect::new(move |_| {
        if is_select_all.get_untracked() && !all_visible_selected.get() {
            set_is_select_all.set(false);
        }
    });

    let toggle = Callback::new(move |k: K| {
        set_selected.try_update(|s| {
            if s.contains(&k) {
                s.remove(&k);
            } else {
                s.insert(k);
            }
        });
    });

    let select_all = Callback::new(move |checked: bool| {
        set_is_select_all.set(checked);
        let visible = filtered.get_untracked();
        set_selected.try_update(|s| {
            for item in visible {
                let k = key(&item);
                if checked {
                    s.insert(k);
                } else {
                    s.remove(&k);
                }
            }
        });
    });

    let clear = Callback::new(move |_: ()| {
        set_selected.try_update(|s| s.clear());
        set_is_select_all.set(false);
    });

    TableSelection {
        selected,
        is_select_all: Signal::derive(move || is_select_all.get()),
        disabled,
        select_all,
        toggle,
        clear,
    }
}
