//! Alias and pattern parsing helpers with optional source-plugin prefix.
//!
//! SSoT: every parser of user-defined aliases/patterns (settings editor, import
//! wizard, API endpoints) goes through these functions.
//!
//! # Prefix format
//!
//! The prefix is the plugin instance ID (`plugin.instance_id()`):
//! ```text
//! @a3f8c91e4b2d:Show Name
//! ```
//! Only **source** plugins (FeedProvider capability) can be targeted. A prefix
//! with a downloader or invalid/malformed id matches no source, so the prefixed
//! alias/pattern is skipped entirely.
//!
//! See `PluginManager::get_source_by_id` for the resolution logic.

/// Parsed alias that may include a source plugin prefix.
///
/// Format: `@instance-id:alias` where `instance-id` is the plugin instance ID
/// (from `plugin.instance_id()`).
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedAlias {
    /// The plugin instance ID from the `@instance-id:` prefix (None if no prefix).
    pub source_id: Option<String>,
    /// The actual alias text (without the prefix).
    pub alias: String,
}

/// Parsed regex pattern that may include a source plugin prefix.
///
/// Format: `@instance-id:pattern` where `instance-id` is the plugin instance ID
/// (from `plugin.instance_id()`).
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedPattern {
    /// The plugin instance ID from the `@instance-id:` prefix (None if no prefix).
    pub source_id: Option<String>,
    /// The actual regex pattern (without the prefix).
    pub pattern: String,
}

/// Parse a string that may have a `@id:value` format.
/// Returns `(Option<id>, value)` where `value` is the remainder after stripping the prefix.
fn parse_prefixed(input: &str) -> (Option<String>, String) {
    if let Some(rest) = input.strip_prefix('@')
        && let Some(colon_pos) = rest.find(':')
    {
        let slug = rest[..colon_pos].to_string();
        let text = rest[colon_pos + 1..].to_string();
        if !slug.is_empty() && !text.is_empty() {
            return (Some(slug), text);
        }
    }
    (None, input.to_string())
}

/// Parse an alias that may have a `@id:text` format (where `id` is a plugin
/// instance id or a legacy display-name slug).
///
/// Returns a [`ParsedAlias`] with the source slug extracted (if present).
/// If the string does not start with `@` or the format is invalid, the entire
/// string is treated as the alias with no source slug.
pub fn parse_alias(alias: &str) -> ParsedAlias {
    let (source_id, alias) = parse_prefixed(alias);
    ParsedAlias { source_id, alias }
}

/// Parse a regex pattern that may have a `@slug:pattern` format.
///
/// Returns a [`ParsedPattern`] with the source slug extracted (if present).
/// If the string does not start with `@` or the format is invalid, the entire
/// string is treated as the pattern with no source slug.
pub fn parse_source_pattern(pattern: &str) -> ParsedPattern {
    let (source_id, pattern) = parse_prefixed(pattern);
    ParsedPattern { source_id, pattern }
}

/// Rewrite every `(?P<name>...)` group as a non-capturing `(?:...)`.
///
/// The regex crate rejects the same group name appearing in two alternation
/// branches, and a filter-only match needs no captures, so callers that combine
/// patterns (gate matching, extraction pre-screening) strip the names first.
/// Only `\w+` names are rewritten, matching the regex crate's own rule.
pub fn strip_named_groups(pattern: &str) -> String {
    static NAMED_GROUP: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"\(\?P<\w+>").unwrap());
    NAMED_GROUP.replace_all(pattern, "(?:").to_string()
}

/// Convert a user-defined plugin name to a slug for use in alias/pattern prefixes.
///
/// Plugin names are limited to `[a-zA-Z0-9 _-]`, so this lowercases
/// and replaces spaces and underscores with hyphens.
pub fn plugin_name_to_slug(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            ' ' | '_' => '-',
            _ => c.to_ascii_lowercase(),
        })
        .collect()
}

crate::test_module! {
    // parse_alias tests

    #[test]
    fn test_parse_alias_no_prefix() {
        // Plain alias without any source prefix
        let result = parse_alias("Test Series");
        assert_eq!(result.source_id, None);
                assert_eq!(result.alias, "Test Series");
    }

    #[test]
    fn test_parse_alias_with_source_prefix() {
        // Normal @slug:alias format
        let result = parse_alias("@nyaa-anime:Test Series");
        assert_eq!(result.source_id, Some("nyaa-anime".to_string()));
                assert_eq!(result.alias, "Test Series");
    }

    #[test]
    fn test_parse_alias_removes_prefix_even_when_plugin_not_found() {
        // The prefix is always stripped regardless of whether the plugin exists.
        // The caller decides what to do when the slug doesn't resolve.
        let result = parse_alias("@nonexistent-slug:Test Series");
        assert_eq!(result.source_id, Some("nonexistent-slug".to_string()));
                assert_eq!(result.alias, "Test Series");
    }

    #[test]
    fn test_parse_alias_at_in_middle_is_not_prefix() {
        // "A@mozon: Apple" does NOT start with '@', so it's treated as a plain alias.
        // The '@' and ':' are just regular characters in the alias text.
        let result = parse_alias("A@mozon: Apple");
        assert_eq!(result.source_id, None);
                assert_eq!(result.alias, "A@mozon: Apple");
    }

    #[test]
    fn test_parse_alias_empty_slug_is_not_prefix() {
        // "@:alias" has an empty slug — not valid, treat as plain alias
        let result = parse_alias("@:alias");
        assert_eq!(result.source_id, None);
                assert_eq!(result.alias, "@:alias");
    }

    #[test]
    fn test_parse_alias_empty_text_is_not_prefix() {
        // "@slug:" has empty text after colon — not valid, treat as plain alias
        let result = parse_alias("@nyaa:");
        assert_eq!(result.source_id, None);
                assert_eq!(result.alias, "@nyaa:");
    }

    #[test]
    fn test_parse_alias_missing_colon_is_not_prefix() {
        // "@slug" without colon — not valid, treat as plain alias
        let result = parse_alias("@nyaa");
        assert_eq!(result.source_id, None);
                assert_eq!(result.alias, "@nyaa");
    }

    #[test]
    fn test_parse_alias_multiple_colons() {
        // "@slug:foo:bar" — the first colon separates slug from alias text.
        // The rest of the text (including extra colons) is part of the alias.
        let result = parse_alias("@nyaa:Test Series: Episode 1");
        assert_eq!(result.source_id, Some("nyaa".to_string()));
                assert_eq!(result.alias, "Test Series: Episode 1");
    }

    #[test]
    fn test_parse_alias_numeric_slug() {
        // Slugs can contain digits
        let result = parse_alias("@my2nd-source:Show");
        assert_eq!(result.source_id, Some("my2nd-source".to_string()));
                assert_eq!(result.alias, "Show");
    }

    #[test]
    fn test_parse_alias_slug_with_uppercase() {
        let result = parse_alias("@NYAA-ANIME:Test Series");
        assert_eq!(result.source_id, Some("NYAA-ANIME".to_string()));
                assert_eq!(result.alias, "Test Series");
    }

    #[test]
    fn test_parse_alias_unicode_alias() {
        // Alias text can contain unicode
        let result = parse_alias("@nyaa:進撃の巨人");
        assert_eq!(result.source_id, Some("nyaa".to_string()));
                assert_eq!(result.alias, "進撃の巨人");
    }

    #[test]
    fn test_parse_alias_at_sign_only() {
        // Just "@" with nothing else — not a valid prefix
        let result = parse_alias("@");
        assert_eq!(result.source_id, None);
                assert_eq!(result.alias, "@");
    }

    #[test]
    fn test_parse_alias_empty_string() {
        let result = parse_alias("");
        assert_eq!(result.source_id, None);
                assert_eq!(result.alias, "");
    }

    #[test]
    fn test_parse_alias_with_uuid_prefix() {
        // UUID-format prefix (new SSoT format)
        let uuid = "550e8400-e29b-41d4-a716-446655440000";
        let result = parse_alias(&format!("@{}:Show Name", uuid));
        assert_eq!(result.source_id, Some(uuid.to_string()));
                assert_eq!(result.alias, "Show Name");
    }

    #[test]
    fn test_parse_alias_with_uuid_prefix_long_alias() {
        // UUID prefix with a long alias containing special characters
        let uuid = "a1b2c3d4-e5f6-7890-abcd-ef1234567890";
        let result = parse_alias(&format!(
            "@{}:Mock Series - S01E01 - 1080p [Multi-Subs]",
            uuid
        ));
        assert_eq!(result.source_id, Some(uuid.to_string()));
        assert_eq!(
            result.alias,
            "Mock Series - S01E01 - 1080p [Multi-Subs]"
        );
    }

    // plugin_name_to_slug tests

    #[test]
    fn test_plugin_name_to_slug_simple() {
        assert_eq!(plugin_name_to_slug("Nyaa"), "nyaa");
    }

    #[test]
    fn test_plugin_name_to_slug_with_spaces() {
        assert_eq!(plugin_name_to_slug("Nyaa Anime"), "nyaa-anime");
    }

    #[test]
    fn test_plugin_name_to_slug_multiple_spaces() {
        assert_eq!(plugin_name_to_slug("My  Custom  Source"), "my--custom--source");
    }

    #[test]
    fn test_plugin_name_to_slug_alphanumeric() {
        assert_eq!(plugin_name_to_slug("Source 2024"), "source-2024");
    }

    #[test]
    fn test_plugin_name_to_slug_already_lowercase() {
        assert_eq!(plugin_name_to_slug("already lowercase"), "already-lowercase");
    }

    #[test]
    fn test_plugin_name_to_slug_mixed_case() {
        assert_eq!(plugin_name_to_slug("Basic RSS"), "basic-rss");
    }

    // parse_source_pattern tests

    #[test]
    fn test_parse_source_pattern_no_prefix() {
        // Plain pattern without any source prefix
        let result = parse_source_pattern("1080p");
        assert_eq!(result.source_id, None);
                assert_eq!(result.pattern, "1080p");
    }

    #[test]
    fn test_parse_source_pattern_with_source_prefix() {
        // Normal @slug:pattern format
        let result = parse_source_pattern("@nyaa:1080p");
        assert_eq!(result.source_id, Some("nyaa".to_string()));
                assert_eq!(result.pattern, "1080p");
    }

    #[test]
    fn test_parse_source_pattern_regex_with_special_chars() {
        // Pattern containing regex metacharacters
        let result = parse_source_pattern("@mockfansub:\\[MockFansub\\]");
        assert_eq!(result.source_id, Some("mockfansub".to_string()));
                assert_eq!(result.pattern, "\\[MockFansub\\]");
    }

    #[test]
    fn test_parse_source_pattern_empty_slug_is_not_prefix() {
        // @:pattern has empty slug — not valid, treat as plain
        let result = parse_source_pattern("@:pattern");
        assert_eq!(result.source_id, None);
                assert_eq!(result.pattern, "@:pattern");
    }

    #[test]
    fn test_parse_source_pattern_empty_text_is_not_prefix() {
        // @slug: has empty text — not valid, treat as plain
        let result = parse_source_pattern("@nyaa:");
        assert_eq!(result.source_id, None);
                assert_eq!(result.pattern, "@nyaa:");
    }

    #[test]
    fn test_parse_source_pattern_missing_colon_is_not_prefix() {
        // @slug without colon — not valid, treat as plain
        let result = parse_source_pattern("@nyaa");
        assert_eq!(result.source_id, None);
                assert_eq!(result.pattern, "@nyaa");
    }

    #[test]
    fn test_parse_source_pattern_at_in_middle_is_not_prefix() {
        // Does not start with @ — plain pattern
        let result = parse_source_pattern("pattern@source");
        assert_eq!(result.source_id, None);
                assert_eq!(result.pattern, "pattern@source");
    }

    #[test]
    fn test_parse_source_pattern_empty_string() {
        let result = parse_source_pattern("");
        assert_eq!(result.source_id, None);
                assert_eq!(result.pattern, "");
    }

    #[test]
    fn test_parse_source_pattern_with_uuid_prefix() {
        // UUID-format prefix for regex patterns
        let uuid = "550e8400-e29b-41d4-a716-446655440000";
        let result = parse_source_pattern(&format!("@{}:1080p", uuid));
        assert_eq!(result.source_id, Some(uuid.to_string()));
                assert_eq!(result.pattern, "1080p");
    }

    #[test]
    fn test_parse_source_pattern_uuid_with_complex_regex() {
        // UUID prefix with a complex regex pattern
        let uuid = "a1b2c3d4-e5f6-7890-abcd-ef1234567890";
        let result =
            parse_source_pattern(&format!("@{}:\\[MockFansub\\] .* 1080p", uuid));
        assert_eq!(result.source_id, Some(uuid.to_string()));
                assert_eq!(result.pattern, "\\[MockFansub\\] .* 1080p");
    }

    #[test]
    fn test_parse_source_pattern_mixed_case_slug() {
        let result = parse_source_pattern("@Nyaa:1080p");
        assert_eq!(result.source_id, Some("Nyaa".to_string()));
                assert_eq!(result.pattern, "1080p");
    }

    #[test]
    fn test_parse_source_pattern_complex_regex() {
        let result = parse_source_pattern("@tokyo-toshokan:\\[Anime\\].*720p");
        assert_eq!(result.source_id, Some("tokyo-toshokan".to_string()));
                assert_eq!(result.pattern, "\\[Anime\\].*720p");
    }
}
