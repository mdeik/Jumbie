// ==UserScript==
// @name         Jumbie - Add Series
// @namespace    https://github.com/mdeik/jumbie/
// @version      1.0.0
// @updateURL    https://github.com/mdeik/jumbie/raw/refs/heads/main/scripts/jumbie-add-series.js
// @downloadURL  https://github.com/mdeik/jumbie/raw/refs/heads/main/scripts/jumbie-add-series.js
// @description  Add series from metadata sources directly to your Jumbie server with one click.
// @match        https://thetvdb.com/series/*
// @match        https://thetvdb.com/search*
// @match        https://www.thetvdb.com/series/*
// @match        https://www.thetvdb.com/search*
// @match        https://tvmaze.com/shows/*
// @match        https://tvmaze.com/search*
// @match        https://www.tvmaze.com/shows/*
// @match        https://www.tvmaze.com/search*
// @grant        GM_getValue
// @grant        GM_setValue
// @grant        GM_addStyle
// ==/UserScript==

(function () {
  "use strict";

  const STORAGE_KEY = "jumbie_server_url";

  // ── Site Detection ─────────────────────────────────────────────────────

  const SITE = (() => {
    const host = window.location.hostname;
    if (host.includes("thetvdb.com")) return "tvdb";
    if (host.includes("tvmaze.com")) return "tvmaze";
    return "unknown";
  })();

  // ── Styles ─────────────────────────────────────────────────────────────

  GM_addStyle(`
        .jumbie-modal-overlay {
            position: fixed;
            top: 0; left: 0; right: 0; bottom: 0;
            background: rgba(0,0,0,0.6);
            z-index: 99999;
            display: flex;
            align-items: center;
            justify-content: center;
        }
        .jumbie-modal {
            background: #1a1d23;
            color: #e0e0e0;
            border-radius: 10px;
            padding: 28px 32px;
            width: 420px;
            max-width: 90vw;
            box-shadow: 0 8px 32px rgba(0,0,0,0.5);
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
        }
        .jumbie-modal h2 {
            margin: 0 0 6px 0;
            font-size: 18px;
            font-weight: 600;
        }
        .jumbie-modal p {
            margin: 0 0 18px 0;
            font-size: 13px;
            color: #999;
        }
        .jumbie-modal label {
            display: block;
            font-size: 13px;
            font-weight: 500;
            margin-bottom: 6px;
        }
        .jumbie-modal input[type="url"] {
            width: 100%;
            padding: 10px 12px;
            border: 1px solid #444;
            border-radius: 6px;
            background: #0f1117;
            color: #e0e0e0;
            font-size: 14px;
            box-sizing: border-box;
            outline: none;
        }
        .jumbie-modal input[type="url"]:focus {
            border-color: #4a9eff;
        }
        .jumbie-modal-actions {
            display: flex;
            gap: 10px;
            justify-content: flex-end;
            margin-top: 20px;
        }
        .jumbie-modal-actions button {
            padding: 8px 18px;
            border: none;
            border-radius: 6px;
            font-size: 14px;
            cursor: pointer;
            font-weight: 500;
        }
        .jumbie-modal-actions .btn-cancel {
            background: #333;
            color: #ccc;
        }
        .jumbie-modal-actions .btn-cancel:hover {
            background: #444;
        }
        .jumbie-modal-actions .btn-save {
            background: #4a9eff;
            color: #fff;
        }
        .jumbie-modal-actions .btn-save:hover {
            background: #3a8eef;
        }
        .jumbie-settings-btn {
            position: fixed;
            bottom: 16px;
            right: 16px;
            width: 40px;
            height: 40px;
            border-radius: 50%;
            background: #1a1d23;
            color: #e0e0e0;
            border: 1px solid #444;
            font-size: 18px;
            cursor: pointer;
            z-index: 99998;
            display: flex;
            align-items: center;
            justify-content: center;
            box-shadow: 0 2px 8px rgba(0,0,0,0.4);
            transition: background 0.2s;
        }
        .jumbie-settings-btn:hover {
            background: #333;
        }
        button[data-jumbie] {
            border-radius: 10px;
        }
        @media (max-width: 640px) {
            button[data-jumbie="series"] {
                margin-left: 0 !important;
            }
            button[data-jumbie] {
                width: 100% !important;
            }
        }
    `);

  // ── Storage helpers ────────────────────────────────────────────────────

  function getServerUrl() {
    return GM_getValue(STORAGE_KEY, "");
  }

  function setServerUrl(url) {
    GM_setValue(STORAGE_KEY, url);
  }

  // ── Configuration Modal ────────────────────────────────────────────────

  function showConfigModal() {
    const existing = document.querySelector(".jumbie-modal-overlay");
    if (existing) existing.remove();

    const overlay = document.createElement("div");
    overlay.className = "jumbie-modal-overlay";

    overlay.innerHTML = `
            <div class="jumbie-modal">
                <h2>Jumbie Server Configuration</h2>
                <p>Enter the address of your Jumbie server (e.g. http://192.168.1.100:8080)</p>
                <label for="jumbie-server-url">Server URL</label>
                <input type="url" id="jumbie-server-url" placeholder="http://192.168.1.100:8080" />
                <div class="jumbie-modal-actions">
                    <button class="btn-cancel" id="jumbie-modal-cancel">Cancel</button>
                    <button class="btn-save" id="jumbie-modal-save">Save</button>
                </div>
            </div>
        `;

    // Set value via DOM property to avoid innerHTML escaping issues
    const input = overlay.querySelector("#jumbie-server-url");
    input.value = getServerUrl();

    document.body.appendChild(overlay);

    const saveBtn = overlay.querySelector("#jumbie-modal-save");

    overlay
      .querySelector("#jumbie-modal-cancel")
      .addEventListener("click", () => overlay.remove());

    saveBtn.addEventListener("click", () => {
      let url = input.value.trim();
      if (url.endsWith("/")) url = url.slice(0, -1);
      if (url) {
        setServerUrl(url);
        overlay.remove();
      } else {
        input.focus();
      }
    });

    overlay.addEventListener("click", (e) => {
      if (e.target === overlay) overlay.remove();
    });

    input.addEventListener("keydown", (e) => {
      if (e.key === "Enter") saveBtn.click();
      if (e.key === "Escape") overlay.remove();
    });

    input.focus();
    input.select();
  }

  function createAddButton(extraClass) {
    const btn = document.createElement("button");
    btn.className = `btn btn-primary btn-md${extraClass ? " " + extraClass : ""}`;
    btn.textContent = "Add to Jumbie";
    return btn;
  }

  function validateOrAlert(title, seriesId) {
    if (!title || !seriesId) {
      alert("Failed to extract title or series ID.");
      return false;
    }
    return true;
  }

  // ── Add settings button ────────────────────────────────────────────────

  function addSettingsButton() {
    if (document.querySelector(".jumbie-settings-btn")) return;
    const btn = document.createElement("button");
    btn.className = "jumbie-settings-btn";
    btn.textContent = "⚙";
    btn.title = "Configure Jumbie Server";
    btn.addEventListener("click", showConfigModal);
    document.body.appendChild(btn);
  }

  // ── Navigate to Jumbie ─────────────────────────────────────────────────

  function addToJumbie(title, seriesId, source) {
    const serverUrl = getServerUrl();
    if (!serverUrl) {
      showConfigModal();
      return;
    }

    const idKey = source === "tvmaze" ? "tvmaze" : "tvdb";

    const params = new URLSearchParams({
      title: title,
      [idKey]: seriesId,
    });
    window.open(`${serverUrl}/series/add?${params.toString()}`, "_blank");
  }

  // --------------------
  // TVDB Series Page
  // --------------------

  function extractTvdbSeries() {
    const title = document.querySelector("#series_title")?.textContent.trim();

    const seriesId = [...document.querySelectorAll("#series_basic_info li")]
      .find(
        (li) =>
          li.querySelector("strong")?.textContent.trim() ===
          "TheTVDB.com Series ID",
      )
      ?.querySelector("span")
      ?.textContent.trim();

    if (validateOrAlert(title, seriesId)) addToJumbie(title, seriesId, "tvdb");
  }

  function addTvdbSeriesButton() {
    const row = document.querySelector(".container > .row.mt-2");
    if (!row || document.getElementById("tvdb-series-extract-btn")) return;

    const wrapper = document.createElement("div");
    wrapper.className = "col-12 mt-3";

    const button = createAddButton();
    button.id = "tvdb-series-extract-btn";
    button.style.marginLeft = "1.5rem";
    button.addEventListener("click", extractTvdbSeries);

    wrapper.appendChild(button);
    row.appendChild(wrapper);
  }

  // --------------------
  // TVDB Search Page
  // --------------------

  function extractTvdbSearchResult(item) {
    const title = item.querySelector(".media-heading a")?.textContent.trim();

    const match = item
      .querySelector(".text-muted")
      ?.textContent.match(/#(\d+)/);
    const seriesId = match?.[1];

    if (validateOrAlert(title, seriesId)) addToJumbie(title, seriesId, "tvdb");
  }

  function addTvdbSearchButtons() {
    document.querySelectorAll("#hits .ais-Hits-item").forEach((item) => {
      if (item.querySelector(".tvdb-extract-btn")) return;
      if (item.querySelector('.media-heading a[href*="/movies/"]')) return;

      const button = createAddButton("tvdb-extract-btn mt-2");

      button.addEventListener("click", (e) => {
        e.preventDefault();
        e.stopPropagation();
        extractTvdbSearchResult(item);
      });

      item.querySelector(".media-body")?.appendChild(button);
    });
  }

  // --------------------
  // TVMaze Series Page
  // --------------------

  function extractTvmazeSeries() {
    const match = window.location.pathname.match(/\/shows\/(\d+)/);
    const seriesId = match?.[1];

    const titleEl = document.querySelector("h1.show-for-medium");
    const title = titleEl?.textContent?.trim();

    if (validateOrAlert(title, seriesId))
      addToJumbie(title, seriesId, "tvmaze");
  }

  function addTvmazeSeriesButton() {
    const container = document.getElementById("watch-show");
    if (!container || container.querySelector("button[data-jumbie]")) return;

    const button = document.createElement("button");
    button.className = "button";
    button.setAttribute("data-jumbie", "series");
    button.textContent = "Add to Jumbie";
    button.style.marginBottom = "15px";
    button.style.marginLeft = "15px";
    button.style.padding = "1.5rem";
    button.style.width = "153.09px";
    button.style.boxSizing = "border-box";
    button.addEventListener("click", extractTvmazeSeries);

    container.insertBefore(button, container.firstChild);
  }

  // --------------------
  // TVMaze Search Page
  // --------------------

  function extractTvmazeSearchResult(item) {
    const link = item.querySelector('.showname a[href^="/shows/"]');
    if (!link) return;

    const title = link.textContent.trim();
    const match = link.getAttribute("href").match(/\/shows\/(\d+)/);
    const seriesId = match?.[1];

    if (validateOrAlert(title, seriesId))
      addToJumbie(title, seriesId, "tvmaze");
  }

  function addTvmazeSearchButtons() {
    document.querySelectorAll("div[data-key]").forEach((item) => {
      // Skip items without a primary show link (people, episodes, etc.)
      if (!item.querySelector('.showname a[href^="/shows/"]')) return;
      if (item.querySelector("button[data-jumbie]")) return;

      const button = document.createElement("button");
      button.className = "button";
      button.setAttribute("data-jumbie", "search");
      button.textContent = "Add to Jumbie";
      button.style.padding = "1rem";

      button.addEventListener("click", (e) => {
        e.preventDefault();
        e.stopPropagation();
        extractTvmazeSearchResult(item);
      });

      const content = item.querySelector("section.content");
      const footer = content?.querySelector("footer");
      if (footer) {
        button.style.marginBottom = "15px";
        content.insertBefore(button, footer);
      }
    });
  }

  // ── Shared button injection ────────────────────────────────────────────

  function addSiteButtons() {
    if (SITE === "tvdb") {
      addTvdbSeriesButton();
      addTvdbSearchButtons();
    } else if (SITE === "tvmaze") {
      addTvmazeSeriesButton();
      addTvmazeSearchButtons();
    }
  }

  // ── Shared path guards ──────────────────────────────────────────────────

  function blockNestedPaths() {
    const { hostname, pathname } = location;

    // TheTVDB
    if (
      hostname.includes("thetvdb.com") &&
      /^\/series\/[^/]+\/.+/.test(pathname)
    ) {
      return true;
    }
    // TVMaze
    if (
      hostname.includes("tvmaze.com") &&
      /^\/shows\/[^/]+\/[^/]+\/.+/.test(pathname)
    ) {
      return true;
    }
    return false;
  }

  // ── Init ───────────────────────────────────────────────────────────────

  if (blockNestedPaths()) return;

  addSettingsButton();

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", addSiteButtons);
  } else {
    addSiteButtons();
  }

  new MutationObserver(addSiteButtons).observe(document.body, {
    childList: true,
    subtree: true,
  });
})();
