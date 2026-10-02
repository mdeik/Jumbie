// Route registry — single source of truth for all route paths & navigation.
//
// The `path` module below holds every URL path. app.rs must repeat these as
// string literals in <Route path=...> (the leptos_router macro needs literals),
// so changing a path means updating both here and in app.rs.
//
// Adding a new route:
//   1. Add its path constant in the `path` module below
//   2. Add its nav item/group in the navigation tree
//   3. Add an icon match in nav_icon() (if it's a top-level item)
//   4. Wire up the <Route> in app.rs

use crate::components::common::icons::*;
use leptos::prelude::*;

// Route path constants — SSoT for every URL path in the application

pub mod path {
    // Standalone routes
    pub const SERIES: &str = "series";
    pub const SERIES_ADD: &str = "series/add";
    pub const CALENDAR: &str = "calendar";
    pub const WANTED: &str = "wanted";
    pub const ACTIVITY: &str = "activity";

    /// Abstract nav identifier for the edit-series page.
    /// Does NOT correspond to a concrete URL path (the actual route is
    /// `/series/:id/edit`). Used only as a sidebar metadata entry.
    pub const EDIT_SERIES: &str = "edit_series";

    // Settings group
    pub const SETTINGS: &str = "settings";
    pub const SETTINGS_GENERAL: &str = "settings/general";
    pub const SETTINGS_ORGANIZATION: &str = "settings/organization";

    // Plugins group
    pub const PLUGINS: &str = "plugins";
    pub const PLUGINS_SOURCES: &str = "plugins/sources";
    pub const PLUGINS_CLIENTS: &str = "plugins/clients";
    pub const PLUGINS_NOTIFIERS: &str = "plugins/notifiers";
    pub const PLUGINS_METADATA: &str = "plugins/metadata";

    // Profiles group
    pub const PROFILES: &str = "profiles";
    pub const PROFILES_RELEASE: &str = "profiles/release";
    pub const PROFILES_QUALITY: &str = "profiles/quality";

    // Management group
    pub const MANAGEMENT: &str = "management";
    pub const MANAGEMENT_RENAME: &str = "management/rename";
    pub const MANAGEMENT_DOWNLOAD: &str = "management/download";
    pub const MANAGEMENT_ORGANIZED: &str = "management/organized";

    // Authentication group
    pub const AUTH: &str = "authentication";
    pub const AUTH_ACCOUNT: &str = "authentication/account";
    pub const AUTH_SECURITY: &str = "authentication/security";
    pub const AUTH_API: &str = "authentication/api";

    // System group
    pub const SYSTEM: &str = "system";
    pub const SYSTEM_STATUS: &str = "system/status";
    pub const SYSTEM_LOGS: &str = "system/logs";
    pub const SYSTEM_ABOUT: &str = "system/about";
}

// Navigation types

/// A single navigable item in the sidebar.
///
/// `id` matches the route path (without leading slash) so the sidebar can
/// compare it against `location.pathname.get().trim_start_matches('/')`
/// to determine the active state.
///
/// Indicator logic (warning/danger/critical dots) is NOT defined here.
/// It lives in `sidebar.rs`'s `compute_indicator_levels()`, which maps
/// nav-item IDs to visual levels — the single source of truth for all
/// nav indicators.
#[derive(Clone, Copy)]
pub struct NavItem {
    pub id: &'static str,
    pub label: &'static str,
    /// Short description shown in landing-page status cards.
    pub desc: &'static str,
    /// Override for the page header title; falls back to `label` when unset.
    pub header_title: Option<&'static str>,
    /// Whether this item appears in the sidebar navigation.
    /// Set to `false` for items that exist only for metadata lookups.
    pub show_in_sidebar: bool,
}

impl NavItem {
    /// Create a sidebar-visible nav item.
    pub const fn sidebar(id: &'static str, label: &'static str, desc: &'static str) -> Self {
        Self {
            id,
            label,
            desc,
            header_title: None,
            show_in_sidebar: true,
        }
    }

    /// Create a sidebar-visible nav item with a custom header title.
    pub const fn sidebar_with_title(
        id: &'static str,
        label: &'static str,
        desc: &'static str,
        header_title: &'static str,
    ) -> Self {
        Self {
            id,
            label,
            desc,
            header_title: Some(header_title),
            show_in_sidebar: true,
        }
    }

    /// The title to display in the page header.
    pub fn header_title(&self) -> &'static str {
        self.header_title.unwrap_or(self.label)
    }
}

/// A parent navigation group with an ordered list of child items.
pub struct NavGroup {
    pub id: &'static str,
    pub label: &'static str,
    pub header_title: Option<&'static str>,
    pub children: &'static [NavItem],
}

impl NavGroup {
    /// The title to display in the page header.
    pub fn header_title(&self) -> &'static str {
        self.header_title.unwrap_or(self.label)
    }
}

/// Standalone top-level items (not inside a parent group).
pub static NAV_STANDALONE: &[NavItem] = &[
    NavItem::sidebar_with_title(path::SERIES, "Series", "", "Series Library"),
    NavItem::sidebar_with_title(path::CALENDAR, "Calendar", "", "Schedule"),
    NavItem::sidebar(path::WANTED, "Wanted", ""),
    NavItem::sidebar(path::ACTIVITY, "Activity", ""),
    // edit_series and series_add are defined here for metadata lookups but
    // not shown in the sidebar (they have no corresponding top-level nav entry).
    NavItem {
        id: path::EDIT_SERIES,
        label: "Edit Series",
        desc: "",
        header_title: None,
        show_in_sidebar: false,
    },
    NavItem {
        id: path::SERIES_ADD,
        label: "Add Series",
        desc: "",
        header_title: None,
        show_in_sidebar: false,
    },
];

// Plugins nav children — ordered by the shared category-order SSoT.
//
// `PLUGIN_CATEGORY_ORDER` (`jumbie_shared::plugin`) is the single source of truth
// for the category order; this table only maps a raw category id to its sidebar
// item. Reordering the shared const reorders the Plugins sidebar — there is no
// second list of categories to keep in sync.

/// Maps a raw plugin category id to its sidebar item.
const fn plugin_category_nav(category: &str) -> NavItem {
    if str_eq(category, "source") {
        NavItem::sidebar(path::PLUGINS_SOURCES, "Sources", "Indexers & Feeds")
    } else if str_eq(category, "metadata") {
        NavItem::sidebar(path::PLUGINS_METADATA, "Metadata", "Metadata providers")
    } else if str_eq(category, "downloader") {
        NavItem::sidebar(path::PLUGINS_CLIENTS, "Downloaders", "Download clients")
    } else if str_eq(category, "notifier") {
        NavItem::sidebar(
            path::PLUGINS_NOTIFIERS,
            "Notifiers",
            "Notification services",
        )
    } else {
        panic!("plugin category has no nav item — add it to plugin_category_nav()")
    }
}

/// `str` equality usable in `const` context (`str: PartialEq` is not `const`).
const fn str_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

const PLUGIN_CATEGORY_COUNT: usize = jumbie_shared::plugin::PLUGIN_CATEGORY_ORDER.len();

static PLUGIN_NAV_CHILDREN: [NavItem; PLUGIN_CATEGORY_COUNT] = build_plugin_nav_children();

/// Built by indexing `PLUGIN_CATEGORY_ORDER`, so the emitted order is that const's
/// order. An unmapped category fails compilation (const `panic!`).
const fn build_plugin_nav_children() -> [NavItem; PLUGIN_CATEGORY_COUNT] {
    let mut items = [NavItem::sidebar("", "", ""); PLUGIN_CATEGORY_COUNT];
    let mut i = 0;
    while i < PLUGIN_CATEGORY_COUNT {
        items[i] = plugin_category_nav(jumbie_shared::plugin::PLUGIN_CATEGORY_ORDER[i]);
        i += 1;
    }
    items
}

/// Parent groups, each with an ordered list of children.
pub static NAV_GROUPS: &[NavGroup] = &[
    NavGroup {
        id: path::PROFILES,
        label: "Profiles",
        header_title: Some("Profile Management"),
        children: &[
            NavItem::sidebar(
                path::PROFILES_QUALITY,
                "Quality Profiles",
                "Groups of Enabled Qualities",
            ),
            NavItem::sidebar(
                path::PROFILES_RELEASE,
                "Release Profiles",
                "Scoring Weights & Minimums",
            ),
        ],
    },
    NavGroup {
        id: path::SETTINGS,
        label: "Settings",
        header_title: Some("Configuration"),
        children: &[
            NavItem::sidebar(
                path::SETTINGS_GENERAL,
                "General",
                "Scanning, Metadata & Behavior",
            ),
            NavItem::sidebar(
                path::SETTINGS_ORGANIZATION,
                "Organization",
                "Naming, Folder Structure & File Handling",
            ),
        ],
    },
    NavGroup {
        id: path::PLUGINS,
        label: "Plugins",
        header_title: Some("Integrations"),
        // Derived from `PLUGIN_CATEGORY_ORDER` (see `PLUGIN_NAV_CHILDREN`).
        children: &PLUGIN_NAV_CHILDREN,
    },
    NavGroup {
        id: path::MANAGEMENT,
        label: "Management",
        header_title: None,
        children: &[
            NavItem::sidebar(
                path::MANAGEMENT_RENAME,
                "Rename Queue",
                "Pending File Renames",
            ),
            NavItem::sidebar(
                path::MANAGEMENT_DOWNLOAD,
                "Download Queue",
                "Pending & Active Downloads",
            ),
            NavItem::sidebar(
                path::MANAGEMENT_ORGANIZED,
                "Organized Series",
                "Managed Folders & Monitoring",
            ),
        ],
    },
    NavGroup {
        id: path::AUTH,
        label: "Authentication",
        header_title: None,
        children: &[
            NavItem::sidebar(path::AUTH_ACCOUNT, "Account", "Manage your account"),
            NavItem::sidebar(
                path::AUTH_SECURITY,
                "Security",
                "Manage proxy and access settings",
            ),
            NavItem::sidebar(path::AUTH_API, "API", "Generate API keys and scopes"),
        ],
    },
    NavGroup {
        id: path::SYSTEM,
        label: "System",
        header_title: Some("System Tools"),
        children: &[
            NavItem::sidebar(path::SYSTEM_STATUS, "Status", "Health Checks & Stats"),
            NavItem::sidebar(path::SYSTEM_LOGS, "Logs", "Application Logs"),
            NavItem::sidebar(path::SYSTEM_ABOUT, "About", "Version Info"),
        ],
    },
];

// Lookup functions

/// Returns the [`NavGroup`] with the given id, if any.
pub fn find_group(id: &str) -> Option<&'static NavGroup> {
    NAV_GROUPS.iter().find(|g| g.id == id)
}

/// Returns the sidebar label for a route id (standalone, group, or child).
pub fn label(id: &str) -> &'static str {
    for item in NAV_STANDALONE {
        if item.id == id {
            return item.label;
        }
    }
    for group in NAV_GROUPS {
        if group.id == id {
            return group.label;
        }
        for child in group.children {
            if child.id == id {
                return child.label;
            }
        }
    }
    ""
}

/// Returns the page header title for a route id (standalone, group, or child).
/// Falls back to the sidebar label if no explicit `header_title` is set.
pub fn header_title(id: &str) -> &'static str {
    for item in NAV_STANDALONE {
        if item.id == id {
            return item.header_title();
        }
    }
    for group in NAV_GROUPS {
        if group.id == id {
            return group.header_title();
        }
        for child in group.children {
            if child.id == id {
                return child.header_title();
            }
        }
    }
    ""
}

// Icon mapping — one place so sidebar.rs doesn't need its own match

/// Returns the icon component for a top-level route id.
/// Child items always use [`DotIcon`].
pub fn nav_icon(id: &str) -> impl IntoView {
    match id {
        path::SERIES => view! { <TvIcon/> }.into_any(),
        path::CALENDAR => view! { <CalendarIcon/> }.into_any(),
        path::WANTED => view! { <SearchIcon/> }.into_any(),
        path::ACTIVITY => view! { <ActivityIcon/> }.into_any(),
        path::PROFILES => view! { <ClapperboardIcon/> }.into_any(),
        path::MANAGEMENT => view! { <FolderIcon/> }.into_any(),
        path::AUTH => view! { <LockIcon/> }.into_any(),
        path::SETTINGS => view! { <SettingsIcon/> }.into_any(),
        path::PLUGINS => view! { <PuzzleIcon/> }.into_any(),
        path::SYSTEM => view! { <MonitorIcon/> }.into_any(),
        _ => view! { <DotIcon/> }.into_any(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Plugins sidebar order is the requirement that the sort order in the
    /// backend mirrors; pin it here so a shared-const reorder is deliberate.
    #[test]
    fn plugins_nav_group_follows_category_order() {
        let group = find_group(path::PLUGINS).expect("Plugins nav group must exist");

        // Tied to the shared SSoT: one sidebar child per category.
        assert_eq!(
            group.children.len(),
            jumbie_shared::plugin::PLUGIN_CATEGORY_ORDER.len()
        );

        let labels: Vec<&str> = group.children.iter().map(|c| c.label).collect();
        assert_eq!(labels, ["Sources", "Metadata", "Downloaders", "Notifiers"]);

        let ids: Vec<&str> = group.children.iter().map(|c| c.id).collect();
        assert_eq!(
            ids,
            [
                path::PLUGINS_SOURCES,
                path::PLUGINS_METADATA,
                path::PLUGINS_CLIENTS,
                path::PLUGINS_NOTIFIERS,
            ]
        );
    }
}
