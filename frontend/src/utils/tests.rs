// Tests for the `utils` module.
//
// Three categories live here, gated appropriately:
//
//   1. **Pure native tests** (`#[test]`) — run with `cargo test --lib`.
//   2. **WASM tests** (`#[wasm_bindgen_test]`, gated on
//      `cfg(target_arch = "wasm32")`) — run via `wasm-pack test --node`
//      (what CI runs). No browser is required.
//   3. **`use_autosave`** — requires both WASM + Leptos reactive runtime.

use super::*;
use chrono::NaiveDate;
use jumbie_shared::config::TimeFormat;
use jumbie_shared::types::MonitorMode;
use jumbie_shared::types::{EpisodeViewModel, ReleaseDates};

// format_size

#[test]
fn test_format_size_zero() {
    assert_eq!(format_size(0), "0 B");
}

#[test]
fn test_format_size_bytes() {
    assert_eq!(format_size(1), "1 B");
    assert_eq!(format_size(512), "512 B");
    assert_eq!(format_size(1023), "1023 B");
}

#[test]
fn test_format_size_kilobytes() {
    assert_eq!(format_size(1024), "1.00 KB");
    assert_eq!(format_size(1536), "1.50 KB");
    assert_eq!(format_size(2048), "2.00 KB");
    // Just below MB boundary
    let just_below_mb = 1024 * 1024 - 1;
    assert_eq!(format_size(just_below_mb), "1024.00 KB");
}

#[test]
fn test_format_size_megabytes() {
    assert_eq!(format_size(1024 * 1024), "1.00 MB");
    assert_eq!(format_size(5 * 1024 * 1024), "5.00 MB");
    // Fractional MB
    let one_and_half_mb = 1024 * 1024 + 512 * 1024;
    assert_eq!(format_size(one_and_half_mb), "1.50 MB");
    // Just below GB boundary
    let just_below_gb = 1024u64 * 1024 * 1024 - 1;
    assert_eq!(format_size(just_below_gb), "1024.00 MB");
}

#[test]
fn test_format_size_gigabytes() {
    assert_eq!(format_size(1024u64 * 1024 * 1024), "1.00 GB");
    assert_eq!(format_size(2 * 1024u64 * 1024 * 1024), "2.00 GB");
    // Fractional GB
    let val = 1024u64 * 1024 * 1024 + 512 * 1024 * 1024;
    assert_eq!(format_size(val), "1.50 GB");
    // Just below TB boundary
    let just_below_tb = 1024u64 * 1024 * 1024 * 1024 - 1;
    assert_eq!(format_size(just_below_tb), "1024.00 GB");
}

#[test]
fn test_format_size_terabytes() {
    assert_eq!(format_size(1024u64 * 1024 * 1024 * 1024), "1.00 TB");
    assert_eq!(format_size(5 * 1024u64 * 1024 * 1024 * 1024), "5.00 TB");
}

#[test]
fn test_format_size_large_values() {
    // u64::MAX is about 16 exabytes — ensure no overflow
    let huge = u64::MAX;
    let result = format_size(huge);
    assert!(result.ends_with(" TB") || result.ends_with(" PB"));
    // Should produce a sane formatted number
    assert!(!result.contains("inf"));
    assert!(!result.contains("nan"));
}

// parse_timestamp_utc / format_datetime_local

#[test]
fn test_format_datetime_local_24h() {
    let result = format_datetime_local("2024-03-15T14:30:00Z", &TimeFormat::Hour24);
    // Expect "03-15-24 14:30" in any timezone; we just verify structure
    assert_eq!(&result[..8], "03-15-24");
    assert!(result.contains(":30"));
}

#[test]
fn test_format_datetime_local_12h() {
    let result = format_datetime_local("2024-03-15T14:30:00Z", &TimeFormat::Hour12);
    assert_eq!(&result[..8], "03-15-24");
    assert!(result.contains(":30"));
    assert!(result.contains("PM") || result.contains("AM"));
}

#[test]
fn test_format_datetime_local_12h_morning() {
    // 09:30 UTC should be morning local in most timezones
    let result = format_datetime_local("2024-03-15T09:30:00Z", &TimeFormat::Hour12);
    assert_eq!(&result[..8], "03-15-24");
    assert!(result.contains("AM") || result.contains("PM"));
}

#[test]
fn test_parse_timestamp_utc_rfc3339() {
    let dt = parse_timestamp_utc("2024-03-15T14:30:00Z").expect("rfc3339 should parse");
    assert_eq!(dt.to_rfc3339(), "2024-03-15T14:30:00+00:00");
}

#[test]
fn test_parse_timestamp_utc_naive_is_utc() {
    // Legacy/DB naive shapes carry no zone and must be treated as UTC.
    let spaced = parse_timestamp_utc("2024-03-15 14:30:00").expect("naive space should parse");
    let t = parse_timestamp_utc("2024-03-15T14:30:00").expect("naive T should parse");
    assert_eq!(spaced, t);
    assert_eq!(spaced.to_rfc3339(), "2024-03-15T14:30:00+00:00");
}

#[test]
fn test_parse_timestamp_utc_naive_fractional() {
    let dt = parse_timestamp_utc("2024-03-15T14:30:00.123456").expect("fractional should parse");
    assert_eq!(dt.to_rfc3339(), "2024-03-15T14:30:00+00:00");
}

#[test]
fn test_parse_timestamp_utc_date_only_is_midnight_utc() {
    // Unified on the shared parser (`jumbie_shared::datetime::parse_utc`), which
    // anchors date-only input at midnight UTC.
    let dt = parse_timestamp_utc("2024-03-15").expect("date-only should parse");
    assert_eq!(dt.to_rfc3339(), "2024-03-15T00:00:00+00:00");
}

#[test]
fn test_parse_timestamp_utc_invalid() {
    assert!(parse_timestamp_utc("not a timestamp").is_none());
    assert!(parse_timestamp_utc("").is_none());
}

#[test]
fn test_format_datetime_local_naive_utc() {
    // Same instant expressed as RFC 3339 and as a naive-UTC string must render identically.
    let rfc = format_datetime_local("2024-03-15T14:30:00Z", &TimeFormat::Hour24);
    let naive = format_datetime_local("2024-03-15 14:30:00", &TimeFormat::Hour24);
    assert_eq!(rfc, naive);
}

#[test]
fn test_format_datetime_local_invalid_fallback() {
    let result = format_datetime_local("not a timestamp", &TimeFormat::Hour24);
    assert_eq!(result, "not a timestamp");
}

#[test]
fn test_format_datetime_local_empty() {
    let result = format_datetime_local("", &TimeFormat::Hour24);
    assert_eq!(result, "");
}

// format_age

#[test]
fn test_format_age_future_date_returns_not_yet_released() {
    // A future date must report "Not yet released" rather than a negative age.
    let future = (Utc::now() + chrono::Duration::days(7)).to_rfc3339();
    assert_eq!(format_age(&future), "Not yet released");
}

#[test]
fn test_format_age_past_date_returns_relative() {
    let past = (Utc::now() - chrono::Duration::days(3)).to_rfc3339();
    assert_eq!(format_age(&past), "3d ago");
}

#[test]
fn test_format_age_just_now() {
    let now = Utc::now().to_rfc3339();
    assert_eq!(format_age(&now), "Just now");
}

#[test]
fn test_format_age_empty_string() {
    assert_eq!(format_age(""), "");
}

#[test]
fn test_format_age_naive_utc() {
    // Naive-UTC input is accepted and treated as UTC.
    let past = (Utc::now() - chrono::Duration::days(3))
        .naive_utc()
        .to_string();
    assert_eq!(format_age(&past), "3d ago");
}

// build_sorted_profile_options

#[test]
fn test_build_sorted_profile_options_empty() {
    let options = build_sorted_profile_options(&[], "None", false);
    assert_eq!(options.len(), 1);
    assert_eq!(options[0].first_value(), Some("".to_string()));
}

#[test]
fn test_build_sorted_profile_options_single() {
    let options = build_sorted_profile_options(
        &[("id-1".to_string(), "HD-1080p".to_string())],
        "None",
        false,
    );
    assert_eq!(options.len(), 2);
    // First is always placeholder
    assert_eq!(options[0].first_value(), Some("".to_string()));
    // Second is the profile — value is the ID, label is the name
    assert_eq!(options[1].first_value(), Some("id-1".to_string()));
}

#[test]
fn test_build_sorted_profile_options_placeholder() {
    // Custom placeholder (used by bulk-edit selectors)
    let options = build_sorted_profile_options(&[], "— Quality —", false);
    assert_eq!(options.len(), 1);
    assert_eq!(options[0].first_value(), Some("".to_string()));
}

#[test]
fn test_build_sorted_profile_options_clear_option() {
    // Clear option appended when flag is true (used by bulk-edit)
    let options = build_sorted_profile_options(
        &[("id-1".to_string(), "HD".to_string())],
        "— Profile —",
        true,
    );
    assert_eq!(options.len(), 3);
    assert_eq!(options[0].first_value(), Some("".to_string())); // placeholder
    assert_eq!(options[1].first_value(), Some("id-1".to_string())); // profile
    assert_eq!(options[2].first_value(), Some("__clear__".to_string())); // clear
    // Label for the clear option is "None" (the only string with value="__clear__")
}

#[test]
fn test_build_sorted_profile_options_multiple() {
    let options = build_sorted_profile_options(
        &[
            ("id-4k".to_string(), "UHD-4K".to_string()),
            ("id-1080".to_string(), "HD-1080p".to_string()),
            ("id-480".to_string(), "SD-480p".to_string()),
        ],
        "None",
        false,
    );
    assert_eq!(options.len(), 4);

    // First is always placeholder
    assert_eq!(options[0].first_value(), Some("".to_string()));

    // Remaining should be alphabetically sorted by NAME (label), but values are IDs
    let values: Vec<Option<String>> = options.iter().map(|o| o.first_value()).collect();
    assert_eq!(values[1], Some("id-1080".to_string())); // HD-1080p
    assert_eq!(values[2], Some("id-480".to_string())); // SD-480p
    assert_eq!(values[3], Some("id-4k".to_string())); // UHD-4K
}

#[test]
fn test_build_sorted_profile_options_none_always_first() {
    // Verify placeholder is *always* at position 0 regardless of alphabetical order
    let options =
        build_sorted_profile_options(&[("id-a".to_string(), "AAAA".to_string())], "None", false);
    assert_eq!(options[0].first_value(), Some("".to_string()));
    assert_eq!(options[1].first_value(), Some("id-a".to_string()));
}

#[test]
fn test_build_sorted_profile_options_case_sensitive_sort() {
    // Rust's default sort is case-sensitive (uppercase before lowercase).
    // This test documents that behaviour so if it ever changes, the test fails.
    let options = build_sorted_profile_options(
        &[
            ("id-apple".to_string(), "apple".to_string()),
            ("id-banana".to_string(), "Banana".to_string()),
            ("id-cherry".to_string(), "Cherry".to_string()),
        ],
        "None",
        false,
    );
    // 'B' < 'a' in ASCII, so Banana comes before apple
    assert_eq!(options[1].first_value(), Some("id-banana".to_string()));
    assert_eq!(options[2].first_value(), Some("id-cherry".to_string()));
    assert_eq!(options[3].first_value(), Some("id-apple".to_string()));
}

#[test]
fn test_build_sorted_profile_options_duplicate_names() {
    // Duplicates are preserved — the function doesn't deduplicate
    let options = build_sorted_profile_options(
        &[
            ("id-hd1".to_string(), "HD".to_string()),
            ("id-hd2".to_string(), "HD".to_string()),
            ("id-sd".to_string(), "SD".to_string()),
        ],
        "None",
        false,
    );
    assert_eq!(options.len(), 4);
    assert_eq!(options[1].first_value(), Some("id-hd1".to_string()));
    assert_eq!(options[2].first_value(), Some("id-hd2".to_string()));
    assert_eq!(options[3].first_value(), Some("id-sd".to_string()));
}

// parse_monitor_mode

#[test]
fn test_parse_monitor_mode_all() {
    assert_eq!(parse_monitor_mode("All"), Some(MonitorMode::All));
}

#[test]
fn test_parse_monitor_mode_future() {
    assert_eq!(parse_monitor_mode("Future"), Some(MonitorMode::Future));
}

#[test]
fn test_parse_monitor_mode_missing() {
    assert_eq!(parse_monitor_mode("Missing"), Some(MonitorMode::Missing));
}

#[test]
fn test_parse_monitor_mode_existing() {
    assert_eq!(parse_monitor_mode("Existing"), Some(MonitorMode::Existing));
}

#[test]
fn test_parse_monitor_mode_pilot() {
    assert_eq!(parse_monitor_mode("Pilot"), Some(MonitorMode::Pilot));
}

#[test]
fn test_parse_monitor_mode_first_season() {
    assert_eq!(
        parse_monitor_mode("FirstSeason"),
        Some(MonitorMode::FirstSeason)
    );
}

#[test]
fn test_parse_monitor_mode_specials() {
    assert_eq!(parse_monitor_mode("Specials"), Some(MonitorMode::Specials));
}

#[test]
fn test_parse_monitor_mode_none() {
    assert_eq!(parse_monitor_mode("None"), Some(MonitorMode::None));
}

#[test]
fn test_parse_monitor_mode_empty() {
    assert_eq!(parse_monitor_mode(""), None);
}

#[test]
fn test_parse_monitor_mode_unknown() {
    assert_eq!(parse_monitor_mode("Invalid"), None);
}

#[test]
fn test_parse_monitor_mode_case_sensitive() {
    // The function is case-sensitive — must match exact casing from MONITOR_OPTIONS
    assert_eq!(parse_monitor_mode("all"), None);
    assert_eq!(parse_monitor_mode("FIRSTSEASON"), None);
}

// resolve_active_metadata_plugins — backend-stamped instance identity

fn metadata_instance_config(enabled: bool, name: &str) -> serde_json::Value {
    serde_json::json!({ "enabled": enabled, "name": name })
}

fn stamped_metadata_instance(
    uuid: &str,
    type_id: &str,
    name: &str,
) -> jumbie_shared::plugin::PluginInstanceInfo {
    jumbie_shared::plugin::PluginInstanceInfo {
        plugin_id: Some(type_id.to_string()),
        instance_id: Some(uuid.to_string()),
        display_name: name.to_string(),
        version: "1.0.0".to_string(),
        author: jumbie_shared::plugin::JUMBIE_AUTHOR.to_string(),
        description: String::new(),
        capabilities: vec![jumbie_shared::plugin::Capability::MetadataProviderNormal],
        supported_protocols: None,
        series_identifier_label: None,
        series_identifier_placeholder: None,
        rate_limit: None,
        supports_test: false,
    }
}

fn metadata_cfg(instances: &[(&str, bool, &str)]) -> jumbie_shared::config::PluginsConfig {
    let mut cfg = jumbie_shared::config::PluginsConfig::default();
    cfg.metadata.insert(
        "jumbie.tvdb".to_string(),
        instances
            .iter()
            .map(|(uuid, enabled, name)| {
                (uuid.to_string(), metadata_instance_config(*enabled, name))
            })
            .collect(),
    );
    cfg
}

#[test]
fn test_resolve_matches_backend_stamped_instances_by_uuid() {
    let cfg = metadata_cfg(&[("uuid-1", true, "My TVDB")]);
    let instance = stamped_metadata_instance("uuid-1", "jumbie.tvdb", "My TVDB");
    let resolved = resolve_active_metadata_plugins(vec![instance], &cfg);
    assert_eq!(resolved.len(), 1);
    assert_eq!(resolved[0].instance_id.as_deref(), Some("uuid-1"));
    assert_eq!(resolved[0].plugin_id.as_deref(), Some("jumbie.tvdb"));
    assert_eq!(resolved[0].display_name, "My TVDB");
}

#[test]
fn test_resolve_skips_disabled_instances() {
    let cfg = metadata_cfg(&[("uuid-1", false, "Off")]);
    let resolved = resolve_active_metadata_plugins(vec![], &cfg);
    assert!(resolved.is_empty());
}

#[test]
fn test_resolve_fallback_for_unloaded_instance() {
    let cfg = metadata_cfg(&[("uuid-1", true, "My TVDB")]);
    let resolved = resolve_active_metadata_plugins(vec![], &cfg);
    assert_eq!(resolved.len(), 1);
    assert_eq!(resolved[0].instance_id.as_deref(), Some("uuid-1"));
    assert_eq!(resolved[0].plugin_id, None);
    assert_eq!(resolved[0].display_name, "My TVDB");
}

#[test]
fn test_resolve_ignores_non_metadata_instances() {
    // A source instance sharing the uuid must NOT satisfy a metadata entry —
    // only metadata-capable backend instances are matched.
    let cfg = metadata_cfg(&[("uuid-1", true, "TVDB")]);
    let mut source = stamped_metadata_instance("uuid-1", "jumbie.nyaa", "Nyaa");
    source.capabilities = vec![jumbie_shared::plugin::Capability::FeedProvider];
    let resolved = resolve_active_metadata_plugins(vec![source], &cfg);
    assert_eq!(resolved.len(), 1);
    assert_eq!(
        resolved[0].plugin_id, None,
        "fallback expected — source ignored"
    );
}

// has_matchable_metadata — match-button visibility logic

/// Minimal PluginInstanceInfo factory for testing has_matchable_metadata.
/// Sets instance_id to simulate a backend-stamped instance entry.
fn plugin(pid: &str) -> jumbie_shared::plugin::PluginInstanceInfo {
    jumbie_shared::plugin::PluginInstanceInfo {
        plugin_id: Some("jumbie.metadata_test".to_string()),
        instance_id: if pid.is_empty() {
            None
        } else {
            Some(pid.to_string())
        },
        display_name: String::new(),
        version: String::new(),
        author: String::new(),
        description: String::new(),
        capabilities: Vec::new(),
        supported_protocols: None,
        series_identifier_label: None,
        series_identifier_placeholder: None,
        rate_limit: None,
        supports_test: false,
    }
}

#[test]
fn test_has_matchable_metadata_not_matchable_when_source_is_provider_uuid() {
    let ids = [("plugin-uuid".to_string(), "meta-123".to_string())].into();
    assert!(!has_matchable_metadata(
        Some("plugin-uuid"),
        &ids,
        &[plugin("plugin-uuid")],
        true
    ));
}

#[test]
fn test_has_matchable_metadata_not_matchable_when_source_is_none() {
    let ids = [("plugin-uuid".to_string(), "meta-123".to_string())].into();
    assert!(!has_matchable_metadata(
        None,
        &ids,
        &[plugin("plugin-uuid")],
        true
    ));
}

#[test]
fn test_has_matchable_metadata_matchable_when_custom_with_cache() {
    let ids = [("plugin-uuid".to_string(), "meta-123".to_string())].into();
    assert!(has_matchable_metadata(
        Some("custom"),
        &ids,
        &[plugin("plugin-uuid")],
        true
    ));
}

#[test]
fn test_has_matchable_metadata_matchable_when_cleared_with_cache() {
    let ids = [("plugin-uuid".to_string(), "meta-123".to_string())].into();
    assert!(has_matchable_metadata(
        Some("cleared"),
        &ids,
        &[plugin("plugin-uuid")],
        true
    ));
}

#[test]
fn test_has_matchable_metadata_not_matchable_when_cache_empty() {
    let ids = std::collections::HashMap::new();
    assert!(!has_matchable_metadata(
        Some("custom"),
        &ids,
        &[plugin("plugin-uuid")],
        true
    ));
}

#[test]
fn test_has_matchable_metadata_not_matchable_when_wrong_provider() {
    let ids = [("other-plugin".to_string(), "meta-123".to_string())].into();
    assert!(!has_matchable_metadata(
        Some("custom"),
        &ids,
        &[plugin("active-plugin")],
        true
    ));
}

#[test]
fn test_has_matchable_metadata_not_matchable_when_no_active_plugins() {
    let ids = [("plugin-uuid".to_string(), "meta-123".to_string())].into();
    assert!(!has_matchable_metadata(Some("custom"), &ids, &[], true));
}

#[test]
fn test_has_matchable_metadata_not_matchable_when_plugin_id_is_none() {
    let ids = [("plugin-uuid".to_string(), "meta-123".to_string())].into();
    assert!(!has_matchable_metadata(
        Some("custom"),
        &ids,
        &[plugin("")],
        true
    ));
}

#[test]
fn test_has_matchable_metadata_not_matchable_when_plugin_id_empty() {
    let ids = [("".to_string(), "meta-123".to_string())].into();
    assert!(!has_matchable_metadata(
        Some("custom"),
        &ids,
        &[plugin("")],
        true
    ));
}

#[test]
fn test_has_matchable_metadata_not_matchable_when_cache_value_empty() {
    let ids = [("plugin-uuid".to_string(), "".to_string())].into();
    assert!(!has_matchable_metadata(
        Some("custom"),
        &ids,
        &[plugin("plugin-uuid")],
        true
    ));
}

#[test]
fn test_has_matchable_metadata_not_matchable_when_season_cache_missing() {
    let ids = [("plugin-uuid".to_string(), "meta-123".to_string())].into();
    assert!(!has_matchable_metadata(
        Some("custom"),
        &ids,
        &[plugin("plugin-uuid")],
        false
    ));
}

#[test]
fn test_has_matchable_metadata_matchable_when_active_provider_has_cache() {
    // Cache belongs to the current active (highest-priority) provider — matchable.
    let ids = [("first-plugin".to_string(), "meta-123".to_string())].into();
    let plugins = [plugin("first-plugin"), plugin("second-plugin")];
    assert!(has_matchable_metadata(Some("custom"), &ids, &plugins, true));
}

#[test]
fn test_has_matchable_metadata_not_matchable_when_only_non_active_provider_has_cache() {
    // Matchability is defined against the CURRENT ACTIVE provider (the first,
    // highest-priority one) — a cache entry belonging to a lower-priority
    // provider does NOT make the entity matchable.
    let ids = [("second-plugin".to_string(), "meta-456".to_string())].into();
    let plugins = [plugin("first-plugin"), plugin("second-plugin")];
    assert!(!has_matchable_metadata(
        Some("custom"),
        &ids,
        &plugins,
        true
    ));
}

// build_calendar_link_url

#[test]
fn test_build_calendar_link_url() {
    let url = build_calendar_link_url("https://example.com", "abc123");
    assert_eq!(url, "https://example.com/api/calendar/ical?token=abc123");
}

#[test]
fn test_build_calendar_link_url_with_localhost() {
    let url = build_calendar_link_url("http://localhost:3000", "tok_xyz");
    assert_eq!(url, "http://localhost:3000/api/calendar/ical?token=tok_xyz");
}

#[test]
fn test_build_calendar_link_url_special_chars() {
    let url = build_calendar_link_url("https://jumbie.app", "tok+abc/def=");
    assert_eq!(
        url,
        "https://jumbie.app/api/calendar/ical?token=tok+abc/def="
    );
}

// Date logic — calc_calendar_window, week_range, week_of_month

#[test]
fn test_calc_calendar_window_mid_year() {
    // May 15, 2026 → window from April 1 to June 30
    let (start, end) = calc_calendar_window(NaiveDate::from_ymd_opt(2026, 5, 15).unwrap());
    assert_eq!(start, NaiveDate::from_ymd_opt(2026, 4, 1).unwrap());
    assert_eq!(end, NaiveDate::from_ymd_opt(2026, 6, 30).unwrap());
}

#[test]
fn test_calc_calendar_window_january() {
    // January → wraps to previous December
    let (start, end) = calc_calendar_window(NaiveDate::from_ymd_opt(2026, 1, 15).unwrap());
    assert_eq!(start, NaiveDate::from_ymd_opt(2025, 12, 1).unwrap());
    assert_eq!(end, NaiveDate::from_ymd_opt(2026, 2, 28).unwrap()); // 2026 is not a leap year
}

#[test]
fn test_calc_calendar_window_december() {
    // December → wraps to next January
    let (start, end) = calc_calendar_window(NaiveDate::from_ymd_opt(2026, 12, 1).unwrap());
    assert_eq!(start, NaiveDate::from_ymd_opt(2026, 11, 1).unwrap());
    assert_eq!(end, NaiveDate::from_ymd_opt(2027, 1, 31).unwrap());
}

#[test]
fn test_calc_calendar_window_leap_year_february() {
    // February 2028 (leap year) → Jan 1 – Mar 31
    let (start, end) = calc_calendar_window(NaiveDate::from_ymd_opt(2028, 2, 29).unwrap());
    assert_eq!(start, NaiveDate::from_ymd_opt(2028, 1, 1).unwrap());
    assert_eq!(end, NaiveDate::from_ymd_opt(2028, 3, 31).unwrap());
}

// week_range

#[test]
fn test_week_range_mid_week() {
    // Wednesday May 27, 2026 → Sunday May 24 – Saturday May 30
    let (start, end) = week_range(NaiveDate::from_ymd_opt(2026, 5, 27).unwrap());
    assert_eq!(start, NaiveDate::from_ymd_opt(2026, 5, 24).unwrap());
    assert_eq!(end, NaiveDate::from_ymd_opt(2026, 5, 30).unwrap());
}

#[test]
fn test_week_range_sunday() {
    // Sunday is identity for start
    let (start, end) = week_range(NaiveDate::from_ymd_opt(2026, 5, 24).unwrap());
    assert_eq!(start, NaiveDate::from_ymd_opt(2026, 5, 24).unwrap());
    assert_eq!(end, NaiveDate::from_ymd_opt(2026, 5, 30).unwrap());
}

#[test]
fn test_week_range_saturday() {
    // Saturday → previous Sunday
    let (start, end) = week_range(NaiveDate::from_ymd_opt(2026, 5, 30).unwrap());
    assert_eq!(start, NaiveDate::from_ymd_opt(2026, 5, 24).unwrap());
    assert_eq!(end, NaiveDate::from_ymd_opt(2026, 5, 30).unwrap());
}

#[test]
fn test_week_range_cross_year() {
    // Jan 1, 2027 (Friday) → Dec 27, 2026 (Sun) – Jan 2, 2027 (Sat)
    let (start, end) = week_range(NaiveDate::from_ymd_opt(2027, 1, 1).unwrap());
    assert_eq!(start, NaiveDate::from_ymd_opt(2026, 12, 27).unwrap());
    assert_eq!(end, NaiveDate::from_ymd_opt(2027, 1, 2).unwrap());
}

// week_of_month

#[test]
fn test_week_of_month_may_first_week() {
    // May 2026 starts on Friday. The first Sunday of May's calendar is Apr 26 (Sun),
    // which is Week 1 of May (even though Apr 26 is in April).
    assert_eq!(
        week_of_month(NaiveDate::from_ymd_opt(2026, 4, 26).unwrap(), (2026, 5)),
        1
    );
}

#[test]
fn test_week_of_month_may_second_week() {
    // May 3 (Sunday) is Week 2 of May
    assert_eq!(
        week_of_month(NaiveDate::from_ymd_opt(2026, 5, 3).unwrap(), (2026, 5)),
        2
    );
}

#[test]
fn test_week_of_month_may_last_week() {
    // May 31 (Sunday) is Week 6 of May
    assert_eq!(
        week_of_month(NaiveDate::from_ymd_opt(2026, 5, 31).unwrap(), (2026, 5)),
        6
    );
}

#[test]
fn test_week_of_month_month_starts_on_sunday() {
    // August 2027 starts on a Sunday. The first calendar Sunday = Aug 1 = Week 1.
    assert_eq!(
        week_of_month(NaiveDate::from_ymd_opt(2027, 8, 1).unwrap(), (2027, 8)),
        1
    );
    // Aug 8 (Sunday) → Week 2
    assert_eq!(
        week_of_month(NaiveDate::from_ymd_opt(2027, 8, 8).unwrap(), (2027, 8)),
        2
    );
}

#[test]
fn test_week_of_month_previous_month_sunday_is_week_1() {
    // May 2027 starts on Saturday. The first Sunday of May's calendar is Apr 25,
    // which is in April but is still Week 1 of May.
    assert_eq!(
        week_of_month(NaiveDate::from_ymd_opt(2027, 4, 25).unwrap(), (2027, 5)),
        1
    );
    // May 2 (Sun) = Week 2 of May
    assert_eq!(
        week_of_month(NaiveDate::from_ymd_opt(2027, 5, 2).unwrap(), (2027, 5)),
        2
    );
    // May 30 (Sun) = Week 6 of May
    assert_eq!(
        week_of_month(NaiveDate::from_ymd_opt(2027, 5, 30).unwrap(), (2027, 5)),
        6
    );
}

#[test]
fn test_refresh_calendar_episode_from_view_model() {
    use jumbie_shared::config::ui::ReleaseDateDisplayConfig;
    use jumbie_shared::types::{CalendarEpisode, EpisodeViewModel, ReleaseDates};

    let config = ReleaseDateDisplayConfig::default();
    let tf = TimeFormat::Hour12;

    // A single-file downloaded episode: path set, parts empty (most common case).
    let vm = EpisodeViewModel {
        unique_id: "ep-1".to_string(),
        season: "S01".to_string(),
        episode: 3,
        header: "Episode 3".to_string(),
        title: Some("Fresh Title".to_string()),
        status: "downloaded".to_string(),
        quality_profile_id: None,
        size: 0,
        submitter: None,
        path: Some("/media/show/S01E03.mkv".to_string()),
        original_path: None,
        media_info: None,
        fingerprint: None,
        created_at: None,
        file_acquired_at: None,
        monitored: false,
        dates: ReleaseDates {
            meta_date: Some("2026-07-10T20:00:00+00:00".to_string()),
            upload_date: None,
            est_date: None,
        },
        metadata_ids: std::collections::HashMap::new(),
        description: None,
        runtime: None,
        image_url: None,
        metadata_source: None,
        release_title: None,
        parts: Vec::new(),
        auxiliary_files: Vec::new(),
        show_only_downloaded: false,
        // Single-file episode: the backend reports it as assigned and on disk.
        assigned: true,
        disk_present: true,
    };

    let mut ep = CalendarEpisode {
        series_title: "Old Title".to_string(),
        series_id: "series-1".to_string(),
        episode_id: "ep-1".to_string(),
        season: "1".to_string(),
        episode: 1,
        episode_title: Some("Old Episode".to_string()),
        dates: ReleaseDates {
            meta_date: Some("2026-06-01T20:00:00+00:00".to_string()),
            upload_date: None,
            est_date: None,
        },
        eff_date: "2026-06-01T20:00:00+00:00".to_string(),
        status: "missing".to_string(),
        assigned: false,
        disk_present: false,
    };

    refresh_calendar_episode_from_view_model(&mut ep, &vm, "Series A", &config, &tf);

    assert_eq!(ep.series_title, "Series A");
    assert_eq!(ep.season, "S01");
    assert_eq!(ep.episode, 3);
    assert_eq!(ep.episode_title.as_deref(), Some("Fresh Title"));
    assert_eq!(ep.status, "downloaded");
    assert!(
        ep.assigned,
        "single-file episode (parts empty) should still count as downloaded"
    );
    assert_eq!(
        ep.dates.meta_date.as_deref(),
        Some("2026-07-10T20:00:00+00:00")
    );
    // eff_date is recomputed from the fresh dates via pick_release_date.
    assert_eq!(ep.eff_date, "2026-07-10T20:00:00+00:00");

    // No path and no parts → not downloaded; status follows the view model.
    let mut ep2 = ep.clone();
    let mut vm2 = vm.clone();
    vm2.path = None;
    vm2.parts = Vec::new();
    vm2.status = "missing".to_string();
    vm2.assigned = false;
    vm2.disk_present = false;
    refresh_calendar_episode_from_view_model(&mut ep2, &vm2, "Series A", &config, &tf);
    assert!(!ep2.assigned, "no path and no parts → assigned=false");
    assert_eq!(ep2.status, "missing");
}

// payload_from_series_details (SSoT modal derivation)

#[test]
fn test_payload_from_series_details_derives_episode_and_bindings() {
    use crate::utils::episode_state::payload_from_series_details;
    use jumbie_shared::mapping::{MappingRule, SeasonOverride, SeriesSettings};
    use jumbie_shared::types::{EpisodeViewModel, ReleaseDates, SeriesDetails, SeriesInfo};

    let mut settings = SeriesSettings {
        aliases: vec![
            "Alias One".to_string(),
            String::new(),
            "Alias Two".to_string(),
        ],
        absolute_numbering: Some(true),
        search_format: None,
        search_format_absolute: None,
        metadata_ids: {
            let mut m = std::collections::HashMap::new();
            m.insert("plugin-1".to_string(), "meta-1".to_string());
            m
        },
        ..SeriesSettings::default()
    };
    // Normal-mode override — must NOT leak into an absolute-mode payload.
    settings.season.insert(
        "1".to_string(),
        SeasonOverride {
            season: "1".to_string(),
            episode_start: None,
            episode_end: None,
            cell_count: None,
            episode_offset: Some(3),
            alias_season_number: None,
            search_format: None,
            aliases: vec![],
            reg_patterns: vec![],
        },
    );
    // Absolute-mode override — must be picked when absolute_numbering is true.
    settings.season_absolute.insert(
        "1".to_string(),
        SeasonOverride {
            season: "1".to_string(),
            episode_start: None,
            episode_end: None,
            cell_count: None,
            episode_offset: Some(9),
            alias_season_number: None,
            search_format: Some("E${episode:02}".to_string()),
            aliases: vec![],
            reg_patterns: vec![],
        },
    );

    let details = SeriesDetails {
        info: SeriesInfo {
            id: "series-1".to_string(),
            title: "My Series".to_string(),
            seasons: vec![],
            season_count: 0,
            release_profile: String::new(),
            quality_profile: String::new(),
            episodes_counts: (0, 0),
            monitored_missing_count: 0,
            queued_count: 0,
            size: 0,
            path: String::new(),
            scan_queue_count: 0,
            absolute_numbering: false,
            aliases: vec![],
            has_not_found_files: false,
        },
        config: MappingRule {
            target_title: "My Series".to_string(),
            series_id: "series-1".to_string(),
            settings,
            ..Default::default()
        },
        episodes: vec![EpisodeViewModel {
            unique_id: "series-1_S01E01".to_string(),
            season: "S01".to_string(),
            episode: 1,
            header: "Episode 1".to_string(),
            title: Some("Pilot".to_string()),
            status: "in_queue".to_string(),
            quality_profile_id: None,
            size: 0,
            submitter: None,
            path: None,
            original_path: None,
            media_info: None,
            fingerprint: None,
            created_at: None,
            file_acquired_at: None,
            monitored: true,
            dates: ReleaseDates {
                meta_date: Some("2026-07-10T20:00:00+00:00".to_string()),
                upload_date: None,
                est_date: None,
            },
            metadata_ids: std::collections::HashMap::new(),
            description: None,
            runtime: None,
            image_url: None,
            metadata_source: None,
            release_title: None,
            parts: Vec::new(),
            auxiliary_files: Vec::new(),
            show_only_downloaded: false,
            assigned: false,
            disk_present: false,
        }],
        metadata_seasons: vec![],
        suppressed_seasons: vec![],
    };

    let payload =
        payload_from_series_details(&details, "series-1_S01E01", false, "GLOBAL", "GLOBALABS")
            .expect("episode should be found");

    assert_eq!(payload.series_id, "series-1");
    assert_eq!(payload.series_title, "My Series");
    assert_eq!(payload.episode.status, "in_queue");
    assert_eq!(
        payload.series_search_format, "GLOBALABS",
        "absolute mode with no series override falls back to the global absolute template"
    );
    assert_eq!(
        payload
            .season_overrides
            .iter()
            .map(|o| o.episode_offset)
            .collect::<Vec<_>>(),
        vec![Some(9)],
        "absolute-mode overrides must be picked, not normal-mode"
    );
    assert_eq!(
        payload
            .series_metadata_ids
            .get("plugin-1")
            .map(String::as_str),
        Some("meta-1")
    );
    assert_eq!(payload.all_episodes.as_ref().map(|e| e.len()), Some(1));

    // Unknown episode → None (no payload to display).
    assert!(
        payload_from_series_details(&details, "series-1_S01E99", false, "GLOBAL", "GLOBALABS")
            .is_none()
    );
}

// Shared helpers for episode/search tests

/// Minimal `EpisodeViewModel` for query-resolution tests.
fn ep_vm(season: &str, episode: i32) -> EpisodeViewModel {
    EpisodeViewModel {
        unique_id: format!("{}_{}", season, episode),
        season: season.to_string(),
        episode,
        header: String::new(),
        title: None,
        status: String::new(),
        quality_profile_id: None,
        size: 0,
        submitter: None,
        release_title: None,
        path: None,
        original_path: None,
        media_info: None,
        fingerprint: None,
        created_at: None,
        file_acquired_at: None,
        monitored: true,
        dates: ReleaseDates {
            meta_date: None,
            upload_date: None,
            est_date: None,
        },
        metadata_ids: std::collections::HashMap::new(),
        description: None,
        runtime: None,
        image_url: None,
        metadata_source: None,
        parts: Vec::new(),
        auxiliary_files: Vec::new(),
        show_only_downloaded: false,
        assigned: false,
        disk_present: false,
    }
}

// episode_state::group_episodes_by_season — the SSoT grouping shared by EpisodesTab
// (expand/collapse-all) and SeasonAccordionList.

#[test]
fn test_group_episodes_by_season_orders_newest_first_preserving_intra_season_order() {
    let groups = crate::utils::episode_state::group_episodes_by_season(
        vec![ep_vm("S01", 1), ep_vm("S02", 1), ep_vm("S01", 2)],
        false,
    );

    let seasons: Vec<&str> = groups.iter().map(|(s, _)| s.as_str()).collect();
    assert_eq!(seasons, vec!["S02", "S01"], "newest season first");

    let s01: Vec<i32> = groups
        .iter()
        .find(|(s, _)| s.as_str() == "S01")
        .expect("S01 present")
        .1
        .iter()
        .map(|e| e.episode)
        .collect();
    assert_eq!(s01, vec![1, 2], "original order preserved within a season");
}

#[test]
fn test_group_episodes_by_season_absolute_collapses_into_one_bucket() {
    let groups = crate::utils::episode_state::group_episodes_by_season(
        vec![ep_vm("S01", 1), ep_vm("S02", 4)],
        true,
    );

    assert_eq!(groups.len(), 1);
    assert_eq!(
        groups[0].0,
        crate::utils::episode_state::ABSOLUTE_SEASON_LABEL
    );
    assert_eq!(groups[0].1.len(), 2);
}

#[test]
fn test_group_episodes_by_season_empty() {
    assert!(crate::utils::episode_state::group_episodes_by_season(Vec::new(), false).is_empty());
}

/// A season override with everything unset except the fields under test.
fn season_override(season: &str) -> jumbie_shared::mapping::SeasonOverride {
    jumbie_shared::mapping::SeasonOverride {
        season: season.to_string(),
        episode_start: None,
        episode_end: None,
        cell_count: None,
        episode_offset: None,
        alias_season_number: None,
        search_format: None,
        aliases: Vec::new(),
        reg_patterns: Vec::new(),
    }
}

// resolve_manual_search_query (`{title} {rendered key}`)

#[test]
fn test_manual_search_query_normal_and_absolute() {
    use crate::utils::episode_search::resolve_manual_search_query;

    let ep = ep_vm("S01", 1195);
    // Normal mode → `{title} S{season:02}E{episode:02}`.
    assert_eq!(
        resolve_manual_search_query(
            &ep,
            "Mock Series",
            &[],
            "S${season:02}E${episode:02}",
            false
        ),
        "Mock Series S01E1195"
    );

    // Absolute mode with an episode-only template → no season marker.
    assert_eq!(
        resolve_manual_search_query(&ep, "Mock Series", &[], "E${episode:02}", true),
        "Mock Series E1195"
    );
}

#[test]
fn test_manual_search_query_applies_alias_offset_and_season_template() {
    use crate::utils::episode_search::resolve_manual_search_query;

    let ep = ep_vm("S01", 5);
    let series_fmt = "S${season:02}E${episode:02}";

    // Season-number alias + episode offset: S01E05 → S21E105.
    let alias_offset = vec![jumbie_shared::mapping::SeasonOverride {
        episode_offset: Some(100),
        alias_season_number: Some(21),
        ..season_override("1")
    }];
    assert_eq!(
        resolve_manual_search_query(&ep, "Show", &alias_offset, series_fmt, false),
        "Show S21E105"
    );

    // A season-level template replaces the series template.
    let season_fmt = vec![jumbie_shared::mapping::SeasonOverride {
        search_format: Some("${episode:02}".to_string()),
        ..season_override("1")
    }];
    assert_eq!(
        resolve_manual_search_query(&ep, "Show", &season_fmt, series_fmt, false),
        "Show 05"
    );

    // A deliberately blank season template searches by title alone.
    let blank = vec![jumbie_shared::mapping::SeasonOverride {
        search_format: Some(String::new()),
        ..season_override("1")
    }];
    assert_eq!(
        resolve_manual_search_query(&ep, "Show", &blank, series_fmt, false),
        "Show"
    );

    // A blank series template also searches by title alone.
    assert_eq!(
        resolve_manual_search_query(&ep, "Show", &[], "", false),
        "Show"
    );
}

// season_mismatch — "Match Seasons" affordance comparison

fn season_info(season_number: i32, episode_count: i32) -> jumbie_shared::types::MetadataSeasonInfo {
    jumbie_shared::types::MetadataSeasonInfo {
        season_number,
        title: None,
        episode_count,
        premiere_date: None,
        end_date: None,
        image_url: None,
        summary: None,
        provider_instance_id: "tvdb".to_string(),
        is_fallback_mode: false,
    }
}

fn season_labels(labels: &[&str]) -> std::collections::HashSet<String> {
    labels.iter().map(|s| s.to_string()).collect()
}

#[test]
fn test_season_mismatch_none_when_no_provider_metadata() {
    let result = season_mismatch(&[], &season_labels(&["1"]), &["1".to_string()]);
    assert!(result.is_none(), "no provider metadata → nothing to match");
}

#[test]
fn test_season_mismatch_none_when_aligned() {
    let metadata = [season_info(1, 10), season_info(2, 8)];
    let result = season_mismatch(
        &metadata,
        &season_labels(&["1", "2"]),
        &["1".to_string(), "2".to_string()],
    );
    assert!(
        result.is_none(),
        "aligned seasons and overrides → no mismatch"
    );
}

#[test]
fn test_season_mismatch_reports_missing_and_extra() {
    let metadata = [season_info(1, 10), season_info(2, 8)];
    let result = season_mismatch(
        &metadata,
        &season_labels(&["1"]),
        &["1".to_string(), "3".to_string()],
    )
    .expect("mismatch expected");

    assert_eq!(result.missing, vec![("2".to_string(), 8)]);
    assert_eq!(result.extra_overrides, vec!["3".to_string()]);
}

#[test]
fn test_season_mismatch_normalizes_numeric_season_labels() {
    // Loaded seasons are normalized to bare numbers before comparison, so a
    // padded label like "02" still matches provider season 2.
    let metadata = [season_info(2, 8)];
    let result = season_mismatch(&metadata, &season_labels(&["2"]), &[]);
    assert!(result.is_none());
}

#[test]
fn test_season_mismatch_only_extra_override() {
    let metadata = [season_info(1, 10)];
    let result = season_mismatch(
        &metadata,
        &season_labels(&["1"]),
        &["1".to_string(), "5".to_string()],
    )
    .expect("extra override expected");
    assert!(result.missing.is_empty());
    assert_eq!(result.extra_overrides, vec!["5".to_string()]);
}

#[test]
fn test_season_mismatch_keeps_deleted_season_as_missing() {
    // INTENDED BEHAVIOR: a deliberately deleted (suppressed) season has no loaded
    // episodes but still exists in provider metadata, so it must surface as
    // `missing`. "Match Seasons" is the intended way to restore it — do NOT filter
    // suppressed seasons out of this comparison.
    let metadata = [season_info(1, 10), season_info(2, 8)];
    let result = season_mismatch(
        &metadata,
        &season_labels(&["1"]), // season 2 was deleted
        &["1".to_string()],
    )
    .expect("a deleted season must surface so it can be restored");
    assert!(result.missing.contains(&("2".to_string(), 8)));
}

// Single-active metadata enforcement — `utils::plugins`

/// Build a `PluginsConfig` whose `metadata` section holds the given
/// `(plugin_key, instance_id, enabled, name)` entries.
fn metadata_enforcement_cfg(
    instances: &[(&str, &str, bool, &str)],
) -> jumbie_shared::config::PluginsConfig {
    let mut cfg = jumbie_shared::config::PluginsConfig::default();
    for (plugin_key, instance_id, enabled, name) in instances {
        cfg.metadata
            .entry(plugin_key.to_string())
            .or_default()
            .insert(
                instance_id.to_string(),
                serde_json::json!({ "enabled": enabled, "name": name }),
            );
    }
    cfg
}

/// Sorted `plugin_key:instance_id` pairs that are enabled.
fn enabled_metadata_pairs(cfg: &jumbie_shared::config::PluginsConfig) -> Vec<String> {
    let mut pairs: Vec<String> = cfg
        .metadata
        .iter()
        .flat_map(|(key, instances)| {
            instances
                .iter()
                .filter(|(_, value)| jumbie_shared::config::instance_is_enabled(value))
                .map(move |(instance_id, _)| format!("{}:{}", key, instance_id))
        })
        .collect();
    pairs.sort();
    pairs
}

#[test]
fn test_enforce_single_metadata_keeps_preferred_and_reports_disabled() {
    let mut cfg = metadata_enforcement_cfg(&[
        ("metadata.tvmaze", "a", true, "TVMaze"),
        ("metadata.tvdb", "b", true, "TVDB"),
    ]);
    let disabled =
        crate::utils::plugins::enforce_single_metadata(&mut cfg, Some(("metadata.tvdb", "b")));
    assert_eq!(disabled, vec!["TVMaze".to_string()]);
    assert_eq!(
        enabled_metadata_pairs(&cfg),
        vec!["metadata.tvdb:b".to_string()]
    );
}

#[test]
fn test_enforce_single_metadata_is_noop_for_single_provider() {
    let mut cfg = metadata_enforcement_cfg(&[
        ("metadata.tvmaze", "a", true, "TVMaze"),
        ("metadata.tvdb", "b", false, "TVDB"),
    ]);
    assert!(
        crate::utils::plugins::enforce_single_metadata(&mut cfg, None).is_empty(),
        "a single enabled provider needs no change"
    );
    assert_eq!(
        enabled_metadata_pairs(&cfg),
        vec!["metadata.tvmaze:a".to_string()]
    );
}

#[test]
fn test_enforce_single_metadata_falls_back_deterministically() {
    // Preferred instance is not enabled → the deterministic (sorted) first wins.
    let mut cfg = metadata_enforcement_cfg(&[
        ("metadata.tvmaze", "a", true, "TVMaze"),
        ("metadata.tvdb", "b", true, "TVDB"),
    ]);
    let disabled = crate::utils::plugins::enforce_single_metadata(
        &mut cfg,
        Some(("metadata.tvdb", "missing")),
    );
    // `metadata.tvdb` sorts before `metadata.tvmaze`, so b wins.
    assert_eq!(disabled, vec!["TVMaze".to_string()]);
    assert_eq!(
        enabled_metadata_pairs(&cfg),
        vec!["metadata.tvdb:b".to_string()]
    );
}

#[test]
fn test_enforce_single_metadata_name_falls_back_to_type_key() {
    let mut cfg = jumbie_shared::config::PluginsConfig::default();
    cfg.metadata.insert(
        "metadata.tvmaze".into(),
        [("a".to_string(), serde_json::json!({ "enabled": true }))]
            .into_iter()
            .collect(),
    );
    cfg.metadata.insert(
        "metadata.tvdb".into(),
        [(
            "b".to_string(),
            serde_json::json!({ "enabled": true, "name": "TVDB" }),
        )]
        .into_iter()
        .collect(),
    );
    let disabled =
        crate::utils::plugins::enforce_single_metadata(&mut cfg, Some(("metadata.tvdb", "b")));
    // The disabled instance has no `name` → the plugin type key is used.
    assert_eq!(disabled, vec!["tvmaze".to_string()]);
}

#[test]
fn test_newly_disabled_metadata_providers_reports_switch_off() {
    let before = metadata_enforcement_cfg(&[
        ("metadata.tvmaze", "a", true, "TVMaze"),
        ("metadata.tvdb", "b", true, "TVDB"),
    ]);
    let after = metadata_enforcement_cfg(&[
        ("metadata.tvmaze", "a", false, "TVMaze"),
        ("metadata.tvdb", "b", true, "TVDB"),
    ]);
    assert_eq!(
        crate::utils::plugins::newly_disabled_metadata_providers(&before, &after),
        vec!["TVMaze".to_string()]
    );
}

#[test]
fn test_newly_disabled_metadata_providers_empty_when_unchanged() {
    let before = metadata_enforcement_cfg(&[("metadata.tvmaze", "a", true, "TVMaze")]);
    let after = metadata_enforcement_cfg(&[("metadata.tvmaze", "a", true, "TVMaze")]);
    assert!(crate::utils::plugins::newly_disabled_metadata_providers(&before, &after).is_empty());
}

// WASM-only tests — cache, pending lifecycle, series profile updates, autosave.
// These require the browser WASM runtime (js_sys, web_sys, etc.).

#[cfg(target_arch = "wasm32")]
mod wasm_tests {
    use super::*;
    use jumbie_shared::config::Config;
    use jumbie_shared::types::SeriesInfo;
    use std::sync::{Arc, Mutex};
    use wasm_bindgen_test::*;

    // Deliberately NOT `wasm_bindgen_test_configure!(run_in_browser)`: these tests
    // don't need a DOM, and configuring the crate for a browser made
    // `wasm-pack test --node` (the CI step) skip the entire suite. Keep them
    // runnable under Node.

    // Helper: clear cache entries by key prefix

    fn clear_cache(prefix: &str) {
        API_CACHE.with(|cache| {
            cache.borrow_mut().retain(|k, _| !k.starts_with(prefix));
        });
        API_CACHE_TIMESTAMPS.with(|ts| {
            ts.borrow_mut().retain(|k, _| !k.starts_with(prefix));
        });
    }

    fn clear_pending() {
        API_PENDING.with(|p| {
            p.borrow_mut().clear();
        });
    }

    // Cache

    #[wasm_bindgen_test]
    fn test_cache_write_and_read() {
        clear_cache("");
        let key = "test_key".to_string();
        let value = serde_json::json!({"name": "test", "value": 42});
        write_cache(&key, &value);
        let cached = read_cache::<serde_json::Value>(&key);
        assert_eq!(cached, Some(value));
    }

    #[wasm_bindgen_test]
    fn test_cache_read_missing_key() {
        clear_cache("");
        let result: Option<serde_json::Value> = read_cache("nonexistent");
        assert_eq!(result, None);
    }

    #[wasm_bindgen_test]
    fn test_cache_overwrite() {
        clear_cache("");
        let key = "overwrite_test".to_string();
        write_cache(&key, &serde_json::json!("first"));
        write_cache(&key, &serde_json::json!("second"));
        let cached = read_cache::<String>(&key);
        assert_eq!(cached, Some("second".to_string()));
    }

    #[wasm_bindgen_test]
    fn test_cache_json_types() {
        clear_cache("");
        #[derive(serde::Serialize, serde::Deserialize, PartialEq, Debug)]
        struct Point {
            x: i32,
            y: i32,
        }
        let point = Point { x: 10, y: 20 };
        write_cache("point", &point);
        let restored: Option<Point> = read_cache("point");
        assert_eq!(restored, Some(point));
    }

    // Cache Freshness

    #[wasm_bindgen_test]
    fn test_is_cache_fresh_missing() {
        clear_cache("");
        assert!(!is_cache_fresh("missing_key"));
    }

    #[wasm_bindgen_test]
    fn test_is_cache_fresh_just_written() {
        clear_cache("");
        let key = "fresh_test";
        write_cache(key, &"data");
        assert!(is_cache_fresh(key));
    }

    #[wasm_bindgen_test]
    fn test_is_cache_fresh_stale() {
        clear_cache("");
        let key = "stale_test";
        write_cache(key, &"data");
        // Override the timestamp to simulate staleness
        API_CACHE_TIMESTAMPS.with(|ts| {
            ts.borrow_mut().insert(key.to_string(), 0.0);
        });
        assert!(!is_cache_fresh(key));
    }

    // Cache Eviction

    #[wasm_bindgen_test]
    fn test_cache_eviction() {
        clear_cache("");

        // Fill cache up to max
        for i in 0..MAX_CACHE_ENTRIES {
            write_cache(&format!("evict_key_{}", i), &i);
        }

        // All should still be present
        for i in 0..MAX_CACHE_ENTRIES {
            let val: Option<i32> = read_cache(&format!("evict_key_{}", i));
            assert_eq!(
                val,
                Some(i as i32),
                "Key {} should exist before eviction",
                i
            );
        }

        // Adding one more triggers eviction of the oldest (key 0)
        write_cache("evict_trigger", &999);

        // The oldest entry (key 0) should be gone
        let val: Option<i32> = read_cache("evict_key_0");
        assert_eq!(val, None, "Oldest key should be evicted");

        // But the rest should remain
        let val: Option<i32> = read_cache("evict_key_1");
        assert_eq!(val, Some(1), "Newer keys should persist after eviction");
        let val: Option<i32> = read_cache("evict_trigger");
        assert_eq!(val, Some(999), "Newest key should exist");
    }

    // Cache Invalidation

    #[wasm_bindgen_test]
    fn test_invalidate_cache_prefix() {
        clear_cache("");

        write_cache("episodes_1", &"data1");
        write_cache("episodes_2", &"data2");
        write_cache("season_1", &"data3");

        invalidate_cache_prefix("episodes");

        assert_eq!(read_cache::<String>("episodes_1"), None);
        assert_eq!(read_cache::<String>("episodes_2"), None);
        assert_eq!(read_cache::<String>("season_1"), Some("data3".to_string()));
    }

    #[wasm_bindgen_test]
    fn test_invalidate_cache_prefix_no_match() {
        clear_cache("");

        write_cache("abc", &"data");

        invalidate_cache_prefix("nonexistent");
        assert_eq!(read_cache::<String>("abc"), Some("data".to_string()));
    }

    // Pending lifecycle

    #[wasm_bindgen_test]
    fn test_pending_lifecycle() {
        clear_pending();
        let key = "pending_lifecycle".to_string();
        assert!(!is_pending(&key));
        mark_pending(&key);
        assert!(is_pending(&key));
        resolve_pending(&key);
        assert!(!is_pending(&key));
    }

    #[wasm_bindgen_test]
    fn test_pending_completion_callbacks() {
        clear_pending();
        let key = "pending_cb".to_string();
        let called = Arc::new(Mutex::new(false));
        let called_clone = called.clone();
        // Key must be marked pending before callbacks can be registered
        mark_pending(&key);
        on_pending_complete(
            &key,
            Box::new(move || {
                *called_clone.lock().unwrap() = true;
            }),
        );
        assert!(!*called.lock().unwrap());
        resolve_pending(&key);
        assert!(*called.lock().unwrap());
    }

    #[wasm_bindgen_test]
    fn test_on_pending_complete_noop_for_non_pending() {
        clear_pending();
        let key = "non_pending".to_string();
        let called = Arc::new(Mutex::new(false));
        let called_clone = called.clone();
        on_pending_complete(
            &key,
            Box::new(move || {
                *called_clone.lock().unwrap() = true;
            }),
        );
        assert!(!*called.lock().unwrap());
        // Resolving without it being pending should not trigger the callback
        resolve_pending(&key);
        assert!(!*called.lock().unwrap());
    }

    // update_cached_series_profiles

    #[wasm_bindgen_test]
    fn test_update_cached_series_profiles() {
        clear_cache("");
        // write_cache uses fetch_series as its cache key, not individual series
        let series = vec![SeriesInfo {
            id: "series-123".to_string(),
            title: "My Series".to_string(),
            seasons: vec![],
            season_count: 0,
            release_profile: "HD-1080p".to_string(),
            quality_profile: "qp-hd".to_string(),
            episodes_counts: (0, 0),
            monitored_missing_count: 0,
            queued_count: 0,
            size: 0,
            has_not_found_files: false,
            path: String::new(),
            scan_queue_count: 0,
            absolute_numbering: false,
            aliases: Vec::new(),
        }];
        write_cache("fetch_series", &series);

        update_cached_series_profiles("series-123", "HD 1080p", "HD-1080p");

        let updated: Option<Vec<SeriesInfo>> = read_cache("fetch_series");
        let entry = updated.unwrap().into_iter().next().unwrap();
        assert_eq!(entry.quality_profile, "HD 1080p");
    }

    #[wasm_bindgen_test]
    fn test_update_cached_series_profiles_nonexistent_id() {
        clear_cache("");
        let series = vec![SeriesInfo {
            id: "other-id".to_string(),
            title: "Other".to_string(),
            seasons: vec![],
            season_count: 0,
            release_profile: String::new(),
            quality_profile: String::new(),
            episodes_counts: (0, 0),
            monitored_missing_count: 0,
            queued_count: 0,
            size: 0,
            has_not_found_files: false,
            path: String::new(),
            scan_queue_count: 0,
            absolute_numbering: false,
            aliases: Vec::new(),
        }];
        write_cache("fetch_series", &series);
        // Should not panic — series ID won't match
        update_cached_series_profiles("nonexistent-id", "HD", "");
    }

    // patch_calendar_cache

    /// Build a minimal `EpisodeViewModel` for calendar-cache patch tests.
    fn test_episode_vm(
        unique_id: &str,
        status: &str,
        meta_date: Option<&str>,
        path: Option<&str>,
    ) -> jumbie_shared::types::EpisodeViewModel {
        jumbie_shared::types::EpisodeViewModel {
            unique_id: unique_id.to_string(),
            season: "S01".to_string(),
            episode: 1,
            header: "Episode 1".to_string(),
            title: None,
            status: status.to_string(),
            quality_profile_id: None,
            size: 0,
            submitter: None,
            path: path.map(|s| s.to_string()),
            original_path: None,
            media_info: None,
            fingerprint: None,
            created_at: None,
            file_acquired_at: None,
            monitored: false,
            dates: jumbie_shared::types::ReleaseDates {
                meta_date: meta_date.map(|s| s.to_string()),
                upload_date: None,
                est_date: None,
            },
            metadata_ids: std::collections::HashMap::new(),
            description: None,
            runtime: None,
            image_url: None,
            metadata_source: None,
            release_title: None,
            parts: Vec::new(),
            auxiliary_files: Vec::new(),
            show_only_downloaded: false,
            // `assigned`/`disk_present` are the backend-computed SSoT that
            // `refresh_calendar_episode_from_view_model` merely mirrors, so a
            // fixture carrying a path reports the file as assigned and on disk.
            assigned: path.is_some(),
            disk_present: path.is_some(),
        }
    }

    fn test_release_config() -> jumbie_shared::config::ui::ReleaseDateDisplayConfig {
        jumbie_shared::config::ui::ReleaseDateDisplayConfig::default()
    }

    #[wasm_bindgen_test]
    fn test_patch_calendar_cache_updates_episode_fields() {
        clear_cache("");

        use jumbie_shared::types::{CalendarEpisode, CalendarResponse};

        let episodes = vec![
            CalendarEpisode {
                series_title: "Series A".to_string(),
                series_id: "series-a".to_string(),
                episode_id: "ep-1".to_string(),
                season: "1".to_string(),
                episode: 1,
                episode_title: Some("Pilot".to_string()),
                dates: jumbie_shared::types::ReleaseDates {
                    meta_date: Some("2026-06-01T20:00:00+00:00".to_string()),
                    upload_date: None,
                    est_date: None,
                },
                eff_date: "2026-06-01T20:00:00+00:00".to_string(),
                status: "missing".to_string(),
                assigned: false,
                disk_present: false,
            },
            CalendarEpisode {
                series_title: "Series A".to_string(),
                series_id: "series-a".to_string(),
                episode_id: "ep-2".to_string(),
                season: "1".to_string(),
                episode: 2,
                episode_title: Some("Second".to_string()),
                dates: jumbie_shared::types::ReleaseDates {
                    meta_date: Some("2026-06-08T20:00:00+00:00".to_string()),
                    upload_date: None,
                    est_date: None,
                },
                eff_date: "2026-06-08T20:00:00+00:00".to_string(),
                status: "missing".to_string(),
                assigned: false,
                disk_present: false,
            },
        ];
        let response = CalendarResponse { episodes };
        write_cache("fetch_calendar_2026-06-01_2026-07-01", &response);

        // Patch ep-1 with a fresh view model: downloaded, new date + status.
        let vm = test_episode_vm(
            "ep-1",
            "downloaded",
            Some("2026-06-02T21:00:00+00:00"),
            Some("/media/ep1.mkv"),
        );
        patch_calendar_cache(&vm, "Series A", &test_release_config(), &TimeFormat::Hour12);

        let cached: Option<CalendarResponse> = read_cache("fetch_calendar_2026-06-01_2026-07-01");
        let response = cached.expect("Cache entry should exist");

        let ep1 = response.episodes.iter().find(|e| e.episode_id == "ep-1");
        assert!(ep1.is_some(), "ep-1 should exist");
        let ep1 = ep1.unwrap();
        assert!(ep1.assigned, "ep-1 should be assigned=true");
        assert_eq!(ep1.status, "downloaded", "status should be patched");
        assert_eq!(
            ep1.dates.meta_date.as_deref(),
            Some("2026-06-02T21:00:00+00:00"),
            "meta_date should be patched"
        );
        assert_eq!(
            ep1.eff_date, "2026-06-02T21:00:00+00:00",
            "eff_date should be recomputed from the patched dates"
        );

        let ep2 = response.episodes.iter().find(|e| e.episode_id == "ep-2");
        assert!(ep2.is_some(), "ep-2 should exist");
        assert!(!ep2.unwrap().assigned, "ep-2 should remain assigned=false");
    }

    #[wasm_bindgen_test]
    fn test_patch_calendar_cache_unknown_episode() {
        clear_cache("");

        use jumbie_shared::types::{CalendarEpisode, CalendarResponse};

        let episodes = vec![CalendarEpisode {
            series_title: "Series B".to_string(),
            series_id: "series-b".to_string(),
            episode_id: "ep-3".to_string(),
            season: "2".to_string(),
            episode: 1,
            episode_title: None,
            dates: jumbie_shared::types::ReleaseDates {
                meta_date: Some("2026-06-15T20:00:00+00:00".to_string()),
                upload_date: None,
                est_date: None,
            },
            eff_date: "2026-06-15T20:00:00+00:00".to_string(),
            status: "missing".to_string(),
            assigned: false,
            disk_present: false,
        }];
        let response = CalendarResponse { episodes };
        write_cache("fetch_calendar_2026-06-01_2026-07-01", &response);

        // Patch an episode that doesn't exist in the cache — should be a no-op
        let vm = test_episode_vm("nonexistent", "missing", None, None);
        patch_calendar_cache(&vm, "Series B", &test_release_config(), &TimeFormat::Hour12);

        let cached: Option<CalendarResponse> = read_cache("fetch_calendar_2026-06-01_2026-07-01");
        let response = cached.expect("Cache entry should exist");
        assert_eq!(response.episodes.len(), 1);
        assert!(!response.episodes[0].assigned, "Should remain unchanged");
    }

    #[wasm_bindgen_test]
    fn test_patch_calendar_cache_only_calendar_keys() {
        clear_cache("");

        use jumbie_shared::types::CalendarResponse;

        // Write a non-calendar cache entry that should NOT be touched
        write_cache("series_list", &vec!["data"]);

        // Calendar entry
        let cal_response = CalendarResponse {
            episodes: vec![jumbie_shared::types::CalendarEpisode {
                series_title: "S".to_string(),
                series_id: "s-1".to_string(),
                episode_id: "ep-4".to_string(),
                season: "1".to_string(),
                episode: 4,
                episode_title: None,
                dates: jumbie_shared::types::ReleaseDates {
                    meta_date: None,
                    upload_date: None,
                    est_date: None,
                },
                eff_date: String::new(),
                status: "missing".to_string(),
                assigned: false,
                disk_present: false,
            }],
        };
        write_cache("fetch_calendar_june", &cal_response);

        let vm = test_episode_vm("ep-4", "downloaded", None, Some("/p"));
        patch_calendar_cache(&vm, "S", &test_release_config(), &TimeFormat::Hour12);

        // Non-calendar cache should be untouched
        let series: Option<Vec<String>> = read_cache("series_list");
        assert_eq!(
            series,
            Some(vec!["data".to_string()]),
            "Non-calendar cache should not be affected"
        );
    }

    // refresh_cached_with

    #[wasm_bindgen_test]
    async fn test_refresh_cached_with_bypasses_ttl() {
        // Initialize the async executor so spawn_local works (same as use_autosave).
        _ = any_spawner::Executor::init_wasm_bindgen();
        clear_cache("");
        clear_pending();
        let key = "refresh_ttl_test".to_string();

        // A TTL-fresh entry must NOT suppress refresh_cached_with (unlike
        // spawn_cached_with, which would silently skip and serve the stale data).
        write_cache(&key, &"stale".to_string());
        assert!(is_cache_fresh(&key));

        let fetched = Arc::new(Mutex::new(0));
        let fetched_clone = fetched.clone();
        let called = Arc::new(Mutex::new(false));
        let called_clone = called.clone();

        refresh_cached_with(
            key.clone(),
            move || {
                *fetched_clone.lock().unwrap() += 1;
                async move { Ok::<String, crate::api_client::ApiError>("fresh".to_string()) }
            },
            move |data: String| {
                assert_eq!(data, "fresh");
                *called_clone.lock().unwrap() = true;
            },
        );

        // Poll until the spawned future completes (bounded, not a busy loop).
        for _ in 0..100 {
            if *called.lock().unwrap() {
                break;
            }
            gloo_timers::future::TimeoutFuture::new(10).await;
        }

        assert!(
            *called.lock().unwrap(),
            "on_data must fire despite the fresh cache entry"
        );
        assert_eq!(*fetched.lock().unwrap(), 1, "fetcher runs exactly once");
        assert_eq!(read_cache::<String>(&key).as_deref(), Some("fresh"));
        clear_pending();
    }

    #[wasm_bindgen_test]
    fn test_refresh_cached_with_registers_pending_callback() {
        clear_cache("");
        clear_pending();
        let key = "refresh_pending".to_string();

        // While a request is in-flight, refresh_cached_with must NOT spawn a
        // duplicate fetch — it registers a completion callback that reads cache.
        mark_pending(&key);
        write_cache(&key, &"cached".to_string());

        let fetcher_called = Arc::new(Mutex::new(false));
        let fetcher_clone = fetcher_called.clone();
        let called = Arc::new(Mutex::new(false));
        let called_clone = called.clone();

        refresh_cached_with(
            key.clone(),
            move || {
                *fetcher_clone.lock().unwrap() = true;
                async move { Ok::<String, crate::api_client::ApiError>("network".to_string()) }
            },
            move |data: String| {
                assert_eq!(data, "cached");
                *called_clone.lock().unwrap() = true;
            },
        );

        assert!(
            !*fetcher_called.lock().unwrap(),
            "no duplicate fetch while pending"
        );
        assert!(!*called.lock().unwrap());

        resolve_pending(&key);
        assert!(
            *called.lock().unwrap(),
            "pending callback fires with the cached data"
        );
    }

    // find_calendar_cache_overlap: freshest window wins

    #[wasm_bindgen_test]
    fn test_overlap_keeps_freshest_window() {
        clear_cache("");
        use jumbie_shared::types::{CalendarEpisode, CalendarResponse};

        let ep = |id: &str, assigned: bool| CalendarEpisode {
            series_title: "S".to_string(),
            series_id: "s-1".to_string(),
            episode_id: id.to_string(),
            season: "1".to_string(),
            episode: 1,
            episode_title: None,
            dates: jumbie_shared::types::ReleaseDates {
                meta_date: Some("2026-06-20T20:00:00+00:00".to_string()),
                upload_date: None,
                est_date: None,
            },
            eff_date: "2026-06-20T20:00:00+00:00".to_string(),
            status: if assigned {
                "downloaded".to_string()
            } else {
                "missing".to_string()
            },
            assigned,
            disk_present: false,
        };

        // Older window holds stale "missing" data.
        write_cache(
            "fetch_calendar_2026-06-01_2026-07-01",
            &CalendarResponse {
                episodes: vec![ep("ep-1", false)],
            },
        );
        // Rewind the older window's timestamp so the newer window wins.
        API_CACHE_TIMESTAMPS.with(|ts| {
            ts.borrow_mut()
                .insert("fetch_calendar_2026-06-01_2026-07-01".to_string(), 1000.0);
        });
        // Newer window holds fresh "downloaded" data for the same episode.
        write_cache(
            "fetch_calendar_2026-06-15_2026-07-15",
            &CalendarResponse {
                episodes: vec![ep("ep-1", true)],
            },
        );

        let result = crate::utils::find_calendar_cache_overlap(
            chrono::NaiveDate::from_ymd_opt(2026, 6, 1).unwrap(),
            chrono::NaiveDate::from_ymd_opt(2026, 7, 31).unwrap(),
        );

        let ep1 = result
            .cached_episodes
            .iter()
            .find(|e| e.episode_id == "ep-1");
        assert!(
            ep1.is_some(),
            "ep-1 should be present in the merged overlap"
        );
        assert!(
            ep1.unwrap().assigned,
            "the freshest window's episode must win over the stale one"
        );
    }

    // invalidate_series_caches

    #[wasm_bindgen_test]
    fn test_invalidate_series_caches() {
        clear_cache("");

        write_cache("fetch_series_details_series-a", &"details".to_string());
        write_cache("fetch_series_details_series-b", &"details".to_string());
        write_cache("fetch_calendar_2026-06-01_2026-07-01", &"cal".to_string());
        write_cache("fetch_wanted_episodes:0", &"wanted".to_string());
        // Unrelated entries must survive.
        write_cache("fetch_series", &"list".to_string());
        write_cache("fetch_activity:0", &"activity".to_string());

        invalidate_series_caches(&["series-a".to_string()]);

        assert_eq!(read_cache::<String>("fetch_series_details_series-a"), None);
        assert_eq!(
            read_cache::<String>("fetch_series_details_series-b"),
            Some("details".to_string()),
            "other series' details must survive"
        );
        // The calendar window here holds a non-CalendarResponse value, so the
        // surgical removal skips it — entries are filtered, not wiped wholesale.
        assert_eq!(
            read_cache::<String>("fetch_calendar_2026-06-01_2026-07-01"),
            Some("cal".to_string())
        );
        assert_eq!(read_cache::<String>("fetch_wanted_episodes:0"), None);
        assert_eq!(
            read_cache::<String>("fetch_series"),
            Some("list".to_string()),
            "fetch_series is managed surgically by the delete/hide call sites, not wiped here"
        );
        assert_eq!(
            read_cache::<String>("fetch_activity:0"),
            Some("activity".to_string())
        );
    }

    #[wasm_bindgen_test]
    fn test_remove_series_from_calendar_cache() {
        clear_cache("");
        use jumbie_shared::types::{CalendarEpisode, CalendarResponse};

        let ep = |sid: &str, id: &str| CalendarEpisode {
            series_title: sid.to_string(),
            series_id: sid.to_string(),
            episode_id: id.to_string(),
            season: "1".to_string(),
            episode: 1,
            episode_title: None,
            dates: jumbie_shared::types::ReleaseDates {
                meta_date: Some("2026-06-20T20:00:00+00:00".to_string()),
                upload_date: None,
                est_date: None,
            },
            eff_date: "2026-06-20T20:00:00+00:00".to_string(),
            status: "missing".to_string(),
            assigned: false,
            disk_present: false,
        };

        let ck = "fetch_calendar_2026-06-01_2026-07-01";
        write_cache(
            ck,
            &CalendarResponse {
                episodes: vec![ep("series-a", "ep-a1"), ep("series-b", "ep-b1")],
            },
        );

        // Remove only series-a — series-b's window must survive untouched.
        remove_series_from_calendar_cache(&["series-a".to_string()]);

        let cached: Option<CalendarResponse> = read_cache(ck);
        let episodes = cached.expect("window should still be cached").episodes;
        assert_eq!(
            episodes.len(),
            1,
            "only the removed series' episodes leave the window"
        );
        assert_eq!(episodes[0].series_id, "series-b");

        // Removing an absent series is a no-op.
        remove_series_from_calendar_cache(&["series-ghost".to_string()]);
        let cached: Option<CalendarResponse> = read_cache(ck);
        assert_eq!(cached.expect("window survives").episodes.len(), 1);
    }

    #[wasm_bindgen_test]
    fn test_rename_series_in_caches() {
        clear_cache("");
        use jumbie_shared::types::{
            CalendarEpisode, CalendarResponse, PaginatedResponse, ReleaseDates, SeriesInfo,
            WantedEpisode,
        };

        let cal_ep = |sid: &str, s_title: &str, id: &str| CalendarEpisode {
            series_title: s_title.to_string(),
            series_id: sid.to_string(),
            episode_id: id.to_string(),
            season: "1".to_string(),
            episode: 1,
            episode_title: None,
            dates: ReleaseDates {
                meta_date: Some("2026-06-20T20:00:00+00:00".to_string()),
                upload_date: None,
                est_date: None,
            },
            eff_date: "2026-06-20T20:00:00+00:00".to_string(),
            status: "missing".to_string(),
            assigned: false,
            disk_present: false,
        };
        let wanted_item = |sid: &str, s_title: &str, eid: &str| WantedEpisode {
            series_id: sid.to_string(),
            series_title: s_title.to_string(),
            episode_id: eid.to_string(),
            season: Some("1".to_string()),
            episode: 1,
            title: None,
            eff_date: String::new(),
            dates: ReleaseDates {
                meta_date: None,
                upload_date: None,
                est_date: None,
            },
            status: "missing".to_string(),
        };
        let series_info = |sid: &str, title: &str| SeriesInfo {
            id: sid.to_string(),
            title: title.to_string(),
            seasons: vec![],
            season_count: 0,
            release_profile: String::new(),
            quality_profile: String::new(),
            episodes_counts: (0, 0),
            monitored_missing_count: 0,
            queued_count: 0,
            size: 0,
            path: String::new(),
            scan_queue_count: 0,
            absolute_numbering: false,
            aliases: vec![],
            has_not_found_files: false,
        };

        // Calendar window with episodes of two series.
        let cal_ck = "fetch_calendar_2026-06-01_2026-07-01";
        write_cache(
            cal_ck,
            &CalendarResponse {
                episodes: vec![
                    cal_ep("series-a", "Old A", "ep-a1"),
                    cal_ep("series-b", "Series B", "ep-b1"),
                ],
            },
        );
        // Library list with both series.
        write_cache(
            "fetch_series",
            &vec![
                series_info("series-a", "Old A"),
                series_info("series-b", "Series B"),
            ],
        );
        // Wanted page with items of both series.
        write_cache(
            "fetch_wanted_episodes:0",
            &PaginatedResponse {
                items: vec![
                    wanted_item("series-a", "Old A", "ep-a1"),
                    wanted_item("series-b", "Series B", "ep-b1"),
                ],
                total: 2,
                page: 0,
                page_size: 50,
            },
        );

        rename_series_in_caches("series-a", "New A");

        // Calendar window survives and only series-a's episodes are retitled.
        let cached: Option<CalendarResponse> = read_cache(cal_ck);
        let eps = cached.expect("calendar window must stay cached").episodes;
        assert_eq!(eps.len(), 2);
        let a = eps.iter().find(|e| e.series_id == "series-a").unwrap();
        assert_eq!(a.series_title, "New A");
        let b = eps.iter().find(|e| e.series_id == "series-b").unwrap();
        assert_eq!(b.series_title, "Series B", "other series untouched");

        // Library list entry retitled in place.
        let list: Option<Vec<SeriesInfo>> = read_cache("fetch_series");
        let list = list.unwrap();
        assert_eq!(
            list.iter().find(|s| s.id == "series-a").unwrap().title,
            "New A"
        );
        assert_eq!(
            list.iter().find(|s| s.id == "series-b").unwrap().title,
            "Series B"
        );

        // Wanted page item retitled in place.
        let page: Option<PaginatedResponse<WantedEpisode>> = read_cache("fetch_wanted_episodes:0");
        let items = page.unwrap().items;
        assert_eq!(
            items
                .iter()
                .find(|i| i.series_id == "series-a")
                .unwrap()
                .series_title,
            "New A"
        );
        assert_eq!(
            items
                .iter()
                .find(|i| i.series_id == "series-b")
                .unwrap()
                .series_title,
            "Series B"
        );
    }

    #[wasm_bindgen_test]
    fn test_write_calendar_cache_change_detection() {
        clear_cache("");
        use jumbie_shared::types::{CalendarEpisode, CalendarResponse};

        let ep = |assigned: bool| CalendarEpisode {
            series_title: "S".to_string(),
            series_id: "s-1".to_string(),
            episode_id: "ep-1".to_string(),
            season: "1".to_string(),
            episode: 1,
            episode_title: None,
            dates: jumbie_shared::types::ReleaseDates {
                meta_date: Some("2026-06-20T20:00:00+00:00".to_string()),
                upload_date: None,
                est_date: None,
            },
            eff_date: "2026-06-20T20:00:00+00:00".to_string(),
            status: if assigned {
                "downloaded".to_string()
            } else {
                "missing".to_string()
            },
            assigned,
            disk_present: false,
        };

        // Seed sibling caches that must be invalidated when the window changes.
        write_cache("fetch_series", &"list".to_string());
        write_cache("fetch_wanted_episodes:0", &"wanted".to_string());

        let ck = "fetch_calendar_2026-06-01_2026-07-01";
        // First write: nothing cached to compare against → no invalidation.
        write_calendar_cache_with_change_detection(
            ck,
            &CalendarResponse {
                episodes: vec![ep(false)],
            },
        );
        assert_eq!(
            read_cache::<String>("fetch_series"),
            Some("list".to_string()),
            "no cached baseline → no sibling invalidation"
        );

        // Second write with different data → change detected → siblings wiped.
        write_calendar_cache_with_change_detection(
            ck,
            &CalendarResponse {
                episodes: vec![ep(true)],
            },
        );
        assert_eq!(read_cache::<String>("fetch_series"), None);
        assert_eq!(read_cache::<String>("fetch_wanted_episodes:0"), None);

        // Identical rewrite → no change → re-seeded siblings survive.
        write_cache("fetch_series", &"list2".to_string());
        write_calendar_cache_with_change_detection(
            ck,
            &CalendarResponse {
                episodes: vec![ep(true)],
            },
        );
        assert_eq!(
            read_cache::<String>("fetch_series"),
            Some("list2".to_string()),
            "identical window → no invalidation"
        );
    }

    // use_autosave

    #[wasm_bindgen_test]
    async fn test_use_autosave_reactivity() {
        // Initialize the async executor (same as mount_to_body does)
        // so that Action::new_local and spawn_local work.
        _ = any_spawner::Executor::init_wasm_bindgen();

        use crate::api_client::ApiError;
        use leptos::prelude::*;

        let config: ReadSignal<Option<Config>> = signal(Some(Config::default())).0;
        let (saved, set_saved) = signal(0u32);

        // Count how many times the save callback is invoked
        let on_save = move |_cfg: Config| {
            let set_saved = set_saved.clone();
            async move {
                set_saved.update(|n| *n += 1);
                Ok::<(), ApiError>(())
            }
        };

        let _autosave = use_autosave(config, on_save);

        // Wait a tick for the initial effect to settle
        gloo_timers::future::TimeoutFuture::new(50).await;

        let saved_count = saved.get_untracked();
        assert_eq!(
            saved_count, 0,
            "Autosave should not trigger on initial config"
        );
    }
}
