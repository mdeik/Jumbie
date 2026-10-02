use super::*;
use crate::config::GeneralConfig;

#[test]
fn test_validate_download_link() {
    assert!(validate_download_link("magnet:?xt=urn:btih:hash").is_ok());
    assert!(validate_download_link("http://example.com").is_ok());
    assert!(validate_download_link("sftp://server/file").is_ok());
    assert!(validate_download_link("file:///path/to/file").is_ok());

    assert!(validate_download_link("").is_err());
    assert!(validate_download_link("not a link").is_err());
}

#[test]
fn test_validate_name() {
    // Valid names
    assert!(validate_name("1080p", "Quality name", 100).is_ok());
    assert!(validate_name("x265 HEVC", "Display name", 100).is_ok());

    // Empty / whitespace-only (delegates to validate_not_empty)
    assert!(validate_name("", "Name", 100).is_err());
    assert!(validate_name("   ", "Name", 100).is_err());

    // Exceeds max length
    assert!(validate_name(&"a".repeat(101), "Name", 100).is_err());
    assert!(validate_name(&"a".repeat(256), "Name", 255).is_err());

    // Control characters
    assert!(validate_name("Name\nWithNewline", "Name", 100).is_err());
    assert!(validate_name("Name\tWithTab", "Name", 100).is_err());
    assert!(validate_name("Name\x00Null", "Name", 100).is_err());

    // Respects custom max_len
    assert!(validate_name("short", "Name", 10).is_ok());
    assert!(validate_name("this is too long", "Name", 10).is_err());
}

#[test]
fn test_validate_tag() {
    // Valid tags
    assert!(validate_tag("1080p").is_ok());
    assert!(validate_tag("x264").is_ok());
    assert!(validate_tag("HEVC").is_ok());
    assert!(validate_tag("BluRay").is_ok());

    // Empty / whitespace-only
    assert!(validate_tag("").is_err());
    assert!(validate_tag("   ").is_err());

    // Exceeds max length (50)
    assert!(validate_tag(&"a".repeat(51)).is_err());
    assert!(validate_tag(&"a".repeat(50)).is_ok());

    // Control characters
    assert!(validate_tag("tag\nwith\nnewline").is_err());
    assert!(validate_tag("tag\x00null").is_err());
}

#[test]
fn test_validate_alias() {
    // Valid aliases
    assert!(validate_alias("Test Series").is_ok());
    assert!(validate_alias("TST").is_ok());
    assert!(validate_alias("A very long alternative title for matching purposes").is_ok());

    // Empty / whitespace-only — allowed; blank lines in textarea are
    // filtered out at search time via `has_non_empty` / `non_empty_strs`.
    assert!(validate_alias("").is_ok());
    assert!(validate_alias("   ").is_ok());

    // Exceeds max length (255)
    assert!(validate_alias(&"a".repeat(256)).is_err());
    assert!(validate_alias(&"a".repeat(255)).is_ok());

    // Control characters
    assert!(validate_alias("Alias\nWithNewline").is_err());
    assert!(validate_alias("Alias\tWithTab").is_err());
    assert!(validate_alias("Alias\x00Null").is_err());

    // Source-prefixed aliases (@slug:alias)

    // Valid source-prefixed alias
    assert!(validate_alias("@nyaa-anime:Test Series").is_ok());
    assert!(validate_alias("@my-source:Show Name").is_ok());
    assert!(validate_alias("@abc123:Title").is_ok());

    // Empty slug (@:alias) — invalid
    assert!(validate_alias("@:alias").is_err());

    // Empty text after colon (@slug:) — invalid
    assert!(validate_alias("@nyaa:").is_err());

    // Missing colon after @ — invalid
    assert!(validate_alias("@nyaa").is_err());
    assert!(validate_alias("@nyaa anime").is_err());

    // Slug with uppercase — invalid (must be [a-z0-9-])
    assert!(validate_alias("@NYAA:Test Series").is_err());
    assert!(validate_alias("@Nyaa-Anime:Show").is_err());

    // Slug with special characters — invalid
    assert!(validate_alias("@my_source:Show").is_err());
    assert!(validate_alias("@my.source:Show").is_err());
    assert!(validate_alias("@my source:Show").is_err());

    // @ in the middle is NOT treated as prefix — still valid as plain alias
    assert!(validate_alias("A@mozon: Apple").is_ok());
}

#[test]
fn test_validate_plugin_name() {
    // Valid names
    assert!(validate_plugin_name("Nyaa").is_ok());
    assert!(validate_plugin_name("Nyaa Anime").is_ok());
    assert!(validate_plugin_name("My Source 2024").is_ok());
    assert!(validate_plugin_name("A").is_ok());

    // Empty
    assert!(validate_plugin_name("").is_err());
    assert!(validate_plugin_name("   ").is_err());

    // Too long
    assert!(validate_plugin_name(&"a".repeat(101)).is_err());
    assert!(validate_plugin_name(&"a".repeat(100)).is_ok());

    // Valid characters — hyphens and underscores are now allowed
    assert!(validate_plugin_name("My_Source").is_ok()); // underscore
    assert!(validate_plugin_name("my-source").is_ok()); // hyphen
    assert!(validate_plugin_name("Nyaa-Anime_HD").is_ok()); // both

    // Invalid characters — only [a-zA-Z0-9 _-] allowed
    assert!(validate_plugin_name("Source!").is_err()); // punctuation
    assert!(validate_plugin_name("Source.2024").is_err()); // dot
    assert!(validate_plugin_name("Source/2024").is_err()); // slash
    assert!(validate_plugin_name("Source 2024!").is_err()); // mixed

    // Control characters
    assert!(validate_plugin_name("Source\nName").is_err());
    assert!(validate_plugin_name("Source\tName").is_err());
}

#[test]
fn test_validate_name_control_char_message() {
    // Verify the error message mentions "control characters"
    let result = validate_name("Bad\x01Name", "Test name", 100);
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.0.contains("control characters"));
}

#[test]
fn test_validate_name_too_long_message() {
    // Verify the error message mentions the max length
    let result = validate_name(&"a".repeat(101), "Test name", 100);
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.0.contains("too long"));
    assert!(err.0.contains("100"));
}

// Template validation: new syntax

#[test]
fn test_validate_template_conditional_block() {
    let vars = ["series", "season", "episode", "title"];
    assert!(validate_template("${series}?{ - ${title}}", &vars, false).is_ok());
}

#[test]
fn test_validate_template_conditional_block_unknown_var() {
    let vars = ["series", "season"];
    // unknown var inside conditional block should be caught
    let result = validate_template("${series}?{ - ${unknown}}", &vars, false);
    assert!(result.is_err());
    assert!(result.unwrap_err().0.contains("unknown"));
}

#[test]
fn test_validate_template_conditional_multiple_vars() {
    let vars = ["series", "season", "episode", "title", "release_year"];
    assert!(validate_template("${series}?{ (${release_year}) ${title}}", &vars, false).is_ok());
}

#[test]
fn test_validate_template_conditional_unbalanced() {
    let vars = ["series", "title"];
    // ?{ with no closing }
    let result = validate_template("${series}?{ - ${title}", &vars, false);
    assert!(result.is_err());
    assert!(result.unwrap_err().0.contains("Unbalanced"));
}

#[test]
fn test_validate_template_default_value() {
    let vars = ["title"];
    assert!(validate_template("${title:-Untitled}", &vars, false).is_ok());
    assert!(validate_template("${title:-}", &vars, false).is_ok());
}

#[test]
fn test_validate_template_default_value_unknown() {
    let vars = ["series"];
    let result = validate_template("${unknown:-default}", &vars, false);
    assert!(result.is_err());
    assert!(result.unwrap_err().0.contains("unknown"));
}

#[test]
fn test_validate_template_crop_syntax() {
    let vars = ["series", "title"];
    assert!(validate_template("${series:<10}", &vars, false).is_ok());
    assert!(validate_template("${title:<20:…}", &vars, false).is_ok());
    assert!(validate_template("${series:<5:}", &vars, false).is_ok());
}

#[test]
fn test_validate_template_replace_syntax() {
    let vars = ["series"];
    assert!(validate_template("${series/ /_}", &vars, false).is_ok());
    assert!(validate_template("${series// /}", &vars, false).is_ok());
}

#[test]
fn test_validate_template_combined_syntax() {
    let vars = ["series", "season", "episode", "title"];
    assert!(
        validate_template(
            "${series}?{ - ${title:-No Title:<15}} S${season:02}E${episode:02}",
            &vars,
            false
        )
        .is_ok()
    );
}

#[test]
fn test_validate_template_brace_balance_with_conditional() {
    let vars = ["series", "title"];
    // A } that closes a conditional should not count as a variable }
    assert!(validate_template("pre?{${title}}post", &vars, false).is_ok());
    // Extra } after everything should fail
    let result = validate_template("${series}}", &vars, false);
    assert!(result.is_err());
    assert!(result.unwrap_err().0.contains("Unexpected closing"));
}

#[test]
fn test_validate_template_double_dollar() {
    let vars = ["series"];
    assert!(validate_template("$${series}", &vars, false).is_ok());
    assert!(validate_template("Price: $$5", &vars, false).is_ok());
}

#[test]
fn test_validate_template_escaped_and_real() {
    let vars = ["series"];
    assert!(validate_template("$${series}: ${series}", &vars, false).is_ok());
}

#[test]
fn test_validate_template_standalone_braces() {
    let vars = ["series"];
    // Standalone { and } should not be treated as variable/conditional syntax
    assert!(validate_template("{not a var}", &vars, false).is_ok());
    assert!(validate_template("{{double}}", &vars, false).is_ok());
}

// Auto-search wanted validation

#[test]
fn test_validate_auto_search_wanted_interval_bounds() {
    // Min boundary
    assert!(
        validate_auto_search_wanted_interval(GeneralConfig::AUTO_SEARCH_WANTED_INTERVAL_MIN)
            .is_ok()
    );
    assert!(
        validate_auto_search_wanted_interval(GeneralConfig::AUTO_SEARCH_WANTED_INTERVAL_MIN - 1)
            .is_err()
    );

    // Max boundary
    assert!(
        validate_auto_search_wanted_interval(GeneralConfig::AUTO_SEARCH_WANTED_INTERVAL_MAX)
            .is_ok()
    );
    assert!(
        validate_auto_search_wanted_interval(GeneralConfig::AUTO_SEARCH_WANTED_INTERVAL_MAX + 1)
            .is_err()
    );

    // Within range
    assert!(validate_auto_search_wanted_interval(30).is_ok());
    assert!(validate_auto_search_wanted_interval(60).is_ok());
    assert!(validate_auto_search_wanted_interval(1440).is_ok());
}

#[test]
fn test_validate_auto_search_wanted_min_wait_bounds() {
    // Below minimum
    assert!(validate_auto_search_wanted_min_wait(0).is_err());

    // At minimum
    assert!(
        validate_auto_search_wanted_min_wait(GeneralConfig::AUTO_SEARCH_WANTED_MIN_WAIT_MIN)
            .is_ok()
    );

    // Normal values
    assert!(validate_auto_search_wanted_min_wait(30).is_ok());
    assert!(validate_auto_search_wanted_min_wait(120).is_ok());
    assert!(validate_auto_search_wanted_min_wait(1440).is_ok());
}

#[test]
fn test_validate_auto_search_wanted_max_age_days_bounds() {
    // Zero is valid (disabled)
    assert!(validate_auto_search_wanted_max_age_days(0).is_ok());

    // At maximum
    assert!(
        validate_auto_search_wanted_max_age_days(
            GeneralConfig::AUTO_SEARCH_WANTED_MAX_AGE_DAYS_MAX
        )
        .is_ok()
    );

    // Above maximum
    assert!(
        validate_auto_search_wanted_max_age_days(
            GeneralConfig::AUTO_SEARCH_WANTED_MAX_AGE_DAYS_MAX + 1
        )
        .is_err()
    );

    // Normal values
    assert!(validate_auto_search_wanted_max_age_days(7).is_ok());
    assert!(validate_auto_search_wanted_max_age_days(14).is_ok());
    assert!(validate_auto_search_wanted_max_age_days(30).is_ok());
}

#[test]
fn test_validate_auto_search_wanted_window_ok() {
    // max_age=0 (disabled) → no window check, always ok
    assert!(validate_auto_search_wanted_window(120, 0).is_ok());
    assert!(validate_auto_search_wanted_window(10080, 0).is_ok());

    // min_wait exactly equals max_age in minutes
    assert!(validate_auto_search_wanted_window(1440, 1).is_ok());

    // min_wait well within window
    assert!(validate_auto_search_wanted_window(60, 14).is_ok());
    assert!(validate_auto_search_wanted_window(120, 7).is_ok());
    assert!(validate_auto_search_wanted_window(5, 1).is_ok());
}

#[test]
fn test_validate_auto_search_wanted_window_contradictory() {
    // min_wait (1 day = 1440 min) exceeds max_age (0.5 days = 720 min) — impossible
    let result = validate_auto_search_wanted_window(1440, 1);
    assert!(
        result.is_ok(),
        "1 day wait vs 1 day max_age should be exactly equal"
    );

    // min_wait (2 days = 2880 min) exceeds max_age (1 day = 1440 min)
    let result = validate_auto_search_wanted_window(2880, 1);
    assert!(
        result.is_err(),
        "2 day wait vs 1 day max_age should be contradictory"
    );

    // Verify the error message is descriptive
    let err = result.unwrap_err();
    assert!(
        err.0.contains("window is empty"),
        "Error should mention window: {}",
        err.0
    );
    assert!(
        err.0.contains("2880"),
        "Error should include min_wait value: {}",
        err.0
    );

    // Large gap
    let result = validate_auto_search_wanted_window(10080, 3); // 7 days vs 3 days
    assert!(
        result.is_err(),
        "7 day wait vs 3 day max_age should be contradictory"
    );

    // Extreme: 1 year wait vs 1 day max_age
    assert!(validate_auto_search_wanted_window(525600, 1).is_err());
}

#[test]
fn test_validate_plugin_version_valid_semver() {
    assert!(validate_plugin_version("1.0.0").is_ok());
    assert!(validate_plugin_version("0.0.1").is_ok());
    assert!(validate_plugin_version("999.999.999").is_ok());
    assert!(validate_plugin_version("2.1.0-beta").is_err()); // no pre-release
    assert!(validate_plugin_version("1.0").is_err()); // two segments
    assert!(validate_plugin_version("1").is_err()); // one segment
    assert!(validate_plugin_version("").is_err()); // empty
    assert!(validate_plugin_version("unknown").is_ok()); // registry default
}

#[test]
fn test_validate_plugin_info_valid() {
    let info = crate::plugin::PluginTypeInfo {
        display_name: "My Plugin".to_string(),
        version: "1.2.3".to_string(),
        author: "developer".to_string(),
        description: "A useful plugin".to_string(),
        capabilities: vec![],
        supported_protocols: None,
        series_identifier_label: None,
        series_identifier_placeholder: None,
        rate_limit: None,
        supports_test: false,
    };
    assert!(validate_plugin_info(&info).is_ok());
}

#[test]
fn test_validate_plugin_info_rejects_reserved_author() {
    for reserved in [
        "system", "System", "SYSTEM", "internal", "Internal", "INTERNAL", "Jumbie", "jumbie",
        "JUMBIE",
    ] {
        let info = crate::plugin::PluginTypeInfo {
            display_name: "My Plugin".to_string(),
            version: "1.0.0".to_string(),
            author: reserved.to_string(),
            description: "".to_string(),
            capabilities: vec![],
            supported_protocols: None,
            series_identifier_label: None,
            series_identifier_placeholder: None,
            rate_limit: None,
            supports_test: false,
        };
        assert!(
            validate_plugin_info(&info).is_err(),
            "Author '{}' should be rejected",
            reserved
        );
    }
}

#[test]
fn test_validate_plugin_info_rejects_empty_author() {
    let info = crate::plugin::PluginTypeInfo {
        display_name: "My Plugin".to_string(),
        version: "1.0.0".to_string(),
        author: "".to_string(),
        description: "".to_string(),
        capabilities: vec![],
        supported_protocols: None,
        series_identifier_label: None,
        series_identifier_placeholder: None,
        rate_limit: None,
        supports_test: false,
    };
    assert!(validate_plugin_info(&info).is_err());
}

#[test]
fn test_validate_plugin_info_rejects_bad_version() {
    let info = crate::plugin::PluginTypeInfo {
        display_name: "My Plugin".to_string(),
        version: "bad".to_string(),
        author: "developer".to_string(),
        description: "".to_string(),
        capabilities: vec![],
        supported_protocols: None,
        series_identifier_label: None,
        series_identifier_placeholder: None,
        rate_limit: None,
        supports_test: false,
    };
    assert!(validate_plugin_info(&info).is_err());
}

// MediaEntry validation tests

#[test]
fn test_validate_media_entry_valid() {
    use crate::types::media::MediaEntry;
    use crate::validation::plugin_data::Validate;

    let entry = MediaEntry {
        title: "My Show S01E01 1080p".to_string(),
        source: "nyaa".to_string(),
        size: Some(500_000_000),
        seeders: Some(100),
        link: Some("https://example.com/download".to_string()),
        ..MediaEntry::default()
    };
    assert!(entry.validate().is_ok());
}

#[test]
fn test_validate_media_entry_empty_title() {
    use crate::types::media::MediaEntry;
    use crate::validation::plugin_data::Validate;

    let entry = MediaEntry {
        title: "".to_string(),
        source: "nyaa".to_string(),
        ..MediaEntry::default()
    };
    assert!(entry.validate().is_err());
}

#[test]
fn test_validate_media_entry_empty_source() {
    use crate::types::media::MediaEntry;
    use crate::validation::plugin_data::Validate;

    let entry = MediaEntry {
        title: "Valid Title".to_string(),
        source: "".to_string(),
        ..MediaEntry::default()
    };
    assert!(entry.validate().is_err());
}

#[test]
fn test_validate_media_entry_oversized_size() {
    use crate::types::media::MediaEntry;
    use crate::validation::plugin_data::Validate;

    let entry = MediaEntry {
        title: "Big File".to_string(),
        source: "test".to_string(),
        size: Some(2_000_000_000_000), // 2TB — exceeds 1TB limit
        ..MediaEntry::default()
    };
    assert!(entry.validate().is_err());
}

#[test]
fn test_validate_and_filter_removes_bad_entries() {
    use crate::types::media::MediaEntry;
    use crate::validation::plugin_data::validate_and_filter;

    let good = MediaEntry {
        title: "Good".to_string(),
        source: "test".to_string(),
        ..MediaEntry::default()
    };
    let bad = MediaEntry {
        title: "".to_string(), // empty title
        source: "test".to_string(),
        ..MediaEntry::default()
    };

    let (clean, rejected) = validate_and_filter(vec![good.clone(), bad]);
    assert_eq!(clean.len(), 1);
    assert_eq!(clean[0].title, "Good");
    assert_eq!(rejected.len(), 1);
}

#[test]
fn test_validate_vec_media_entry_collects_all_errors() {
    use crate::types::media::MediaEntry;
    use crate::validation::plugin_data::Validate;

    let bad1 = MediaEntry {
        title: "".to_string(),
        source: "test".to_string(),
        ..MediaEntry::default()
    };
    let bad2 = MediaEntry {
        title: "Title".to_string(),
        source: "".to_string(),
        ..MediaEntry::default()
    };

    let result = vec![bad1, bad2].validate();
    assert!(result.is_err());
    let errors = result.unwrap_err();
    assert!(errors.len() >= 2);
}

#[test]
fn test_validate_all_rejects_batch() {
    use crate::types::media::MediaEntry;
    use crate::validation::plugin_data::validate_all;

    let bad = MediaEntry {
        title: "".to_string(),
        source: "test".to_string(),
        ..MediaEntry::default()
    };

    assert!(validate_all(&[bad]).is_err());
    assert!(validate_all::<MediaEntry>(&[]).is_ok());
}

#[test]
fn test_validate_runtime() {
    assert!(validate_runtime(0).is_ok());
    assert!(validate_runtime(22).is_ok());
    assert!(validate_runtime(480).is_ok());
    assert!(validate_runtime(1440).is_ok()); // 24 hours
    assert!(validate_runtime(-1).is_err());
    assert!(validate_runtime(1441).is_err());
}

#[test]
fn test_validate_max_length() {
    assert!(validate_max_length("hello", "field", 10).is_ok());
    assert!(validate_max_length("hello world", "field", 5).is_err());
    assert!(validate_max_length("", "field", 10).is_ok());
}

#[test]
fn test_validate_no_control_chars() {
    assert!(validate_no_control_chars("hello", "field").is_ok());
    assert!(validate_no_control_chars("hello\nworld", "field").is_err());
    assert!(validate_no_control_chars("hello\0world", "field").is_err());
    assert!(validate_no_control_chars("", "field").is_ok());
}

#[test]
fn test_validate_rfc3339_timestamp() {
    // Explicit offset (`Z` or ±HH:MM) is required.
    assert!(validate_rfc3339_timestamp("2026-06-18T20:00:00Z", "t").is_ok());
    assert!(validate_rfc3339_timestamp("2026-06-18T20:00:00+00:00", "t").is_ok());
    assert!(validate_rfc3339_timestamp("2026-06-18T20:30:00+09:00", "t").is_ok());

    // Zone-less, date-only, empty, and garbage are all rejected.
    assert!(validate_rfc3339_timestamp("2026-06-18T20:00:00", "t").is_err());
    assert!(validate_rfc3339_timestamp("2026-06-18 20:00:00", "t").is_err());
    assert!(validate_rfc3339_timestamp("2026-06-18", "t").is_err());
    assert!(validate_rfc3339_timestamp("", "t").is_err());
    assert!(validate_rfc3339_timestamp("not-a-date", "t").is_err());
}
