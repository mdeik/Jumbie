// SSoT for parsing user-typed time durations. Pure UI module — no hooks,
// no ConfigBinding. Extracts all `NUMBER [UNIT]` groups, sums them in seconds,
// floor-divides by the native unit, and clamps to [min, max]. Bare numbers use
// the native unit; unparseable text becomes an inline error.

use crate::components::common::form_fields::NumberInput;
use leptos::prelude::*;
use regex::Regex;
use std::sync::LazyLock;

/// Unit context for a time input field.
#[derive(Clone, Copy, PartialEq, Default)]
pub enum TimeUnit {
    /// Stores and displays in seconds. Accepts `s`, `m`, `h`, `d`, `mon`, `y`.
    #[default]
    Seconds,
    /// Stores and displays in minutes. Accepts `s`, `m`, `h`, `d`, `mon`, `y`.
    Minutes,
    /// Stores and displays in hours. Accepts `s`, `m`, `h`, `d`, `mon`, `y`.
    Hours,
    /// Stores and displays in days. Accepts `w`, `d`, `mon`, `y`.
    Days,
}

/// Matches a number followed by an optional time unit identifier.
///
/// Alternation is ordered longest-first so that `second` matches before
/// `sec`, `minute` before `min`, `hour` before `hr`, `month` before `mon`,
/// `year` before `yr`. Single-letter forms (`s`, `m`, `h`, `d`, `y`) are
/// listed last to avoid short-circuiting longer matches.
///
/// The trailing `s?` handles optional plural on the match.
static ITEM_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(\d+)\s*(second|sec|minute|min|hour|hr|day|week|wk|month|mon|year|yr|s|m|h|d|w|y)s?",
    )
    .unwrap()
});

fn unit_multiplier(ident: &str) -> Option<u64> {
    match ident {
        "sec" | "second" | "s" => Some(1),
        "min" | "minute" | "m" => Some(60),
        "hr" | "hour" | "h" => Some(3_600),
        "day" | "d" => Some(86_400),
        "week" | "wk" | "w" => Some(604_800),
        "mon" | "month" => Some(2_592_000),
        "yr" | "year" | "y" => Some(31_536_000),
        _ => None,
    }
}

pub struct TimeParseResult {
    pub value: u64,
    pub error: Option<String>,
}

/// Parse user-typed time input into a native-unit value, clamped to `[min, max]`;
/// unrecognized segments produce an inline error.
pub fn parse_time_input(input: &str, native_unit: TimeUnit, min: u64, max: u64) -> TimeParseResult {
    let input = input.trim().to_lowercase();
    if input.is_empty() {
        return TimeParseResult {
            value: 0,
            error: Some("Enter a duration".to_string()),
        };
    }

    let seconds_per_unit = match native_unit {
        TimeUnit::Seconds => 1,
        TimeUnit::Minutes => 60,
        TimeUnit::Hours => 3_600,
        TimeUnit::Days => 86_400,
    };

    let mut total_seconds: u64 = 0;
    let mut last_end: usize = 0;
    let mut has_match: bool = false;
    let mut invalid_parts: Vec<String> = Vec::new();

    for cap in ITEM_RE.captures_iter(&input) {
        has_match = true;
        let m = cap.get(0).unwrap();

        // Anything between the previous match and this one: check for bare numbers.
        if m.start() > last_end {
            let gap = &input[last_end..m.start()];
            let trimmed = gap.trim();
            if !trimmed.is_empty() {
                if let Ok(n) = trimmed.parse::<u64>() {
                    total_seconds += n.saturating_mul(seconds_per_unit);
                } else {
                    invalid_parts.push(trimmed.to_string());
                }
            }
        }

        let num: u64 = cap[1].parse().unwrap_or(0);
        let ident = cap.get(2).map(|m| m.as_str()).unwrap_or("");
        if let Some(mult) = unit_multiplier(ident) {
            total_seconds += num * mult;
        }

        last_end = m.end();
    }

    if has_match && last_end < input.len() {
        let remaining = input[last_end..].trim();
        if !remaining.is_empty() {
            if let Ok(n) = remaining.parse::<u64>() {
                total_seconds += n.saturating_mul(seconds_per_unit);
            } else {
                invalid_parts.push(remaining.to_string());
            }
        }
    }

    // If no regex match was found, try the entire input as a bare number.
    if !has_match && invalid_parts.is_empty() {
        let trimmed = input.trim();
        if !trimmed.is_empty() {
            if let Ok(n) = trimmed.parse::<u64>() {
                total_seconds += n.saturating_mul(seconds_per_unit);
            } else {
                invalid_parts.push(trimmed.to_string());
            }
        }
    }

    let error = if !invalid_parts.is_empty() {
        Some(format!("Unrecognized: {}", invalid_parts.join(", ")))
    } else {
        None
    };

    if total_seconds == 0 && error.is_some() {
        return TimeParseResult { value: 0, error };
    }

    let mut value = total_seconds / seconds_per_unit;
    if value < min {
        value = min;
    }
    if value > max {
        value = max;
    }

    TimeParseResult { value, error }
}

/// Format a native-unit value into a user-friendly display string, choosing the
/// largest unit that divides cleanly (e.g. `60` minutes → `"1h"`, `7` days →
/// `"1w"`). Zero is displayed as `"0"`.
pub fn format_duration(value: u64, native_unit: TimeUnit) -> String {
    if value == 0 {
        return "0".to_string();
    }
    match native_unit {
        TimeUnit::Seconds => {
            if value >= 86400 && value.is_multiple_of(86400) {
                format!("{}d", value / 86400)
            } else if value >= 3600 && value.is_multiple_of(3600) {
                format!("{}h", value / 3600)
            } else if value >= 60 && value.is_multiple_of(60) {
                format!("{}m", value / 60)
            } else {
                format!("{}s", value)
            }
        }
        TimeUnit::Minutes => {
            if value >= 1440 && value.is_multiple_of(1440) {
                format!("{}d", value / 1440)
            } else if value >= 60 && value.is_multiple_of(60) {
                format!("{}h", value / 60)
            } else {
                format!("{}m", value)
            }
        }
        TimeUnit::Hours => {
            if value >= 24 && value.is_multiple_of(24) {
                format!("{}d", value / 24)
            } else {
                format!("{}h", value)
            }
        }
        TimeUnit::Days => {
            if value >= 365 && value.is_multiple_of(365) {
                format!("{}y", value / 365)
            } else if value >= 30 && value.is_multiple_of(30) {
                format!("{}mon", value / 30)
            } else if value >= 7 && value.is_multiple_of(7) {
                format!("{}w", value / 7)
            } else {
                format!("{}d", value)
            }
        }
    }
}

/// A `NumberInput` wrapper that accepts human-readable durations like `"2h"`,
/// `"30min"`, or `"3d"`. `set_value` emits a plain number string in the native
/// unit; the display formats the stored value back automatically.
///
/// ## Plugin schema usage
///
/// ```json
/// {
///     "refresh_interval": {
///         "type": "integer",
///         "format": "duration",
///         "x-time-unit": "minutes",
///         "title": "Refresh Interval",
///         "default": 10
///     }
/// }
/// ```
#[component]
pub fn TimeInput(
    #[prop(into)] value: Signal<String>,
    #[prop(into)] set_value: Callback<String>,
    #[prop(optional, into)] error: MaybeProp<String>,
    #[prop(optional, into)] _min: Signal<String>,
    #[prop(optional, into)] _max: Signal<String>,
    #[prop(optional, into)] placeholder: Signal<String>,
    #[prop(optional)] id: String,
    #[prop(optional, into)] class: Signal<String>,
    #[prop(optional, into)] disabled: Signal<bool>,
    #[prop(optional, into)] label: Signal<String>,
    #[prop(optional, into)] help_text: Signal<String>,
    #[prop(optional)] autofocus: bool,
    /// The native unit for the stored value. Defaults to `Minutes`.
    #[prop(optional)]
    unit: TimeUnit,
    /// Minimum native-unit value. Applied when parsing user input.
    #[prop(optional)]
    min_value: u64,
    /// Maximum native-unit value. Applied when parsing user input.
    #[prop(optional)]
    max_value: u64,
) -> impl IntoView {
    let internal_error: RwSignal<Option<String>> = RwSignal::new(None);

    let display_val = Signal::derive(move || {
        let raw = value.get();
        let num: u64 = raw.parse().unwrap_or(0);
        format_duration(num, unit)
    });

    // Clear internal error when value changes externally.
    let err_for_effect = internal_error;
    Effect::new(move |_| {
        value.get();
        err_for_effect.set(None);
    });

    let err_for_cb = internal_error;
    let set_val = set_value.clone();
    let parsed_set = Callback::new(move |input: String| {
        let result = parse_time_input(&input, unit, min_value, max_value);
        if let Some(err_msg) = result.error {
            err_for_cb.set(Some(err_msg));
        } else {
            err_for_cb.set(None);
            set_val.run(result.value.to_string());
        }
    });

    let combined_error = Signal::derive(move || {
        let ext = error.get();
        let int = internal_error.get();
        // External error takes precedence; internal error shows when no external.
        ext.or(int)
    });

    view! {
        <NumberInput
            id
            label
            value=display_val
            set_value=parsed_set
            error=combined_error
            help_text
            placeholder
            class
            disabled
            autofocus
            // No min/max HTML attrs — they're meaningless in text mode
        />
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_number_minutes() {
        let r = parse_time_input("30", TimeUnit::Minutes, 0, 1440);
        assert_eq!(r.value, 30);
        assert!(r.error.is_none());
    }

    #[test]
    fn bare_number_days() {
        let r = parse_time_input("7", TimeUnit::Days, 0, 365);
        assert_eq!(r.value, 7);
        assert!(r.error.is_none());
    }

    #[test]
    fn h_short_form() {
        let r = parse_time_input("2h", TimeUnit::Minutes, 0, 1440);
        assert_eq!(r.value, 120);
        assert!(r.error.is_none());
    }

    #[test]
    fn hours_plural() {
        let r = parse_time_input("2 hours", TimeUnit::Minutes, 0, 1440);
        assert_eq!(r.value, 120);
        assert!(r.error.is_none());
    }

    #[test]
    fn seconds_to_minutes_floor() {
        let r = parse_time_input("500sec", TimeUnit::Minutes, 0, 1440);
        assert_eq!(r.value, 8); // 500/60 = 8.33, floor to 8
        assert!(r.error.is_none());
    }

    #[test]
    fn seconds_long_form() {
        let r = parse_time_input("90 seconds", TimeUnit::Minutes, 0, 1440);
        assert_eq!(r.value, 1); // 90/60 = 1.5, floor to 1
        assert!(r.error.is_none());
    }

    #[test]
    fn minutes_short() {
        let r = parse_time_input("45m", TimeUnit::Minutes, 0, 1440);
        assert_eq!(r.value, 45);
        assert!(r.error.is_none());
    }

    #[test]
    fn minutes_plural() {
        let r = parse_time_input("30 minutes", TimeUnit::Minutes, 0, 1440);
        assert_eq!(r.value, 30);
        assert!(r.error.is_none());
    }

    #[test]
    fn days_short() {
        let r = parse_time_input("2d", TimeUnit::Minutes, 0, 10080);
        assert_eq!(r.value, 2880);
        assert!(r.error.is_none());
    }

    #[test]
    fn days_native() {
        let r = parse_time_input("3 days", TimeUnit::Days, 0, 365);
        assert_eq!(r.value, 3);
        assert!(r.error.is_none());
    }

    #[test]
    fn months_to_days() {
        let r = parse_time_input("2mon", TimeUnit::Days, 0, 365);
        assert_eq!(r.value, 60);
        assert!(r.error.is_none());
    }

    #[test]
    fn years_to_days() {
        let r = parse_time_input("1y", TimeUnit::Days, 0, 3650);
        assert_eq!(r.value, 365);
        assert!(r.error.is_none());
    }

    #[test]
    fn years_long_form() {
        let r = parse_time_input("2 years", TimeUnit::Days, 0, 3650);
        assert_eq!(r.value, 730);
        assert!(r.error.is_none());
    }

    #[test]
    fn multi_group_sum() {
        let r = parse_time_input("1h 30m", TimeUnit::Minutes, 0, 1440);
        assert_eq!(r.value, 90);
        assert!(r.error.is_none());
    }

    #[test]
    fn clamps_min() {
        let r = parse_time_input("1", TimeUnit::Minutes, 5, 1440);
        assert_eq!(r.value, 5);
    }

    #[test]
    fn clamps_max() {
        let r = parse_time_input("2000", TimeUnit::Minutes, 5, 1440);
        assert_eq!(r.value, 1440);
    }

    #[test]
    fn empty_input() {
        let r = parse_time_input("", TimeUnit::Minutes, 0, 1440);
        assert!(r.error.is_some());
    }

    #[test]
    fn unknown_unit() {
        let r = parse_time_input("30x", TimeUnit::Minutes, 0, 1440);
        assert!(r.error.is_some());
    }

    #[test]
    fn mixed_valid_and_invalid() {
        let r = parse_time_input("2h abc 30m", TimeUnit::Minutes, 0, 1440);
        assert_eq!(r.value, 150);
        assert!(r.error.is_some());
    }

    #[test]
    fn bare_minutes_seconds_unit() {
        let r = parse_time_input("5", TimeUnit::Seconds, 0, 3600);
        assert_eq!(r.value, 5);
        assert!(r.error.is_none());
    }

    #[test]
    fn minutes_to_seconds() {
        let r = parse_time_input("5min", TimeUnit::Seconds, 0, 3600);
        assert_eq!(r.value, 300); // 5 × 60
        assert!(r.error.is_none());
    }

    #[test]
    fn hours_to_seconds() {
        let r = parse_time_input("2h", TimeUnit::Seconds, 0, 86400);
        assert_eq!(r.value, 7200);
        assert!(r.error.is_none());
    }

    #[test]
    fn bare_hours() {
        let r = parse_time_input("4", TimeUnit::Hours, 0, 168);
        assert_eq!(r.value, 4);
        assert!(r.error.is_none());
    }

    #[test]
    fn minutes_to_hours() {
        let r = parse_time_input("120min", TimeUnit::Hours, 0, 168);
        assert_eq!(r.value, 2);
        assert!(r.error.is_none());
    }

    #[test]
    fn weeks_to_days() {
        let r = parse_time_input("2w", TimeUnit::Days, 0, 365);
        assert_eq!(r.value, 14);
        assert!(r.error.is_none());
    }

    #[test]
    fn week_long_form() {
        let r = parse_time_input("1 week", TimeUnit::Days, 0, 365);
        assert_eq!(r.value, 7);
        assert!(r.error.is_none());
    }

    #[test]
    fn weeks_to_minutes() {
        let r = parse_time_input("1w", TimeUnit::Minutes, 0, 10080);
        assert_eq!(r.value, 10080);
        assert!(r.error.is_none());
    }

    #[test]
    fn wk_short_form() {
        let r = parse_time_input("1 wk", TimeUnit::Days, 0, 365);
        assert_eq!(r.value, 7);
        assert!(r.error.is_none(), "1 wk should not leave k as remainder");
    }

    #[test]
    fn wks_plural() {
        let r = parse_time_input("2 wks", TimeUnit::Days, 0, 365);
        assert_eq!(r.value, 14);
        assert!(r.error.is_none(), "2 wks should parse cleanly");
    }

    #[test]
    fn thirteen_wks() {
        let r = parse_time_input("13 wks", TimeUnit::Days, 0, 365);
        assert_eq!(r.value, 91); // 13 × 7
        assert!(r.error.is_none(), "13 wks should work");
    }

    #[test]
    fn format_zero() {
        assert_eq!(format_duration(0, TimeUnit::Minutes), "0");
        assert_eq!(format_duration(0, TimeUnit::Days), "0");
    }

    #[test]
    fn format_minutes_plain() {
        assert_eq!(format_duration(30, TimeUnit::Minutes), "30m");
        assert_eq!(format_duration(90, TimeUnit::Minutes), "90m");
    }

    #[test]
    fn format_minutes_to_hours() {
        assert_eq!(format_duration(60, TimeUnit::Minutes), "1h");
        assert_eq!(format_duration(120, TimeUnit::Minutes), "2h");
        assert_eq!(format_duration(1440, TimeUnit::Minutes), "1d");
    }

    #[test]
    fn format_days_plain() {
        assert_eq!(format_duration(5, TimeUnit::Days), "5d");
        assert_eq!(format_duration(7, TimeUnit::Days), "1w");
        assert_eq!(format_duration(14, TimeUnit::Days), "2w");
        assert_eq!(format_duration(60, TimeUnit::Days), "2mon");
    }

    #[test]
    fn format_days_to_years() {
        assert_eq!(format_duration(365, TimeUnit::Days), "1y");
        assert_eq!(format_duration(730, TimeUnit::Days), "2y");
    }

    #[test]
    fn format_seconds_plain() {
        assert_eq!(format_duration(30, TimeUnit::Seconds), "30s");
        assert_eq!(format_duration(90, TimeUnit::Seconds), "90s");
    }

    #[test]
    fn format_seconds_to_minutes() {
        assert_eq!(format_duration(60, TimeUnit::Seconds), "1m");
        assert_eq!(format_duration(300, TimeUnit::Seconds), "5m");
    }

    #[test]
    fn format_seconds_to_hours() {
        assert_eq!(format_duration(3600, TimeUnit::Seconds), "1h");
        assert_eq!(format_duration(7200, TimeUnit::Seconds), "2h");
    }

    #[test]
    fn format_seconds_to_days() {
        assert_eq!(format_duration(86400, TimeUnit::Seconds), "1d");
        assert_eq!(format_duration(172800, TimeUnit::Seconds), "2d");
    }

    #[test]
    fn format_hours_plain() {
        assert_eq!(format_duration(1, TimeUnit::Hours), "1h");
        assert_eq!(format_duration(12, TimeUnit::Hours), "12h");
    }

    #[test]
    fn format_hours_to_days() {
        assert_eq!(format_duration(24, TimeUnit::Hours), "1d");
        assert_eq!(format_duration(48, TimeUnit::Hours), "2d");
    }
}
