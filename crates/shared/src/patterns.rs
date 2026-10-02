//! Regex patterns for parsing media filenames and extracting metadata.
//!
//! Compiled once via `LazyLock` to avoid recompiling on every `parse_filename` call.
//!
//! `CLEAN_*` strip noise from titles; `GROUP_*`/`INVALID_GROUPS` extract release
//! groups (ordered by specificity); `VERSION`/`PART_PATTERN` are secondary
//! extractors; `RANGE_PATTERNS`/`SINGLE_PATTERNS`/`SEASON_PACK_PATTERNS` parse
//! episodes (ranges → singles → season-packs).

use regex::Regex;
use std::sync::LazyLock;

// Filesystem-safe filename patterns shared by the WASM frontend and backend.
// The regexes are compiled lazily per OS family; `platform_safe_chars` picks at
// runtime because the frontend must emulate the server's OS, not the browser's.

/// The operating system a path policy applies to.
///
/// `Host` resolves via `cfg!(target_os)`; the frontend never uses it — it passes
/// the backend's OS explicitly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PlatformOs {
    /// The OS the current binary runs on.
    #[default]
    Host,
    /// Windows (NTFS filename restrictions apply).
    Windows,
    /// Any Unix-like OS (macOS, Linux, BSD).
    Unix,
}

impl PlatformOs {
    /// Whether this OS applies NTFS filename restrictions.
    pub fn is_windows(self) -> bool {
        match self {
            Self::Windows => true,
            Self::Unix => false,
            Self::Host => cfg!(target_os = "windows"),
        }
    }
}

/// SSoT: Filesystem-illegal printable characters, their Unicode look-alikes,
/// and whether they are universally invalid (not just Windows-specific).
///
/// Each entry: `(character, unicode_lookalike, is_universally_invalid)`
/// - `is_universally_invalid`: `true` if this char cannot appear in filenames
///   on ANY platform (e.g. `/` is the Unix path separator). `false` if it is
///   only invalid on Windows (`<>:"\\|?*`).
///
/// Control characters (\x00-\x1f) are handled separately — they have no visual
/// counterpart and are always removed regardless of policy.
pub static ILLEGAL_CHARS: &[(char, &str, bool)] = &[
    ('<', "\u{FF1C}", false),  // fullwidth less-than
    ('>', "\u{FF1E}", false),  // fullwidth greater-than
    (':', "\u{FF1A}", false),  // fullwidth colon
    ('"', "\u{FF02}", false),  // fullwidth quotation mark
    ('/', "\u{2215}", true),   // division slash — universal (path separator on all OSes)
    ('\\', "\u{FF3C}", false), // fullwidth reverse solidus — Windows-only path separator
    ('|', "\u{FF5C}", false),  // fullwidth vertical line
    ('?', "\u{FF1F}", false),  // fullwidth question mark
    ('*', "\u{FF0A}", false),  // fullwidth asterisk
];

/// Build a regex character class from a subset of `ILLEGAL_CHARS`.
/// `predicate` filters which entries to include (e.g. only universal ones).
fn build_illegal_regex<F>(predicate: F) -> Regex
where
    F: Fn(&(char, &str, bool)) -> bool,
{
    let mut pattern = String::from("[");
    for (c, _, _) in ILLEGAL_CHARS.iter().filter(|entry| predicate(entry)) {
        match c {
            '\\' => pattern.push_str(r"\\"),
            other => pattern.push(*other),
        }
    }
    pattern.push_str(r"\x00-\x1f]");
    Regex::new(&pattern).expect("illegal char regex")
}

/// Returns `true` if a character from `ILLEGAL_CHARS` is invalid in filenames
/// on the given OS.
///
/// - `/` is invalid everywhere (Unix path separator; Windows APIs also treat it
///   as a separator).
/// - `\\`, `<>`, `:`, `"`, `|`, `?`, `*` are only invalid on Windows (NTFS
///   filename restrictions). They are perfectly valid on Linux/macOS.
fn is_invalid_on_os(os: PlatformOs, entry: &(char, &str, bool)) -> bool {
    let (c, _, _) = entry;
    match c {
        // / is the path separator on all Unix-like systems, and Windows APIs
        // also normalise / to \, so it effectively cannot be in a filename.
        '/' => true,
        // \ is the path separator on Windows, valid in filenames elsewhere.
        '\\' => os.is_windows(),
        // The remaining chars are NTFS filename restrictions:
        // https://learn.microsoft.com/en-us/windows/win32/fileio/naming-a-file
        '<' | '>' | ':' | '"' | '|' | '?' | '*' => os.is_windows(),
        _ => false,
    }
}

/// Cross-platform safe set: ALL `ILLEGAL_CHARS` + control chars.
/// This is the strictest set, ensuring files are valid on Windows, macOS, Linux,
/// and SMB/CIFS network mounts alike.
pub static INVALID_CHARS: LazyLock<Regex> = LazyLock::new(|| build_illegal_regex(|_| true));

/// Unix-aware set: only chars that are invalid on Unix + control chars
/// (`/` and control chars). On Windows the platform-safe set is identical to
/// [`INVALID_CHARS`] (NTFS rejects those chars itself), so
/// [`platform_safe_chars`] returns `INVALID_CHARS` for `PlatformOs::Windows`.
pub static PLATFORM_SAFE_CHARS_UNIX: LazyLock<Regex> =
    LazyLock::new(|| build_illegal_regex(|e| is_invalid_on_os(PlatformOs::Unix, e)));

/// The platform-safe regex for the given OS: only chars that are invalid on
/// THAT OS + control chars.
///
/// Runtime-selected (not compile-time `cfg!`) because the frontend sanitizes
/// for the SERVER's OS, which is unknown at compile time.
pub fn platform_safe_chars(os: PlatformOs) -> &'static Regex {
    match os {
        PlatformOs::Windows => &INVALID_CHARS,
        PlatformOs::Unix => &PLATFORM_SAFE_CHARS_UNIX,
        PlatformOs::Host => platform_safe_chars(if cfg!(target_os = "windows") {
            PlatformOs::Windows
        } else {
            PlatformOs::Unix
        }),
    }
}

/// Strip square-bracket tags: `[1080p]`, `[MockFansub]`, `[12345678]`, including
/// the preceding whitespace. Applied first in `clean_title`.
pub static CLEAN_TAGS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s*\[[^\]]*\]").unwrap());

/// Strip parenthesized content: `(2024)`, `(v2)` — applied after brackets in
/// `clean_title`.
pub static CLEAN_PARENS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s*\([^)]*\)").unwrap());

/// Strip trailing quality/codec tags: `1080p`, `HEVC`, `HDR`, `x264`, etc.
///
/// Applied last in `clean_title`. The trailing `.*` deliberately consumes the
/// rest of the stem, handling multi-tag suffixes like "1080p HEVC HDR".
pub static CLEAN_QUALITY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\s*(?:1080p|720p|480p|2160p|4K|HDR|HEVC|x264|x265|AV1|VP9|10bit|8bit).*")
        .unwrap()
});

/// Strip ` - Complete ...` suffixes from season-pack titles, applied only when
/// `is_season_pack || is_complete_pack`. Requires the leading ` - Complete` so
/// names like "The Complete Series" are untouched; understrips by design.
pub static CLEAN_TRAILING_DESCRIPTORS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\s*-\s*Complete\s+.*$").unwrap());

/// Extract the release group name, ordered most reliable first:
/// 1. `[Group] Title...` bracket prefix
/// 2. `... [tags]-Group` bracket suffix
/// 3. `... - Group (trailing)` spaced-dash suffix
/// 4. `... - Group` plain suffix for dot-notation
///
/// Ordering keeps hyphenated quality tags like `WEB-DL` from being captured (see
/// per-pattern comments below).
pub static GROUP_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    vec![
        Regex::new(r"(?i)^\[([^\]]+)\]").unwrap(), // [Group] Title...
        // Suffix after bracketed tags: "... [1080p][x264]-Group-Name"
        Regex::new(r"(?i)\]-([\w-]+)$").unwrap(),
        // Spaced-dash suffix with optional trailing metadata:
        // "... - Group-Name" or "... - Group-Name (CRC)"
        Regex::new(r"(?i) - ([\w-]+)(?:\s*\([^)]*\)\s*)*$").unwrap(),
        // Plain suffix fallback for dot-notation: "... -GRP" or "... -iAHD"
        Regex::new(r"(?i)-[ \.]*([\w-]+?)(?:\s*\([^)]*\)\s*)*$").unwrap(),
    ]
});

/// Quality/codec tags that look like group names: scene naming puts quality
/// markers at the end of a filename, the same position the suffix group pattern
/// checks. Missing a tag breaks cross-seeding and search matching.
pub static INVALID_GROUPS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
    r"(?i)^(1080p|720p|480p|2160p|4k|x264|x265|hevc|av1|web-dl|webrip|hdtv|bdrip|brrip|blu-ray|dual-audio|multi|truehd|flac|aac|eac3|ac3|s\d{2}e\d{2}(?:e\d{2})*|s\d{2}|e\d{2}|v\d)$"
).unwrap()
});

/// Version-marker prefix: `v`, `v.`, `ver`, `ver.`, `version`, `version.`
///
/// SSoT: [`VERSION`] and [`STRIP_VERSION`] both embed this via `format!`.
const VERSION_PREFIX: &str = r"(?:v\.?|ver(?:sion)?\.?)";

/// Extract version number from a filename: `v2`, `v.2`, `ver2`, `version2`, etc.
///
/// The `(?:\A|[\s_\[\.\-\(])` before the prefix and `(?:[\s_\]\.\-\)]|$)` after
/// the digits prevent matching inside words.  `_` is included in both character
/// classes because scene releases often use underscore as a word separator.
pub static VERSION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"(?i)(?:\A|[\s_\[\.\-\(]){}(\d+)(?:[\s_\]\.\-\)]|$)",
        VERSION_PREFIX
    ))
    .unwrap()
});

/// Strip version markers from a release title for title-normalization comparison.
///
/// SSoT: Embeds [`VERSION_PREFIX`] so it automatically stays in sync with
/// [`VERSION`].  Each alternation covers a common delimiter format.
pub static STRIP_VERSION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"(?i)\s*\[{0}\d+\]|\s*\({0}\d+\)|\.{0}\d+|\s+{0}\d+|_{0}\d+|-{0}\d+|\A{0}\d+",
        VERSION_PREFIX
    ))
    .unwrap()
});

/// Detect multi-part episode indicators: `part-1`, `cd2`, `disc 3`, `pt.a`, etc.
/// Numeric and alpha (`a`-`d`) part labels; alpha stops at `d` since >4-part
/// episodes are vanishingly rare in scene releases.
pub static PART_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)[_ .\-](cd|dvd|part|pt|disc|disk)[_ .\-]?(\d+|[a-d])(?:\.|$|\s|\[)").unwrap()
});

/// Multi-episode range patterns, ordered most-specific to least-specific: season
/// ranges (`S01E01-E05`) before absolute ranges (`01-05`), and within each group
/// bracketed titles before plain ones, so a plain pattern can't match part of a
/// bracketed filename and misparse the title.
pub static RANGE_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    vec![
    // Season-based with bracket prefix
    Regex::new(r"(?i)^\[[^\]]+\]\s*(.+?)\s*-\s*S(\d+)[Ee](\d+)(?:v\d+)?\s*[-~]\s*[Ee](\d+)(?:v\d+)?.*$").unwrap(),
    Regex::new(r"(?i)^\[[^\]]+\]\s*(.+?)\s*-\s*S(\d+)[Ee](\d+)(?:v\d+)?[-~](\d+)(?:v\d+)?.*$").unwrap(),
        // Season-based without bracket prefix
        Regex::new(r"(?i)^(.+?)\s*-\s*S(\d+)[Ee](\d+)(?:v\d+)?\s*[-~]\s*[Ee](\d+)(?:v\d+)?.*$").unwrap(),
        Regex::new(r"(?i)^(.+?)\s+S(\d+)[Ee](\d+)(?:v\d+)?\s*[-~]\s*[Ee](\d+)(?:v\d+)?.*$").unwrap(),
        Regex::new(r"(?i)^(.+?)\s*-\s*S(\d+)[Ee](\d+)(?:v\d+)?[-~](\d+)(?:v\d+)?.*$").unwrap(),
        Regex::new(r"(?i)^(.+?)\s+S(\d+)[Ee](\d+)(?:v\d+)?[-~](\d+)(?:v\d+)?.*$").unwrap(),
        // Season-based with space-separated ep range, no E (bracket prefix)
        Regex::new(r"(?i)^\[[^\]]+\]\s*(.+?)\s+S(\d+)\s+(\d+)\s*[-~]\s*(\d+)(?:v\d+)?.*$").unwrap(),
        // Season-based with space-separated ep range, no E (no bracket)
        Regex::new(r"(?i)^(.+?)\s+S(\d+)\s+(\d+)\s*[-~]\s*(\d+)(?:v\d+)?.*$").unwrap(),
        // Season-based with season num + E/Ep prefix range (bracket)
        Regex::new(r"(?i)^\[[^\]]+\]\s*(.+?)\s+(\d{1,3})\s+[Ee]p?\s*(\d+)\s*[-~]\s*(\d+)(?:v\d+)?.*$").unwrap(),
        // Season-based with season num + E/Ep prefix range (no bracket)
        Regex::new(r"(?i)^(.+?)\s+(\d{1,3})\s+[Ee]p?\s*(\d+)\s*[-~]\s*(\d+)(?:v\d+)?.*$").unwrap(),
    // Absolute-numbered ranges with bracket prefix
    Regex::new(r"(?i)^\[[^\]]+\]\s*(.+?)\s*-\s*(\d+)(?:v\d+)?\s*[-~]\s*(\d+)(?:v\d+)?.*$").unwrap(),
    // Absolute-numbered ranges without bracket prefix
    Regex::new(r"(?i)^(.+?)\s*-\s*(\d+)(?:v\d+)?\s*[-~]\s*(\d+)(?:v\d+)?.*$").unwrap(),
    // "Episode 01-05" or "EP01-05" style ranges
    Regex::new(r"(?i)^(.+?)[\s-]+(?:Episode|Ep)[\s\.]*(\d+)(?:v\d+)?\s*[-~]\s*(\d+)(?:v\d+)?").unwrap(),
]
});

/// Concatenated multi-episode runs with no separator (`Show.S01E01E02.mkv`,
/// `Show - S01E01E02E03`). None of `RANGE_PATTERNS` match this form — they all
/// require a `-~` separator — so without this a file would be read as its first
/// episode only.
///
/// Captures: group 1 title_part, group 2 season, group 3 the appended `E01E02…` run.
pub static CONCAT_EPISODE_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    vec![
        // Bracket prefix
        Regex::new(r"(?i)^\[[^\]]+\]\s*(.+?)\s*-\s*S(\d+)((?:[Ee]\d+){2,}).*$").unwrap(),
        // No bracket prefix (dash-separated)
        Regex::new(r"(?i)^(.+?)\s*-\s*S(\d+)((?:[Ee]\d+){2,}).*$").unwrap(),
        // No bracket prefix (space-separated)
        Regex::new(r"(?i)^(.+?)\s+S(\d+)((?:[Ee]\d+){2,}).*$").unwrap(),
    ]
});

/// Single episode patterns, ordered most-specific to least-specific: same
/// bracket-first ordering as `RANGE_PATTERNS`, and season info (`S01E01`) before
/// absolute numbers (`- 01`) so `"Show - S01E01"` isn't misparsed as `"Show - 01"`.
pub static SINGLE_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    vec![
        // Season-based with bracket prefix
        Regex::new(r"(?i)^\[[^\]]+\]\s*(.+?)\s*-\s*S(\d+)[Ee](\d+)(?:v\d+)?.*$").unwrap(),
        // Absolute-numbered with bracket prefix
        Regex::new(r"(?i)^\[[^\]]+\]\s*(.+?)\s*-\s*(\d+)(?:v\d+)?.*$").unwrap(),
        // Season-based without bracket prefix (dash-separated)
        Regex::new(r"(?i)^(.+?)\s*-\s*S(\d+)[Ee](\d+)(?:v\d+)?.*$").unwrap(),
        // Season-based without bracket prefix (space-separated)
        Regex::new(r"(?i)^(.+?)\s+S(\d+)[Ee](\d+)(?:v\d+)?.*$").unwrap(),
        // Season-based with season num + E/Ep prefix (bracket prefix)
        Regex::new(r"(?i)^\[[^\]]+\]\s*(.+?)\s+(\d{1,3})\s+[Ee]p?\s*(\d+)(?:v\d+)?.*$").unwrap(),
        // Season-based with season num + E/Ep prefix (no bracket)
        Regex::new(r"(?i)^(.+?)\s+(\d{1,3})\s+[Ee]p?\s*(\d+)(?:v\d+)?.*$").unwrap(),
        // Season-based with space-separated ep, no E (bracket prefix)
        Regex::new(r"(?i)^\[[^\]]+\]\s*(.+?)\s+S(\d+)\s+(\d+)(?:v\d+)?.*$").unwrap(),
        // Season-based with space-separated ep, no E (no bracket)
        Regex::new(r"(?i)^(.+?)\s+S(\d+)\s+(\d+)(?:v\d+)?.*$").unwrap(),
        // Absolute-numbered without bracket prefix (dash-separated)
        Regex::new(r"(?i)^(.+?)\s*-\s*(\d+)(?:v\d+)?.*$").unwrap(),
        // "Episode 1", "Ep 1", or "EP01" style
        Regex::new(r"(?i)^(.+?)[\s-]+(?:Episode|Ep)[\s\.]*(\d+)(?:v\d+)?").unwrap(),
    ]
});

/// Season pack patterns (entire season, no episode number). Checked last because
/// `S01` could appear as part of `S01E01`; the trailing terminator group keeps
/// `"Show S01E01"` from being misidentified as a pack.
/// Match lone numbers (1-3 digits on word boundaries) in cleaned filenames.
///
/// `parse_filename` uses this as a last-resort fallback: exactly one such group
/// means the number is the episode (season defaults to 1, overridable by folder
/// inference). Capped at 3 digits to avoid matching year markers.
pub static LONE_NUMBERS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b(\d{1,3})\b").unwrap());

/// Detect anime opening/ending theme files that are NOT episodes (matched against
/// the cleaned stem). Two branches, both anchored at the start so mid-title "op"/"ed"
/// (e.g. "Top Secret") can't match: `NC(?:OP|ED)` non-credit files, and bare
/// `(?:OP|ED)` which requires a following digit, `v2`, or dash to avoid "Ed, Edd n Eddy".
///
/// Matches `NCOP`, `NCOP1`, `NCOPv2`, `NCED - Title`, `OP1`, `OPv2`, `OP- ...`, `ED2`.
pub static OP_ED_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(?:NC(?:OP|ED)\d?(?:v\d+)?(?:\s|-|\.|$)|(?:OP|ED)(?:\d|v\d+|-))").unwrap()
});

/// Detect decimal episode numbers (e.g. `S01E1.5`, `Episode 1.5`). Episode numbers
/// are integers, so such releases are blocked from auto-download/assignment
/// (users can override with custom RSS patterns). Does NOT match `5.1 audio`,
/// `v1.5`, `1080p`, or file extensions.
pub static DECIMAL_EPISODE_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    // Decimal part capped at 1-2 digits, then `(?:\D|$)` (Rust regex has no
    // look-around) so resolution numbers like `S01E05.1080p` aren't read as 5.1080.
    Regex::new(
        // Allow `-` between `Ep` and the number (e.g. `Show-Ep-1.5.mkv`).
        r"(?i)(?:S\d+\s*[Ee]\s*\d+\.\d{1,2}(?:\D|$)|(?:^|[\s_-])(?:Episode|Ep)[-\s]*\d+\.\d{1,2}(?:\D|$))",
    )
    .unwrap()
});

/// Season-range patterns — multi-season packs (e.g. "S01-S02", "Season 1-2").
/// Checked before `SEASON_PACK_PATTERNS` so `S01-S02` is caught before `S01`.
/// Captures: group 1 title_part, group 2 first season, group 3 second season.
/// Extract season numbers from comma-separated tails like ", S03, S04" or " + S03 + S04"
/// (used by the comma/space variants of `SEASON_RANGE_PATTERNS`).
pub static S_IN_TAIL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"S(\d+)").unwrap());

/// Extract episode numbers from comma-separated tails like ", E03, E04" or ", e03, e04".
/// Used by COMMA_EPISODE_PATTERNS to parse additional episodes.
pub static EP_IN_TAIL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[Ee](\d+)").unwrap());

pub static SEASON_RANGE_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    vec![
        // Scene-style "S01-S02" or "S01 - S02" (space-separated before S)
        Regex::new(r"(?i)^(.+?)\s+S(\d+)\s*-\s*S(\d+)(?:v\d+)?\s*(?:\[.*|\(.*|\.\w+$|.*)$")
            .unwrap(),
        // Spelled-out "Season 1-2" or "Season 1 - 2"
        Regex::new(r"(?i)^(.+?)\s+Season\s+(\d+)\s*-\s*(\d+)(?:v\d+)?\s*(?:\[.*|\(.*|\.\w+$|.*)$")
            .unwrap(),
        // Plural "Seasons 1-2" or "Seasons 1 - 2"
        Regex::new(r"(?i)^(.+?)\s+Seasons\s+(\d+)\s*-\s*(\d+)(?:v\d+)?\s*(?:\[.*|\(.*|\.\w+$|.*)$")
            .unwrap(),
        // Comma-separated "S01, S02" or "S01, S02, S03" (first two explicitly captured)
        Regex::new(
            r"(?i)^(.+?)\s+S(\d+)[,\s+]+S(\d+)((?:[,\s+]+S\d+)*)\s*(?:\[.*|\(.*|\.\w+$|.*)$",
        )
        .unwrap(),
        // Plus-separated "S01 + S02"
        Regex::new(r"(?i)^(.+?)\s+S(\d+)\s*\+\s*S(\d+)\s*(?:\[.*|\(.*|\.\w+$|.*)$").unwrap(),
        // Space-separated seasons "S01 S02 S03" — no dashes/commas between them
        Regex::new(r"(?i)^(.+?)\s+S(\d+)\s+S(\d+)((?:\s+S\d+)*)\s*(?:\[.*|\(.*|\.\w+$|.*)$")
            .unwrap(),
        // Spelled-out comma list "Season 1, 2, 3" or "Seasons 1, 2, 3" — groups: title, s1, s2, tail
        Regex::new(
            r"(?i)^(.+?)\s+Seasons?\s+(\d+)[,\s]+(\d+)((?:[,\s]+\d+)*)\s*(?:\[.*|\(.*|\.\w+$|.*)$",
        )
        .unwrap(),
        // Implicit second S: "S01-04" (no S before second number).
        // The `\b` prevents matching resolution numbers (S01-1080p).
        Regex::new(r"(?i)^(.+?)\s+S(\d+)\s*-\s*(\d{1,2})\b\s*(?:\[.*|\(.*|\.\w+$|.*)$").unwrap(),
    ]
});

/// Comma-separated episode patterns — non-contiguous lists like "S01E02, E03, E04".
/// Checked after `RANGE_PATTERNS` but before `SINGLE_PATTERNS` so the list is caught
/// before a single pattern matches just "S01E02". Captures: group 1 title_part,
/// group 2 season, group 3 first episode, group 4 tail (parsed with `EP_IN_TAIL`).
/// Spelled-out episode list patterns ("Episodes 1, 2, 3"), Search context only, no season.
pub static SPELLED_OUT_EPISODE_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    vec![
        // "Episodes 1, 2, 3" or "Episode 1, 2, 3"
        Regex::new(r"(?i)^(.+?)\s+Episodes?\s+(\d+)((?:[,\s+]+\d+)*)\s*(?:\[.*|\(.*|\.\w+$|.*)$")
            .unwrap(),
    ]
});

/// Spelled-out "Season N Episode M" pairs, e.g. `Show Season 3 Episode 4`,
/// `Show Season 3, Episode 4`, `Show Season 3 - Episode 4`, and the ordinal
/// form `Show 2nd Season - Episode 4`.
///
/// Checked before [`SINGLE_PATTERNS`], whose generic `Episode N` pattern would
/// otherwise read the pair as a season-less episode (season 1).
/// Captures: group 1 title_part, group 2 season, group 3 episode.
pub static SEASON_EPISODE_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    vec![
        // Ordinal with bracket prefix: "[Group] Title 2nd Season - Episode 4"
        Regex::new(
            r"(?i)^\[[^\]]+\]\s*(.+?)\s+(\d+)\s*(?:st|nd|rd|th)\s+Season\s*[-,\s]?\s*(?:Episode|Ep)\s*(\d+)",
        )
        .unwrap(),
        // Ordinal without bracket prefix: "Title 2nd Season - Episode 4"
        Regex::new(
            r"(?i)^(.+?)\s+(\d+)\s*(?:st|nd|rd|th)\s+Season\s*[-,\s]?\s*(?:Episode|Ep)\s*(\d+)",
        )
        .unwrap(),
        // With a bracket prefix: "[Group] Title Season 3 Episode 4"
        Regex::new(
            r"(?i)^\[[^\]]+\]\s*(.+?)\s+Season\s+(\d+)\s*(?:v\d+)?\s*[-,\s]?\s*(?:Episode|Ep)\s*(\d+)",
        )
        .unwrap(),
        // No bracket prefix: "Title Season 3 Episode 4"
        Regex::new(
            r"(?i)^(.+?)\s+Season\s+(\d+)\s*(?:v\d+)?\s*[-,\s]?\s*(?:Episode|Ep)\s*(\d+)",
        )
        .unwrap(),
    ]
});

pub static COMMA_EPISODE_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    vec![
        // With season: "Title - S01E02, E03, E04"
        Regex::new(
            r"(?i)^(.+?)\s*-\s*S(\d+)[Ee](\d+)((?:[,\s+]+[Ee]\d+)*)\s*(?:\[.*|\(.*|\.\w+$|.*)$",
        )
        .unwrap(),
        // With bracket prefix: "[Group] Title - S01E02, E03, E04"
        Regex::new(
            r"(?i)^\[[^\]]+\]\s*(.+?)\s*-\s*S(\d+)[Ee](\d+)((?:[,\s+]+[Ee]\d+)*)\s*(?:\[.*|\(.*|\.\w+$|.*)$",
        )
        .unwrap(),
    ]
});

pub static SEASON_PACK_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    vec![
        // Season pack with bracket prefix (e.g. "[Group] Title - S02")
        Regex::new(r"(?i)^\[[^\]]+\]\s*(.+?)\s*-\s*S(\d+)(?:v\d+)?\s*(?:\[.*|\(.*|\.\w+$|.*)$")
            .unwrap(),
        // Season pack without bracket prefix, dash-separated (e.g. "Title - S05")
        Regex::new(r"(?i)^(.+?)\s*-\s*S(\d+)(?:v\d+)?\s*(?:\[.*|\(.*|\.\w+$|.*)$").unwrap(),
        // Season pack without bracket prefix, space-separated (e.g. "Title S06")
        Regex::new(r"(?i)^(.+?)\s+S(\d+)(?:v\d+)?\s*(?:\[.*|\(.*|\.\w+$|.*)$").unwrap(),
        // Spelled-out "Season N", dash-separated (e.g. "Test Series - Season 5 Complete")
        Regex::new(r"(?i)^(.+?)\s*-\s*Season\s+(\d+)(?:v\d+)?\s*(?:\[.*|\(.*|\.\w+$|.*)$").unwrap(),
        // Spelled-out "Season N" with comma prefix (e.g. "Show Name, Season 1")
        // MUST come before space-separated Season variant to prevent the comma
        // from being captured as part of the title.
        Regex::new(r"(?i)^(.+?)\s*,\s*Season\s+(\d+)(?:v\d+)?\s*(?:\[.*|\(.*|\.\w+$|.*)$").unwrap(),
        // Spelled-out "Season N", space-separated (e.g. "Show Name Season 1 Complete")
        Regex::new(r"(?i)^(.+?)\s+Season\s+(\d+)(?:v\d+)?\s*(?:\[.*|\(.*|\.\w+$|.*)$").unwrap(),
        // "Complete" without season — treated as complete series pack (all seasons)
        Regex::new(r"(?i)^(?:\[[^\]]+\]\s*)?(.+?)\s+Complete(?:\s|$|\[|\(|\-)").unwrap(),
        // [BATCH] / [Batch] suffix — common among anime release groups (MockGroup, etc.)
        Regex::new(r"(?i)^(?:\[[^\]]+\]\s*)?(.+?)\s+S(\d+).*?\[batch\]").unwrap(),
    ]
});
