// Indicator types and computation, extracted into a standalone module so the pure
// logic can be unit-tested without a WASM runtime. sidebar.rs imports these and
// wraps them in the Leptos reactive layer.
//
// Priority order (lowest → highest): Warning (yellow dot) → Danger (red dot) →
// Critical (red pulsing). Only the highest active level is ever `true` — the match
// arms in `compute_indicator_levels` enforce this.

/// All system-state booleans that nav-item indicators depend on.
/// Kept in sync with the backend `SystemStatus` payload.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StateSnapshot {
    pub rename_populated: bool,
    pub rename_failed: bool,
    pub downloader_disabled: bool,
    pub download_queue_failed: bool,
    pub download_queue_populated: bool,
    pub wanted_has_items: bool,
}

/// Visual indicator level for a nav item.
/// Only the highest-priority field is ever set to `true`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct IndicatorLevels {
    pub should_warn: bool,
    pub should_danger: bool,
    pub should_critical: bool,
}

// SSoT: the single function that computes indicator levels for any nav item (leaf
// or group) given its id and the aggregated indicator state. Both leaf items and
// parent groups in sidebar.rs delegate here. Every match arm checks only the signals
// belonging to its domain, preventing crossed signals.
pub fn compute_indicator_levels(id: &str, s: StateSnapshot) -> IndicatorLevels {
    match id {
        "management/rename" => {
            if s.rename_failed {
                IndicatorLevels {
                    should_critical: true,
                    ..Default::default()
                }
            } else if s.rename_populated {
                IndicatorLevels {
                    should_warn: true,
                    ..Default::default()
                }
            } else {
                IndicatorLevels::default()
            }
        }
        "management/download" => {
            if s.download_queue_failed {
                IndicatorLevels {
                    should_critical: true,
                    ..Default::default()
                }
            } else if s.downloader_disabled && s.download_queue_populated {
                IndicatorLevels {
                    should_danger: true,
                    ..Default::default()
                }
            } else if s.download_queue_populated {
                IndicatorLevels {
                    should_warn: true,
                    ..Default::default()
                }
            } else {
                IndicatorLevels::default()
            }
        }
        "wanted" => {
            if s.wanted_has_items {
                IndicatorLevels {
                    should_warn: true,
                    ..Default::default()
                }
            } else {
                IndicatorLevels::default()
            }
        }
        // Management group — aggregates its children's signals.
        "management" => {
            if s.rename_failed || s.download_queue_failed {
                IndicatorLevels {
                    should_critical: true,
                    ..Default::default()
                }
            } else if s.rename_populated {
                IndicatorLevels {
                    should_warn: true,
                    ..Default::default()
                }
            } else if s.downloader_disabled && s.download_queue_populated {
                IndicatorLevels {
                    should_danger: true,
                    ..Default::default()
                }
            } else if s.download_queue_populated {
                IndicatorLevels {
                    should_warn: true,
                    ..Default::default()
                }
            } else {
                IndicatorLevels::default()
            }
        }
        // System group + its Status child — no lightweight health data is available
        // from the status endpoint (by design — the /health endpoint runs expensive
        // disk/DB checks). Using wanted_has_items here would be a crossed signal.
        "system" | "system/status" => IndicatorLevels::default(),
        _ => IndicatorLevels::default(),
    }
}

// Helpers for tests

#[cfg(test)]
mod tests {
    use super::*;

    // State presets

    fn none() -> IndicatorLevels {
        IndicatorLevels::default()
    }

    fn warn() -> IndicatorLevels {
        IndicatorLevels {
            should_warn: true,
            ..Default::default()
        }
    }

    fn danger() -> IndicatorLevels {
        IndicatorLevels {
            should_danger: true,
            ..Default::default()
        }
    }

    fn critical() -> IndicatorLevels {
        IndicatorLevels {
            should_critical: true,
            ..Default::default()
        }
    }

    fn all_false() -> StateSnapshot {
        StateSnapshot::default()
    }

    // No signal lights any unrelated nav item

    #[test]
    fn all_false_returns_default_for_all() {
        let s = all_false();
        assert_eq!(compute_indicator_levels("wanted", s), none());
        assert_eq!(compute_indicator_levels("management/rename", s), none());
        assert_eq!(compute_indicator_levels("management/download", s), none());
        assert_eq!(compute_indicator_levels("management", s), none());
        assert_eq!(compute_indicator_levels("system", s), none());
        assert_eq!(compute_indicator_levels("system/status", s), none());
        assert_eq!(compute_indicator_levels("series", s), none());
        assert_eq!(compute_indicator_levels("calendar", s), none());
        assert_eq!(compute_indicator_levels("activity", s), none());
    }

    // "wanted" only responds to wanted_has_items

    #[test]
    fn wanted_lights_up_for_wanted_has_items() {
        let s = StateSnapshot {
            wanted_has_items: true,
            ..all_false()
        };
        assert_eq!(compute_indicator_levels("wanted", s), warn());
    }

    #[test]
    fn wanted_ignores_rename_signals() {
        let s = StateSnapshot {
            rename_populated: true,
            ..all_false()
        };
        assert_eq!(compute_indicator_levels("wanted", s), none());
    }

    #[test]
    fn wanted_ignores_download_signals() {
        let s = StateSnapshot {
            download_queue_populated: true,
            ..all_false()
        };
        assert_eq!(compute_indicator_levels("wanted", s), none());
    }

    // "management/rename" only responds to its own signals

    #[test]
    fn rename_shows_critical_on_failure() {
        let s = StateSnapshot {
            rename_failed: true,
            ..all_false()
        };
        assert_eq!(compute_indicator_levels("management/rename", s), critical());
    }

    #[test]
    fn rename_shows_warning_on_populated() {
        let s = StateSnapshot {
            rename_populated: true,
            ..all_false()
        };
        assert_eq!(compute_indicator_levels("management/rename", s), warn());
    }

    #[test]
    fn rename_failure_takes_priority_over_populated() {
        let s = StateSnapshot {
            rename_failed: true,
            rename_populated: true,
            ..all_false()
        };
        assert_eq!(compute_indicator_levels("management/rename", s), critical());
    }

    #[test]
    fn rename_ignores_download_signals() {
        let s = StateSnapshot {
            download_queue_populated: true,
            ..all_false()
        };
        assert_eq!(compute_indicator_levels("management/rename", s), none());
    }

    #[test]
    fn rename_ignores_wanted() {
        let s = StateSnapshot {
            wanted_has_items: true,
            ..all_false()
        };
        assert_eq!(compute_indicator_levels("management/rename", s), none());
    }

    // "management/download" only responds to its own signals

    #[test]
    fn download_shows_critical_on_failure() {
        let s = StateSnapshot {
            download_queue_failed: true,
            ..all_false()
        };
        assert_eq!(
            compute_indicator_levels("management/download", s),
            critical()
        );
    }

    #[test]
    fn download_shows_danger_when_disabled_with_items() {
        let s = StateSnapshot {
            downloader_disabled: true,
            download_queue_populated: true,
            ..all_false()
        };
        assert_eq!(compute_indicator_levels("management/download", s), danger());
    }

    #[test]
    fn download_shows_warning_when_populated() {
        let s = StateSnapshot {
            download_queue_populated: true,
            ..all_false()
        };
        assert_eq!(compute_indicator_levels("management/download", s), warn());
    }

    #[test]
    fn download_no_indicator_when_disabled_but_empty() {
        let s = StateSnapshot {
            downloader_disabled: true,
            ..all_false()
        };
        assert_eq!(compute_indicator_levels("management/download", s), none());
    }

    #[test]
    fn download_failure_takes_priority_over_disabled() {
        let s = StateSnapshot {
            download_queue_failed: true,
            downloader_disabled: true,
            download_queue_populated: true,
            ..all_false()
        };
        assert_eq!(
            compute_indicator_levels("management/download", s),
            critical()
        );
    }

    #[test]
    fn download_disabled_takes_priority_over_populated() {
        let s = StateSnapshot {
            downloader_disabled: true,
            download_queue_populated: true,
            ..all_false()
        };
        assert_eq!(compute_indicator_levels("management/download", s), danger());
    }

    #[test]
    fn download_ignores_rename_signals() {
        let s = StateSnapshot {
            rename_populated: true,
            ..all_false()
        };
        assert_eq!(compute_indicator_levels("management/download", s), none());
    }

    #[test]
    fn download_ignores_wanted() {
        let s = StateSnapshot {
            wanted_has_items: true,
            ..all_false()
        };
        assert_eq!(compute_indicator_levels("management/download", s), none());
    }

    // "management" parent group aggregates its children

    #[test]
    fn management_aggregates_rename_failure() {
        let s = StateSnapshot {
            rename_failed: true,
            ..all_false()
        };
        assert_eq!(compute_indicator_levels("management", s), critical());
    }

    #[test]
    fn management_aggregates_download_failure() {
        let s = StateSnapshot {
            download_queue_failed: true,
            ..all_false()
        };
        assert_eq!(compute_indicator_levels("management", s), critical());
    }

    #[test]
    fn management_aggregates_rename_populated() {
        let s = StateSnapshot {
            rename_populated: true,
            ..all_false()
        };
        assert_eq!(compute_indicator_levels("management", s), warn());
    }

    #[test]
    fn management_aggregates_download_populated() {
        let s = StateSnapshot {
            download_queue_populated: true,
            ..all_false()
        };
        assert_eq!(compute_indicator_levels("management", s), warn());
    }

    #[test]
    fn management_shows_danger_for_disabled_downloader_with_items() {
        let s = StateSnapshot {
            downloader_disabled: true,
            download_queue_populated: true,
            ..all_false()
        };
        assert_eq!(compute_indicator_levels("management", s), danger());
    }

    #[test]
    fn management_critical_takes_priority_over_rename_warn() {
        let s = StateSnapshot {
            rename_failed: true,
            rename_populated: true,
            ..all_false()
        };
        assert_eq!(compute_indicator_levels("management", s), critical());
    }

    #[test]
    fn management_download_disabled_takes_priority_over_populated() {
        let s = StateSnapshot {
            downloader_disabled: true,
            download_queue_populated: true,
            ..all_false()
        };
        assert_eq!(compute_indicator_levels("management", s), danger());
    }

    #[test]
    fn management_does_not_aggregate_wanted() {
        // Wanted is NOT a child of the Management group — it's a standalone nav
        // item. This test ensures it stays that way.
        let s = StateSnapshot {
            wanted_has_items: true,
            ..all_false()
        };
        assert_eq!(compute_indicator_levels("management", s), none());
    }

    // System / Status shows no indicator (no lightweight data)

    #[test]
    fn system_never_shows_indicator() {
        let s = StateSnapshot {
            wanted_has_items: true,
            rename_populated: true,
            download_queue_populated: true,
            ..all_false()
        };
        assert_eq!(compute_indicator_levels("system", s), none());
        assert_eq!(compute_indicator_levels("system/status", s), none());
    }

    // Unknown nav items fall through to default

    #[test]
    fn unknown_nav_item_returns_default() {
        let s = StateSnapshot {
            wanted_has_items: true,
            ..all_false()
        };
        assert_eq!(compute_indicator_levels("series", s), none());
        assert_eq!(compute_indicator_levels("nonexistent", s), none());
    }
}
