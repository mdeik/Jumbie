use wasm_bindgen::JsCast;
use web_sys::HtmlInputElement;

// Re-export shared validation functions so frontend code never calls the shared
// crate directly — call sites route through this module.
pub use jumbie_shared::validation::{
    VALID_UNEXPECTED_FILES_HANDLING, VALID_UNNEEDED_EPISODES_HANDLING, is_valid_port, is_valid_url,
    validate_alias as shared_validate_alias,
    validate_api_key_scopes as shared_validate_api_key_scopes,
    validate_episode_numbers_not_empty as shared_validate_episode_numbers_not_empty,
    validate_media_info_scan_interval as shared_validate_media_info_scan_interval,
    validate_name as shared_validate_name, validate_plugin_info as shared_validate_plugin_info,
    validate_plugin_name as shared_validate_plugin_name,
    validate_search_query as shared_validate_search_query,
    validate_season_pack_replace_threshold as shared_validate_season_pack_threshold,
    validate_tag as shared_validate_tag,
    validate_unexpected_files_handling as shared_validate_unexpected_files_handling,
    validate_unneeded_episodes_handling as shared_validate_unneeded_episodes_handling,
};

macro_rules! make_validation_fn {
    ($name:ident, $ret:ty, $shared_fn:path, $($arg:ident: $arg_ty:ty),+ $(,)?) => {
        pub fn $name($($arg: $arg_ty),+) -> Result<$ret, String> {
            $shared_fn($($arg),+).map_err(|e| e.to_string())
        }
    };
}

make_validation_fn!(validate_not_empty, (), jumbie_shared::validation::validate_not_empty, value: &str, field_name: &str);
make_validation_fn!(validate_title, (), jumbie_shared::validation::validate_title, name: &str);
make_validation_fn!(validate_season_number, i32, jumbie_shared::validation::validate_season_number, season: &str);
make_validation_fn!(validate_episode_number, (), jumbie_shared::validation::validate_episode_number, episode: i32);
make_validation_fn!(validate_episode_offset, (), jumbie_shared::validation::validate_episode_offset, offset: i32);
make_validation_fn!(parse_port, u16, jumbie_shared::validation::parse_port, value: &str);
make_validation_fn!(validate_template, (), jumbie_shared::validation::validate_template, template: &str, allowed_vars: &[&str], allow_empty: bool);
make_validation_fn!(validate_name, (), shared_validate_name, value: &str, field_name: &str, max_len: usize);
make_validation_fn!(validate_tag, (), shared_validate_tag, tag: &str);
make_validation_fn!(validate_alias, (), shared_validate_alias, alias: &str);
make_validation_fn!(validate_search_query, (), shared_validate_search_query, query: &str);
make_validation_fn!(validate_plugin_name, (), shared_validate_plugin_name, name: &str);
make_validation_fn!(validate_media_info_scan_interval, (), shared_validate_media_info_scan_interval, interval: u64);
make_validation_fn!(validate_season_pack_threshold, (), shared_validate_season_pack_threshold, threshold: u32);
make_validation_fn!(validate_unexpected_files_handling, (), shared_validate_unexpected_files_handling, handling: &str);
make_validation_fn!(validate_unneeded_episodes_handling, (), shared_validate_unneeded_episodes_handling, handling: &str);
make_validation_fn!(validate_api_key_scopes, (), shared_validate_api_key_scopes, count: usize);
make_validation_fn!(validate_episode_numbers_not_empty, (), shared_validate_episode_numbers_not_empty, count: usize);

/// Validate a template field and show a **warning** toast on failure.
///
/// Returns `true` if validation failed (caller should `return` early).
/// The toast type and message format live here rather than in each caller.
pub fn validate_template_field(
    val: &str,
    allowed_vars: &[&str],
    allow_empty: bool,
    field_name: &str,
) -> bool {
    if let Err(err) = validate_template(val, allowed_vars, allow_empty) {
        crate::components::common::toast::show_toast(
            format!("Invalid {field_name}: {err}"),
            crate::components::common::toast::NotificationType::Warning,
        );
        true
    } else {
        false
    }
}

/// Validate a search-format field (only `${season}`/`${episode}` + padding) and
/// show a **warning** toast on failure.
///
/// Returns `true` if validation failed (caller should `return` early).
/// SSoT: the restriction itself lives in
/// `jumbie_shared::validation::validate_search_template`.
pub fn validate_search_format_field(val: &str, allow_empty: bool, field_name: &str) -> bool {
    match jumbie_shared::validation::validate_search_template(val, allow_empty) {
        Err(err) => {
            crate::components::common::toast::show_toast(
                format!("Invalid {field_name}: {err}"),
                crate::components::common::toast::NotificationType::Warning,
            );
            true
        }
        Ok(()) => false,
    }
}

/// Help text shared by every Search Format field (global, series, season).
///
/// The template only controls the season/episode key; the title and aliases are
/// added by the search pipeline, so an empty template still searches by title.
pub const SEARCH_FORMAT_FIELD_HELP: &str = "Only shapes the season/episode part of the query; the series title and aliases are added automatically. Leave blank to search by title alone.";

/// Help text for the season-folder format fields (global normal + absolute).
pub const SEASON_FOLDER_FORMAT_FIELD_HELP: &str = "Names the per-season sub-folder inside the series folder. The help button lists the available variables and formatting.";

/// Help text for the episode-file format fields (global normal + absolute).
pub const EPISODE_FILE_FORMAT_FIELD_HELP: &str = "Names episode files, applied only when renaming is enabled. The help button lists the available variables and formatting.";

/// Shared hint for the series-tab naming-formats group (season folder + episode file).
pub const NAMING_FORMATS_FIELD_HELP: &str = "Templates for the season folder and episode file names. Each help button lists the available variables and formatting.";

/// Parse a season input string that supports ranges (`-`) and comma-separated lists (`,`).
///
/// # Format
/// - Single season: `1`
/// - Range (inclusive): `1-5`
/// - Comma-separated combination: `1-3, 5, 7-9`
///
/// Only digits, commas, hyphens, and spaces are allowed.
///
/// Returns a sorted, deduplicated `Vec<i32>` of validated season numbers,
/// or an error describing the problem.
pub fn parse_season_input(input: &str) -> Result<Vec<i32>, String> {
    let input = input.trim();
    if input.is_empty() {
        return Err("Season input is empty".to_string());
    }

    // Only digits, commas, hyphens, and spaces are allowed.
    for c in input.chars() {
        if !c.is_ascii_digit() && c != ',' && c != '-' && c != ' ' {
            return Err(format!(
                "Invalid character '{}'. Only numbers, commas, and hyphens are allowed.",
                c
            ));
        }
    }

    let mut seen = std::collections::BTreeSet::new();

    for part in input.split(',') {
        let part = part.trim();
        if part.is_empty() {
            return Err("Empty segment found (e.g., consecutive commas).".to_string());
        }

        if let Some(dash_pos) = part.find('-') {
            let dash_count = part.chars().filter(|&c| c == '-').count();
            if dash_count != 1 {
                return Err(format!(
                    "Invalid range '{}': use format start-end (single hyphen).",
                    part
                ));
            }

            let start_str = &part[..dash_pos].trim();
            let end_str = &part[dash_pos + 1..].trim();

            if start_str.is_empty() || end_str.is_empty() {
                return Err(format!(
                    "Invalid range '{}': both start and end must be specified.",
                    part
                ));
            }

            let start = validate_season_number(start_str)?;
            let end = validate_season_number(end_str)?;

            if start > end {
                return Err(format!(
                    "Invalid range '{}': start ({}) cannot be greater than end ({}).",
                    part, start, end
                ));
            }

            for s in start..=end {
                seen.insert(s);
            }
        } else {
            let num = validate_season_number(part)?;
            seen.insert(num);
        }
    }

    Ok(seen.into_iter().collect())
}

// Extracts the `.value` from an on:input event: Leptos's on:input typing
// requires this cast to `HtmlInputElement`.
pub fn event_target_value(event: &web_sys::Event) -> String {
    event
        .target()
        .and_then(|t| t.dyn_into::<HtmlInputElement>().ok())
        .map(|input| input.value())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    // `event_target_value` needs a real DOM (`HtmlInputElement`), so it is not
    // unit-tested.

    // Validation function tests are in the shared validation crate
    // (`jumbie_shared::validation`). Only frontend-specific wrapper behavior
    // is tested here.

    #[test]
    fn test_validate_search_query() {
        assert!(validate_search_query("test series").is_ok());
        assert!(validate_search_query("").is_err());
        assert!(validate_search_query("   ").is_err());
        let long: String = "a".repeat(256);
        assert!(validate_search_query(&long).is_err());
    }

    #[test]
    fn test_validate_media_info_scan_interval() {
        assert!(validate_media_info_scan_interval(1).is_ok());
        assert!(validate_media_info_scan_interval(60).is_ok());
        assert!(validate_media_info_scan_interval(0).is_err());
    }

    #[test]
    fn test_validate_season_pack_threshold() {
        assert!(validate_season_pack_threshold(0).is_ok());
        assert!(validate_season_pack_threshold(50).is_ok());
        assert!(validate_season_pack_threshold(100).is_ok());
        assert!(validate_season_pack_threshold(101).is_err());
    }

    #[test]
    fn test_validate_unexpected_files_handling() {
        assert!(validate_unexpected_files_handling("keep").is_ok());
        assert!(validate_unexpected_files_handling("delete").is_ok());
        assert!(validate_unexpected_files_handling("banana").is_err());
    }

    #[test]
    fn test_validate_api_key_scopes() {
        assert!(validate_api_key_scopes(1).is_ok());
        assert!(validate_api_key_scopes(2).is_ok());
        assert!(validate_api_key_scopes(0).is_err());
    }

    #[test]
    fn test_validate_episode_numbers_not_empty() {
        assert!(validate_episode_numbers_not_empty(1).is_ok());
        assert!(validate_episode_numbers_not_empty(0).is_err());
    }

    #[test]
    fn test_valid_unexpected_files_handling_const() {
        assert!(VALID_UNEXPECTED_FILES_HANDLING.contains(&"keep"));
        assert!(VALID_UNEXPECTED_FILES_HANDLING.contains(&"delete"));
        assert_eq!(VALID_UNEXPECTED_FILES_HANDLING.len(), 2);
    }

    #[test]
    fn test_is_valid_url() {
        assert!(is_valid_url("http://example.com"));
        assert!(is_valid_url("https://example.com"));
        assert!(is_valid_url("")); // Empty is OK
        assert!(!is_valid_url("ftp://example.com"));
        assert!(!is_valid_url("not a url"));
    }

    #[test]
    fn test_is_valid_port() {
        assert!(is_valid_port(80));
        assert!(is_valid_port(8080));
        assert!(is_valid_port(65535));
        assert!(!is_valid_port(0));
    }

    #[test]
    fn test_parse_port() {
        assert_eq!(parse_port("8080").unwrap(), 8080);
        assert!(parse_port("0").is_err());
        assert!(parse_port("70000").is_err());
        assert!(parse_port("abc").is_err());
    }

    #[test]
    fn test_validate_template() {
        let vars = ["series", "season", "episode", "unknown_var"];
        assert!(validate_template("${series} - S${season:02}E${episode:02}", &vars, false).is_ok());
        assert!(validate_template("Season ${season}", &vars, false).is_ok());
        assert!(validate_template("", &vars, false).is_err());
        assert!(validate_template("", &vars, true).is_ok());
        assert!(validate_template("${unclosed", &vars, false).is_err());
        assert!(validate_template("unclosed}", &vars, false).is_err());
        assert!(validate_template("${not_in_array}", &vars, false).is_err());
    }

    // --- parse_season_input tests ---

    #[test]
    fn test_parse_single_season() {
        assert_eq!(parse_season_input("1").unwrap(), vec![1]);
        assert_eq!(parse_season_input("0").unwrap(), vec![0]);
        assert_eq!(parse_season_input(" 5 ").unwrap(), vec![5]);
        assert_eq!(parse_season_input("500").unwrap(), vec![500]);
    }

    #[test]
    fn test_parse_season_range() {
        assert_eq!(parse_season_input("1-3").unwrap(), vec![1, 2, 3]);
        assert_eq!(parse_season_input("0-2").unwrap(), vec![0, 1, 2]);
        assert_eq!(parse_season_input(" 1 - 5 ").unwrap(), vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn test_parse_comma_separated() {
        assert_eq!(parse_season_input("1,3,5").unwrap(), vec![1, 3, 5]);
        assert_eq!(parse_season_input(" 1 , 3 , 5 ").unwrap(), vec![1, 3, 5]);
    }

    #[test]
    fn test_parse_combined_range_and_list() {
        assert_eq!(
            parse_season_input("1-3,5,7-9").unwrap(),
            vec![1, 2, 3, 5, 7, 8, 9]
        );
        assert_eq!(
            parse_season_input(" 1-3 , 5 , 7-9 ").unwrap(),
            vec![1, 2, 3, 5, 7, 8, 9]
        );
    }

    #[test]
    fn test_parse_deduplicates() {
        let result = parse_season_input("1-3,2,1,3").unwrap();
        assert_eq!(result, vec![1, 2, 3]);
    }

    #[test]
    fn test_parse_single_value_range() {
        assert_eq!(parse_season_input("3-3").unwrap(), vec![3]);
    }

    #[test]
    fn test_parse_empty_input_fails() {
        assert!(parse_season_input("").is_err());
        assert!(parse_season_input("   ").is_err());
    }

    #[test]
    fn test_parse_invalid_characters_fails() {
        assert!(parse_season_input("a").is_err());
        assert!(parse_season_input("1,a").is_err());
        assert!(parse_season_input("1.5").is_err());
        assert!(parse_season_input("season 1").is_err());
        assert!(parse_season_input("1_2").is_err());
    }

    #[test]
    fn test_parse_consecutive_commas_fails() {
        assert!(parse_season_input("1,,3").is_err());
        assert!(parse_season_input("1,").is_err());
        assert!(parse_season_input(",1").is_err());
    }

    #[test]
    fn test_parse_malformed_range_fails() {
        assert!(parse_season_input("1--3").is_err());
        assert!(parse_season_input("1-").is_err());
        assert!(parse_season_input("-3").is_err());
        assert!(parse_season_input("5-3").is_err());
    }

    #[test]
    fn test_parse_negative_season_fails() {
        assert!(parse_season_input("-1").is_err());
        assert!(parse_season_input("-1-5").is_err());
    }

    #[test]
    fn test_parse_range() {
        // Range expansion logic only (the season limit boundary lives in shared)
        let result = parse_season_input("1-10").unwrap();
        assert_eq!(result.len(), 10);
        assert_eq!(result[0], 1);
        assert_eq!(result[9], 10);
    }

    #[test]
    fn test_parse_multiple_hyphens_fails() {
        // Two non-adjacent hyphens in one segment (distinct from "1--3")
        assert!(parse_season_input("1-3-5").is_err());
        assert!(parse_season_input("1-2-3").is_err());
    }

    #[test]
    fn test_parse_bare_separators_fails() {
        assert!(parse_season_input(",").is_err());
        assert!(parse_season_input("-").is_err());
    }

    #[test]
    fn test_parse_leading_zeros() {
        assert_eq!(parse_season_input("01").unwrap(), vec![1]);
        assert_eq!(parse_season_input("01-03").unwrap(), vec![1, 2, 3]);
    }

    #[test]
    fn test_parse_long_comma_list() {
        let input = (1..=20)
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join(",");
        let result = parse_season_input(&input).unwrap();
        assert_eq!(result.len(), 20);
        assert_eq!(result[0], 1);
        assert_eq!(result[19], 20);
    }
}
