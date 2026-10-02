use crate::plugins::notifiers::{NotifierContext, NotifierEvent};
use std::collections::HashMap;

/// Context for template rendering.
///
/// Values are pre-resolved to `String` at context-build time so the template
/// engine never deals with type coercion — notifications are always plain text
/// (subject lines, messages, file paths).
#[derive(Debug, Clone, Default)]
pub struct TemplateContext {
    variables: HashMap<String, String>,
}

impl TemplateContext {
    pub fn new() -> Self {
        Self {
            variables: HashMap::new(),
        }
    }

    /// Consuming builder: returns `Self` so a context can be assembled inline. Use
    /// `set` for conditional, multi-statement construction.
    pub fn with_variable(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.variables.insert(key.into(), value.into());
        self
    }

    pub fn set(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.variables.insert(key.into(), value.into());
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.variables.get(key).map(|s| s.as_str())
    }
}

/// Render a template string with variable substitution and formatting.
///
/// Delegates to the canonical engine in `crate::utils::apply_template` (SSoT,
/// shared across notifier channels so parsing/formatting never diverges). This is
/// a thin notifier facade accepting a [`TemplateContext`] rather than a raw map.
///
/// Supports:
/// - Simple substitution: `${variable}` -> value
/// - Zero-padding: `${variable:02}` -> "01" (if value is "1")
/// - Escaped dollar: `$${variable}` -> literal `${variable}`
///
/// # Examples
///
/// ```
/// use jumbie::plugins::notifiers::template::{render_template, TemplateContext};
///
/// let ctx = TemplateContext::new()
///     .with_variable("series", "Test Series")
///     .with_variable("season", "1")
///     .with_variable("episode", "5")
///     .with_variable("title", "Test.Series.S01E05.1080p.WEB-DL");
///
/// let result = render_template("${series} - S${season:02}E${episode:02}", &ctx);
/// assert_eq!(result, "Test Series - S01E05");
/// assert_eq!(render_template("${title}", &ctx), "Test.Series.S01E05.1080p.WEB-DL");
/// ```
pub fn render_template(template: &str, context: &TemplateContext) -> String {
    let vars: HashMap<String, String> = context.variables.clone();
    crate::utils::apply_template(template, &vars, None)
}

/// Build a `TemplateContext` from a `NotifierContext` and `NotifierEvent`.
///
/// The SSoT mapping from notification context fields to template variables — all
/// notifiers delegate here so custom templates (`${series}`, `${season}`,
/// `${path}`, ...) resolve identically on every channel.
///
/// Variables emitted:
///   - `series`            — series name (from `series_title`)
///   - `title`             — episode/release title (from `release_title`, e.g. "Show.Name.S01E02.1080p")
///   - `season`            — season number
///   - `episode`           — episode number
///   - `episode_end`       — end episode for multi-episode ranges (e.g. "3" for S01E01-E03)
///   - `path`              — file path
///   - `episode_id`        — episode identifier (e.g. "S01E02")
///   - `error_context`     — error context label
///   - `error_message`     — error detail
///   - `affected_count`    — number of affected items
///   - `event`             — event type name (e.g. "DownloadStarted")
///
/// `${title}` is the *episode* title, NOT the series name: use `${series}` for the
/// show name and `${title}` for the specific episode/release title.
pub fn notification_context_to_template(
    context: &NotifierContext,
    event: &NotifierEvent,
) -> TemplateContext {
    let mut ctx = TemplateContext::new();

    if let Some(series) = &context.series_title {
        ctx.set("series", series);
    }

    if let Some(title) = &context.release_title {
        ctx.set("title", title);
    }

    if let Some(season) = context.season {
        ctx.set("season", season.to_string());
    }

    if let Some(episode) = context.episode {
        ctx.set("episode", episode.to_string());
    }

    if let Some(ep_end) = context.episode_end {
        ctx.set("episode_end", ep_end.to_string());
    }

    if let Some(path) = &context.path {
        ctx.set("path", path);
    }

    if let Some(error_ctx) = &context.error_context {
        ctx.set("error_context", error_ctx);
    }

    if let Some(error_msg) = &context.error_message {
        ctx.set("error_message", error_msg);
    }

    if let Some(affected_count) = context.affected_count {
        ctx.set("affected_count", affected_count.to_string());
    }

    if let Some(episode_id) = &context.episode_id {
        ctx.set("episode_id", episode_id);
    }

    ctx.set("event", format!("{:?}", event));

    ctx
}

// Tests live in `tests/template.rs` (via `#[path]`) to keep this module focused
// on the public API surface.
#[cfg(test)]
#[path = "tests/template.rs"]
mod tests;
