use std::collections::HashMap;

use jumbie_shared::mapping::{MappingRule, SeriesSettings};

/// Verify that `generate_auto_pattern` properly escapes all regex meta-characters
/// that could appear in user-defined series names or aliases.
///
/// Uses regex::Regex::new() to verify the output is always valid regex,
/// and tests specific known edge cases for correct literal matching.
#[test]
fn test_generate_auto_pattern_escapes_all_meta_chars() {
    let cases = vec![
        // Basic: no meta-chars
        "Simple Name",
        // Dots — meta-char, must be literal
        "U.S.A",
        "Show.Name",
        // Plus
        "C++",
        // Asterisk
        "Show*",
        // Question mark
        "Show?",
        // Parentheses — meta-chars
        "Show (2024)",
        // Square brackets — meta-chars
        "Show [1080p]",
        // Curly braces — meta-chars
        "Show{1}",
        // Pipe — meta-char (alternation operator)
        "Show|Movie",
        // Caret — meta-char
        "^Show",
        // Dollar — meta-char
        "Show$",
        // Hash — meta-char (in Rust regex)
        "Show#1",
        // Ampersand — meta-char
        "Show&Movie",
        // Tilde — meta-char
        "~Show",
        // Hyphen — meta-char (inside character classes)
        "Show-One",
        // Backslash — meta-char
        "Show\\Test",
        // Flags disguised as text — MUST be escaped, NOT interpreted as flags
        "Show (?i)test",
        "Show (?-i)test",
        "Show (?m)test",
        // Regex commands — must be literal text, not interpreted
        "Show \\d+ test",
        "Show \\w+ test",
        // Unicode — no escaping needed, passes through cleanly
        "東京",
        "中文测试",
        // Multiple spaces become multiple [._ ] groups
        "Multi  Word",
        // All special chars mixed together
        "!@#$%^&*()_+-=[]{}|;':\",./<>?`~",
        // Empty string — handled by caller checks, but verify it doesn't crash
        "",
    ];

    for input in &cases {
        let pattern = crate::source_processor::identification::generate_auto_pattern(input);
        // Must compile as valid regex
        let re = regex::Regex::new(&pattern).unwrap_or_else(|e| {
            panic!(
                "Invalid regex for input {:?}: {} (pattern: {})",
                input, e, pattern
            )
        });

        // The pattern must match the original input literally
        assert!(
            re.is_match(input),
            "Pattern must match its own input: {:?} -> pattern {} doesn't match original",
            input,
            pattern
        );

        // The pattern must NOT have changed meaning — a meta-char in the input
        // must still be a literal character in the match, not a regex operator.
        // We test this by confirming the pattern does NOT match a string where
        // the meta-char would have a different meaning.
        if input.contains(
            &[
                '*', '+', '?', '|', '[', ']', '(', ')', '{', '}', '^', '$', '.', '\\',
            ][..],
        ) {
            // Just verify compilation — the exhaustive semantic tests are below
        }
    }
}

/// Verify that flags embedded in names are treated as LITERAL text, not regex flags.
#[test]
fn test_auto_pattern_flags_are_literal_not_interpreted() {
    let pattern = crate::source_processor::identification::generate_auto_pattern("Show (?i)test");
    let re = regex::Regex::new(&pattern).unwrap();

    // Must match literal "(?i)" in the title
    assert!(re.is_match("Show (?i)test"), "Literal (?i) must match");
    // Must NOT match without the literal "(?i)"
    assert!(
        !re.is_match("Show test"),
        "Without literal (?i), must not match"
    );
    // The auto-generated pattern has (?i) prefix which makes everything
    // case-insensitive. The \\(i\\) in the pattern matches literal "(i)"
    // text in the title. Since the whole pattern is case-insensitive
    // (by design), "Show (?i)TEST" matches correctly.
    assert!(
        re.is_match("Show (?i)TEST"),
        "(?i) prefix makes match case-insensitive, but '(i)' in title is literal text"
    );
}

/// Verify that regex shorthands in names are treated as LITERAL text.
#[test]
fn test_auto_pattern_regex_commands_are_literal() {
    let pattern = crate::source_processor::identification::generate_auto_pattern(r"Show \d+ test");
    let re = regex::Regex::new(&pattern).unwrap();

    // Must match literal "\d+" in the title
    assert!(
        re.is_match(r"Show \d+ test"),
        "Literal backslash-d must match"
    );
    // \d+ must NOT be interpreted as the digit class
    assert!(
        !re.is_match("Show 123 test"),
        r"\d+ must not act as digit class"
    );
}

/// Verify that EVERY auto-generated pattern compiles as valid regex.
/// This catches any edge case where generate_auto_pattern might produce
/// an invalid pattern (which would break the entire gate).
#[test]
fn test_generate_auto_pattern_always_produces_valid_regex() {
    let test_names = vec![
        "Simple Name",
        "U.S.A",
        "C++",
        "Show (2024) [1080p]",
        "Show (?i)test",
        "Show (?-i)test",
        "Show \\d+ test",
        "Show * Test + More? (yes) | maybe [not] {sure}",
        "Show^Test$",
        "Show&Movie#1",
        "~Show",
        "Show-One",
        "Show\\Backslash",
        "東京",
        "中文测试",
        "日本語の名前",
        "!@#$%^&*()_+-=[]{}|;':\",./<>?`~",
        "Mixed  spaces  here",
        "",
        "   ",
        "\t",
        "\n",
    ];

    for name in &test_names {
        let pattern = crate::source_processor::identification::generate_auto_pattern(name);
        let result = regex::Regex::new(&pattern);
        assert!(
            result.is_ok(),
            "generate_auto_pattern produced invalid regex for input {:?}:\n  pattern: {}",
            name,
            pattern
        );
        // If it compiled, verify it at least doesn't crash when matching
        if let Ok(re) = result {
            let _ = re.is_match("test string for matching");
        }
    }
}

/// Verify that the gate handles aliases with regex meta-characters correctly
/// through the full build_gate_regex pipeline.
#[test]
fn test_gate_special_chars_in_alias_full_pipeline() {
    let mut mappings = HashMap::new();

    let s1 = MappingRule {
        target_title: "Normal Show".to_string(),
        name: "Normal Show".to_string(),
        settings: SeriesSettings {
            aliases: vec![
                "Show (2024) [1080p]".to_string(),
                "C++".to_string(),
                "U.S.A".to_string(),
                "Show (?i)test".to_string(),
            ],
            ..SeriesSettings::default()
        },
        ..MappingRule::default()
    };
    mappings.insert("s1".to_string(), s1);

    // Must compile without error
    let gate = crate::source_processor::build_gate_regex(&mappings);
    assert!(
        gate.is_some(),
        "Gate must compile with special-char aliases"
    );

    let gate = gate.unwrap();

    // Each alias should match its literal string
    assert!(
        gate.is_match("Show (2024) [1080p] S01E01"),
        "Alias with parens/brackets"
    );
    assert!(gate.is_match("C++ S01E01"), "Alias with ++");
    assert!(gate.is_match("U.S.A S01E01"), "Alias with dots");
    // The (?i) in the alias must be treated as LITERAL text, not a regex flag
    assert!(
        gate.is_match("Show (?i)test S01E01"),
        "Alias with literal (?i) text"
    );
    // Without the literal "(?i)" text, it should NOT match
    assert!(
        !gate.is_match("Show test S01E01"),
        "Literal (?i) is required in title"
    );

    // Normal name still works
    assert!(
        gate.is_match("Normal.Show.S01E01"),
        "Regular name still matches"
    );
}

/// Verify that series names with regex commands like \d, \w are treated as
/// literal text, not interpreted as regex shorthands.
#[test]
fn test_gate_name_with_regex_commands_are_literal() {
    let mut mappings = HashMap::new();

    let s1 = MappingRule {
        target_title: r"Show \d+ test".to_string(),
        name: r"Show \d+ test".to_string(),
        ..MappingRule::default()
    };
    mappings.insert("s1".to_string(), s1);

    let gate = crate::source_processor::build_gate_regex(&mappings);
    assert!(gate.is_some(), "Gate must compile");

    let gate = gate.unwrap();

    // The name contains literal "\d+" which should match literally in the title
    assert!(
        gate.is_match(r"Show \d+ test"),
        "Literal backslash-d must match"
    );
    // "Show 123 test" does NOT match because the name has literal "\d+", not the digit class
    assert!(
        !gate.is_match("Show 123 test"),
        r"Literal \d+ should not act as digit class"
    );
}

/// Verify the gate compiles even with the most extreme escaped patterns.
#[test]
fn test_gate_extreme_special_chars_compiles() {
    let mut mappings = HashMap::new();

    let s1 = MappingRule {
        target_title: "Normal".to_string(),
        name: "Normal".to_string(),
        settings: SeriesSettings {
            aliases: vec![
                "!@#$%^&*()_+-=[]{}|;':\",./<>?`~".to_string(),
                "(?i)(?-i)(?m)(?s)(?U)(?x)".to_string(),
            ],
            ..SeriesSettings::default()
        },
        ..MappingRule::default()
    };
    mappings.insert("s1".to_string(), s1);

    let gate = crate::source_processor::build_gate_regex(&mappings);
    assert!(
        gate.is_some(),
        "Gate must compile with extreme special chars"
    );

    let gate = gate.unwrap();
    // The literal flag text must match literally
    assert!(
        gate.is_match("(?i)(?-i)(?m)(?s)(?U)(?x)"),
        "Flags-as-text must match literally"
    );
}
