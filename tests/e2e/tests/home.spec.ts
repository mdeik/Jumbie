import { test, expect } from "@playwright/test";

test.describe("Jumbie App", () => {
  test("should load the initial page and render sidebar", async ({ page }) => {
    // Navigate to the root URL
    await page.goto("/");

    // Verify that the document title is set
    await expect(page).toHaveTitle(/Jumbie/);

    // Wait for the app to mount and render the sidebar
    const sidebar = page.locator("aside.sidebar");
    await expect(sidebar).toBeVisible({ timeout: 10000 });

    // Verify the sidebar has the expected navigation buttons using
    // data-testid so tests are decoupled from text labels and CSS classes.
    await expect(
      page.locator('[data-testid="nav-item-series"]'),
    ).toBeVisible();
    await expect(
      page.locator('[data-testid="nav-item-calendar"]'),
    ).toBeVisible();
    await expect(
      page.locator('[data-testid="nav-item-wanted"]'),
    ).toBeVisible();
    await expect(
      page.locator('[data-testid="nav-item-activity"]'),
    ).toBeVisible();

    // Verify the app redirected to /series
    await expect(page).toHaveURL(/.*\/series/);
  });
});
