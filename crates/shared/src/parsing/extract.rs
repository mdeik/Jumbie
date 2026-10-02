use crate::media_format::is_valid_media_ext;
use crate::patterns::*;

/// Strip quality/tag markers from a raw title part, returning a clean title.
///
/// Three separate passes (tags, parens, quality) so markers can appear in any order
/// or adjacency (e.g. `"Show [1080p](2024)"`); a single regex would need
/// lookaround/alternation that is harder to maintain.
///
/// Examples:
/// - `"My Show [1080p]"` → `"My Show"`
/// - `"Title (2024) [BD]"` → `"Title"`
pub fn clean_title(title: &str) -> String {
    let t = CLEAN_TAGS.replace_all(title, "");
    let t = CLEAN_PARENS.replace_all(&t, "");
    let t = CLEAN_QUALITY.replace_all(&t, "");
    t.trim().to_string()
}

/// Formats a byte count into a human-readable string (e.g. "1.5 GB", "450 MB").
pub fn format_bytes(bytes: u64) -> String {
    if bytes == 0 {
        return "0.0 B".to_string();
    }
    let b = bytes as f64;
    let k = 1024.0;
    if b < k {
        format!("{} B", bytes)
    } else if b < k * k {
        format!("{:.1} KB", b / k)
    } else if b < k * k * k {
        format!("{:.2} MB", b / (k * k))
    } else if b < k * k * k * k {
        format!("{:.2} GB", b / (k * k * k))
    } else {
        format!("{:.2} TB", b / (k * k * k * k))
    }
}

/// Extract the release group name from a media filename.
///
/// Supports `[Group] Title - 01.mkv` and `Title - 01 - Group.mkv`. GROUP_PATTERNS is
/// ordered with the bracket pattern first (less ambiguous) and the suffix pattern
/// second (it can match title-ending phrases or quality tags).
///
/// Returns `None` when no valid group is detected or the match is a known
/// quality/codec tag — the INVALID_GROUPS filter is essential because scene naming
/// often places quality markers after a dash where the suffix pattern would match them.
pub fn extract_submitter(filename: &str) -> Option<String> {
    // Strip known video/subtitle extensions first, otherwise the suffix pattern for
    // "Show - 01 - Group.mkv" would capture ".mkv" along with the group.
    let mut name_to_check = filename;
    if let Some(idx) = name_to_check.rfind('.') {
        let ext = &name_to_check[idx + 1..].to_lowercase();
        if is_valid_media_ext(ext.as_str()) {
            name_to_check = &name_to_check[..idx];
        }
    }

    for pattern in GROUP_PATTERNS.iter() {
        if let Some(captures) = pattern.captures(name_to_check)
            && let Some(group) = captures.get(1)
        {
            let g = group.as_str().trim();
            if !g.is_empty() && !INVALID_GROUPS.is_match(g) {
                return Some(g.to_string());
            }
        }
    }
    None
}

/// Extract the version number from a filename (e.g. "v2" in "Show S01E01 v2.mkv").
///
/// Defaults to 1 when absent so the return type stays a plain `i32`.
pub fn extract_version(filename: &str) -> i32 {
    if let Some(caps) = VERSION.captures(filename)
        && let Ok(v) = caps.get(1).unwrap().as_str().parse::<i32>()
    {
        return v;
    }
    1
}

/// Detect a multi-part episode indicator in a filename (e.g. "part-1", "cd2", "disc 3").
///
/// Part numbers may be numeric or the scene convention `a`-`d` (for 1-4); the mapping
/// stops at `d` since episodes with more than 4 parts are extremely rare.
pub fn detect_part_number(filename: &str) -> Option<u32> {
    let caps = PART_PATTERN.captures(filename)?;
    let raw = caps.get(2)?.as_str();

    if let Ok(n) = raw.parse::<u32>() {
        return Some(n);
    }

    match raw.to_lowercase().as_str() {
        "a" => Some(1),
        "b" => Some(2),
        "c" => Some(3),
        "d" => Some(4),
        _ => None,
    }
}
