//! Release acceptance gates, shared by auto-search and manual search.
//!
//! Auto-search silently drops releases that fail these gates. Manual search runs
//! the exact same gates and reports the failures as informational suggestions —
//! results are never hidden. Keeping the rules here (rather than inline in each
//! path) is the SSoT: manual and auto cannot drift.

use std::collections::HashMap;

use jumbie_shared::scoring::{Quality, QualityProfile, ReleaseProfile};
use jumbie_shared::types::{MappingRule, ReleaseCheck, ReleaseCheckId};

use crate::db::autoresolve::RejectedDownloads;
use crate::search::SeasonSearchMeta;

/// One candidate release under evaluation.
pub struct ReleaseCandidate<'a> {
    pub title: &'a str,
    pub link: Option<&'a str>,
    pub download_id: Option<&'a str>,
    /// Final score, after pack modifier and automatic-profile adjustments.
    pub score: i32,
    pub is_season_pack: bool,
}

/// The episode a manual search was scoped to, when any. Absent for a
/// series-level search, which omits the episode gate entirely.
pub struct EpisodeScope {
    /// Season search metadata (search season + local→source offset).
    pub meta: SeasonSearchMeta,
    /// Episodes in SOURCE numbering the release must cover.
    pub source_episodes: Vec<i32>,
    /// Human-readable target, e.g. `S01E02`.
    pub label: String,
    /// Score of the episode's current download, when one exists.
    pub current_score: Option<i32>,
}

/// Everything the gates need. A `None` context means that gate is not applicable
/// and is omitted from the result.
pub struct ReleaseCheckContext<'a> {
    pub mapping: Option<&'a MappingRule>,
    pub merged_scoring: Option<&'a ReleaseProfile>,
    pub quality_profile_name: Option<&'a str>,
    pub quality_profiles: &'a HashMap<String, QualityProfile>,
    pub qualities: &'a HashMap<String, Quality>,
    pub rejected: Option<&'a RejectedDownloads>,
    pub episode_scope: Option<&'a EpisodeScope>,
}

/// The reject gate as a cheap boolean (no description formatting). Every auto
/// path filters its candidates through this before selecting/queueing, so a
/// release rejected after a stalled download can never be re-picked again.
/// Manual search uses [`rejected_failure`] instead, for the human-readable
/// reason and remaining time.
pub fn is_rejected(
    rejected: &RejectedDownloads,
    link: Option<&str>,
    download_id: Option<&str>,
) -> bool {
    rejected.contains(link, download_id)
}

/// The reject-list gate: `Some(reason)` when the release is rejected, describing
/// why and how long until it leaves the reject list.
pub fn rejected_failure(
    rejected: &RejectedDownloads,
    link: Option<&str>,
    download_id: Option<&str>,
) -> Option<String> {
    let rejection = rejected.lookup(link, download_id)?;
    Some(format!(
        "{} — clears in {}",
        rejection.reason,
        remaining_time(rejection.expires_at)
    ))
}

/// The episode-match gate: `Some(reason)` when the release covers none of the
/// requested source episodes.
pub fn episode_failure(
    meta: &SeasonSearchMeta,
    title: &str,
    source_episodes: &[i32],
    label: &str,
) -> Option<String> {
    if source_episodes.is_empty() {
        return None;
    }
    let release = meta.match_release(title);
    if source_episodes.iter().any(|&ep| release.accepts(ep)) {
        None
    } else {
        Some(format!("Does not cover {label}"))
    }
}

/// The quality-profile gate: `Some(reason)` when the title's quality is not in
/// the series' quality profile.
pub fn quality_failure(
    title: &str,
    quality_profile_name: Option<&str>,
    quality_profiles: &HashMap<String, QualityProfile>,
    qualities: &HashMap<String, Quality>,
) -> Option<String> {
    let name = quality_profile_name.filter(|n| !n.is_empty())?;
    if !quality_profiles.contains_key(name) {
        return None;
    }
    if jumbie_shared::quality::passes_quality_filter(title, Some(name), quality_profiles, qualities)
    {
        return None;
    }
    // SSoT: qualities are identified by UUID only (names are mutable). The
    // profile-ordered `detect_quality` is deterministic; on None/Unknown,
    // `detect_quality_any` returns the matched non-profile quality's UUID.
    let quality_id =
        jumbie_shared::quality::detect_quality(title, quality_profiles, qualities, Some(name));
    let label = if quality_id == "None" || quality_id == "Unknown" {
        jumbie_shared::quality::detect_quality_any(title, qualities)
            .map(|uuid| uuid.as_str())
            .unwrap_or("Unknown")
    } else {
        quality_id.as_str()
    };
    Some(format!("Quality '{label}' not allowed by profile"))
}

/// The minimum-score gate: `Some(reason)` when the release scores below the
/// release profile's minimum.
pub fn score_failure(score: i32, min_score: i32) -> Option<String> {
    (score < min_score).then(|| format!("Score {score} below profile minimum {min_score}"))
}

/// The upgrade-target gate: `Some(reason)` when the release would replace an
/// existing download but its quality is not an upgrade target in the profile.
pub fn upgrade_failure(
    mapping: &MappingRule,
    title: &str,
    quality_profiles: &HashMap<String, QualityProfile>,
    qualities: &HashMap<String, Quality>,
) -> Option<String> {
    jumbie_shared::quality::is_upgrade_blocked_by_profile(
        mapping,
        title,
        quality_profiles,
        qualities,
    )
    .then(|| "Quality is not an allowed upgrade target".to_string())
}

/// Evaluate every applicable gate for a candidate, in display order. Only
/// failures carry a description; callers render failures only.
pub fn evaluate(
    candidate: &ReleaseCandidate<'_>,
    ctx: &ReleaseCheckContext<'_>,
) -> Vec<ReleaseCheck> {
    let mut checks = Vec::new();

    // Rejected — always evaluated; the reject list simply may be empty.
    let rejected = ctx
        .rejected
        .and_then(|r| rejected_failure(r, candidate.link, candidate.download_id));
    checks.push(match rejected {
        Some(description) => ReleaseCheck::failed(ReleaseCheckId::Rejected, description),
        None => ReleaseCheck::passed(ReleaseCheckId::Rejected),
    });

    // Episode — only present when the search was scoped to an episode.
    if let Some(scope) = ctx.episode_scope {
        checks.push(
            match episode_failure(
                &scope.meta,
                candidate.title,
                &scope.source_episodes,
                &scope.label,
            ) {
                Some(description) => ReleaseCheck::failed(ReleaseCheckId::Episode, description),
                None => ReleaseCheck::passed(ReleaseCheckId::Episode),
            },
        );
    }

    // Profile — quality, minimum score, and (when replacing) upgrade target.
    if ctx.mapping.is_some() {
        let mut failures: Vec<String> = Vec::new();

        if let Some(description) = quality_failure(
            candidate.title,
            ctx.quality_profile_name,
            ctx.quality_profiles,
            ctx.qualities,
        ) {
            failures.push(description);
        }

        if let Some(scoring) = ctx.merged_scoring
            && let Some(description) = score_failure(candidate.score, scoring.min_score)
        {
            failures.push(description);
        }

        // The upgrade gate only applies to a single-episode release that scores
        // above the episode's current download (SSoT: auto-search's
        // `is_upgrade_for_any`).
        let replaces = !candidate.is_season_pack
            && ctx.episode_scope.is_some_and(|s| {
                s.current_score
                    .is_some_and(|cur| cur > 0 && candidate.score > cur)
            });
        if replaces
            && let Some(mapping) = ctx.mapping
            && let Some(description) = upgrade_failure(
                mapping,
                candidate.title,
                ctx.quality_profiles,
                ctx.qualities,
            )
        {
            failures.push(description);
        }

        checks.push(if failures.is_empty() {
            ReleaseCheck::passed(ReleaseCheckId::Profile)
        } else {
            ReleaseCheck::failed(ReleaseCheckId::Profile, failures.join("; "))
        });
    }

    checks
}

/// Human-readable time until `expires_at`, e.g. `47h 12m`.
fn remaining_time(expires_at: chrono::NaiveDateTime) -> String {
    let seconds = (expires_at - chrono::Utc::now().naive_utc()).num_seconds();
    if seconds <= 0 {
        return "less than a minute".to_string();
    }
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    if hours > 0 {
        format!("{hours}h {minutes}m")
    } else {
        format!("{minutes}m")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::autoresolve::Rejection;

    fn quality(name: &str, tags: &[&str]) -> Quality {
        Quality {
            name: name.to_string(),
            tags: tags.iter().map(|t| t.to_string()).collect(),
        }
    }

    /// A profile whose only quality is `1080p`, with no upgrade-only restriction.
    fn hd_profile() -> (HashMap<String, QualityProfile>, HashMap<String, Quality>) {
        let mut qualities = HashMap::new();
        qualities.insert("1080p".to_string(), quality("1080p", &["1080P"]));
        qualities.insert("720p".to_string(), quality("720p", &["720P"]));
        let mut profiles = HashMap::new();
        profiles.insert(
            "HD".to_string(),
            QualityProfile {
                name: "HD".to_string(),
                qualities: vec!["1080p".to_string()],
                upgrade_only_qualities: Vec::new(),
            },
        );
        (profiles, qualities)
    }

    fn meta(requires_season: bool) -> SeasonSearchMeta {
        SeasonSearchMeta {
            search_format: "S${season:02}E${episode:02}".to_string(),
            search_season_num: 1,
            episode_offset: 0,
            requires_season,
        }
    }

    #[test]
    fn quality_failure_only_fires_when_a_profile_rejects_the_title() {
        let (profiles, qualities) = hd_profile();
        // A profile quality matches.
        assert!(quality_failure("Show 1080p", Some("HD"), &profiles, &qualities).is_none());
        // A non-profile quality is rejected, naming the offending quality.
        let reason = quality_failure("Show 720p", Some("HD"), &profiles, &qualities)
            .expect("720p is not in the HD profile");
        assert!(reason.contains("720p"), "reason was: {reason}");
        // No profile (or a blank one) never rejects.
        assert!(quality_failure("Show 720p", None, &profiles, &qualities).is_none());
        assert!(quality_failure("Show 720p", Some(""), &profiles, &qualities).is_none());
    }

    #[test]
    fn score_failure_fires_below_the_minimum() {
        assert!(score_failure(100, 100).is_none());
        assert!(score_failure(99, 100).is_some());
    }

    #[test]
    fn episode_failure_uses_the_season_scoped_match() {
        let meta = meta(true);
        assert!(episode_failure(&meta, "Show S01E02 1080p", &[2], "S01E02").is_none());
        let reason = episode_failure(&meta, "Show S01E05 1080p", &[2], "S01E02")
            .expect("E05 does not cover E02");
        assert!(reason.contains("S01E02"), "reason was: {reason}");
    }

    #[test]
    fn rejected_failure_reports_reason_and_remaining_time() {
        let mut rejected = RejectedDownloads::default();
        rejected.insert(
            "magnet:a",
            Some("hash-a"),
            Rejection {
                reason: "No progress after 30 min".to_string(),
                expires_at: chrono::Utc::now().naive_utc() + chrono::Duration::hours(48),
            },
        );

        let by_link = rejected_failure(&rejected, Some("magnet:a"), None).unwrap();
        assert!(by_link.contains("No progress after 30 min"), "{by_link}");
        assert!(by_link.contains("clears in"), "{by_link}");
        // The download id resolves to the same rejection.
        assert!(rejected_failure(&rejected, None, Some("hash-a")).is_some());
        // Unrelated identifiers are not rejected.
        assert!(rejected_failure(&rejected, Some("magnet:b"), Some("hash-b")).is_none());
        // The cheap boolean gate the auto paths use agrees with it.
        assert!(is_rejected(&rejected, Some("magnet:a"), None));
        assert!(!is_rejected(&rejected, Some("magnet:b"), Some("hash-b")));
    }

    #[test]
    fn evaluate_orders_checks_and_omits_episode_at_series_level() {
        let (profiles, qualities) = hd_profile();
        let mapping = MappingRule::default();
        let rejected = RejectedDownloads::default();
        let ctx = ReleaseCheckContext {
            mapping: Some(&mapping),
            merged_scoring: None,
            quality_profile_name: None,
            quality_profiles: &profiles,
            qualities: &qualities,
            rejected: Some(&rejected),
            episode_scope: None,
        };
        let candidate = ReleaseCandidate {
            title: "Show S01E01 1080p",
            link: Some("magnet:a"),
            download_id: None,
            score: 50,
            is_season_pack: false,
        };

        let checks = evaluate(&candidate, &ctx);
        let ids: Vec<ReleaseCheckId> = checks.iter().map(|c| c.id).collect();
        assert_eq!(ids, vec![ReleaseCheckId::Rejected, ReleaseCheckId::Profile]);
        assert!(checks.iter().all(|c| c.passed && c.description.is_empty()));
        assert_eq!(checks[1].label, "Profile", "labels come from the id");
    }

    #[test]
    fn evaluate_reports_failures_with_descriptions_in_order() {
        let (profiles, qualities) = hd_profile();
        let mapping = MappingRule::default();
        let mut rejected = RejectedDownloads::default();
        rejected.insert(
            "magnet:a",
            None,
            Rejection {
                reason: "No progress after 30 min".to_string(),
                expires_at: chrono::Utc::now().naive_utc() + chrono::Duration::hours(1),
            },
        );
        let scope = EpisodeScope {
            meta: meta(true),
            source_episodes: vec![2],
            label: "S01E02".to_string(),
            current_score: None,
        };
        let context = ReleaseCheckContext {
            mapping: Some(&mapping),
            merged_scoring: None,
            quality_profile_name: Some("HD"),
            quality_profiles: &profiles,
            qualities: &qualities,
            rejected: Some(&rejected),
            episode_scope: Some(&scope),
        };
        let candidate = ReleaseCandidate {
            // Rejected, wrong episode, and a quality outside the profile.
            title: "Show S01E05 720p",
            link: Some("magnet:a"),
            download_id: None,
            score: 0,
            is_season_pack: false,
        };

        let checks = evaluate(&candidate, &context);
        let ids: Vec<ReleaseCheckId> = checks.iter().map(|c| c.id).collect();
        assert_eq!(
            ids,
            vec![
                ReleaseCheckId::Rejected,
                ReleaseCheckId::Episode,
                ReleaseCheckId::Profile
            ]
        );
        assert!(checks.iter().all(|c| !c.passed));
        assert!(checks[0].description.contains("No progress after 30 min"));
        assert!(checks[1].description.contains("S01E02"));
        assert!(checks[2].description.contains("720p"));
    }
}
