import { test, expect } from "@playwright/test";
import { navigateSidebarSection } from "../support/helpers";

test.describe("Authentication Group", () => {
  test("should navigate to Account settings", async ({ page }) => {
    await page.goto("/authentication");
    await expect(page.locator(".main-content")).toBeVisible();

    await navigateSidebarSection(page, "authentication", "authentication/account");

    await expect(page).toHaveURL(/.*\/authentication\/account/);
    await expect(page.locator(".main-content")).toContainText("Credentials");
  });

  test("should navigate to Security settings", async ({ page }) => {
    await page.goto("/authentication");
    await expect(page.locator(".main-content")).toBeVisible();

    await navigateSidebarSection(page, "authentication", "authentication/security");

    await expect(page).toHaveURL(/.*\/authentication\/security/);
    await expect(page.locator(".main-content")).toContainText("Access Control");
  });

  test("should navigate to API settings", async ({ page }) => {
    await page.goto("/authentication");
    await expect(page.locator(".main-content")).toBeVisible();

    await navigateSidebarSection(page, "authentication", "authentication/api");

    await expect(page).toHaveURL(/.*\/authentication\/api/);
    await expect(page.locator(".main-content")).toContainText("API Keys");
  });
});
