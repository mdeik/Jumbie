/// Universal sorting helper to reduce boilerplate in frontend components.
///
/// ```rust
/// use jumbie_frontend::utils::sorting::apply_sort;
/// use std::cmp::Ordering;
///
/// let mut items = vec!["c", "a", "b"];
/// apply_sort(&mut items, "name", true, |a, b, _| a.cmp(b));
/// assert_eq!(items, vec!["a", "b", "c"]);
///
/// apply_sort(&mut items, "name", false, |a, b, _| a.cmp(b));
/// assert_eq!(items, vec!["c", "b", "a"]);
/// ```
pub fn apply_sort<T>(
    items: &mut [T],
    column: &str,
    ascending: bool,
    compare: impl Fn(&T, &T, &str) -> std::cmp::Ordering,
) {
    items.sort_by(|a, b| {
        let res = compare(a, b, column);
        if ascending { res } else { res.reverse() }
    });
}

/// A wrapper to easily apply sorting using Leptos signals.
pub struct TableSorter<'a> {
    pub column: &'a str,
    pub ascending: bool,
}

impl<'a> TableSorter<'a> {
    pub fn new(column: &'a str, ascending: bool) -> Self {
        Self { column, ascending }
    }

    pub fn apply<T>(&self, items: &mut [T], compare: impl Fn(&T, &T, &str) -> std::cmp::Ordering) {
        apply_sort(items, self.column, self.ascending, compare);
    }
}

/// Build a [`Callback`] for use with [`TableBuilder::build_managed`] from a plain
/// comparison closure, removing the destructuring boilerplate at each call site.
///
/// Instead of:
/// ```ignore
/// Callback::new(|(a, b, col): (T, T, String)| match col.as_str() { ... })
/// ```
///
/// Write:
/// ```ignore
/// table_comparator(|a: &T, b: &T, col: &str| match col { ... })
/// ```
pub fn table_comparator<T: Clone + Send + Sync + 'static>(
    f: impl Fn(&T, &T, &str) -> std::cmp::Ordering + Send + Sync + 'static,
) -> leptos::prelude::Callback<(T, T, String), std::cmp::Ordering> {
    leptos::prelude::Callback::new(move |(a, b, col): (T, T, String)| f(&a, &b, &col))
}
