use std::collections::HashMap;

use crate::mapping::MappingRule;
use crate::types::{Quality, QualityProfile};

/// Detect the quality of a release by matching its title against configured quality profiles.
///
/// Returns the UUID of the matched quality, `"None"` when no quality profile is specified,
/// or `"Unknown"` when no quality within the profile matches the title.
pub fn detect_quality(
    title: &str,
    quality_profiles: &HashMap<String, QualityProfile>,
    qualities: &HashMap<String, Quality>,
    quality_profile_name: Option<&str>,
) -> String {
    let Some(profile_name) = quality_profile_name else {
        return "None".to_string();
    };

    let title_upper = title.to_uppercase();

    let Some(qp) = quality_profiles.get(profile_name) else {
        return "Unknown".to_string();
    };

    for q_ref in &qp.qualities {
        if let Some(quality) = qualities.get(q_ref) {
            if quality.tags.is_empty() {
                return q_ref.clone();
            }

            for tag in &quality.tags {
                if title_upper.contains(&tag.to_uppercase()) {
                    return q_ref.clone();
                }
            }
        }
    }

    "Unknown".to_string()
}

/// SSoT: whether a download's quality is blocked by the profile's
/// `upgrade_only_qualities` restriction.
///
/// Empty `upgrade_only_qualities` means any profile quality is a valid upgrade
/// target (not blocked). Otherwise only the listed qualities count, and a download
/// whose quality is not in the list (or cannot be detected) is blocked. Shared by
/// `select_winners` (RSS auto-download) and `organize_completed` (post-download).
pub fn is_upgrade_blocked_by_profile(
    mapping: &MappingRule,
    media_name: &str,
    quality_profiles: &HashMap<String, QualityProfile>,
    qualities: &HashMap<String, Quality>,
) -> bool {
    let Some(qp_name) = mapping.quality_profile.as_deref() else {
        return false;
    };

    let Some(qp) = quality_profiles.get(qp_name) else {
        return false;
    };

    if qp.upgrade_only_qualities.is_empty() {
        return false;
    }

    let download_quality = detect_quality(media_name, quality_profiles, qualities, Some(qp_name));

    if download_quality == "None" || download_quality == "Unknown" {
        return true;
    }

    !qp.is_upgrade_only(&download_quality)
}

/// Detect a release's quality by checking its title against ALL known qualities.
///
/// Returns `Some(&str)` — the UUID key of the matched quality — or `None` if no
/// configured quality's tags appear in the title.  Callers that need the display
/// name can look it up from the `qualities` map using the UUID.
pub fn detect_quality_any<'a>(
    title: &str,
    qualities: &'a HashMap<String, Quality>,
) -> Option<&'a String> {
    let title_upper = title.to_uppercase();

    for (uuid, quality) in qualities {
        for tag in &quality.tags {
            if title_upper.contains(&tag.to_uppercase()) {
                return Some(uuid);
            }
        }
    }

    None
}

/// SSoT: whether a release title passes the series' configured quality profile.
///
/// Only the qualities **in the profile** are checked against the title, never all
/// registered qualities — this avoids flaky rejections when a title matches tags
/// from multiple qualities (e.g. "1080p" and the "web" inside "WEB-DL").
///
/// Returns `true` when no profile is configured (`None`/empty), or the profile
/// exists and at least one of its qualities has a matching tag; `false` otherwise.
pub fn passes_quality_filter(
    title: &str,
    quality_profile_name: Option<&str>,
    quality_profiles: &HashMap<String, QualityProfile>,
    qualities: &HashMap<String, Quality>,
) -> bool {
    let Some(qp_name) = quality_profile_name else {
        return true;
    };
    if qp_name.is_empty() {
        return true;
    }

    let Some(qp) = quality_profiles.get(qp_name) else {
        return true;
    };

    let title_upper = title.to_uppercase();

    for q_ref in &qp.qualities {
        if let Some(quality) = qualities.get(q_ref)
            && (quality.tags.is_empty()
                || quality
                    .tags
                    .iter()
                    .any(|tag| title_upper.contains(&tag.to_uppercase())))
        {
            return true;
        }
    }

    false
}

/// Strip version markers from a release title so that two releases differing
/// only by version number produce the same base string.
///
/// SSoT: This delegates to [`crate::patterns::STRIP_VERSION`], which is defined
/// adjacent to [`crate::patterns::VERSION`] so both stay in sync.
pub fn strip_version_markers(title: &str) -> String {
    crate::patterns::STRIP_VERSION
        .replace_all(title, "")
        .to_string()
}

/// The identifying fields of one release that [`should_upgrade`] compares: its
/// score, submitter, version, and release title. Bundling them into a struct
/// keeps the comparison signature small and the two sides symmetric.
#[derive(Debug, Clone, Copy)]
pub struct ReleaseVersion<'a> {
    pub score: i32,
    pub submitter: Option<&'a str>,
    pub version: i32,
    pub release_title: Option<&'a str>,
}

/// Determine whether a new release should upgrade an existing episode.
///
/// A version bump by the same submitter (same base title after stripping version
/// markers) always triggers an upgrade — it's assumed to be a correction.
/// Otherwise, the score comparison decides.
pub fn should_upgrade(
    current: ReleaseVersion<'_>,
    candidate: ReleaseVersion<'_>,
    min_score: i32,
) -> bool {
    if candidate.score < min_score {
        return false;
    }

    // Same-submitter version bump of the same base release — assumed correction
    if let (Some(cur_sub), Some(cur_title)) = (current.submitter, current.release_title)
        && cur_sub == candidate.submitter.unwrap_or("")
        && candidate.version > current.version
        && strip_version_markers(cur_title)
            == strip_version_markers(candidate.release_title.unwrap_or(""))
    {
        return true;
    }

    // Otherwise score decides
    candidate.score > current.score
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `ReleaseVersion` for the incumbent episode.
    fn current<'a>(
        score: i32,
        submitter: Option<&'a str>,
        version: i32,
        release_title: Option<&'a str>,
    ) -> ReleaseVersion<'a> {
        ReleaseVersion {
            score,
            submitter,
            version,
            release_title,
        }
    }

    /// `ReleaseVersion` for the candidate release.
    fn candidate<'a>(
        score: i32,
        submitter: Option<&'a str>,
        version: i32,
        release_title: &'a str,
    ) -> ReleaseVersion<'a> {
        ReleaseVersion {
            score,
            submitter,
            version,
            release_title: Some(release_title),
        }
    }

    fn sample_qualities() -> HashMap<String, Quality> {
        let mut q = HashMap::new();
        q.insert(
            "qual-720p-uuid".into(),
            Quality {
                name: "720p".into(),
                tags: vec!["720p".into(), "720".into()],
            },
        );
        q.insert(
            "qual-1080p-uuid".into(),
            Quality {
                name: "1080p".into(),
                tags: vec!["1080p".into(), "1080".into()],
            },
        );
        q
    }

    fn profiles_with_distinct_id_and_name() -> HashMap<String, QualityProfile> {
        let mut p = HashMap::new();
        p.insert(
            "profile-hd-uuid-00001".into(),
            QualityProfile {
                name: "High Definition".into(),
                qualities: vec!["qual-720p-uuid".into(), "qual-1080p-uuid".into()],
                upgrade_only_qualities: vec![],
            },
        );
        p
    }

    #[test]
    fn test_detect_quality_by_uuid() {
        let profiles = profiles_with_distinct_id_and_name();
        let qualities = sample_qualities();

        let result = detect_quality(
            "My.Show.1080p.mkv",
            &profiles,
            &qualities,
            Some("profile-hd-uuid-00001"),
        );
        assert_eq!(result, "qual-1080p-uuid");
    }

    #[test]
    fn test_detect_quality_none_profile() {
        let profiles = profiles_with_distinct_id_and_name();
        let qualities = sample_qualities();

        let result = detect_quality("My.Show.1080p.mkv", &profiles, &qualities, None);
        assert_eq!(result, "None");
    }

    #[test]
    fn test_detect_quality_nonexistent_profile() {
        let profiles = profiles_with_distinct_id_and_name();
        let qualities = sample_qualities();

        let result = detect_quality(
            "My.Show.1080p.mkv",
            &profiles,
            &qualities,
            Some("Bogus Profile"),
        );
        assert_eq!(result, "Unknown");
    }

    #[test]
    fn test_detect_quality_no_match_respects_quality_order() {
        let mut profiles = HashMap::new();
        profiles.insert(
            "profile-ordered".into(),
            QualityProfile {
                name: "Ordered".into(),
                qualities: vec!["q-1080p".into(), "q-720p".into()],
                upgrade_only_qualities: vec![],
            },
        );
        let mut qualities = HashMap::new();
        qualities.insert(
            "q-720p".into(),
            Quality {
                name: "720p".into(),
                tags: vec!["720".into()],
            },
        );
        qualities.insert(
            "q-1080p".into(),
            Quality {
                name: "1080p".into(),
                tags: vec!["1080".into()],
            },
        );

        let result = detect_quality(
            "Show.1080p.720p.HDTV.mkv",
            &profiles,
            &qualities,
            Some("profile-ordered"),
        );
        assert_eq!(result, "q-1080p");
    }

    // passes_quality_filter tests

    fn filter_qualities() -> HashMap<String, Quality> {
        let mut q = HashMap::new();
        q.insert(
            "q-720p".into(),
            Quality {
                name: "720p".into(),
                tags: vec!["720p".into(), "720".into()],
            },
        );
        q.insert(
            "q-1080p".into(),
            Quality {
                name: "1080p".into(),
                tags: vec!["1080p".into(), "1080".into()],
            },
        );
        q.insert(
            "q-4k".into(),
            Quality {
                name: "4K".into(),
                tags: vec!["2160p".into(), "4k".into()],
            },
        );
        q
    }

    fn hd_profile() -> HashMap<String, QualityProfile> {
        let mut p = HashMap::new();
        p.insert(
            "profile-hd".into(),
            QualityProfile {
                name: "HD".into(),
                qualities: vec!["q-720p".into(), "q-1080p".into()],
                upgrade_only_qualities: vec![],
            },
        );
        p
    }

    #[test]
    fn passes_no_profile_configured() {
        let qual = filter_qualities();
        let prof = HashMap::new();
        assert!(passes_quality_filter("Show.1080p.mkv", None, &prof, &qual));
        assert!(passes_quality_filter(
            "Show.1080p.mkv",
            Some(""),
            &prof,
            &qual
        ));
    }

    #[test]
    fn passes_quality_in_profile() {
        let qual = filter_qualities();
        let prof = hd_profile();
        assert!(passes_quality_filter(
            "Show.1080p.mkv",
            Some("profile-hd"),
            &prof,
            &qual
        ));
        assert!(passes_quality_filter(
            "Show.720p.mkv",
            Some("profile-hd"),
            &prof,
            &qual
        ));
    }

    #[test]
    fn rejects_quality_not_in_profile() {
        let qual = filter_qualities();
        let prof = hd_profile();
        assert!(!passes_quality_filter(
            "Show.2160p.mkv",
            Some("profile-hd"),
            &prof,
            &qual
        ));
        assert!(!passes_quality_filter(
            "Show.4k.mkv",
            Some("profile-hd"),
            &prof,
            &qual
        ));
    }

    #[test]
    fn rejects_unknown_quality() {
        let qual = filter_qualities();
        let prof = hd_profile();
        assert!(!passes_quality_filter(
            "Show.UnknownFormat.mkv",
            Some("profile-hd"),
            &prof,
            &qual
        ));
    }

    #[test]
    fn passes_nonexistent_profile() {
        let qual = filter_qualities();
        let prof = HashMap::new();
        assert!(passes_quality_filter(
            "Show.1080p.mkv",
            Some("nonexistent"),
            &prof,
            &qual
        ));
    }

    #[test]
    fn detect_quality_skips_stale_uuid_in_qualities_list() {
        let mut profiles = HashMap::new();
        profiles.insert(
            "profile".into(),
            QualityProfile {
                name: "Test".into(),
                qualities: vec!["stale-deleted-uuid".into(), "qual-1080p-uuid".into()],
                upgrade_only_qualities: vec![],
            },
        );
        let qualities = sample_qualities();

        let result = detect_quality("Show.1080p.mkv", &profiles, &qualities, Some("profile"));
        assert_eq!(result, "qual-1080p-uuid");
    }

    #[test]
    fn detect_quality_all_stale_uuids_returns_unknown() {
        let mut profiles = HashMap::new();
        profiles.insert(
            "profile".into(),
            QualityProfile {
                name: "Test".into(),
                qualities: vec!["stale-uuid-one".into(), "stale-uuid-two".into()],
                upgrade_only_qualities: vec![],
            },
        );
        let qualities = sample_qualities();

        let result = detect_quality("Show.1080p.mkv", &profiles, &qualities, Some("profile"));
        assert_eq!(result, "Unknown");
    }

    // UUID-based quality references (production default pattern)

    fn uuid_qualities() -> HashMap<String, Quality> {
        let mut q = HashMap::new();
        q.insert(
            "quality-4k-uuid-00000000000".into(),
            Quality {
                name: "4k".into(),
                tags: vec!["4k".into(), "2160p".into(), "2160".into()],
            },
        );
        q.insert(
            "quality-1080p-uuid-00000000".into(),
            Quality {
                name: "1080p".into(),
                tags: vec![
                    "1080p".into(),
                    "1920x1080".into(),
                    "1080".into(),
                    "fhd".into(),
                ],
            },
        );
        q.insert(
            "quality-720p-uuid-000000000".into(),
            Quality {
                name: "720p".into(),
                tags: vec!["720p".into(), "1280x720".into(), "720".into(), "hd".into()],
            },
        );
        q
    }

    fn uuid_profile() -> HashMap<String, QualityProfile> {
        let mut p = HashMap::new();
        p.insert(
            "high-def-profile-uuid-000000".into(),
            QualityProfile {
                name: "High Definition".into(),
                qualities: vec![
                    "quality-4k-uuid-00000000000".into(),
                    "quality-1080p-uuid-00000000".into(),
                    "quality-720p-uuid-000000000".into(),
                ],
                upgrade_only_qualities: vec![],
            },
        );
        p
    }

    #[test]
    fn detect_quality_with_uuid_references() {
        let profiles = uuid_profile();
        let qualities = uuid_qualities();

        let result = detect_quality(
            "[Unfucked] Gals Can't Be Kind to Otaku!? - S01E09 (1080p CR WEB-DL AVC AAC 2.0)",
            &profiles,
            &qualities,
            Some("high-def-profile-uuid-000000"),
        );
        assert_eq!(result, "quality-1080p-uuid-00000000");
    }

    #[test]
    fn passes_quality_filter_with_uuid_references() {
        let profiles = uuid_profile();
        let qualities = uuid_qualities();

        assert!(passes_quality_filter(
            "[Unfucked] Gals Can't Be Kind to Otaku!? - S01E09 (1080p CR WEB-DL AVC AAC 2.0)",
            Some("high-def-profile-uuid-000000"),
            &profiles,
            &qualities
        ));

        assert!(passes_quality_filter(
            "Show.2160p.mkv",
            Some("high-def-profile-uuid-000000"),
            &profiles,
            &qualities
        ));
    }

    #[test]
    fn rejects_quality_not_in_uuid_profile() {
        let profiles = uuid_profile();
        let qualities = {
            let mut q = uuid_qualities();
            q.insert(
                "quality-sd-uuid-00000000000".into(),
                Quality {
                    name: "SD".into(),
                    tags: vec!["480p".into(), "480".into(), "sd".into()],
                },
            );
            q
        };

        assert!(!passes_quality_filter(
            "Show.480p.mkv",
            Some("high-def-profile-uuid-000000"),
            &profiles,
            &qualities
        ));
    }

    #[test]
    fn passes_when_any_matched_quality_is_in_profile() {
        // Regression test: a release matching BOTH a non-profile quality (e.g.
        // WEB via the "web" tag in "WEB-DL") AND a profile quality (1080p)
        // should pass — the old "first match wins" approach was order-dependent.
        let mut qualities = uuid_qualities();
        qualities.insert(
            "quality-web-uuid-00000000000".into(),
            Quality {
                name: "WEB".into(),
                tags: vec!["web-rip".into(), "web".into()],
            },
        );
        let profiles = uuid_profile();

        assert!(passes_quality_filter(
            "[MockGroup] The Time Machine S04E09 1080p CR WEB-DL DUAL AAC2.0 H.264",
            Some("high-def-profile-uuid-000000"),
            &profiles,
            &qualities
        ));
    }

    // is_upgrade_blocked_by_profile tests

    fn make_upgrade_mapping(profile_name: Option<&str>) -> crate::mapping::MappingRule {
        crate::mapping::MappingRule {
            quality_profile: profile_name.map(|s| s.to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn upgrade_blocked_no_profile() {
        // No quality profile on mapping → not blocked
        let profiles = HashMap::new();
        let qualities = HashMap::new();
        let mapping = make_upgrade_mapping(None);
        assert!(!is_upgrade_blocked_by_profile(
            &mapping,
            "Show.1080p.mkv",
            &profiles,
            &qualities
        ));
    }

    #[test]
    fn upgrade_blocked_nonexistent_profile() {
        // Profile name doesn't exist in DB → not blocked
        let profiles = HashMap::new();
        let qualities = HashMap::new();
        let mapping = make_upgrade_mapping(Some("nonexistent"));
        assert!(!is_upgrade_blocked_by_profile(
            &mapping,
            "Show.1080p.mkv",
            &profiles,
            &qualities
        ));
    }

    #[test]
    fn upgrade_blocked_empty_upgrade_only() {
        // Empty upgrade_only_qualities → any quality in profile is valid
        let profiles = uuid_profile(); // profile has empty upgrade_only
        let qualities = uuid_qualities();
        let mapping = make_upgrade_mapping(Some("high-def-profile-uuid-000000"));
        // 1080p is in the profile → not blocked
        assert!(!is_upgrade_blocked_by_profile(
            &mapping,
            "Show.1080p.mkv",
            &profiles,
            &qualities
        ));
        // 720p is in the profile → not blocked
        assert!(!is_upgrade_blocked_by_profile(
            &mapping,
            "Show.720p.mkv",
            &profiles,
            &qualities
        ));
    }

    #[test]
    fn upgrade_blocked_match_in_upgrade_only() {
        // Quality is in upgrade_only_qualities → not blocked (valid upgrade target)
        let profiles = profile_with_upgrade_only();
        let qualities = shared_qualities();
        let mapping = make_upgrade_mapping(Some("profile-with-uo"));
        // 1080p is upgrade-only → not blocked
        assert!(!is_upgrade_blocked_by_profile(
            &mapping,
            "Show.1080p.mkv",
            &profiles,
            &qualities
        ));
    }

    #[test]
    fn upgrade_blocked_match_not_in_upgrade_only() {
        // Quality is in profile but NOT in upgrade_only → blocked
        let profiles = profile_with_upgrade_only();
        let qualities = shared_qualities();
        let mapping = make_upgrade_mapping(Some("profile-with-uo"));
        // 720p is in profile but NOT upgrade-only → blocked
        assert!(is_upgrade_blocked_by_profile(
            &mapping,
            "Show.720p.mkv",
            &profiles,
            &qualities
        ));
    }

    #[test]
    fn upgrade_blocked_unknown_quality() {
        // Can't detect quality → blocked (safe default)
        let profiles = profile_with_upgrade_only();
        let qualities = shared_qualities();
        let mapping = make_upgrade_mapping(Some("profile-with-uo"));
        // Unknown format → blocked
        assert!(is_upgrade_blocked_by_profile(
            &mapping,
            "Show.UnknownFormat.mkv",
            &profiles,
            &qualities
        ));
    }

    fn shared_qualities() -> HashMap<String, Quality> {
        let mut q = HashMap::new();
        q.insert(
            "qual-720p-uuid".into(),
            Quality {
                name: "720p".into(),
                tags: vec!["720p".into()],
            },
        );
        q.insert(
            "qual-1080p-uuid".into(),
            Quality {
                name: "1080p".into(),
                tags: vec!["1080p".into()],
            },
        );
        q
    }

    #[test]
    fn upgrade_blocked_empty_profile_name_string() {
        // Empty string profile name → profile likely won't be found → not blocked
        let profiles = profile_with_upgrade_only();
        let qualities = shared_qualities();
        let mapping = make_upgrade_mapping(Some(""));
        assert!(!is_upgrade_blocked_by_profile(
            &mapping,
            "Show.1080p.mkv",
            &profiles,
            &qualities
        ));
    }

    #[test]
    fn upgrade_blocked_all_qualities_are_upgrade_only() {
        // All qualities in the profile are upgrade-only → all are valid upgrade targets
        let profiles = profile_all_upgrade_only();
        let qualities = shared_qualities();
        let mapping = make_upgrade_mapping(Some("profile-all-uo"));
        // 720p is upgrade-only → not blocked
        assert!(!is_upgrade_blocked_by_profile(
            &mapping,
            "Show.720p.mkv",
            &profiles,
            &qualities
        ));
        // 1080p is upgrade-only → not blocked
        assert!(!is_upgrade_blocked_by_profile(
            &mapping,
            "Show.1080p.mkv",
            &profiles,
            &qualities
        ));
    }

    #[test]
    fn upgrade_blocked_empty_qualities_list_not_blocked() {
        // Profile exists with no qualities AND no upgrade_only → not blocked.
        // The early-return for empty upgrade_only_qualities fires first.
        let mut profiles = HashMap::new();
        profiles.insert(
            "profile-empty-qualities".into(),
            QualityProfile {
                name: "Empty".into(),
                qualities: vec![],
                upgrade_only_qualities: vec![],
            },
        );
        let qualities = shared_qualities();
        let mapping = make_upgrade_mapping(Some("profile-empty-qualities"));
        assert!(!is_upgrade_blocked_by_profile(
            &mapping,
            "Show.1080p.mkv",
            &profiles,
            &qualities
        ));
    }

    #[test]
    fn upgrade_blocked_unknown_quality_with_upgrade_only() {
        // Profile has upgrade_only set but quality can't be detected → blocked.
        // This tests the path where upgrade_only_qualities is non-empty
        // AND detect_quality returns "Unknown".
        let mut profiles = HashMap::new();
        profiles.insert(
            "profile-uo-unknown".into(),
            QualityProfile {
                name: "UO Unknown".into(),
                qualities: vec!["qual-1080p-uuid".into()],
                upgrade_only_qualities: vec!["qual-1080p-uuid".into()],
            },
        );
        let qualities = shared_qualities();
        let mapping = make_upgrade_mapping(Some("profile-uo-unknown"));
        // "UnknownFormat" doesn't match 1080p → "Unknown" → blocked
        assert!(is_upgrade_blocked_by_profile(
            &mapping,
            "Show.UnknownFormat.mkv",
            &profiles,
            &qualities
        ));
    }

    fn profile_with_upgrade_only() -> HashMap<String, QualityProfile> {
        let mut p = HashMap::new();
        p.insert(
            "profile-with-uo".into(),
            QualityProfile {
                name: "Profile With UO".into(),
                qualities: vec!["qual-720p-uuid".into(), "qual-1080p-uuid".into()],
                upgrade_only_qualities: vec!["qual-1080p-uuid".into()],
            },
        );
        p
    }

    fn profile_all_upgrade_only() -> HashMap<String, QualityProfile> {
        let mut p = HashMap::new();
        p.insert(
            "profile-all-uo".into(),
            QualityProfile {
                name: "All UO".into(),
                qualities: vec!["qual-720p-uuid".into(), "qual-1080p-uuid".into()],
                upgrade_only_qualities: vec!["qual-720p-uuid".into(), "qual-1080p-uuid".into()],
            },
        );
        p
    }

    // strip_version_markers tests

    #[test]
    fn test_strip_version_bracketed() {
        assert_eq!(
            strip_version_markers("[MockFansub] Show - S01E01 [v2] [1080p].mkv"),
            "[MockFansub] Show - S01E01 [1080p].mkv"
        );
    }

    #[test]
    fn test_strip_version_parenthesized() {
        assert_eq!(
            strip_version_markers("[MockFansub] Show - S01E01 (v3) [1080p].mkv"),
            "[MockFansub] Show - S01E01 [1080p].mkv"
        );
    }

    #[test]
    fn test_strip_version_dot_prefix() {
        assert_eq!(
            strip_version_markers("Show.S01E01.v4.1080p.mkv"),
            "Show.S01E01.1080p.mkv"
        );
    }

    #[test]
    fn test_strip_version_space_prefix() {
        assert_eq!(
            strip_version_markers("Show S01E01 v5 [1080p].mkv"),
            "Show S01E01 [1080p].mkv"
        );
    }

    #[test]
    fn test_strip_version_no_marker() {
        // No version marker → unchanged
        assert_eq!(
            strip_version_markers("[MockFansub] Show - S01E01 [1080p].mkv"),
            "[MockFansub] Show - S01E01 [1080p].mkv"
        );
    }

    #[test]
    fn test_strip_version_does_not_strip_quality_tags() {
        // "v" inside "1080p" or "4k" should NOT be stripped
        assert_eq!(
            strip_version_markers("[Group] Show - 01 [1080p].mkv"),
            "[Group] Show - 01 [1080p].mkv"
        );
        assert_eq!(
            strip_version_markers("Show - 01 HEVC [2160p].mkv"),
            "Show - 01 HEVC [2160p].mkv"
        );
    }

    #[test]
    fn test_strip_version_multiple_markers() {
        // Multiple version markers in the same title (unusual but possible)
        assert_eq!(
            strip_version_markers("[Group] Show S01E01 [v2] [v3].mkv"),
            "[Group] Show S01E01.mkv"
        );
    }

    #[test]
    fn test_strip_version_extended_formats() {
        // Extended formats: v., ver, ver., version, version.
        assert_eq!(
            strip_version_markers("Shironeko - 01 v.2 [1080p].mkv"),
            "Shironeko - 01 [1080p].mkv"
        );
        assert_eq!(strip_version_markers("Show - 01 ver2.mkv"), "Show - 01.mkv");
        assert_eq!(
            strip_version_markers("Show - 01 ver.2.mkv"),
            "Show - 01.mkv"
        );
        assert_eq!(
            strip_version_markers("Show - 01 version2.mkv"),
            "Show - 01.mkv"
        );
        assert_eq!(
            strip_version_markers("Show - 01 version.2.mkv"),
            "Show - 01.mkv"
        );
    }

    #[test]
    fn test_version_and_strip_in_sync() {
        // For every format VERSION recognizes, STRIP_VERSION should cleanly
        // remove the marker so the base title is comparable.
        // This test catches drift between the two patterns.
        let cases = [
            (
                "[MockFansub] Show - S01E01 [v2] [1080p].mkv",
                "[MockFansub] Show - S01E01 [1080p].mkv",
            ),
            (
                "[MockFansub] Show - S01E01 (v3) [1080p].mkv",
                "[MockFansub] Show - S01E01 [1080p].mkv",
            ),
            ("Show.S01E01.v4.1080p.mkv", "Show.S01E01.1080p.mkv"),
            ("Show S01E01 v5 [1080p].mkv", "Show S01E01 [1080p].mkv"),
            ("Show_S01E01_v2.mkv", "Show_S01E01.mkv"),
            ("Show-S01E01-v2.mkv", "Show-S01E01.mkv"),
            ("v2_Show_S01E01.mkv", "_Show_S01E01.mkv"),
        ];
        for (input, expected) in cases {
            let stripped = strip_version_markers(input);
            assert_eq!(
                stripped, expected,
                "strip_version_markers mismatch for '{}'\n  expected: {}\n  got:      {}",
                input, expected, stripped
            );
        }
    }

    // should_upgrade tests

    #[test]
    fn test_same_submitter_higher_version_upgrades() {
        assert!(should_upgrade(
            current(
                700,
                Some("MockFansub"),
                1,
                Some("[MockFansub] Show - S01E01 [1080p].mkv"),
            ),
            candidate(
                700,
                Some("MockFansub"),
                2,
                "[MockFansub] Show - S01E01 [v2] [1080p].mkv",
            ),
            0,
        ));
    }

    #[test]
    fn test_same_submitter_same_version_does_not_upgrade_by_version() {
        // Same version → must rely on score
        assert!(!should_upgrade(
            current(
                800,
                Some("MockFansub"),
                2,
                Some("[MockFansub] Show - S01E01 [v2] [1080p].mkv"),
            ),
            candidate(
                800,
                Some("MockFansub"),
                2,
                "[MockFansub] Show - S01E01 [v2] [1080p].mkv",
            ),
            0,
        ));
    }

    #[test]
    fn test_same_submitter_higher_version_but_different_base() {
        // Same submitter but completely different encode (resolution changed) →
        // stripped titles differ → fall through to score comparison
        assert!(!should_upgrade(
            current(
                700,
                Some("MockFansub"),
                1,
                Some("[MockFansub] Show - S01E01 [1080p].mkv"),
            ),
            candidate(
                700,
                Some("MockFansub"),
                2,
                "[MockFansub] Show - S01E01 [v2] [2160p].mkv",
            ),
            0,
        ));
    }

    #[test]
    fn test_same_submitter_higher_version_but_score_too_low() {
        // Version bump but below min_score → blocked
        assert!(!should_upgrade(
            current(
                700,
                Some("MockFansub"),
                1,
                Some("[MockFansub] Show - S01E01 [1080p].mkv"),
            ),
            candidate(
                700,
                Some("MockFansub"),
                2,
                "[MockFansub] Show - S01E01 [v2] [1080p].mkv",
            ),
            800,
        ));
    }

    #[test]
    fn test_different_submitter_higher_score_upgrades() {
        assert!(should_upgrade(
            current(
                700,
                Some("MockFansub"),
                1,
                Some("[MockFansub] Show - S01E01 [1080p].mkv"),
            ),
            candidate(
                900,
                Some("AnimeRG"),
                1,
                "[AnimeRG] Show - S01E01 [1080p].mkv",
            ),
            0,
        ));
    }

    #[test]
    fn test_different_submitter_lower_score_does_not_upgrade() {
        assert!(!should_upgrade(
            current(
                700,
                Some("MockFansub"),
                1,
                Some("[MockFansub] Show - S01E01 [1080p].mkv"),
            ),
            candidate(
                600,
                Some("AnimeRG"),
                1,
                "[AnimeRG] Show - S01E01 [1080p].mkv",
            ),
            0,
        ));
    }

    #[test]
    fn test_no_current_submitter_uses_score() {
        // Manual/imported: no submitter, no release title → pure score comparison
        assert!(should_upgrade(
            current(0, None, 1, None),
            candidate(
                800,
                Some("MockFansub"),
                1,
                "[MockFansub] Show - S01E01 [1080p].mkv"
            ),
            0,
        ));
    }

    #[test]
    fn test_no_candidate_submitter_uses_score() {
        assert!(should_upgrade(
            current(
                500,
                Some("MockFansub"),
                1,
                Some("[MockFansub] Show - S01E01 [1080p].mkv"),
            ),
            candidate(700, None, 1, "Show - S01E01 [1080p].mkv"),
            0,
        ));
    }

    #[test]
    fn test_version_upgrade_with_no_existing_version_title() {
        // Current has no release_title (legacy/manual) → version check skipped,
        // falls through to score (which is equal) → no upgrade
        assert!(!should_upgrade(
            current(700, Some("MockFansub"), 1, None),
            candidate(
                700,
                Some("MockFansub"),
                2,
                "[MockFansub] Show - S01E01 [v2] [1080p].mkv",
            ),
            0,
        ));
    }

    #[test]
    fn test_prevents_downgrade_to_lower_version() {
        // Current is v3 with score 800, candidate is v2 with score 600
        // → blocked by both version check (v2 < v3) and score check (600 < 800)
        assert!(!should_upgrade(
            current(
                800,
                Some("MockFansub"),
                3,
                Some("[MockFansub] Show - S01E01 [v3] [1080p].mkv"),
            ),
            candidate(
                600,
                Some("MockFansub"),
                2,
                "[MockFansub] Show - S01E01 [v2] [1080p].mkv",
            ),
            0,
        ));
    }

    // Pack version upgrade tests

    #[test]
    fn test_season_pack_version_upgrade() {
        // Season pack v2 from same submitter, same base title → upgrade
        assert!(should_upgrade(
            current(
                700,
                Some("MockFansub"),
                1,
                Some("[MockFansub] Show - Season 01 [1080p].mkv"),
            ),
            candidate(
                700,
                Some("MockFansub"),
                2,
                "[MockFansub] Show - Season 01 [v2] [1080p].mkv",
            ),
            0,
        ));
    }

    #[test]
    fn test_season_pack_version_upgrade_quality_change() {
        // Season pack v2 with different quality tag → stripped titles differ
        // → version check fails, falls through to score (equal) → no upgrade
        assert!(!should_upgrade(
            current(
                700,
                Some("MockFansub"),
                1,
                Some("[MockFansub] Show - Season 01 [1080p].mkv"),
            ),
            candidate(
                700,
                Some("MockFansub"),
                2,
                "[MockFansub] Show - Season 01 [v2] [2160p].mkv",
            ),
            0,
        ));
    }

    #[test]
    fn test_complete_pack_version_upgrade() {
        // Complete series pack v2 from same submitter → upgrade
        assert!(should_upgrade(
            current(
                500,
                Some("MockFansub"),
                1,
                Some("[MockFansub] Show - Complete [1080p].mkv"),
            ),
            candidate(
                500,
                Some("MockFansub"),
                2,
                "[MockFansub] Show - Complete [v2] [1080p].mkv",
            ),
            0,
        ));
    }

    #[test]
    fn test_season_0_pack_version_upgrade() {
        // Season 0 pack v2 from same submitter — should_upgrade doesn't
        // care about season number, only about submitter, version, and
        // stripped title match. The guard lives in select_winners, not here.
        assert!(should_upgrade(
            current(
                500,
                Some("MockFansub"),
                1,
                Some("[MockFansub] Show - Specials [1080p].mkv"),
            ),
            candidate(
                500,
                Some("MockFansub"),
                2,
                "[MockFansub] Show - Specials [v2] [1080p].mkv",
            ),
            0,
        ));
    }
}
