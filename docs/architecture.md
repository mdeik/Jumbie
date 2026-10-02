# Jumbie — Architecture Guide

---

## Table of Contents

1. [Project Overview](#1-project-overview)
   - [Technology Stack](#technology-stack)
   - [Workspace Structure](#workspace-structure)
2. [Data Flow — Download Pipeline](#2-data-flow--download-pipeline)
3. [Key Subsystems](#3-key-subsystems)
   - [Source Processor](#source-processor)
     - [Gate Pre-Filter](#gate-pre-filter)
     - [Phase 1: Pattern Identification](#phase-1-pattern-identification)
     - [Phase 2: Filename Fallback](#phase-2-filename-fallback)
   - [Download Orchestrator](#download-orchestrator)
   - [File Organization](#file-organization)
   - [Metadata System](#metadata-system)
   - [Plugin System](#plugin-system)
   - [Background Tasks](#background-tasks)
   - [Authentication & Security](#authentication--security)
4. [Configuration Model](#4-configuration-model)
5. [Frontend Architecture](#5-frontend-architecture)
   - [Module Structure](#module-structure)
6. [Database Schema](#6-database-schema)
7. [Shutdown & Graceful Stop](#7-shutdown--graceful-stop)
8. [Memory Management](#8-memory-management)
   - [Why jemalloc](#why-jemalloc)
   - [Decay Configuration](#decay-configuration)
   - [Thread Pool Sizing](#thread-pool-sizing)
   - [Monitoring](#monitoring)
   - [Memory Profile (Measured)](#memory-profile-measured)
   - [Logging (size-bounded)](#logging-size-bounded)

---

## 1. Project Overview

Jumbie is a self-hosted media organization tool for series and episode content. It polls sources (RSS feeds, torrent indexers), identifies matching episodes, manages downloads, and renames/moves files into a library structure.

### Technology Stack

| Component     | Technology                              |
|---------------|-----------------------------------------|
| Language      | Rust                                     |
| Async Runtime | Tokio                                   |
| HTTP Server   | Axum                                    |
| HTTP Client   | reqwest (rustls-tls)                    |
| Database      | SQLite via sqlx with async migrations   |
| Frontend      | Leptos CSR (WASM), built with Trunk     |
| Configuration | TOML (config crate)                     |
| CLI           | clap                                     |
| Logging       | tracing + file appender                 |

### Workspace Structure

```
jumbie/
├── backend/             # Server binary & domain logic
│   └── src/
│       ├── api/         # Axum router + handler registration
│       ├── api_routes/  # Endpoint handler implementations
│       ├── cli/         # CLI argument parsing, setup, server, task registration
│       ├── datetime/    # Date/time utility types
│       ├── db/          # SQLite data access layer
│       ├── download_orchestrator/ # Download lifecycle & completion handling
│       ├── file_manager/# File rename/move orchestration
│       ├── middleware/  # Auth, rate limiting, security headers
│       ├── models/      # Domain models (MediaEntry, ReleaseCandidate, activity)
│       ├── plugins/     # Plugin host, manager, sandbox, registry
│       ├── scanner/     # Directory scanning & file fingerprinting
│       ├── source_processor/ # Source polling, identification, scoring
│       ├── tests/       # Integration & behavior tests
│       ├── utils/       # HTTP client, media info, path utils
│       └── validation/  # Collision detection, plugin data validation
├── crates/
│   ├── shared/          # Core types, config, parsing, mapping, scoring, quality,
│   │                    # filtering, formatting, validation, plugin config, media formats
│   └── plugin-sdk/      # Plugin author SDK (RPC types, traits, server)
├── frontend/            # Leptos SPA (compiled to WASM)
├── examples/plugins/    # External plugin examples (Python, Rust dummies)
└── docs/                # Documentation
```

---

## 2. Data Flow — Download Pipeline

The core pipeline transforms a raw source entry into an organized file:

1. **Source polling** → `MediaEntry` — Sources (RSS, Nyaa) return entries with title, link, size.
2. **Gate pre-filter** — A single `RegexSet` checks if the title matches ANY series' name,
   alias, or pattern. Entries for untracked series are skipped immediately — no further work.
3. **Phase 1: Pattern identification** — Custom regex patterns (user-defined or auto-generated
   from names/aliases) are tried against the raw title. A match identifies the series directly
   without `parse_filename` or a mapping lookup. If the pattern has capture groups, episode
   info is extracted directly; otherwise `parse_filename` runs for values only.
4. **Phase 2: Filename parsing fallback** — `parse_filename()` extracts series, season, episode
   from the title. The series is found via `lookup_mapping_in()` — an in-memory substring match
   against the already-loaded mapping set.
5. **Season mapping** — Season-level aliases, episode offsets, and patterns are resolved.
6. **Filter application** — Global + per-series filters merge; matched releases are scored.
7. **Scoring** — `ReleaseProfile` assigns points for resolution, source type, codec.
8. **Winner selection** — Best-scoring release for each wanted episode is chosen.
9. **Queueing** → Download client — The winner is sent to qBittorrent or another client.
10. **File watching** → Stability check → Fingerprinting — Downloads are monitored for completion.
11. **Organization** — The completed file is renamed per template and moved to the library.

Steps 2–4 share one precompiled `SeriesMatcher` built from the pre-loaded `all_mappings`
HashMap, so the gate and every series' Phase 1 patterns are compiled once per sync rather than
per entry.

---

## 3. Key Subsystems

### Source Processor

The source processor (`source_processor/`) polls configured sources for new entries, identifies matching series, scores releases, and queues winners for download. It runs on a configurable schedule.

All feed entries are processed through **three phases** that share a single pre-loaded mapping set:

#### Gate Pre-Filter

Before any per-series work, `SeriesMatcher` builds a `RegexSet` with one pattern per series,
covering every name, alias, and (named-group-stripped) user pattern. A title that matches no
pattern in the set is skipped entirely — and because the set reports *which* patterns matched, a
hit narrows Phase 1 to the candidate series instead of re-scanning the whole library. For noisy
feeds where most entries are for untracked series, this eliminates the vast majority of per-entry
work.

The set includes:
- **Auto-generated patterns** from every series' name, target_title, and aliases — always
  case-insensitive, with dot↔space flexible separators.
- **User-defined patterns** (filter and extraction) with named groups stripped to `(?:...)` so they
  can be combined without conflicting names.
- Source-scoped patterns with their `@source:` prefix stripped.

Each series' alternatives are wrapped in `(?:...)` so regex flags (`(?i)`, `(?m)`, `(?s)`, etc.) stay
scoped to that series. Invalid patterns are validated individually and skipped so they don't break
the whole set. If the set exceeds the regex size limit, it degrades to no gate and the system falls
through to Phase 1 + Phase 2.

#### Phase 1: Pattern Identification

`SeriesMatcher::identify` tests each candidate series' precompiled patterns against the raw title.
For series with user-defined `reg_patterns`, those patterns are used as-is. For series without,
patterns are generated from `name`, `target_title`, and `search_aliases()` via
`generate_auto_pattern(name)` — which escapes all regex meta-characters and replaces spaces with
`[._ ]` for dot↔space flexibility. Filter-only patterns are combined into a single alternation;
extraction patterns are pre-screened with a `RegexSet` so `captures` runs only on patterns that can
match.

If a pattern matches with capture groups (`(?P<episode>...)` / `(?P<season>...)`), the episode info
is extracted directly. If it matches without capture groups (filter-only), `parse_filename` runs
for values only — the series is already identified, so no mapping lookup is needed.

#### Phase 2: Filename Fallback

Only reached when no pattern (user or auto-generated) matched. `parse_filename()` extracts episode
info using its static regex library. `lookup_mapping_in()` finds the series via substring matching
against `name`/`target_title`/`aliases` — identical semantics to `get_mapping_by_key` but operates
on the already-loaded in-memory HashMap.

`lookup_mapping_in` is the Single Source of Truth for inline mapping lookup. It is also delegated
to by `get_mapping_by_key` in `queries.rs`, ensuring alias/name matching semantics live in one place.

### Download Orchestrator

The download orchestrator (`download_orchestrator/`) manages the lifecycle of a download: adding to the client, polling for completion (with backoff), verifying file stability, fingerprinting, and triggering organization. It also handles retries for failed downloads.

### File Organization

The file manager (`file_manager/`) handles file renaming, moving, collision detection, and library path planning. Organization follows user-defined templates for season folders and episode filenames, supporting conditional blocks, zero-padding, and media info variables.

### Metadata System

Metadata providers (TVDB, TVMaze) fetch episode titles, air dates, descriptions, and season structures. Each series can have multiple metadata source IDs (one per provider). Metadata enrichment is queued and applies to episodes that have no title.

### Plugin System

Jumbie supports two plugin architectures:
- **Internal plugins** — Compiled into the binary (e.g., Nyaa source, qBittorrent downloader, Discord notifier, TVDB/TVMaze metadata).
- **External plugins** — Standalone processes communicating via stdin/stdout JSON-RPC 2.0. Any language supported.

Internal plugins implement `PluginInstance` directly (see plugin specification). External plugins use the `plugin-sdk` crate or implement the JSON-RPC protocol manually. All plugins declare capabilities from the following set:

| Capability | Purpose |
|---|---|
| `FeedProvider` | Periodic feed polling (RSS, Nyaa) |
| `Downloader` | Sending downloads to a client |
| `Notifier` | Sending notifications (Discord) |
| `MetadataProvider` | Generic metadata fetching |
| `MetadataProviderNormal` | Season-relative episode metadata |
| `MetadataProviderAbsolute` | Absolute-numbered episode metadata |
| `Polling` | Supports periodic refresh — source feed polling or metadata refresh |
| `ManualSearch` | Source supports manual search from the UI |
| `AutomaticSearch` | Source supports automatic missing-episode search |
| `CanPauseResume` | Download client supports pause/resume |
| `CanSeed` | Download client supports seeding |
| `FetchSeriesTitle` | Metadata provider can fetch canonical series title |
| `FetchSeriesAliases` | Metadata provider can fetch alternative titles |

Capabilities are **type-level** (`plugin_info().capabilities`): they say what a plugin *type* supports, and are identical for every instance of that type. Some capabilities are additionally gated by a **per-instance config toggle**, so a capability is only effective when the type declares it **and** the user has enabled it for that instance:

| Config toggle | Capability it gates |
|---|---|
| `enable_polling` | `Polling` (source polling and metadata refresh loops) |
| `enable_manual_search` | `ManualSearch` |
| `enable_automatic_search` | `AutomaticSearch` |
| `enable_seeding` | `CanSeed` |

The mapping lives in one place (`plugins::capabilities`) and is applied at the single dispatch chokepoint (`PluginManager::get_plugins_by_all_capabilities`), which every capability lookup goes through. A toggle can only *disable* a declared capability — it can never grant one the type doesn't support. Metadata providers (TVDB, TVMaze) declare `Polling` so the metadata refresh loop can dispatch to them when `enable_polling` is on.

Plugins are wrapped with a shared failure-policy decorator (`PolicyPlugin`) that applies rate limiting, retry with backoff, and failure cooldowns (auth/transient) — see the plugin specification §8.

External plugins run **one shared process per plugin type** — all instances of a type live inside that process, routed by `instance_id` on every JSON-RPC request. Adding/removing an instance is an RPC, never a spawn, and config is **input**: pushing a new config is the same idempotent `set_config` RPC that creates an instance. Requests travel through an unbounded queue owned by the lifecycle task (a slow or hung instance never drops a request), and after a crash the lifecycle task respawns the process and replays `set_config` for every live instance from its cached config.

Config saves are **targeted**: only instances whose config changed are touched (diffed against what each was last built with). Changed instances get `set_config` — plugins never implement "reconfigure" logic; derived state (tokens, transports) is memoized against the config and re-derived when it changes. A failed `set_config` marks the instance Failed (keeping its old config) — there is no rebuild/re-init fallback, because a rebuild would fail with the same config. The `PolicyPlugin` wrapper stores the backend-managed host fields (priority/enabled/refresh_interval) and one shared token bucket is enforced per TYPE (the declared rate limit describes the upstream endpoint all instances share). Health is type-level: one probe per process, per-instance failure state read in-process — the status page performs zero plugin RPCs. See the plugin specification §3/§7/§8.

The following plugins are compiled into the binary and registered at startup:

| Name | Type | Description |
|---|---|---|
| Nyaa | Source | Nyaa.si Torrent Indexer |
| Basic RSS | Source | Basic RSS Feed |
| qBittorrent | Downloader | qBittorrent Web API client |
| Discord | Notifier | Discord webhook notifications |
| TVMaze | Metadata | TVMaze Metadata Provider |
| TVDB | Metadata | TheTVDB Metadata Provider (V4 API) |

### Background Tasks

A `TaskRegistry` manages recurring background tasks (source polling, metadata sync, queue processing, etc.). Each task receives a `CancellationToken` for graceful shutdown. Tasks can adapt their polling intervals dynamically.

### Authentication & Security

Authentication supports HTTP Basic (with admin password) and Bearer tokens (scoped API keys). Scopes form a write-implies-read hierarchy. IP-based banning kicks in after configurable failed auth attempts with exponential backoff. Security headers (CORS, CSP, X-Frame-Options) are configurable but disabled by default to avoid conflicts with reverse proxies.

The client IP is resolved **once** per request (honouring `trusted_proxies` / `X-Forwarded-For`) and shared via request extensions, so banning, rate limiting, and the localhost/subnet bypasses all key on the same identity. The ban list and rate-limiter maps are bounded by a background reaper, and the admin password hash / Argon2 verification / API-key lookups are short-lived cached. See [authentication.md](authentication.md) for the full model.

---

## 4. Configuration Model

Configuration is split across two storage tiers:

- **`config.toml`** — Bootstrap config with paths only (database, plugins, logs, tmp). Read once at setup, everything else lives in the DB.
- **Database** — All user-facing settings: organization formats, source configs, quality/release profiles, UI preferences, auth config, security settings.

---

## 5. Frontend Architecture

The frontend is a Leptos CSR (WASM) application with lazy-loaded routes and shared state via a singleton `ApiClient`. Key patterns:

- **`ApiClient`** — Singleton HTTP client with automatic base URL resolution, typed request builders, and error handling.
- **`API_CACHE`** — Thread-local stale-while-revalidate cache layer for GET responses (paginated data, config) to reduce server load and deduplicate in-flight requests.
- **Autosave** — Form fields save changes with debounced PUT requests.
- **Preloading** — Navigation sections trigger data preloads for the next likely page.
- **Theme** — Auto/Light/Dark theme detection with CSS class switching.

### Module Structure

Routes mirror the backend's logical grouping (series, calendar, system, settings, etc.). Components are organized by feature domain (authentication, series library, profiles, plugins, system).

---

## 6. Database Schema

Key tables:

| Table | Purpose |
|---|---|
| `series_mappings` | Central series registry — title, aliases, settings, paths |
| `episodes` | Episode metadata per series — season/ep num, title, air date |
| `episode_parts` | Multi-part episode file references |
| `file_contents` | Content identity via fingerprint (xxhash) — media info, original path, retention |
| `file_paths` | Paths seen for a content fingerprint (inode/mtime/state, episode link) |
| `file_event_log` | Audit trail for file operations |
| `download_queue` | Pending / active / completed downloads |
| `quality_profiles` | Named quality sets with upgrade rules |
| `release_profiles` | Scoring rules with weighted terms |
| `plugin_instances` | Plugin config storage per instance |
| `api_keys` / `calendar_tokens` | Auth tokens |
| `banned_ips` | Persistent ban tracking |
| `automatic_profile` (+ `automatic_profile_records` / `_unknown_files` / `_media_scans`) | Auto quality escalation rules (per-submitter scoring) |
| `system_state` | Server-level key-value state |
| `config_defaults` | DB-stored config sections |

---

## 7. Shutdown & Graceful Stop

Jumbie handles three shutdown signals:

1. **SIGINT** (Ctrl+C) — Standard user-initiated shutdown.
2. **SIGTERM** — Container/orchestrator shutdown signal (Docker, systemd).
3. **System tray quit** — Desktop application exit.

All signals flow through a shared `CancellationToken` that cascades to background tasks, plugin processes, and the HTTP server. The shutdown sequence is:

1. HTTP server stops accepting connections (graceful drain).
2. Plugin manager shuts down external processes.
3. `ScanQueue` and `MetadataQueue` are shut down.
4. `CancellationToken` is signalled — all background tasks exit their loops.
5. `TaskRegistry::cancel()` + `wait_all()` awaits all task handles.
6. Process exits.

## 8. Memory Management

Jumbie uses **jemalloc** as its global memory allocator (instead of the default glibc `malloc`).

### Why jemalloc

In a long-running multi-threaded process, glibc creates per-thread memory arenas that cache freed pages indefinitely — never returning them to the OS. Over hours of background cycles (scanner, source processing, metadata refresh), this ratchets RSS to the peak of all concurrent allocations across all threads, regardless of whether that memory is still needed.

jemalloc solves this via:
- **Background threads** that periodically scan and purge unused pages.
- **Tunable decay rates** (`dirty_decay_ms`, `muzzy_decay_ms`) that control how quickly freed pages are returned to the kernel.
- **Transparent huge page support** for large allocations.
- **Built-in heap profiling** via the `mallctl` interface.

### Decay Configuration

The compiled-in defaults return freed pages to the OS within ~5 seconds:

```rust
// In alloc.rs — configure(), called from main() before the runtime starts:
unsafe {
    tikv_jemalloc_ctl::raw::update(b"dirty_decay_ms\0", &5000u64);
    tikv_jemalloc_ctl::raw::update(b"muzzy_decay_ms\0", &5000u64);
}
```

Background thread purging is enabled at compile time on Linux (the
`background_threads` feature of tikv-jemalloc-sys), so freed pages are returned
to the OS even when the process is idle. jemalloc only compiles background
threads for non-Mach-O ABIs, so on macOS the feature is intentionally not
requested and the decay rates above do the purging. The decay rates are the
only runtime tuning knobs; there is intentionally no per-container env override
(jemalloc itself reads `MALLOC_CONF`, which is not wired through).

### Thread Pool Sizing

All async tasks (HTTP handlers, background scanner, source processing, metadata refresh, queue workers) run on tokio's work-stealing scheduler. The number of async worker threads defaults to `min(4, CPU count)` — adapted from `std::thread::available_parallelism()`, which respects Docker `--cpus` limits:

| Container CPUs | Async Workers | Blocking Threads |
|---------------|---------------|------------------|
| 1             | 1             | 8                |
| 2             | 2             | 8                |
| 4+            | 4 (capped)    | 8                |

Set `JUMBIE_WORKER_THREADS` and `JUMBIE_BLOCKING_THREADS` to override:

```bash
# High-concurrency deployment (16 workers, matching host CPUs):
docker run -e JUMBIE_WORKER_THREADS=16 ...

# Minimal memory footprint (2 workers):
docker run -e JUMBIE_WORKER_THREADS=2 ...
```

### Monitoring

Heap statistics are exposed via `GET /api/system/memory`:

```json
{
  "jemalloc": {
    "active_bytes": 32960512,
    "allocated_bytes": 26778608,
    "decay_goal_ms": 5000,
    "mapped_bytes": 96256000,
    "overhead_bytes": 69477392,
    "resident_bytes": 50192384
  },
  "logs": {
    "buffer_budget_bytes": 4194304,
    "buffer_bytes": 4194020,
    "buffer_entries": 5806,
    "disk_bytes": 6348547
  },
  "note": "jemalloc returns freed pages to the OS within ~5 seconds via background thread decay",
  "peak_bytes": 896708608,
  "rss_bytes": 253751296
}
```

The `overhead_bytes` field (mapped − allocated) shows how much memory jemalloc is holding for future use but hasn't yet returned to the OS — under load this will spike temporarily, then drop as the background purging threads reclaim pages.

### Memory Profile (Measured)

With 2000 series, 500 ReleaseCandidates, and 6 scanner cycles:

| Metric | Measured Value |
|--------|----------------|
| Scanner cycle 1 | 0.19s, +12 MB RSS |
| Scanner cycle 2 | 0.02s, +0 MB RSS (cached) |
| Scanner cycle 3 | 0.02s, +0 MB RSS (cached) |
| 5 concurrent mapping loads | 0 MB (shared cache) |
| Source processing (500 candidates) | 2.4 MB |
| **Long-run RSS** | **~200-500 MB** (jemalloc decay) |

### Logging (size-bounded)

Logs are written with a **size-rolled** file scheme (`backend/src/logging.rs`)
and served from a **byte-bounded** in-memory ring (`LogRing` in
`crates/shared/src/types/log.rs`). Retention is purely size-based:

- **Destinations**: a size-rolled **file** appender and a **byte-bounded in-memory**
  ring are always active. Console (stdout) output is a development-only
  affordance: enabled in debug builds, suppressed for the release-mode server
  (so a supervised production deployment writes application logs to the log file
  only), and re-enabled for one-shot maintenance commands (`--vacuum`, …). The
  policy is the single `logging::console_enabled` predicate, and
  `logging::console_layer` is the only place a stdout writer is built. CI
  enforces this: direct print macros are denied crate-wide (`cfg_attr(not(test),
  deny(…))` in `src/lib.rs`/`src/main.rs`) and a grep rejects any raw
  stdout/stderr writer outside `logging.rs`.
- **Disk**: `jumbie.log` rolls at 2 MiB into `jumbie.log.1 … jumbie.log.3`;
  on rotation the oldest archives are deleted while the total exceeds
  **8 MiB**. Names outside the scheme are ignored (never read, served, or culled).
- **Memory**: the API buffer holds the newest **4 MiB** (by estimated entry
  bytes, not entry count), so memory is fixed even if a giant line/file
  appears. Startup pre-population reads newest-first and stops at the budget.
- **Volume**: `sqlx=info` in the default filter and hot-loop statements at
  `trace!` keep even TRACE deployments at ~1–2 MB/day.
- **API**: `GET /api/system/logs` serves the buffer (paginated, `min_level`);
  `GET /api/system/logs/file?tail=1000|head=1000&level=debug` serves the raw
  merged on-disk files (chronological, level-filtered, never arbitrary
  files, read capped per file).
- **Observability**: `GET /api/system/memory` includes a `logs` category
  (`buffer_bytes` vs `buffer_budget_bytes`, `buffer_entries`, `disk_bytes`)
  so a recurrence of the buffer blow-up is identifiable from the API in one
  call.
