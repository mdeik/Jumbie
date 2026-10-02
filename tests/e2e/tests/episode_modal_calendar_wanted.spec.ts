import { test, expect, Page } from "@playwright/test";
import * as fs from "fs";
import * as path from "path";
import {
  backdateFile,
  createSeries,
  daysAgoMidnightUtcIso,
  makeTestDir,
} from "../support/helpers";

const SERIES_NAME = "E2E Calendar Test";

/// Fetch a single episode's view model from the series-scoped endpoint.
/// SSoT: modal data comes from `/api/series/{id}` (SeriesDetails) — the
/// per-episode `/api/episodes/{id}` endpoint no longer exists, so tests read
/// episode state from the same source the frontend uses.
async function fetchEpisode(
  page: Page,
  seriesId: string,
  episodeId: string,
): Promise<any> {
  const resp = await page.request.fetch(`/api/series/${seriesId}`);
  expect(resp.ok()).toBeTruthy();
  const details = await resp.json();
  const episode = details.episodes?.find(
    (ep: any) => ep.unique_id === episodeId,
  );
  expect(episode).toBeTruthy();
  return episode;
}

// ── Episode Modal tests ───────────────────────────────────────────────────────

test.describe("Episode Modal", () => {
  test("should not close episode modal when deleting file", async ({
    page,
  }) => {
    const testDir = makeTestDir(
      `modal_noclose_${Math.random().toString(36).slice(2, 8)}`,
    );
    try {
      await createSeries(page, SERIES_NAME, testDir);

      // Navigate to Episodes tab and open the episode detail modal
      await page.locator('[data-testid="edit-tab-episodes"]').click();
      await page.waitForSelector(".episodes-grid", { timeout: 15000 });

      const episodeCells = page.locator(".episode-cell");
      await expect(episodeCells.first()).toBeVisible({ timeout: 10000 });
      await episodeCells.first().click();

      // Modal should open
      await expect(page.locator(".modal-overlay.active")).toBeVisible({
        timeout: 5000,
      });

      // Click the episode delete button
      const deleteBtn = page
        .locator(".modal-overlay.active")
        .getByRole("button", { name: "Episode", exact: true });
      await expect(deleteBtn).toBeVisible({ timeout: 5000 });
      await deleteBtn.click();

      // Confirm deletion
      const confirmBtn = page
        .locator(".modal-overlay.active")
        .getByRole("button", { name: "Confirm" });
      await expect(confirmBtn).toBeVisible({ timeout: 5000 });
      await confirmBtn.click();

      // The episode detail modal should NOT have closed — wait for the
      // status badge to reflect deletion rather than using a hard timeout.
      await expect(
        page.locator(".modal-overlay.active .status-badge").first(),
      ).toBeVisible({ timeout: 10000 });

      // Close modal
      await page
        .locator(".modal-overlay.active")
        .click({ position: { x: 0, y: 0 } });
      await expect(page.locator(".modal-overlay.active")).not.toBeVisible({
        timeout: 5000,
      });
    } finally {
      if (testDir) fs.rmSync(testDir, { recursive: true, force: true });
    }
  });

  test("should open episode modal from Episodes tab and verify contents", async ({
    page,
  }) => {
    const testDir = makeTestDir(
      `modal_verify_${Math.random().toString(36).slice(2, 8)}`,
    );
    try {
      await createSeries(page, SERIES_NAME, testDir);

      // Navigate to Episodes tab
      await page.locator('[data-testid="edit-tab-episodes"]').click();
      await page.waitForSelector(".episodes-grid", { timeout: 15000 });

      const episodeCells = page.locator(".episode-cell");
      await expect(episodeCells.first()).toBeVisible({ timeout: 10000 });
      await episodeCells.first().click();

      // Modal should open
      await expect(page.locator(".modal-overlay.active")).toBeVisible({
        timeout: 5000,
      });

      // Wait for episode details to render inside the modal
      const statusBadge = page
        .locator(".modal-overlay.active .status-badge")
        .first();
      await expect(statusBadge).toBeVisible({ timeout: 5000 });

      // Title input should be editable
      const titleInput = page
        .locator(".modal-overlay.active .info-section")
        .filter({ hasText: "Title" })
        .locator("input");
      await expect(titleInput).toBeVisible({ timeout: 5000 });

      // Clear metadata should be present
      await expect(
        page
          .locator(".modal-overlay.active")
          .getByRole("button", { name: "Clear Metadata" }),
      ).toBeVisible({ timeout: 5000 });

      // Close modal
      await page
        .locator(".modal-overlay.active")
        .click({ position: { x: 0, y: 0 } });
      await expect(page.locator(".modal-overlay.active")).not.toBeVisible({
        timeout: 5000,
      });
    } finally {
      if (testDir) fs.rmSync(testDir, { recursive: true, force: true });
    }
  });
});

// ── Calendar background refresh ───────────────────────────────────────────────

test.describe("Calendar Background Refresh", () => {
  let testDir: string;

  test.beforeAll(() => {
    testDir = makeTestDir("calendar");
  });

  test.afterAll(() => {
    if (testDir) fs.rmSync(testDir, { recursive: true, force: true });
  });

  test("calendar reflects file deletion via background re-fetch", async ({
    page,
  }) => {
    const dummyFile = path.join(testDir, "MyShow.S01E01.mkv");
    // Anchor the fixture file in the past so the scanner records an old source
    // date and the episode is genuinely past due regardless of wall-clock time.
    backdateFile(dummyFile, 7);

    const seriesId = await createSeries(page, SERIES_NAME, testDir);

    // Navigate to edit series page to read the episode_id from the DOM
    await page.goto(`/series/${seriesId}/edit`);
    await page.locator('[data-testid="edit-tab-episodes"]').click();
    await page.waitForSelector(".episodes-grid", { timeout: 15000 });

    // Get the first episode's episode_id from the data attribute
    const firstCell = page.locator(".episode-cell").first();
    await expect(firstCell).toBeVisible({ timeout: 10000 });
    const episodeId = await firstCell.getAttribute("data-episode-id");
    expect(episodeId).not.toBeNull();
    if (!episodeId) return;

    // Set estimated_release_date to yesterday as a past-due fallback. Must be
    // RFC 3339 — the API rejects date-only input. This call also triggers
    // re-estimation, which projects from the fixture's backdated source date, so
    // the episode stays past due either way.
    const yesterdayStr = daysAgoMidnightUtcIso(1);

    const estResp = await page.request.fetch(
      `/api/episodes/${episodeId}/est_date`,
      {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        data: { date: yesterdayStr },
      },
    );
    expect(estResp.ok()).toBeTruthy();

    // Verify the episode has proper status before deletion
    const detailsBefore = await fetchEpisode(page, seriesId, episodeId);
    expect(detailsBefore.status).toBe("organized");

    // Get the current file path from the episode details
    const currentFilePath =
      (await fetchEpisode(page, seriesId, episodeId))?.path || dummyFile;

    // Delete the file via the API to simulate background download removal
    const unassignResp = await page.request.fetch(
      `/api/series/${seriesId}/files/unassign`,
      {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        data: { paths: [currentFilePath] },
      },
    );
    expect(unassignResp.ok()).toBeTruthy();

    // Verify the episode status changed to 'missing' after unassign
    const detailsAfter = await fetchEpisode(page, seriesId, episodeId);
    expect(detailsAfter.status).toBe("missing");

    // Also verify the calendar API reflects the change
    const calToday = new Date();
    const calYear = calToday.getFullYear();
    const calMonth = calToday.getMonth();
    const calPrevMonth = calMonth === 0 ? 11 : calMonth - 1;
    const calPrevYear = calMonth === 0 ? calYear - 1 : calYear;
    const calNextMonth = calMonth === 11 ? 0 : calMonth + 1;
    const calNextYear = calMonth === 11 ? calYear + 1 : calYear;
    const tzOffset = -new Date().getTimezoneOffset();
    const tzSign = tzOffset >= 0 ? "+" : "-";
    const tzHours = Math.floor(Math.abs(tzOffset) / 60);
    const tzMins = Math.abs(tzOffset) % 60;
    const pad = (n: number) => String(n).padStart(2, "0");
    const startStr = `${calPrevYear}-${pad(calPrevMonth + 1)}-01T00:00:00${tzSign}${pad(tzHours)}:${pad(tzMins)}`;
    const endStr = `${calNextYear}-${pad(calNextMonth + 1)}-01T23:59:59${tzSign}${pad(tzHours)}:${pad(tzMins)}`;

    const calResp = await (
      await page.request.fetch(
        `/api/calendar?start_date=${encodeURIComponent(startStr)}&end_date=${encodeURIComponent(endStr)}`,
      )
    ).json();

    // Find our episode in the calendar data and verify it is no longer assigned
    // (the unassign dropped its file association), which drives the calendar's
    // Downloaded/Missing status.
    const ourEpisode = calResp.episodes.find(
      (ep: any) => ep.episode_id === episodeId,
    );
    expect(ourEpisode).toBeTruthy();
    expect(ourEpisode.assigned).toBe(false);
  });
});

// ── Wanted page estimated date fallback ────────────────────────────────────

test.describe("Wanted Page Estimated Date", () => {
  let testDir: string;

  test.beforeAll(() => {
    testDir = makeTestDir("wanted_est");
  });

  test.afterAll(() => {
    if (testDir) fs.rmSync(testDir, { recursive: true, force: true });
  });

  test("wanted page shows episode with correct age after file deletion", async ({
    page,
  }) => {
    const dummyFile = path.join(testDir, "MyShow.S01E01.mkv");
    // Anchor the fixture file in the past so the scanner records an old source
    // date and the episode is genuinely past due regardless of wall-clock time.
    backdateFile(dummyFile, 7);

    const seriesId = await createSeries(page, SERIES_NAME, testDir);

    // Navigate to edit series page to read the episode_id from the DOM
    await page.goto(`/series/${seriesId}/edit`);
    await page.locator('[data-testid="edit-tab-episodes"]').click();
    await page.waitForSelector(".episodes-grid", { timeout: 15000 });

    const firstCell = page.locator(".episode-cell").first();
    await expect(firstCell).toBeVisible({ timeout: 10000 });
    const episodeId = await firstCell.getAttribute("data-episode-id");
    expect(episodeId).not.toBeNull();
    if (!episodeId) return;

    // Set estimated_release_date to yesterday as a past-due fallback. Must be
    // RFC 3339 — the API rejects date-only input. This call also triggers
    // re-estimation, which projects from the fixture's backdated source date, so
    // the episode stays past due either way.
    const yesterdayStr = daysAgoMidnightUtcIso(1);

    const estResp = await page.request.fetch(
      `/api/episodes/${episodeId}/est_date`,
      {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        data: { date: yesterdayStr },
      },
    );
    expect(estResp.ok()).toBeTruthy();

    // Get the current file path from the episode details
    const currentFilePath =
      (await fetchEpisode(page, seriesId, episodeId))?.path || dummyFile;

    // Delete the file so the episode becomes "missing" and appears in wanted
    const unassignResp = await page.request.fetch(
      `/api/series/${seriesId}/files/unassign`,
      {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        data: { paths: [currentFilePath] },
      },
    );
    expect(unassignResp.ok()).toBeTruthy();

    // Navigate to wanted page and wait for the table to load
    await page.goto("/wanted", { waitUntil: "networkidle" });
    await expect(page.locator("#wanted .table-container")).toBeVisible({
      timeout: 10000,
    });

    // Check that the page doesn't show "Not yet released" anywhere
    const bodyText = await page.evaluate(() => document.body.innerText);
    expect(bodyText).not.toContain("Not yet released");
  });
});
