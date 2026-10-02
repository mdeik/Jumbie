use super::*;
use std::collections::HashMap;

#[test]
fn test_normalize_title() {
    assert_eq!(
        normalize_title("Mock Series: Subtitle - Second Half"),
        "mockseriessubtitlesecondhalf"
    );
    assert_eq!(
        normalize_title("Another Mock Series: The Beginning"),
        "anothermockseriesthebeginning"
    );
    // Collapses spaces and removes brackets
    assert_eq!(
        normalize_title("  [SomeGroup]  My Title  (2024) "),
        "somegroupmytitle2024"
    );

    // Pure symbols title (edge case, falls back to hex)
    let symbols_only = "!!!???";
    let norm = normalize_title(symbols_only);
    assert!(
        norm.len() == 16 || norm.len() == 32 || norm.len() == 64,
        "Pure symbols should fallback to a hash: {}",
        norm
    );
    assert_ne!(norm, "");
}

#[test]
fn test_platform_safe_chars_matches_host_os() {
    // Verify the host platform-safe regex matches exactly what's invalid on the
    // current OS — by testing actual match behavior, not pattern string.
    let on_windows = cfg!(target_os = "windows");
    let re = crate::patterns::platform_safe_chars(crate::patterns::PlatformOs::Host);

    // '/' is a path separator on ALL platforms — always matched
    assert!(re.is_match("/"), "/ should be matched on all platforms");

    // Windows-invalid chars: only matched on Windows
    for (label, ch) in &[
        ("backslash", '\\'),
        ("less-than", '<'),
        ("greater-than", '>'),
        ("colon", ':'),
        ("quote", '\"'),
        ("pipe", '|'),
        ("question", '?'),
        ("asterisk", '*'),
    ] {
        let matched = re.is_match(&ch.to_string());
        if on_windows {
            assert!(matched, "'{}' ({}) should be matched on Windows", ch, label);
        } else {
            assert!(
                !matched,
                "'{}' ({}) should NOT be matched on Linux/macOS",
                ch, label
            );
        }
    }

    // Control char \x01 should be matched on all platforms
    assert!(
        re.is_match("\x01"),
        "control char should be matched on all platforms"
    );
}

#[test]
fn test_apply_template() {
    let mut vars = HashMap::new();
    vars.insert("series".to_string(), "Show Name".to_string());
    vars.insert("season".to_string(), "1".to_string());
    vars.insert("episode".to_string(), "5".to_string());
    vars.insert("title".to_string(), "Show Name".to_string());

    // Basic replacement
    assert_eq!(
        apply_template("${series} - S${season}E${episode}", &vars, None),
        "Show Name - S1E5"
    );

    // Zero padding
    assert_eq!(
        apply_template("S${season:02}E${episode:03}", &vars, None),
        "S01E005"
    );

    // A bare width (`:2`) is equivalent to `:02`: both zero-pad.
    assert_eq!(apply_template("${season:2}", &vars, None), "01");

    // Mixed text
    assert_eq!(
        apply_template("Season ${season:02}/${series} - ${episode:02}", &vars, None),
        "Season 01/Show Name - 05"
    );

    // Retain unclosed braces completely
    assert_eq!(apply_template("${unclosed", &vars, None), "${unclosed");

    // Retain invalid variables as explicit string since we are returning them rather than assuming empty
    assert_eq!(apply_template("${missing}", &vars, None), "${missing}");
    assert_eq!(
        apply_template("${invalid_var:02}", &vars, None),
        "${invalid_var:02}"
    );

    // `${year:02}` with a non-numeric value is left unchanged.
    vars.insert("year".to_string(), "test".to_string());
    assert_eq!(apply_template("${year:02}", &vars, None), "test");

    // Padding is opt-in. `:0N` is a fixed minimum width; bare variables are
    // emitted verbatim; `:auto` widens to fit the highest episode (max_episode
    // 150 needs 3 digits).
    let opts = TemplatePadOptions {
        max_episode: 150,
        ..Default::default()
    };
    vars.insert("episode".to_string(), "5".to_string());
    assert_eq!(
        apply_template("S${season}E${episode}", &vars, Some(&opts)),
        "S1E5"
    );
    assert_eq!(
        apply_template("S${season:02}E${episode:02}", &vars, Some(&opts)),
        "S01E05"
    );
    assert_eq!(
        apply_template("S${season:02}E${episode:auto}", &vars, Some(&opts)),
        "S01E005"
    );

    // `:auto` pads each component of a multi-episode range independently.
    vars.insert("episode".to_string(), "5-7".to_string());
    assert_eq!(
        apply_template("E${episode:auto}", &vars, Some(&opts)),
        "E005-007"
    );
    assert_eq!(apply_template("E${episode}", &vars, Some(&opts)), "E5-7");

    // `:auto` has no minimum width: a series topping out at episode 5 stays
    // single-digit.
    let small = TemplatePadOptions {
        max_episode: 5,
        ..Default::default()
    };
    assert_eq!(
        apply_template("E${episode:auto}", &vars, Some(&small)),
        "E5-7"
    );

    // `:autoN` floors the width: `:auto2` never goes below two digits, and
    // `:auto3` still grows with the value when it needs to.
    assert_eq!(
        apply_template("E${episode:auto2}", &vars, Some(&small)),
        "E05-07"
    );
    assert_eq!(
        apply_template("E${episode:auto2}", &vars, Some(&opts)),
        "E005-007"
    );
    assert_eq!(
        apply_template("E${episode:auto}", &vars, Some(&small)),
        "E5-7"
    );

    // `:auto` also applies to season, sizing to the highest season number.
    vars.insert("season".to_string(), "3".to_string());
    let seasons = TemplatePadOptions {
        max_season: 12,
        ..Default::default()
    };
    assert_eq!(
        apply_template("Season ${season:auto}", &vars, Some(&seasons)),
        "Season 03"
    );
    // `:autoN` floors a season's width too (a 5-season series alone is 1 digit).
    let few_seasons = TemplatePadOptions {
        max_season: 5,
        ..Default::default()
    };
    assert_eq!(
        apply_template("Season ${season:auto}", &vars, Some(&few_seasons)),
        "Season 3"
    );
    assert_eq!(
        apply_template("Season ${season:auto2}", &vars, Some(&few_seasons)),
        "Season 03"
    );
}

#[test]
fn test_combine_seasons() {
    assert_eq!(combine_seasons(&[]), Vec::<String>::new());
    assert_eq!(
        combine_seasons(&[
            "1".to_string(),
            "2".to_string(),
            "3".to_string(),
            "6".to_string()
        ]),
        vec!["S01-S03", "S06"]
    );
    assert_eq!(
        combine_seasons(&["1".to_string(), "2".to_string(), "Specials".to_string()]),
        vec!["S01-S02", "SSpecials"]
    );
}

#[test]
fn test_extract_submitter() {
    assert_eq!(
        extract_submitter("[MyGroup] My Awesome Show - 55 (WEB 1080p HEVC EAC-3) [CF20AC5D]")
            .as_deref(),
        Some("MyGroup")
    );
    assert_eq!(
        extract_submitter(
            "My.Awesome.Show.S04.P3.1080p.Blu-Ray.10-Bit.Dual-Audio.TrueHD.x265-FakeGrp"
        )
        .as_deref(),
        Some("FakeGrp")
    );
    assert_eq!(
        extract_submitter(
            "My.Awesome.Show.S04.P3.1080p.Blu-Ray.10-Bit.Dual-Audio.TrueHD.x265-FakeGrp.mkv"
        )
        .as_deref(),
        Some("FakeGrp")
    );
    assert_eq!(
        extract_submitter("Some.Show.S01E01.1080p.WEB-DL.x264-GRP").as_deref(),
        Some("GRP")
    );
    // It shouldn't pick up common tags
    assert_eq!(
        extract_submitter("Show Name - S01E01 - 1080p.mkv").as_deref(),
        None
    );
    // Should handle trailing parenthesized metadata (e.g. `Multi-Subs)` at end)
    assert_eq!(
        extract_submitter(
            "My Awesome Show S01E12 Episode Title 1080p NF WEB-DL DUAL AAC2.0 H 264-MadeUpGrp (Some Metadata Here, Dual-Audio, Multi-Subs)"
        )
        .as_deref(),
        Some("MadeUpGrp")
    );
    // Unicode + ampersand in bracket group name
    assert_eq!(
        extract_submitter(
            "[喵呜茶屋&ABC-Studio] A Fake Show / Fake / Jam 10-bit 1080p HEVC BDRip [S1 Fin]"
        )
        .as_deref(),
        Some("喵呜茶屋&ABC-Studio")
    );
    // Another Pattern 4 trailing-paren variant with longer metadata tail
    assert_eq!(
        extract_submitter(
            "A Fictional Show Paradise 2025 1080p AMZN WEB-DL DUAL DDP2.0 H 264-FakeGrp (A Fictional Show: Paradise, Dual-Audio, Multi-Subs)"
        )
        .as_deref(),
        Some("FakeGrp")
    );
}

// Default value tests

#[test]
fn test_default_value_present() {
    let mut vars = HashMap::new();
    vars.insert("title".to_string(), "Pilot".to_string());
    assert_eq!(apply_template("${title:-Untitled}", &vars, None), "Pilot");
}

#[test]
fn test_default_value_absent() {
    let vars = HashMap::new();
    assert_eq!(
        apply_template("${title:-Untitled}", &vars, None),
        "Untitled"
    );
}

#[test]
fn test_default_value_empty_string_in_map() {
    let mut vars = HashMap::new();
    vars.insert("title".to_string(), "".to_string());
    assert_eq!(
        apply_template("${title:-Fallback}", &vars, None),
        "Fallback"
    );
}

#[test]
fn test_default_value_empty_default() {
    let mut vars = HashMap::new();
    vars.insert("title".to_string(), "Real".to_string());
    assert_eq!(apply_template("${title:-}", &vars, None), "Real");
}

#[test]
fn test_default_value_empty_default_absent() {
    let vars = HashMap::new();
    assert_eq!(apply_template("${title:-}", &vars, None), "");
}

#[test]
fn test_default_value_unknown_var() {
    let vars = HashMap::new();
    assert_eq!(
        apply_template("${unknown:-default}", &vars, None),
        "${unknown:-default}"
    );
}

#[test]
fn test_default_value_unknown_var_in_vars() {
    let mut vars = HashMap::new();
    vars.insert("unknown".to_string(), "real_value".to_string());
    assert_eq!(
        apply_template("${unknown:-default}", &vars, None),
        "real_value"
    );
}

// Crop tests

#[test]
fn test_crop_shorter_than_limit() {
    let mut vars = HashMap::new();
    vars.insert("series".to_string(), "Short".to_string());
    assert_eq!(apply_template("${series:<10}", &vars, None), "Short");
}

#[test]
fn test_crop_exact_limit() {
    let mut vars = HashMap::new();
    vars.insert("series".to_string(), "Exactly10".to_string());
    assert_eq!(apply_template("${series:<10}", &vars, None), "Exactly10");
}

#[test]
fn test_crop_truncates() {
    let mut vars = HashMap::new();
    vars.insert("series".to_string(), "Very Long Series Name".to_string());
    let result = apply_template("${series:<10}", &vars, None);
    assert_eq!(result.chars().count(), 10, "9 chars + '…' = 10 total");
    assert_eq!(result, "Very Long…");
}

#[test]
fn test_crop_with_custom_suffix() {
    let mut vars = HashMap::new();
    vars.insert("series".to_string(), "Long Series Name Here".to_string());
    let result = apply_template("${series:<10:..}", &vars, None);
    assert!(result.ends_with(".."));
    assert_eq!(
        result.chars().count(),
        10,
        "crop 10 with '..' suffix = 10 chars"
    );
}

#[test]
fn test_crop_with_empty_suffix() {
    let mut vars = HashMap::new();
    vars.insert("series".to_string(), "Long Series Name".to_string());
    assert_eq!(apply_template("${series:<10:}", &vars, None), "Long Serie");
}

#[test]
fn test_crop_numeric_value() {
    let mut vars = HashMap::new();
    vars.insert("episode".to_string(), "12345".to_string());
    let result = apply_template("${episode:<3}", &vars, None);
    assert_eq!(result.chars().count(), 3, "crop 3 = 2 chars + '…' suffix");
    assert_eq!(result, "12…");
}

#[test]
fn test_crop_zero_limit() {
    let mut vars = HashMap::new();
    vars.insert("title".to_string(), "Anything".to_string());
    assert_eq!(apply_template("${title:<0}", &vars, None), "…");
}

#[test]
fn test_crop_with_default() {
    let mut vars = HashMap::new();
    vars.insert("title".to_string(), "A Very Long Episode Title".to_string());
    let result = apply_template("${title:-Untitled:<10}", &vars, None);
    assert_eq!(result.chars().count(), 10, "9 chars + '…' = 10 total");
    assert_eq!(result, "A Very Lo…");
}

#[test]
fn test_crop_with_default_fallback() {
    let vars = HashMap::new();
    assert_eq!(
        apply_template("${title:-Untitled:<10}", &vars, None),
        "Untitled"
    );
}

// Replacement tests

#[test]
fn test_replace_first_occurrence() {
    let mut vars = HashMap::new();
    vars.insert("series".to_string(), "Test Series".to_string());
    assert_eq!(apply_template("${series/ /_}", &vars, None), "Test_Series");
}

#[test]
fn test_replace_all_occurrences() {
    let mut vars = HashMap::new();
    vars.insert("title".to_string(), "A B C D".to_string());
    assert_eq!(apply_template("${title// /}", &vars, None), "ABCD");
}

#[test]
fn test_replace_all_global() {
    let mut vars = HashMap::new();
    vars.insert("series".to_string(), "a-a-a-a".to_string());
    assert_eq!(apply_template("${series//a/b}", &vars, None), "b-b-b-b");
}

#[test]
fn test_replace_no_match() {
    let mut vars = HashMap::new();
    vars.insert("series".to_string(), "Test Series".to_string());
    assert_eq!(apply_template("${series/x/y}", &vars, None), "Test Series");
}

#[test]
fn test_replace_empty_pattern() {
    let mut vars = HashMap::new();
    vars.insert("series".to_string(), "Test".to_string());
    assert_eq!(apply_template("${series//}", &vars, None), "Test");
}

#[test]
fn test_replace_with_format() {
    let mut vars = HashMap::new();
    vars.insert("series".to_string(), "Show Title".to_string());
    let result = apply_template("${series/ /_:<10}", &vars, None);
    // "Show Title" → replace " " with "_" → "Show_Title" (10 chars, no crop needed)
    assert_eq!(result, "Show_Title");
}

// Conditional block tests

#[test]
fn test_conditional_block_present() {
    let mut vars = HashMap::new();
    vars.insert("title".to_string(), "Pilot".to_string());
    assert_eq!(
        apply_template("text?{ - ${title}}end", &vars, None),
        "text - Pilotend"
    );
}

#[test]
fn test_conditional_block_absent() {
    let vars = HashMap::new();
    assert_eq!(
        apply_template("prefix?{ - ${title}}suffix", &vars, None),
        "prefixsuffix"
    );
}

#[test]
fn test_conditional_block_multiple_vars_all_present() {
    let mut vars = HashMap::new();
    vars.insert("title".to_string(), "Pilot".to_string());
    vars.insert("year".to_string(), "2008".to_string());
    assert_eq!(
        apply_template("a?{ (${year}) - ${title}}b", &vars, None),
        "a (2008) - Pilotb"
    );
}

#[test]
fn test_conditional_block_multiple_vars_one_missing() {
    let mut vars = HashMap::new();
    vars.insert("title".to_string(), "Pilot".to_string());
    assert_eq!(
        apply_template("x?{ (${year}) - ${title}}y", &vars, None),
        "xy"
    );
}

#[test]
fn test_conditional_block_empty_value_in_map() {
    let mut vars = HashMap::new();
    vars.insert("title".to_string(), "".to_string());
    assert_eq!(
        apply_template("pre?{ - ${title}}post", &vars, None),
        "prepost"
    );
}

#[test]
fn test_conditional_block_unknown_var() {
    let mut vars = HashMap::new();
    vars.insert("title".to_string(), "Pilot".to_string());
    assert_eq!(apply_template("a?{ - ${unknown}}b", &vars, None), "ab");
}

// Conditional block fallback tests

#[test]
fn test_conditional_fallback_present() {
    // Fallback exists but all vars present — fallback is NOT used
    let mut vars = HashMap::new();
    vars.insert("title".to_string(), "Pilot".to_string());
    assert_eq!(
        apply_template("text?{ - ${title}:-Untitled}end", &vars, None),
        "text - Pilotend"
    );
}

#[test]
fn test_conditional_fallback_absent() {
    // Fallback exists and var is absent — fallback IS used
    let vars = HashMap::new();
    assert_eq!(
        apply_template("pre?{ - ${title}:-Untitled}suf", &vars, None),
        "preUntitledsuf"
    );
}

#[test]
fn test_conditional_fallback_empty_string_in_map() {
    // Variable exists in map but is empty string — treated as absent
    let mut vars = HashMap::new();
    vars.insert("title".to_string(), "".to_string());
    assert_eq!(
        apply_template("x?{ - ${title}:-Fallback}y", &vars, None),
        "xFallbacky"
    );
}

#[test]
fn test_conditional_fallback_no_fallback() {
    // No fallback, var absent — emits nothing (original behavior)
    let vars = HashMap::new();
    assert_eq!(
        apply_template("pre?{ - ${title}}post", &vars, None),
        "prepost"
    );
}

#[test]
fn test_conditional_fallback_with_variable_default_present() {
    // Variable-level :-default AND block-level :-fallback.
    // Variable IS present in map, so the block renders normally.
    let mut vars = HashMap::new();
    vars.insert("title".to_string(), "Pilot".to_string());
    vars.insert("season".to_string(), "2".to_string());
    assert_eq!(
        apply_template("a?{ - ${title} S${season:-01}}b", &vars, None),
        "a - Pilot S2b"
    );
}

#[test]
fn test_conditional_fallback_with_variable_default_absent() {
    // Variable-level :-default exists but key is absent from the map.
    // all_vars_present doesn't evaluate defaults, so it treats the var as missing.
    // Block-level fallback fires instead.
    let mut vars = HashMap::new();
    vars.insert("title".to_string(), "Pilot".to_string());
    assert_eq!(
        apply_template("a?{ - ${title} S${season:-01}:-No Season}b", &vars, None),
        "aNo Seasonb"
    );
}

#[test]
fn test_conditional_fallback_multiple_vars_one_missing() {
    // One var present, one absent — should use fallback
    let mut vars = HashMap::new();
    vars.insert("title".to_string(), "Pilot".to_string());
    assert_eq!(
        apply_template("x?{ - ${title} (${year}):-No Year}y", &vars, None),
        "xNo Yeary"
    );
}

#[test]
fn test_conditional_fallback_literal_colon() {
    // A colon that is NOT followed by `-` should not trigger fallback splitting.
    // The `:` is part of the time format `14:00`, not a block-level :-.
    let mut vars = HashMap::new();
    vars.insert("time".to_string(), "14:00".to_string());
    assert_eq!(
        apply_template("a?{ at ${time}}b", &vars, None),
        "a at 14:00b"
    );
}

#[test]
fn test_conditional_fallback_edge_double_fallback() {
    // Two `:-` at block level — the FIRST one wins (splits there)
    let vars = HashMap::new();
    assert_eq!(
        apply_template("a?{ - ${title}:-first:-second}b", &vars, None),
        "afirst:-secondb"
    );
}

// Conditional block fallback edge cases

#[test]
fn test_conditional_fallback_empty_fallback_string() {
    // Explicitly empty fallback via :- with nothing after it
    let vars = HashMap::new();
    assert_eq!(apply_template("a?{${title}:-}b", &vars, None), "ab");
}

#[test]
fn test_conditional_fallback_whitespace_only_fallback() {
    // Fallback is just whitespace
    let vars = HashMap::new();
    assert_eq!(apply_template("a?{${title}:- }b", &vars, None), "a b");
}

#[test]
fn test_conditional_fallback_dollar_in_fallback() {
    // Dollar sign in fallback text (not a variable — literal)
    let vars = HashMap::new();
    // The $5 in the fallback is literal text, not a variable reference
    let result = apply_template("a?{${title}:-$5 dollars}b", &vars, None);
    assert_eq!(result, "a$5 dollarsb");
}

#[test]
fn test_conditional_fallback_braces_in_fallback() {
    // Braces in fallback text (literal text, NOT a conditional block)
    let vars = HashMap::new();
    assert_eq!(
        apply_template("a?{${title}:-text {with braces}}b", &vars, None),
        "atext {with braces}b"
    );
}

#[test]
fn test_conditional_fallback_percent_in_fallback() {
    // Percent sign in fallback text (not a date format block — literal)
    let vars = HashMap::new();
    assert_eq!(
        apply_template("a?{${title}:-50% off}b", &vars, None),
        "a50% offb"
    );
}

#[test]
fn test_conditional_fallback_colon_in_fallback() {
    // Literal colon in fallback text (not followed by `-`, so not a split point)
    let vars = HashMap::new();
    assert_eq!(
        apply_template("a?{${title}:-time: 14:00}b", &vars, None),
        "atime: 14:00b"
    );
}

#[test]
fn test_conditional_fallback_percent_date_syntax() {
    // %{release:...} and ${title} both present — block renders fully
    let mut vars = HashMap::new();
    vars.insert(
        "__release_date".to_string(),
        "2024-03-15 10:30:00".to_string(),
    );
    vars.insert("title".to_string(), "Pilot".to_string());
    assert_eq!(
        apply_template("a?{%{release:%Y-%m-%d} - ${title}:-No Date}b", &vars, None),
        "a2024-03-15 - Pilotb"
    );
}

#[test]
fn test_conditional_fallback_percent_date_absent() {
    // %{release:...} inside conditional with no date (date absent → var absent → fallback fires)
    let vars = HashMap::new();
    assert_eq!(
        apply_template("a?{%{release:%Y} - ${title}:-No Date}b", &vars, None),
        "aNo Dateb"
    );
}

// %{...} with ${...} inside format string tests

#[test]
fn test_percent_date_with_variable_in_format() {
    // %{release:${title}} — ${...} inside the format string should be consumed
    // as literal text so its } doesn't prematurely close the %{...} block.
    let mut vars = HashMap::new();
    vars.insert(
        "__release_date".to_string(),
        "2024-03-15 10:30:00".to_string(),
    );
    vars.insert("title".to_string(), "Pilot".to_string());
    // "${title}" in the format string is passed to chrono as literal text
    // (chrono treats $ { } as non-special characters), so the output is
    // the date year followed by the literal "${title}"
    assert_eq!(
        apply_template("%{release:%Y-${title}}", &vars, None),
        "2024-${title}"
    );
}

#[test]
fn test_percent_date_variable_in_format_absent_date() {
    // %{release:${title}} with no release date — the %{...} block emits nothing
    let mut vars = HashMap::new();
    vars.insert("title".to_string(), "Pilot".to_string());
    // No __release_date in vars, so %{...} emits its literal
    assert_eq!(
        apply_template("a%{release:%Y-${title}}b", &vars, None),
        "a%{release:%Y-${title}}b"
    );
}

#[test]
fn test_percent_date_variable_in_format_empty_date() {
    // %{release:${title}} with empty release date — the %{...} block emits nothing
    let mut vars = HashMap::new();
    vars.insert("__release_date".to_string(), "".to_string());
    vars.insert("title".to_string(), "Pilot".to_string());
    assert_eq!(
        apply_template("a%{release:%Y-${title}}b", &vars, None),
        "ab"
    );
}

#[test]
fn test_percent_date_nested_dollar_braces_in_format() {
    // ${${example}${}bad} inside %{release:...} format — all braces consumed correctly
    let mut vars = HashMap::new();
    vars.insert("example".to_string(), "x".to_string());
    vars.insert(
        "__release_date".to_string(),
        "2024-03-15 10:30:00".to_string(),
    );
    // The entire ${${example}${}bad} is consumed as literal text in the format.
    // chrono treats it all as literal text since $, {, } are not format specifiers.
    assert_eq!(
        apply_template("%{release:%Y-${${example}${}bad}}", &vars, None),
        "2024-${${example}${}bad}"
    );
}

#[test]
fn test_percent_date_escaped_dollar_in_format() {
    // $${$}}} inside %{release:...} — $$ is escaped dollar, then ${}
    let mut vars = HashMap::new();
    vars.insert(
        "__release_date".to_string(),
        "2024-03-15 10:30:00".to_string(),
    );
    // $$ → literal $, then ${$} consumed as ${} block (consuming { $ }),
    // next } closes %{...}, remaining }} are literal in the outer template.
    // chrono format string becomes "%Y-$${$}", outputting "2024-$${$}"
    assert_eq!(
        apply_template("%{release:%Y-$${$}}}", &vars, None),
        "2024-$${$}}"
    );
}

#[test]
fn test_percent_date_variable_as_source_name() {
    // %{${source}:%Y} — variable as date source name, not recognized → literal
    let mut vars = HashMap::new();
    vars.insert("source".to_string(), "release".to_string());
    vars.insert(
        "__release_date".to_string(),
        "2024-03-15 10:30:00".to_string(),
    );
    // ${source} is consumed as literal in source name → "release" doesn't match
    // because the literal is "${source}", so it falls through to literal output
    assert_eq!(
        apply_template("a%{${source}:%Y}b", &vars, None),
        "a%{${source}:%Y}b"
    );
}

#[test]
fn test_percent_date_multiple_variables_in_format() {
    // %{release:%Y-${season}-${episode}} — two ${} blocks in the format string
    let mut vars = HashMap::new();
    vars.insert(
        "__release_date".to_string(),
        "2024-03-15 10:30:00".to_string(),
    );
    vars.insert("season".to_string(), "01".to_string());
    vars.insert("episode".to_string(), "05".to_string());
    assert_eq!(
        apply_template("%{release:%Y-${season}-${episode}}", &vars, None),
        "2024-${season}-${episode}"
    );
}

#[test]
fn test_percent_date_empty_format_with_variable() {
    // Edge: %{release:${}} — empty variable name inside %{...}
    let mut vars = HashMap::new();
    vars.insert(
        "__release_date".to_string(),
        "2024-03-15 10:30:00".to_string(),
    );
    assert_eq!(apply_template("%{release:%Y-${}}", &vars, None), "2024-${}");
}

// Nested conditional tests

#[test]
fn test_nested_conditional_both_present() {
    // Two nested conditionals: outer and inner — both have all vars present
    let mut vars = HashMap::new();
    vars.insert("series".to_string(), "Show".to_string());
    vars.insert("season".to_string(), "1".to_string());
    assert_eq!(
        apply_template("a?{${series}?{ S${season}}}b", &vars, None),
        "aShow S1b"
    );
}

#[test]
fn test_nested_conditional_outer_present_inner_absent() {
    // Outer present (series), inner absent (season) — inner suppresses
    let mut vars = HashMap::new();
    vars.insert("series".to_string(), "Show".to_string());
    assert_eq!(
        apply_template("a?{${series}?{ S${season}}}b", &vars, None),
        "aShowb"
    );
}

#[test]
fn test_nested_conditional_outer_absent() {
    // Outer absent (series) — entire block suppresses, inner never evaluated
    let mut vars = HashMap::new();
    vars.insert("season".to_string(), "1".to_string());
    assert_eq!(
        apply_template("a?{${series}?{ S${season}}}b", &vars, None),
        "ab"
    );
}

#[test]
fn test_nested_conditional_both_absent() {
    // Both outer and inner absent
    let vars = HashMap::new();
    assert_eq!(
        apply_template("a?{${series}?{ S${season}}}b", &vars, None),
        "ab"
    );
}

#[test]
fn test_nested_conditional_three_levels_all_present() {
    // Three levels of nesting — all vars present
    let mut vars = HashMap::new();
    vars.insert("series".to_string(), "Show".to_string());
    vars.insert("season".to_string(), "1".to_string());
    vars.insert("episode".to_string(), "5".to_string());
    assert_eq!(
        apply_template("a?{${series}?{ S${season}?{E${episode}}}}b", &vars, None),
        "aShow S1E5b"
    );
}

#[test]
fn test_nested_conditional_three_levels_mid_absent() {
    // Three levels — season is absent, so middle block suppresses, deep inner never reached
    let mut vars = HashMap::new();
    vars.insert("series".to_string(), "Show".to_string());
    vars.insert("episode".to_string(), "5".to_string());
    assert_eq!(
        apply_template("a?{${series}?{ S${season}?{E${episode}}}}b", &vars, None),
        "aShowb"
    );
}

#[test]
fn test_nested_conditional_sibling_blocks() {
    // Sibling conditionals at the same level
    let mut vars = HashMap::new();
    vars.insert("year".to_string(), "2024".to_string());
    vars.insert("group".to_string(), "GRP".to_string());
    assert_eq!(
        apply_template("a?{ (${year})}?{ [${group}]}b", &vars, None),
        "a (2024) [GRP]b"
    );
}

#[test]
fn test_nested_conditional_sibling_one_absent() {
    // One sibling absent, other present
    let mut vars = HashMap::new();
    vars.insert("group".to_string(), "GRP".to_string());
    assert_eq!(
        apply_template("a?{ (${year})}?{ [${group}]}b", &vars, None),
        "a [GRP]b"
    );
}

#[test]
fn test_nested_conditional_with_fallback_outer_present() {
    // Outer has fallback, inner conditional inside — inner absent triggers outer fallback
    let mut vars = HashMap::new();
    vars.insert("series".to_string(), "Show".to_string());
    assert_eq!(
        apply_template("a?{${series}?{ - ${season}}:-No Season}b", &vars, None),
        "aShowb" // inner suppressed, but series is present so outer renders its template (without inner content)
    );
}

#[test]
fn test_nested_conditional_with_fallback_outer_absent() {
    // Outer absent → outer fallback fires
    let vars = HashMap::new();
    assert_eq!(
        apply_template("a?{${series}?{ - ${season}}:-Fallback}b", &vars, None),
        "aFallbackb"
    );
}

// Break-attempt & edge case tests

#[test]
fn test_conditional_unclosed_at_eof() {
    // Unclosed ?{ at end of template — should not panic.
    // process_conditional reads until EOF and treats the content as a normal
    // conditional block. Since ${title} is absent, nothing is emitted.
    let vars = HashMap::new();
    assert_eq!(apply_template("a?{ - ${title}", &vars, None), "a");
}

#[test]
fn test_conditional_multiple_blocks() {
    // Three separate conditional blocks in sequence, mix of present/absent
    let mut vars = HashMap::new();
    vars.insert("a".to_string(), "A".to_string());
    vars.insert("c".to_string(), "C".to_string());
    assert_eq!(apply_template("?{${a}}?{${b}}?{${c}}", &vars, None), "AC");
}

#[test]
fn test_conditional_no_variables() {
    // Conditional block with no variable references — always renders
    let vars = HashMap::new();
    assert_eq!(
        apply_template("a?{literal text}b", &vars, None),
        "aliteral textb"
    );
}

#[test]
fn test_conditional_no_variables_with_fallback() {
    // No variables in template, fallback exists — template always present so fallback unused
    let vars = HashMap::new();
    assert_eq!(apply_template("a?{hello:-world}b", &vars, None), "ahellob");
}

#[test]
fn test_conditional_only_fallback_no_template() {
    // No template content, just `:-fallback` — no vars to check, renders template (empty)
    let vars = HashMap::new();
    assert_eq!(apply_template("a?{:-fallback}b", &vars, None), "ab");
}

#[test]
fn test_conditional_variable_with_replace_and_fallback() {
    // Variable uses replacement syntax ${var/old/new} inside conditional with block-level fallback
    let mut vars = HashMap::new();
    vars.insert("title".to_string(), "Bad Title".to_string());
    assert_eq!(
        apply_template("a?{${title/ /_}}b", &vars, None),
        "aBad_Titleb"
    );
}

#[test]
fn test_conditional_variable_with_replace_absent() {
    // Variable with replacement is absent — block-level fallback fires
    let vars = HashMap::new();
    assert_eq!(
        apply_template("a?{${title/ /_}:-Untitled}b", &vars, None),
        "aUntitledb"
    );
}

#[test]
fn test_conditional_fallback_syntax_similar_text() {
    // Text that looks like a fallback separator but isn't: `?-` (not `:-`)
    let mut vars = HashMap::new();
    vars.insert("title".to_string(), "Test".to_string());
    assert_eq!(apply_template("a?{?- ${title}}b", &vars, None), "a?- Testb");
}

#[test]
fn test_conditional_colon_in_variable_default() {
    // Variable-level :-default: "${title:-default:value}" — the `:` inside the default
    // should NOT confuse the block-level fallback parser
    let vars = HashMap::new();
    assert_eq!(
        apply_template("a?{${title:-default:value}:-block_fallback}b", &vars, None),
        "ablock_fallbackb"
    );
}

#[test]
fn test_conditional_colon_in_variable_default_value_present() {
    // Variable-level :-default: value present in map, so renders value, not default
    let mut vars = HashMap::new();
    vars.insert("title".to_string(), "real".to_string());
    assert_eq!(
        apply_template("a?{${title:-default:value}:-block_fallback}b", &vars, None),
        "arealb"
    );
}

#[test]
fn test_conditional_many_variables_some_present_with_fallback() {
    // 5 variables, 3 present, 2 absent → fallback fires
    let mut vars = HashMap::new();
    vars.insert("a".to_string(), "1".to_string());
    vars.insert("b".to_string(), "2".to_string());
    vars.insert("d".to_string(), "4".to_string());
    assert_eq!(
        apply_template("?{${a}${b}${c}${d}${e}:-MISSING}", &vars, None),
        "MISSING"
    );
}

#[test]
fn test_conditional_many_variables_all_present() {
    // 5 variables, all present → renders
    let mut vars = HashMap::new();
    vars.insert("a".to_string(), "1".to_string());
    vars.insert("b".to_string(), "2".to_string());
    vars.insert("c".to_string(), "3".to_string());
    vars.insert("d".to_string(), "4".to_string());
    vars.insert("e".to_string(), "5".to_string());
    assert_eq!(
        apply_template("?{${a}${b}${c}${d}${e}}", &vars, None),
        "12345"
    );
}

#[test]
fn test_conditional_unicode_content() {
    // Unicode characters in template and fallback
    let mut vars = HashMap::new();
    vars.insert("title".to_string(), "Épisode".to_string());
    assert_eq!(
        apply_template("?{ - ${title}:-N/A}", &vars, None),
        " - Épisode"
    );
}

#[test]
fn test_conditional_unicode_content_absent() {
    // Unicode characters in fallback (when var absent)
    let vars = HashMap::new();
    assert_eq!(
        apply_template("?{ - ${title}:-¡No disponible!}", &vars, None),
        "¡No disponible!"
    );
}

#[test]
fn test_conditional_fallback_embedded_dollar() {
    // Fallback text contains ${...} syntax — should be literal, NOT evaluated
    let vars = HashMap::new();
    let result = apply_template("a?{${title}:-${literal}}b", &vars, None);
    assert_eq!(result, "a${literal}b");
}

// Security & injection tests

#[test]
fn test_security_variable_value_contains_template_syntax() {
    let mut vars = HashMap::new();
    vars.insert("title".to_string(), "${injected}".to_string());
    assert_eq!(apply_template("${title}", &vars, None), "${injected}");
}

#[test]
fn test_security_variable_value_contains_conditional_syntax_old() {
    let mut vars = HashMap::new();
    vars.insert("title".to_string(), "{? - secret}".to_string());
    assert_eq!(apply_template("${title}", &vars, None), "{? - secret}");
}

#[test]
fn test_security_variable_value_contains_conditional_new_syntax() {
    // Variable value containing `?{...}` should be literal, not parsed
    let mut vars = HashMap::new();
    vars.insert("title".to_string(), "?{injected}".to_string());
    assert_eq!(apply_template("${title}", &vars, None), "?{injected}");
}

#[test]
fn test_security_variable_value_contains_date_syntax() {
    // Variable value containing `%{release:...}` should be literal, not parsed as date
    let mut vars = HashMap::new();
    vars.insert("title".to_string(), "%{release:%Y}".to_string());
    assert_eq!(apply_template("${title}", &vars, None), "%{release:%Y}");
}

#[test]
fn test_security_fallback_contains_conditional_syntax() {
    // Fallback text containing `?{...}` should be literal, not re-parsed
    let vars = HashMap::new();
    let result = apply_template("a?{${title}:-?{nested}}b", &vars, None);
    assert_eq!(result, "a?{nested}b");
}

#[test]
fn test_security_fallback_contains_date_syntax() {
    // Fallback text containing `%{...}` should be literal, not re-parsed
    let vars = HashMap::new();
    let result = apply_template("a?{${title}:-%{release:%Y}}b", &vars, None);
    assert_eq!(result, "a%{release:%Y}b");
}

#[test]
fn test_security_sql_injection_in_variable_value() {
    // SQL keywords in variable value should be emitted as literal text
    let mut vars = HashMap::new();
    vars.insert(
        "title".to_string(),
        "'; DROP TABLE episodes; --".to_string(),
    );
    assert_eq!(
        apply_template("${title}", &vars, None),
        "'; DROP TABLE episodes; --"
    );
}

#[test]
fn test_security_sql_injection_in_fallback() {
    // SQL in a conditional fallback should be emitted literally (never reaches a DB)
    let vars = HashMap::new();
    let result = apply_template("?{${title}:-' OR '1'='1}", &vars, None);
    assert_eq!(result, "' OR '1'='1");
}

#[test]
fn test_security_sql_injection_in_nested_conditional_fallback() {
    // SQL in a nested conditional's fallback
    let mut vars = HashMap::new();
    vars.insert("series".to_string(), "Show".to_string());
    let result = apply_template("?{${series}?{ - ${season}:-' OR '1'='1}}", &vars, None);
    assert_eq!(result, "Show");
}

#[test]
fn test_security_path_traversal_in_variable() {
    // Path traversal sequences in variable value should be literal text
    let mut vars = HashMap::new();
    vars.insert("title".to_string(), "../../etc/passwd".to_string());
    assert_eq!(apply_template("${title}", &vars, None), "../../etc/passwd");
}

#[test]
fn test_security_variable_value_contains_escape_sequences() {
    // Escape sequences like \x00, \n, \r should be literal (Rust strings handle these)
    let mut vars = HashMap::new();
    vars.insert("title".to_string(), "null\x00byte".to_string());
    assert_eq!(apply_template("${title}", &vars, None), "null\x00byte");
}

#[test]
fn test_security_variable_value_contains_interleaved_syntax() {
    // Mixed syntax in value: `${...}` inside `%{...}` inside `?{...}` — all literal
    let mut vars = HashMap::new();
    vars.insert("title".to_string(), "%{release:%Y}?{nested}".to_string());
    assert_eq!(
        apply_template("${title}", &vars, None),
        "%{release:%Y}?{nested}"
    );
}

// Overflow & stress tests

#[test]
fn test_overflow_very_long_variable_value() {
    // Very long (10KB) variable value should not cause issues
    let long_val = "A".repeat(10_240);
    let mut vars = HashMap::new();
    vars.insert("title".to_string(), long_val.clone());
    let result = apply_template("${title}", &vars, None);
    assert_eq!(result.len(), 10_240);
    assert!(result.starts_with("AAA"));
}

#[test]
fn test_overflow_very_long_template() {
    // Very long template with many valid variable references should not cause issues
    let mut vars = HashMap::new();
    vars.insert("a".to_string(), "X".to_string());
    let template = (0..1000)
        .map(|i| format!("${{a}}{}", i))
        .collect::<Vec<_>>()
        .join("");
    let result = apply_template(&template, &vars, None);
    assert!(result.starts_with("X0"));
    assert!(result.ends_with("X999"));
}

#[test]
fn test_overflow_deeply_nested_conditionals() {
    // 50 levels of nested ?{...} — should not stack overflow
    let mut vars = HashMap::new();
    vars.insert("a".to_string(), "X".to_string());
    // Build: ?{${a}?{${a}?...?{${a}}...}}
    let mut template = String::new();
    for _ in 0..50 {
        template.push_str("?{${a}");
    }
    for _ in 0..50 {
        template.push('}');
    }
    let result = apply_template(&template, &vars, None);
    assert_eq!(result, "X".repeat(50));
}

#[test]
fn test_overflow_deeply_nested_conditionals_with_missing_mid() {
    // 50 levels, but one mid-level var is absent — entire chain should suppress at that point
    let mut vars = HashMap::new();
    vars.insert("a".to_string(), "X".to_string());
    // Insert a b that's absent at varying depths — test at depth 25
    let mut template = String::new();
    for i in 0..50 {
        if i == 25 {
            template.push_str("?{${b}");
        } else {
            template.push_str("?{${a}");
        }
    }
    for _ in 0..50 {
        template.push('}');
    }
    let result = apply_template(&template, &vars, None);
    // At depth 25, ${b} is absent, so that conditional suppresses, suppressing all deeper levels
    // The outer 25 levels (indices 0-24) render, so we should get 25 "X"s
    assert_eq!(result, "X".repeat(25));
}

#[test]
fn test_overflow_deep_nesting_with_fallback() {
    // Deep nesting where deepest block has fallback
    let mut vars = HashMap::new();
    vars.insert("a".to_string(), "X".to_string());
    let mut template = String::new();
    template.push('a');
    for _ in 0..100 {
        template.push_str("?{${a}");
    }
    template.push_str(":-END");
    for _ in 0..100 {
        template.push('}');
    }
    template.push('b');
    let result = apply_template(&template, &vars, None);
    // "a" + 100 X's + "b" = 102 chars
    assert_eq!(result.len(), 102);
    assert!(result.starts_with('a'));
    assert!(result.ends_with('b'));
}

#[test]
fn test_overflow_large_conditional_content() {
    // Conditional with 100KB of literal content — should not choke
    let large_text = "B".repeat(102_400);
    let mut vars = HashMap::new();
    vars.insert("title".to_string(), large_text.clone());
    let result = apply_template("?{${title}}", &vars, None);
    assert_eq!(result.len(), 102_400);
}

#[test]
fn test_overflow_conditional_with_very_long_fallback() {
    // Fallback text is 100KB — should not choke
    let long_fallback = "C".repeat(102_400);
    let vars = HashMap::new();
    let template = format!("?{{{}:-{}}}", "${title}", long_fallback);
    let result = apply_template(&template, &vars, None);
    assert_eq!(result.len(), 102_400);
}

#[test]
fn test_overflow_many_sibling_conditionals() {
    // 5000 sibling ?{...} blocks — should not overflow
    let mut vars = HashMap::new();
    vars.insert("a".to_string(), "X".to_string());
    let template = (0..5000).map(|_| "?{${a}}").collect::<Vec<_>>().join("");
    let result = apply_template(&template, &vars, None);
    assert_eq!(result.len(), 5000);
    assert!(result.chars().all(|c| c == 'X'));
}

#[test]
fn test_overflow_many_variables_in_one_block() {
    // 1000 variables in a single conditional block — all present
    let mut vars = HashMap::new();
    for i in 0..1000 {
        vars.insert(format!("v{}", i), format!("{}", i % 10));
    }
    let template = (0..1000)
        .map(|i| format!("${{v{}}}", i))
        .collect::<Vec<_>>()
        .join("");
    let full_template = format!("?{{{}}}", template);
    let result = apply_template(&full_template, &vars, None);
    assert_eq!(result.len(), 1000);
}

#[test]
fn test_overflow_many_variables_one_absent() {
    // 1000 variables in a block, last one absent — should suppress entirely via fallback
    let mut vars = HashMap::new();
    for i in 0..999 {
        vars.insert(format!("v{}", i), "X".to_string());
    }
    // v999 is absent
    let template = (0..1000)
        .map(|i| format!("${{v{}}}", i))
        .collect::<Vec<_>>()
        .join("");
    let full_template = format!("?{{{}}}", template);
    // Just checking it doesn't panic — all_vars_present scans all 1000 vars,
    // hits the absent one at v999, returns false → emits nothing
    let result = apply_template(&full_template, &vars, None);
    assert_eq!(result, "");
}

#[test]
fn test_overflow_many_variables_with_fallback() {
    // 1000 variables with one absent + fallback
    let mut vars = HashMap::new();
    for i in 0..999 {
        vars.insert(format!("v{}", i), "X".to_string());
    }
    let template = (0..1000)
        .map(|i| format!("${{v{}}}", i))
        .collect::<Vec<_>>()
        .join("");
    let full_template = format!("?{{{}:-FALLBACK}}", template);
    let result = apply_template(&full_template, &vars, None);
    assert_eq!(result, "FALLBACK");
}

// Post-expansion crop tests

fn make_opts_with_max(max: usize) -> TemplatePadOptions {
    TemplatePadOptions {
        max_length: max,
        ..Default::default()
    }
}

#[test]
fn test_crop_below_limit() {
    // Output shorter than limit — no cropping
    let mut vars = HashMap::new();
    vars.insert("title".to_string(), "Pilot".to_string());
    let opts = make_opts_with_max(50);
    assert_eq!(apply_template("${title}", &vars, Some(&opts)), "Pilot");
}

#[test]
fn test_crop_exactly_at_limit() {
    // Output exactly at limit — no cropping
    let mut vars = HashMap::new();
    vars.insert("title".to_string(), "ABCDE".to_string());
    let opts = make_opts_with_max(5);
    assert_eq!(apply_template("${title}", &vars, Some(&opts)), "ABCDE");
}

#[test]
fn test_crop_simple_truncation() {
    // No extension — truncate with … suffix
    let mut vars = HashMap::new();
    vars.insert("title".to_string(), "Hello World Long".to_string());
    let opts = make_opts_with_max(5);
    assert_eq!(apply_template("${title}", &vars, Some(&opts)), "Hello");
}

#[test]
fn test_crop_preserves_extension() {
    // Has .mkv extension — preserve it
    let mut vars = HashMap::new();
    vars.insert("series".to_string(), "VeryLongSeriesName".to_string());
    let opts = make_opts_with_max(20);
    // "VeryLongSeriesName.mkv" = 22 chars, max=20
    // ext=".mkv" (4), available=16, stem=16 bytes → "VeryLongSeriesNa"
    assert_eq!(
        apply_template("${series}.mkv", &vars, Some(&opts)),
        "VeryLongSeriesNa.mkv"
    );
}

#[test]
fn test_crop_extension_preserved_with_static_text() {
    let mut vars = HashMap::new();
    vars.insert("series".to_string(), "My Show".to_string());
    vars.insert(
        "title".to_string(),
        "An Extremely Long Episode Title That Exceeds Limits".to_string(),
    );
    let opts = make_opts_with_max(30);
    let result = apply_template("${series} - ${title}.mkv", &vars, Some(&opts));
    // Rendered: "My Show - An Extremely Long Episode Title That Exceeds Limits.mkv"
    // max=30, ext=".mkv" (4), available=26, stem=26 bytes → "My Show - An Extremely Lon"
    assert_eq!(result, "My Show - An Extremely Lon.mkv");
}

#[test]
fn test_crop_zero_limit_disabled() {
    // max_length=0 means unlimited
    let mut vars = HashMap::new();
    vars.insert(
        "title".to_string(),
        "A very long title that should not be cropped".to_string(),
    );
    let opts = make_opts_with_max(0);
    assert_eq!(
        apply_template("${title}", &vars, Some(&opts)),
        "A very long title that should not be cropped"
    );
}

#[test]
fn test_crop_conditional_with_fallback_cropped() {
    // Full conditional rendering then cropped
    let mut vars = HashMap::new();
    vars.insert(
        "title".to_string(),
        "Very Long Title Here Exceeds".to_string(),
    );
    let opts = make_opts_with_max(10);
    // Rendered: "Very Long Title Here Exceeds" (30 bytes), max=10
    // No extension: truncate at byte 10 → "Very Long "
    assert_eq!(
        apply_template("?{${title}:-short}", &vars, Some(&opts)),
        "Very Long "
    );
}

#[test]
fn test_crop_conditional_fallback_used_then_cropped() {
    // Fallback rendered then cropped
    let vars = HashMap::new();
    // Rendered: "longfallback" (12 chars), max=3
    let opts = make_opts_with_max(3);
    assert_eq!(
        apply_template("?{${title}:-longfallback}", &vars, Some(&opts)),
        "lon"
    );
}

#[test]
fn test_crop_very_short_limit_with_extension() {
    // Extension alone nearly fills or exceeds limit
    let mut vars = HashMap::new();
    vars.insert("title".to_string(), "Hello".to_string());
    let opts = make_opts_with_max(5); // ".mkv" = 4 bytes, available=1, stem=1 byte
    // Rendered: "Hello.mkv" (10 bytes), max=5, ext=".mkv" (4), stem=1 byte → "H.mkv"
    let result = apply_template("${title}.mkv", &vars, Some(&opts));
    assert_eq!(result, "H.mkv");
}

#[test]
fn test_crop_with_dates() {
    // Date formatting then cropped
    let mut vars = HashMap::new();
    vars.insert(
        "__release_date".to_string(),
        "2024-03-15 10:30:00".to_string(),
    );
    vars.insert("title".to_string(), "Episode".to_string());
    let opts = make_opts_with_max(15);
    // Rendered: "2024-03-15 - Episode" (20 bytes), max=15, no ext → byte 15 is 'i'
    assert_eq!(
        apply_template("%{release:%Y-%m-%d} - ${title}", &vars, Some(&opts)),
        "2024-03-15 - Ep"
    );
}

#[test]
fn test_crop_no_extension_double_dot() {
    // Multiple dots — rfind finds the LAST dot (before .gz)
    let mut vars = HashMap::new();
    vars.insert("series".to_string(), "Show.Name.With.Dots".to_string());
    let opts = make_opts_with_max(15);
    // Rendered: "Show.Name.With.Dots.tar.gz" = 28 chars, max=15
    // rfind('.') finds the '.' before "gz", ext=".gz" (3), available=12, stem=12 bytes
    assert_eq!(
        apply_template("${series}.tar.gz", &vars, Some(&opts)),
        "Show.Name.Wi.gz"
    );
}

#[test]
fn test_crop_multiple_variables() {
    // Multiple variables, combined output cropped
    let mut vars = HashMap::new();
    vars.insert("series".to_string(), "Long Series Name Here".to_string());
    vars.insert("season".to_string(), "1".to_string());
    vars.insert("episode".to_string(), "23".to_string());
    let opts = make_opts_with_max(25);
    // Rendered: "Long Series Name Here S01E23.mkv" = 32 chars, max=25
    // ext=".mkv" (4), available=21, stem=21 bytes → "Long Series Name Here"
    assert_eq!(
        apply_template(
            "${series} S${season:02}E${episode:02}.mkv",
            &vars,
            Some(&opts)
        ),
        "Long Series Name Here.mkv"
    );
}

#[test]
fn test_crop_unicode_preserved() {
    // Unicode chars — crop on char boundaries, not byte boundaries
    let mut vars = HashMap::new();
    vars.insert("title".to_string(), "Café résumé spécial".to_string());
    let opts = make_opts_with_max(8);
    // "Café résumé spécial" = 22 bytes, max=8
    // No extension: byte 8 is mid-é, walk back to byte 7 → "Café r"
    assert_eq!(apply_template("${title}", &vars, Some(&opts)), "Café r");
}

#[test]
fn test_crop_empty_output() {
    // Empty output — no crash
    let vars = HashMap::new();
    let opts = make_opts_with_max(10);
    assert_eq!(apply_template("", &vars, Some(&opts)), "");
}

// Edge cases

#[test]
fn test_edge_very_long_format_spec() {
    let mut vars = HashMap::new();
    vars.insert("series".to_string(), "Test".to_string());
    assert_eq!(
        apply_template("${series:unknown_format}", &vars, None),
        "Test"
    );
}

#[test]
fn test_edge_double_dollar_in_template() {
    let vars = HashMap::new();
    assert_eq!(apply_template("Price: $$5", &vars, None), "Price: $5");
}

#[test]
fn test_edge_double_dollar_near_variable() {
    let mut vars = HashMap::new();
    vars.insert("series".to_string(), "Show".to_string());
    assert_eq!(apply_template("$${series}", &vars, None), "${series}");
}

#[test]
fn test_edge_multiple_conditionals() {
    let mut vars = HashMap::new();
    vars.insert("title".to_string(), "Pilot".to_string());
    vars.insert("year".to_string(), "2008".to_string());
    assert_eq!(
        apply_template("?{${title}}?{ - ${year}}", &vars, None),
        "Pilot - 2008"
    );
}

#[test]
fn test_edge_braces_in_literal_text() {
    let vars = HashMap::new();
    assert_eq!(
        apply_template("plain text {not a var}", &vars, None),
        "plain text {not a var}"
    );
}

#[test]
fn test_edge_escaped_dollar_with_default() {
    let mut vars = HashMap::new();
    vars.insert("series".to_string(), "Show".to_string());
    assert_eq!(
        apply_template("$${series:-Fallback}", &vars, None),
        "${series:-Fallback}"
    );
}

#[test]
fn test_edge_all_features_combined() {
    let mut vars = HashMap::new();
    vars.insert("series".to_string(), "The Test Series".to_string());
    vars.insert("season".to_string(), "1".to_string());
    vars.insert("episode".to_string(), "5".to_string());
    vars.insert(
        "title".to_string(),
        "A Very Long Episode Title Here".to_string(),
    );
    let template = "${series:<10}?{ - ${title:<20:…} (S${season:02}E${episode:02})}";
    let result = apply_template(template, &vars, None);
    assert_eq!(
        result, "The Test … - A Very Long Episode… (S01E05)",
        "Combined features should produce exact expected output"
    );
}

#[test]
fn test_edge_old_conditional_syntax_is_literal() {
    // The old `{?` syntax (used before the migration to `?{`) must NOT be
    // treated as a conditional block. It should render as literal text,
    // even when a variable inside is absent. This guards against accidental
    // re-introduction of the old syntax.
    //
    // Old syntax used in template:  {? - ${missing}}
    // New syntax that IS conditional: ?{ - ${missing}}

    // We use `series` and `title` here because they are known valid variables.
    // "missing" is not a valid variable, so even `${missing}` renders literally.

    // Case 1: old syntax `{?` with an absent var — renders literally, block NOT suppressed
    let vars = HashMap::new();
    assert_eq!(
        apply_template("pre{? - ${series}}post", &vars, None),
        "pre{? - }post",
        "Old `{{?` syntax must render literally when var is absent (block not suppressed)"
    );

    // Case 2: new syntax `?{` with an absent var — block IS suppressed
    assert_eq!(
        apply_template("pre?{ - ${series}}post", &vars, None),
        "prepost",
        "New `?{{` syntax must suppress block when var is absent"
    );

    // Case 3: old syntax `{?` with a present var — still renders literally (literal braces)
    let mut vars2 = HashMap::new();
    vars2.insert("series".to_string(), "Test Series".to_string());
    assert_eq!(
        apply_template("pre{? - ${series}}post", &vars2, None),
        "pre{? - Test Series}post",
        "Old `{{?` syntax must render literally even when var is present"
    );

    // Case 4: new syntax `?{` with a present var — block content is rendered correctly
    assert_eq!(
        apply_template("pre?{ - ${series}}post", &vars2, None),
        "pre - Test Seriespost",
        "New `?{{` syntax must render block content when var is present"
    );
}

#[test]
fn test_edge_year_variable_non_numeric_with_format() {
    let mut vars = HashMap::new();
    vars.insert("year".to_string(), "test".to_string());
    assert_eq!(apply_template("${year:02}", &vars, None), "test");
}

// Media info variable tests
//
// Media info variables (codec, resolution, bitrate, audio_codec, etc.) are valid
// via `is_valid_variable`: when absent they render as empty string, not literal
// `${codec}`.

#[test]
fn test_media_info_var_renders_value() {
    let mut vars = HashMap::new();
    vars.insert("codec".to_string(), "h264".to_string());
    assert_eq!(apply_template("${codec}", &vars, None), "h264");
}

#[test]
fn test_media_info_var_absent() {
    let vars = HashMap::new();
    // `codec` is a valid variable via is_valid_variable, so it does NOT render
    // as literal `${codec}`. Instead it renders as empty string.
    assert_eq!(apply_template("${codec}", &vars, None), "");
}

#[test]
fn test_multiple_media_info_vars() {
    let mut vars = HashMap::new();
    vars.insert("codec".to_string(), "h264".to_string());
    vars.insert("resolution".to_string(), "1920x1080".to_string());
    vars.insert("audio_codec".to_string(), "aac".to_string());
    assert_eq!(
        apply_template("${codec} ${resolution} ${audio_codec}", &vars, None),
        "h264 1920x1080 aac"
    );
}

#[test]
fn test_conditional_media_info_present() {
    let mut vars = HashMap::new();
    vars.insert("codec".to_string(), "h264".to_string());
    assert_eq!(
        apply_template("text?{ - ${codec}}end", &vars, None),
        "text - h264end"
    );
}

#[test]
fn test_conditional_media_info_absent() {
    let vars = HashMap::new();
    assert_eq!(
        apply_template("text?{ - ${codec}}end", &vars, None),
        "textend"
    );
}

#[test]
fn test_conditional_media_info_some_absent() {
    let mut vars = HashMap::new();
    vars.insert("codec".to_string(), "h264".to_string());
    // resolution is NOT in vars — block should be suppressed
    assert_eq!(
        apply_template("text?{ - ${codec} ${resolution}}end", &vars, None),
        "textend"
    );
}

#[test]
fn test_media_info_default_fallback() {
    let vars = HashMap::new();
    assert_eq!(apply_template("${codec:-Unknown}", &vars, None), "Unknown");
}

#[test]
fn test_zero_values_suppress_conditional() {
    let mut vars = HashMap::new();
    // MediaInfo::to_template_vars inserts empty string when width is 0
    vars.insert("width".to_string(), "".to_string());
    assert_eq!(
        apply_template("text?{ - ${width}}end", &vars, None),
        "textend"
    );

    // When width has a positive value, the conditional block renders
    let mut vars_with_value = HashMap::new();
    vars_with_value.insert("width".to_string(), "1920".to_string());
    assert_eq!(
        apply_template("text?{ - ${width}}end", &vars_with_value, None),
        "text - 1920end"
    );
}
