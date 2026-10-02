//! Language variants.
//!
//! Two files are *language variants* of one another when they describe the same
//! episode artifact (same season/episode/part) and differ only by a trailing
//! **language tag** (`.en`, `.eng`, `.jp`, …). Such siblings are meant to be kept
//! alongside the primary file, not to compete with it.
//!
//! Everything else is a genuinely different artifact and therefore a conflict, not
//! a variant: an alternative **version** marker (`v2`, `v3`, `ver3`), a rename
//! **collision counter** (`.001`), or a different quality/codec/group. Only a
//! language tag is peeled — a counter or version marker left in the base keeps the
//! two files apart.
//!
//! [`variant_base`] reduces a filename to that comparable base. Callers MUST only
//! compare files that resolve to the **same assignment slot**: the reduction can
//! strip a token that is also an episode number (e.g. `Show.001.mkv`), so it is not
//! meaningful across different episodes.

use crate::languages::is_language_tag;
use crate::media_format::{is_auxiliary_ext, is_video_ext};

/// Reduce a media filename to its language-variant base (lowercased): the basename
/// with its trailing extension and any trailing language tags removed.
///
/// Quality, codec, group, episode, part and version tokens are preserved, so
/// genuinely different releases (`1080p` vs `720p`, `v1` vs `v2`, `001` vs plain)
/// never collapse together.
pub fn variant_base(filename: &str) -> String {
    let name = filename.rsplit(['/', '\\']).next().unwrap_or(filename);
    let mut stem = strip_media_extension(name);
    // Peel trailing dot-separated language tags (`Show.S01E01.en`, `...jpn`).
    while let Some((rest, token)) = stem.rsplit_once('.') {
        if rest.is_empty() || !is_language_tag(token) {
            break;
        }
        stem = rest;
    }
    stem.trim().to_ascii_lowercase()
}

/// Whether `a` and `b` are language variants of the same file: they share a
/// non-empty base yet are **different names** (one carries a language tag the other
/// does not). Two identical names are the same file, not variants. Only meaningful
/// for files in the same assignment slot.
pub fn is_variant_of(a: &str, b: &str) -> bool {
    let (base_a, base_b) = (variant_base(a), variant_base(b));
    !base_a.is_empty() && base_a == base_b && plain_stem(a) != plain_stem(b)
}

/// Whether `filename` carries a trailing language tag, i.e. it is a *variant*
/// rather than the plain name for its artifact (`Show.S01E01.en.mkv` → `true`,
/// `Show.S01E01.mkv` → `false`). Used to pick the primary file among siblings.
pub fn has_language_tag(filename: &str) -> bool {
    variant_base(filename) != plain_stem(filename)
}

/// The trailing language tag of `filename` (lowercased), or `None` when it carries
/// none. This is the file's **language slot key** within its artifact base.
pub fn language_tag(filename: &str) -> Option<String> {
    let stem = plain_stem(filename);
    let base = variant_base(filename);
    if base.is_empty() || stem == base {
        None
    } else {
        Some(stem[base.len() + 1..].to_string())
    }
}

/// Whether `a` and `b` occupy the **same language slot** of the same artifact — the
/// same base *and* the same language tag. Two files in one slot are duplicates and
/// therefore a conflict; only different tags of the same base are variants that can
/// coexist.
pub fn same_language_slot(a: &str, b: &str) -> bool {
    let (base_a, base_b) = (variant_base(a), variant_base(b));
    !base_a.is_empty() && base_a == base_b && plain_stem(a) == plain_stem(b)
}

/// The lowercased basename with its media extension removed and nothing peeled.
fn plain_stem(filename: &str) -> String {
    let name = filename.rsplit(['/', '\\']).next().unwrap_or(filename);
    strip_media_extension(name).trim().to_ascii_lowercase()
}

fn strip_media_extension(name: &str) -> &str {
    match name.rsplit_once('.') {
        Some((stem, ext)) => {
            let ext = ext.to_ascii_lowercase();
            if is_video_ext(&ext) || is_auxiliary_ext(&ext) {
                stem
            } else {
                name
            }
        }
        None => name,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_tags_are_variants() {
        for pair in [
            ("Show.S01E01.mkv", "Show.S01E01.en.mkv"),
            ("Show.S01E01.mkv", "Show.S01E01.eng.mkv"),
            ("Show.S01E01.mkv", "Show.S01E01.jpn.mkv"),
            ("Show.S01E01E02.mkv", "Show.S01E01E02.spa.mkv"),
            ("Show.S01E01.pt1.mkv", "Show.S01E01.pt1.en.mkv"),
        ] {
            assert!(is_variant_of(pair.0, pair.1), "{pair:?} should be variants");
            assert!(is_variant_of(pair.1, pair.0), "{pair:?} must be symmetric");
        }
    }

    #[test]
    fn version_markers_are_not_variants() {
        for pair in [
            ("Show.S01E01.mkv", "Show.S01E01.v2.mkv"),
            ("Show.S01E01.mkv", "Show.S01E01.ver3.mkv"),
            ("Show.S01E01.mkv", "Show.S01E01.version10.mkv"),
            ("Show S01E01.mkv", "Show S01E01 v2.mkv"),
        ] {
            assert!(
                !is_variant_of(pair.0, pair.1),
                "{pair:?} must not be variants"
            );
        }
    }

    #[test]
    fn collision_counters_are_not_variants() {
        // A counter is a distinct artifact, whether bare or in front of a language tag.
        for pair in [
            ("Show.S01E01.mkv", "Show.S01E01.001.mkv"),
            ("Show.S01E01.mkv", "Show.S01E01.001.en.mkv"),
            ("Show.S01E01.002.mkv", "Show.S01E01.001.mkv"),
        ] {
            assert!(
                !is_variant_of(pair.0, pair.1),
                "{pair:?} must not be variants"
            );
        }
    }

    #[test]
    fn quality_and_release_differences_are_not_variants() {
        for pair in [
            ("Show.S01E01.1080p.mkv", "Show.S01E01.720p.mkv"),
            ("Show.S01E01.WEB-DL.mkv", "Show.S01E01.HDTV.mkv"),
            ("Show.S01E01.mkv", "Show.S01E01.1080p.mkv"),
            ("Show.S01E02.mkv", "Show.S01E03.mkv"),
            ("Show.S01E01.pt1.mkv", "Show.S01E01.pt2.mkv"),
        ] {
            assert!(
                !is_variant_of(pair.0, pair.1),
                "{pair:?} must not be variants"
            );
        }
    }

    #[test]
    fn empty_or_unrelated_names_are_not_variants() {
        assert!(!is_variant_of("", ""));
        assert!(!is_variant_of("Show.mkv", "Other.mkv"));
    }

    #[test]
    fn identical_names_are_the_same_file_not_variants() {
        // A redundant re-download keeps the same name at a different path.
        assert!(!is_variant_of(
            "/library/Show.S01E01.mkv",
            "/downloads/Show.S01E01.mkv"
        ));
        assert!(!is_variant_of(
            "/library/Show.S01E01.en.mkv",
            "/downloads/Show.S01E01.en.mkv"
        ));
        assert!(!is_variant_of("/a/Show.mkv", "/a/Show.mkv"));
    }

    #[test]
    fn language_tag_detection_marks_only_trailing_tags() {
        assert!(has_language_tag("Show.S01E01.en.mkv"));
        assert!(has_language_tag("/lib/Show.S01E01.jpn.mkv"));
        assert!(!has_language_tag("Show.S01E01.mkv"));
        assert!(!has_language_tag("Show.S01E01.1080p.mkv"));
        assert!(!has_language_tag("Show.S01E01.en.1080p.mkv"));
    }

    #[test]
    fn language_tag_is_the_trailing_key() {
        assert_eq!(language_tag("Show.S01E01.mkv"), None);
        assert_eq!(
            language_tag("/lib/Show.S01E01.EN.mkv").as_deref(),
            Some("en")
        );
        assert_eq!(language_tag("Show.S01E01.eng.mkv").as_deref(), Some("eng"));
        // A non-tag token is part of the base, not the language slot.
        assert_eq!(language_tag("Show.S01E01.1080p.mkv"), None);
    }

    #[test]
    fn same_language_slot_is_duplicate_not_variant() {
        // Distinct tags of one artifact coexist...
        assert!(!same_language_slot(
            "Show.S01E01.en.mkv",
            "Show.S01E01.eng.mkv"
        ));
        assert!(!same_language_slot("Show.S01E01.mkv", "Show.S01E01.en.mkv"));
        // ...but two files in one slot (same base and tag) are duplicates.
        assert!(same_language_slot(
            "/lib/Show.S01E01.en.mkv",
            "/dl/Show.S01E01.en.mkv"
        ));
        assert!(same_language_slot(
            "/lib/Show.S01E01.mkv",
            "/dl/Show.S01E01.mkv"
        ));
        assert!(same_language_slot(
            "/lib/Show.S01E01.EN.mkv",
            "/dl/Show.S01E01.en.mkv"
        ));
        // Different artifacts/quality never share a slot.
        assert!(!same_language_slot(
            "Show.S01E01.720p.en.mkv",
            "Show.S01E01.1080p.mkv"
        ));
        assert!(!same_language_slot(
            "Show.S01E01.pt1.en.mkv",
            "Show.S01E01.pt2.en.mkv"
        ));
    }
}
