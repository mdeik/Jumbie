/**
 * Navigation tree definition — mirrors `frontend/src/routes.rs` as the SSoT
 * for sidebar structure. Update this when `routes.rs` nav items change.
 *
 * WHY this exists:
 *   Tests use these selectors directly instead of matching text labels or CSS
 *   classes, so changing a label or class in the frontend doesn't break tests.
 */
export interface NavItem {
  id: string;
  label: string;
  /** data-testid for the nav-item button. */
  testId: string;
}

export interface NavGroup {
  id: string;
  label: string;
  testId: string;
  children: NavItem[];
}

export const NAV_STANDALONE: NavItem[] = [
  { id: "series", label: "Series", testId: "nav-item-series" },
  { id: "calendar", label: "Calendar", testId: "nav-item-calendar" },
  { id: "wanted", label: "Wanted", testId: "nav-item-wanted" },
  { id: "activity", label: "Activity", testId: "nav-item-activity" },
];

export const NAV_GROUPS: NavGroup[] = [
  {
    id: "profiles",
    label: "Profiles",
    testId: "nav-group-profiles",
    children: [
      { id: "profiles/quality", label: "Quality Profiles", testId: "nav-item-profiles/quality" },
      { id: "profiles/release", label: "Release Profiles", testId: "nav-item-profiles/release" },
    ],
  },
  {
    id: "settings",
    label: "Settings",
    testId: "nav-group-settings",
    children: [
      { id: "settings/general", label: "General", testId: "nav-item-settings/general" },
      { id: "settings/organization", label: "Organization", testId: "nav-item-settings/organization" }
    ],
  },
  {
    id: "plugins",
    label: "Plugins",
    testId: "nav-group-plugins",
    children: [
      { id: "plugins/sources", label: "Sources", testId: "nav-item-plugins/sources" },
      { id: "plugins/metadata", label: "Metadata", testId: "nav-item-plugins/metadata" },
      { id: "plugins/clients", label: "Downloaders", testId: "nav-item-plugins/clients" },
      { id: "plugins/notifiers", label: "Notifiers", testId: "nav-item-plugins/notifiers" },
    ],
  },
  {
    id: "management",
    label: "Management",
    testId: "nav-group-management",
    children: [
      { id: "management/rename", label: "Rename Queue", testId: "nav-item-management/rename" },
      { id: "management/download", label: "Download Queue", testId: "nav-item-management/download" },
      { id: "management/organized", label: "Organized Series", testId: "nav-item-management/organized" },
    ],
  },
  {
    id: "authentication",
    label: "Authentication",
    testId: "nav-group-authentication",
    children: [
      { id: "authentication/account", label: "Account", testId: "nav-item-authentication/account" },
      { id: "authentication/security", label: "Security", testId: "nav-item-authentication/security" },
      { id: "authentication/api", label: "API", testId: "nav-item-authentication/api" },
    ],
  },
  {
    id: "system",
    label: "System",
    testId: "nav-group-system",
    children: [
      { id: "system/status", label: "Status", testId: "nav-item-system/status" },
      { id: "system/logs", label: "Logs", testId: "nav-item-system/logs" },
      { id: "system/about", label: "About", testId: "nav-item-system/about" },
    ],
  },
];

/** Look up a nav item by id, searching standalone then group children. */
export function findNavItem(id: string): NavItem | undefined {
  const standalone = NAV_STANDALONE.find((n) => n.id === id);
  if (standalone) return standalone;
  for (const group of NAV_GROUPS) {
    const child = group.children.find((c) => c.id === id);
    if (child) return child;
  }
  return undefined;
}

/** Look up a nav group by id. */
export function findNavGroup(id: string): NavGroup | undefined {
  return NAV_GROUPS.find((g) => g.id === id);
}
