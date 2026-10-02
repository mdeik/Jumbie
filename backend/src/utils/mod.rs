pub mod error;
pub mod file_naming;
pub mod http;
pub mod media_info;
pub mod metadata;
pub mod path_utils;

use jumbie_shared::formatting::LabelStyle;
pub use jumbie_shared::parsing::{
    ParseContext, clean_title, detect_part_number, extract_submitter, extract_version,
    match_title_compiled, parse_filename, parse_filename_for_season, parse_title_with_custom_regex,
};

use crate::patterns::{SYMBOLS, WHITESPACE};
pub use error::describe_error_chain;
pub use jumbie_shared::template::{TemplatePadOptions, apply_template};

/// Extract the series key (title) from a filename.
///
/// Falls back to separator-stripping + " S" detection when the shared parser
/// returns `None` (irregular torrent filenames, single-episode files, etc.).
pub fn generate_series_key(filename: &str) -> String {
    if let Some(info) = parse_filename(filename, ParseContext::FileScan) {
        info.series_key
    } else {
        let simple = filename.replace(['.', '_'], " ");
        if let Some(idx) = simple.find(" S") {
            return simple[..idx].trim().to_string();
        }
        simple
    }
}

/// Normalize a title for deduplication and comparison.
///
/// Titles made entirely of symbols would normalize to empty and break dedup, so
/// those fall back to a deterministic truncated xxh3_64 hash.
pub fn normalize_title(title: &str) -> String {
    if title.is_empty() {
        return String::new();
    }

    let normalized = title.to_lowercase();
    let normalized = SYMBOLS.replace_all(&normalized, "");
    let normalized = WHITESPACE.replace_all(&normalized, "");

    if normalized.is_empty() {
        return format!("{:x}", xxhash_rust::xxh3::xxh3_64(title.as_bytes()))
            .chars()
            .take(16)
            .collect();
    }

    normalized.to_string()
}

#[cfg(test)]
mod tests;

/// Combine season numbers into human-readable range strings.
///
/// Combine season numbers into human-readable range strings, matching the
/// Sonarr/Radarr convention. Numeric seasons are grouped into ranges (e.g.
/// "S01-S03"); non-numeric labels (e.g. "Specials") get an "S" prefix.
pub fn combine_seasons(seasons: &[String]) -> Vec<String> {
    if seasons.is_empty() {
        return Vec::new();
    }

    let mut parsed_seasons: Vec<i32> = Vec::new();
    let mut non_numeric = Vec::new();

    for s in seasons {
        if let Ok(num) = s.parse::<i32>() {
            parsed_seasons.push(num);
        } else {
            non_numeric.push(s.clone());
        }
    }

    parsed_seasons.sort();
    parsed_seasons.dedup();

    let mut result = Vec::new();

    if parsed_seasons.is_empty() {
        return non_numeric.into_iter().map(|s| format!("S{}", s)).collect();
    }

    let mut range_start = parsed_seasons[0];
    let mut prev = parsed_seasons[0];

    for &curr in &parsed_seasons[1..] {
        if curr == prev + 1 {
            prev = curr;
        } else {
            if range_start == prev {
                result.push(jumbie_shared::formatting::fmt_season(
                    range_start,
                    LabelStyle::Short,
                ));
            } else {
                result.push(format!(
                    "{}-{}",
                    jumbie_shared::formatting::fmt_season(range_start, LabelStyle::Short),
                    jumbie_shared::formatting::fmt_season(prev, LabelStyle::Short)
                ));
            }
            range_start = curr;
            prev = curr;
        }
    }

    if range_start == prev {
        result.push(jumbie_shared::formatting::fmt_season(
            range_start,
            LabelStyle::Short,
        ));
    } else {
        result.push(format!(
            "{}-{}",
            jumbie_shared::formatting::fmt_season(range_start, LabelStyle::Short),
            jumbie_shared::formatting::fmt_season(prev, LabelStyle::Short)
        ));
    }

    let non_numeric_prefixed = non_numeric.into_iter().map(|s| format!("S{}", s));

    result.extend(non_numeric_prefixed);

    result
}

/// Get the extension of a filename, including double extensions like ".en.srt".
///
/// Get the extension of a filename, including double extensions like ".en.srt".
///
/// Subtitle files often have a 2-3 char language code before the format extension
/// (e.g. "Show.S01E01.en.srt"); this returns the compound extension so the language
/// tag is not stripped as if it were part of the stem.
pub fn get_extended_extension(filename: &str) -> String {
    let path = std::path::Path::new(filename);
    let mut ext = match path.extension().and_then(|e| e.to_str()) {
        Some(e) => e.to_lowercase(),
        None => return String::new(),
    };

    if let Some(stem_path) = path.file_stem()
        && let Some(stem_str) = stem_path.to_str()
    {
        let inner_path = std::path::Path::new(stem_str);
        if let Some(inner_ext) = inner_path.extension().and_then(|e| e.to_str()) {
            let lower_inner = inner_ext.to_lowercase();
            if lower_inner.len() == 2 || lower_inner.len() == 3 {
                // Likely a language code e.g. .en.srt
                ext = format!("{}.{}", lower_inner, ext);
            }
        }
    }

    ext
}
