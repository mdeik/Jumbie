import { test, expect } from "@playwright/test";
import { createSeries, uniqueDir } from "../support/helpers";
import * as fs from "fs";

test.describe("Aliases & Patterns", () => {
  test("should persist aliases and patterns through save and reload", async ({
    page,
  }) => {
    const seriesName = "E2E Persist Alias";
    const testDir = uniqueDir("persist");
    fs.mkdirSync(testDir, { recursive: true });

    try {
      await createSeries(page, seriesName, testDir);

      // Switch to the Advanced tab
      await page.locator('[data-testid="edit-tab-advanced"]').click();

      // Fill in aliases
      const aliasTextarea = page.locator("textarea#seriesAliases");
      await expect(aliasTextarea).toBeVisible();
      await aliasTextarea.fill("First Alias\nSecond Alias\nThird Alias");

      // Fill in regex patterns
      const patternTextarea = page.locator("textarea#seriesRegexPatterns");
      await expect(patternTextarea).toBeVisible();
      await patternTextarea.fill("(?i)pattern.*one\n^Exact Match$");

      // Navigate away and back to verify persistence without page reload
      await page.locator('[data-testid="edit-tab-general"]').click();
      await expect(aliasTextarea).not.toBeVisible();
      await page.locator('[data-testid="edit-tab-advanced"]').click();

      await expect(aliasTextarea).toHaveValue(
        "First Alias\nSecond Alias\nThird Alias",
      );
      await expect(patternTextarea).toHaveValue(
        "(?i)pattern.*one\n^Exact Match$",
      );
    } finally {
      fs.rmSync(testDir, { recursive: true, force: true });
    }
  });
});
