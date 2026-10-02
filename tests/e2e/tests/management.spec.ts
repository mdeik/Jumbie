import { test, expect } from "@playwright/test";
import { navigateSidebarSection } from "../support/helpers";

test.describe("Management Group", () => {
  test("should navigate to Rename Queue", async ({ page }) => {
    await page.goto("/management");
    await expect(page.locator(".main-content")).toBeVisible();

    await navigateSidebarSection(page, "management", "management/rename");

    await expect(page).toHaveURL(/.*\/management\/rename/);
    // The rename queue should have an empty state or a table
    await expect(page.locator(".main-content")).not.toBeEmpty();
  });

  test("should navigate to Download Queue", async ({ page }) => {
    await page.goto("/management");
    await expect(page.locator(".main-content")).toBeVisible();

    await navigateSidebarSection(page, "management", "management/download");

    await expect(page).toHaveURL(/.*\/management\/download/);
    // The download queue page renders a heading or empty state
    await expect(page.locator(".main-content")).not.toBeEmpty();
  });

  test("should navigate to Organized Series", async ({ page }) => {
    await page.goto("/management");
    await expect(page.locator(".main-content")).toBeVisible();

    await navigateSidebarSection(page, "management", "management/organized");

    await expect(page).toHaveURL(/.*\/management\/organized/);
    await expect(page.locator(".main-content")).toContainText(
      "Destination Roots",
    );
  });
});
