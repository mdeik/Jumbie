//! Validation helpers for alias and pattern lists.
//!
//! SSoT: `SeasonOverride::validate()` and `SeriesSettings::validate()` iterate
//! aliases and patterns with the same format and error messages.

/// Maximum number of series-level regex patterns allowed.
pub const MAX_SERIES_PATTERNS: usize = 100;
/// Maximum number of season-level regex patterns allowed per season.
pub const MAX_SEASON_PATTERNS: usize = 50;

/// Returns `true` if the slice contains any non-empty (trimmed) string.
///
/// Shared by alias and pattern lists so blank textarea lines don't count.
pub fn has_non_empty(items: &[String]) -> bool {
    items.iter().any(|s| !s.trim().is_empty())
}

/// Returns an iterator over trimmed, non-empty strings, filtering out blank lines.
pub fn non_empty_strs(items: &[String]) -> impl Iterator<Item = &str> {
    items
        .iter()
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.trim())
}

/// Validate a list of aliases, returning an error with the index on failure.
/// No count limit — source plugins decide how many aliases to use.
pub(crate) fn validate_aliases(aliases: &[String], label: &str) -> Result<(), String> {
    for (i, alias) in aliases.iter().enumerate() {
        crate::validation::validate_alias(alias)
            .map_err(|e| format!("{} alias at index {}: {}", label, i, e))?;
    }
    Ok(())
}

/// Validate a search-format template.
///
/// Blank is allowed (the rendered query then carries no season/episode key).
/// Only `${season}` / `${episode}` with optional numeric padding is permitted
/// (SSoT: [`crate::validation::validate_search_template`]), so unknown variables
/// and unused modifiers are rejected rather than silently rendered as literals.
pub(crate) fn validate_search_format(format: Option<&str>, label: &str) -> Result<(), String> {
    let Some(format) = format else {
        return Ok(());
    };
    crate::validation::validate_search_template(format, true)
        .map_err(|e| format!("{} search format: {}", label, e))
}

#[cfg(test)]
mod search_format_tests {
    use super::*;

    #[test]
    fn none_and_blank_are_allowed() {
        assert!(validate_search_format(None, "Series").is_ok());
        assert!(validate_search_format(Some(""), "Series").is_ok());
        assert!(validate_search_format(Some("   "), "Series").is_ok());
    }

    #[test]
    fn season_and_episode_variables_are_allowed() {
        assert!(validate_search_format(Some("S${season:02}E${episode:02}"), "Series").is_ok());
        assert!(validate_search_format(Some("${episode}"), "Series").is_ok());
    }

    #[test]
    fn unknown_variables_are_rejected() {
        let err = validate_search_format(Some("${series} ${episode}"), "Series").unwrap_err();
        assert!(err.contains("series"), "unexpected error: {err}");
    }

    #[test]
    fn unbalanced_braces_are_rejected() {
        let err = validate_search_format(Some("S${season:02"), "Series").unwrap_err();
        assert!(
            err.contains("Unbalanced") || err.contains("brace"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn non_padding_modifiers_are_rejected() {
        for bad in [
            "${episode:-x}",
            "${episode/a/b}",
            "${episode:<3}",
            "?{${episode}}",
            "%{release:%Y}",
            "{episode}",
            "${episode:x}",
        ] {
            assert!(
                validate_search_format(Some(bad), "Series").is_err(),
                "expected rejection: {bad}"
            );
        }
    }
}

/// Validate a list of regex patterns, returning an error with the index on failure.
/// Also enforces the total count limit.
pub(crate) fn validate_patterns(
    patterns: &[String],
    max_count: usize,
    label: &str,
) -> Result<(), String> {
    if patterns.len() > max_count {
        return Err(format!(
            "{} patterns exceed the maximum of {}: found {}",
            label,
            max_count,
            patterns.len()
        ));
    }
    for (i, pattern) in patterns.iter().enumerate() {
        crate::validation::validate_regex(pattern)
            .map_err(|e| format!("Invalid {} pattern at index {}: {}", label, i, e))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_has_non_empty_with_all_empty() {
        // All empty / whitespace-only — should be treated as "no real aliases"
        assert!(!has_non_empty(&[] as &[String]));
        assert!(!has_non_empty(&[String::new()]));
        assert!(!has_non_empty(&["".to_string(), String::new()]));
        // Whitespace-only is also "blank" — trimmed before check
        assert!(!has_non_empty(&["   ".to_string()]));
        assert!(!has_non_empty(&["\t".to_string()]));
    }

    #[test]
    fn test_has_non_empty_with_real_entries() {
        assert!(has_non_empty(&["alias1".to_string()]));
        assert!(has_non_empty(&["alias1".to_string(), String::new()]));
        assert!(has_non_empty(&[String::new(), "alias1".to_string()]));
        assert!(has_non_empty(&["alias1".to_string(), "alias2".to_string()]));
    }

    #[test]
    fn test_non_empty_strs_filters_empties() {
        let items = vec![
            "keep".to_string(),
            String::new(),
            "also-keep".to_string(),
            String::new(),
        ];
        let result: Vec<&str> = non_empty_strs(&items).collect();
        assert_eq!(result, vec!["keep", "also-keep"]);
    }

    #[test]
    fn test_non_empty_strs_trims_whitespace_only() {
        let items = vec!["  alias  ".to_string(), "   ".to_string(), String::new()];
        let result: Vec<&str> = non_empty_strs(&items).collect();
        assert_eq!(result, vec!["alias"]);
    }

    #[test]
    fn test_non_empty_strs_all_empty_yields_empty() {
        let items = vec![String::new(), String::new()];
        let result: Vec<&str> = non_empty_strs(&items).collect();
        assert!(result.is_empty());
    }

    #[test]
    fn test_non_empty_strs_preserves_order() {
        let items = vec![
            String::new(),
            "first".to_string(),
            String::new(),
            "second".to_string(),
            String::new(),
        ];
        let result: Vec<&str> = non_empty_strs(&items).collect();
        assert_eq!(result, vec!["first", "second"]);
    }
}
