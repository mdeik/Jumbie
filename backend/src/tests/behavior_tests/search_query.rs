//! Search query construction with series aliases.
//!
//! Verifies the query format produced by the shared utilities
//! (`build_search_payload`, `build_search_queries`) rather than calling
//! `auto_search`, which would make real HTTP requests. The source plugins (Nyaa,
//! RSS) call these utilities internally, so this covers the same behaviour without
//! network dependencies.

use crate::search::{build_search_payload, resolve_season_search_meta};
use crate::tests::behavior_tests::helpers::{make_mapping, make_season_override};
use jumbie_shared::formatting::{SearchParams, build_search_queries, extract_search_params};
use std::collections::HashMap;

const NORMAL_FMT: &str = "S${season:02}E${episode:02}";
const ABS_FMT: &str = "E${episode:02}";

/// Regression test for the production season auto-search query build, end to end:
/// DB season "23" with a season-number alias (1) and an episode offset (1155), so
/// local episodes 14/15 are released as S01E1169/E1170.
///
/// The backend renders one key per episode from the search-format template; the
/// plugin then issues one query per alias × key, using the aliased season number
/// and the offset source episodes.
#[test]
fn test_season_autosearch_uses_offset_source_episodes_and_aliased_season() {
    let mut overrides: HashMap<String, jumbie_shared::mapping::SeasonOverride> = HashMap::new();
    overrides.insert(
        "23".to_string(),
        make_season_override("23", Some(1), Some(1155), vec![]),
    );
    let mapping = make_mapping("Mock Show", overrides);

    let meta = resolve_season_search_meta(&mapping, "23", 23, false, NORMAL_FMT, ABS_FMT);
    assert_eq!(meta.search_season_num, 1);
    assert_eq!(meta.episode_offset, 1155);

    let aliases = vec!["Mock Show".to_string(), "Mock Show (1998)".to_string()];
    let payload = build_search_payload(
        "Mock Show",
        meta.search_season_num,
        &[14, 15],
        aliases,
        &meta.search_format,
        meta.episode_offset,
    );

    let params = extract_search_params(&payload);
    let queries = build_search_queries(&params);

    // One query per episode (aliases OR-chained, episodes not).
    assert_eq!(queries.len(), 2, "got: {queries:?}");
    // Season marker uses the aliased season (1 -> S01); episodes are offset sources.
    assert!(
        queries.contains(&"(\"Mock Show\"|\"Mock Show (1998)\") S01E1169".to_string()),
        "got: {queries:?}"
    );
    assert!(
        queries.contains(&"(\"Mock Show\"|\"Mock Show (1998)\") S01E1170".to_string()),
        "got: {queries:?}"
    );
    // The real season (23) must never appear.
    assert!(
        !queries.iter().any(|q| q.contains("S23")),
        "aliased season must replace the real season, got: {queries:?}"
    );
}

#[test]
fn test_search_query_uses_alias_instead_of_series_title() {
    let params = SearchParams {
        series_title: "My Original Series Title".to_string(),
        season: 1,
        episodes: vec![1],
        aliases: vec!["My Alias Name".to_string(), "Another Alias".to_string()],
        keys: vec!["S01E01".to_string()],
    };

    let queries = build_search_queries(&params);
    assert!(
        queries.iter().any(|q| q.contains("My Alias Name"))
            && queries.iter().any(|q| q.contains("Another Alias")),
        "every alias should appear in the query set, got: {queries:?}"
    );
    assert!(
        !queries
            .iter()
            .any(|q| q.contains("My Original Series Title")),
        "series title must not appear when aliases are present, got: {queries:?}"
    );
}

#[test]
fn test_search_query_falls_back_to_series_title_when_no_alias() {
    let params = SearchParams {
        series_title: "Fallback Title".to_string(),
        season: 1,
        episodes: vec![1],
        aliases: vec![],
        keys: vec!["S01E01".to_string()],
    };

    let queries = build_search_queries(&params);
    assert_eq!(queries, vec!["\"Fallback Title\" S01E01"]);
}

#[test]
fn test_blank_template_searches_by_title_alone() {
    let params = SearchParams {
        series_title: "Original Series".to_string(),
        season: 2,
        episodes: vec![1],
        aliases: vec!["AliasName".to_string()],
        keys: vec![String::new()],
    };

    let queries = build_search_queries(&params);
    assert_eq!(queries, vec!["\"AliasName\""]);
}

#[test]
fn test_episode_only_template_has_no_season_marker() {
    let payload = build_search_payload("Show", 5, &[3], vec![], ABS_FMT, 0);
    let params = extract_search_params(&payload);
    let queries = build_search_queries(&params);
    assert_eq!(queries, vec!["\"Show\" E03"]);
    assert!(!queries[0].contains("S05"));
}

#[test]
fn test_alias_and_title_produce_identical_query_for_nyaa() {
    let with_alias = SearchParams {
        series_title: "Some Show".to_string(),
        season: 1,
        episodes: vec![1],
        aliases: vec!["Actual Search Term".to_string()],
        keys: vec!["S01E01".to_string()],
    };
    let title_fallback = SearchParams {
        series_title: "Actual Search Term".to_string(),
        season: 1,
        episodes: vec![1],
        aliases: vec![],
        keys: vec!["S01E01".to_string()],
    };

    assert_eq!(
        build_search_queries(&with_alias),
        build_search_queries(&title_fallback),
        "alias and title fallback MUST produce identical queries"
    );
}

#[test]
fn test_payload_roundtrips_keys_through_extract() {
    let payload = serde_json::json!({
        "series_title": "Some Show",
        "season": 1,
        "episodes": [1, 2],
        "aliases": ["Actual Search Term"],
        "keys": ["S01E01", "S01E02"],
    });

    let params = extract_search_params(&payload);
    let queries = build_search_queries(&params);
    assert_eq!(
        queries,
        vec![
            "\"Actual Search Term\" S01E01",
            "\"Actual Search Term\" S01E02"
        ]
    );
}
