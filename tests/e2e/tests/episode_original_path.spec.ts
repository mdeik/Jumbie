import { test, expect, Page } from "@playwright/test";
import * as fs from "fs";
import * as path from "path";
import { createSeries, makeTestDir } from "../support/helpers";

const SERIES_NAME = "E2E Original Path";

/// Read a single episode's view model from the series-scoped endpoint (the SSoT
/// the modal renders from).
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

/// Create a series with a unique-content dummy file so the fingerprint (and the
/// content-keyed origin) is not shared with another spec's file.
async function createUniqueSeries(
  page: Page,
  label: string,
): Promise<{ seriesId: string; testDir: string }> {
  const testDir = makeTestDir(
    `${label}_${Math.random().toString(36).slice(2, 8)}`,
  );
  fs.writeFileSync(
    path.join(testDir, "MyShow.S01E01.mkv"),
    `original-path-${testDir}`,
  );
  const seriesId = await createSeries(page, SERIES_NAME, testDir);
  return { seriesId, testDir };
}

async function openEpisodeModal(
  page: Page,
  seriesId: string,
  episodeId: string,
): Promise<void> {
  await page.goto(`/series/${seriesId}/edit`, { waitUntil: "networkidle" });
  await page.locator('[data-testid="edit-tab-episodes"]').click();
  await page.waitForSelector(".episodes-grid", { timeout: 15000 });
  const cell = page.locator(`.episode-cell[data-episode-id="${episodeId}"]`);
  await expect(cell).toBeVisible({ timeout: 10000 });
  await cell.click();
  await expect(page.locator(".modal-overlay.active")).toBeVisible({
    timeout: 5000,
  });
}

const filePathRow = (page: Page) =>
  page.locator(
    '.modal-overlay.active .info-section:has(.info-label:text-is("File Path"))',
  );
const originalPathRow = (page: Page) =>
  page.locator(
    '.modal-overlay.active .info-section:has(.info-label:text-is("Original Path"))',
  );

test.describe("Episode original path", () => {
  test("File Path is not rendered when the episode has no file", async ({
    page,
  }) => {
    const { seriesId, testDir } = await createUniqueSeries(page, "origpath_nf");
    try {
      await page.goto(`/series/${seriesId}/edit`);
      await page.locator('[data-testid="edit-tab-episodes"]').click();
      await page.waitForSelector(".episodes-grid", { timeout: 15000 });
      const firstCell = page.locator(".episode-cell").first();
      await expect(firstCell).toBeVisible({ timeout: 10000 });
      const episodeId = await firstCell.getAttribute("data-episode-id");
      if (!episodeId) throw new Error("no episode id");

      const before = await fetchEpisode(page, seriesId, episodeId);
      const originalPath: string = before.path;

      const unassignResp = await page.request.fetch(
        `/api/series/${seriesId}/files/unassign`,
        {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          data: { paths: [originalPath] },
        },
      );
      expect(unassignResp.ok()).toBeTruthy();
      expect((await fetchEpisode(page, seriesId, episodeId)).path ?? null).toBeNull();

      await openEpisodeModal(page, seriesId, episodeId);
      // No current path → the row must not render at all, even though an origin
      // is retained server-side.
      await expect(filePathRow(page)).toHaveCount(0);
      await expect(originalPathRow(page)).toHaveCount(0);
    } finally {
      fs.rmSync(testDir, { recursive: true, force: true });
    }
  });

  test("File Path reveals the original path after the file is moved", async ({
    page,
  }) => {
    const { seriesId, testDir } = await createUniqueSeries(page, "origpath_mv");
    try {
      await page.goto(`/series/${seriesId}/edit`);
      await page.locator('[data-testid="edit-tab-episodes"]').click();
      await page.waitForSelector(".episodes-grid", { timeout: 15000 });
      const firstCell = page.locator(".episode-cell").first();
      await expect(firstCell).toBeVisible({ timeout: 10000 });
      const episodeId = await firstCell.getAttribute("data-episode-id");
      if (!episodeId) throw new Error("no episode id");

      const originalPath: string = (await fetchEpisode(page, seriesId, episodeId)).path;
      expect(originalPath).toBeTruthy();

      // Detach, move the file, then re-assign it at the new path. The origin
      // anchor is set-once, so the original path must not move.
      const unassignResp = await page.request.fetch(
        `/api/series/${seriesId}/files/unassign`,
        {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          data: { paths: [originalPath] },
        },
      );
      expect(unassignResp.ok()).toBeTruthy();

      const newPath = path.join(testDir, "MyShow.S01E01.v2.mkv");
      fs.renameSync(originalPath, newPath);

      const assignResp = await page.request.fetch(
        `/api/series/${seriesId}/files/assign`,
        {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          data: { path: newPath, season: "1", episode: "1" },
        },
      );
      expect(assignResp.ok()).toBeTruthy();

      const after = await fetchEpisode(page, seriesId, episodeId);
      expect(after.path).toBeTruthy();
      // Assignment organizes the file; the origin anchor is set-once so the
      // original path must not move.
      expect(after.original_path).toBe(originalPath);
      expect(after.path).not.toBe(originalPath);

      await openEpisodeModal(page, seriesId, episodeId);
      await expect(filePathRow(page).locator(".info-value")).toHaveText(
        after.path,
      );

      await filePathRow(page).click();
      await expect(originalPathRow(page).locator(".info-value")).toHaveText(
        originalPath,
      );

      await originalPathRow(page).click();
      await expect(filePathRow(page).locator(".info-value")).toHaveText(
        after.path,
      );
    } finally {
      fs.rmSync(testDir, { recursive: true, force: true });
    }
  });

  test("File Path has no origin toggle when no distinct original path is recorded", async ({
    page,
  }) => {
    const { seriesId, testDir } = await createUniqueSeries(page, "origpath_same");
    try {
      await page.goto(`/series/${seriesId}/edit`);
      await page.locator('[data-testid="edit-tab-episodes"]').click();
      await page.waitForSelector(".episodes-grid", { timeout: 15000 });
      const firstCell = page.locator(".episode-cell").first();
      await expect(firstCell).toBeVisible({ timeout: 10000 });
      const episodeId = await firstCell.getAttribute("data-episode-id");
      if (!episodeId) throw new Error("no episode id");

      // Freshly scanned and never moved: the episode's content origin is either
      // absent or identical to the current path, so there is nothing distinct to
      // reveal — the modal must not offer an "Original Path" toggle.
      const episode = await fetchEpisode(page, seriesId, episodeId);
      expect(episode.path).toBeTruthy();
      expect(
        episode.original_path == null || episode.original_path === episode.path,
      ).toBe(true);

      await openEpisodeModal(page, seriesId, episodeId);
      await expect(filePathRow(page).locator(".info-value")).toHaveText(
        episode.path,
      );
      // No Original Path row, and clicking the File Path row must not create one.
      await expect(originalPathRow(page)).toHaveCount(0);
      await filePathRow(page).click();
      await expect(originalPathRow(page)).toHaveCount(0);
      await expect(filePathRow(page).locator(".info-value")).toHaveText(
        episode.path,
      );
    } finally {
      fs.rmSync(testDir, { recursive: true, force: true });
    }
  });
});
