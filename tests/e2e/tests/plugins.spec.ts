import { test, expect } from "@playwright/test";
import { navigateSidebarSection } from "../support/helpers";

test.describe("Plugins Group", () => {
  test("should navigate to Sources settings", async ({ page }) => {
    await page.goto("/plugins");
    await expect(page.locator(".main-content")).toBeVisible();

    await navigateSidebarSection(page, "plugins", "plugins/sources");

    await expect(page).toHaveURL(/.*\/plugins\/sources/);
    await expect(page.locator(".main-content")).toContainText("Active Sources");
  });

  test("should navigate to Downloader settings", async ({ page }) => {
    await page.goto("/plugins");
    await expect(page.locator(".main-content")).toBeVisible();

    await navigateSidebarSection(page, "plugins", "plugins/clients");

    await expect(page).toHaveURL(/.*\/plugins\/clients/);
    await expect(page.locator(".main-content")).toContainText("Active Clients");
  });

  test("should navigate to Notifier settings", async ({ page }) => {
    await page.goto("/plugins");
    await expect(page.locator(".main-content")).toBeVisible();

    await navigateSidebarSection(page, "plugins", "plugins/notifiers");

    await expect(page).toHaveURL(/.*\/plugins\/notifiers/);
    await expect(page.locator(".main-content")).toContainText(
      "Active Notifiers",
    );
  });

  test("should navigate to Metadata settings", async ({ page }) => {
    await page.goto("/plugins");
    await expect(page.locator(".main-content")).toBeVisible();

    await navigateSidebarSection(page, "plugins", "plugins/metadata");

    await expect(page).toHaveURL(/.*\/plugins\/metadata/);
    await expect(page.locator(".main-content")).toContainText("Configurations");
  });
});
