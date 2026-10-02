use regex::Regex;
use std::collections::HashSet;
use std::sync::LazyLock;

// The illegal-char table and compiled regexes live in the shared crate so the
// frontend (WASM) and backend use one definition; re-exported here so existing
// `crate::patterns::…` call sites stay unchanged.
pub use jumbie_shared::patterns::{ILLEGAL_CHARS, INVALID_CHARS, PlatformOs, platform_safe_chars};

/// Windows reserves these filenames at the filesystem level, even with
/// extensions like CON.txt. A cross-platform tool must avoid them because
/// archives may be extracted on — or transferred to — a Windows system.
pub static RESERVED_FILENAMES: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    let mut s = HashSet::new();
    for name in &[
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ] {
        s.insert(*name);
    }
    s
});

pub static MULTI_SPACE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[_\s]+").unwrap());

pub static SYMBOLS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^\w\s]").unwrap());
pub static WHITESPACE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").unwrap());

// Backend-only (not in the protocol crate): used to extract info-hashes from
// magnet links in file metadata or .torrent filenames, then find matching
// cached torrent files on disk.
pub static BTIH_URN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"urn:btih:([a-zA-Z0-9]+)").unwrap());

// Backend-only: matches multi-episode ranges that only arise when organizing
// already-downloaded files — "S01E01-E03", "E01+E02" (same season, no leading
// S), and old-style "1x01-1x02".
pub static MULTI_EPISODE_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:S\d{1,2})?E(\d{1,3})(?:[-+]?E(\d{1,3}))*|(\d{1,2})x(\d{1,2})").unwrap()
});
