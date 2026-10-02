use crate::plugins::{RuntimePluginState, RuntimePluginTypeInfo};

/// Minimal entry for ordering tests — `Failed` avoids constructing a real
/// `PluginInstance` while still exercising the fields the sort reads.
fn entry(id: &str, name: &str, category: &str) -> RuntimePluginTypeInfo {
    RuntimePluginTypeInfo {
        id: id.to_string(),
        name: name.to_string(),
        category: category.to_string(),
        state: RuntimePluginState::Failed("test".to_string()),
    }
}

#[test]
fn plugin_status_sort_orders_categories_sources_metadata_downloader_notifier() {
    // Deliberately shuffled; expected order is the canonical Sources → Metadata
    // → Downloaders → Notifiers, NOT alphabetical (which would be downloader,
    // metadata, notifier, source).
    let mut statuses = vec![
        entry("n1", "Discord", "notifier"),
        entry("d1", "qBittorrent", "downloader"),
        entry("m1", "tvdb", "metadata"),
        entry("s1", "nyaa", "source"),
    ];

    crate::plugins::sort_plugin_statuses(&mut statuses, |_| None);

    let got: Vec<&str> = statuses.iter().map(|e| e.category.as_str()).collect();
    assert_eq!(got, vec!["source", "metadata", "downloader", "notifier"]);
}

#[test]
fn plugin_status_sort_breaks_ties_by_case_insensitive_name_then_type_id() {
    let mut statuses = vec![
        entry("i3", "beta", "downloader"),
        entry("i1", "Alpha", "downloader"),
        entry("i2", "alpha", "downloader"),
    ];

    // "Alpha"/"alpha" collapse to the same lowercased name, so the type id
    // decides: alpha.type (i2) before zeta.type (i1). "beta" (i3) sorts last.
    crate::plugins::sort_plugin_statuses(&mut statuses, |id| match id {
        "i1" => Some("zeta.type".to_string()),
        "i2" => Some("alpha.type".to_string()),
        _ => None,
    });

    let got: Vec<&str> = statuses.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(got, vec!["i2", "i1", "i3"]);
}

#[test]
fn plugin_status_sort_falls_back_to_instance_id_for_full_determinism() {
    // Same name, same category, same (looked-up) type id — only the instance id
    // can break the tie, which is required for multiple instances of one type.
    let mut statuses = vec![entry("b", "Same", "source"), entry("a", "Same", "source")];

    crate::plugins::sort_plugin_statuses(&mut statuses, |_| Some("jumbie.same".to_string()));

    let got: Vec<&str> = statuses.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(got, vec!["a", "b"]);
}

#[test]
fn plugin_status_sort_puts_unknown_categories_last() {
    let mut statuses = vec![
        entry("u", "Mystery", "unknown-category"),
        entry("s", "nyaa", "source"),
    ];

    crate::plugins::sort_plugin_statuses(&mut statuses, |_| None);

    let got: Vec<&str> = statuses.iter().map(|e| e.category.as_str()).collect();
    assert_eq!(got, vec!["source", "unknown-category"]);
}
