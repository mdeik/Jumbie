//! Batch auto-assign planning for the "Manage Series Files" modal.
//!
//! The preview and the actual assignment both derive their targets from
//! [`plan_batch_targets`], so what the user sees is exactly what gets assigned.
//!
//! Inputs are the two optional start fields (`manage-series-season-input` /
//! `manage-series-episode-input`). The season is **always fixed** — never rolled
//! over — while episodes count up (or are derived per file):
//!
//! * **Both provided** — season is the supplied season; episodes count up from
//!   the supplied start (a multipart batch instead keeps one episode and numbers
//!   its parts).
//! * **Season only** — season is the supplied season; each file's episode is
//!   derived from its name, else it takes the fallback count (1, 2, …).
//! * **Episode only** — episodes count up from the supplied start; each file's
//!   season is derived from its name, else season 1.
//! * **Both empty** — each file must derive its own season/episode; a file that
//!   cannot is reported as an error and skipped.

/// A single file's resolved assignment target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedTarget {
    pub season: String,
    /// Episode number or range (`"5"` or `"1-3"`).
    pub episode: String,
    /// Multi-part marker, when the batch is a multipart assignment.
    pub part: Option<u32>,
}

/// Why a file could not be planned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanError {
    /// The file's season and/or episode could not be parsed (both-empty mode).
    Unparseable,
}

/// Parse a path into `(season, episode, part)`; an empty season/episode means
/// "could not parse".
pub type ParseFn<'a> = &'a dyn Fn(&str) -> (String, String, Option<u32>);

/// Compute per-file targets for `paths`, in order.
///
/// Returns `(path, Ok(target))` for each assignable file and
/// `(path, Err(PlanError::Unparseable))` for files that could not be parsed.
pub fn plan_batch_targets(
    paths: &[String],
    season_input: &str,
    episode_input: &str,
    is_multipart: bool,
    parse: ParseFn<'_>,
) -> Vec<(String, Result<PlannedTarget, PlanError>)> {
    let season_provided = !season_input.trim().is_empty();
    let episode_provided = !episode_input.trim().is_empty();
    let auto_parse = !season_provided && !episode_provided;
    let both_provided = season_provided && episode_provided;

    let start_season = season_input.trim().parse::<i32>().unwrap_or(1);
    let start_episode = episode_input.trim().parse::<i32>().unwrap_or(1);

    let mut out = Vec::with_capacity(paths.len());
    for (i, path) in paths.iter().enumerate() {
        // Both inputs provided: fixed season, episodes count up from the start.
        // A multipart batch keeps a single episode and numbers its parts instead.
        if both_provided {
            let (episode, part) = if is_multipart {
                (start_episode, Some((i + 1) as u32))
            } else {
                (start_episode + i as i32, None)
            };
            out.push((
                path.clone(),
                Ok(PlannedTarget {
                    season: start_season.to_string(),
                    episode: episode.to_string(),
                    part,
                }),
            ));
            continue;
        }

        let (parsed_s, parsed_e, parsed_part) = parse(path);

        if auto_parse {
            if parsed_s.is_empty() || parsed_e.is_empty() {
                out.push((path.clone(), Err(PlanError::Unparseable)));
                continue;
            }
            out.push((
                path.clone(),
                Ok(PlannedTarget {
                    season: parsed_s,
                    episode: parsed_e,
                    part: parsed_part,
                }),
            ));
            continue;
        }

        // Exactly one input provided.
        let target = if season_provided {
            // Season fixed; derive the episode, else fall back to the count (1, 2, …).
            PlannedTarget {
                season: start_season.to_string(),
                episode: if parsed_e.is_empty() {
                    (i as i32 + 1).to_string()
                } else {
                    parsed_e
                },
                part: parsed_part,
            }
        } else {
            // Episodes count up from the supplied start; derive the season, else
            // use season 1.
            PlannedTarget {
                season: if parsed_s.is_empty() {
                    "1".to_string()
                } else {
                    parsed_s
                },
                episode: (start_episode + i as i32).to_string(),
                part: parsed_part,
            }
        };
        out.push((path.clone(), Ok(target)));
    }
    out
}

/// Whether a planned video assignment conflicts with the episode's existing videos.
///
/// The candidate can attach alongside the episode's files only when **every** one of
/// them (that is not itself being reassigned) is a language sibling with a different
/// tag; a duplicate language tag (same artifact base and tag) or a different artifact
/// competes for the slot and conflicts. Auxiliary sidecars never conflict.
pub fn slot_conflict(
    candidate_is_video: bool,
    occupants: &[(&str, bool)],
    candidate_path: &str,
) -> bool {
    if !candidate_is_video {
        return false;
    }
    let live: Vec<&str> = occupants
        .iter()
        .filter(|(_, selected)| !*selected)
        .map(|(path, _)| *path)
        .collect();
    if live.is_empty() {
        return false;
    }
    !live
        .into_iter()
        .all(|occupant| jumbie_shared::parsing::is_variant_of(occupant, candidate_path))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_parse(_path: &str) -> (String, String, Option<u32>) {
        (String::new(), String::new(), None)
    }

    fn paths(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    fn targets(plan: Vec<(String, Result<PlannedTarget, PlanError>)>) -> Vec<(String, String)> {
        plan.into_iter()
            .map(|(_, t)| t.unwrap())
            .map(|t| (t.season, t.episode))
            .collect()
    }

    #[test]
    fn both_provided_fixes_season_and_counts_episodes_up() {
        let p = paths(&["a", "b", "c"]);
        let plan = plan_batch_targets(&p, "2", "5", false, &no_parse);
        assert_eq!(
            targets(plan),
            vec![
                ("2".to_string(), "5".to_string()),
                ("2".to_string(), "6".to_string()),
                ("2".to_string(), "7".to_string()),
            ]
        );
    }

    #[test]
    fn both_provided_is_not_parsed() {
        // A parse that would produce nonsense must be ignored when both inputs are
        // provided (the sequential path never consults it).
        let p = paths(&["a", "b"]);
        let parse = |_: &str| ("9".to_string(), "9".to_string(), None);
        let plan = plan_batch_targets(&p, "1", "1", false, &parse);
        assert_eq!(
            targets(plan),
            vec![
                ("1".to_string(), "1".to_string()),
                ("1".to_string(), "2".to_string()),
            ]
        );
    }

    #[test]
    fn both_provided_multipart_keeps_one_episode_and_numbers_parts() {
        let p = paths(&["a", "b"]);
        let plan = plan_batch_targets(&p, "1", "3", true, &no_parse);
        let targets: Vec<_> = plan
            .into_iter()
            .map(|(_, t)| t.unwrap())
            .map(|t| (t.season, t.episode, t.part))
            .collect();
        assert_eq!(
            targets,
            vec![
                ("1".to_string(), "3".to_string(), Some(1)),
                ("1".to_string(), "3".to_string(), Some(2)),
            ]
        );
    }

    #[test]
    fn both_empty_requires_a_parse_each() {
        let p = paths(&["a", "b"]);
        let parse = |path: &str| match path {
            "a" => ("1".to_string(), "4".to_string(), None),
            _ => (String::new(), String::new(), None),
        };
        let plan = plan_batch_targets(&p, "", "", false, &parse);
        assert_eq!(
            plan[0].1,
            Ok(PlannedTarget {
                season: "1".into(),
                episode: "4".into(),
                part: None
            })
        );
        assert_eq!(plan[1].1, Err(PlanError::Unparseable));
    }

    #[test]
    fn season_only_fixes_season_and_derives_or_counts_episodes() {
        // The supplied season is fixed; each file's episode is derived when
        // available, otherwise it takes the fallback count (1, 2, …).
        let p = paths(&["a", "b", "c"]);
        let parse = |path: &str| match path {
            "a" => ("9".to_string(), "7".to_string(), None),
            _ => (String::new(), String::new(), None),
        };
        let plan = plan_batch_targets(&p, "3", "", false, &parse);
        assert_eq!(
            targets(plan),
            vec![
                ("3".to_string(), "7".to_string()),
                ("3".to_string(), "2".to_string()),
                ("3".to_string(), "3".to_string()),
            ]
        );
    }

    #[test]
    fn season_only_never_rolls_the_season() {
        // Many files, fixed season: the season stays put regardless of count.
        let p = paths(&["a", "b", "c", "d", "e"]);
        let plan = plan_batch_targets(&p, "1", "", false, &no_parse);
        let seasons: Vec<_> = plan.into_iter().map(|(_, t)| t.unwrap().season).collect();
        assert_eq!(seasons, vec!["1", "1", "1", "1", "1"]);
    }

    #[test]
    fn episode_only_counts_up_and_derives_season_else_one() {
        let p = paths(&["a", "b"]);
        let parse = |path: &str| match path {
            "a" => ("4".to_string(), "9".to_string(), None),
            _ => (String::new(), String::new(), None),
        };
        let plan = plan_batch_targets(&p, "", "5", false, &parse);
        assert_eq!(
            targets(plan),
            vec![
                ("4".to_string(), "5".to_string()),
                ("1".to_string(), "6".to_string()),
            ]
        );
    }

    #[test]
    fn slot_conflict_skips_auxiliary_and_selected_occupants() {
        let occ: &[(&str, bool)] = &[("/lib/Show.S01E01.mkv", false)];
        // Auxiliary sidecars never conflict.
        assert!(!slot_conflict(false, occ, "/lib/Show.S01E01.en.srt"));
        // A different video into an occupied, unselected slot conflicts.
        assert!(slot_conflict(true, occ, "/lib/Show.S01E01.1080p.mkv"));
        // The occupant being reassigned clears the conflict.
        assert!(!slot_conflict(true, &[("/lib/x.mkv", true)], "/lib/y.mkv"));
        // An empty slot never conflicts.
        assert!(!slot_conflict(true, &[], "/lib/y.mkv"));
    }

    #[test]
    fn slot_conflict_allows_language_variants_only() {
        let occ: &[(&str, bool)] = &[("/lib/Show.S01E01.mkv", false)];
        // Language tags attach alongside the occupant.
        assert!(!slot_conflict(true, occ, "/lib/Show.S01E01.en.mkv"));
        // An alternative version or collision counter still competes for the slot.
        assert!(slot_conflict(true, occ, "/lib/Show.S01E01.v2.mkv"));
        assert!(slot_conflict(true, occ, "/lib/Show.S01E01.001.mkv"));
        // A genuinely different release still conflicts.
        assert!(slot_conflict(true, occ, "/lib/Show.S01E01.720p.mkv"));
    }

    #[test]
    fn slot_conflict_blocks_a_duplicate_language_tag() {
        // `.en` and the plain file are already attached: a second `.en` conflicts
        // (one file per language tag) while `.eng` attaches alongside.
        let occ: &[(&str, bool)] = &[
            ("/lib/Show.S01E01.mkv", false),
            ("/lib/Show.S01E01.en.mkv", false),
        ];
        assert!(slot_conflict(true, occ, "/dl/Show.S01E01.en.mkv"));
        assert!(!slot_conflict(true, occ, "/dl/Show.S01E01.eng.mkv"));
        // Different tags without a plain name coexist.
        let occ: &[(&str, bool)] = &[("/lib/Show.S01E01.en.mkv", false)];
        assert!(!slot_conflict(true, occ, "/dl/Show.S01E01.eng.mkv"));
        assert!(slot_conflict(true, occ, "/dl/Show.S01E01.en.mkv"));
    }
}
