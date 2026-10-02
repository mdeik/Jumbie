// Pagination query parameters — SSoT for "what makes this request distinct".
//
// Two things must agree for a paginated list: the query key handed to
// `use_pagination` (it refetches iff the key changes) and the request parameters
// actually sent to the API. [`ListQueryParams`] is the single value both are built
// from — `key()` for the former and the typed accessors for the latter.
//
// The subtle case is an *optional* filter: `None` means "no user override — the
// backend applies the stored preference", while `Some("")` is an explicit empty
// selection. A naive `.unwrap_or_default()` would collapse both and never refetch
// on clearing; the encoding below keeps them distinct.

use jumbie_shared::config::TableSortState;

/// Sentinel for "no override" in a query key.
///
/// `\0` cannot occur in a real filter value or sort column name, so it can never
/// collide with the empty string produced by an explicit empty `Some("")`.
const NO_OVERRIDE: &str = "\u{0}";

/// The query-key contribution of an optional filter value.
///
/// `None` (defer to the stored preference) and `Some("")` (explicitly empty)
/// yield distinct segments.
fn filter_segment(value: Option<&str>) -> String {
    match value {
        None => NO_OVERRIDE.to_string(),
        Some(v) => v.to_string(),
    }
}

/// The query-key contribution of an optional sort override.
///
/// `None` (defer to the stored sort) is distinct from any explicit column.
fn sort_segment(state: Option<TableSortState>) -> String {
    state
        .map(|s| format!("{}|{}", s.column, s.ascending))
        .unwrap_or_else(|| NO_OVERRIDE.to_string())
}

/// Join per-parameter segments into the single key `use_pagination` tracks.
fn query_key(segments: &[String]) -> String {
    segments.join("|")
}

/// The non-page parameters of a list request.
///
/// Built with the chained `sort`/`search`/`filter` setters and consumed twice:
/// [`key`](Self::key) for change detection and the typed accessors for the API
/// call.  See the module docs for why that matters.
#[derive(Clone, Debug, Default)]
pub struct ListQueryParams {
    sort: Option<TableSortState>,
    search: Option<String>,
    /// Endpoint-specific filters as `(name, value)` pairs, where `name` is the
    /// query parameter (`"min_level"`, `"types"`, …).
    filters: Vec<(&'static str, Option<String>)>,
}

impl ListQueryParams {
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the sort override (`None` = the backend applies the stored sort).
    pub fn sort(mut self, sort: Option<TableSortState>) -> Self {
        self.sort = sort;
        self
    }

    /// Set the free-text search. `None` or empty means "no text filter" (there
    /// is no stored `search` preference).
    pub fn search(mut self, search: Option<String>) -> Self {
        self.search = search;
        self
    }

    /// Add an endpoint-specific filter. `None` means "defer to the stored
    /// default"; `Some("")` is an explicit empty value.
    ///
    /// Re-adding the same name replaces it, so the builder is idempotent.
    pub fn filter(mut self, name: &'static str, value: Option<String>) -> Self {
        self.filters.retain(|(n, _)| *n != name);
        self.filters.push((name, value));
        self
    }

    /// The sort override, for the request builder.
    pub fn sort_state(&self) -> Option<TableSortState> {
        self.sort.clone()
    }

    /// The search value, for the request builder.
    pub fn search_value(&self) -> Option<String> {
        self.search.clone()
    }

    /// A filter value by parameter name, for the request builder.
    pub fn filter_value(&self, name: &str) -> Option<String> {
        self.filters
            .iter()
            .find(|(n, _)| *n == name)
            .and_then(|(_, v)| v.clone())
    }

    /// Stable identity for `use_pagination`: changes **iff** any parameter
    /// changes. Order-independent (filters are sorted by name).
    pub fn key(&self) -> String {
        let mut segments = vec![
            sort_segment(self.sort.clone()),
            filter_segment(self.search.as_deref()),
        ];

        let mut filters: Vec<(&str, Option<&str>)> = self
            .filters
            .iter()
            .map(|(name, value)| (*name, value.as_deref()))
            .collect();
        filters.sort_by(|a, b| a.0.cmp(b.0));
        for (name, value) in filters {
            segments.push(format!("{}={}", name, filter_segment(value)));
        }

        query_key(&segments)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sort(column: &str, ascending: bool) -> Option<TableSortState> {
        Some(TableSortState {
            column: column.to_string(),
            ascending,
        })
    }

    // The bug this module exists to prevent

    #[test]
    fn no_override_differs_from_explicit_empty_filter() {
        // The activity "clear all types" case: without this distinction the
        // query key would not change, so the refetch never happened.
        assert_ne!(filter_segment(None), filter_segment(Some("")));
    }

    // filter_segment / sort_segment

    #[test]
    fn filter_segment_passes_value_through() {
        assert_eq!(filter_segment(Some("start")), "start");
        assert_eq!(filter_segment(Some("")), "");
    }

    #[test]
    fn sort_segment_encodes_column_and_direction() {
        assert_eq!(sort_segment(sort("level", true)), "level|true");
        assert_eq!(sort_segment(sort("timestamp", false)), "timestamp|false");
    }

    #[test]
    fn sort_segment_none_is_distinct_from_empty_column() {
        assert_ne!(sort_segment(None), sort_segment(sort("", false)));
    }

    // ListQueryParams: key changes iff a parameter changes. These guard the class
    // of bug where a filter is added to the request but not to the key.

    #[test]
    fn key_is_stable_for_identical_params() {
        let build = || {
            ListQueryParams::new()
                .sort(sort("level", true))
                .search(Some("foo".into()))
                .filter("min_level", Some("WARN".into()))
        };
        assert_eq!(build().key(), build().key());
    }

    #[test]
    fn sort_change_changes_the_key() {
        let a = ListQueryParams::new().sort(None);
        let b = ListQueryParams::new().sort(sort("level", true));
        let c = ListQueryParams::new().sort(sort("level", false));
        assert_ne!(a.key(), b.key());
        assert_ne!(b.key(), c.key(), "direction must affect the key");
    }

    #[test]
    fn search_change_changes_the_key() {
        let a = ListQueryParams::new().search(None);
        let b = ListQueryParams::new().search(Some("foo".into()));
        let c = ListQueryParams::new().search(Some("bar".into()));
        assert_ne!(a.key(), b.key());
        assert_ne!(b.key(), c.key());
    }

    #[test]
    fn filter_change_changes_the_key() {
        let a = ListQueryParams::new().filter("min_level", None);
        let b = ListQueryParams::new().filter("min_level", Some("WARN".into()));
        let c = ListQueryParams::new().filter("min_level", Some("ERROR".into()));
        assert_ne!(a.key(), b.key());
        assert_ne!(b.key(), c.key());
    }

    #[test]
    fn filter_none_differs_from_explicit_empty_in_the_key() {
        let unset = ListQueryParams::new().filter("types", None);
        let cleared = ListQueryParams::new().filter("types", Some(String::new()));
        assert_ne!(unset.key(), cleared.key());
    }

    #[test]
    fn key_is_order_independent() {
        // The same parameters registered in a different order must produce the
        // same key (the filter list is sorted internally).
        let a = ListQueryParams::new()
            .filter("min_level", Some("WARN".into()))
            .filter("types", Some("download".into()));
        let b = ListQueryParams::new()
            .filter("types", Some("download".into()))
            .filter("min_level", Some("WARN".into()));
        assert_eq!(a.key(), b.key());
    }

    #[test]
    fn filters_do_not_collide_across_names() {
        // "a=b" as a value must not be confused with a different filter name.
        let a = ListQueryParams::new().filter("min_level", Some("b=c".into()));
        let b = ListQueryParams::new().filter("types", Some("b=c".into()));
        assert_ne!(a.key(), b.key());
    }

    #[test]
    fn re_adding_a_filter_replaces_it() {
        let p = ListQueryParams::new()
            .filter("types", Some("download".into()))
            .filter("types", Some("import".into()));
        assert_eq!(p.filter_value("types").as_deref(), Some("import"));
        // ...and the replaced value is not left dangling in the key.
        let single = ListQueryParams::new().filter("types", Some("import".into()));
        assert_eq!(p.key(), single.key());
    }

    // ListQueryParams: typed accessors used by the request builder

    #[test]
    fn accessors_return_what_was_set() {
        let p = ListQueryParams::new()
            .sort(sort("level", true))
            .search(Some("foo".into()))
            .filter("min_level", Some("WARN".into()));
        assert_eq!(p.sort_state(), sort("level", true));
        assert_eq!(p.search_value().as_deref(), Some("foo"));
        assert_eq!(p.filter_value("min_level").as_deref(), Some("WARN"));
        // Absent names are `None` (→ the request omits the parameter).
        assert_eq!(p.filter_value("types"), None);
    }
}
