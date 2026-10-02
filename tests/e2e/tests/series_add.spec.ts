import { test, expect } from "@playwright/test";
import { createSeries, makeEmptyDir } from "../support/helpers";
import * as fs from "fs";

const SERIES_NAME = "E2E Test Mock Series";

test.describe("Add Series Flow", () => {
  let testDir: string;

  test.beforeAll(() => {
    testDir = makeEmptyDir("mock_series");
  });

  test.afterAll(() => {
    if (testDir) fs.rmSync(testDir, { recursive: true, force: true });
  });

  test("should successfully add a new mock series", async ({ page }) => {
    const seriesId = await createSeries(page, SERIES_NAME, testDir);

    // Verify the edit page header displays the series title
    await expect(page.locator(".header-title")).toContainText(
      /e2e[_ ]test[_ ]mock[_ ]series/i,
    );

    // Verify we're on the edit page with a valid series ID
    expect(seriesId).toMatch(/^[a-zA-Z0-9-]+$/);
  });
});
