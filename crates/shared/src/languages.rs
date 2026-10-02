//! Language identifier mappings used for subtitle labeling and series metadata.
//!
//! A flat tuple list rather than an enum so plugins/external indexers can extend
//! it without recompiling. Each entry pairs a human-readable label shown in UI
//! picklists with comma-separated search tokens recognized by indexers (the first
//! token is always the ISO 639-2 code). Ordered roughly by prevalence in the scene
//! release ecosystem so common selections appear first in dropdowns.

pub static LANGUAGES: &[(&str, &str)] = &[
    ("English (eng; en)", "eng,en"),
    ("Middle English (1100-1500) (enm)", "enm"),
    ("Spanish (spa; es)", "spa,es"),
    ("French (fre; fr)", "fre,fr"),
    ("German (ger; de)", "ger,de"),
    ("Italian (ita; it)", "ita,it"),
    ("Portuguese (por; pt)", "por,pt"),
    ("Russian (rus; ru)", "rus,ru"),
    ("Chinese (chi; zh)", "chi,zh"),
    ("Japanese (jpn; ja)", "jpn,ja,jp"),
    ("Korean (kor; ko)", "kor,ko"),
];

/// Normalize a language code or name to its ISO 639-2 three-letter code.
///
/// The **single source of truth** for language normalization: the mapping is
/// derived from [`LANGUAGES`], so adding an entry there makes it usable here.
/// Matches the display-name prefix (label text before the first `(`) or any
/// comma-separated search token, returning the first token (the 3-letter code).
/// Input is trimmed and lowercased; unknown input is returned lowercased
/// unchanged rather than erroring. A 3-letter code is always a valid token, so
/// `normalize_language("eng")` returns `"eng"`.
///
/// # Example
///
/// ```rust
/// use jumbie_shared::languages::normalize_language;
///
/// assert_eq!(normalize_language("en"), "eng");
/// assert_eq!(normalize_language("english"), "eng");
/// assert_eq!(normalize_language("jp"), "jpn");
/// assert_eq!(normalize_language("japanese"), "jpn");
/// assert_eq!(normalize_language("de"), "ger");
/// assert_eq!(normalize_language("zh"), "chi");
/// assert_eq!(normalize_language("unknown"), "unknown"); // passes through
/// ```
pub fn normalize_language(lang: &str) -> String {
    let lower = lang.trim().to_lowercase();

    for &(label, tokens) in LANGUAGES {
        let primary = tokens.split(',').next().unwrap_or(tokens);

        let name_end = label.find('(').unwrap_or(label.len());
        let display_name = label[..name_end].trim().to_lowercase();
        if display_name == lower {
            return primary.to_string();
        }

        for token in tokens.split(',') {
            let token = token.trim();
            if !token.is_empty() && token.to_lowercase() == lower {
                return primary.to_string();
            }
        }
    }

    lower
}

/// Language tokens recognised in release filenames: ISO 639-1 two-letter codes,
/// ISO 639-2/B (and a few /T) three-letter codes, and common scene aliases
/// (`jp`, `chs`, `cht`).
///
/// Matching is an explicit allowlist rather than "any 2–3 letter run" because
/// video release names are full of short tokens that are not languages (`HDR`,
/// `SDR`, `WEB`, `AAC`, `DDP`, `DTS`). Two otherwise-valid codes are omitted on
/// purpose because they collide with far more common release tags: `dv`
/// (Dhivehi vs Dolby Vision) and `vo` (Volapük vs "version originale").
pub static LANGUAGE_TAG_TOKENS: &[&str] = &[
    // ISO 639-1 (two-letter).
    "aa", "ab", "ae", "af", "ak", "am", "an", "ar", "as", "av", "ay", "az", "ba", "be", "bg", "bh",
    "bi", "bm", "bn", "bo", "br", "bs", "ca", "ce", "ch", "co", "cr", "cs", "cu", "cv", "cy", "da",
    "de", "dz", "ee", "el", "en", "eo", "es", "et", "eu", "fa", "ff", "fi", "fj", "fo", "fr", "fy",
    "ga", "gd", "gl", "gn", "gu", "gv", "ha", "he", "hi", "ho", "hr", "ht", "hu", "hy", "hz", "ia",
    "id", "ie", "ig", "ii", "ik", "io", "is", "it", "iu", "ja", "jv", "ka", "kg", "ki", "kj", "kk",
    "kl", "km", "kn", "ko", "kr", "ks", "ku", "kv", "kw", "ky", "la", "lb", "lg", "li", "ln", "lo",
    "lt", "lu", "lv", "mg", "mh", "mi", "mk", "ml", "mn", "mr", "ms", "mt", "my", "na", "nb", "nd",
    "ne", "ng", "nl", "nn", "no", "nr", "nv", "ny", "oc", "oj", "om", "or", "os", "pa", "pi", "pl",
    "ps", "pt", "qu", "rm", "rn", "ro", "ru", "rw", "sa", "sc", "sd", "se", "sg", "si", "sk", "sl",
    "sm", "sn", "so", "sq", "sr", "ss", "st", "su", "sv", "sw", "ta", "te", "tg", "th", "ti", "tk",
    "tl", "tn", "to", "tr", "ts", "tt", "tw", "ty", "ug", "uk", "ur", "uz", "ve", "vi", "wa", "wo",
    "xh", "yi", "yo", "za", "zh", "zu",
    // ISO 639-2/B (and common /T) three-letter codes.
    "eng", "enm", "spa", "fre", "fra", "ger", "deu", "ita", "por", "rus", "chi", "zho", "jpn",
    "kor", "ara", "hin", "tha", "vie", "tur", "pol", "dut", "nld", "swe", "nor", "dan", "fin",
    "cze", "ces", "hun", "gre", "ell", "heb", "ukr", "ron", "rum", "ind", "may", "msa", "tam",
    "tel", "ben", "urd", "mal", "kan", "mar", "guj", "pan", "srp", "hrv", "slk", "slo", "slv",
    "bul", "cat", "fil", "tgl", "eus", "baq", "glg", "isl", "ice", "lit", "lav", "est", "sqi",
    "alb", "mkd", "mac", "bos", "aze", "kaz", "uzb", "mon", "khm", "lao", "mya", "bur", "sin",
    "nep", "amh", "swa", "zul", "afr", "yid", "jav", "fas", "per", "mul", "und",
    // Scene aliases.
    "jp", "chs", "cht",
];

/// True when `token` is a recognised language tag (case-insensitive).
///
/// SSoT for "is this dot-segment a language tag", shared by path collision
/// handling (so a counter is placed before the tag) and any other caller that
/// needs to tell a language apart from a release token.
pub fn is_language_tag(token: &str) -> bool {
    let lower = token.to_ascii_lowercase();
    LANGUAGE_TAG_TOKENS.contains(&lower.as_str())
}

#[cfg(test)]
mod tests {
    use super::{is_language_tag, normalize_language};

    #[test]
    fn recognises_common_language_tags_case_insensitively() {
        for tag in [
            "en", "eng", "EN", "Eng", "jp", "jpn", "ja", "zh", "chi", "zho", "chs", "cht", "spa",
            "es", "deu", "ger", "de", "fre", "fra", "pt", "por", "kor", "tha", "vie",
        ] {
            assert!(is_language_tag(tag), "{tag} should be a language tag");
        }
    }

    #[test]
    fn rejects_release_tokens_numbers_and_flags() {
        for token in [
            "hdr", "sdr", "dv", "vo", "aac", "ddp", "dts", "web", "bluray", "remux", "hdtv",
            "hevc", "x265", "1080p", "2019", "001", "forced", "sdh", "multi", "default", "",
        ] {
            assert!(
                !is_language_tag(token),
                "{token} must not be a language tag"
            );
        }
    }

    #[test]
    fn normalize_still_maps_codes_and_names() {
        assert_eq!(normalize_language("en"), "eng");
        assert_eq!(normalize_language("Japanese"), "jpn");
    }
}
