import { test, expect } from "@playwright/test";

test.describe("Sidebar Navigation \u2014 Standalone Items", () => {
  test.beforeEach(async ({ page }) => {
    await page.goto("/");
    await expect(page.locator("aside.sidebar")).toBeVisible();
  });

  test("should navigate to Series library", async ({ page }) => {
    const seriesBtn = page.getByRole("button", { name: "Series", exact: true });
    await expect(seriesBtn).toBeVisible();
    await seriesBtn.click();

    await expect(page).toHaveURL(/.*\/series/);
    await expect(page.locator(".main-content")).toContainText("Series Library");
  });

  test("should navigate to Calendar", async ({ page }) => {
    const calendarBtn = page.getByRole("button", {
      name: "Calendar",
      exact: true,
    });
    await expect(calendarBtn).toBeVisible();
    await calendarBtn.click();

    await expect(page).toHaveURL(/.*\/calendar/);
    await expect(page.locator(".main-content")).toContainText("Calendar");
  });

  test("should navigate to Wanted", async ({ page }) => {
    const wantedBtn = page.getByRole("button", { name: "Wanted", exact: true });
    await expect(wantedBtn).toBeVisible();
    await wantedBtn.click();

    await expect(page).toHaveURL(/.*\/wanted/);
    await expect(page.locator(".main-content")).toContainText("Wanted");
  });

  test("should navigate to Activity", async ({ page }) => {
    const activityBtn = page.getByRole("button", {
      name: "Activity",
      exact: true,
    });
    await expect(activityBtn).toBeVisible();
    await activityBtn.click();

    await expect(page).toHaveURL(/.*\/activity/);
    await expect(page.locator(".main-content")).toContainText("Activity");
  });
});
