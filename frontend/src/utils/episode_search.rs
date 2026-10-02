// Episode search-query resolution — SSoT for the episode-details modal's manual
// search box.
//
// The manual box is seeded with a plain, human-editable default and resolves the
// episode's source numbering through the same season override the backend uses
// (`resolve_search_coords`), so aliases/offsets can't diverge.
//
// Auto-search queries are NOT built here: the backend renders the search keys and
// the source plugin reports the string(s) it sent.

use jumbie_shared::formatting::render_search_key;
use jumbie_shared::mapping::{DEFAULT_SEASON_NUM, resolve_season_num};
use jumbie_shared::types::{EpisodeViewModel, SeasonOverride};

/// The numbering and search format for one episode, resolved through the active
/// season override.
struct SearchCoords {
    season: i32,
    episode: i32,
    search_format: String,
}

/// Resolve the season override and source numbering for one episode.
///
/// `season_overrides` is the override list for the *active* numbering mode
/// (the caller picks `settings.season` vs `settings.season_absolute`), matching
/// the backend's `resolve_season_search_meta`. Every local→source conversion
/// goes through the override, so the frontend and backend can't diverge on the
/// episode offset.
///
/// `series_search_format` is the series-level template already resolved against
/// the global default for the active mode; a season override replaces it.
///
/// Absolute mode needs no special handling beyond the season number: its
/// episodes are canonically `S01` (`ABSOLUTE_SEASON_NUM`) with the episode
/// number already absolute, so the same conversion applies as in normal mode.
fn resolve_search_coords(
    episode: &EpisodeViewModel,
    season_overrides: &[SeasonOverride],
    series_search_format: &str,
    absolute_numbering: bool,
) -> SearchCoords {
    // SSoT: absolute mode is canonically season 1; a normal-mode label must be
    // numeric. A normal-mode episode with no recorded season is legacy/partial data
    // — fall back to the release convention (season 1), never to 0.
    let season_num =
        resolve_season_num(&episode.season, absolute_numbering).unwrap_or(DEFAULT_SEASON_NUM);

    // Match on either the label (`"S01"`) or its numeric form (`"1"`), covering
    // both key styles overrides are stored under.
    let matched = season_overrides
        .iter()
        .find(|o| o.season == episode.season || o.season == season_num.to_string());

    SearchCoords {
        season: matched.map_or(season_num, |o| o.effective_search_season(season_num)),
        episode: matched.map_or(episode.episode, |o| o.source_episode(episode.episode)),
        search_format: matched
            .and_then(|o| o.search_format.clone())
            .unwrap_or_else(|| series_search_format.to_string()),
    }
}

/// Resolve the **manual** search-box default text for one episode.
///
/// The box is seeded with `{title} {rendered search key}`; a blank/omitted key
/// falls back to the title alone. Season and episode come from
/// [`resolve_search_coords`], so the alias/offset/template handling can't diverge
/// from the backend.
pub fn resolve_manual_search_query(
    episode: &EpisodeViewModel,
    series_title: &str,
    season_overrides: &[SeasonOverride],
    series_search_format: &str,
    absolute_numbering: bool,
) -> String {
    let coords = resolve_search_coords(
        episode,
        season_overrides,
        series_search_format,
        absolute_numbering,
    );

    let key = render_search_key(&coords.search_format, coords.season, coords.episode);
    if key.is_empty() {
        series_title.to_string()
    } else {
        format!("{} {}", series_title, key)
    }
}
