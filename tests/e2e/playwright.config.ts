import { defineConfig, devices } from "@playwright/test";

export default defineConfig({
  testDir: "./tests",
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 2 : 0,
  workers: process.env.CI ? 1 : undefined,
  reporter: "html",
  use: {
    baseURL: "http://127.0.0.1:3001",
    trace: "on-first-retry",
  },
  // 10s default gives the WASM SPA time to compile in slow containers
  // without being painful on real failures. Playwright's stock 5s was tight for
  // debug WASM on single-threaded CI runners. CI gets more headroom: it builds
  // a fresh release wasm (trunk build --release) but still runs single-threaded.
  expect: { timeout: process.env.CI ? 30_000 : 10_000 },
  // CI retries flaky tests (see retries above) and runs single-threaded, so
  // give each test enough wall-clock room for a slow runner.
  timeout: process.env.CI ? 90_000 : 30_000,
  projects: [
    {
      name: "chromium",
      use: { ...devices["Desktop Chrome"] },
    },
  ],
  webServer: {
    // The runner has no session D-Bus, so on Linux the StatusNotifierItem tray
    // fails to connect and the server falls back to headless automatically.
    // DISPLAY/WAYLAND_DISPLAY are unset defensively for the non-Linux backends.
    command: "env -u DISPLAY -u WAYLAND_DISPLAY JUMBIE_PORT=3001 ./target/debug/jumbie",
    cwd: "../../", // Run cargo command from the workspace root
    url: "http://127.0.0.1:3001/api/public/ping",
    reuseExistingServer: !process.env.CI,
    env: {
      // Store DB, config, and logs in a temporary location so we don't pollute the dev environment
      JUMBIE_DATABASE: "./tests/e2e/.tmp/jumbie.db",
      JUMBIE_CONFIG: "./tests/e2e/.tmp/config.toml",
      JUMBIE_TMP_DIR: "./tests/e2e/.tmp/tmp",
      JUMBIE_LOGS_DIR: "./tests/e2e/.tmp/logs",
    },
    stdout: "pipe",
    stderr: "pipe",
    timeout: 120 * 1000,
  },
});
