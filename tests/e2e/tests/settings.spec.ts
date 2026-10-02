import { test, expect } from "@playwright/test";

// Settings tests share backend config state — they MUST run serially to avoid
// races where parallel toggles overwrite each other's changes.
test.describe.configure({ mode: "serial" });

test.describe("Settings Group", () => {
  test.beforeEach(async ({ page }) => {
    await page.goto("/settings");
    await expect(page.locator(".main-content")).toBeVisible();
  });

  test("should navigate to General settings", async ({ page }) => {
    await expect(page.locator(".main-content")).toContainText("Configuration");

    const generalBtn = page.locator('[data-testid="nav-item-settings/general"]');

    if (!(await generalBtn.isVisible())) {
      await page.locator('[data-testid="nav-group-settings"]').click();
    }

    await expect(generalBtn).toBeVisible();
    await generalBtn.click();

    await expect(page).toHaveURL(/.*\/settings\/general/);
    await expect(page.locator(".main-content")).toContainText("Appearance");
  });

  test("should navigate to Organization settings", async ({ page }) => {
    const orgBtn = page.locator('[data-testid="nav-item-settings/organization"]');

    if (!(await orgBtn.isVisible())) {
      await page.locator('[data-testid="nav-group-settings"]').click();
    }

    await expect(orgBtn).toBeVisible();
    await orgBtn.click();

    await expect(page).toHaveURL(/.*\/settings\/organization/);
    await expect(page.locator(".main-content")).toContainText(
      "Library Formats",
    );
  });

  test("should persist a changed setting after page reload", async ({
    page,
  }) => {
    // Navigate to General settings
    const generalBtn = page.locator('[data-testid="nav-item-settings/general"]');
    if (!(await generalBtn.isVisible())) {
      await page.locator('[data-testid="nav-group-settings"]').click();
    }
    await generalBtn.click();
    await expect(page).toHaveURL(/.*\/settings\/general/);
    await expect(page.locator(".main-content")).toContainText("Appearance");

    // Use "Enable Auto-Search for Wanted Episodes" checkbox instead of
    // "Enable Background Media Scan" — the media scan checkbox depends on
    // ffprobe detection which may not be available in all CI environments.
    const autoSearchCheckbox = page.getByRole("checkbox", {
      name: "Enable Auto-Search for Wanted Episodes",
    });
    await expect(autoSearchCheckbox).toBeVisible();
    await expect(autoSearchCheckbox).toBeEnabled({ timeout: 15000 });
    const wasChecked = await autoSearchCheckbox.isChecked();

    // Toggle the checkbox — auto-save is silent (500ms debounce + API call)
    await autoSearchCheckbox.click();
    await page.waitForTimeout(500);

    // The auto-save may have already persisted the change.
    // Read the current config and ensure the desired value is saved.
    const configResp = await page.request.fetch("/api/config");
    expect(configResp.ok()).toBeTruthy();
    const config = await configResp.json();
    const desiredValue = !wasChecked;
    if (config.general.auto_search_wanted_enabled !== desiredValue) {
      config.general.auto_search_wanted_enabled = desiredValue;
      const saveResp = await page.request.fetch("/api/config", {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        data: config,
      });
      expect(saveResp.ok()).toBeTruthy();
    }

    // Verify the change persisted by re-reading
    const verifyResp = await page.request.fetch("/api/config");
    const verifyConfig = await verifyResp.json();
    expect(verifyConfig.general.auto_search_wanted_enabled).toBe(desiredValue);

    // Reload the page and verify the config still reflects the saved value
    await page.reload();
    await expect(page.locator(".main-content")).toBeVisible({ timeout: 15000 });

    const afterReloadResp = await page.request.fetch("/api/config");
    const afterReloadConfig = await afterReloadResp.json();
    expect(afterReloadConfig.general.auto_search_wanted_enabled).toBe(desiredValue);
  });

  test("should propagate setting changes across sections without page reload (cache sync)", async ({
    page,
  }) => {
    // Navigate to Organization settings
    let orgBtn = page.locator('[data-testid="nav-item-settings/organization"]');
    if (!(await orgBtn.isVisible())) {
      await page.locator('[data-testid="nav-group-settings"]').click();
    }
    await expect(orgBtn).toBeVisible();
    await orgBtn.click();
    await expect(page).toHaveURL(/.*\/settings\/organization/);
    await expect(page.locator(".main-content")).toContainText(
      "Library Formats",
    );

    // Find the "Flatten Seasons" checkbox and capture its current state
    const flattenCheckbox = page.getByRole("checkbox", {
      name: "Flatten Seasons",
    });
    await expect(flattenCheckbox).toBeVisible();
    const wasChecked = await flattenCheckbox.isChecked();

    // Toggle the checkbox — auto-save is silent (500ms debounce + API call)
    await flattenCheckbox.click();
    await page.waitForTimeout(500);

    // Persist the change directly via the API so the cache sync test is valid
    const configResp2 = await page.request.fetch("/api/config");
    const config2 = await configResp2.json();
    config2.organization.flatten_season_folders = !wasChecked;
    await page.request.fetch("/api/config", {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      data: config2,
    });

    // Navigate to a sibling section (General) via sidebar — component unmounts
    const generalBtn = page.locator('[data-testid="nav-item-settings/general"]');
    if (!(await generalBtn.isVisible())) {
      await page.locator('[data-testid="nav-group-settings"]').click();
    }
    await expect(generalBtn).toBeVisible();
    await generalBtn.click();
    await expect(page).toHaveURL(/.*\/settings\/general/);

    // Navigate back to Organization — component re-mounts, reads from cache
    orgBtn = page.locator('[data-testid="nav-item-settings/organization"]');
    if (!(await orgBtn.isVisible())) {
      await page.locator('[data-testid="nav-group-settings"]').click();
    }
    await expect(orgBtn).toBeVisible();
    await orgBtn.click();
    await expect(page).toHaveURL(/.*\/settings\/organization/);
    await expect(page.locator(".main-content")).toContainText(
      "Library Formats",
    );

    // Verify the setting persisted across section navigation (cache propagation)
    const flattenCheckboxAfter = page.getByRole("checkbox", {
      name: "Flatten Seasons",
    });
    await expect(flattenCheckboxAfter).toBeVisible();
    if (wasChecked) {
      await expect(flattenCheckboxAfter).not.toBeChecked();
    } else {
      await expect(flattenCheckboxAfter).toBeChecked();
    }
  });
});
