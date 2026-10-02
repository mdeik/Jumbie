/// Maximum bytes for a single path component (series/season folder or episode
/// filename including extension).
///
/// Most Linux filesystems cap a directory entry at 255 bytes; this leaves
/// headroom for truncation and collision counters appended by
/// `crate::paths::next_free_suffixed_path`.
pub const MAX_NAME_BYTES: usize = 250;

/// Truncate a name to fit within `max_bytes`, preserving the extension when possible.
///
/// Crops the stem (name without extension) to make the total fit; if the
/// extension itself is too long, truncates the whole name. Byte-aware rather than
/// character-aware because filesystem limits are specified in bytes, and multi-byte
/// UTF-8 sequences are never split (we back up to a character boundary).
///
/// # Example
///
/// ```
/// // Short names pass through unchanged
/// assert_eq!(jumbie::utils::file_naming::truncate_name_to_bytes("short.mkv", 10), "short.mkv");
///
/// // Long stem is cropped, extension preserved
/// assert_eq!(jumbie::utils::file_naming::truncate_name_to_bytes("very_long_name.mkv", 12), "very_lon.mkv");
/// ```
pub fn truncate_name_to_bytes(name: &str, max_bytes: usize) -> String {
    let name_bytes = name.len();
    if name_bytes <= max_bytes {
        return name.to_string();
    }

    // Split at the last dot to separate stem from extension; with no dot, the
    // whole thing is the stem.
    if let Some(dot_idx) = name.rfind('.') {
        let stem = &name[..dot_idx];
        let ext = &name[dot_idx..]; // includes the leading dot

        let ext_bytes = ext.len();
        let max_stem_bytes = max_bytes.saturating_sub(ext_bytes);

        if max_stem_bytes > 0 {
            // Truncate the stem at `max_stem_bytes` bytes, backing up if we
            // land in the middle of a multi-byte UTF-8 continuation byte.
            let bytes = stem.as_bytes();
            let end = max_stem_bytes.min(bytes.len());
            let mut adjusted_end = end;
            // Continuation bytes have the high bits 10xxxxxx.
            while adjusted_end > 0 && (bytes[adjusted_end] & 0xC0) == 0x80 {
                adjusted_end -= 1;
            }
            if adjusted_end == 0 {
                // Even one byte would split a multi-byte char; drop to empty stem.
                return ext.to_string();
            }
            let truncated_stem = &stem[..adjusted_end];
            return format!("{}{}", truncated_stem, ext);
        }
    }

    // No extension, or the extension consumes the whole budget: truncate at
    // max_bytes, respecting UTF-8 boundaries.
    let bytes = name.as_bytes();
    let end = max_bytes.min(bytes.len());
    let mut adjusted_end = end;
    while adjusted_end > 0 && (bytes[adjusted_end] & 0xC0) == 0x80 {
        adjusted_end -= 1;
    }
    name[..adjusted_end].to_string()
}

/// Detect and strip a collision suffix from a path.
///
/// SSoT: re-exported from [`jumbie_shared::paths`] so the reverse of the naming
/// logic lives next to the engine that applies it.
pub use jumbie_shared::paths::strip_collision_suffix;

#[cfg(test)]
mod tests {
    use super::*;
    use jumbie_shared::config::organization::CollisionRenameSuffix;
    use std::path::{Path, PathBuf};

    // truncate_name_to_bytes tests

    #[test]
    fn test_truncate_name_shorter_than_limit() {
        // Names under the limit pass through unchanged
        assert_eq!(truncate_name_to_bytes("short.mkv", 50), "short.mkv");
        assert_eq!(truncate_name_to_bytes("no_ext", 50), "no_ext");
        assert_eq!(truncate_name_to_bytes("", 50), "");
    }

    #[test]
    fn test_truncate_name_exactly_at_limit() {
        // Name exactly at the limit — no truncation needed
        assert_eq!(truncate_name_to_bytes("12345.mkv", 9), "12345.mkv");
        assert_eq!(truncate_name_to_bytes("noext", 5), "noext");
    }

    #[test]
    fn test_truncate_name_crops_stem_preserves_extension() {
        let result = truncate_name_to_bytes("very_long_episode_name.mkv", 15);
        // ".mkv" = 4 bytes, stem budget = 11 bytes
        assert!(result.ends_with(".mkv"), "Extension must be preserved");
        assert_eq!(result.len(), 15, "Total byte count must fit limit");
    }

    #[test]
    fn test_truncate_name_crops_stem_preserves_long_extension() {
        // Double extension: only last dot groups as extension
        let result = truncate_name_to_bytes("EpisodeTitle.extracted_subtitle.srt", 20);
        // Last dot → ".srt" (4), stem budget = 16
        assert!(result.ends_with(".srt"), "Last extension must be preserved");
        assert!(result.len() <= 20, "Total byte count must fit limit");
    }

    #[test]
    fn test_truncate_name_without_extension() {
        let result = truncate_name_to_bytes("ThisIsAVeryLongNameWithoutExtension", 10);
        assert_eq!(result, "ThisIsAVer");
        assert_eq!(result.len(), 10);
    }

    #[test]
    fn test_truncate_name_unicode_multi_byte() {
        // CJK chars are 3 bytes each — must not split multi-byte sequences
        let name = "日本語のタイトル.mkv"; // 18 bytes stem + 4 bytes ext = 22
        let result = truncate_name_to_bytes(name, 15);
        // stem budget = 11. "日本語" = 9 bytes. Next byte starts 3-byte "の" but only 2 left →
        // back up to 9 → "日本語.mkv" (13 bytes)
        assert!(result.ends_with(".mkv"), "Extension must be preserved");
        assert!(result.len() <= 15, "Byte count must fit limit");
        // "日本語" is 9 bytes, "の" would be 12 (over budget), so we only get "日本語"
        assert_eq!(result, "日本語.mkv");
    }

    #[test]
    fn test_truncate_name_unicode_without_extension() {
        // Multi-byte chars — must not split a character
        let name = "Café résumé"; // total 14 bytes
        let result = truncate_name_to_bytes(name, 8);
        // byte 8 is middle of "é" (bytes 7-8), back up to byte 7 → "Café r"
        assert_eq!(result, "Café r");
        assert!(result.len() <= 8);
    }

    #[test]
    fn test_truncate_name_extension_exceeds_limit() {
        // Extension alone exceeds the limit
        let result = truncate_name_to_bytes("Short.very_long_extension_here", 12);
        // ext = ".very_long_extension_here" = 27 bytes, max_stem_bytes = 0
        // Falls through to byte-truncation at 12
        assert_eq!(result.len(), 12);
    }

    #[test]
    fn test_truncate_name_stem_reduces_to_empty() {
        // Only 1 byte left for stem
        let result = truncate_name_to_bytes("abc.mkv", 5);
        // ext=".mkv"=4, max_stem=1, stem="abc"=3, truncate to 1 byte → "a.mkv"
        assert_eq!(result, "a.mkv");
        assert_eq!(result.len(), 5);

        // No room for even one stem byte plus the dot at max_bytes=4
        let result = truncate_name_to_bytes("abc.mkv", 4);
        // ext=".mkv"=4, max_stem=0 → falls to byte-truncation at 4 → "abc."
        assert_eq!(result, "abc.");
        assert_eq!(result.len(), 4);
    }

    #[test]
    fn test_truncate_name_max_bytes_constant_used() {
        // Verify that names exactly at MAX_NAME_BYTES pass through,
        // and names one byte over get the stem cropped.
        // Build a stem where stem + ".mkv" = MAX_NAME_BYTES + 1
        let long_stem = "A".repeat(MAX_NAME_BYTES - 4 + 1); // stem that makes total = MAX+1
        let name = format!("{}.mkv", long_stem);
        assert_eq!(name.len(), MAX_NAME_BYTES + 1);

        let result = truncate_name_to_bytes(&name, MAX_NAME_BYTES);
        assert!(result.ends_with(".mkv"), "Extension must be preserved");
        assert_eq!(result.len(), MAX_NAME_BYTES, "Must fit exactly in limit");
        // Stem should have been cropped by 1 byte
        let expected_stem_len = MAX_NAME_BYTES - 4;
        assert_eq!(
            result.trim_end_matches(".mkv").len(),
            expected_stem_len,
            "Stem is {} bytes, expected {}",
            result.trim_end_matches(".mkv").len(),
            expected_stem_len
        );
    }

    // strip_collision_suffix tests

    #[test]
    fn test_strip_dot_numeric_with_extension() {
        let path = Path::new("/media/Episode.001.mkv");
        let (stripped, variant) = strip_collision_suffix(path).unwrap();
        assert_eq!(stripped, Path::new("/media/Episode.mkv"));
        assert_eq!(variant, CollisionRenameSuffix::DotNumeric);
    }

    #[test]
    fn test_strip_dot_numeric_without_extension() {
        let path = Path::new("/media/Episode.001");
        let (stripped, variant) = strip_collision_suffix(path).unwrap();
        assert_eq!(stripped, Path::new("/media/Episode"));
        assert_eq!(variant, CollisionRenameSuffix::DotNumeric);
    }

    #[test]
    fn test_strip_dot_numeric_no_false_positive_on_4_digits() {
        // ".0001" is 4 digits — should NOT match DotNumeric
        let path = Path::new("/media/Episode.0001.mkv");
        assert!(strip_collision_suffix(path).is_none());
    }

    #[test]
    fn test_strip_paren_numeric_with_extension() {
        let path = Path::new("/media/Episode (1).mkv");
        let (stripped, variant) = strip_collision_suffix(path).unwrap();
        assert_eq!(stripped, Path::new("/media/Episode.mkv"));
        assert_eq!(variant, CollisionRenameSuffix::ParenNumeric);
    }

    #[test]
    fn test_strip_paren_numeric_without_extension() {
        let path = Path::new("/media/Episode (42)");
        let (stripped, variant) = strip_collision_suffix(path).unwrap();
        assert_eq!(stripped, Path::new("/media/Episode"));
        assert_eq!(variant, CollisionRenameSuffix::ParenNumeric);
    }

    #[test]
    fn test_strip_paren_numeric_requires_space_before_paren() {
        // "name(1).ext" — no space before paren, should NOT match
        let path = Path::new("/media/Episode(1).mkv");
        assert!(strip_collision_suffix(path).is_none());
    }

    #[test]
    fn test_strip_underscore_numeric_with_extension() {
        let path = Path::new("/media/Episode_001.mkv");
        let (stripped, variant) = strip_collision_suffix(path).unwrap();
        assert_eq!(stripped, Path::new("/media/Episode.mkv"));
        assert_eq!(variant, CollisionRenameSuffix::UnderscoreNumeric);
    }

    #[test]
    fn test_strip_underscore_numeric_without_extension() {
        let path = Path::new("/media/Episode_001");
        let (stripped, variant) = strip_collision_suffix(path).unwrap();
        assert_eq!(stripped, Path::new("/media/Episode"));
        assert_eq!(variant, CollisionRenameSuffix::UnderscoreNumeric);
    }

    #[test]
    fn test_strip_underscore_numeric_exactly_3_digits() {
        // Only 2 digits after underscore — should NOT match
        let path = Path::new("/media/Episode_01.mkv");
        assert!(strip_collision_suffix(path).is_none());
    }

    #[test]
    fn test_strip_dash_numeric_with_extension() {
        let path = Path::new("/media/Episode-1.mkv");
        let (stripped, variant) = strip_collision_suffix(path).unwrap();
        assert_eq!(stripped, Path::new("/media/Episode.mkv"));
        assert_eq!(variant, CollisionRenameSuffix::DashNumeric);
    }

    #[test]
    fn test_strip_dash_numeric_multi_digit() {
        let path = Path::new("/media/Episode-999.mkv");
        let (stripped, variant) = strip_collision_suffix(path).unwrap();
        assert_eq!(stripped, Path::new("/media/Episode.mkv"));
        assert_eq!(variant, CollisionRenameSuffix::DashNumeric);
    }

    #[test]
    fn test_strip_dash_numeric_without_extension() {
        let path = Path::new("/media/Episode-7");
        let (stripped, variant) = strip_collision_suffix(path).unwrap();
        assert_eq!(stripped, Path::new("/media/Episode"));
        assert_eq!(variant, CollisionRenameSuffix::DashNumeric);
    }

    #[test]
    fn test_no_suffix_returns_none() {
        let path = Path::new("/media/Episode.mkv");
        assert!(strip_collision_suffix(path).is_none());
    }

    #[test]
    fn test_no_suffix_no_extension() {
        let path = Path::new("/media/Episode");
        assert!(strip_collision_suffix(path).is_none());
    }

    #[test]
    fn test_strip_stem_that_ends_with_digits_not_a_suffix() {
        // Stem ends with digits but they are part of the original name
        let path = Path::new("/media/Season1_001.mkv");
        let (stripped, variant) = strip_collision_suffix(path).unwrap();
        assert_eq!(stripped, Path::new("/media/Season1.mkv"));
        assert_eq!(variant, CollisionRenameSuffix::UnderscoreNumeric);
    }

    #[test]
    fn test_strip_multiple_suffixes_by_repeated_calls() {
        // Stripping "-2" from "name-2-1.mkv" should give "name-1.mkv"
        let path = Path::new("/media/name-2-1.mkv");
        let (stripped, _) = strip_collision_suffix(path).unwrap();
        assert_eq!(stripped, PathBuf::from("/media/name-2.mkv"));

        // Stripping again should give "name.mkv"
        let (stripped2, _) = strip_collision_suffix(&stripped).unwrap();
        assert_eq!(stripped2, PathBuf::from("/media/name.mkv"));
    }
}
