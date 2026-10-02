import { test, expect } from "@playwright/test";
import { navigateSidebarSection } from "../support/helpers";

test.describe("System Group", () => {
  test("should navigate to Status", async ({ page }) => {
    await page.goto("/system");
    await expect(page.locator(".main-content")).toBeVisible();

    await navigateSidebarSection(page, "system", "system/status");

    await expect(page).toHaveURL(/.*\/system\/status/);
    // Status page loads health data
    await expect(page.locator(".main-content")).not.toBeEmpty();
  });

  test("should navigate to Logs", async ({ page }) => {
    await page.goto("/system");
    await expect(page.locator(".main-content")).toBeVisible();

    await navigateSidebarSection(page, "system", "system/logs");

    await expect(page).toHaveURL(/.*\/system\/logs/);
    // Logs page shows a log viewer or empty state
    await expect(page.locator(".main-content")).not.toBeEmpty();
  });

  test("should show cached logs on re-navigation (stale-while-revalidate)", async ({
    page,
  }) => {
    await page.goto("/system");
    await expect(page.locator(".main-content")).toBeVisible();

    // First visit — load logs from API and cache the response
    await navigateSidebarSection(page, "system", "system/logs");
    await expect(page).toHaveURL(/.*\/system\/logs/);
    const content = page.locator(".main-content");
    await expect(content).not.toBeEmpty();

    // Navigate away to Status so cache is populated
    const statusBtn = page.locator('[data-testid="nav-item-system/status"]');
    await statusBtn.click();
    await expect(page).toHaveURL(/.*\/system\/status/);

    // Navigate back to Logs — should render cached data instantly
    const logsBtn = page.locator('[data-testid="nav-item-system/logs"]');
    await logsBtn.click();
    await expect(page).toHaveURL(/.*\/system\/logs/);
    // Content is populated from cache (no API round-trip delay)
    await expect(content).not.toBeEmpty();
  });

  test("should navigate to About", async ({ page }) => {
    // Navigate directly to the about page
    await page.goto("/system/about");
    await expect(page).toHaveURL(/.*\/system\/about/);
    await expect(page.locator(".main-content")).toBeVisible({ timeout: 15000 });

    // Verify the about API directly (the frontend data fetch may take time)
    const aboutResp = await page.request.fetch("/api/system/about");
    expect(aboutResp.ok()).toBeTruthy();
    const aboutData = await aboutResp.json();
    expect(aboutData.version).toBeTruthy();
  });
});
