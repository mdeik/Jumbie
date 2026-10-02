use crate::utils::sorting::apply_sort;
use leptos::prelude::*;

/// Return type for [`use_table_sort`].
pub struct TableSortResult<T: Clone + Send + Sync + 'static> {
    pub sorted_data: Signal<Vec<T>>,
    pub sort_column: ReadSignal<String>,
    pub set_sort_column: WriteSignal<String>,
    pub sort_asc: ReadSignal<bool>,
    pub set_sort_asc: WriteSignal<bool>,
    pub toggle_sort: Callback<String>,
}

/// A reusable hook for table sorting logic.
pub fn use_table_sort<T: Clone + Send + Sync + 'static>(
    data: Signal<Vec<T>>,
    initial_column: String,
    initial_asc: bool,
    compare: impl Fn(&T, &T, &str) -> std::cmp::Ordering + Send + Sync + 'static,
) -> TableSortResult<T> {
    let (sort_column, set_sort_column) = signal(initial_column);
    let (sort_asc, set_sort_asc) = signal(initial_asc);
    let compare_stored = StoredValue::new_local(compare);

    let sorted_data = Signal::derive(move || {
        let mut items = data.get();
        let column = sort_column.get();
        let ascending = sort_asc.get();
        apply_sort(&mut items, &column, ascending, move |a, b, col| {
            compare_stored.with_value(|f| f(a, b, col))
        });
        items
    });

    let toggle_sort = Callback::new(move |column: String| {
        if sort_column.get() == column {
            set_sort_asc.update(|asc| *asc = !*asc);
        } else {
            set_sort_column.set(column);
            set_sort_asc.set(true);
        }
    });

    TableSortResult {
        sorted_data,
        sort_column,
        set_sort_column,
        sort_asc,
        set_sort_asc,
        toggle_sort,
    }
}
