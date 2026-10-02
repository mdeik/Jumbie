use crate::components::common::form_fields::TooltipBuilder;
use leptos::prelude::*;

/// Reusable badge component.
///
/// Renders `<span class="badge {variant}">{label}</span>`.
///
/// Pass the variant suffix only (e.g. `"danger"`, `"success"`) or the full
/// class string (e.g. `"badge-danger"`) — both forms work because the outer
/// `"badge"` class is always applied automatically.
///
/// # Example
/// ```no_run
/// // view! { <StatusBadge variant="danger" label="Missing" /> }
/// // → <span class="badge badge-danger">Missing</span>
/// ```
#[component]
pub fn StatusBadge(
    /// The colour/variant suffix (e.g. `"danger"`, `"success"`, `"blue"`).
    #[prop(into)]
    variant: String,
    /// The text displayed inside the badge.
    #[prop(into)]
    label: String,
    /// Optional native tooltip title. Ignored when `tooltip` is set.
    #[prop(into, optional)]
    title: Option<String>,
    /// Optional rich tooltip. When set, the shared [`TooltipBuilder`] content is
    /// shown instead of the native `title`.
    #[prop(optional, into)]
    tooltip: MaybeProp<TooltipBuilder>,
) -> impl IntoView {
    let class = format!("badge badge-{}", variant);
    match tooltip.get() {
        Some(tooltip) => tooltip.build_with(&class, label).into_any(),
        None => view! { <span class=class title=title>{label}</span> }.into_any(),
    }
}

/// Badge for a download queue item: reports progress when available, otherwise
/// falls back to the item's status (shared by the desktop table and mobile card).
///
/// - When `progress` is `Some`, renders a `badge-primary` badge filled
///   proportionally to the percentage via an inline gradient.
/// - Otherwise renders a `StatusBadge` for the status string (with the failure
///   message as a tooltip for `Failed` items).
#[component]
pub fn ProgressBadge(
    /// Download progress as a 0.0–1.0 fraction, if known.
    progress: Option<f32>,
    /// The download queue status string (e.g. `"Downloading"`).
    #[prop(into)]
    status: String,
    /// Optional failure message, used as a tooltip for failed items.
    error_message: Option<String>,
    /// True when progress has not advanced past the configured window.
    #[prop(optional)]
    no_progress: bool,
    /// Minutes without progress, shown as the warning tooltip.
    no_progress_minutes: Option<i64>,
) -> impl IntoView {
    if no_progress {
        let tooltip = TooltipBuilder::new()
            .without_title()
            .with_text(no_progress_title(no_progress_minutes));
        return view! {
            <StatusBadge variant="warning" label="No Progress" tooltip=tooltip />
        }
        .into_any();
    }

    if let Some(progress) = progress {
        let pct = (progress * 100.0).round() as i32;
        let label = format!("{:.1}%", progress * 100.0);
        view! {
            <span
                class="badge badge-primary"
                style=format!(
                    "background: linear-gradient(to right, var(--accent-secondary-alpha-35) {}%, var(--accent-secondary-alpha-20) {}%);",
                    pct, pct
                )
            >
                {label}
            </span>
        }
        .into_any()
    } else {
        let variant = download_status_variant(&status);
        let tooltip = if status == "Failed" {
            error_message
                .filter(|m| !m.is_empty())
                .map(|m| TooltipBuilder::new().without_title().with_text(m))
        } else {
            None
        };
        view! {
            <StatusBadge variant=variant label=status tooltip=tooltip />
        }
        .into_any()
    }
}

/// Tooltip for the no-progress warning badge: how long the download has been
/// without progress, when known.
pub fn no_progress_title(no_progress_minutes: Option<i64>) -> String {
    match no_progress_minutes {
        Some(m) => format!("No progress for {} min", m),
        None => "No progress".to_string(),
    }
}

/// Maps a **download queue** status string to its badge variant.
/// This is the single source of truth for download status colours.
///
/// Returns the variant suffix (e.g. `"warning"`, `"success"`) to be used with
/// `StatusBadge` or composed into a class string directly.
pub fn download_status_variant(status: &str) -> &'static str {
    match status {
        "Paused" => "warning",
        "Queued" => "secondary",
        "Downloading" => "primary",
        "Organizing" => "purple",

        "Failed" => "danger",
        "Completed" => "success",
        // Kept for manual review: nothing was assigned, but it is not an error.
        "Review" => "warning",
        _ => "secondary",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_progress_title_includes_minutes_when_known() {
        assert_eq!(no_progress_title(Some(42)), "No progress for 42 min");
    }

    #[test]
    fn no_progress_title_falls_back_without_minutes() {
        assert_eq!(no_progress_title(None), "No progress");
    }

    /// `Review` (a download that placed no intended files) must keep its distinct
    /// warning colour rather than falling through to the neutral default.
    #[test]
    fn review_status_is_warning_variant() {
        assert_eq!(download_status_variant("Review"), "warning");
    }
}
