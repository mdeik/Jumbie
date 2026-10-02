// Capability toggles (SSoT)
//
// Dispatch rule: a capability is available on an instance only when the plugin
// TYPE declares it AND the user has not disabled the matching per-instance
// config toggle: effective(C) = declared(C) AND (toggle(C) != false).
//
// The single source of truth for the field→capability mapping, read by both the
// settings-schema injector (which renders the toggles) and the manager's
// capability queries (which enforce them).

use jumbie_shared::plugin::Capability;
use serde_json::Value;

/// Config field → the capability it gates.
///
/// A capability listed here is only effective when its toggle is enabled.
/// A missing toggle defaults to enabled (matching the schema/serde defaults).
/// Capabilities not listed here are never gated by config.
pub const CAPABILITY_TOGGLES: &[(&str, Capability)] = &[
    ("enable_polling", Capability::Polling),
    ("enable_manual_search", Capability::ManualSearch),
    ("enable_automatic_search", Capability::AutomaticSearch),
    ("enable_seeding", Capability::CanSeed),
];

/// The per-instance config field that gates `capability`, if any.
pub fn toggle_field_for_capability(capability: &Capability) -> Option<&'static str> {
    CAPABILITY_TOGGLES
        .iter()
        .find(|(_, cap)| cap == capability)
        .map(|(field, _)| *field)
}

/// The capability gated by per-instance config field `field`, if any.
pub fn capability_for_toggle(field: &str) -> Option<Capability> {
    CAPABILITY_TOGGLES
        .iter()
        .find(|(f, _)| *f == field)
        .map(|(_, cap)| cap.clone())
}

/// Whether `capability` is enabled for an instance whose config is `config`.
///
/// Capabilities without a toggle are always enabled; a missing toggle defaults
/// to enabled.
pub fn is_capability_enabled(capability: &Capability, config: Option<&Value>) -> bool {
    match toggle_field_for_capability(capability) {
        Some(field) => config
            .and_then(|c| c.get(field))
            .and_then(|v| v.as_bool())
            .unwrap_or(true),
        None => true,
    }
}

/// Effective capabilities = declared ∩ enabled.
///
/// `declared` is the plugin type's static capability list; `config` is the
/// instance's raw config (from `PluginManager::applied_configs`). Capabilities
/// the plugin does not declare can never appear here, so a user toggle can only
/// *disable* a supported capability, never grant an unsupported one.
pub fn effective_capabilities(declared: &[Capability], config: Option<&Value>) -> Vec<Capability> {
    declared
        .iter()
        .filter(|cap| is_capability_enabled(cap, config))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn declared_and_enabled_is_kept() {
        let caps = vec![Capability::Polling];
        assert_eq!(
            effective_capabilities(&caps, Some(&json!({ "enable_polling": true }))),
            caps
        );
    }

    #[test]
    fn declared_but_disabled_is_dropped() {
        let caps = vec![Capability::Polling, Capability::FeedProvider];
        let out = effective_capabilities(&caps, Some(&json!({ "enable_polling": false })));
        assert!(!out.contains(&Capability::Polling));
        // Non-toggle capability passes through untouched.
        assert!(out.contains(&Capability::FeedProvider));
    }

    #[test]
    fn missing_toggle_defaults_to_enabled() {
        let caps = vec![Capability::Polling, Capability::ManualSearch];
        assert_eq!(effective_capabilities(&caps, Some(&json!({}))), caps);
        assert_eq!(effective_capabilities(&caps, None), caps);
    }

    #[test]
    fn undeclared_capability_is_never_present() {
        // Polling enabled in config, but the plugin type doesn't declare it.
        let caps = vec![Capability::MetadataProviderNormal];
        let out = effective_capabilities(&caps, Some(&json!({ "enable_polling": true })));
        assert!(!out.contains(&Capability::Polling));
        assert!(out.contains(&Capability::MetadataProviderNormal));
    }

    #[test]
    fn toggle_and_capability_lookups_round_trip() {
        for (field, cap) in CAPABILITY_TOGGLES {
            assert_eq!(capability_for_toggle(field), Some(cap.clone()));
            assert_eq!(toggle_field_for_capability(cap), Some(*field));
        }
        assert_eq!(capability_for_toggle("not_a_toggle"), None);
        assert_eq!(toggle_field_for_capability(&Capability::Downloader), None);
    }
}
