#!/usr/bin/env node

/**
 * Generate brand icons by rendering the real .brand-icon CSS in a headless
 * browser. This produces a pixel-perfect match with what the browser shows,
 * including antialiasing, font resolution, and box-shadow.
 *
 * Usage:
 *   node scripts/generate_icons.mjs                          # dark mode (default)
 *   node scripts/generate_icons.mjs --theme light            # light mode only
 *   node scripts/generate_icons.mjs --theme both             # both themes
 *   node scripts/generate_icons.mjs --font /path/to/font.ttf
 */

import { chromium } from "playwright";
import path from "node:path";
import fs from "node:fs";
import { fileURLToPath } from "node:url";

// ---------------------------------------------------------------------------
// Paths
// ---------------------------------------------------------------------------

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const ROOT = path.resolve(__dirname, "..");
const STYLE_CSS = path.join(ROOT, "frontend", "style.css");
const OUTPUT_DIR = path.join(ROOT, "frontend", "icons");

// ---------------------------------------------------------------------------
// Configuration – edit these to match your brand
// ---------------------------------------------------------------------------

const BRAND_TEXT = "JB";

/** Sizes required by manifest.json plus common PWA / favicon / iOS sizes. */
const ICON_SIZES = [16, 32, 48, 72, 96, 128, 144, 152, 180, 192, 384, 512];

// ---------------------------------------------------------------------------
// HTML builder
// ---------------------------------------------------------------------------

/**
 * Build a complete HTML page that renders the .brand-icon element at `size` px.
 *
 * The trick: set `1rem = size / 2` so that `.brand-icon { width: 2rem }`
 * evaluates to exactly `size` pixels.  All other rem-based values (border-radius,
 * font-size, box-shadow) scale proportionally.
 *
 * The `box-shadow` extends outside the element's border-box but is captured in
 * the screenshot because Playwright includes the element's full painted output.
 */
function buildHtml(size, css, fontOverride) {
  const remBase = size / 2; // .brand-icon is 2rem → size px

  const fontFace =
    fontOverride &&
    `@font-face {
         font-family: "IconFont";
         src: url("file://${fontOverride}") format("truetype");
       }`;

  const fontRule = fontOverride
    ? `.brand-icon { font-family: "IconFont", var(--font-sans) !important; }`
    : "";

  return `<!DOCTYPE html>
<html style="font-size:${remBase}px">
<head><style>
  ${fontFace || ""}
  ${fontRule}
  ${css}
  body {
    margin:0;
    background:transparent;
  }
</style></head>
<body><div class="brand-icon">${BRAND_TEXT}</div></body>
</html>`;
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function getThemeOutputDir(theme) {
  return theme === "dark" ? OUTPUT_DIR : path.join(OUTPUT_DIR, theme);
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

async function generateIcons(css, fontOverride, theme) {
  const outputDir = getThemeOutputDir(theme);
  fs.mkdirSync(outputDir, { recursive: true });

  // Large enough viewport to fit the biggest icon + shadow padding
  const maxSize = Math.max(...ICON_SIZES);
  const viewport = maxSize * 4;

  const browser = await chromium.launch();
  const context = await browser.newContext({
    colorScheme: theme,
    deviceScaleFactor: 1,
    viewport: { width: viewport, height: viewport },
  });
  const page = await context.newPage();

  for (const size of ICON_SIZES) {
    const html = buildHtml(size, css, fontOverride);
    await page.setContent(html, { waitUntil: "networkidle" });

    const icon = page.locator(".brand-icon");
    const out = path.join(outputDir, `icon-${size}.png`);
    await icon.screenshot({ path: out, omitBackground: true });
    console.log(`  ✓  ${path.relative(ROOT, out)}  (${size}×${size})`);
  }

  await browser.close();
}

async function main() {
  // Manual argv parsing
  let fontOverride;
  let theme = "dark";
  for (let i = 2; i < process.argv.length; i++) {
    if (process.argv[i] === "--font" || process.argv[i] === "-f") {
      fontOverride = process.argv[++i];
    } else if (process.argv[i] === "--theme" || process.argv[i] === "-t") {
      theme = process.argv[++i];
    }
  }

  const themes = theme === "both" ? ["dark", "light"] : [theme];

  const css = fs.readFileSync(STYLE_CSS, "utf-8");

  console.log(`Generating brand icons ('${BRAND_TEXT}') …`);
  if (fontOverride) console.log(`  Font: ${fontOverride}`);
  console.log(`  Themes: ${themes.join(", ")}`);
  console.log(`  Output: ${OUTPUT_DIR}\n`);

  for (const t of themes) {
    console.log(`[${t}]`);
    await generateIcons(css, fontOverride, t);
    console.log();
  }

  console.log("Done.");
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
