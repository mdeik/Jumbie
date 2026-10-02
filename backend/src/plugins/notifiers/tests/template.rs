use super::*;
use crate::plugins::notifiers::{NotifierContext, NotifierEvent};

#[test]
fn test_simple_substitution() {
    let ctx = TemplateContext::new()
        .with_variable("series", "Test Series")
        .with_variable("title", "Test.Series.S01E02.1080p.WEB-DL");

    assert_eq!(
        render_template("Series: ${series}", &ctx),
        "Series: Test Series"
    );
    assert_eq!(
        render_template("Episode: ${title}", &ctx),
        "Episode: Test.Series.S01E02.1080p.WEB-DL"
    );
}

#[test]
fn test_zero_padding() {
    let ctx = TemplateContext::new()
        .with_variable("season", "1")
        .with_variable("episode", "5");

    assert_eq!(
        render_template("S${season:02}E${episode:02}", &ctx),
        "S01E05"
    );

    assert_eq!(render_template("E${episode:03}", &ctx), "E005");
}

#[test]
fn test_combined_template() {
    let ctx = TemplateContext::new()
        .with_variable("series", "My Show")
        .with_variable("season", "2")
        .with_variable("episode", "12");

    assert_eq!(
        render_template("${series} - S${season:02}E${episode:02}", &ctx),
        "My Show - S02E12"
    );
}

#[test]
fn test_missing_variable() {
    let ctx = TemplateContext::new().with_variable("series", "Show");

    assert_eq!(
        render_template("${series} - ${missing}", &ctx),
        "Show - ${missing}"
    );
}

#[test]
fn test_escaped_braces() {
    let ctx = TemplateContext::new().with_variable("series", "Show");

    assert_eq!(
        render_template("$${series}: ${series}", &ctx),
        "${series}: Show"
    );
}

#[test]
fn test_empty_template() {
    let ctx = TemplateContext::new();
    assert_eq!(render_template("", &ctx), "");
}

#[test]
fn test_no_placeholders() {
    let ctx = TemplateContext::new();
    assert_eq!(
        render_template("No variables here!", &ctx),
        "No variables here!"
    );
}

#[test]
fn test_multiple_same_variable() {
    let ctx = TemplateContext::new().with_variable("name", "Test");

    assert_eq!(
        render_template("${name} ${name} ${name}", &ctx),
        "Test Test Test"
    );
}

#[test]
fn test_large_episode_number() {
    let ctx = TemplateContext::new().with_variable("episode", "999");

    assert_eq!(
        render_template("E${episode:02}", &ctx),
        "E999" // Should not truncate
    );
}

#[test]
fn test_non_numeric_with_format() {
    let ctx = TemplateContext::new().with_variable("text", "abc");

    // Non-numeric values should pass through unchanged even with numeric format
    assert_eq!(render_template("${text:02}", &ctx), "abc");
}

#[test]
fn test_notification_context_to_template_series_title() {
    let ctx = NotifierContext::new()
        .with_series("Test Show")
        .with_release_title("Test.Show.S01E02.1080p.WEB-DL")
        .with_episode(1, 2)
        .with_path("/media/Test.Show/Season 1/S01E02.mkv")
        .with_episode_id("S01E02")
        .with_error("Timeout", "Connection timed out")
        .with_affected_count(5);

    let tmpl = notification_context_to_template(&ctx, &NotifierEvent::DownloadStarted);
    assert_eq!(tmpl.get("series"), Some("Test Show"));
    assert_eq!(tmpl.get("title"), Some("Test.Show.S01E02.1080p.WEB-DL"));
    assert_eq!(tmpl.get("season"), Some("1"));
    assert_eq!(tmpl.get("episode"), Some("2"));
    assert_eq!(
        tmpl.get("path"),
        Some("/media/Test.Show/Season 1/S01E02.mkv")
    );
    assert_eq!(tmpl.get("episode_id"), Some("S01E02"));
    assert_eq!(tmpl.get("error_context"), Some("Timeout"));
    assert_eq!(tmpl.get("error_message"), Some("Connection timed out"));
    assert_eq!(tmpl.get("affected_count"), Some("5"));
    assert_eq!(tmpl.get("event"), Some("DownloadStarted"));

    // Canonical names — no legacy aliases
    assert_eq!(tmpl.get("context"), None);
    assert_eq!(tmpl.get("error"), None);
}

#[test]
fn test_notification_context_to_template_minimal() {
    // Empty context should produce only event
    let tmpl = notification_context_to_template(&NotifierContext::new(), &NotifierEvent::Test);
    assert_eq!(tmpl.get("series"), None);
    assert_eq!(tmpl.get("title"), None);
    assert_eq!(tmpl.get("event"), Some("Test"));
}
