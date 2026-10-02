import { expect, type Page } from "@playwright/test";
import * as fs from "fs";
import * as path from "path";
import * as crypto from "crypto";
import { findNavGroup, findNavItem } from "./nav-tree";

/**
 * Create a series via the Add Series form (Custom Path mode).
 *
 * @returns The series ID extracted from the edit-page URL.
 */
export async function createSeries(
  page: Page,
  seriesName: string,
  testDir: string,
): Promise<string> {
  await page.goto("/series/add");
  await expect(page.locator("input#seriesName")).toBeVisible({
    timeout: 15000,
  });
  await page.locator("input#seriesName").fill(seriesName);
  await page
    .locator("select#destinationRoot")
    .selectOption({ value: "__CUSTOM__" });
  await page.locator("input#customPath").fill(testDir);
  // ── Click and wait for navigation ───────────────────────────────────
  // The SPA does a full page reload on navigation (Leptos CSR behavior).
  // The toast shown before navigation is lost on reload, so we verify
  // success via the URL instead.
  // ── Create series via API directly ───────────────────────────────────
  // The SPA does a pushState navigation on success which causes WASM to
  // hang (pre-existing Leptos CSR issue).  Instead, we create the series
  // via the API directly, then navigate to the edit page fresh.
  //
  // WHY resolve_collisions: false: this helper emulates the form's Custom
  // Path mode (it selects `__CUSTOM__` above), and the UI sends
  // `resolve_collisions: !is_custom_root` — so custom paths are honored
  // verbatim. The field defaults to `true` (root-derived names), which
  // would collision-resolve the already-existing test directory (default
  // "rename" handling suffixes it) and the scanner would never find the
  // dummy episode file.
  const resp = await page.request.fetch("/api/series", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    data: {
      series_name: seriesName,
      path: testDir,
      scan_for_existing: true,
      monitor_mode: "none",
      resolve_collisions: false,
    },
  });
  expect(resp.ok()).toBeTruthy();
  const seriesId = (await resp.text()).replace(/^"/, "").replace(/"/, "");
  expect(seriesId).toMatch(/^[a-zA-Z0-9-]+$/);

  // Navigate directly to the edit page (full page load, no SPA bugs)
  await page.goto(`/series/${seriesId}/edit`, { waitUntil: "networkidle" });
  return seriesId;
}

/**
 * Create a temp directory with a dummy episode file for scanner detection.
 */
export function makeTestDir(label: string, runId?: number): string {
  const id = runId ?? Date.now();
  const dir = `/tmp/e2e_${label}_${id}`;
  fs.mkdirSync(dir, { recursive: true });
  // Content is unique per directory. Fingerprints are content-keyed (xxh3) and
  // `release_info` is shared by every file with the same content hash, so
  // identical fixture bytes across tests would collide on one row — letting one
  // test's mtime-derived source date bleed into another's fixtures.
  fs.writeFileSync(
    path.join(dir, "MyShow.S01E01.mkv"),
    `dummy video content for ${dir}`,
  );
  return dir;
}

/**
 * Create a temp directory (no files inside) for tests that don't need episodes.
 */
export function makeEmptyDir(label: string): string {
  const dir = `/tmp/e2e_test_${label}_${Date.now()}`;
  fs.mkdirSync(dir, { recursive: true });
  return dir;
}

/**
 * Unique dir name with random suffix to avoid collisions between retries.
 */
export function uniqueDir(label: string): string {
  return `/tmp/e2e_test_${label}_${crypto.randomUUID().slice(0, 8)}`;
}

/**
 * RFC 3339 UTC timestamp for N days ago at 00:00:00Z.
 *
 * The API accepts timestamps only as RFC 3339 with an explicit offset —
 * date-only strings like "2026-06-18" are rejected with 400. Tests that need a
 * past date (e.g. an estimated release date) must send a full timestamp.
 */
export function daysAgoMidnightUtcIso(days: number): string {
  const d = new Date();
  d.setDate(d.getDate() - days);
  const yyyy = d.getFullYear();
  const mm = String(d.getMonth() + 1).padStart(2, "0");
  const dd = String(d.getDate()).padStart(2, "0");
  return `${yyyy}-${mm}-${dd}T00:00:00Z`;
}

/**
 * Backdate a fixture file's access and modification times by `days`.
 *
 * The scanner records a file's mtime as its source (upload) date fallback, and
 * the release estimator anchors its estimate on that date. A file created "now"
 * therefore projects an estimate of today 12:00 UTC, which reads as
 * "unreleased" until noon UTC. Backdating the mtime makes the episode genuinely
 * past due regardless of the wall-clock time the test runs, so status-derived
 * assertions stay deterministic.
 */
export function backdateFile(filePath: string, days: number): void {
  const t = new Date(Date.now() - days * 24 * 60 * 60 * 1000);
  fs.utimesSync(filePath, t, t);
}

/**
 * Navigate to a sidebar sub-section by expanding the parent group if needed
 * and clicking the child button.
 *
 * Uses `data-testid` attributes (`nav-group-{id}` for group buttons,
 * `nav-item-{id}` for child buttons) so tests are decoupled from text labels
 * and CSS classes. The nav tree definition in `nav-tree.ts` is the SSoT.
 *
 * @param page    Playwright page object.
 * @param groupId The nav-group id (e.g. "settings", "system", "profiles").
 * @param childId The child nav-item id (e.g. "settings/general", "system/about").
 */
export async function navigateSidebarSection(
  page: Page,
  groupId: string,
  childId: string,
) {
  const group = findNavGroup(groupId);
  if (!group) throw new Error(`Unknown nav group: ${groupId}`);
  const child = findNavItem(childId);
  if (!child) throw new Error(`Unknown nav item: ${childId}`);

  const childBtn = page.locator(`[data-testid="${child.testId}"]`);
  // Wait for the sidebar to settle first
  await expect(
    page.locator(`[data-testid="${group.testId}"]`),
  ).toBeVisible({ timeout: 10_000 });
  if (!(await childBtn.isVisible())) {
    await page.locator(`[data-testid="${group.testId}"]`).click();
  }
  await expect(childBtn).toBeVisible({ timeout: 10_000 });
  await childBtn.click();
}
