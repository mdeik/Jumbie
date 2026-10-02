use super::*;

#[test]
fn test_validate_path() {
    let allowed = std::path::Path::new("/allowed/root");

    assert!(validate_path("./test/path", allowed).is_ok());
    // Unix-style absolute paths are valid on Linux (is_relative returns false,
    // path is used as-is and canonicalized), but rejected on Windows where
    // they are treated as root-relative and could bypass allowed_root.
    #[cfg(not(windows))]
    assert!(validate_path("/allowed/root/path", allowed).is_ok());
    #[cfg(windows)]
    assert!(
        validate_path("/allowed/root/path", allowed).is_err(),
        "Unix absolute paths must be rejected on Windows"
    );
    assert!(validate_path("relative/path", allowed).is_ok());

    assert!(validate_path("", allowed).is_err());
    assert!(validate_path("   ", allowed).is_err());
    assert!(validate_path("../../../etc/passwd", allowed).is_err());
    assert!(validate_path("test/../../../etc", allowed).is_err());
    assert!(validate_path("test//double", allowed).is_err());
}

#[test]
fn test_validate_title() {
    assert!(validate_title("Normal Title").is_ok());
    assert!(validate_title("").is_err());
    assert!(validate_title("Title\nWith\nNewlines").is_err());
    let long_title = "a".repeat(10001);
    assert!(validate_title(&long_title).is_err());
}

#[test]
fn test_validate_regex() {
    assert!(validate_regex(r"\d+").is_ok());
    assert!(validate_regex("").is_ok());
    assert!(validate_regex("[a-z]+").is_ok());
    assert!(validate_regex("(unclosed group").is_err());
    assert!(validate_regex("[unclosed").is_err());
}

#[test]
fn test_validate_url() {
    assert!(validate_url("http://example.com").is_ok());
    assert!(validate_url("https://example.com").is_ok());
    assert!(validate_url("").is_err());
    assert!(validate_url("ftp://example.com").is_err());
    assert!(validate_url("not a url").is_err());
    assert!(validate_url("http://example.com/with space").is_err());
}

#[test]
fn test_validate_not_empty() {
    assert!(validate_not_empty("value", "field").is_ok());
    assert!(validate_not_empty("", "field").is_err());
    assert!(validate_not_empty("   ", "field").is_err());
}

#[test]
fn test_validate_episode_number() {
    assert!(validate_episode_number(1).is_ok());
    assert!(validate_episode_number(100).is_ok());
    assert!(validate_episode_number(10000).is_ok());
    assert!(validate_episode_number(0).is_err());
    assert!(validate_episode_number(-1).is_err());
    assert!(validate_episode_number(10001).is_err());
}

#[test]
fn test_validate_season_number() {
    assert!(validate_season_number("1").is_ok());
    assert!(validate_season_number("0").is_ok()); // Season 0 is often used for specials
    assert!(validate_season_number("2023").is_ok()); // Release year as season number
    assert!(validate_season_number("10000").is_ok());
    assert!(validate_season_number("-1").is_err());
    assert!(validate_season_number("abc").is_err());
    assert!(validate_season_number("").is_err());
    assert!(validate_season_number("10001").is_err());
}

#[test]
fn test_validate_episode_offset() {
    assert!(validate_episode_offset(0).is_ok());
    assert!(validate_episode_offset(100).is_ok());
    assert!(validate_episode_offset(-100).is_ok());
    assert!(validate_episode_offset(10000).is_ok());
    assert!(validate_episode_offset(-10000).is_ok());

    assert!(validate_episode_offset(10001).is_err());
    assert!(validate_episode_offset(-10001).is_err());
}

#[test]
fn test_validate_quality_profile() {
    assert!(validate_quality_profile("1080p Web").is_ok());
    assert!(
        validate_quality_profile("").is_ok(),
        "Empty quality profile should pass validation"
    );
    assert!(validate_quality_profile("   ").is_ok());

    let long_profile = "a".repeat(101);
    assert!(
        validate_quality_profile(&long_profile).is_err(),
        "Profiles over 100 chars should fail validation"
    );
}

#[test]
fn test_contains_path_traversal() {
    // Actual traversal sequences — must be flagged.
    assert!(contains_path_traversal("../etc/passwd"), "starts with ../");
    assert!(contains_path_traversal("foo/../bar"), "contains /../");
    assert!(contains_path_traversal(".."), "exact ..");
    assert!(contains_path_traversal("foo/.."), "ends with /..");
    assert!(contains_path_traversal("/../"), "just /../");
    // Windows-style backslash traversal is caught on all platforms: on Unix `\`
    // is a literal character, so the extra backslash guard flags it as injection.
    assert!(
        contains_path_traversal("..\\etc"),
        "backslash start should be flagged"
    );
    assert!(
        contains_path_traversal("foo\\..\\bar"),
        "backslash embedded should be flagged"
    );
    assert!(
        contains_path_traversal("foo\\.."),
        "backslash end should be flagged"
    );

    // Dots embedded in filenames — must not be flagged.
    assert!(
        !contains_path_traversal("Part..2"),
        "dots within filename component"
    );
    assert!(
        !contains_path_traversal("Version..final"),
        "trailing dots in stem"
    );
    assert!(
        !contains_path_traversal("..Hidden"),
        "dots prefix (not a component)"
    );
    assert!(!contains_path_traversal("normal.mkv"), "normal filename");
    assert!(
        !contains_path_traversal("path/to/file..mkv"),
        "dots before ext"
    );
    assert!(
        !contains_path_traversal("show.s01e01.mkv"),
        "dots as separators"
    );
    assert!(!contains_path_traversal(""), "empty string");
}

#[test]
fn test_validate_id() {
    assert!(validate_id("550e8400-e29b-41d4-a716-446655440000", "id").is_ok());
    assert!(validate_id("S01E05_550e8400", "id").is_ok());
    assert!(validate_id("a", "id").is_ok());

    assert!(validate_id("", "id").is_err());
    assert!(validate_id("a\0b", "id").is_err());
    let long = "x".repeat(513);
    assert!(validate_id(&long, "id").is_err());
}

#[test]
fn test_validate_path_max_length() {
    let allowed = std::path::Path::new("/");
    let long_path = "a".repeat(4097);
    assert!(validate_path(&long_path, allowed).is_err());

    // Relative path works on all platforms; Unix-style absolutes are rejected on Windows.
    assert!(validate_path("./short/path", allowed).is_ok());
}
