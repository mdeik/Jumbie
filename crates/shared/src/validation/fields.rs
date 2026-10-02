use regex::Regex;
use std::fmt;

// Shared size/length constants — SSoT for their limits. Every validation layer
// (bridge, DB gate, API) references these; never hardcode them.

/// Max length for any provider identifier (episode unique_id, series ID, plugin ID).
/// Provider IDs are typically numeric (TVDB: 6 digits) or short slugs (TVMaze: ~50).
/// 512 is generous while still rejecting multi-kilobyte garbage inputs.
pub const MAX_ID_LENGTH: usize = 512;

/// Max length for a source plugin's display name (e.g. "nyaa", "basic_rss").
pub const MAX_SOURCE_LENGTH: usize = 200;

/// Max length for a client-reported failure reason (e.g. a download client's
/// error text). Generous for any real client message, but bounded so a
/// misbehaving plugin can't exhaust resources through the DB or UI.
pub const MAX_FAILURE_REASON_LENGTH: usize = 1024;

/// Max length for a download client's status token (e.g. `"downloading"`).
/// Tokens are short, so this only rejects absurd plugin output.
pub const MAX_STATUS_LENGTH: usize = 256;

/// Max length for a plugin-provided human-readable message (e.g. a connection
/// test result). Long enough for a wrapped client error, bounded for display.
pub const MAX_MESSAGE_LENGTH: usize = 4096;

#[derive(Debug, Clone)]
pub struct ValidationError(pub String);

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for ValidationError {}

/// Validate that a string is non-empty after trimming.
///
/// Used as a building block by other validators. Empty trimmed strings
/// are almost never valid input in the contexts where this is called
/// (titles, names, labels).
pub fn validate_not_empty(value: &str, field_name: &str) -> Result<(), ValidationError> {
    if value.trim().is_empty() {
        Err(ValidationError(format!("{} cannot be empty", field_name)))
    } else {
        Ok(())
    }
}

/// Validate a name with an optional maximum length constraint.
///
/// Rejects empty names, enforces the length limit, and rejects control characters.
/// SSoT for name-length validation across all entity types (quality names, profile
/// names, display names, etc.).
pub fn validate_name(value: &str, field_name: &str, max_len: usize) -> Result<(), ValidationError> {
    validate_not_empty(value, field_name)?;

    if value.len() > max_len {
        return Err(ValidationError(format!(
            "{} is too long (max {} characters)",
            field_name, max_len
        )));
    }

    if value.chars().any(|c| c.is_control()) {
        return Err(ValidationError(format!(
            "{} cannot contain control characters",
            field_name
        )));
    }

    Ok(())
}

/// Validate a plugin's user-defined `name` field.
///
/// Plugin names are limited to `[a-zA-Z0-9 _-]` (alphanumeric, spaces,
/// underscores, and hyphens) so they can be reliably converted to a slug
/// for use in alias prefixes (e.g. `@nyaa-anime:Test Series`).
/// Underscores are allowed for readability but will be converted to hyphens
/// when the name is turned into a slug.
pub fn validate_plugin_name(name: &str) -> Result<(), ValidationError> {
    validate_not_empty(name, "Plugin name")?;

    if name.len() > 100 {
        return Err(ValidationError(
            "Plugin name is too long (max 100 characters)".to_string(),
        ));
    }

    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == ' ' || c == '-' || c == '_')
    {
        return Err(ValidationError(
            "Plugin name can only contain letters, numbers, spaces, hyphens, and underscores"
                .to_string(),
        ));
    }

    Ok(())
}

/// Validate a plugin version string enforces semver `X.Y.Z` format.
///
/// Built-in plugins version independently (each declares its own version),
/// and external plugins MUST declare a valid semver version for compatibility
/// tracking. The format is intentionally strict — no pre-release suffixes, no
/// build metadata. Every external plugin version is a concrete release.
pub fn validate_plugin_version(version: &str) -> Result<(), ValidationError> {
    validate_not_empty(version, "Plugin version")?;

    if version == "unknown" {
        return Ok(()); // registry default fallback
    }

    if !version.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return Err(ValidationError(
            "Plugin version must be a semver string (digits and dots only)".to_string(),
        ));
    }

    let parts: Vec<&str> = version.split('.').collect();
    if parts.len() != 3 || parts.iter().any(|p| p.is_empty()) {
        return Err(ValidationError(
            "Plugin version must be in X.Y.Z format (e.g. 1.0.0)".to_string(),
        ));
    }

    Ok(())
}

/// Validate a plugin's self-reported `PluginTypeInfo` before the backend trusts it.
///
/// The backend owns the identity fields `plugin_id` and `instance_id` (derived by the
/// backend, cleared from plugin-supplied info via `sanitize_plugin_info`). Everything
/// else here is plugin-declared and honored: `capabilities` drive capability-based
/// routing (any capability can be claimed), and `rate_limit` is honored for throttling
/// (`None` skips throttling). Reserved authors (`system`, `internal`, `Jumbie`) are
/// rejected.
pub fn validate_plugin_info(info: &crate::plugin::PluginTypeInfo) -> Result<(), ValidationError> {
    // Display name
    if info.display_name.trim().is_empty() {
        return Err(ValidationError(
            "Plugin display_name cannot be empty".to_string(),
        ));
    }
    if info.display_name.len() > 100 {
        return Err(ValidationError(
            "Plugin display_name is too long (max 100 characters)".to_string(),
        ));
    }
    if info.display_name.chars().any(|c| c.is_control()) {
        return Err(ValidationError(
            "Plugin display_name cannot contain control characters".to_string(),
        ));
    }

    // Author
    if info.author.is_empty() {
        return Err(ValidationError("Plugin author cannot be empty".to_string()));
    }
    if info.author.len() > 100 {
        return Err(ValidationError(
            "Plugin author is too long (max 100 characters)".to_string(),
        ));
    }
    if info.author.chars().any(|c| c.is_control()) {
        return Err(ValidationError(
            "Plugin author cannot contain control characters".to_string(),
        ));
    }
    if info.author.eq_ignore_ascii_case("system")
        || info.author.eq_ignore_ascii_case("internal")
        || info
            .author
            .eq_ignore_ascii_case(crate::plugin::JUMBIE_AUTHOR)
    {
        return Err(ValidationError(format!(
            "Plugin author '{}' is reserved for built-in plugins",
            info.author
        )));
    }

    // Version
    validate_plugin_version(&info.version)?;

    // Description
    if info.description.len() > 1000 {
        return Err(ValidationError(
            "Plugin description is too long (max 1000 characters)".to_string(),
        ));
    }

    // Series identifier fields
    if let Some(ref label) = info.series_identifier_label {
        if label.is_empty() {
            return Err(ValidationError(
                "Plugin series_identifier_label cannot be empty".to_string(),
            ));
        }
        if label.len() > 100 {
            return Err(ValidationError(
                "Plugin series_identifier_label is too long (max 100 characters)".to_string(),
            ));
        }
    }
    // series_identifier_placeholder is free text — no constraint beyond sanity
    if let Some(ref placeholder) = info.series_identifier_placeholder
        && placeholder.len() > 200
    {
        return Err(ValidationError(
            "Plugin series_identifier_placeholder is too long (max 200 characters)".to_string(),
        ));
    }

    // Rate limit
    if let Some(ref rl) = info.rate_limit {
        if rl.requests_per_minute == 0 {
            return Err(ValidationError(
                "Plugin rate_limit.requests_per_minute must be > 0".to_string(),
            ));
        }
        if rl.burst == 0 {
            return Err(ValidationError(
                "Plugin rate_limit.burst must be > 0".to_string(),
            ));
        }
        if rl.requests_per_minute > 1_000_000 {
            return Err(ValidationError(
                "Plugin rate_limit.requests_per_minute is unreasonably high (max 1,000,000)"
                    .to_string(),
            ));
        }
        if rl.burst > 1_000_000 {
            return Err(ValidationError(
                "Plugin rate_limit.burst is unreasonably high (max 1,000,000)".to_string(),
            ));
        }
    }

    Ok(())
}

/// Validate a single quality tag string.
///
/// Tags are short identifiers used for matching releases (e.g. "1080p", "x264");
/// must be non-empty, at most 50 characters, and free of control characters.
pub fn validate_tag(tag: &str) -> Result<(), ValidationError> {
    if tag.trim().is_empty() {
        return Err(ValidationError("Tag cannot be empty".to_string()));
    }

    if tag.len() > 50 {
        return Err(ValidationError(
            "Tag is too long (max 50 characters)".to_string(),
        ));
    }

    if tag.chars().any(|c| c.is_control()) {
        return Err(ValidationError(
            "Tag cannot contain control characters".to_string(),
        ));
    }

    Ok(())
}

/// Validate an alias string.
///
/// Aliases are alternative titles used for regex matching and search queries; they
/// share the 255-character limit of series titles. Control characters are rejected
/// because they break filesystem paths, DB queries, and JSON downstream. An `@slug:`
/// prefix must have a non-empty lowercase-hyphen slug and non-empty text.
pub fn validate_alias(alias: &str) -> Result<(), ValidationError> {
    // Allow empty strings — blank lines in the textarea are filtered
    // out by consumers via `non_empty_strs` / `has_non_empty` at search time.
    // This matches `validate_regex` which also accepts empty patterns.
    if alias.trim().is_empty() {
        return Ok(());
    }

    if alias.len() > 255 {
        return Err(ValidationError(
            "Alias is too long (max 255 characters)".to_string(),
        ));
    }

    if alias.chars().any(|c| c.is_control()) {
        return Err(ValidationError(
            "Alias cannot contain control characters".to_string(),
        ));
    }

    // If the alias starts with '@', validate the source-prefix format.
    if let Some(rest) = alias.strip_prefix('@') {
        if let Some(colon_pos) = rest.find(':') {
            let slug = &rest[..colon_pos];
            let text = &rest[colon_pos + 1..];

            if slug.is_empty() {
                return Err(ValidationError(
                    "Alias with '@' prefix must have a non-empty slug before ':'".to_string(),
                ));
            }
            if text.is_empty() {
                return Err(ValidationError(
                    "Alias with '@' prefix must have non-empty text after ':'".to_string(),
                ));
            }
            if !slug
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            {
                return Err(ValidationError(
                    "Alias slug (between '@' and ':') must only contain lowercase letters, digits, and hyphens"
                        .to_string(),
                ));
            }
        } else {
            return Err(ValidationError(
                "Alias starting with '@' must use the format @slug:alias (missing ':')".to_string(),
            ));
        }
    }

    Ok(())
}

/// Maximum length for any title or name field (10,000 chars).
///
/// This is a sanity bound, not a practical limit — no real series title,
/// episode title, or alias approaches 10K chars.  The only purpose is
/// preventing resource exhaustion from a plugin returning an absurdly long
/// string.  The DB schema uses TEXT (no explicit limit), so there is no
/// storage-level constraint.
///
/// Control characters are separately rejected because they can corrupt
/// JSON serialization, terminal output, and filesystem paths.
pub const MAX_TITLE_LENGTH: usize = 10_000;

/// Validate a series or episode title.
///
/// Control characters are rejected because they can cause issues in
/// filesystem paths, database queries, and JSON serialization downstream.
/// The length limit is a sanity bound (see [`MAX_TITLE_LENGTH`]) — no real
/// title should approach it.
pub fn validate_title(name: &str) -> Result<(), ValidationError> {
    validate_not_empty(name, "title")?;

    if name.len() > MAX_TITLE_LENGTH {
        return Err(ValidationError(format!(
            "Title is too long (max {} characters)",
            MAX_TITLE_LENGTH
        )));
    }

    if name.chars().any(|c| c.is_control()) {
        return Err(ValidationError(
            "Title cannot contain control characters".to_string(),
        ));
    }

    Ok(())
}

/// Parse and validate a season number from a string.
///
/// Negative seasons are rejected (no semantic meaning in TV numbering); the upper
/// bound is a sanity cap. Episode-count limits live in `validate_episode_number`.
pub fn validate_season_number(season: &str) -> Result<i32, ValidationError> {
    const MAX_SEASON: i32 = 10000;

    let num = season
        .parse::<i32>()
        .map_err(|_| ValidationError("Season must be a valid number".to_string()))?;

    if num < 0 {
        return Err(ValidationError(
            "Season number cannot be negative".to_string(),
        ));
    }

    if num > MAX_SEASON {
        return Err(ValidationError(format!(
            "Season number {} is too large (max {})",
            num, MAX_SEASON
        )));
    }

    Ok(num)
}

/// Validate an episode number.
///
/// Must be positive and within [`MAX_EPISODE`]. Episode 0 is rejected because
/// numbering starts at 1 in both absolute and season-relative conventions.
pub const MAX_EPISODE: i32 = 10000;

pub fn validate_episode_number(episode: i32) -> Result<(), ValidationError> {
    if episode <= 0 {
        return Err(ValidationError(
            "Episode number must be positive".to_string(),
        ));
    }

    if episode > MAX_EPISODE {
        return Err(ValidationError(format!(
            "Episode number seems unreasonably large (max {})",
            MAX_EPISODE
        )));
    }

    Ok(())
}

/// Validate an episode offset (used for manual +/- corrections).
///
/// Offsets beyond ±10000 would indicate a configuration error rather than
/// a legitimate correction — used to align episode numbering with an indexer.
pub fn validate_episode_offset(offset: i32) -> Result<(), ValidationError> {
    const MAX_OFFSET: i32 = 10000;
    if offset.abs() > MAX_OFFSET {
        return Err(ValidationError(format!(
            "Episode offset seems unreasonably large (max ±{})",
            MAX_OFFSET
        )));
    }

    Ok(())
}

/// Quick check if a URL is valid (empty is allowed for optional fields).
///
/// Separate from `validate_url` because some callers just need a boolean
/// without error handling (e.g., disabling a UI button).
pub fn is_valid_url(url: &str) -> bool {
    if url.trim().is_empty() {
        return true; // Empty is OK (optional field)
    }
    url.starts_with("http://") || url.starts_with("https://")
}

/// Validate a URL with detailed error messages.
///
/// Only http/https is accepted (other schemes would imply capability the app
/// doesn't have); spaces are rejected since valid URLs must percent-encode them.
pub fn validate_url(url: &str) -> Result<(), ValidationError> {
    if url.trim().is_empty() {
        return Err(ValidationError("URL cannot be empty".to_string()));
    }

    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err(ValidationError(
            "URL must start with http:// or https://".to_string(),
        ));
    }

    if url.contains(' ') {
        return Err(ValidationError("URL cannot contain spaces".to_string()));
    }

    Ok(())
}

/// Validates a download link (magnet or URI with scheme).
///
/// Delegates to `protocol::is_likely_link` — SSoT for link detection.
pub fn validate_download_link(link: &str) -> Result<(), ValidationError> {
    if link.trim().is_empty() {
        return Err(ValidationError("Download link cannot be empty".to_string()));
    }

    if crate::protocol::is_likely_link(link) {
        Ok(())
    } else {
        Err(ValidationError(
            "Invalid download link or magnet. Must start with 'magnet:?' or have a protocol (e.g. 'http://', 'sftp://')".to_string(),
        ))
    }
}

/// Quick check if a port number is valid (boolean version).
pub fn is_valid_port(port: u16) -> bool {
    port > 0
}

/// Validate a port number with error message.
///
/// Port 0 is reserved by the OS and should not be used for application services.
pub fn validate_port(port: u16) -> Result<(), ValidationError> {
    if port == 0 {
        return Err(ValidationError("Port cannot be 0".to_string()));
    }
    Ok(())
}

/// Parse a port number from a string, returning both parsed value and validation.
pub fn parse_port(value: &str) -> Result<u16, ValidationError> {
    let port = value
        .parse::<u16>()
        .map_err(|_| ValidationError("Port must be a number".to_string()))?;

    if !is_valid_port(port) {
        return Err(ValidationError(
            "Port must be between 1 and 65535".to_string(),
        ));
    }

    Ok(port)
}

/// Validate a quality profile name.
///
/// Empty is allowed (treated as "not set"). Delegates to `validate_name` so name
/// constraints live in one place.
pub fn validate_quality_profile(profile: &str) -> Result<(), ValidationError> {
    if profile.trim().is_empty() {
        return Ok(());
    }

    validate_name(profile, "Quality profile name", 100)
}

/// Validate a regex pattern string.
///
/// Empty patterns are allowed (optional fields). The validation compiles
/// the regex to catch syntax errors at configuration time rather than at
/// runtime when the pattern is first used for matching.
pub fn validate_regex(pattern: &str) -> Result<(), ValidationError> {
    if pattern.trim().is_empty() {
        return Ok(());
    }

    Regex::new(pattern).map_err(|e| ValidationError(format!("Invalid regex pattern: {}", e)))?;

    Ok(())
}

/// Validate a search query: not empty and ≤ 255 characters.
pub fn validate_search_query(query: &str) -> Result<(), ValidationError> {
    if query.trim().is_empty() {
        return Err(ValidationError("Search query cannot be empty".to_string()));
    }
    if query.len() > 255 {
        return Err(ValidationError(
            "Search query too long (max 255 characters)".to_string(),
        ));
    }
    Ok(())
}

/// Validate that an episode numbers list is not empty.
pub fn validate_episode_numbers_not_empty(count: usize) -> Result<(), ValidationError> {
    if count == 0 {
        Err(ValidationError(
            "At least one episode number must be provided".to_string(),
        ))
    } else {
        Ok(())
    }
}

/// Validate an episode runtime in minutes.
///
/// Caps at 1440 (24 hours) and rejects negative values.
pub fn validate_runtime(runtime: i32) -> Result<(), ValidationError> {
    const MAX_RUNTIME: i32 = 1440;
    if runtime < 0 {
        return Err(ValidationError("Runtime cannot be negative".to_string()));
    }
    if runtime > MAX_RUNTIME {
        return Err(ValidationError(format!(
            "Runtime is too large (max {} minutes)",
            MAX_RUNTIME
        )));
    }
    Ok(())
}

/// Validate that a string does not exceed the given length limit.
///
/// Empty strings are allowed (callers check their own "not empty" constraints).
pub fn validate_max_length(
    value: &str,
    field_name: &str,
    max_len: usize,
) -> Result<(), ValidationError> {
    if value.len() > max_len {
        return Err(ValidationError(format!(
            "{} exceeds maximum length ({} characters)",
            field_name, max_len
        )));
    }
    Ok(())
}

/// Validate that a string contains no control characters.
///
/// Control characters (\x00-\x1F, \x7F) are problematic in DB queries,
/// display rendering, and filesystem paths. Reject them uniformly.
pub fn validate_no_control_chars(value: &str, field_name: &str) -> Result<(), ValidationError> {
    if value.chars().any(|c| c.is_control()) {
        return Err(ValidationError(format!(
            "{} cannot contain control characters",
            field_name
        )));
    }
    Ok(())
}

/// Sanitize a client-reported failure reason (download client error text).
///
/// Trims surrounding whitespace, replaces control characters with spaces (they
/// corrupt JSON, terminal output, and tooltip rendering), and truncates to
/// [`MAX_FAILURE_REASON_LENGTH`] on a char boundary with an ellipsis.
///
/// Unlike most validators this *sanitizes* rather than rejects: the reason is a
/// terminal signal (the download hard-failed), so a cosmetically bad string must
/// still be honored rather than dropped. Returns `None` when nothing meaningful
/// remains, which the caller treats as "no failure reported".
pub fn sanitize_failure_reason(raw: &str) -> Option<String> {
    let flattened: String = raw
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let trimmed = flattened.trim();
    if trimmed.is_empty() {
        return None;
    }

    let mut cleaned = trimmed.to_string();
    if cleaned.len() > MAX_FAILURE_REASON_LENGTH {
        let mut end = MAX_FAILURE_REASON_LENGTH;
        while !cleaned.is_char_boundary(end) {
            end -= 1;
        }
        cleaned.truncate(end);
        cleaned.push('\u{2026}');
    }
    Some(cleaned)
}
