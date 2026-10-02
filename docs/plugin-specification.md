# Jumbie Plugin Specification

> Mirrors `backend/src/plugins/`, `crates/plugin-sdk/`, and `crates/shared/src/plugin.rs`

---

## Table of Contents

1. [Plugin Types & Capabilities](#1-plugin-types--capabilities)
2. [Plugin Architectures](#2-plugin-architectures)
   - [Internal Plugins (built-in)](#internal-plugins-built-in)
   - [External Plugins (standalone)](#external-plugins-standalone)
3. [External Plugin Protocol (JSON-RPC 2.0)](#3-external-plugin-protocol-json-rpc-20)
   - [Transport](#transport)
   - [Request Format](#request-format)
   - [Response Formats](#response-formats)
   - [Standard Error Codes](#standard-error-codes)
   - [Protocol Flow](#protocol-flow)
   - [Discovery and Shared-Process Instance Sync](#discovery-and-shared-process-instance-sync)
   - [Reconfigure Semantics — config is input](#reconfigure-semantics--config-is-input)
4. [Required Methods by Capability](#4-required-methods-by-capability)
   - [Base Methods (All Plugins)](#base-methods-all-plugins)
   - [FeedProvider Methods](#feedprovider-methods)
   - [Downloader Methods](#downloader-methods)
   - [Notifier Methods](#notifier-methods)
     - [Schema Field: `"format": "duration"`](#schema-field-format-duration)
   - [MetadataProvider Methods](#metadataprovider-methods)
5. [PluginTypeInfo Structure](#5-plugintypeinfo-structure)
6. [Plugin SDK Crate](#6-plugin-sdk-crate)
7. [Internal Plugin Development](#7-internal-plugin-development)
   - [PluginInstance Trait](#plugininstance-trait)
8. [Rate Limiting & Failure Policy](#8-rate-limiting--failure-policy)
9. [Configuration](#9-configuration)
   - [PluginConfig Trait](#pluginconfig-trait)
   - [Config Storage](#config-storage)
   - [Manifest File (External Plugins)](#manifest-file-external-plugins)
     - [Security Constraints](#security-constraints)
     - [Field Details](#field-details)
10. [Existing Plugins](#10-existing-plugins)
11. [Building Your Own Plugin (Step-by-Step)](#11-building-your-own-plugin-step-by-step)
    - [External Plugin in Python (Minimal)](#external-plugin-in-python-minimal)
    - [External Plugin in Rust (using the SDK)](#external-plugin-in-rust-using-the-sdk)
12. [Testing a Plugin](#12-testing-a-plugin)
    - [`submitter` (release group) contract](#submitter-release-group-contract)
    - [`MediaEntry` fields](#mediaentry-fields)
13. [Plugin Data Validation Architecture](#13-plugin-data-validation-architecture)
    - [Layer 1 — Plugin Bridge](#layer-1--plugin-bridge)
    - [Layer 2 — Validate trait](#layer-2--validate-trait)
    - [Layer 3 — DB gate](#layer-3--db-gate)
    - [Validation rules per plugin type](#validation-rules-per-plugin-type)
14. [Best Practices](#14-best-practices)

---

## 1. Plugin Types & Capabilities

Jumbie defines four plugin capabilities:

| Capability | Purpose |
|---|---|
| **FeedProvider** | Discovers media entries from RSS feeds, torrent indexers, or API endpoints |
| **Downloader** | Manages downloads via a client (qBittorrent, Transmission) |
| **Notifier** | Sends out-of-band notifications (Discord webhooks) |
| **MetadataProvider** | Fetches episode/season metadata from external APIs (TVMaze, TVDB) |

A plugin may declare multiple capabilities. Variants like `MetadataProviderNormal`, `MetadataProviderAbsolute`, `Polling`, `ManualSearch`, `AutomaticSearch`, `CanPauseResume`, `CanSeed`, `FetchSeriesTitle`, and `FetchSeriesAliases` provide finer-grained capability metadata.

Capabilities are **type-level**: `plugin_info().capabilities` describes what a plugin *type* supports and is identical for every instance of that type. A subset of capabilities is additionally gated by a **per-instance config toggle** — the capability is only effective when the type declares it **and** the instance has it enabled:

| Config toggle | Capability it gates |
|---|---|
| `enable_polling` | `Polling` |
| `enable_manual_search` | `ManualSearch` |
| `enable_automatic_search` | `AutomaticSearch` |
| `enable_seeding` | `CanSeed` |

Dispatch (`get_plugins_by_all_capabilities`) evaluates these **effective capabilities**, so a plugin that declares a capability but has its toggle disabled is excluded. Metadata providers declare `Polling` to opt into the background refresh loop.

### Plugin selection intents (backend)

`PluginManager` is the SSoT for selecting plugins. Every lookup is filtered by
**enabled** + **effective capabilities** and ordered by **priority DESC, then
`instance_id` ASC** (deterministic regardless of load order). Use the named
method for the intent:

| Intent | Method | Typical use |
|---|---|---|
| **ALL** | `get_plugins_by_capability` / `get_plugins_by_all_capabilities` | notifiers, sources, search, polling loops |
| **FIRST available** | `first_plugin_by_capability` / `first_plugin_by_all_capabilities` | downloaders, single-target dispatch |
| **SPECIFIC** | `get_plugin(instance_id)` / `get_plugin_with_capability(id, cap)` | per-instance actions (e.g. one metadata provider) |

Metadata providers are currently operated **single-active** (the highest-priority
configured provider acts), but all plugin types support multiple instances and
the selectors never assume a single provider — switching metadata to true
multi-provider is a policy change, not a rewrite. A specific provider is always
addressed by `instance_id` (e.g. `?provider_instance_id=` on the Fetch actions).

**Single-active enforcement.** Only one metadata provider may be enabled at a
time. The rule lives once — `jumbie_shared::config::ensure_single_metadata_plugin`
— and is called by both sides:

- **Backend** on every write (`create_plugin_instance_endpoint`,
  `put_plugins_section_endpoint`) **and on config load** (`main.rs`), so a
  hand-edited config or a legacy row can never leave two providers enabled at
  runtime.
- **Frontend** before saving a metadata instance, so the outgoing payload is
  already valid and the UI can explain which provider was switched off.

`PluginManager::active_metadata_provider` is the runtime SSoT for "the one that
acts": it returns the highest-priority enabled provider, logs an error if more
than one is enabled, and is what the metadata refresh loop polls.

**Matchability (frontend).** Because metadata is single-active, a series/season
is considered *matchable* only when the **current active provider** has cached
metadata for it — a cache entry belonging solely to a lower-priority provider
does **not** make it matchable.
This is the SSoT implemented by `utils::has_matchable_metadata` (match-button
visibility in `EpisodeDetailsModal` and `SeasonAccordionList`). Series-level
matching uses `utils::season_mismatch` (the EpisodesTab "Match Seasons" action).

**Deleted seasons.** A user-deleted season is recorded durably in
`suppressed_seasons` (the "intentional removal" marker, distinct from "missing").
Suppressed seasons are excluded from the season list and from metadata merges,
and **filesystem rescans skip them** so they cannot resurrect via raw counts.
"Match Seasons" deliberately *includes* a suppressed season as `missing` — it is
the intended way to restore one (re-add the cell + rehydrate from the provider
cache); it is never auto-restored.

> **Serialization:** Capability enum values use `snake_case` in JSON. `FeedProvider` → `"feed_provider"`, `MetadataProviderNormal` → `"metadata_provider_normal"`, `CanPauseResume` → `"can_pause_resume"`, etc.

---

## 2. Plugin Architectures

### Internal Plugins (built-in)

Compiled into the backend binary. They implement the `PluginInstance` trait directly with their own `handle_custom_method()` for method dispatch. Each internal plugin lives in its own module under `backend/src/plugins/` and self-registers via `register_full()`.

**Advantages:** Performance, full access to the backend crate ecosystem, synchronous startup.

**Disadvantages:** Requires the Rust compiler and the full backend codebase.

### External Plugins (standalone)

Standalone executables communicating over stdin/stdout via newline-delimited JSON-RPC 2.0. Each lives in a subdirectory under the `plugins.directory` config path with a `manifest.json` file. **One process serves ALL instances of a plugin type** — instances are routed by `instance_id` on every request, and adding/removing/reconfiguring an instance is an RPC, never a process spawn.

**Advantages:** Any language, no recompilation needed, independent release cycles, process isolation.

**Disadvantages:** IPC overhead, 30-second timeout on calls.

---

## 3. External Plugin Protocol (JSON-RPC 2.0)

### Transport

- **stdin:** Host writes JSON-RPC requests, one per line.
- **stdout:** Plugin writes JSON-RPC responses, one per line.
- **stderr:** Reserved for logging — inherited by the host.
  **Type-level only:** stderr forwarding is per PROCESS, not per instance —
  one process serves all instances of a type, so a log line cannot be
  attributed to an instance from the stream alone. The host attaches a
  `plugin.type` span to the read task and an `instance` field to every
  outbound request, so per-call logs are filterable by instance; anything the
  plugin prints to stderr outside a call is attributed to the type.
- All I/O is newline-delimited JSON.

### Request Format

```json
{"jsonrpc":"2.0","method":"<method>","params":<any JSON>,"id":<number>,"instance_id":"<instance key>"}
```

- `instance_id` (optional) routes the request to one instance inside the shared process. `None`/absent targets **type-level** methods (`get_info`, `get_config_schema`, `validate_config`, health-check probe).
- `auth` (optional) carries an IPC authentication token. The host sets `JUMBIE_PLUGIN_SECRET` in the plugin's environment at spawn time and includes `auth: "<secret>"` in every request. The plugin must echo this back in every response — the host validates the echo to prevent another process from injecting fake responses.

### Response Formats

**Success:**
```json
{"jsonrpc":"2.0","result":<any JSON>,"id":<number>,"auth":"<echoed_secret>"}
```

**Error:**
```json
{"jsonrpc":"2.0","error":{"code":<int>,"message":"<str>"},"id":<number>,"auth":"<echoed_secret>"}
```

### Standard Error Codes

| Code | Meaning |
|---|---|
| `-32700` | Parse error |
| `-32600` | Invalid request |
| `-32601` | Method not found |
| `-32602` | Invalid params |
| `-32603` | Internal error (unhandled exception in a scripted plugin) |
| `-32000` | Plugin error (generic handler failure inside the plugin) |
| `-32001` | Invalid request state — auth failure, unknown instance, or failed health check |
| `-32029` | Rate limited (includes `retry_after` in `data`) |
| `-32030` | Authentication failed (credentials rejected — never retried, enters cooldown) |
| `-32031` | Transient failure (no handshake / 5xx — retried with backoff) |
| `-32032` | Permanent failure (deterministic — never retried) |

### Protocol Flow

1. Host spawns ONE process per plugin TYPE (stdin/stdout piped, stderr inherited). Sets `JUMBIE_PLUGIN_SECRET` env var for IPC auth.
2. Reader task continuously reads stdout and routes responses by matching `id`.
3. A lifecycle task owns the process and an **unbounded request queue** — a slow or hung instance can never cause a request to be dropped; callers wait (or time out after 30s).
4. After a crash/restart, the lifecycle task **replays** `set_config` for every live instance (configs are cached host-side), then drains queued requests into the fresh process.
5. Host sends `get_info` (type-level, no `instance_id`) during discovery to learn capabilities.
6. Request IDs are sequential per host. Health-check probes use a separate counter.
7. Plugin should exit cleanly when stdin reaches EOF.

### Transport Semantics — pipelining, matching, timeouts

- **Requests are pipelined, never serialized by response.** The host does not
  wait for a response before sending the next request — many requests can be
  in flight at once. Responses may arrive in ANY order: they are matched to
  requests by `id` (sequential per host; the health/replay probes use a
  separate counter so the two never collide) and every response must echo the
  request's `auth` token (fail-closed — a wrong/missing echo is discarded
  before id routing).
- **Serial processing is fine.** The host absorbs bursts in its queue, so a
  plugin may process requests one at a time (the SDK server does); handling
  them concurrently is also correct as long as each response echoes its
  request's `id` and `auth`. The pipeline contract is: answer what you can,
  in any order, matching by id.
- **Timeouts.** Per-call response timeout: 30s. Health probe: 10s. Writing to
  the plugin's stdin: bounded at 5s — a plugin that stops reading stdin fills
  the OS pipe buffer and is treated as **hung**: killed and respawned. Queued
  requests survive the restart and instance configs are replayed (see Protocol Flow).
- **Health checks bypass the request queue.** The lifecycle task writes the
  probe directly to the process every 60s — never more often, regardless of
  queue depth — and counts a miss when no response arrives within 10s. Three
  consecutive misses restart the process.
- **No response to a call** ⇒ the caller times out at 30s and the error is
  classified as transient (retried with backoff, then a shared transient
  cooldown — see §8).

### Discovery and Shared-Process Instance Sync

External plugins are loaded in two phases:

1. **Discovery (once at startup):** the backend spawns a transient probe process
   and calls `get_info` (type-level) to learn the plugin's version, capabilities,
   and rate limit. The response is validated (reserved author values + semver) and
   the canonical TYPE id is derived from author + display name
   (`PluginTypeInfo::derived_id`) — `plugin_id`/`instance_id` are backend-owned and
   never plugin-supplied. The probe exits; the type's real process (one per type,
   serving all instances) spawns separately on instance sync. If two plugin
   directories report the same type id, the higher version wins.
2. **Instance sync (startup + config changes):** ONE shared process per type is
   spawned and each desired instance's config is pushed into it via an
   idempotent `set_config` RPC carrying the config and `instance_id`. Instances
   are keyed by their config instance id. A type with no config entries gets a
   single instance keyed by its canonical id (so installed-but-unconfigured
   plugins still appear).

Removing an instance from config sends a `shutdown_instance` RPC (the shared
process stays alive); adding or changing one sends `set_config` — no restart
required.

**Crash recovery:** if the process dies, the lifecycle task respawns it and
replays `set_config` for every live instance from its cached config.

**Queue-based IPC:** requests travel through an unbounded queue owned by the
lifecycle task; a slow or hung instance never drops a request, and the queue
survives process restarts.

### Reconfigure Semantics — config is input

Config saves are **targeted**: only instances whose config actually changed are
touched (diffed against what each instance was last built with).

1. **`set_config` (idempotent):** the backend pushes the new config to the
   instance via `set_config(instance_id, config)` — the same RPC creates and
   updates. **Plugins never implement "reconfigure" logic**: they receive the
   config as INPUT on every call (via the SDK's `CallContext`), and any derived
   state (tokens, sessions, transports) is memoized against the config — the
   host clears the per-instance cache on `set_config`, so stale state is
   re-derived.
2. **No rebuild / re-init fallback:** a failed `set_config` (invalid config that
   slipped past schema validation, or a plugin that doesn't support it) marks
   the instance **Failed** — the instance keeps running on its previous config
   and the next save retries. A rebuild would fail with the same config, so the
   fallback machinery does not exist.

Because config is validated against the schema at the API boundary, `set_config`
is infallible-by-contract for valid configs.

---

## 4. Required Methods by Capability

### Base Methods (All Plugins)

| Method | Purpose | Response |
|---|---|---|
| `get_info` | Return type-level `PluginTypeInfo` metadata | `PluginTypeInfo` object |
| `get_config_schema` | Return JSON Schema for config | JSON Schema object |
| `validate_config` | Validate a configuration object | `true` or `{"errors": [...]}` |
| `set_config` | Push (create or update) an instance's config — config is input | `true` or error |
| `shutdown_instance` | Remove an instance's config from the process | `true` or error |
| `health_check` | Verify IPC is alive (lightweight, no external calls) | `"ok"` or error |
| `*` (catch-all) | Any method not listed above: external plugins dispatch it to the SDK `PluginHandler::handle` (config + params via `CallContext`); internal plugins to `PluginInstance::handle_custom_method` | varies |

**`test` method:** Plugins declare support via `supports_test: true` in `PluginTypeInfo`. When set, the UI shows a "Test" button that sends `{"method": "test", "params": <current_config>}` to the plugin.

- **External plugins:** the `test` method arrives as a raw JSON-RPC call and is dispatched through `handle_custom_method` just like any capability-specific method.
- **Internal plugins:** the default `PluginInstance::call()` implementation handles `test` directly by delegating to `test_impl()` — it does NOT route through `handle_custom_method`.

### FeedProvider Methods

| Method | Params | Response |
|---|---|---|
| `fetch_entries` | none | `Vec<MediaEntry>` |
| `search` | query string or object | `{ "entries": [MediaEntry], "queries": [string] }` |
| `auto_search` | `{series_title, season, episodes, aliases, keys}` | `{ "entries": [MediaEntry], "queries": [string] }` |

> **Envelope (v3).** `search` and `auto_search` MUST return the
> `{ "entries": [...], "queries": [...] }` envelope — a bare `Vec<MediaEntry>` is
> rejected at the bridge. `queries` is the exact query string(s) the plugin sent to
> its indexer, in order; the backend logs them verbatim (tracing target
> `search.query`) and treats them as authoritative. `fetch_entries` (polling) has no
> query and stays a bare `Vec<MediaEntry>`.
>
> Built-in sources build the query with `plugin_sdk::query` (`build_search_queries`, or the
> `SearchResponse::from_auto_search` / `from_manual` helpers) so reporting is
> automatic. External plugins may build their own queries, but MUST report the
> string(s) they sent in `queries`.
>
> `auto_search` payloads are **information, not a mandated query format** — the plugin
> owns how it turns them into its source's search syntax.
> `episodes` are already in **source numbering** (any per-season `episode_offset` has
> been applied by the backend), so a plugin should use them verbatim in queries.
> `keys` holds one rendered search key per episode, aligned with `episodes` (e.g.
> `S01E05`); a blank key means search by title alone. Aliases are OR-chained into
> one quoted group and episodes are searched individually — the SDK default is one
> query per key, e.g. `("Alias A"|"Alias B") S01E05`.

Each `MediaEntry` is a JSON object with this shape:

```typescript
{
  "title": string,                  // Release title
  "source": string,                 // Source plugin name badge (e.g. "nyaa")
  "guid": string | null,            // Unique ID / GUID from the feed
  "link": string | null,            // Original feed item link (e.g. torrent info page)
  "published": string | null,       // RFC 3339 with an explicit offset (e.g. "2025-01-15T14:30:00Z")
  "download_url": string | null,    // Actual download URL (magnet URI or .torrent URL)
  "download_id": string | null,     // Unique ID for tracking in download client (alias = "info_hash")
  "size": number | null,            // Size in bytes
  "seeders": number | null,
  "leechers": number | null,
  "description": string | null,
  "file_list": string[],            // Defaults to []
  "category": string | null,
  "submitter": string | null        // Release group / submitter
}
```

> The `source` field is **required** — it's used for display badges in the UI.
> The `download_id` field is **required** — it is the unique identifier the backend uses to track the download in the client.
> Sources that parse RSS feeds with only magnet links (no separate hash element) are responsible for parsing the BTIH hash from the magnet URI and setting `download_id` themselves.
> The `link` field is the original feed item link; `download_url` is the actual download URL (which may be a magnet URI).

### Downloader Methods

| Method | Params | Response |
|---|---|---|
| `add_download` | `{url, category, tag?, title?}` | `null` | The return value is **not used** — the backend identifies the download by the source-supplied `download_id`, so plugins may return `null`. |
| `get_completed_downloads` | none | `Vec<String>` (IDs) |
| `get_download_progress` | download ID | `f64` (0.0–1.0) or `null` |
| `get_download_status` | download ID | `string` or `null` (e.g. `"downloading"`, `"paused"`, `"seeding"`, `"completed"`) | Bounded to 256 chars and free of control characters — an out-of-bounds token is rejected. |
| `get_download_failure` | download ID | reason string or `null` | **Optional.** Return a reason when the client reports a hard, non-recoverable failure (e.g. qBittorrent's `error` / `missingFiles` states). `null` means no failure. A reported failure marks the queue item `Failed` immediately — the item is never treated as merely slow. Clients that don't implement the method are skipped (the call returns "method not found", which is not a plugin health failure). The reason is **sanitized, not rejected**: it is trimmed, control characters are replaced with spaces, and it is truncated to 1024 chars (with an ellipsis). A sanitized reason is still honored as a failure and logged at `warn`. |
| `get_download_content_path` | download ID | path string | Returns the path to a **unique directory** for this download. Must be a directory (not a file path) that contains all downloaded files/subdirectories. This directory is expected to be safely deletable — its contents are isolated to this single download. The path must be returned for any active download (even while still downloading); if the download does not exist, the plugin should return an error instead of `null`. For per-download isolation, use UUID subfolders or similar. The directory is used for cleanup: after organization, empty subdirectories up to and including this path are removed. |
| `get_download_id_by_name` | name string | ID string or `null` |
| `pause_download` | download ID | `null` |
| `resume_download` | download ID | `null` |
| `delete_download` | `{id, delete_files}` | `null` |
| `complete_download` | download ID | `null` | Notifies the client that a download has been fully processed (organized to its final destination). The client may clean up its internal state — for qBittorrent this removes the torrent entry without deleting files on disk. Other clients may no-op. |
| `retry` | download ID | `null` |
| `get_download_path` | none | path string or `null` |
| `get_organizer_path` | none | path string or `null` |
| `test_connection` | none | success message string | Non-empty, ≤4096 chars, no control characters. |

> The `add_download` `url` parameter accepts magnet URIs or `.torrent` URLs — sources send their `download_url` (which may be a magnet URI) here.
> Downloader plugins declare their supported protocols via `supported_protocols` in `PluginTypeInfo`, e.g. `["magnet:*", "*://*.torrent"]`.

### Notifier Methods

| Method | Params |
|---|---|
| `notify` | `{event, context}` |

Event types: `DownloadStarted`, `DownloadCompleted`, `Error`, `RenameQueue`, `Test`.

Context is a JSON object. Each event type provides a specific subset of fields:

| Event | Context Fields |
|---|---|
| `DownloadStarted` | `series_title`, `release_title`, `indexer` (optional), `size_bytes` (optional), `season` (optional), `episode` (optional), `episode_end` (optional), `is_season_pack` (optional), `images` (optional) |
| `DownloadCompleted` | `series_title`, `release_title`, `episode_id`, `indexer` (optional), `size_bytes` (optional), `media_info` (optional), `season` (optional), `episode` (optional), `episode_end` (optional), `is_season_pack` (optional), `images` (optional) |
| `Error` | `error_context`, `error_message` |
| `RenameQueue` | `rename_queue_entries` (preferred — list of `[series, count]` pairs), or `series_title` + `affected_count` (legacy single-entry) |
| `Test` | *(none required)* |

`episode_end` is present only for multi-episode ranges (e.g. `episode=1`, `episode_end=3` for S01E01-E03).
Single-episode releases omit it (`null`).

`is_season_pack` is `true` when the download covers a range of episodes (e.g. a full season pack);
`episode` + `episode_end` carry the actual range data.

`indexer` is the name of the source plugin that discovered the download (e.g. `"nyaa"`, `"basic_rss"`).
Omitted (`null`) for manual downloads.

`size_bytes` is the total release size in bytes. Present for automated downloads; may be omitted
for manual entries.

`images` is a map of semantic keys to URLs. Shared conventions:

| Key | Usage |
|---|---|
| `"series"` | Series poster / thumbnail (used as embed thumbnail in Discord) |
| `"episode"` | Episode still / backdrop (used as large embed image in Discord) |

Any plugin can set images via `context.with_image("series", url)`. Keys are
open-ended — notifiers that consume new keys should document the convention.

`media_info` contains the ffprobe scan result. It is only populated for
`DownloadCompleted` (fired after fingerprinting). Structure:

| Field | Type | Description |
|---|---|---|
| `resolution` | `string?` | e.g. `"1920x1080"` |
| `codec` | `string?` | e.g. `"HEVC"`, `"h264"` |
| `duration` | `string?` | e.g. `"24 min"` |
| `audio_languages` | `string[]` | e.g. `["eng", "jpn"]` |
| `subtitle_languages` | `string[]` | e.g. `["eng"]` |

Notifier config schemas may include `"x-template-context"` on string fields
to signal that the field supports `${variable}` substitution. The value names
the variable scope (e.g. `"NotifierDownload"`). The UI reads this to render
variable-reference tooltips showing only the variables available in that
scope. See `jumbie_shared::template::apply_template` for the substitution syntax.

#### Schema Field: `"format": "duration"`

Integer fields may declare `"format": "duration"` to render a time-input
widget that accepts human-readable durations instead of a plain number field.

| Schema field | Value | Description |
|---|---|---|
| `"format"` | `"duration"` | Enables the time-input widget |
| `"x-time-unit"` | `"seconds"` / `"minutes"` / `"hours"` / `"days"` | The native storage unit for the integer value. Defaults to `"minutes"`. |

The UI converts between the native unit and the following human-readable forms:

| Suffix | Examples | Multiplier (seconds) |
|---|---|---|
| `s` / `sec` / `second` / `seconds` | `30s`, `500sec` | 1 |
| `m` / `min` / `minute` / `minutes` | `5m`, `30min` | 60 |
| `h` / `hr` / `hour` / `hours` | `2h`, `24 hours` | 3,600 |
| `d` / `day` / `days` | `7d`, `1 day` | 86,400 |
| `w` / `wk` / `week` / `weeks` | `2w`, `1 wk`, `3 weeks` | 604,800 |
| `mon` / `month` / `months` | `1mon`, `6 months` | 2,592,000 (30 days) |
| `y` / `yr` / `year` / `years` | `1y`, `2 years` | 31,536,000 (365 days) |

Multiple groups may be combined with spaces (e.g. `"1h 30min"`).
A bare number with no suffix is treated as the native unit.
The stored value is always a plain integer in the native unit — the
parsing and formatting are frontend-only.

Example:

```json
"refresh_interval": {
    "type": "integer",
    "format": "duration",
    "x-time-unit": "minutes",
    "title": "Refresh Interval",
    "description": "How often to check for new entries (e.g. 30m, 2h)",
    "default": 10,
    "minimum": 5
}
```

### MetadataProvider Methods

| Method | Params | Response |
|---|---|---|
| `fetch_series_metadata` | `{id: string, language?: string, ...}` | `SeriesMetadata` |
| `fetch_series_info` | `{id: string}` | `SeriesMetadataInfo` |
| `fetch_series_aliases` | `{id: string}` | `HashMap<String, Vec<String>>` |
| `get_updated_series` | none | `Vec<String>` (IDs) |
| `uses_absolute_episode_numbering` | none | `bool` |

`SeriesMetadata` shape:

```typescript
{
  episodes: [
    {
      "unique_id": string,     // Provider-specific ID (e.g. TVDB ID). Non-empty, max 512 chars.
      "season": number,        // 0..10000. 0 = specials (season 0). Negative rejected.
      "episode": number,       // 1..10000. 0 and negative rejected.
      "title": string,         // Non-empty, max 500 chars.
      "description": string | null,
      "runtime": number | null,   // Seconds (TVDB/TVMaze format). 0..86400 (24h).
      "image_url": string | null, // Must start with http://, https://, or / (relative).
      "meta_date": string | null  // RFC 3339 WITH an explicit offset (e.g. "...Z" or "...+00:00"). Zone-less/date-only input is rejected — the plugin owns the zone.
    }
  ],
  seasons: [
    {
      "season": number,         // 0..10000
      "episode_count": number   // 0..10000, non-negative.
    }
  ]
}
```

`SeriesMetadataInfo` shape:

```typescript
{
  "name": string,                  // Canonical series title. Non-empty, max 500 chars.
  "overview": string | null,       // Description / synopsis
  "original_country": string | null, // ISO 3166-1 alpha-3 (e.g. "jpn", "usa")
  "aliases": { [language: string]: string[] },  // Alternative titles keyed by language code.
  // Each alias: non-empty, max 500 chars.
  "image_url": string | null         // Must start with http:// or https://.
}
```

`EpisodeMetadata` fields are also used inside `SeriesMetadata.episodes[]`.

> **Validation SSoT**: All field limits above are enforced by the `Validate` trait
> implementations in `backend/src/validation/plugin_data.rs`, which delegate to
> the shared validators in `jumbie_shared::validation::fields::*`.
> Updating a limit in the shared crate updates validation across the API
> boundary, plugin bridge, and DB gate automatically.

---

## 5. PluginTypeInfo Structure

`PluginTypeInfo` is the **immutable type-level metadata** — identical for every
instance of a type. Backend-owned identity (`plugin_id` type id / `instance_id`
instance key) is NOT part of it: the API boundary attaches identity as flat
instance records (`PluginInstanceInfo` for `/api/plugins`, `PluginTypeListing`
for `/api/plugins/available`).

```rust
pub struct PluginTypeInfo {
    pub display_name: String,
    pub version: String,
    pub author: String,
    pub description: String,
    pub capabilities: Vec<Capability>,
    pub supported_protocols: Option<Vec<String>>,
    pub series_identifier_label: Option<String>,
    pub series_identifier_placeholder: Option<String>,
    pub rate_limit: Option<RateLimit>,
    pub supports_test: bool,
}
```

`RateLimit` struct (used in `rate_limit` field):

```rust
pub struct RateLimit {
    pub requests_per_minute: u32,  // Max API calls per minute
    pub burst: u32,                 // Initial burst allowance
}
```

Serialized as JSON: `{"requests_per_minute": 30, "burst": 2}`.

The host derives a canonical plugin ID: `{author_slug}.{display_name_slug}`. Slugs are lowercased, non-alphanumeric replaced with `_`, consecutive underscores collapsed, leading/trailing trimmed.

The plugin type key (e.g. `"tvdb"`, `"tvmaze"`) is derived from `plugin_id` by taking the component after the last `.` — use `jumbie_shared::plugin::plugin_type_key()` (shared helper) to extract it.

---

## 6. Plugin SDK Crate

`crates/plugin-sdk/` provides the tooling for building external plugins in Rust:

- **Default features:** Types (`PluginTypeInfo`, `Capability`, `PluginError`, `JsonRpcRequest`, `JsonRpcResponse`).
- **`server` feature:** `PluginServer` harness (requires tokio).

Key types:

- `JsonRpcRequest` — Request with `jsonrpc`, `method`, `params`, `id`, optional `instance_id` (instance routing) and `auth` (IPC token).
- `JsonRpcResponse` — Discriminated union: `Result(ResultResponse)` or `Error(ErrorResponse)`.
- `PluginError` — Structured failure a handler can return to control the host's failure policy (see §8). `PluginServer` maps it to the matching JSON-RPC code; any other handler error becomes the generic `-32000`.
- `PluginServer` — Multi-instance server harness: ONE handler per process, reads stdin, routes by `instance_id`, dispatches to the handler, writes stdout. Constructed with a `PluginTypeInfo` (type-level metadata) and a `Box<dyn PluginHandler>`.

`PluginServer` handles:
1. Reading newline-delimited JSON-RPC requests from stdin.
2. Validating the IPC auth token (`JUMBIE_PLUGIN_SECRET` env var) — **fail-closed**: a missing secret rejects every request.
3. The `hello` startup handshake (`{protocol_version}`) — the host requires it before serving requests.
4. Routing requests by `instance_id`: `None` → type-level methods (`get_info` / `get_config_schema` / `validate_config` / `health_check`); `Some` → that instance's config + derived-state cache, passed to the handler via `CallContext`.
5. Instance lifecycle: `set_config` (idempotent create-or-update of the config row + cache clear), `shutdown_instance` (drop the row). Config is INPUT — there is no `reconfigure` logic anywhere.
6. Writing newline-delimited JSON-RPC responses to stdout.
7. Echoing the auth token in every response.
8. Graceful shutdown on stdin EOF.

`PluginHandler` trait — ONE object per process, never per instance:

```rust
#[async_trait]
pub trait PluginHandler: Send + Sync {
    async fn get_info(&self) -> PluginTypeInfo;      // type-level, immutable
    async fn get_config_schema(&self) -> Value;
    async fn validate_config(&self, config: Value) -> Result<(), Vec<String>>;
    async fn health_check(&self) -> Result<(), anyhow::Error>; // type-level
    async fn handle(&self, ctx: CallContext<'_>) -> Result<Value, anyhow::Error>;
}

pub struct CallContext<'a> {
    pub instance_id: &'a str,   // which instance this call targets
    pub config: &'a Value,      // the instance's current config (INPUT)
    pub method: &'a str,        // capability-specific method (e.g. "search")
    pub params: Value,          // method parameters
    pub cache: &'a mut HashMap<String, Value>, // per-instance derived-state
}                                               // cache; cleared on set_config
```

`handle` returns `anyhow::Error`. Return a `PluginError` to signal a *specific*
failure kind instead of the generic `-32000`; `PluginServer` downcasts it and emits
the matching code (§8):

```rust
use plugin_sdk::PluginError;

// 401/403 on login -> never retried, auth cooldown
return Err(PluginError::AuthFailed("invalid API key".into()).into());
// connect/timeout/DNS/TLS or 5xx -> retried with backoff
return Err(PluginError::Transient(format!("{e}")).into());
// 429 -> pause for retry_after seconds
return Err(PluginError::RateLimited { retry_after_secs: 30, message: "slow down".into() }.into());
// deterministic (bad config / not found) -> surfaced immediately
return Err(PluginError::Permanent("series not found".into()).into());
// method not implemented -> caller skips it quietly
return Err(PluginError::MethodNotSupported("search".into()).into());
```

Any other error (e.g. `anyhow::bail!`) is reported as `-32000` and treated as
transient by the host.

---

## 7. Internal Plugin Development

All internal plugins implement the `PluginInstance` trait directly — there are no separate domain traits or wrapper structs. Each plugin (Nyaa, RSS, qBittorrent, Discord, TVMaze, TVDB) lives in its own module under `backend/src/plugins/` and self-registers via `register_full()` on the `InternalPluginRegistry`.

Built-in plugins declare the first-party author `"Jumbie"` (`jumbie_shared::plugin::JUMBIE_AUTHOR`), so their derived type id is namespaced as `jumbie.{name}` (e.g. `jumbie.tvdb`). External plugins must not use that author — it is reserved. Each built-in plugin versions independently via its own module-level `PLUGIN_VERSION` constant, and the UI hides the version for built-in plugins since they ship with the app.

### PluginInstance Trait

```rust
pub trait PluginInstance: Send + Sync {
    fn instance_id(&self) -> &str;
    fn supported_protocols(&self) -> Option<&[String]>;
    fn priority(&self) -> i32;
    fn set_priority(&self, new_priority: i32) {}
    fn is_enabled(&self) -> bool { true }
    fn set_enabled(&self, enabled: bool) {}
    fn refresh_interval(&self) -> Option<u64> { None }
    fn set_refresh_interval(&self, minutes: Option<u64>) {}
    fn plugin_info(&self) -> PluginTypeInfo;
    async fn call(&self, method: &str, params: Option<Value>) -> Result<Value>;
    async fn handle_custom_method(&self, method: &str, params: Option<Value>) -> Result<Value>;
    async fn health_check_impl(&self) -> Result<()> { Ok(()) }
    async fn test_impl(&self) -> Result<()> { Ok(()) }
    async fn set_config(&self, config: Value) -> Result<Value>;
    fn is_healthy(&self) -> bool { true }              // in-process, no RPC
    async fn health_status(&self) -> (bool, Option<String>) { (self.is_healthy(), None) }
}
```

The `call()` method has a default implementation that handles `get_info` (via `plugin_info()`), `health_check` (via `health_check_impl()`), and `test` (via `test_impl()`). Plugins override `handle_custom_method()` for their capability-specific methods.

### `set_config` for internal plugins — config is input

Built-in plugins implement `PluginInstance::set_config` so config saves update them in place (preserving runtime state like the TVDB token) instead of factory-replacing them. The pattern (SSoT):

1. **Parse + validate FIRST** — deserialize the plugin's existing `*Settings`/`*Config` struct from the config `Value` (serde ignores the reserved `name`/`enabled`/`priority`/`refresh_interval` keys). On any error return `Err` **before mutating anything** (see §3 for the failed-save contract).
2. **Apply** — write the new settings behind the plugin's `std::sync::RwLock` config field (all config reads go through a `cfg()` clone accessor so the std lock is never held across an await).
3. **Invalidate auth state only when credentials changed** — e.g. TVDB clears its cached token iff `api_key`/`pin` changed. Plugins that re-login per operation (qBittorrent) need no explicit invalidation.
4. **Structural fields apply IN PLACE too** — a config field baked into a transport at construction (e.g. qBittorrent's `link`/`verify_ssl` → Referer/CSRF base + TLS acceptance in its HTTP client) is rebuilt inside `set_config` with the same global config the factory would use, so the rebuilt transport is byte-identical to a factory-constructed one. **No valid config ever requires a factory rebuild**; a trait-default `set_config` error (e.g. `MethodNotSupported`) is handled like any other failure (§3).

`enabled`, `priority`, and `refresh_interval` are backend-managed host fields: the manager syncs them onto the plugin wrapper (`PolicyPlugin` stores them) after a successful `set_config`, so plugins never handle them.

Config saves are **targeted**: `apply_config` diffs each instance's config against
what it was last built with and only touches changed instances. Unchanged
instances keep their exact `Arc` (identity and runtime state survive); changed
internal instances apply `set_config` in place (same `Arc`). There is exactly
**one copy of an instance at any time** — the swap commits either the kept
instance or a fresh build (new instances only), never both.

---

## 8. Rate Limiting & Failure Policy

All plugin calls pass through a decorator (`PolicyPlugin` in `policy.rs`) that
applies the backend's shared failure policy. Plugins never decide policy — they only
*classify* failures, using the shared vocabulary below. The decorator stack has four
layers:

1. **Failure cooldowns** — A plugin that fails in a way retrying won't fix (rejected
   credentials) — or that keeps failing transiently (network down, provider erroring)
   — is put on an escalating backoff window shared by all concurrent callers:
   1 min → 5 min → 15 min → capped at 1 hour. During the window, calls return the
   cached failure immediately without touching the plugin.
2. **Explicit pause** — If a plugin returns a rate-limit error, all calls are paused
   for `retry_after` seconds.
3. **Token bucket** — Uses `governor` crate's GCRA algorithm for per-minute quotas
   with burst allowance (declared via `rate_limit` in `PluginTypeInfo`).
4. **Retry with backoff** — Transient errors trigger up to 3 retries with jitter
   (500ms → 1000ms).

### Failure classification (SSoT)

Plugins classify failures; the wrapper applies policy:

| Failure | Internal plugins signal | External plugins signal | Policy |
|---|---|---|---|
| **Auth failure** — remote answered but rejected the credentials (e.g. HTTP 401/403 on login) | `PluginCallError::AuthFailed` | JSON-RPC code `-32030` | **Never retried** — the same credentials will fail again. Enters the auth cooldown. |
| **Rate limited** — remote asked us to slow down (429) | `PluginCallError::RetryAfter` | JSON-RPC code `-32029` (with `retry_after` in `data`) | Pause all calls for `retry_after` seconds. |
| **Transient** — no handshake (connect/timeout/DNS/TLS) or 5xx | `PluginCallError::Transient` | JSON-RPC code `-32031` | Retried up to 3× with backoff; if all attempts fail, enters the transient cooldown. |
| **Permanent** — deterministic: bad config, not-found, unsupported operation, structural errors | `PluginCallError::Permanent` (plus `InvalidResponse`, `MethodNotSupported`, `PluginPanicked`, `ProcessDead`, `Internal`) | JSON-RPC code `-32032` | **Never retried, no cooldown** — surfaced immediately. |
| **Unknown / custom** — anything else | any other error | any other error / unknown code | Catch-all: treated as transient (safe default) and logged distinctly. |

### Cooldown resets

A cooldown clears automatically on: a successful call, an explicit `test` (the Test
button forces a live attempt and bypasses the cooldown), or a successful
`set_config` (e.g. fixed credentials — the wrapper clears the cooldown before
delegating). A failed `set_config` marks the instance Failed (see §3).

### Status surfacing

Health is **type-level**: the host probes each plugin PROCESS (external) or
reports loaded state (internal); per-instance failure state (cooldowns, 429
pauses) is read **in-process** from the wrapper — the status page performs zero
RPCs to plugin processes. `health_check` as an instance-level RPC does not
exist. `get_info` is never gated.

### Author guidance

- On a **login endpoint**, treat every 4xx (except 429) as an auth failure — it's
  your config, not a transient blip. Treat "no response" (connect/timeout) and 5xx
  as transient (`-32031` / `PluginCallError::Transient`).
- Return the most specific signal you can: `-32030` / `AuthFailed` for credentials,
  `-32029` / `RetryAfter` for 429s, `-32031` / `Transient` for network/5xx,
  `-32032` / `Permanent` for deterministic failures. When in doubt, return a plain
  error — the catch-all handles it safely.
- **Rust SDK**: return a `plugin_sdk::PluginError` (`AuthFailed` / `RateLimited` /
  `Transient` / `Permanent` / `MethodNotSupported`) from `handle` and `PluginServer`
  emits the matching code above; scripted plugins send the code directly. Any other
  error is reported as the generic `-32000` (unknown → treated as transient).
- Declare a `supports_test` implementation that performs a real login — it's the
  user's escape hatch from the auth cooldown.

### Trust model & sandboxing

External plugin processes run with the backend user's file/network access. The
sandbox bounds **abuse and secret access**, not hostile-code confinement:

| Control | Scope |
|---|---|
| IPC auth (per-spawn secret, both directions, fail-closed) | Pipe injection by third-party processes — the plugin cannot be fed forged requests, the host cannot be fed forged responses |
| Authenticated `hello` handshake + protocol version | Startup: proves liveness + both auth directions + protocol compatibility before any request is served |
| Environment isolation (`env_clear` + allowlist) | The plugin never inherits backend secrets (DB credentials, API keys) — only `JUMBIE_PLUGIN_SECRET`, `PATH`, `HOME`, TMP*, and proxy vars |
| Per-plugin rlimits (`pre_exec`: CPU, NOFILE, FSIZE, DATA) | A runaway plugin is bounded without constraining the host. Deliberately no `RLIMIT_NPROC` — on Linux it counts the user's whole session, so it false-fails legitimate plugin shell-outs on busy machines; process-count abuse is bounded by group-kill + deployment cgroups |
| Process-group kill | Plugin grandchildren (shell-outs) die with it — no orphans |
| Bounded stderr | Log floods are capped, not written unbounded to disk |
| Persistent PID registry | Stale processes from a crashed host are swept on next startup |
| Optional Ed25519 signature verification | The executable is what the author signed (per-manifest); a global `require_plugin_signatures` policy (SecurityConfig) rejects unsigned plugins at discovery when enabled |

**Not done by design:** seccomp default-deny (breaks Python plugins via dynamic
loading) and Landlock (Linux-only, adds cross-platform complexity without
closing the hostile-code gap). **For hostile or untrusted plugins, run Jumbie
containerized** — the in-app sandbox bounds abuse and secret access, not
confinement. Prefer signed executables + container isolation over relying on
the in-app sandbox when the plugin is not trusted.

**Platform notes:** the per-plugin rlimits and process-group kill are
Unix-only (`pre_exec`; Windows has no pre-exec hook and `child.kill()` there is
TerminateProcess — the child dies but grandchildren may survive). Environment
isolation, IPC auth, the hello handshake, bounded stderr, and the persistent
PID registry are cross-platform; the orphan sweep kills stale processes with
SIGTERM on Unix and `taskkill /F` on Windows.

---

## 9. Configuration

### PluginConfig Trait

```rust
pub trait PluginConfig: Sized + Serialize + DeserializeOwned + Clone {
    fn plugin_id() -> &'static str;
    fn default() -> Self;
    fn validate(&self) -> Result<(), String>;
    fn is_enabled(&self) -> bool;
    fn merge_with_defaults(&self) -> Self;
    fn series_identifier_label() -> Option<&'static str>;
    fn description() -> &'static str;
}
```

### Config Storage

Plugin configs are stored in the `plugin_instances` DB table (one row per instance), not in `config.toml`. Columns: `id` (composite key), `category`, `plugin_id`, `instance_id`, `data` (JSON blob).

### Manifest File (External Plugins)

```json
{
    "display_name": "My Plugin",
    "executable": "my-plugin-binary",
    "public_key": "<optional base64 public key>",
    "signature": "<optional base64 signature>"
}
```

#### Security Constraints

- **`executable` must be a simple filename** — no path separators (`/`, `\`) or `..` components. The host resolves it relative to the plugin's own directory. This prevents directory traversal attacks.
- **Unknown fields are rejected** — the host uses `#[serde(deny_unknown_fields)]`, so any unrecognized JSON keys cause the manifest to fail loading. This catches typos and injection attempts.

#### Field Details

| Field | Required | Description |
|-------|----------|-------------|
| `display_name` | ✅ | Human-readable name shown in the UI |
| `executable` | ✅ | Simple filename (e.g. `"run.py"`, `"plugin.bin"`). Must not contain `/`, `\`, or `..` |
| `public_key` | ❌ | Base64url-encoded Ed25519 public key (32 bytes). If present, `signature` must also be present |
| `signature` | ❌ | Base64url-encoded Ed25519 signature (64 bytes) over the executable's BLAKE3 hash |

The `public_key` and `signature` fields are optional. When both are present, the host verifies the executable's hash before spawning. If only one is set, loading fails with a validation error.

The plugin manager scans subdirectories of `plugins.directory` for `manifest.json`. Discovery: read manifest → validate executable name (rejects path separators) → verify executable exists → (optional) verify signature → spawn a transient probe → `get_info` (type-level) → resolve ID collisions by version (higher wins). On sync, one shared process per type is spawned, each instance's config is pushed via `set_config`, and the instance is wrapped in `PolicyPlugin` (full sequence in §3).

---

## 10. Existing Plugins

| Plugin | Category | Key |
|---|---|---|
| **qBittorrent** | Downloader | `downloader.qbittorrent` |
| **Nyaa** | Source | `source.nyaa` |
| **Basic RSS** | Source | `source.basic_rss` |
| **Discord** | Notifier | `notifier.discord` |
| **TVMaze** | Metadata | `metadata.tvmaze` |
| **TVDB** | Metadata | `metadata.tvdb` |

External dummy examples in `examples/plugins/dummy-rust/` (Rust) and `examples/plugins/dummy-python/` (Python) demonstrate the full plugin lifecycle.

---

## 11. Building Your Own Plugin (Step-by-Step)

### External Plugin in Python (Minimal)

1. Create a directory under your `plugins_dir`:

```
plugins/my-plugin/
├── manifest.json
└── run.py
```

2. Write `manifest.json`:

```json
{
    "display_name": "My Custom Source",
    "executable": "run.py"
}
```

> `executable` must be a simple filename — no `./` prefix, no path separators, no `..`. It is resolved relative to the plugin's own directory.

3. Write `run.py` — a minimal plugin that stubs all required methods:

```python
#!/usr/bin/env python3
import os, sys, json

JUMBIE_SECRET = os.environ.get("JUMBIE_PLUGIN_SECRET")

# One process serves ALL instances of this plugin type: instance_id → config.
instances = {}

def send_response(req_id, result, auth=None):
    if req_id is None:
        return  # Notification — no response expected
    resp = {"jsonrpc": "2.0", "result": result, "id": req_id}
    if auth:
        resp["auth"] = auth
    print(json.dumps(resp), flush=True)

def send_error(req_id, code, message, auth=None):
    if req_id is None:
        return
    resp = {"jsonrpc": "2.0", "error": {"code": code, "message": message}, "id": req_id}
    if auth:
        resp["auth"] = auth
    print(json.dumps(resp), flush=True)

def main():
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        req = json.loads(line)
        method = req.get("method")
        rid = req.get("id")
        instance_id = req.get("instance_id")
        # Normalize null/missing params to empty dict SSoT — all handlers
        # below can safely call params.get(...) without None-guards.
        raw_params = req.get("params")
        params = raw_params if isinstance(raw_params, dict) else {}
        req_auth = req.get("auth")  # Echo back in responses

        # ── Auth check (optional, mirroring PluginServer behavior) ─────
        if JUMBIE_SECRET and req_auth != JUMBIE_SECRET:
            send_error(rid, -32001, "Authentication failed", req_auth)
            continue

        # ── Startup handshake (required — see §3) ──────────────────────
        # `hello` must echo the protocol version; without it the host
        # rejects the process at startup.
        if method == "hello":
            send_response(rid, {"protocol_version": 3}, req_auth)
            continue

        # ── Instance lifecycle (generic — one process, many instances) ──
        # `set_config` is idempotent: config is INPUT, so creating and updating
        # an instance are the same RPC. There is no "reconfigure" logic.
        if method == "set_config":
            instances[instance_id] = params
            send_response(rid, True, req_auth)
            continue
        if method == "shutdown_instance":
            instances.pop(instance_id, None)
            send_response(rid, True, req_auth)
            continue

        # ── Type-level methods (no instance_id) ─────────────────────────
        if method == "get_info":
            send_response(rid, {
                "display_name": "My Custom Source",
                "version": "1.0.0",
                "author": "Me",
                "description": "Fetches entries from my custom source",
                "capabilities": ["feed_provider"],
                "supported_protocols": None,
                "rate_limit": {"requests_per_minute": 30, "burst": 2}
            }, req_auth)
        elif method == "get_config_schema":
            send_response(rid, {
                "type": "object",
                "properties": {
                    "feed_url": {"type": "string", "description": "RSS feed URL"},
                    "refresh_interval": {"type": "integer", "default": 10}
                },
                "required": ["feed_url"]
            }, req_auth)
        elif method == "validate_config":
            errors = []
            if not params.get("feed_url"):
                errors.append("feed_url is required")
            result = True if not errors else {"errors": errors}
            send_response(rid, result, req_auth)
        elif method == "health_check":
            send_response(rid, "ok", req_auth)
        # ── Instance-level methods (routed by instance_id) ──────────────
        elif method == "fetch_entries":
            cfg = instances.get(instance_id, {})
            send_response(rid, [], req_auth)
        elif method == "search":
            # Envelope: report the exact query string(s) sent (here: the raw query).
            send_response(rid, {"entries": [], "queries": [params.get("query", "")]}, req_auth)
        elif method == "auto_search":
            # { series_title, season, episodes, aliases, keys }
            # (`episodes` are already in source numbering — offset applied; `keys` is
            # one rendered search key per episode.)
            # Envelope: report the exact query string(s) you sent to your indexer.
            send_response(rid, {"entries": [], "queries": []}, req_auth)
        else:
            send_error(rid, -32601, f"Unknown method: {method}", req_auth)

if __name__ == "__main__":
    main()
```

4. Make it executable and restart Jumbie. Your plugin will appear in the Plugins page.

### External Plugin in Rust (using the SDK)

1. Create a new binary crate:

```sh
cargo init --name my-plugin plugins/my-plugin
```

2. Add the SDK dependency to `Cargo.toml`. If your plugin lives in a subdirectory of the project, use a relative path; otherwise publish the SDK or use a git dependency:

```toml
[dependencies]
plugin-sdk = { path = "../../crates/plugin-sdk", features = ["server"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tokio = { version = "1", features = ["full"] }
tracing-subscriber = "0.3"
```

> **Note:** If your plugin lives inside the project repo (e.g. `plugins/my-plugin/`), add it as a workspace member in the root `Cargo.toml` so it builds with the rest of the project. Alternatively, build standalone from within its own directory:
> ```sh
> cd plugins/my-plugin && cargo build --release
> ```

3. Implement your plugin:

```rust
use plugin_sdk::*;
use serde_json::Value;

struct MyPlugin;

#[async_trait::async_trait]
impl PluginHandler for MyPlugin {
    async fn get_info(&self) -> PluginTypeInfo {
        PluginTypeInfo {
            display_name: "My Plugin".into(),
            version: "1.0.0".into(),
            author: "Me".into(),
            description: "A custom Rust plugin".into(),
            capabilities: vec![Capability::FeedProvider],
            supported_protocols: None,
            series_identifier_label: None,
            series_identifier_placeholder: None,
            rate_limit: Some(RateLimit { requests_per_minute: 30, burst: 2 }),
            supports_test: false,
        }
    }

    async fn get_config_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "api_key": {"type": "string", "description": "API key"}
            }
        })
    }

    async fn validate_config(&self, _config: Value) -> Result<(), Vec<String>> {
        Ok(())
    }

    async fn health_check(&self) -> Result<(), anyhow::Error> {
        Ok(())
    }

    async fn handle(&self, ctx: CallContext<'_>) -> Result<Value, anyhow::Error> {
        // ctx.config is this instance's config (INPUT); ctx.cache holds
        // memoized derived state (cleared automatically on set_config).
        let _cfg = ctx.config;
        match ctx.method {
            "fetch_entries" => Ok(serde_json::json!([])),
            "search" => Ok(serde_json::to_value(
                plugin_sdk::query::SearchResponse::new(Vec::<serde_json::Value>::new(), vec![]),
            )?),
            "auto_search" => Ok(serde_json::to_value(
                plugin_sdk::query::SearchResponse::new(Vec::<serde_json::Value>::new(), vec![]),
            )?),
            // Structured "unsupported method" so the host skips it quietly
            // instead of retrying it as a transient failure.
            _ => Err(PluginError::MethodNotSupported(format!("Unknown method: {}", ctx.method)).into()),
        }
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .init();
    // ONE handler object serves ALL instances: type-level metadata plus the
    // handler. The server stores per-instance configs + caches and clears the
    // cache on set_config — there is no per-instance plugin object and no
    // reconfigure logic.
    let plugin = MyPlugin;
    let info = plugin.get_info().await;
    let server = PluginServer::new(info, Box::new(plugin));
    server.run().await;
}
```

4. Create `manifest.json`:

```json
{
    "display_name": "My Plugin",
    "executable": "my-plugin"
}
```

5. Build and restart Jumbie:

```sh
cd plugins/my-plugin && cargo build --release
cp target/release/my-plugin ./
# Restart Jumbie for it to discover the new plugin
```

---

## 12. Testing a Plugin

Test the JSON-RPC interface directly with a subprocess:

```sh
# Start the plugin in the background and capture its stdout to a temp file
./plugins/my-plugin/run.py > /tmp/plugin-test-out.json 2>/tmp/plugin-test-err.log &
PLUGIN_PID=$!
sleep 0.1

# Send get_info via stdin (use a temp file piped in for reliability)
echo '{"jsonrpc":"2.0","method":"get_info","params":null,"id":1}' > /tmp/plugin-test-req.json
cat /tmp/plugin-test-req.json > /proc/$PLUGIN_PID/fd/0 2>/dev/null || {
  # Fallback: use a named pipe (FIFO)
  kill $PLUGIN_PID
  mkfifo /tmp/plugin-fifo 2>/dev/null
  cat /tmp/plugin-fifo | ./plugins/my-plugin/run.py &
  PLUGIN_PID=$!
  echo '{"jsonrpc":"2.0","method":"get_info","params":null,"id":1}' > /tmp/plugin-fifo
  sleep 0.2
  rm -f /tmp/plugin-fifo
}

# Kill the test process
kill $PLUGIN_PID 2>/dev/null
wait $PLUGIN_PID 2>/dev/null

# Read the response
cat /tmp/plugin-test-out.json
```

For plugins using the Rust SDK with `PluginServer`, the host's log output will show plugin registration success or error details.

### `submitter` (release group) contract

**Plugins are responsible for providing `submitter`.** The system trusts the plugin's value and never re-derives it downstream.

The plugin has two options:

1. **Explicit field:** If your source has a dedicated uploader/submitter field (e.g. a private tracker API), set `submitter` directly from that field.
2. **Derive from title:** If the release group is embedded in the title (e.g. `[SubsGroup] Show - 01`, `Show.S01E01-GRP`), call `entry.resolve_submitter()` in your `parse_entries`. This helper is available in `jumbie_shared::types::media::MediaEntry::resolve_submitter()` — it calls `extract_submitter` from `jumbie_shared::parsing`.

Once set, the value flows through the pipeline as an opaque `Option<String>` — downstream code never calls `extract_submitter`:

```
Plugin → entry.resolve_submitter()        ← plugin calls this in parse_entries
         ↓
from_plugin_response()                     ← just deserializes, no auto-resolution
         ↓
SearchResult.submitter                    ← carried through by to_search_result()
         ↓
ReleaseCandidate.submitter                ← read from entry.submitter
         ↓
EnqueueDownloadParams → download_queue   ← stored on the queue row
         ↓  (finalize time)
set_release_info_by_path()                ← written to release_info (keyed by content hash);
                                           first-write-wins for submitter
         ↓
EpisodeDetailRow.submitter               ← COALESCE(NULLIF(rm.submitter, ''), dq.submitter)
                                           (empty-string guard so legacy rows fall through)
```

Submitter is a **property of the file** — it follows the file's fingerprint into `release_metadata`. If a file is copied or moved to another episode, its submitter travels with it. The `download_queue` row holds it as a bridge until finalize time.

If the plugin provides `None` (doesn't call `resolve_submitter` and doesn't set it explicitly), the system treats it as "unknown". No fallback occurs — the plugin's value (or absence) is the final word.

For non-plugin paths (scanner, manual `add_download`), submitter is derived at their own entry boundary by calling `extract_submitter` directly, then follows the same opaque-carriage contract.

### `MediaEntry` fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `title` | `String` | Yes | Release title. Used for episode identification and scoring. |
| `guid` | `Option<String>` | No | Feed item GUID. Used for deduplication. |
| `link` | `Option<String>` | No | Original feed item link (torrent info page). |
| `published` | `Option<DateTime<Utc>>` | No | RSS `pubDate` — used for age-based scoring and release date display. |
| `download_url` | `Option<String>` | No | Actual download URL (magnet URI or `.torrent` URL). |
| `download_id` | `Option<String>` | **Yes** (for downloads) | Unique download identifier (e.g. BTIH info hash). Required for queueable items. |
| `size` | `Option<u64>` | No | File size in bytes. Used for scoring. |
| `description` | `Option<String>` | No | Release description text. |
| `file_list` | `Vec<String>` | No | List of files in the release (torrent contents). |
| `seeders` | `Option<u32>` | No | Current seeder count. Used for scoring. |
| `leechers` | `Option<u32>` | No | Current leecher count. |
| `category` | `Option<String>` | No | Category identifier from the source (e.g. Nyaa category ID). |
| `source` | `String` | **Yes** | Human-readable source plugin name — shown in the frontend as a source badge. Must be set by every source plugin. |
| `submitter` | `Option<String>` | No | Release group / submitter name. Set explicitly by the plugin or via `resolve_submitter()`. If `None`, treated as unknown — system never re-derives. |

## 13. Plugin Data Validation Architecture

All data crossing the plugin→backend boundary is validated at three layers:

### Layer 1 — Plugin Bridge (`backend/src/plugins/bridge.rs`)

Every plugin method call is routed through typed bridge functions that:
1. Call the plugin via `PluginInstance::call()`
2. Deserialize the JSON response to the expected Rust type
3. Call `.validate()` on the deserialized value
4. Return `BridgeError` on failure (with `Call`, `Deserialization`, or `Validation` variants)

The bridge is the SSoT for which type each method returns and what validation is applied.
Source `search` / `auto_search` responses are the `SearchResponse<Vec<MediaEntry>>`
envelope (`{ entries, queries }`): the bridge validates/filters the entries and
reports the plugin-declared `queries` into the canonical `search.query` log line.

Item **counts** are bounded as well as per-item sizes: every list-returning method
(`search`, `auto_search`, `fetch_entries`, `get_completed_downloads`,
`fetch_series_aliases`, `get_updated_series`) is capped at `MAX_PLUGIN_ITEMS`
(10,000) and plugin-reported query lists at `MAX_SEARCH_QUERIES` (100). Exceeding a
cap truncates the list and logs a `warn`, so a runaway plugin cannot exhaust
resources with an enormous response.

### Layer 2 — Validate trait (`backend/src/validation/plugin_data.rs`)

Every plugin-boundary struct implements the `Validate` trait. Impls delegate scalar
limits to the shared validators so changing a limit in one place updates all layers:

| SSoT Validator (in `jumbie_shared::validation::fields`) | Used by |
|---|---|
| `validate_season_number` | `EpisodeMetadata`, `SeasonMetadata`, `insert_episode`, `set_episode_status`, `assign_file_to_episode`, notifier context |
| `validate_episode_number` | `EpisodeMetadata`, `insert_episode`, `set_episode_status`, `assign_file_to_episode`, notifier context |
| `validate_url` | `SeriesMetadataInfo.image_url` |
| `validate_runtime` | User-facing runtime inputs (minutes) — NOT episode runtime (seconds) |
| `validate_title` | `SeriesMetadataInfo.name` |

### Layer 3 — DB gate (`backend/src/db/episodes/crud.rs`)

`insert_episode` and `set_episode_status` call `validate_season_number` and
`validate_episode_number` before any SQL write. This catches all paths that
bypass the bridge (e.g. direct API endpoints, internal flows).

### Validation rules per plugin type

| Plugin type | What's validated | Reject or warn |
|---|---|---|
| **Metadata** | `EpisodeMetadata` (season, episode, title, runtime, image_url), `SeasonMetadata` (season, episode_count), `SeriesMetadata` (duplicate seasons), `SeriesMetadataInfo` (name, image_url, aliases) | **Reject** — full batch fails on any invalid entry |
| **Source** | `MediaEntry` (title, source, size ≤1TB, seeders ≤1M, link scheme) | **Filter** — invalid entries are logged and skipped; valid entries proceed |
| **Downloader** | `add_download` URL (non-empty), completed download identifiers (non-empty, ≤128 chars — opaque strings, not necessarily hashes), content paths and download paths (non-empty, ≤4096 chars), progress (0.0..1.0), status token (≤256 chars, no control chars), failure reason (sanitized: trimmed, control chars → spaces, ≤1024 chars), connection test message (non-empty, ≤4096 chars, no control chars) | **Reject** for individual method calls — except the failure reason, which is **sanitized and still honored** (it is a terminal signal) |
| **Notifier** | Context field presence per event type (`series_title` + `release_title` required for downloads) + content validation (season/episode ranges, size ≤1TB) | Presence: **reject** before dispatch. Content: **warn** only |

---

## 14. Best Practices

- **Logging:** External plugins write to stderr; internal plugins use `tracing`.
  Stderr forwarding is type-level (one process per type); filter per-call host
  logs on `instance=<id>` for instance-scoped visibility (see §3).
- **Timeouts:** External calls 30s, `health_check` 10s, `test` 10s. 3 consecutive health check failures kill and restart the plugin.
- **Shutdown:** External plugins should exit on stdin EOF. Host controls lifecycle via `CancellationToken`.
- **Rate limits:** Declare realistic limits in `PluginTypeInfo`. Prefer `burst = 1` or `2`.
- **Classify failures:** Return the most specific signal (`-32030`/`AuthFailed`, `-32029`/`RetryAfter`, `-32031`/`Transient`, `-32032`/`Permanent`); unclassified errors are transient. See §8.
- **Plugin ID stability:** Once released, don't change `author` or `display_name` — the derived ID is used for config lookup.
- **Config validation:** Validate each field independently so users can fix all issues at once.
- **Security:** External plugins run as the same user as the backend. Do not run untrusted plugins.
- **Provide `download_id`:** Sources must set `download_id` on every `MediaEntry` — the backend uses it to track the download in the client. RSS sources with only magnet links (no separate `<nyaa:infoHash>`) must parse the BTIH hash from the magnet URI.
- **`link` vs `download_url`:** `link` is the original feed item link (torrent info page); `download_url` is the actual download URL, which may be a magnet URI or a `.torrent` URL. Sources put their magnet URIs or `.torrent` URLs in `download_url`.
- **Submitter ownership:** Set `submitter` on each `MediaEntry` — explicitly, or via `entry.resolve_submitter()` when the source has no uploader field. If left as `None`, the system treats it as "unknown" — no auto-resolution occurs (see §12).
- **`resolve_submitter()` helper:** Available on `MediaEntry` for Rust plugins. External plugins can call `extract_submitter` from the Python/Rust SDK (`jumbie_shared::parsing`). Call it in your `parse_entries` after constructing each `MediaEntry`.
