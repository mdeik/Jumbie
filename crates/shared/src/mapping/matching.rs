//! Text normalization shared by release and scan matching.
//!
//! Release recognition, file-scan series lookup, and season-alias resolution all
//! compare a candidate against series names/titles/aliases, so the normalization
//! lives here and changes every caller at once.

use crate::parsing::clean_title;

/// Normalize text for matching: lowercase, dots → spaces, trimmed.
pub fn normalize_for_match(text: &str) -> String {
    text.to_lowercase().replace('.', " ").trim().to_string()
}

/// The `clean_title` form of [`normalize_for_match`] — strips bracketed tags,
/// parenthesised groups and quality tokens before normalizing.
pub fn normalize_cleaned_for_match(text: &str) -> String {
    normalize_for_match(&clean_title(text))
}

/// Whether `needle` (a series name, target title, or alias) appears in the
/// candidate under either normalization.
///
/// `candidate_raw` / `candidate_clean` are the candidate's own
/// [`normalize_for_match`] / [`normalize_cleaned_for_match`] results, precomputed
/// once by the caller so a loop over many needles does not re-normalize the
/// candidate each time.
pub fn matches_normalized(candidate_raw: &str, candidate_clean: &str, needle: &str) -> bool {
    let raw = normalize_for_match(needle);
    if !raw.is_empty() && candidate_raw.contains(&raw) {
        return true;
    }
    let clean = normalize_cleaned_for_match(needle);
    !clean.is_empty() && candidate_clean.contains(&clean)
}

crate::test_module! {
    #[test]
    fn normalizes_case_and_dots() {
        assert_eq!(normalize_for_match("My.Show.Name"), "my show name");
        assert_eq!(normalize_for_match("  My Show  "), "my show");
    }

    #[test]
    fn matches_raw_and_cleaned() {
        let raw = normalize_for_match("My Show - 01 [1080p]");
        let clean = normalize_cleaned_for_match("My Show - 01 [1080p]");
        assert!(matches_normalized(&raw, &clean, "My Show"));
        assert!(matches_normalized(&raw, &clean, "My.Show"));
        assert!(!matches_normalized(&raw, &clean, "Other Show"));
    }

    #[test]
    fn empty_needle_never_matches() {
        let raw = normalize_for_match("Anything");
        let clean = normalize_cleaned_for_match("Anything");
        assert!(!matches_normalized(&raw, &clean, ""));
        assert!(!matches_normalized(&raw, &clean, "  "));
    }
}
