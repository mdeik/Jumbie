//! Source search-query construction + the source response envelope.
//!
//! SSoT for how a source plugin turns an `auto_search` (or manual `search`)
//! request into the query string(s) it sends, and for the envelope every source
//! search response is returned in. The backend logs the reported queries.
//!
//! The backend renders each episode's search key from the series' search-format
//! template (`jumbie_shared::formatting::render_search_key`) and passes the
//! strings in `keys`; the plugin only concatenates a title/alias with each key.

use serde::{Deserialize, Serialize};

/// Structured search-query parameters received from the backend.
///
/// `episodes` are already in **source numbering** (any per-season `episode_offset`
/// has been applied by the backend) — use them verbatim.
/// `keys` holds one rendered search key per episode, aligned with `episodes`; a
/// blank key means "search by title alone".
#[derive(Debug, Clone, Default)]
pub struct SearchParams {
    pub series_title: String,
    pub season: i32,
    pub episodes: Vec<i32>,
    pub aliases: Vec<String>,
    pub keys: Vec<String>,
}

/// Extract [`SearchParams`] from a `serde_json::Value` map. All fields default
/// gracefully when missing or `null` — never panics.
pub fn extract_search_params(params: &serde_json::Value) -> SearchParams {
    SearchParams {
        series_title: params
            .get("series_title")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        season: params.get("season").and_then(|v| v.as_i64()).unwrap_or(1) as i32,
        episodes: params
            .get("episodes")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_i64())
                    .map(|n| n as i32)
                    .collect()
            })
            .unwrap_or_default(),
        aliases: params
            .get("aliases")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str())
                    .map(|s| s.to_string())
                    .collect()
            })
            .unwrap_or_default(),
        keys: params
            .get("keys")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str())
                    .map(|s| s.to_string())
                    .collect()
            })
            .unwrap_or_default(),
    }
}

/// Build the search queries for a request: one query per episode key, with all
/// aliases OR-chained into a single quoted group.
///
/// Titles are quoted phrases so multi-word aliases (and punctuation like `:` or
/// parentheses) survive on indexers that parse operators; multiple aliases are
/// joined with `|` inside parentheses. Episodes are NOT OR-chained — each key
/// gets its own query (e.g. `("A"|"B") S01E01`, `("A"|"B") S01E02`). A blank
/// key yields a title-only query.
pub fn build_search_queries(params: &SearchParams) -> Vec<String> {
    let titles: Vec<&str> = if params.aliases.is_empty() {
        vec![params.series_title.as_str()]
    } else {
        params.aliases.iter().map(|s| s.as_str()).collect()
    };

    let alias_part = if titles.len() == 1 {
        format!("\"{}\"", titles[0])
    } else {
        let quoted: Vec<String> = titles.iter().map(|t| format!("\"{}\"", t)).collect();
        format!("({})", quoted.join("|"))
    };

    let keys: Vec<&str> = if params.keys.is_empty() {
        vec![""]
    } else {
        params.keys.iter().map(|s| s.as_str()).collect()
    };

    keys.iter()
        .map(|key| {
            if key.is_empty() {
                alias_part.clone()
            } else {
                format!("{} {}", alias_part, key)
            }
        })
        .collect()
}

/// The response shape every source search method (`search`, `auto_search`) returns:
/// the entries plus the exact query string(s) the plugin sent. Both fields are
/// required — a bare `Vec<MediaEntry>` no longer deserializes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResponse<T> {
    pub entries: T,
    pub queries: Vec<String>,
}

impl<T> SearchResponse<T> {
    pub fn new(entries: T, queries: Vec<String>) -> Self {
        Self { entries, queries }
    }

    /// SDK default for `auto_search`: build the per-episode queries from `params`.
    pub fn from_auto_search(entries: T, params: &SearchParams) -> Self {
        Self {
            entries,
            queries: build_search_queries(params),
        }
    }

    /// SDK default for a manual `search`: the single literal query the caller asked for.
    pub fn from_manual(entries: T, query: impl Into<String>) -> Self {
        Self {
            entries,
            queries: vec![query.into()],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(aliases: Vec<&str>, keys: Vec<&str>) -> SearchParams {
        SearchParams {
            series_title: "Mock Show".to_string(),
            season: 1,
            episodes: vec![1, 2],
            aliases: aliases.into_iter().map(|s| s.to_string()).collect(),
            keys: keys.into_iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn one_query_per_key_with_aliases_or_chained() {
        let queries = build_search_queries(&params(vec!["A", "B"], vec!["S01E01", "S01E02"]));
        assert_eq!(
            queries,
            vec!["(\"A\"|\"B\") S01E01", "(\"A\"|\"B\") S01E02"]
        );
    }

    #[test]
    fn single_title_is_quoted_without_a_group() {
        let queries = build_search_queries(&params(vec![], vec!["S01E01"]));
        assert_eq!(queries, vec!["\"Mock Show\" S01E01"]);
    }

    #[test]
    fn single_alias_is_quoted_without_a_group() {
        let queries = build_search_queries(&params(vec!["OnlyOne"], vec!["S01E01"]));
        assert_eq!(queries, vec!["\"OnlyOne\" S01E01"]);
    }

    #[test]
    fn blank_key_searches_by_title_alone() {
        let queries = build_search_queries(&params(vec!["A", "B"], vec![""]));
        assert_eq!(queries, vec!["(\"A\"|\"B\")"]);
    }

    #[test]
    fn empty_keys_fall_back_to_a_title_query() {
        let queries = build_search_queries(&params(vec!["A"], vec![]));
        assert_eq!(queries, vec!["\"A\""]);
    }

    #[test]
    fn extract_defaults_gracefully_on_garbage() {
        let extracted = extract_search_params(&serde_json::json!({
            "series_title": 42,
            "episodes": ["1", null, 3],
            "aliases": [1, "ok"],
            "keys": [null, "S01E01"],
        }));
        assert_eq!(extracted.series_title, "");
        assert_eq!(extracted.season, 1);
        assert_eq!(extracted.episodes, vec![3]);
        assert_eq!(extracted.aliases, vec!["ok".to_string()]);
        assert_eq!(extracted.keys, vec!["S01E01".to_string()]);
    }

    #[test]
    fn extract_reads_a_well_formed_payload() {
        let extracted = extract_search_params(&serde_json::json!({
            "series_title": "Show",
            "season": 3,
            "episodes": [1, 2],
            "aliases": ["A"],
            "keys": ["S03E01", "S03E02"],
        }));
        assert_eq!(extracted.series_title, "Show");
        assert_eq!(extracted.season, 3);
        assert_eq!(extracted.episodes, vec![1, 2]);
        assert_eq!(extracted.aliases, vec!["A".to_string()]);
        assert_eq!(
            extracted.keys,
            vec!["S03E01".to_string(), "S03E02".to_string()]
        );
    }

    #[test]
    fn manual_response_reports_the_literal_query() {
        let resp = SearchResponse::from_manual(vec!["entry"], "Mock Show S01E1169");
        assert_eq!(resp.queries, vec!["Mock Show S01E1169"]);
    }

    #[test]
    fn auto_response_reports_every_query() {
        let resp = SearchResponse::from_auto_search(vec!["entry"], &params(vec![], vec!["E01"]));
        assert_eq!(resp.queries, vec!["\"Mock Show\" E01"]);
    }
}
