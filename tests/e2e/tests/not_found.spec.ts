import { test, expect } from "@playwright/test";

test.describe("404 Not Found Page", () => {
  test("should show error page for unknown routes", async ({ page }) => {
    await page.goto("/this-route-does-not-exist");

    // The app renders an ErrorPage component with 404 content
    await expect(page.locator(".main-content")).toContainText("Not Found", {
      timeout: 10000,
    });
  });

  test("should show error page for nested unknown routes", async ({
    page,
  }) => {
    await page.goto("/series/does-not-exist/nested");

    await expect(page.locator(".main-content")).toContainText("Not Found", {
      timeout: 10000,
    });
  });
});
