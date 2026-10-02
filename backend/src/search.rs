//! Shared search helpers for auto-search flows.
//!
//! All auto-search paths (`auto_search_season`, `auto_search_missing`,
//! `search_media`) share one alias-resolution pipeline — call
//! [`resolve_search_aliases`] rather than the individual steps, then build
//! payloads with [`build_search_payload`].

use std::collections::HashMap;

use jumbie_shared::mapping::parse_alias;
use jumbie_shared::types::{EpisodeInfo, MappingRule};

use crate::utils::{ParseContext, parse_filename, parse_filename_for_season};

/// Stable tracing target for the canonical search-query log line.
pub const SEARCH_QUERY_LOG_TARGET: &str = "search.query";

/// Which search flow emitted a query. Set by the backend — the plugin only
/// reports the query string(s) it actually sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchKind {
    Manual,
    AutoEpisode,
    AutoSeason,
    AutoMissing,
}

impl SearchKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SearchKind::Manual => "manual",
            SearchKind::AutoEpisode => "auto_episode",
            SearchKind::AutoSeason => "auto_season",
            SearchKind::AutoMissing => "auto_missing",
        }
    }
}

/// Context for the canonical search-query log line. The backend sets `kind` and
/// the request context; the plugin reports the `queries` it sent. Emitted once per
/// source by the bridge ([`SearchLog::emit`]) for every search path.
#[derive(Debug, Clone)]
pub struct SearchLog {
    pub kind: SearchKind,
    pub source: String,
    pub series_id: Option<String>,
    pub series_title: Option<String>,
    pub season: Option<i32>,
    /// Episodes in source numbering (offset applied) — what the source searched.
    pub source_episodes: Vec<i32>,
    pub aliases: Vec<String>,
}

impl SearchLog {
    /// Emit the canonical INFO line with the plugin-reported queries.
    pub fn emit(&self, queries: &[String]) {
        let season = self.season.map(|s| s.to_string()).unwrap_or_default();
        tracing::info!(
            target: SEARCH_QUERY_LOG_TARGET,
            "kind={} source='{}' series_id='{}' series='{}' season={} source_episodes={:?} aliases={:?} queries={:?}",
            self.kind.as_str(),
            self.source,
            self.series_id.as_deref().unwrap_or(""),
            self.series_title.as_deref().unwrap_or(""),
            season,
            self.source_episodes,
            self.aliases,
            queries,
        );
    }
}

/// Split aliases into generic (no prefix) and per-slug groups, returning
/// `(generic_aliases, source_slug → aliases)`.
///
/// Priority (SSoT): season aliases > series aliases > `mapping.target_title`.
pub fn split_aliases_by_source(
    mapping: &MappingRule,
    season: &str,
    global_absolute: bool,
) -> (Vec<String>, HashMap<String, Vec<String>>) {
    let season_override = mapping
        .settings
        .find_season_override(season, global_absolute);
    let season_has_aliases = season_override
        .map(|s| s.has_search_aliases())
        .unwrap_or(false);
    let series_has_aliases = mapping.settings.has_search_aliases();

    // Priority: season aliases > series aliases > target title fallback
    // Priority: season aliases > series aliases > target title fallback
    if season_has_aliases {
        split_raw_aliases(season_override.unwrap().search_aliases())
    } else if series_has_aliases {
        split_raw_aliases(mapping.settings.search_aliases())
    } else {
        (vec![mapping.target_title.clone()], HashMap::new())
    }
}

/// Parse raw alias strings into (generic, source-scoped) groups.
fn split_raw_aliases<'a>(
    aliases: impl Iterator<Item = &'a str>,
) -> (Vec<String>, HashMap<String, Vec<String>>) {
    let mut generic_aliases: Vec<String> = vec![];
    let mut source_aliases: HashMap<String, Vec<String>> = HashMap::new();
    for alias in aliases {
        let parsed = parse_alias(alias);
        match parsed.source_id {
            Some(slug) => {
                source_aliases.entry(slug).or_default().push(parsed.alias);
            }
            None => {
                generic_aliases.push(parsed.alias);
            }
        }
    }
    (generic_aliases, source_aliases)
}

/// Resolve effective patterns for a season: season patterns > series patterns >
/// empty slice (caller uses default parsing).
pub fn resolve_effective_patterns<'a>(
    season_override: Option<&'a jumbie_shared::mapping::SeasonOverride>,
    series_patterns: &'a [String],
) -> &'a [String] {
    season_override
        .and_then(|so| {
            if so.has_search_patterns() {
                Some(so.reg_patterns.as_slice())
            } else {
                None
            }
        })
        .unwrap_or(series_patterns)
}

/// Season-level search metadata: the resolved search-format template, the season
/// to search, and the local→source episode offset.
///
/// Every auto-search path resolves season search config through this struct so
/// they all stay on the same numbering-space pipeline.
pub struct SeasonSearchMeta {
    /// The effective `${season}`/`${episode}` template (season → series → global),
    /// resolved for the series' active numbering mode.
    pub search_format: String,
    /// The season number the source should be searched with (alias applied).
    pub search_season_num: i32,
    /// Local→source episode offset for this season (0 when unset).
    pub episode_offset: i32,
    /// Whether `search_format` references `${season}` (precomputed once at resolve
    /// time). A season-scoped template means releases must declare the season;
    /// otherwise season-less releases are allowed.
    pub requires_season: bool,
}

/// Resolve the [`SeasonSearchMeta`] for a season (search format + alias + offset).
///
/// `global_format`/`global_format_absolute` are the organization config's search
/// templates; the series' active numbering mode selects between them before the
/// season → series → global layering runs.
pub fn resolve_season_search_meta(
    mapping: &MappingRule,
    season: &str,
    season_num: i32,
    global_absolute: bool,
    global_format: &str,
    global_format_absolute: &str,
) -> SeasonSearchMeta {
    let absolute = mapping.settings.active_mode(global_absolute).is_absolute();
    let global = if absolute {
        global_format_absolute
    } else {
        global_format
    };
    let season_override = mapping
        .settings
        .find_season_override(season, global_absolute);
    let search_format = jumbie_shared::mapping::resolve_search_format(
        &mapping.settings,
        season_override,
        global,
        absolute,
    )
    .to_string();
    let requires_season =
        jumbie_shared::validation::template_uses_variable(&search_format, "season");
    SeasonSearchMeta {
        search_format,
        search_season_num: season_override
            .map(|o| o.effective_search_season(season_num))
            .unwrap_or(season_num),
        episode_offset: season_override.map(|o| o.offset()).unwrap_or(0),
        requires_season,
    }
}

impl SeasonSearchMeta {
    /// The season the parser must require, or `None` to allow season-less titles:
    /// the searched season when the template is season-scoped, else `None`
    /// (absolute / episode-only templates).
    pub fn required_season(&self) -> Option<i32> {
        self.requires_season.then_some(self.search_season_num)
    }

    /// Parse a release title once under this search's season scope.
    ///
    /// Reuse the returned [`ReleaseMatch`] to check several episodes without
    /// re-parsing; [`matches_result`](Self::matches_result) is the single-episode
    /// convenience wrapper. SSoT for the "is a result acceptable" rule:
    ///
    /// * With `${season}` in the template the query is season-scoped, so the title
    ///   must *declare* the searched season — the lenient `S01` fallback is not
    ///   enough (season-less releases are blocked). Without `${season}` (e.g.
    ///   absolute `E${episode}`) season-less releases are allowed.
    /// * If the structured parser cannot place the release, the
    ///   [`contains_ordered_season_episode`](jumbie_shared::formatting::contains_ordered_season_episode)
    ///   hail mary checks for both numbers as ordered, standalone tokens
    ///   (`Show 2 - 45` for S02E45).
    /// * Season packs / complete packs are handled by dedicated paths and never
    ///   match here.
    pub fn match_release<'a>(&'a self, title: &'a str) -> ReleaseMatch<'a> {
        ReleaseMatch {
            meta: self,
            title,
            info: parse_search_result_title(title, self.required_season()),
        }
    }

    /// Whether a release title satisfies this search for one source episode.
    ///
    /// Convenience for one-off checks; prefer [`match_release`](Self::match_release)
    /// when testing several episodes of the same title.
    pub fn matches_result(&self, title: &str, source_episode: i32) -> bool {
        self.match_release(title).accepts(source_episode)
    }
}

/// A release title parsed once for a [`SeasonSearchMeta`].
///
/// Produced by [`SeasonSearchMeta::match_release`] so a title is parsed a single
/// time and then checked for any number of episodes without re-parsing.
pub struct ReleaseMatch<'a> {
    meta: &'a SeasonSearchMeta,
    title: &'a str,
    info: Option<EpisodeInfo>,
}

impl ReleaseMatch<'_> {
    /// Whether this release is accepted for `source_episode`.
    pub fn accepts(&self, source_episode: i32) -> bool {
        if let Some(info) = &self.info
            && !info.is_season_pack
            && !info.is_complete_pack
            && info.episodes.contains(&source_episode)
        {
            return true;
        }
        // Hail mary: the structured parser could not place the release, so fall
        // back to bare ordered numbers (`Show 2 - 45` for S02E45).
        jumbie_shared::formatting::contains_ordered_season_episode(
            self.title,
            self.meta.search_season_num,
            source_episode,
        )
    }
}

/// Parse a search-result title under the season rule from [`SeasonSearchMeta`].
///
/// `required_season` comes from [`SeasonSearchMeta::required_season`]: when set,
/// the title must declare that season (see [`parse_filename_for_season`]) instead
/// of falling back to `S01`; when `None`, a missing season is allowed. RSS-feed
/// processing passes `None`, since a series-scoped feed may omit the season.
pub fn parse_search_result_title(title: &str, required_season: Option<i32>) -> Option<EpisodeInfo> {
    match required_season {
        Some(season) => parse_filename_for_season(title, ParseContext::Search, season),
        None => parse_filename(title, ParseContext::Search),
    }
}

/// Validate `@uuid:`-scoped alias keys against the plugin manager and drop
/// unresolvable ones (skipped entirely, not folded into generic). Returns
/// `(plugin_id → aliases, generic_aliases)`.
///
/// The first element has unresolvable entries already removed, so callers can
/// iterate it directly (the UUID IS the plugin_id).
pub async fn resolve_source_aliases(
    pm: &tokio::sync::RwLockReadGuard<'_, crate::plugins::PluginManager>,
    generic_aliases: Vec<String>,
    mut source_aliases: HashMap<String, Vec<String>>,
) -> (HashMap<String, Vec<String>>, Vec<String>) {
    let all_slugs: Vec<String> = source_aliases.keys().cloned().collect();

    for id in &all_slugs {
        if pm.get_source_by_id(id).await.is_none() {
            tracing::debug!(
                "Source prefix '{}' does not match any source plugin UUID — skipping {} alias(es)",
                id,
                source_aliases.get(id).map_or(0, Vec::len),
            );
            source_aliases.remove(id);
        }
    }

    (source_aliases, generic_aliases)
}

/// Single entry point: split aliases by UUID prefix, resolve valid plugin
/// instances, and build the `plugin_id → aliases` map.
pub async fn resolve_search_aliases(
    pm: &tokio::sync::RwLockReadGuard<'_, crate::plugins::PluginManager>,
    mapping: &MappingRule,
    season: &str,
    global_absolute: bool,
) -> (HashMap<String, Vec<String>>, Vec<String>) {
    let (generic_aliases, source_aliases) =
        split_aliases_by_source(mapping, season, global_absolute);
    let (resolved, generic) = resolve_source_aliases(pm, generic_aliases, source_aliases).await;
    let plugin_aliases = build_plugin_aliases(&resolved, &generic);
    (plugin_aliases, generic)
}

/// Map local (DB) episode numbers to the source numbers releases use under a
/// season's `episode_offset`. The single local→source conversion for search:
/// `build_search_payload` uses it for the query, callers use it for the log line.
pub fn source_episode_numbers(local_episodes: &[i32], episode_offset: i32) -> Vec<i32> {
    local_episodes
        .iter()
        .map(|&e| jumbie_shared::mapping::local_to_source_episode(e, episode_offset))
        .collect()
}

/// Build the per-source JSON payload for an auto-search call
/// (`{series_title, season, episodes, aliases, keys}`), with aliases scoped to the
/// specific plugin and episodes/keys in SOURCE numbering.
pub fn build_search_payload(
    target_title: &str,
    season_num: i32,
    local_episodes: &[i32],
    aliases: Vec<String>,
    search_format: &str,
    episode_offset: i32,
) -> serde_json::Value {
    let episodes = source_episode_numbers(local_episodes, episode_offset);
    let keys: Vec<String> = episodes
        .iter()
        .map(|&e| jumbie_shared::formatting::render_search_key(search_format, season_num, e))
        .collect();
    serde_json::json!({
        "series_title": target_title,
        "season": season_num,
        "episodes": episodes,
        "aliases": aliases,
        "keys": keys,
    })
}

/// Build the `plugin_id → aliases` map, combining generic aliases with each
/// source's UUID-scoped aliases. `resolved_source_aliases` must already be
/// filtered (see [`resolve_source_aliases`]).
pub fn build_plugin_aliases(
    resolved_source_aliases: &HashMap<String, Vec<String>>,
    generic_aliases: &[String],
) -> HashMap<String, Vec<String>> {
    let mut plugin_aliases: HashMap<String, Vec<String>> = HashMap::new();
    for (plugin_id, specific) in resolved_source_aliases {
        let entry = plugin_aliases.entry(plugin_id.clone()).or_default();
        entry.extend(generic_aliases.iter().cloned());
        entry.extend(specific.iter().cloned());
    }
    plugin_aliases
}

/// Build final per-source alias list for a given plugin.
///
/// If no aliases are available for this plugin, falls back to the target title.
pub fn resolve_final_aliases(
    plugin_id: &str,
    plugin_aliases: &HashMap<String, Vec<String>>,
    generic_aliases: &[String],
    target_title: &str,
) -> Vec<String> {
    let source_specific = plugin_aliases
        .get(plugin_id)
        .cloned()
        .unwrap_or_else(|| generic_aliases.to_vec());

    if source_specific.is_empty() {
        vec![target_title.to_string()]
    } else {
        source_specific
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NORMAL_FMT: &str = "S${season:02}E${episode:02}";
    const ABS_FMT: &str = "E${episode:02}";

    // Canonical search-query log

    /// Proves the canonical line carries the backend-set `kind` and the exact
    /// queries the plugin reported.
    #[test]
    fn test_search_log_emits_kind_and_reported_queries() {
        use std::io;
        use std::sync::{Arc, Mutex};

        #[derive(Clone)]
        struct BufWriter(Arc<Mutex<Vec<u8>>>);
        impl io::Write for BufWriter {
            fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for BufWriter {
            type Writer = BufWriter;
            fn make_writer(&'a self) -> Self::Writer {
                self.clone()
            }
        }

        let buffer = Arc::new(Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::fmt()
            .with_writer(BufWriter(buffer.clone()))
            .with_max_level(tracing::Level::INFO)
            .finish();

        let log = SearchLog {
            kind: SearchKind::AutoSeason,
            source: "Nyaa".to_string(),
            series_id: Some("sid".to_string()),
            series_title: Some("Mock Show".to_string()),
            season: Some(1),
            source_episodes: vec![1169],
            aliases: vec!["Mock Show".to_string()],
        };
        let reported = vec!["(\"Mock Show\") S01(\"E1169\")".to_string()];
        tracing::subscriber::with_default(subscriber, || log.emit(&reported));

        let out = String::from_utf8(buffer.lock().unwrap().clone()).unwrap();
        assert!(out.contains("kind=auto_season"), "missing kind: {out}");
        assert!(out.contains("source='Nyaa'"), "missing source: {out}");
        assert!(
            out.contains("source_episodes=[1169]"),
            "missing source episodes: {out}"
        );
        assert!(
            out.contains("queries=[\"(\\\"Mock Show\\\") S01(\\\"E1169\\\")\"]"),
            "missing reported query: {out}"
        );
    }

    // build_search_payload tests

    #[test]
    fn test_build_search_payload_structure() {
        let payload = build_search_payload(
            "My Show",
            2,
            &[1, 2, 3],
            vec!["Show".to_string()],
            "S${season:02}E${episode:02}",
            0,
        );

        let obj = payload.as_object().unwrap();
        assert_eq!(obj["series_title"], "My Show");
        assert_eq!(obj["season"], 2);
        assert_eq!(obj["aliases"].as_array().unwrap().len(), 1);
        assert_eq!(obj["episodes"].as_array().unwrap().len(), 3);
        assert_eq!(
            obj["keys"],
            serde_json::json!(["S02E01", "S02E02", "S02E03"])
        );
        assert!(
            obj.get("episode_abs").is_none(),
            "episode_abs was removed from the payload"
        );
    }

    #[test]
    fn test_build_search_payload_contains_both_title_and_aliases() {
        // Verify the payload always contains both `series_title` and `aliases`
        // so plugins can decide which to use for their query.
        let payload = build_search_payload(
            "Target Title",
            1,
            &[1, 2],
            vec!["Alias One".to_string(), "Alias Two".to_string()],
            "S${season:02}E${episode:02}",
            0,
        );
        let obj = payload.as_object().unwrap();
        assert_eq!(
            obj["series_title"], "Target Title",
            "series_title must always be the canonical target title"
        );
        let aliases = obj["aliases"].as_array().unwrap();
        assert_eq!(
            aliases.len(),
            2,
            "aliases field should contain the provided alias list"
        );
        assert_eq!(aliases[0], "Alias One");
        assert_eq!(aliases[1], "Alias Two");
    }

    #[test]
    fn test_build_search_payload_renders_one_key_per_episode() {
        let aliases = vec![
            "Anime Title".to_string(),
            "Alt Name".to_string(),
            "Third Alias".to_string(),
        ];
        let payload =
            build_search_payload("Official Title", 3, &[4, 5], aliases, "E${episode:03}", 0);
        let obj = payload.as_object().unwrap();
        assert_eq!(obj["series_title"], "Official Title");
        assert_eq!(obj["season"], 3);
        assert_eq!(obj["aliases"].as_array().unwrap().len(), 3);
        assert_eq!(obj["episodes"], serde_json::json!([4, 5]));
        assert_eq!(obj["keys"], serde_json::json!(["E004", "E005"]));
    }

    #[test]
    fn test_build_search_payload_blank_template_yields_empty_keys() {
        // An empty template is legal: the query carries no key and the release is
        // matched on title + a parsed episode number alone.
        let payload = build_search_payload("Show", 2, &[1], vec![], "", 0);
        let obj = payload.as_object().unwrap();
        assert_eq!(obj["keys"], serde_json::json!([""]));
    }

    #[test]
    fn test_build_search_payload_applies_episode_offset() {
        // SSoT: callers pass LOCAL numbers; the builder emits SOURCE numbers and
        // renders the key from those source numbers.
        let payload =
            build_search_payload("Mock Series", 1, &[1, 2], vec![], "E${episode:02}", 1169);
        let obj = payload.as_object().unwrap();
        let eps: Vec<i64> = obj["episodes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_i64().unwrap())
            .collect();
        assert_eq!(eps, vec![1170, 1171], "episodes must be local + offset");
        assert_eq!(obj["keys"], serde_json::json!(["E1170", "E1171"]));
    }

    #[test]
    fn test_build_search_payload_zero_offset_is_passthrough() {
        let payload =
            build_search_payload("Show", 1, &[5], vec![], "S${season:02}E${episode:02}", 0);
        let obj = payload.as_object().unwrap();
        assert_eq!(obj["episodes"].as_array().unwrap()[0].as_i64(), Some(5));
        assert_eq!(obj["keys"], serde_json::json!(["S01E05"]));
    }

    #[test]
    fn test_resolve_then_payload_applies_offset_end_to_end() {
        // Mirrors the real auto-search path: resolve the season's offset, then
        // build the payload from LOCAL episodes. The payload must come out in
        // SOURCE numbering so the query matches real release titles.
        let mapping: MappingRule = serde_json::from_value(serde_json::json!({
            "target_title": "Mock Series",
            "season": { "1": { "season": "1", "episode_offset": 1169 } }
        }))
        .expect("Failed to create mapping");

        let meta = resolve_season_search_meta(&mapping, "1", 1, false, NORMAL_FMT, ABS_FMT);
        assert_eq!(meta.episode_offset, 1169);

        let payload = build_search_payload(
            &mapping.target_title,
            meta.search_season_num,
            &[1],
            vec![],
            &meta.search_format,
            meta.episode_offset,
        );
        let obj = payload.as_object().unwrap();
        assert_eq!(
            obj["episodes"].as_array().unwrap()[0].as_i64(),
            Some(1170),
            "local episode 1 with offset 1169 must be queried as source 1170"
        );
        assert_eq!(obj["keys"], serde_json::json!(["S01E1170"]));
    }

    #[test]
    fn test_alias_offset_and_template_drive_query_and_matching() {
        // A season override combining an alias season, an episode offset, and its
        // own template must drive BOTH the emitted query key and result matching.
        let mapping: MappingRule = serde_json::from_value(serde_json::json!({
            "target_title": "Mock Series",
            "season": {
                "1": {
                    "season": "1",
                    "alias_season_number": 4,
                    "episode_offset": 1169,
                    "search_format": "S${season:02}E${episode:02}"
                }
            }
        }))
        .expect("mapping");

        let meta = resolve_season_search_meta(&mapping, "1", 1, false, NORMAL_FMT, ABS_FMT);
        assert_eq!(
            meta.search_season_num, 4,
            "alias overrides the searched season"
        );
        assert_eq!(meta.episode_offset, 1169);
        assert_eq!(meta.search_format, "S${season:02}E${episode:02}");
        assert_eq!(meta.required_season(), Some(4));

        // Query: local episode 1 + offset 1169, rendered with the aliased season.
        let payload = build_search_payload(
            &mapping.target_title,
            meta.search_season_num,
            &[1],
            vec![],
            &meta.search_format,
            meta.episode_offset,
        );
        assert_eq!(payload["episodes"], serde_json::json!([1170]));
        assert_eq!(payload["keys"], serde_json::json!(["S04E1170"]));

        // Matching uses the same aliased season + source episode.
        assert!(meta.matches_result("Mock Series S04E1170 1080p", 1170));
        // A different source episode, and the wrong season, are rejected.
        assert!(!meta.matches_result("Mock Series S04E1169 1080p", 1170));
        assert!(!meta.matches_result("Mock Series S01E1170 1080p", 1170));
        // Hail mary too: aliased season 4, source episode 1170.
        assert!(meta.matches_result("Mock Series 4 - 1170 1080p", 1170));
    }

    // split_aliases_by_source tests
    // These tests verify that search aliases from the mapping are correctly
    // split into generic (unprefixed) and per-slug groups, and that the
    // target_title fallback is used when no aliases exist.

    fn make_mapping(target_title: &str, aliases: Vec<&str>) -> MappingRule {
        serde_json::from_value(serde_json::json!({
            "target_title": target_title,
            "aliases": aliases,
        }))
        .expect("Failed to create test MappingRule")
    }

    fn make_mapping_with_season_aliases(
        target_title: &str,
        aliases: Vec<&str>,
        season_key: &str,
        season_aliases: Vec<&str>,
    ) -> MappingRule {
        serde_json::from_value(serde_json::json!({
            "target_title": target_title,
            "aliases": aliases,
            "season": {
                season_key: {
                    "season": season_key,
                    "aliases": season_aliases,
                }
            }
        }))
        .expect("Failed to create test MappingRule with season aliases")
    }

    #[test]
    fn test_split_aliases_no_aliases_falls_back_to_title() {
        let mapping = make_mapping("My Show", vec![]);
        let (generic, source) = split_aliases_by_source(&mapping, "S01", false);
        assert_eq!(
            generic,
            vec!["My Show".to_string()],
            "no aliases => generic should be [target_title]"
        );
        assert!(
            source.is_empty(),
            "no aliases => source_aliases should be empty"
        );
    }

    #[test]
    fn test_split_aliases_generic_only() {
        let mapping = make_mapping("Official", vec!["Alias One", "Alias Two"]);
        let (generic, source) = split_aliases_by_source(&mapping, "S01", false);
        assert_eq!(generic.len(), 2);
        assert!(generic.contains(&"Alias One".to_string()));
        assert!(generic.contains(&"Alias Two".to_string()));
        assert!(source.is_empty());
    }

    #[test]
    fn test_split_aliases_source_prefixed() {
        let mapping = make_mapping("Official", vec!["@nyaa:Anime Title", "Generic Alias"]);
        let (generic, source) = split_aliases_by_source(&mapping, "S01", false);
        assert_eq!(
            generic,
            vec!["Generic Alias".to_string()],
            "unprefixed aliases go to generic"
        );
        let nyaa_aliases = source.get("nyaa").expect("nyaa slug should have aliases");
        assert_eq!(nyaa_aliases, &vec!["Anime Title".to_string()]);
    }

    #[test]
    fn test_split_aliases_multiple_sources() {
        let mapping = make_mapping(
            "Official",
            vec!["@nyaa:Anime", "@rss:Generic RSS", "Common Alias"],
        );
        let (generic, source) = split_aliases_by_source(&mapping, "S01", false);
        assert_eq!(generic, vec!["Common Alias".to_string()]);
        assert_eq!(source.get("nyaa").unwrap(), &vec!["Anime".to_string()]);
        assert_eq!(source.get("rss").unwrap(), &vec!["Generic RSS".to_string()]);
    }

    #[test]
    fn test_split_aliases_season_override_takes_precedence() {
        // Season-level aliases REPLACE series-level aliases (priority: season > series)
        let mapping = make_mapping_with_season_aliases(
            "Official",
            vec!["@nyaa:Generic Anime"],
            "S02",
            vec!["@nyaa:Season Two Specific"],
        );
        let (generic, source) = split_aliases_by_source(&mapping, "S02", false);
        assert!(generic.is_empty());
        let nyaa_aliases = source.get("nyaa").unwrap();
        assert_eq!(
            nyaa_aliases.len(),
            1,
            "season override should ONLY have season-level aliases, not series-level"
        );
        assert!(
            nyaa_aliases.contains(&"Season Two Specific".to_string()),
            "season override should have season-level aliases"
        );
        assert!(
            !nyaa_aliases.contains(&"Generic Anime".to_string()),
            "season override should NOT include series-level aliases"
        );
    }

    #[test]
    fn test_split_aliases_season_override_with_generic() {
        // Season has its own aliases → series-level aliases are IGNORED
        let mapping = make_mapping_with_season_aliases(
            "Official",
            vec!["Series Alias"],
            "S03",
            vec!["Season Alias"],
        );
        let (generic, _source) = split_aliases_by_source(&mapping, "S03", false);
        assert_eq!(
            generic,
            vec!["Season Alias"],
            "Season override should ONLY contain its own aliases"
        );
    }

    #[test]
    fn test_split_aliases_season_override_empty_falls_to_series() {
        // Season exists but has NO aliases → falls back to series-level aliases
        let mapping = make_mapping_with_season_aliases(
            "Fallback",
            vec!["Series Alias"],
            "S04",
            vec![], // empty season aliases
        );
        let (generic, source) = split_aliases_by_source(&mapping, "S04", false);
        assert_eq!(
            generic,
            vec!["Series Alias"],
            "Empty season aliases should fall back to series aliases"
        );
        assert!(
            source.is_empty(),
            "No source-specific aliases in this scenario"
        );
    }

    #[test]
    fn test_split_aliases_season_empty_both_falls_to_title() {
        // Neither season nor series has aliases → falls back to target_title
        let mapping = make_mapping("My Show", vec![]);
        // Create a season override with no aliases
        let (generic, source) = split_aliases_by_source(&mapping, "S05", false);
        assert_eq!(generic, vec!["My Show".to_string()]);
        assert!(source.is_empty());
    }

    // resolve_effective_patterns tests

    #[test]
    fn test_patterns_uses_season_when_present() {
        let so = serde_json::from_value(serde_json::json!({
            "season": "S01",
            "reg_patterns": ["Season.*Pattern"],
        }))
        .unwrap();
        let series_patterns = vec!["SeriesPattern".to_string()];
        let result = resolve_effective_patterns(Some(&so), &series_patterns);
        assert_eq!(result, &["Season.*Pattern"]);
    }

    #[test]
    fn test_patterns_falls_back_to_series_when_season_empty() {
        let so = serde_json::from_value(serde_json::json!({
            "season": "S01",
            "reg_patterns": [],
        }))
        .unwrap();
        let series_patterns = vec!["SeriesOnly".to_string()];
        let result = resolve_effective_patterns(Some(&so), &series_patterns);
        assert_eq!(result, &["SeriesOnly"]);
    }

    #[test]
    fn test_patterns_no_season_override_uses_series() {
        let series_patterns = vec!["SeriesFallback".to_string()];
        let result = resolve_effective_patterns(None, &series_patterns);
        assert_eq!(result, &["SeriesFallback"]);
    }

    #[test]
    fn test_patterns_both_empty_returns_empty() {
        let so = serde_json::from_value(serde_json::json!({
            "season": "S01",
            "reg_patterns": [],
        }))
        .unwrap();
        let series_patterns: Vec<String> = vec![];
        let result = resolve_effective_patterns(Some(&so), &series_patterns);
        assert!(result.is_empty());
    }

    // resolve_season_search_meta tests

    #[test]
    fn test_resolve_season_search_meta_no_override() {
        let mapping = make_mapping("Test", vec![]);
        let meta = resolve_season_search_meta(&mapping, "S99", 5, false, NORMAL_FMT, ABS_FMT);
        assert_eq!(meta.search_format, NORMAL_FMT);
        assert_eq!(meta.search_season_num, 5);
        assert_eq!(meta.episode_offset, 0);
    }

    #[test]
    fn test_resolve_search_format_layers_season_over_series_over_global() {
        let mapping: MappingRule = serde_json::from_value(serde_json::json!({
            "target_title": "Test",
            "search_format": "SERIES ${episode}",
            "season": {
                "1": { "season": "1" },
                "2": { "season": "2", "search_format": "SEASON2 ${episode}" },
                "3": { "season": "3", "search_format": "" }
            }
        }))
        .expect("mapping");

        // Season without an override inherits the series template.
        assert_eq!(
            resolve_season_search_meta(&mapping, "1", 1, false, NORMAL_FMT, ABS_FMT).search_format,
            "SERIES ${episode}"
        );
        // Season override wins over the series template.
        assert_eq!(
            resolve_season_search_meta(&mapping, "2", 2, false, NORMAL_FMT, ABS_FMT).search_format,
            "SEASON2 ${episode}"
        );
        // A deliberately blank season override beats the series template.
        assert_eq!(
            resolve_season_search_meta(&mapping, "3", 3, false, NORMAL_FMT, ABS_FMT).search_format,
            ""
        );
        // No override and no series template → global.
        let bare = make_mapping("Test", vec![]);
        assert_eq!(
            resolve_season_search_meta(&bare, "9", 9, false, NORMAL_FMT, ABS_FMT).search_format,
            NORMAL_FMT
        );
    }

    #[test]
    fn test_resolve_search_format_is_mode_aware() {
        let mapping: MappingRule = serde_json::from_value(serde_json::json!({
            "target_title": "Test",
            "absolute_numbering": true,
            "search_format": "NORMAL ${episode}",
            "search_format_absolute": "ABS ${episode}",
        }))
        .expect("mapping");
        let meta = resolve_season_search_meta(&mapping, "1", 1, false, NORMAL_FMT, ABS_FMT);
        assert_eq!(meta.search_format, "ABS ${episode}");
    }

    #[test]
    fn test_resolve_season_search_meta_absolute_layers_and_mode_isolation() {
        // A series override for the normal mode must not leak into absolute mode:
        // absolute falls through to the global absolute template instead.
        let mapping: MappingRule = serde_json::from_value(serde_json::json!({
            "target_title": "Test",
            "search_format": "NORMAL ${episode}",
        }))
        .expect("mapping");
        assert_eq!(
            resolve_season_search_meta(&mapping, "1", 1, false, NORMAL_FMT, ABS_FMT).search_format,
            "NORMAL ${episode}"
        );
        assert_eq!(
            resolve_season_search_meta(&mapping, "1", 1, true, NORMAL_FMT, ABS_FMT).search_format,
            ABS_FMT
        );

        // Season overrides live in the per-mode map and win in that mode.
        let mapping: MappingRule = serde_json::from_value(serde_json::json!({
            "target_title": "Test",
            "search_format": "NORMAL ${episode}",
            "search_format_absolute": "ABS ${episode}",
            "season": { "1": { "season": "1", "search_format": "SEASON ${episode}" } },
            "season_absolute": { "1": { "season": "1", "search_format": "SEASON-ABS ${episode}" } }
        }))
        .expect("mapping");
        assert_eq!(
            resolve_season_search_meta(&mapping, "1", 1, false, NORMAL_FMT, ABS_FMT).search_format,
            "SEASON ${episode}"
        );
        assert_eq!(
            resolve_season_search_meta(&mapping, "1", 1, true, NORMAL_FMT, ABS_FMT).search_format,
            "SEASON-ABS ${episode}"
        );

        // A normal-map season override is invisible in absolute mode.
        let mapping: MappingRule = serde_json::from_value(serde_json::json!({
            "target_title": "Test",
            "search_format_absolute": "ABS ${episode}",
            "season": { "1": { "season": "1", "search_format": "SEASON ${episode}" } }
        }))
        .expect("mapping");
        assert_eq!(
            resolve_season_search_meta(&mapping, "1", 1, true, NORMAL_FMT, ABS_FMT).search_format,
            "ABS ${episode}"
        );
    }

    #[test]
    fn test_resolve_season_search_meta_sets_requires_season() {
        let mapping: MappingRule = serde_json::from_value(serde_json::json!({
            "target_title": "Test",
            "search_format": "S${season:02}E${episode:02}",
            "search_format_absolute": "E${episode:02}",
        }))
        .expect("mapping");

        // Season-scoped template → season is required.
        let normal = resolve_season_search_meta(&mapping, "1", 1, false, NORMAL_FMT, ABS_FMT);
        assert!(normal.requires_season);
        assert_eq!(normal.required_season(), Some(1));

        // Absolute template omits `${season}` → season-less releases are allowed.
        let absolute = resolve_season_search_meta(&mapping, "1", 1, true, NORMAL_FMT, ABS_FMT);
        assert!(!absolute.requires_season);
        assert_eq!(absolute.required_season(), None);
    }

    // matches_result — the "is a result acceptable" search SSoT

    #[test]
    fn test_matches_result_requires_the_searched_season_and_episode() {
        let mapping = make_mapping("Test", vec![]);
        let meta = resolve_season_search_meta(&mapping, "S01", 1, false, NORMAL_FMT, ABS_FMT);

        assert!(meta.matches_result("Show S01E05 [1080p]", 5));
        // Wrong episode: not among the parsed episode numbers.
        assert!(!meta.matches_result("Show S01E06 [1080p]", 5));
        // Wrong season: the parsed season is not the searched one.
        assert!(!meta.matches_result("Show S02E05 [1080p]", 5));
        // No episode at all.
        assert!(!meta.matches_result("Show [1080p]", 5));
    }

    #[test]
    fn test_matches_result_ignores_the_query_template() {
        // The template only shapes the query; a template carrying source-specific
        // syntax (here a literal `|` alternation) must not gate matching.
        let mapping: MappingRule = serde_json::from_value(serde_json::json!({
            "target_title": "Test",
            "search_format": "S${season:02}E${episode:02}|${episode:02}",
        }))
        .expect("mapping");
        let meta = resolve_season_search_meta(&mapping, "S01", 1, false, NORMAL_FMT, ABS_FMT);
        assert!(meta.matches_result("Show S01E05 [1080p]", 5));
        assert!(!meta.matches_result("Show S01E06 [1080p]", 5));
    }

    #[test]
    fn test_matches_result_requires_declared_season_when_template_has_season() {
        let mapping = make_mapping("Test", vec![]);
        let meta = resolve_season_search_meta(&mapping, "S01", 1, false, NORMAL_FMT, ABS_FMT);
        // The template has `${season}`, so a season-less release is blocked even
        // though the lenient parser would fall back to season 1.
        assert!(!meta.matches_result("Test Show - 01 1080p", 1));
        assert!(meta.matches_result("Test Show S01E01 1080p", 1));
    }

    #[test]
    fn test_matches_result_allows_seasonless_when_template_has_no_season() {
        let mapping: MappingRule = serde_json::from_value(serde_json::json!({
            "target_title": "Test",
            "search_format": "${episode:02}",
        }))
        .expect("mapping");
        let meta = resolve_season_search_meta(&mapping, "S01", 1, false, NORMAL_FMT, ABS_FMT);
        // No `${season}` in the template → the lenient parse may place it.
        assert!(meta.matches_result("Test Show - 01 1080p", 1));
    }

    #[test]
    fn test_matches_result_hail_mary_ordered_bare_numbers() {
        let mapping: MappingRule = serde_json::from_value(serde_json::json!({
            "target_title": "Test",
            "search_format": "S${season:02}E${episode:02}",
        }))
        .expect("mapping");
        let meta = resolve_season_search_meta(&mapping, "S02", 2, false, NORMAL_FMT, ABS_FMT);
        // The structured parser can't place this, so the hail mary matches the
        // bare ordered numbers.
        assert!(meta.matches_result("Test Show 2 - 45 1080p", 45));
        // Wrong order, missing episode, or a non-standalone number all fail.
        assert!(!meta.matches_result("Test Show 45 - 2 1080p", 45));
        assert!(!meta.matches_result("Test Show 2 1080p", 45));
        assert!(!meta.matches_result("Test Show 245 1080p", 45));
    }

    #[test]
    fn test_parse_search_result_title_enforces_required_season() {
        // A season-based search accepts only the declared searched season.
        let same =
            parse_search_result_title("Show S02E05 [1080p]", Some(2)).expect("matching season");
        assert_eq!(same.seasons, vec![2]);
        assert_eq!(same.episodes, vec![5]);

        // No season (would default to S01) or a different season is unrelated.
        assert!(parse_search_result_title("Show - 05 [1080p]", Some(1)).is_none());
        assert!(parse_search_result_title("Show S05E05 [1080p]", Some(2)).is_none());
    }

    #[test]
    fn test_parse_search_result_title_allows_seasonless_when_none() {
        // `required_season = None` (RSS / manual): season-less is fine and still
        // defaults to season 1.
        let info = parse_search_result_title("Show - 05 [1080p]", None).expect("lenient parse");
        assert_eq!(info.seasons, vec![1]);
        assert_eq!(info.episodes, vec![5]);
    }

    #[test]
    fn test_resolve_season_search_meta_with_alias_number() {
        let mapping: MappingRule = serde_json::from_value(serde_json::json!({
            "target_title": "Test",
            "season": {
                "S02": {
                    "season": "S02",
                    "alias_season_number": 4,
                    "search_format": "E${episode:02}",
                }
            }
        }))
        .expect("Failed to create mapping");
        let meta = resolve_season_search_meta(&mapping, "S02", 2, false, NORMAL_FMT, ABS_FMT);
        assert_eq!(meta.search_format, "E${episode:02}");
        assert_eq!(
            meta.search_season_num, 4,
            "alias_season_number=4 should override season=2"
        );
    }

    #[test]
    fn test_resolve_season_search_meta_matches_numeric_key_from_padded_input() {
        // Regression: the episode-level auto-search sends `EpisodeViewModel.season`
        // ("S02") while overrides are keyed "2" (the normalised UI form). The
        // alias must still apply — otherwise the search uses the real season.
        let mapping: MappingRule = serde_json::from_value(serde_json::json!({
            "target_title": "Test",
            "season": {
                "2": { "season": "2", "alias_season_number": 1 }
            }
        }))
        .expect("Failed to create mapping");
        let meta = resolve_season_search_meta(&mapping, "S02", 2, false, NORMAL_FMT, ABS_FMT);
        assert_eq!(
            meta.search_season_num, 1,
            "'S02' should match the override keyed '2'"
        );
    }

    #[test]
    fn test_resolve_season_search_meta_returns_episode_offset() {
        let mapping: MappingRule = serde_json::from_value(serde_json::json!({
            "target_title": "Test",
            "season": {
                "2": { "season": "2", "episode_offset": 1169 }
            }
        }))
        .expect("Failed to create mapping");
        let meta = resolve_season_search_meta(&mapping, "S02", 2, false, NORMAL_FMT, ABS_FMT);
        assert_eq!(
            meta.episode_offset, 1169,
            "offset must be surfaced to callers"
        );
    }

    #[test]
    fn test_split_aliases_by_source_matches_season_alias_across_key_formats() {
        let mapping: MappingRule = serde_json::from_value(serde_json::json!({
            "target_title": "Test",
            "season": {
                "2": { "season": "2", "aliases": ["SeasonTwoAlias"] }
            }
        }))
        .expect("Failed to create mapping");
        let (generic, _) = split_aliases_by_source(&mapping, "S02", false);
        assert_eq!(generic, vec!["SeasonTwoAlias"]);
    }

    // Custom slug prefix with season override
    // Verifies that `@nyaa:term` notation works correctly at every priority level.

    #[test]
    fn test_custom_slug_prefix_works_at_series_level() {
        let mapping = make_mapping("Official", vec!["@nyaa:NyaaSpecific", "Generic"]);
        let (generic, source) = split_aliases_by_source(&mapping, "S01", false);
        assert_eq!(generic, vec!["Generic"]);
        assert_eq!(source.get("nyaa").unwrap(), &vec!["NyaaSpecific"]);
    }

    #[test]
    fn test_custom_slug_prefix_works_at_season_level() {
        let mapping = make_mapping_with_season_aliases(
            "Official",
            vec!["@nyaa:IgnoredSeries"],
            "S02",
            vec!["@nyaa:SeasonSpecific"],
        );
        let (generic, source) = split_aliases_by_source(&mapping, "S02", false);
        assert!(generic.is_empty(), "season slugs should have no generic");
        assert_eq!(
            source.get("nyaa").unwrap(),
            &vec!["SeasonSpecific"],
            "Season-level @nyaa: prefix should take priority"
        );
    }

    #[test]
    fn test_custom_slug_prefix_mixed_source_and_generic_at_season_level() {
        let mapping = make_mapping_with_season_aliases(
            "Official",
            vec![],
            "S03",
            vec!["@nyaa:NyaaOnly", "GenericSeason"],
        );
        let (generic, source) = split_aliases_by_source(&mapping, "S03", false);
        assert_eq!(generic, vec!["GenericSeason"]);
        assert_eq!(
            source.get("nyaa").unwrap(),
            &vec!["NyaaOnly"],
            "@nyaa: prefix at season level should still route to Nyaa"
        );
    }

    // build_plugin_aliases tests

    #[test]
    fn test_build_plugin_aliases_combines_generic_and_specific() {
        let mut source_aliases = HashMap::new();
        source_aliases.insert("plugin-nyaa".to_string(), vec!["Anime".to_string()]);

        let generic = vec!["Generic Show".to_string()];

        let result = build_plugin_aliases(&source_aliases, &generic);
        let entry = result.get("plugin-nyaa").unwrap();
        assert!(entry.contains(&"Generic Show".to_string()));
        assert!(entry.contains(&"Anime".to_string()));
    }

    #[test]
    fn test_build_plugin_aliases_no_source_uses_generic_only() {
        let mut source_aliases = HashMap::new();
        source_aliases.insert("plugin-rss".to_string(), vec![]);

        let generic = vec!["Show".to_string()];

        let result = build_plugin_aliases(&source_aliases, &generic);
        let entry = result.get("plugin-rss").unwrap();
        assert_eq!(entry.len(), 1);
        assert!(entry.contains(&"Show".to_string()));
    }

    // resolve_final_aliases tests

    #[test]
    fn test_resolve_final_aliases_uses_plugin_specific() {
        let mut plugin_aliases = HashMap::new();
        plugin_aliases.insert("p1".to_string(), vec!["Specific".to_string()]);
        let generic = vec!["Generic".to_string()];

        let result = resolve_final_aliases("p1", &plugin_aliases, &generic, "Title");
        assert_eq!(result, vec!["Specific"]);
    }

    #[test]
    fn test_resolve_final_aliases_falls_back_to_generic() {
        let plugin_aliases = HashMap::new();
        let generic = vec!["Generic Alias".to_string()];

        let result = resolve_final_aliases("unknown", &plugin_aliases, &generic, "Title");
        assert_eq!(result, vec!["Generic Alias"]);
    }

    #[test]
    fn test_resolve_final_aliases_falls_back_to_target_title() {
        let plugin_aliases = HashMap::new();
        let generic: Vec<String> = vec![];

        let result = resolve_final_aliases("unknown", &plugin_aliases, &generic, "Fallback Title");
        assert_eq!(result, vec!["Fallback Title"]);
    }
}
