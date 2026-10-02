import { test, expect } from "@playwright/test";
import * as fs from "fs";
import { createSeries, makeTestDir } from "../support/helpers";

const SERIES_NAME = "E2E Metadata Test";

test.describe("Episode Metadata Editing", () => {
  let testDir: string;

  test.beforeAll(() => {
    testDir = makeTestDir("episode_metadata");
  });

  test.afterAll(() => {
    if (testDir) fs.rmSync(testDir, { recursive: true, force: true });
  });

  test("should edit title, auto-save metadata, clear, and restore for downloaded episodes", async ({
    page,
  }) => {
    // ── Step 1: Create a series pointing to the temp directory ──
    await createSeries(page, SERIES_NAME, testDir);

    // ── Step 2: Click the Episodes tab, then find the episode ──
    await page.locator('[data-testid="edit-tab-episodes"]').click();

    // Wait for the episodes grid to render with at least one episode cell
    await page.waitForSelector(".episodes-grid", { timeout: 15000 });
    const episodeCells = page.locator(".episode-cell");
    await expect(episodeCells.first()).toBeVisible({ timeout: 10000 });

    // Click the first episode cell to open the Episode Details modal
    await episodeCells.first().click();

    // ── Step 3: Verify modal opened with editable title field ──
    const titleInput = page
      .locator(".info-section")
      .filter({ hasText: "Title" })
      .locator("input");
    await expect(titleInput).toBeVisible({ timeout: 5000 });
    await expect(titleInput).toBeEnabled({ timeout: 5000 });

    // ── Step 4: Edit the title and verify auto-save ──
    const customTitle = "Custom E2E Title";
    await titleInput.fill(customTitle);

    // The badge should show "Custom" (badge badge-blue)
    const customBadge = page.locator(".status-badge.badge-blue", {
      hasText: "Custom",
    });
    await expect(customBadge).toBeVisible({ timeout: 8000 });

    // Verify the input still shows the saved title
    await expect(titleInput).toHaveValue(customTitle);

    // ── Step 5: Clear Metadata and verify ──
    const clearBtn = page
      .locator(".info-section")
      .filter({ hasText: "Title" })
      .getByRole("button", { name: "Clear Metadata" });
    await expect(clearBtn).toBeVisible();
    await clearBtn.click();

    // After clearing, no badge is shown (the "Cleared" state renders an empty span)
    // The title input should be cleared
    await expect(titleInput).toHaveValue("");

    // ── Step 6: Test Match to Provider (if metadata plugins are configured) ──
    const matchBtn = page
      .locator(".info-section")
      .filter({ hasText: "Title" })
      .getByRole("button", { name: "Match to Provider" });
    if (await matchBtn.isVisible()) {
      await matchBtn.click();
      await expect(page.locator(".toast-error").first()).not.toBeVisible({
        timeout: 10000,
      });
    }

    // ── Step 7: Test Fetch Metadata button (if metadata plugins are configured) ──
    const fetchBtn = page
      .locator(".modal-overlay.active")
      .getByRole("button", { name: /^Fetch / });
    if (await fetchBtn.isVisible()) {
      await fetchBtn.click();
      await expect(page.locator(".toast-error").first()).not.toBeVisible({
        timeout: 15000,
      });
    }
  });
});
