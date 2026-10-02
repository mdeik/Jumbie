// Episodes Database Operations
// Episode-level database operations. Sub-modules: crud (insert/update/delete),
// queries (read-only), assign (file assignment + batch metadata).
//
// The episodes table has a single unique constraint on episode_id (UUID) — the
// old (series_title, season, episode) UNIQUE was dropped because series_title is
// mutable. `insert_episode` checks by episode_id and then UPDATEs or INSERTs.

pub mod assign;
pub mod crud;
pub mod queries;

pub use crud::InsertEpisodeParams;
pub use crud::InsertParentEpisodeParams;
pub use crud::SaveCustomMetadataParams;

use crate::models::activity::log::ActivityLogRow;
use jumbie_shared::formatting::{LabelStyle, fmt_episode, fmt_season_episode};

// Activity Row Merging
// Activity-row merging: consecutive activity_log rows for the same series, event
// type, and status within a 5-minute window are merged, with the episode range
// rendered into `details` (e.g. "S03E01-E06"). The merge happens at the row level
// so episode info is still available before the ActivityItem conversion.
//
// TIME_WINDOW_MINUTES: max gap between consecutive rows to consider merging (rows
// arrive in DESC timestamp order).
pub(crate) const TIME_WINDOW_MINUTES: i64 = 5;

pub(crate) fn merge_activity_rows(rows: Vec<ActivityLogRow>) -> Vec<ActivityLogRow> {
    if rows.is_empty() {
        return rows;
    }

    // Merge-group accumulator: base row + collected episode entries.
    struct MergeGroup {
        base: ActivityLogRow,
        episodes: Vec<(Option<String>, i32, Option<i32>)>,
    }

    let mut groups: Vec<MergeGroup> = Vec::new();

    for row in rows {
        let season = row.season.clone();
        let ep = row.episode;
        let ep_end = row.episode_end;

        // Match any group with the same event_type, series_title, and status within
        // the time window. Checking ALL groups (not just the last) lets same-series
        // events interleaved with other activity still consolidate.
        let merge_idx = groups.iter().position(|g| {
            g.base.event_type == row.event_type
                && !g.base.series_title.is_empty()
                && g.base.series_title == row.series_title
                && g.base.status == row.status
                && season.is_some()
                && ep.is_some()
                && (g.base.created_at - row.created_at).num_minutes() <= TIME_WINDOW_MINUTES
        });

        if let Some(idx) = merge_idx {
            let group = &mut groups[idx];
            // Keep the latest created_at (group base is newer since rows DESC)
            if row.created_at > group.base.created_at {
                group.base.created_at = row.created_at;
            }
            group.episodes.push((season, ep.unwrap(), ep_end));
        } else {
            let mut eps = Vec::new();
            if let Some(ep_val) = ep {
                eps.push((season, ep_val, ep_end));
            }
            groups.push(MergeGroup {
                base: row,
                episodes: eps,
            });
        }
    }

    groups
        .into_iter()
        .map(|g| {
            let mut merged = g.base;
            if g.episodes.len() > 1 {
                merged.details = Some(format_episode_ranges(&g.episodes));
            }
            merged
        })
        .collect()
}

/// Format a list of (season, episode, episode_end) tuples into a human-readable
/// episode range string.
///
/// When the same tuple appears multiple times, the count is shown as `(xN)`.
/// Consecutive episodes with the same count are merged into a range.
///
/// Examples:
///   [(Some("3"), 1, None)]                                       → "S03E01"
///   [(Some("3"), 1, Some(6))]                                     → "S03E01-E06"
///   [(Some("3"), 1, None), (Some("3"), 5, None)]                → "S03E01, S03E05"
///   [(Some("3"), 1, Some(6)), (Some("4"), 1, Some(9))]          → "S03E01-E06, S04E01-E09"
///   [(Some("3"), 8, None), (Some("3"), 8, None), (Some("3"), 8, None)]  → "S03E08 (x3)"
///   [(Some("1"), 8, None), (Some("1"), 8, None), (Some("1"), 9, None), (Some("1"), 9, None)] → "S01E08-E09 (x2)"
///   [(Some("1"), 2, None), (Some("1"), 3, None), (Some("1"), 4, None), (Some("1"), 5, None)] → "S01E02-E05"
pub(crate) fn format_episode_ranges(episodes: &[(Option<String>, i32, Option<i32>)]) -> String {
    if episodes.is_empty() {
        return String::new();
    }

    // Count occurrences of each distinct (season, episode, episode_end)
    let mut counts = std::collections::HashMap::<(Option<String>, i32, Option<i32>), i32>::new();
    for ep in episodes {
        *counts.entry(ep.clone()).or_insert(0) += 1;
    }

    let mut unique: Vec<_> = counts.keys().cloned().collect();
    unique.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));

    struct EpisodeGroup {
        season: String,
        start: i32,
        end: i32,
        count: i32,
    }

    let mut groups: Vec<EpisodeGroup> = Vec::new();

    for (season, ep, ep_end) in &unique {
        let season_str = season.as_deref().unwrap_or("").to_string();
        let count = counts[&(season.clone(), *ep, *ep_end)];
        let range_end = ep_end.filter(|e| *e > *ep).unwrap_or(*ep);

        // Extend the last group when the season matches, the episode is consecutive,
        // and the multiplicity count matches.
        let mut extended = false;
        if let Some(last) = groups.last_mut()
            && last.season == season_str
            && last.end + 1 == *ep
            && last.count == count
        {
            last.end = range_end;
            extended = true;
        }

        if !extended {
            groups.push(EpisodeGroup {
                season: season_str,
                start: *ep,
                end: range_end,
                count,
            });
        }
    }

    groups
        .iter()
        .map(|g| {
            let ep_str = if g.season.is_empty() {
                if g.start == g.end {
                    fmt_episode(g.start, LabelStyle::Short)
                } else {
                    format!(
                        "{}-{}",
                        fmt_episode(g.start, LabelStyle::Short),
                        fmt_episode(g.end, LabelStyle::Short)
                    )
                }
            } else {
                let s_int = g.season.parse::<i32>().unwrap_or(1);
                if g.start == g.end {
                    fmt_season_episode(s_int, g.start, None, LabelStyle::Short)
                } else {
                    fmt_season_episode(s_int, g.start, Some(g.end), LabelStyle::Short)
                }
            };
            if g.count > 1 {
                format!("{} (x{})", ep_str, g.count)
            } else {
                ep_str
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    /// Episode window (season + episode range) of an activity row.
    struct EpWindow<'a> {
        season: Option<&'a str>,
        episode: Option<i32>,
        episode_end: Option<i32>,
    }

    fn make_row(
        id: i64,
        ts_secs: i64,
        event_type: &str,
        series_title: &str,
        window: EpWindow<'_>,
        status: &str,
    ) -> ActivityLogRow {
        ActivityLogRow {
            id,
            created_at: chrono::Utc.timestamp_opt(ts_secs, 0).unwrap(),
            event_type: event_type.to_string(),
            series_title: series_title.to_string(),
            season: window.season.map(|s| s.to_string()),
            episode: window.episode,
            episode_end: window.episode_end,
            title: None,
            details: None,
            status: status.to_string(),
        }
    }

    #[test]
    fn test_merge_same_series_consecutive_episodes() {
        let rows = vec![
            make_row(
                1,
                1000,
                "download",
                "The Time Machine",
                EpWindow {
                    season: Some("3"),
                    episode: Some(3),
                    episode_end: None,
                },
                "Success",
            ),
            make_row(
                2,
                990,
                "download",
                "The Time Machine",
                EpWindow {
                    season: Some("3"),
                    episode: Some(2),
                    episode_end: None,
                },
                "Success",
            ),
            make_row(
                3,
                980,
                "download",
                "The Time Machine",
                EpWindow {
                    season: Some("3"),
                    episode: Some(1),
                    episode_end: None,
                },
                "Success",
            ),
        ];
        let merged = merge_activity_rows(rows);
        assert_eq!(merged.len(), 1, "3 consecutive eps should merge into 1");
        assert_eq!(
            merged[0].details.as_deref(),
            Some("S03E01-E03"),
            "Should collapse to range"
        );
        assert_eq!(merged[0].series_title, "The Time Machine");
    }

    #[test]
    fn test_merge_different_series_stays_separate() {
        let rows = vec![
            make_row(
                1,
                1000,
                "download",
                "The Time Machine",
                EpWindow {
                    season: Some("3"),
                    episode: Some(1),
                    episode_end: None,
                },
                "Success",
            ),
            make_row(
                2,
                990,
                "download",
                "Classroom of the Elite",
                EpWindow {
                    season: Some("4"),
                    episode: Some(14),
                    episode_end: None,
                },
                "Success",
            ),
        ];
        let merged = merge_activity_rows(rows);
        assert_eq!(merged.len(), 2, "Different series should not merge");
    }

    #[test]
    fn test_merge_different_event_types_stays_separate() {
        let rows = vec![
            make_row(
                1,
                1000,
                "download",
                "The Time Machine",
                EpWindow {
                    season: Some("3"),
                    episode: Some(1),
                    episode_end: None,
                },
                "Success",
            ),
            make_row(
                2,
                990,
                "analyze",
                "The Time Machine",
                EpWindow {
                    season: Some("3"),
                    episode: Some(1),
                    episode_end: None,
                },
                "Success",
            ),
        ];
        let merged = merge_activity_rows(rows);
        assert_eq!(merged.len(), 2, "Different event types should not merge");
    }

    #[test]
    fn test_merge_different_statuses_stays_separate() {
        let rows = vec![
            make_row(
                1,
                1000,
                "download",
                "The Time Machine",
                EpWindow {
                    season: Some("3"),
                    episode: Some(1),
                    episode_end: None,
                },
                "Success",
            ),
            make_row(
                2,
                990,
                "download",
                "The Time Machine",
                EpWindow {
                    season: Some("3"),
                    episode: Some(2),
                    episode_end: None,
                },
                "Failed",
            ),
        ];
        let merged = merge_activity_rows(rows);
        assert_eq!(merged.len(), 2, "Different statuses should not merge");
    }

    #[test]
    fn test_merge_beyond_time_window_stays_separate() {
        // 6 minutes apart (TIME_WINDOW_MINUTES = 5)
        let rows = vec![
            make_row(
                1,
                600,
                "download",
                "The Time Machine",
                EpWindow {
                    season: Some("3"),
                    episode: Some(3),
                    episode_end: None,
                },
                "Success",
            ),
            make_row(
                2,
                200,
                "download",
                "The Time Machine",
                EpWindow {
                    season: Some("3"),
                    episode: Some(2),
                    episode_end: None,
                },
                "Success",
            ),
        ];
        let merged = merge_activity_rows(rows);
        assert_eq!(merged.len(), 2, "Events 6min apart should not merge");
    }

    #[test]
    fn test_merge_no_episode_info_does_not_merge() {
        let rows = vec![
            make_row(
                1,
                1000,
                "analyze",
                "",
                EpWindow {
                    season: None,
                    episode: None,
                    episode_end: None,
                },
                "Success",
            ),
            make_row(
                2,
                990,
                "analyze",
                "",
                EpWindow {
                    season: None,
                    episode: None,
                    episode_end: None,
                },
                "Success",
            ),
        ];
        let merged = merge_activity_rows(rows);
        assert_eq!(
            merged.len(),
            2,
            "Rows without episode info should not merge"
        );
    }

    #[test]
    fn test_merge_non_consecutive_episodes() {
        let rows = vec![
            make_row(
                1,
                1000,
                "download",
                "The Time Machine",
                EpWindow {
                    season: Some("3"),
                    episode: Some(5),
                    episode_end: None,
                },
                "Success",
            ),
            make_row(
                2,
                990,
                "download",
                "The Time Machine",
                EpWindow {
                    season: Some("3"),
                    episode: Some(3),
                    episode_end: None,
                },
                "Success",
            ),
            make_row(
                3,
                980,
                "download",
                "The Time Machine",
                EpWindow {
                    season: Some("3"),
                    episode: Some(1),
                    episode_end: None,
                },
                "Success",
            ),
        ];
        let merged = merge_activity_rows(rows);
        assert_eq!(merged.len(), 1, "Should merge into 1 group");
        assert_eq!(
            merged[0].details.as_deref(),
            Some("S03E01, S03E03, S03E05"),
            "Non-consecutive should list individually"
        );
    }

    #[test]
    fn test_merge_with_episode_end_range() {
        let rows = vec![
            make_row(
                1,
                1000,
                "download",
                "The Time Machine",
                EpWindow {
                    season: Some("3"),
                    episode: Some(4),
                    episode_end: Some(6),
                },
                "Success",
            ),
            make_row(
                2,
                990,
                "download",
                "The Time Machine",
                EpWindow {
                    season: Some("3"),
                    episode: Some(1),
                    episode_end: Some(3),
                },
                "Success",
            ),
        ];
        let merged = merge_activity_rows(rows);
        assert_eq!(merged.len(), 1, "Multi-ep ranges should merge");
        assert_eq!(
            merged[0].details.as_deref(),
            Some("S03E01-E06"),
            "Adjacent multi-ep ranges should collapse"
        );
    }

    #[test]
    fn test_merge_different_seasons() {
        let rows = vec![
            make_row(
                1,
                1000,
                "download",
                "The Time Machine",
                EpWindow {
                    season: Some("3"),
                    episode: Some(1),
                    episode_end: None,
                },
                "Success",
            ),
            make_row(
                2,
                990,
                "download",
                "The Time Machine",
                EpWindow {
                    season: Some("4"),
                    episode: Some(1),
                    episode_end: None,
                },
                "Success",
            ),
        ];
        let merged = merge_activity_rows(rows);
        assert_eq!(merged.len(), 1, "Same series across seasons should merge");
        assert_eq!(
            merged[0].details.as_deref(),
            Some("S03E01, S04E01"),
            "Different seasons should list separately"
        );
    }

    #[test]
    fn test_merge_empty_rows() {
        let merged = merge_activity_rows(vec![]);
        assert!(merged.is_empty());
    }

    #[test]
    fn test_merge_same_series_interleaved_with_other_activity() {
        // Reproduces the user's bug: 5 Metadata rows for "Red River" with
        // other activity in between.  The previous groups.last() approach
        // could not merge these — each Metadata row after an intervening
        // row would fail to match the *last* group (wrong type/series).
        let now = 1000;
        let rows = vec![
            make_row(
                1,
                now,
                "metadata",
                "Red River",
                EpWindow {
                    season: Some("1"),
                    episode: Some(1),
                    episode_end: Some(12),
                },
                "Success",
            ),
            make_row(
                2,
                now - 10,
                "download",
                "Other Show",
                EpWindow {
                    season: Some("2"),
                    episode: Some(3),
                    episode_end: None,
                },
                "Success",
            ),
            make_row(
                3,
                now - 20,
                "metadata",
                "Red River",
                EpWindow {
                    season: Some("1"),
                    episode: Some(1),
                    episode_end: Some(12),
                },
                "Success",
            ),
            make_row(
                4,
                now - 30,
                "import",
                "Another",
                EpWindow {
                    season: None,
                    episode: None,
                    episode_end: None,
                },
                "Success",
            ),
            make_row(
                5,
                now - 40,
                "metadata",
                "Red River",
                EpWindow {
                    season: Some("1"),
                    episode: Some(1),
                    episode_end: Some(12),
                },
                "Success",
            ),
            make_row(
                6,
                now - 50,
                "download",
                "Yet Another",
                EpWindow {
                    season: Some("1"),
                    episode: Some(5),
                    episode_end: None,
                },
                "Failed",
            ),
            make_row(
                7,
                now - 60,
                "metadata",
                "Red River",
                EpWindow {
                    season: Some("1"),
                    episode: Some(1),
                    episode_end: Some(12),
                },
                "Success",
            ),
            make_row(
                8,
                now - 70,
                "metadata",
                "Red River",
                EpWindow {
                    season: Some("1"),
                    episode: Some(1),
                    episode_end: Some(12),
                },
                "Success",
            ),
        ];
        let merged = merge_activity_rows(rows);

        // Should be: 5 groups for the 5 unique (type, series, status) combos:
        //   1. Metadata/Red River/Success  (5 rows merged → 1)
        //   2. Download/Other Show/Success
        //   3. Import/Another/Success
        //   4. Download/Yet Another/Failed
        let metadata: Vec<_> = merged
            .iter()
            .filter(|r| r.event_type == "metadata")
            .collect();
        assert_eq!(
            metadata.len(),
            1,
            "All 5 Metadata/Red River rows should merge into 1"
        );
        assert_eq!(metadata[0].series_title, "Red River");
        assert_eq!(
            metadata[0].details.as_deref(),
            Some("S01E01-E12 (x5)"),
            "5 identical episode tuples should show (x5)"
        );
        assert_eq!(merged.len(), 4, "Should have 4 total groups after merging");
    }

    #[test]
    fn test_merge_single_row_unchanged() {
        let rows = vec![make_row(
            1,
            1000,
            "download",
            "The Time Machine",
            EpWindow {
                season: Some("3"),
                episode: Some(1),
                episode_end: None,
            },
            "Success",
        )];
        let merged = merge_activity_rows(rows);
        assert_eq!(merged.len(), 1);
        assert_eq!(
            merged[0].details, None,
            "Single row details should not be set by merge"
        );
    }

    #[test]
    fn test_format_single_episode() {
        let eps = vec![(Some("3".into()), 1, None)];
        assert_eq!(format_episode_ranges(&eps), "S03E01");
    }

    #[test]
    fn test_format_single_episode_no_season() {
        let eps = vec![(None, 5, None)];
        assert_eq!(format_episode_ranges(&eps), "E05");
    }

    #[test]
    fn test_format_consecutive_range() {
        let eps = vec![(Some("1".into()), 3, Some(6))];
        assert_eq!(format_episode_ranges(&eps), "S01E03-E06");
    }

    #[test]
    fn test_format_non_consecutive() {
        let eps = vec![
            (Some("1".into()), 1, None),
            (Some("1".into()), 3, None),
            (Some("1".into()), 5, None),
        ];
        assert_eq!(format_episode_ranges(&eps), "S01E01, S01E03, S01E05");
    }

    #[test]
    fn test_format_mixed_seasons() {
        let eps = vec![(Some("1".into()), 1, None), (Some("2".into()), 5, Some(9))];
        assert_eq!(format_episode_ranges(&eps), "S01E01, S02E05-E09");
    }

    #[test]
    fn test_format_adjacent_ranges_collapse() {
        let eps = vec![
            (Some("1".into()), 1, Some(3)),
            (Some("1".into()), 4, Some(6)),
        ];
        assert_eq!(format_episode_ranges(&eps), "S01E01-E06");
    }

    #[test]
    fn test_format_empty() {
        assert_eq!(format_episode_ranges(&[]), "");
    }

    #[test]
    fn test_format_complex_mix() {
        // Represents: S01E03, S01E05, S04E01-E09, S04E11
        let eps = vec![
            (Some("1".into()), 3, None),
            (Some("1".into()), 5, None),
            (Some("4".into()), 1, Some(9)),
            (Some("4".into()), 11, None),
        ];
        assert_eq!(
            format_episode_ranges(&eps),
            "S01E03, S01E05, S04E01-E09, S04E11"
        );
    }

    #[test]
    fn test_format_unsorted_input() {
        let eps = vec![
            (Some("1".into()), 5, None),
            (Some("1".into()), 1, None),
            (Some("1".into()), 3, None),
        ];
        assert_eq!(format_episode_ranges(&eps), "S01E01, S01E03, S01E05");
    }

    #[test]
    fn test_format_duplicate_single_episode() {
        let eps = vec![
            (Some("3".into()), 8, None),
            (Some("3".into()), 8, None),
            (Some("3".into()), 8, None),
        ];
        assert_eq!(format_episode_ranges(&eps), "S03E08 (x3)");
    }

    #[test]
    fn test_format_consecutive_with_same_count() {
        let eps = vec![
            (Some("1".into()), 8, None),
            (Some("1".into()), 8, None),
            (Some("1".into()), 9, None),
            (Some("1".into()), 9, None),
        ];
        assert_eq!(format_episode_ranges(&eps), "S01E08-E09 (x2)");
    }

    #[test]
    fn test_format_consecutive_single_count() {
        let eps = vec![
            (Some("1".into()), 2, None),
            (Some("1".into()), 3, None),
            (Some("1".into()), 4, None),
            (Some("1".into()), 5, None),
        ];
        assert_eq!(format_episode_ranges(&eps), "S01E02-E05");
    }

    #[test]
    fn test_format_duplicate_and_consecutive_mix() {
        let eps = vec![
            (Some("1".into()), 2, None),
            (Some("1".into()), 3, None),
            (Some("1".into()), 4, None),
            (Some("1".into()), 5, None),
            (Some("1".into()), 8, None),
            (Some("1".into()), 8, None),
            (Some("1".into()), 8, None),
            (Some("1".into()), 9, None),
            (Some("1".into()), 9, None),
            (Some("1".into()), 9, None),
            (Some("1".into()), 10, None),
            (Some("1".into()), 10, None),
            (Some("1".into()), 10, None),
            (Some("1".into()), 10, None),
            (Some("1".into()), 10, None),
        ];
        assert_eq!(
            format_episode_ranges(&eps),
            "S01E02-E05, S01E08-E09 (x3), S01E10 (x5)"
        );
    }

    #[test]
    fn test_format_duplicate_range() {
        let eps = vec![
            (Some("1".into()), 1, Some(6)),
            (Some("1".into()), 1, Some(6)),
        ];
        assert_eq!(format_episode_ranges(&eps), "S01E01-E06 (x2)");
    }

    #[test]
    fn test_format_duplicate_with_different_counts_not_merged() {
        let eps = vec![
            (Some("1".into()), 1, None),
            (Some("1".into()), 1, None),
            (Some("1".into()), 2, None),
        ];
        // E01 appears twice (count=2), E02 appears once (count=1)
        // Different counts → separate groups, even though consecutive
        assert_eq!(format_episode_ranges(&eps), "S01E01 (x2), S01E02");
    }
}
