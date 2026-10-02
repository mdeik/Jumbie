# Jumbie Add Series UserScript

> [!NOTE]
> If you are looking for instructions on how to add a series normally via the Jumbie web interface, please refer to the [Getting Started Guide](getting-started.md#series-library).

The [`jumbie-add-series.js`](../scripts/jumbie-add-series.js) script is a browser userscript that lets you add series to your Jumbie server directly from metadata provider websites with a single click.

## What It Does

The script integrates with:
- **TheTVDB** (`thetvdb.com`)
- **TVMaze** (`tvmaze.com`)

It injects an **Add to Jumbie** button on:
- Series detail pages (next to the title/actions).
- Search result lists.

When clicked, the button extracts the series name and its metadata provider ID, then opens a new browser tab to your Jumbie instance's series-add page with those details pre-filled.

## Installation

To use this script, you need a browser extension that manages UserScripts (such as Tampermonkey, Violentmonkey, or Greasemonkey).

1. Install a UserScript manager extension for your browser (e.g., [Tampermonkey](https://www.tampermonkey.net/)).
2. Create a new script in your UserScript manager dashboard.
3. Copy the entire contents of [`jumbie-add-series.js`](../scripts/jumbie-add-series.js) and paste it into the editor.
4. Save the script.

## How to Use

1. **Configure your Jumbie Server URL:**
   - Go to either [TheTVDB](https://thetvdb.com) or [TVMaze](https://tvmaze.com).
   - Look for a gear icon (**⚙**) in the bottom-right corner of the page.
   - Click it to open the **Jumbie Server Configuration** dialog.
   - Enter the full URL of your running Jumbie server (e.g., `http://localhost:8080` or `http://192.168.1.100:8080`) and click **Save**.

2. **Add Series:**
   - Search for a show or visit any series page on TheTVDB or TVMaze.
   - Click the **Add to Jumbie** button.
   - A new tab opens on your Jumbie dashboard, pre-filled with the series name and metadata provider ID.
