use regex::Regex;
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

// Caches compiled wildcard regexes; the set of unique patterns is small enough
// that a plain `Mutex<HashMap>` has negligible contention.
static REGEX_CACHE: LazyLock<Mutex<HashMap<String, Regex>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Matches a URI/link against a wildcard pattern (`*` matches any run of characters),
/// e.g. `*`, `magnet:*`, `https://apple.*`, `*://nyaa.*`.
pub fn matches_protocol(pattern: &str, uri: &str) -> bool {
    let regex = {
        let mut cache = REGEX_CACHE.lock().unwrap();
        if let Some(re) = cache.get(pattern) {
            re.clone()
        } else {
            let re_str = wildcard_to_regex(pattern);
            match Regex::new(&re_str) {
                Ok(re) => {
                    cache.insert(pattern.to_string(), re.clone());
                    re
                }
                Err(_) => return false, // Invalid pattern — don't panic, just don't match
            }
        }
    };

    regex.is_match(uri)
}

/// Heuristic check for a download link or magnet, intentional rather than using a
/// URI parser: `magnet:?` or any `"://"` scheme passes.
pub fn is_likely_link(link: &str) -> bool {
    link.starts_with("magnet:?") || link.contains("://")
}

/// Convert a wildcard pattern (with `*`) to an anchored regex.
///
/// Anchoring makes `*` mean "from this position" rather than "anywhere":
/// `"magnet:*"` must start with `magnet:`, `"*.torrent"` must end with `.torrent`.
fn wildcard_to_regex(pattern: &str) -> String {
    let mut re = String::from("^");
    for c in pattern.chars() {
        match c {
            '*' => re.push_str(".*"),
            // Escape all regex metacharacters so they're treated literally
            '.' | '+' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '^' | '$' | '|' | '\\' => {
                re.push('\\');
                re.push(c);
            }
            _ => re.push(c),
        }
    }
    re.push('$');
    re
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_matches_protocol() {
        assert!(matches_protocol("*", "anything"));
        assert!(matches_protocol("magnet:*", "magnet:?xt=urn:btih:hash"));
        assert!(!matches_protocol("magnet:*", "http://example.com"));

        assert!(matches_protocol(
            "https://apple.*",
            "https://apple.com/test"
        ));
        assert!(matches_protocol("https://apple.*", "https://apple.it"));
        assert!(!matches_protocol("https://apple.*", "http://apple.com"));

        assert!(matches_protocol("*://nyaa.*", "https://nyaa.si/view/123"));
        assert!(matches_protocol("*://nyaa.*", "http://nyaa.pantsu.cat/"));
        assert!(!matches_protocol("*://nyaa.*", "https://google.com"));

        assert!(matches_protocol(
            "*.torrent",
            "http://example.com/file.torrent"
        ));
        assert!(!matches_protocol(
            "*.torrent",
            "http://example.com/file.mkv"
        ));
    }

    #[test]
    fn test_is_likely_link() {
        assert!(is_likely_link("magnet:?xt=urn:btih:hash"));
        assert!(is_likely_link("http://example.com"));
        assert!(is_likely_link("https://example.com"));
        assert!(is_likely_link("sftp://server/file"));
        assert!(is_likely_link("file:///path/to/file"));
        assert!(!is_likely_link("not a link"));
        assert!(!is_likely_link("http:/missing_slash"));
    }
}
