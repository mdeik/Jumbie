use crate::config::GeneralConfig;
use crate::validation::fields::ValidationError;

// Config field validators: business rules enforced on both the frontend (immediate
// feedback) and backend (data integrity), so a change updates exactly one place.
// Each is a pure function with no I/O, usable in WASM and native alike.

/// Validate that a media info scan interval is at least 1 minute.
pub fn validate_media_info_scan_interval(interval: u64) -> Result<(), ValidationError> {
    if interval < 1 {
        Err(ValidationError(
            "media_info_scan_interval must be at least 1 minute".to_string(),
        ))
    } else {
        Ok(())
    }
}

/// Validate that a season pack replace threshold is between 0 and 100.
pub fn validate_season_pack_replace_threshold(threshold: u32) -> Result<(), ValidationError> {
    if threshold > 100 {
        Err(ValidationError(
            "season_pack_replace_threshold must be between 0 and 100".to_string(),
        ))
    } else {
        Ok(())
    }
}

/// Allowed values for the `unexpected_files_handling` config field.
///
/// SSoT: the frontend dropdown and backend validator both use this list, so adding
/// a handling strategy means updating one place.
pub const VALID_UNEXPECTED_FILES_HANDLING: &[&str] = &["keep", "delete"];

/// Allowed values for the `unneeded_episodes_handling` config field.
/// SSoT: must match the frontend dropdown options in general.rs.
pub const VALID_UNNEEDED_EPISODES_HANDLING: &[&str] = &["delete", "keep"];

/// Validate that `unneeded_episodes_handling` is one of the allowed values.
pub fn validate_unneeded_episodes_handling(handling: &str) -> Result<(), ValidationError> {
    if VALID_UNNEEDED_EPISODES_HANDLING.contains(&handling) {
        Ok(())
    } else {
        Err(ValidationError(format!(
            "Invalid unneeded_episodes_handling '{}': must be one of {}",
            handling,
            VALID_UNNEEDED_EPISODES_HANDLING.join(", ")
        )))
    }
}

/// Validate that `unexpected_files_handling` is one of the allowed values.
pub fn validate_unexpected_files_handling(handling: &str) -> Result<(), ValidationError> {
    if VALID_UNEXPECTED_FILES_HANDLING.contains(&handling) {
        Ok(())
    } else {
        Err(ValidationError(format!(
            "Invalid unexpected_files_handling '{}': must be one of {}",
            handling,
            VALID_UNEXPECTED_FILES_HANDLING.join(", ")
        )))
    }
}

/// Validate that an API key has at least one scope assigned.
pub fn validate_api_key_scopes(count: usize) -> Result<(), ValidationError> {
    if count == 0 {
        Err(ValidationError(
            "API key must have at least one scope".to_string(),
        ))
    } else {
        Ok(())
    }
}

/// Validate that the auto-search interval is within the SSoT bounds defined on
/// [`GeneralConfig`].
pub fn validate_auto_search_wanted_interval(interval: u64) -> Result<(), ValidationError> {
    if interval < GeneralConfig::AUTO_SEARCH_WANTED_INTERVAL_MIN {
        Err(ValidationError(format!(
            "auto_search_wanted_interval must be at least {} minutes",
            GeneralConfig::AUTO_SEARCH_WANTED_INTERVAL_MIN
        )))
    } else if interval > GeneralConfig::AUTO_SEARCH_WANTED_INTERVAL_MAX {
        Err(ValidationError(format!(
            "auto_search_wanted_interval must be at most {} minutes",
            GeneralConfig::AUTO_SEARCH_WANTED_INTERVAL_MAX
        )))
    } else {
        Ok(())
    }
}

/// Validate that the minimum wait time for auto-search is at least the minimum.
pub fn validate_auto_search_wanted_min_wait(wait: u64) -> Result<(), ValidationError> {
    if wait < GeneralConfig::AUTO_SEARCH_WANTED_MIN_WAIT_MIN {
        Err(ValidationError(format!(
            "auto_search_wanted_min_wait must be at least {} minute",
            GeneralConfig::AUTO_SEARCH_WANTED_MIN_WAIT_MIN
        )))
    } else {
        Ok(())
    }
}

/// Validate that the max age for auto-search doesn't exceed the SSoT upper bound.
/// Validate that the auto-search max age doesn't exceed the SSoT upper bound.
pub fn validate_auto_search_wanted_max_age_days(days: u64) -> Result<(), ValidationError> {
    if days > GeneralConfig::AUTO_SEARCH_WANTED_MAX_AGE_DAYS_MAX {
        Err(ValidationError(format!(
            "auto_search_wanted_max_age_days must be at most {} days",
            GeneralConfig::AUTO_SEARCH_WANTED_MAX_AGE_DAYS_MAX
        )))
    } else {
        Ok(())
    }
}

/// Cross-field validation: when `max_age_days > 0`, the search window must be
/// non-empty. If `min_wait > max_age_days * 1440`, no episode can satisfy both
/// "at least min_wait old" and "newer than max_age" — the DB query returns zero
/// rows and the feature silently does nothing.
///
/// This is only checked when both fields are present (the save endpoint passes
/// both). Individual validators run first; this runs after.
pub fn validate_auto_search_wanted_window(
    min_wait: u64,
    max_age_days: u64,
) -> Result<(), ValidationError> {
    if max_age_days > 0 && min_wait > max_age_days * 1440 {
        Err(ValidationError(format!(
            "Search window is empty: min_wait ({} min / {:.1} days) exceeds max_age ({} days). Either reduce min_wait or increase max_age for any episodes to be searchable.",
            min_wait,
            min_wait as f64 / 1440.0,
            max_age_days,
        )))
    } else {
        Ok(())
    }
}

/// Validate that a config timestamp is RFC 3339 with an explicit offset.
///
/// Config fields that carry timestamps (`auth.api_keys[].expires_at`,
/// `auth.banned_ips[].banned_at` / `banned_until`) are plain `String`s, so they
/// are not covered by the structural `UtcDateTime` deserialization used for
/// plugin output. This is the SSoT check that keeps them on-standard at the
/// config boundary — the same contract as every inbound HTTP timestamp.
pub fn validate_rfc3339_timestamp(value: &str, field_name: &str) -> Result<(), ValidationError> {
    match crate::datetime::parse_rfc3339(value) {
        Ok(_) => Ok(()),
        Err(e) => Err(ValidationError(format!("{field_name}: {e}"))),
    }
}
