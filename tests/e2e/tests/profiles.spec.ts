import { test, expect, type Page } from "@playwright/test";
import { navigateSidebarSection } from "../support/helpers";

/** Helper: ensure we're on the Quality Profiles sub-tab */
async function ensureQualityProfiles(page: Page) {
  await page.goto("/profiles");
  await expect(page.locator(".main-content")).toBeVisible();

  await navigateSidebarSection(page, "profiles", "profiles/quality");
  await expect(page).toHaveURL(/.*\/profiles\/quality/);
}

/** Helper: ensure we're on the Release Profiles sub-tab */
async function ensureReleaseProfiles(page: Page) {
  await page.goto("/profiles");
  await expect(page.locator("body")).not.toBeEmpty({ timeout: 10000 });
  await expect(page.locator(".main-content")).toBeVisible({ timeout: 10000 });

  await navigateSidebarSection(page, "profiles", "profiles/release");
  await expect(page).toHaveURL(/.*\/profiles\/release/);
}

test.describe("Profiles Group", () => {
  // Profiles tests share backend config state — they MUST run serially.
  test.describe.configure({ mode: "serial" });

  test("should navigate to Quality Profiles", async ({ page }) => {
    await ensureQualityProfiles(page);
    await expect(page.locator(".main-content")).toContainText(
      "Quality Profiles",
    );
  });

  test("should cycle quality tristate through off / normal / upgrade target", async ({
    page,
  }) => {
    await ensureQualityProfiles(page);

    // Open the edit modal by adding a new profile
    await page
      .getByRole("button", { name: "Add Profile", exact: true })
      .click();
    await expect(page.getByText("Edit Quality Profile")).toBeVisible();

    // Find the first quality button (should start as "off" — □)
    const firstBtn = page.locator(".quality-btn").first();
    await expect(firstBtn).toBeVisible();

    // State 0: off — button should have quality-btn-off class
    await expect(firstBtn).toHaveClass(/quality-btn-off/);

    // Click: off → normal (✓)
    await firstBtn.click();
    await expect(firstBtn).toHaveClass(/quality-btn-on/);

    // Click: normal → upgrade target (⬆)
    await firstBtn.click();
    await expect(firstBtn).toHaveClass(/quality-btn-upgrade/);

    // Click: upgrade target → off (□)
    await firstBtn.click();
    await expect(firstBtn).toHaveClass(/quality-btn-off/);

    // Close the modal
    await page.getByRole("button", { name: "Close" }).click();
    await expect(page.getByText("Edit Quality Profile")).not.toBeVisible();
  });

  test("should save and reopen a profile with upgrade target", async ({
    page,
  }) => {
    await ensureQualityProfiles(page);

    // Open the edit modal by adding a new profile
    await page
      .getByRole("button", { name: "Add Profile", exact: true })
      .click();
    await expect(page.getByText("Edit Quality Profile")).toBeVisible();

    // Read the auto-generated profile name
    const nameInput = page.locator("#qualityProfileName");
    const profileName = await nameInput.inputValue();

    // Find the first quality button and cycle it to upgrade target
    await page.locator(".quality-btn").first().click();
    await expect(page.locator(".quality-btn").first()).toHaveClass(
      /quality-btn-on/,
    );
    await page.locator(".quality-btn").first().click();
    await expect(page.locator(".quality-btn").first()).toHaveClass(
      /quality-btn-upgrade/,
    );

    // Save the profile
    await page.getByRole("button", { name: "Save", exact: true }).click();
    await expect(page.getByText("Edit Quality Profile")).not.toBeVisible();

    // Reopen the profile by clicking its card — match by name substring
    await page
      .locator(".table-container")
      .getByText(profileName)
      .first()
      .click();
    await expect(page.getByText("Edit Quality Profile")).toBeVisible();

    // Verify the quality is still marked as upgrade target
    await expect(page.locator(".quality-btn").first()).toHaveClass(
      /quality-btn-upgrade/,
    );

    // Close without saving
    await page.getByRole("button", { name: "Close" }).click();
    await expect(page.getByText("Edit Quality Profile")).not.toBeVisible();
  });

  test("should navigate to Release Profiles", async ({ page }) => {
    await page.goto("/profiles");
    await expect(page.locator(".main-content")).toBeVisible();

    await navigateSidebarSection(page, "profiles", "profiles/release");

    await expect(page).toHaveURL(/.*\/profiles\/release/);
    await expect(page.locator(".main-content")).toContainText(
      "Release Profiles",
    );
  });

  test("automatic profiles section is rendered", async ({ page }) => {
    await ensureReleaseProfiles(page);

    // Verify the section title
    await expect(
      page.getByRole("heading", { name: "Automatic Profiles" }),
    ).toBeVisible();

    // Verify buttons exist
    await expect(
      page.getByRole("button", { name: "Recalculate" }),
    ).toBeVisible();
    await expect(page.getByRole("button", { name: "Configure" })).toBeVisible();

    // Verify the enable checkbox exists
    await expect(page.locator("#cb-auto-profiles")).toBeVisible();

    // Verify the table shows empty state
    await expect(
      page.getByText("No automatic profiles defined.").first(),
    ).toBeVisible();
  });

  test("recalculate scores triggers API call (no error toast)", async ({
    page,
  }) => {
    await ensureReleaseProfiles(page);

    // Click the recalculate button
    const recalcBtn = page.getByRole("button", { name: "Recalculate" });
    await expect(recalcBtn).toBeVisible();
    await recalcBtn.click();

    // Wait briefly for the API call to process, then verify no error toast
    await expect(page.locator(".toast-error").first()).not.toBeVisible({
      timeout: 15000,
    });
  });

  test("configure modal opens and shows category form", async ({ page }) => {
    await ensureReleaseProfiles(page);

    // Open the configure modal
    await page.getByRole("button", { name: "Configure" }).click();

    // Verify the modal title
    await expect(
      page.getByText("Automatic Profiles Configuration"),
    ).toBeVisible();

    // Verify the "Add New Category" section is visible
    await expect(page.getByText("Add New Category")).toBeVisible();

    // Verify form fields exist
    await expect(page.locator("#new-cat-name")).toBeVisible();
    await expect(page.locator("#new-cat-rule")).toBeVisible();
    await expect(page.locator("#new-cat-modifier")).toBeVisible();
    await expect(page.locator("#new-cat-max-modifier")).toBeVisible();

    // Verify the Add Category button exists
    await expect(
      page.getByRole("button", { name: "Add Category" }),
    ).toBeVisible();

    // Close the modal
    await page.getByRole("button", { name: "Close" }).click();
    await expect(
      page.getByText("Automatic Profiles Configuration"),
    ).not.toBeVisible();
  });

  test("enable automatic profiles checkbox toggles (no error)", async ({
    page,
  }) => {
    await ensureReleaseProfiles(page);

    const checkbox = page.locator("#cb-auto-profiles");
    await expect(checkbox).toBeVisible();

    // Toggle it on and verify the checkbox state changes
    await checkbox.check();
    await expect(checkbox).toBeChecked();
  });

  test("automatic profiles table displays submitters and scores", async ({
    page,
  }) => {
    await ensureReleaseProfiles(page);

    // Verify the table columns exist in the rendered output
    await expect(page.getByText("Submitter").first()).toBeVisible();
    await expect(page.getByText("Score").first()).toBeVisible();

    // The table either shows "No automatic profiles defined." when empty
    // or lists profiles when the backend has data.
    // Both states are valid — we just verify the UI renders without errors.
    const emptyState = page.getByText("No automatic profiles defined.").first();
    const hasProfiles = page.locator(".table-container table tbody tr").count();

    if (await emptyState.isVisible()) {
      // Empty state is rendered correctly
      await expect(emptyState).toBeVisible();
    } else {
      // Profiles exist — verify score formatting
      const rowCount = await hasProfiles;
      expect(rowCount).toBeGreaterThan(0);

      // Each row should have a submitter name and a score value
      const firstSubmitter = page
        .locator(".table-container table tbody tr td")
        .first();
      await expect(firstSubmitter).toBeVisible();

      // Score modifier column should contain a number
      const scoreCell = page
        .locator(".table-container table tbody tr td.text-danger")
        .first();
      await expect(scoreCell).toBeVisible();
      const scoreText = await scoreCell.textContent();
      expect(scoreText).not.toBeNull();
      // Score should be a negative number (penalty)
      expect(scoreText!.startsWith("-")).toBeTruthy();
    }
  });
});
