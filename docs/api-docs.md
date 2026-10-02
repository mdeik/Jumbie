# Jumbie HTTP API Reference

**Base URL:** `http://<host>:<port>/api`

---

## Table of Contents

1. [Authentication](#1-authentication)
   - [Basic Auth (Admin Password)](#basic-auth-admin-password)
   - [Bearer Token Auth (API Keys)](#bearer-token-auth-api-keys)
   - [Authentication Bypass](#authentication-bypass)
   - [IP Banning](#ip-banning)
2. [Public Endpoints](#2-public-endpoints)
3. [Auth Endpoints](#3-auth-endpoints)
   - [Ban Management](#ban-management)
   - [API Keys](#api-keys)
   - [Calendar Tokens](#calendar-tokens)
4. [Series Endpoints](#4-series-endpoints)
   - [Series Read](#series-read)
   - [Series Write](#series-write)
5. [Search Endpoints](#5-search-endpoints)
6. [Queue & Download Endpoints](#6-queue--download-endpoints)
   - [Queue Read](#queue-read)
   - [Queue Write](#queue-write)
7. [System Endpoints](#7-system-endpoints)
   - [System Read](#system-read)
   - [Wanted Read](#wanted-read)
   - [Activity Read](#activity-read)
   - [Logs Read](#logs-read)
   - [Files Read](#files-read)
   - [Files Write](#files-write)
   - [Rename Read](#rename-read)
   - [Rename Write](#rename-write)
8. [Settings Endpoints](#8-settings-endpoints)
   - [Config Read](#config-read)
   - [Config Write](#config-write)
   - [Plugins Read](#plugins-read)
   - [Plugins Write](#plugins-write)
9. [Series Import Endpoints](#9-series-import-endpoints)
10. [Validation](#10-validation)
11. [API Key Scopes](#11-api-key-scopes)
12. [Common Response Formats](#12-common-response-formats)
    - [Paginated Response](#paginated-response)
    - [Error Response](#error-response)
    - [Series List Sorting](#series-list-sorting)
    - [API Key Generation Response](#api-key-generation-response)
    - [Calendar Token Generation Response](#calendar-token-generation-response)
    - [Add Download Response](#add-download-response)
13. [Notes](#13-notes)
    - [Timestamps](#timestamps)

---

## 1. Authentication

> For the full model — client-IP resolution, trusted proxies, the auth check
> order, banning, rate limiting, caching, and how the settings interact — see
> [authentication.md](authentication.md).

### Basic Auth (Admin Password)

```
Authorization: Basic <base64(username:password)>
```

If an admin password is configured, basic auth grants full access. The username is ignored.

### Bearer Token Auth (API Keys)

```
Authorization: Bearer jb_<key>
```

Scoped API keys limit access to specific operations. See [API Key Scopes](#11-api-key-scopes).

### Authentication Bypass

If no admin password is configured, all requests are granted full access.

### IP Banning

After repeated failed auth attempts, the offending IP is temporarily banned with configurable duration and exponential backoff. Bans are managed through the auth ban endpoints. A banned client receives `403` with `{"error": "IP is banned"}`. See [authentication.md § 6](authentication.md#6-ip-banning) for the full algorithm (escalation, forgiveness window, persistence, and interaction with bypasses).

---

## 2. Public Endpoints

### `GET /api/public/ping`

**Scope:** none

#### Input

None.

#### Output

```json
{
  "status": "ok"
}
```

* `status` — Always `"ok"`. A lightweight "is the server running?" check with zero I/O; monitoring and load balancers should hit this instead of the heavier `/api/health`.

### `GET /api/public/theme`

**Scope:** none

#### Input

None.

#### Output

```json
{
  "theme": "auto"
}
```

* `theme` — Current UI theme setting: one of `"auto"`, `"light"`, or `"dark"`. Falls back to `"auto"` if UI preferences cannot be read. Reachable before authentication so the login page can be rendered with the correct theme.

### `GET /api/calendar/ical`

**Scope:** none

#### Input

* `token` — query parameter. Required. A valid calendar token, generated via `POST /api/config/auth/calendar_tokens/generate` (a legacy config `calendar_token` is also accepted). A missing or invalid token returns `401` with a plain-text body (`Invalid or missing calendar token`), not the JSON error envelope.

#### Output

Non-JSON. Responds with `Content-Type: text/calendar; charset=utf-8` and `Content-Disposition: attachment; filename="calendar.ics"`; the body is an iCalendar (`VCALENDAR`) feed.

The feed covers episodes dated from 30 days in the past through 90 days ahead. The token's `hide_unmonitored` flag omits episodes with `monitored: false`, and `show_as_all_day` switches `DTSTART`/`DTEND` to date-only values. Timed events default to a 30-minute duration unless media-info runtime or an episode runtime is available.

```text
BEGIN:VCALENDAR
VERSION:2.0
PRODID:-//Jumbie//EN
CALSCALE:GREGORIAN
BEGIN:VEVENT
UID:BreakingBad-S1E1@jumbie
DTSTART:20260618T200000Z
DTEND:20260618T203000Z
DTSTAMP:20260601T120000Z
SUMMARY:Breaking Bad - S01E01 - Pilot
DESCRIPTION:Date: 2026-06-18T20:00:00+00:00\nStatus: Unreleased
END:VEVENT
END:VCALENDAR
```

---


## 3. Auth Endpoints

### Ban Management

#### `GET /api/auth/bans`

**Scope:** `auth:read`

##### Input

None.

##### Output

```json
[
  {
    "ip": "203.0.113.42",
    "fail_count": 5,
    "ban_count": 1,
    "banned_at": "2026-06-18T20:00:00+00:00",
    "banned_until": "2026-06-18T20:05:00+00:00",
    "is_permanent": false
  }
]
```

* `ip` — string. The banned client's IP address.
* `fail_count` — integer. Failed auth attempts recorded for this IP.
* `ban_count` — integer. How many times this IP has been banned (drives exponential backoff).
* `banned_at` — string. RFC 3339 UTC timestamp of when the ban started.
* `banned_until` — string or `null`. RFC 3339 UTC timestamp of when the ban expires; `null` for a permanent ban.
* `is_permanent` — boolean. `true` when the ban has no expiry.

Returns the current in-memory ban list merged with persisted bans. Dual storage (in-memory + DB): auth checks run on every request, so bans live in an in-memory `HashMap` for O(1) lookup while the DB is the durable source of truth that survives restarts. A ban added via the API lands in memory immediately but its DB write is async, so the list also emits still-active runtime-only bans to avoid showing stale data.

#### `POST /api/auth/bans`

**Scope:** `auth:write`

##### Input

```json
{
  "ip": "203.0.113.42",
  "duration_seconds": 3600
}
```

* `ip` — string, required. IPv4/IPv6 address to ban; a value that does not parse as an IP address is rejected with `400`.
* `duration_seconds` — integer, optional, no default. Ban length in seconds; omitted or `null` means a permanent ban. Values greater than `31536000` (10 years) are rejected with `400`.

##### Output

No content (`201`).

Manually adds a ban (admin action). The in-memory list is updated before the DB so the ban is effective immediately for subsequent requests; the DB write makes it survive a restart.

#### `DELETE /api/auth/bans/{ip}`

**Scope:** `auth:write`

##### Input

* `ip` — string, required (path). IPv4/IPv6 address to unban; a value that does not parse as an IP address is rejected with `400`.

##### Output

No content (`204`).

Unbans an IP address. Removed from memory first (same reasoning as `POST /api/auth/bans`) so the unban takes effect immediately.

### API Keys

#### `POST /api/config/auth/api_keys/generate`

**Scope:** `auth:write`

##### Input

```json
{
  "name": "CI Pipeline",
  "scopes": ["queue:read", "queue:write"],
  "duration_days": 90
}
```

* `name` — string, required. Human-readable label; must be non-empty, at most 255 characters, and contain no control characters (otherwise `400`).
* `scopes` — array of scope strings, required. At least one scope must be present (otherwise `400`); see [§11](#11-api-key-scopes) for the available values.
* `duration_days` — integer, optional, no default. Validity period in days; omitted, `null`, or `≤ 0` means the key never expires. Values greater than `36500` (~100 years) are rejected with `400`.

##### Output

```json
{
  "id": "550e8400-e29b-41d4-a716-446655440000",
  "key": "jb_aBcDeFgHiJkLmNoPqRsTuVwXyZ0123456789-abc",
  "prefix": "jb_aBcDeFgH"
}
```

* `id` — string. UUID identifying the stored key.
* `key` — string. The full API key (`jb_` plus 32 bytes of URL-safe base64). Returned only once in this response — it cannot be retrieved later, so store it now.
* `prefix` — string. `jb_` plus the first 8 characters of the key, shown in the UI so a key can be identified without exposing it.

Generates a new API key. Only a hash of the key is persisted, so a lost key must be revoked and regenerated.

### Calendar Tokens

#### `POST /api/config/auth/calendar_tokens/generate`

**Scope:** `auth:write`

##### Input

```json
{
  "name": "Living Room",
  "hide_unmonitored": false,
  "show_as_all_day": false
}
```

* `name` — string, required. Human-readable label; must be non-empty, at most 255 characters, and contain no control characters (otherwise `400`).
* `hide_unmonitored` — boolean, optional, default `false`. When `true`, unmonitored episodes are omitted from the generated calendar feed.
* `show_as_all_day` — boolean, optional, default `false`. When `true`, episodes are emitted as all-day calendar events.

##### Output

```json
{
  "id": "550e8400-e29b-41d4-a716-446655440000",
  "token": "cal_aBcDeFgHiJkLmNoPqRsTuVw",
  "hide_unmonitored": false,
  "show_as_all_day": false
}
```

* `id` — string. UUID identifying the stored token.
* `token` — string. The full calendar feed token (`cal_` plus 24 bytes of URL-safe base64).
* `hide_unmonitored` — boolean. The stored `hide_unmonitored` setting.
* `show_as_all_day` — boolean. The stored `show_as_all_day` setting.

Generates an iCal calendar feed token. Unlike API keys, calendar tokens grant no scoped access — purely reading episode air dates in calendar format.

API keys and calendar tokens are listed (key value masked) and removed through `GET` / `PUT /api/config` (`config:read` / `config:write`); the two endpoints above only generate them.

---

## 4. Series Endpoints

### Series Read

#### `GET /api/series`

**Scope:** `series:read`

##### Input

None.

##### Output

```json
[
  {
    "id": "550e8400-e29b-41d4-a716-446655440000",
    "title": "Example Show",
    "seasons": ["01", "02"],
    "season_count": 2,
    "release_profile": "Default",
    "quality_profile": "HD-1080p",
    "episodes_counts": [18, 20],
    "monitored_missing_count": 2,
    "queued_count": 1,
    "size": 21474836480,
    "path": "/media/tv/Example Show",
    "scan_queue_count": 0,
    "absolute_numbering": false,
    "aliases": ["Example Show (2019)"],
    "has_not_found_files": false
  }
]
```

* `id` — series UUID.
* `title` — display title.
* `seasons` — formatted season labels present for the series; empty in absolute-numbering mode.
* `season_count` — number of distinct seasons; `0` in absolute-numbering mode.
* `release_profile` — assigned release profile name (empty string when unset).
* `quality_profile` — assigned quality profile name (empty string when unset).
* `episodes_counts` — `[downloaded, total]` episode counts.
* `monitored_missing_count` — wanted episodes whose effective status is `missing`.
* `queued_count` — episodes currently in the download queue (in-flight: Queued/Downloading), monitored or not; shown yellow instead of red.
* `size` — total size on disk in bytes.
* `path` — stored/organized series path; empty string when derived from the destination root.
* `scan_queue_count` — media-info scan queue depth for this series (currently always `0`).
* `absolute_numbering` — whether the effective numbering mode is absolute.
* `aliases` — alternate search names.
* `has_not_found_files` — `true` when downloaded episodes reference files that no longer exist on disk.

Series marked hidden in the library (`hidden_in_library`) are omitted from this list. It is an unbounded, unsorted array — pagination and ordering are not applied server-side.

Series List Sorting — the series list is sortable by `title`, `season_count`, `quality_profile`, `progress`, and `size`. The frontend applies the selected field and direction (`sort` / `order`, where `order` is `asc` or `desc`) client-side; this handler accepts no query parameters and returns the complete list.

#### `GET /api/series/{id}`

**Scope:** `series:read`

##### Input

* `id` — path parameter, required. Series UUID.

##### Output

```json
{
  "info": {
    "id": "550e8400-e29b-41d4-a716-446655440000",
    "title": "Example Show",
    "seasons": ["01", "02"],
    "season_count": 2,
    "release_profile": "Default",
    "quality_profile": "HD-1080p",
    "episodes_counts": [18, 20],
    "monitored_missing_count": 2,
    "queued_count": 1,
    "size": 21474836480,
    "path": "/media/tv/Example Show",
    "scan_queue_count": 0,
    "absolute_numbering": false,
    "aliases": ["Example Show (2019)"],
    "has_not_found_files": false
  },
  "config": {
    "target_title": "Example Show",
    "series_id": "550e8400-e29b-41d4-a716-446655440000",
    "name": "example-show",
    "release_profile": "Default",
    "quality_profile": "HD-1080p",
    "hidden_in_library": false,
    "aliases": [],
    "absolute_numbering": false,
    "season": {},
    "season_absolute": {},
    "metadata_ids": { "tvdb": "12345" },
    "metadata_last_synced_at": { "tvdb": "2026-06-18T20:00:00+00:00" },
    "monitor_mode": "all",
    "path": "/media/tv/Example Show"
  },
  "episodes": [
    {
      "unique_id": "0f8fad5b-d9cb-469f-a165-70867728950e",
      "season": "1",
      "episode": 1,
      "header": "S01E01",
      "title": "Pilot",
      "status": "downloaded",
      "quality_profile_id": "hd-1080p",
      "size": 1610612736,
      "submitter": null,
      "path": "/media/tv/Example Show/Season 01/Example Show - S01E01.mkv",
      "original_path": "/downloads/Example.Show.S01E01.mkv",
      "media_info": null,
      "fingerprint": null,
      "created_at": "2026-06-18T20:00:00+00:00",
      "file_acquired_at": "2026-06-18T20:00:00+00:00",
      "monitored": true,
      "dates": {
        "meta_date": "2026-06-18T20:00:00+00:00",
        "upload_date": "2026-06-18T22:00:00+00:00",
        "est_date": null
      },
      "metadata_ids": { "tvdb": "12345" },
      "description": null,
      "runtime": 45,
      "image_url": null,
      "metadata_source": "tvdb",
      "release_title": "Example.Show.S01E01.1080p.WEB-DL",
      "parts": [],
      "auxiliary_files": [],
      "show_only_downloaded": false,
      "file_exists": true
    }
  ],
  "metadata_seasons": [
    {
      "season_number": 1,
      "title": null,
      "episode_count": 10,
      "premiere_date": null,
      "end_date": null,
      "image_url": null,
      "summary": null,
      "provider_instance_id": "6ba7b810-9dad-11d1-80b4-00c04fd430c8",
      "is_fallback_mode": false
    }
  ],
  "suppressed_seasons": [
    { "season": 3, "cache_available": true }
  ]
}
```

* `info` — summary row, identical in shape to an entry of `GET /api/series`.
* `config` — the full stored mapping rule; `SeriesSettings` fields are flattened to the same level (see `metadata_last_synced_at`).
* `config.metadata_ids` — provider instance id → provider metadata id map.
* `config.metadata_last_synced_at` — provider instance id → last successful sync time, RFC 3339 with explicit UTC offset.
* `config.monitor_mode` — one of `all`, `future`, `missing`, `existing`, `pilot`, `firstSeason`, `specials`, `none`.
* `episodes` — episode view models, including placeholder "cells" for expected episodes with no file.
* `episodes[].status` — one of `missing`, `unreleased`, `downloaded`, `organized`, `in_queue`, `out_of_range`.
* `episodes[].created_at` / `episodes[].file_acquired_at` — RFC 3339 with explicit UTC offset, or `null`.
* `episodes[].dates` — `meta_date` (provider airdate), `upload_date` (source feed), `est_date` (estimate); each RFC 3339 or `null`.
* `episodes[].parts` — `EpisodePartInfo` entries for multi-part files.
* `episodes[].auxiliary_files` — subtitle / nfo sidecars attached to the episode.
* `episodes[].file_exists` — whether the file at `path` actually exists on disk.
* `metadata_seasons` — provider season cache (present only when the series has been synced); may be empty.
* `metadata_seasons[].provider_instance_id` — provider instance uuid that produced the season data.
* `suppressed_seasons` — seasons the user deleted, with whether cached metadata can restore them.
* `suppressed_seasons[].season` — suppressed season number.
* `suppressed_seasons[].cache_available` — `true` when the provider cache can restore the season without an API call.

Returns `200` with a `null` body when no series matches `id` — this endpoint does not return `404`.

#### `POST /api/series/details/batch`

**Scope:** `series:read`

##### Input

```json
{
  "ids": ["550e8400-e29b-41d4-a716-446655440000", "6ba7b810-9dad-11d1-80b4-00c04fd430c8"],
  "limit": 25,
  "offset": 0
}
```

* `ids` — array of series UUIDs, required.
* `limit` — optional, no default. Maximum number of series to return after sorting by id.
* `offset` — optional, no default. Number of series to skip after sorting by id.

##### Output

```json
{
  "550e8400-e29b-41d4-a716-446655440000": {
    "info": { "...": "SeriesInfo, same shape as above" },
    "config": { "...": "MappingRule" },
    "episodes": [],
    "metadata_seasons": [],
    "suppressed_seasons": []
  }
}
```

* object keys — series UUIDs; each value is a `SeriesDetails` (same shape as `GET /api/series/{id}`).
* `info` — summary row.
* `config` — full mapping rule (flattened `SeriesSettings`).
* `episodes` — episode view models.
* `metadata_seasons` — provider season cache.
* `suppressed_seasons` — deleted seasons with restore availability.

Requested ids that do not exist are omitted. Matching series are sorted by id before `offset`/`limit` are applied; an empty object `{}` is returned when nothing matches (including when `offset` is at or beyond the match count). Applies the same pagination to the detail payloads, not just the keys.

#### `GET /api/series/{id}/files`

**Scope:** `series:read`

##### Input

* `id` — path parameter, required. Series UUID.

##### Output

```json
[
  {
    "path": "/media/tv/Example Show/Season 01/Example Show - S01E01.mkv",
    "filename": "Example Show - S01E01.mkv",
    "size": 1610612736,
    "assigned_id": "0f8fad5b-d9cb-469f-a165-70867728950e",
    "assigned_header": "S01E01",
    "kind": "video"
  },
  {
    "path": "/downloads/Example.Show.S01E02.mkv",
    "filename": "Example.Show.S01E02.mkv",
    "size": 1610612736,
    "assigned_id": null,
    "assigned_header": null,
    "kind": "video"
  }
]
```

* `path` — series-root-relative path when organized, otherwise the download/filesystem path.
* `filename` — base filename.
* `size` — file size in bytes.
* `assigned_id` — assigned episode UUID, or `null` when unassigned.
* `assigned_header` — human-readable assignment label (`S01E01`, contiguous ranges like `S01E01-E03`, comma lists for gaps, and ` Part N` suffixes), or `null` when unassigned.
* `kind` — `video`, `subtitle`, or `nfo`.

This is a disk-scan operation, not a DB query: unassigned files on disk are listed alongside assigned ones for a complete inventory, and results are sorted by natural filename. Stale DB references (files assigned in the database but gone from disk) are not cleaned here and are simply not shown; a background Stale File Cleanup task removes them. Returns `404` when the series id is unknown and `500` on a database error.

#### `GET /api/calendar`

**Scope:** `series:read`

##### Input

* `start_date` — query parameter, required. RFC 3339 timestamp with an explicit UTC offset (the frontend sends the browser's offset).
* `end_date` — query parameter, required. RFC 3339 timestamp with an explicit UTC offset.

Both are strict: a naive, zone-less, or date-only value is rejected with `400`.

##### Output

```json
{
  "episodes": [
    {
      "series_title": "Example Show",
      "series_id": "550e8400-e29b-41d4-a716-446655440000",
      "episode_id": "0f8fad5b-d9cb-469f-a165-70867728950e",
      "season": "1",
      "episode": 3,
      "episode_title": "The Third One",
      "dates": {
        "meta_date": "2026-06-18T20:00:00+00:00",
        "upload_date": "2026-06-18T22:00:00+00:00",
        "est_date": null
      },
      "eff_date": "2026-06-18T20:00:00+00:00",
      "status": "unreleased",
      "has_file": false
    }
  ]
}
```

* `episodes` — episodes whose release falls within `start_date`–`end_date`.
* `episodes[].series_title` — resolved series title.
* `episodes[].series_id` — series UUID, letting clients open the episode modal without a lookup.
* `episodes[].episode_id` — episode UUID.
* `episodes[].season` — season as a string; absolute-numbering rows and legacy `NULL` seasons resolve to `"1"`.
* `episodes[].episode` — episode number.
* `episodes[].episode_title` — episode title, or `null`.
* `episodes[].dates` — `meta_date`, `upload_date`, `est_date`; each RFC 3339 with explicit UTC offset, or `null`.
* `episodes[].eff_date` — effective release date chosen by the user's display preference (metadata → source → estimated); RFC 3339, empty string when none is available.
* `episodes[].status` — episode status string (defaults to `unreleased`).
* `episodes[].has_file` — whether a file path is recorded for the episode.

#### `POST /api/episodes/batch-monitor`

**Scope:** `series:read`

##### Input

```json
{
  "ids": ["0f8fad5b-d9cb-469f-a165-70867728950e", "1c2d3e4f-5a6b-7c8d-9e0f-1234567890ab"],
  "monitored": true
}
```

* `ids` — array of episode UUIDs, required.
* `monitored` — boolean, required. `true` to monitor, `false` to unmonitor each listed episode.

##### Output

No content (`200 OK`).

Direct database write: sets `monitor_override = 1` with the requested state for every listed episode, so periodic monitor sweeps respect the user's choice. An empty `ids` array is a no-op and still returns `200 OK`.

### Series Write

#### `POST /api/series`

**Scope:** `series:write`

Add a new series. Registers the folder and mapping, optionally scans for existing files, applies the monitor mode, and auto-syncs metadata for every provider that has a non-empty ID (sync is bounded by an internal deadline and failures only record activity events — the series is not rolled back). When `search_missing_on_add` is set and at least one sync succeeded, monitored+missing+released episodes are also queued for search. Series-level settings (aliases, absolute numbering, search flags, format overrides, rename/flatten toggles) can be supplied in the same call.

##### Input

```json
{
  "path": "/media/tv/My Show",
  "series_name": "My Show",
  "scan_for_existing": true,
  "monitor_mode": "all",
  "quality_profile": "HD-1080p",
  "release_profile": "Default",
  "metadata_ids": {
    "tvdb-instance-1": "12345"
  },
  "search_missing_on_add": false,
  "resolve_collisions": true,
  "aliases": ["Alt Name"],
  "reg_patterns": [],
  "absolute_numbering": true,
  "season_folder_format": "Season ${season:auto2}",
  "episode_file_format": "${series} - S${season:auto2}E${episode:auto2}",
  "season_folder_format_absolute": "S${season:auto2}",
  "episode_file_format_absolute": "${series} - S${season:auto2}E${episode:auto2}",
  "flatten_season_folders": false,
  "rename_episodes": false,
  "search_format": "S${season:02}E${episode:02}",
  "search_format_absolute": "E${episode:02}"
}
```

* `path` — string, required. Destination path; resolved and permission-checked against `config.organization.primary_root`. An invalid path returns `400`.
* `series_name` — string, optional. Dedicated title input; when omitted the folder name (or `path`) is used. Validated by `validate_title`.
* `scan_for_existing` — boolean, optional, default `true`. Scan the directory for existing episode files immediately on creation.
* `monitor_mode` — enum, optional, default unset (treated as `None` when applied after the scan). One of `all`, `future`, `missing`, `existing`, `pilot`, `firstSeason`, `specials`, `none`. Applied after the scan so the scan does not trigger download searches.
* `quality_profile` — string, optional. Falls back to the first/default quality profile. Validated.
* `release_profile` — string, optional. Falls back to the first/default release profile. Validated.
* `metadata_ids` — object of string→string, optional, default `{}`. Provider instance ID → external metadata ID. Empty values are dropped; only non-empty IDs are stored and drive the auto-sync loop.
* `search_missing_on_add` — boolean, optional, default `false`. After a successful sync, auto-search monitored+missing+released episodes. Only effective when at least one non-empty metadata ID is present.
* `resolve_collisions` — boolean, optional, default `true`. When true the folder name is sanitized per the illegal-char policy and folder collisions are resolved per config; explicit custom paths send `false` to be honored verbatim.
* `aliases` — array of strings, optional, default `[]`. Series search aliases (alternate titles). Blank entries are dropped and duplicates removed on save.
* `reg_patterns` — array of strings, optional, default `[]`. Custom regex patterns used to identify this series' releases.
* `absolute_numbering` — boolean, optional. Tri-state numbering mode: `true` = absolute, `false` = normal, omitted = inherit the global `general.absolute_numbering` default.
* `season_folder_format` — string, optional. Per-series season folder template; omitted = use the global default.
* `episode_file_format` — string, optional. Per-series episode filename template; omitted = use the global default.
* `season_folder_format_absolute` — string, optional. Season folder template used in absolute numbering mode; omitted = use the global default.
* `episode_file_format_absolute` — string, optional. Episode filename template used in absolute numbering mode; omitted = use the global default.
* `flatten_season_folders` — boolean, optional. `true` places all episodes directly in the series folder; omitted = inherit the global default.
* `rename_episodes` — boolean, optional. Per-series rename override; omitted = inherit the global default.
* `search_format` — string, optional. Per-series search-key template for normal numbering (`${season}`/`${episode}`, e.g. `S${season:02}E${episode:02}`); omitted = use the global default. A blank string searches by title alone.
* `search_format_absolute` — string, optional. Per-series search-key template for absolute numbering; omitted = use the global default.

These are the same series-level settings `PUT /api/series/{id}` accepts, flattened at the top level. **Season settings are not accepted at creation** — `season` / `season_absolute` overrides are ignored here; add the series first, then configure seasons via the season endpoints.

##### Output

```json
"3f2b1c4e-8a7d-4c5b-9e10-2f6a1b7c8d9e"
```

* `body` — the new series UUID, encoded as a bare JSON string (not an object). Returns `201 Created`.

* On error: `400` for an invalid path, title, quality/release profile, alias, regex pattern, or a folder collision (`{"error": "...", "series_id": "<claiming-series-id>"}` when the path is claimed by an existing visible series).

#### `POST /api/series/batch-edit`

**Scope:** `series:write`

Batch edit series (quality profile, monitor mode). At least one series ID is required. The monitor mode is persisted to each mapping first, then applied per-episode in a batched pass; a release-profile change triggers a background rescore. Per-series failures are non-fatal and the batch still returns `200`.

##### Input

```json
{
  "series_ids": [
    "3f2b1c4e-8a7d-4c5b-9e10-2f6a1b7c8d9e"
  ],
  "quality_profile": "HD-1080p",
  "release_profile": "Default",
  "monitor_mode": "missing"
}
```

* `series_ids` — array of strings, required. Must be non-empty; an empty array returns `400`.
* `quality_profile` — string, optional. Empty string clears the series' quality profile (`null`); otherwise validated and stored.
* `release_profile` — string, optional. Empty string clears it; a real change triggers a background episode rescore.
* `monitor_mode` — enum, optional. Applied to every series when present; one of `all`, `future`, `missing`, `existing`, `pilot`, `firstSeason`, `specials`, `none`.

##### Output

Empty body (`200`).

#### `POST /api/series/batch-delete`

**Scope:** `series:write`

Batch delete series. At least one series ID is required. Series currently locked by another modification are skipped. With neither flag set the series is only hidden; see below for the flag combinations.

##### Input

```json
{
  "series_ids": [
    "3f2b1c4e-8a7d-4c5b-9e10-2f6a1b7c8d9e"
  ],
  "delete_configurations": true,
  "delete_episode_data": false,
  "delete_episodes": true
}
```

* `series_ids` — array of strings, required. Must be non-empty; an empty array returns `400`.
* `delete_configurations` — boolean, optional, default `false`. Removes series settings, season settings, and the mapping from the DB.
* `delete_episode_data` — boolean, optional, default `false`. Removes episode data (metadata, media scan fingerprints) from the DB but keeps files on disk.
* `delete_episodes` — boolean, optional, default `false`. Removes episode data **and** deletes the episode files from disk; implicitly makes `delete_episode_data` true.

##### Output

Empty body (`200`).

#### `POST /api/series/batch-fetch-metadata`

**Scope:** `series:write`

Batch fetch metadata for series. Requires an active metadata provider (`400` if none is installed). Fetches metadata for multiple series at once, skipping any without a configured metadata ID or that do not exist. Returns immediately after queuing — results are logged, not returned.

##### Input

```json
{
  "series_ids": [
    "3f2b1c4e-8a7d-4c5b-9e10-2f6a1b7c8d9e"
  ]
}
```

* `series_ids` — array of strings, required. Series to queue; each eligible series is submitted to the metadata queue (concurrency = 1, deduplicated per series) so provider rate limits are respected.

##### Output

Empty body (`200`).

* This is fire-and-forget: no `task_id` is returned and there is no polling endpoint. Failures for an individual series are logged and surface in the activity log.

* On error: `400` when no active metadata provider is found.

#### `PUT /api/series/{id}`

**Scope:** `series:write`

Update series settings. The body is `UpdateSeriesPayload` with `SeriesSettings` flattened, so every field below is top-level. A path change with a `path_operation` performs the filesystem move/copy/delete; an `absolute_numbering` change runs the mode-switch handler (which may move files between `_unmatched/` and the series directory and returns a warning). Returns `409` when the series is locked while performing a path move.

##### Input

```json
{
  "quality_profile": "HD-1080p",
  "release_profile": "Default",
  "title": "My Show",
  "path_operation": "Move",
  "aliases": ["My Show"],
  "absolute_numbering": false,
  "season": {
    "1": {
      "season": "1",
      "episode_start": 1,
      "episode_end": 12,
      "cell_count": 12,
      "episode_offset": 0,
      "alias_season_number": null,
      "search_format": null,
      "aliases": [],
      "reg_patterns": []
    }
  },
  "season_absolute": {},
  "reg_patterns": [],
  "season_folder_format": "Season ${season:auto2}",
  "episode_file_format": "${series} - S${season:auto2}E${episode:auto2}",
  "season_folder_format_absolute": "S${season:auto2}",
  "episode_file_format_absolute": "${series} - S${season:auto2}E${episode:auto2}",
  "flatten_season_folders": false,
  "path": "/media/tv/My Show",
  "monitor_mode": "all",
  "metadata_ids": {
    "tvdb-instance-1": "12345"
  },
  "metadata_last_synced_at": {},
  "rename_episodes": null,
  "search_format": null,
  "search_format_absolute": null,
  "last_known_dir_mtimes": {}
}
```

* `id` — path parameter; the series ID.
* `quality_profile` — string, required. Validated; saved to the mapping.
* `release_profile` — string, required. A change triggers a background episode rescore.
* `title` — string, optional. A non-empty value replaces `target_title`.
* `path_operation` — enum, optional. One of `Move`, `Copy`, `Delete`, `DoNothing`; used only when `path` changes and is not `DoNothing`.
* `aliases` — array of strings, optional. Deduplicated on save; empty strings are dropped.
* `absolute_numbering` — boolean, optional. Tristate: omitted/`null` inherits the global default; a resolved change triggers the mode-switch handler.
* `season` — object, optional. Map of season key → season override (see `SeasonOverride` fields below).
* `season_absolute` — object, optional. Independent season overrides for absolute numbering mode.
* `reg_patterns` — array of strings, optional. Custom regex patterns for this series.
* `season_folder_format` — string, optional. `null` = use the server's global default.
* `episode_file_format` — string, optional. `null` = use the server's global default.
* `season_folder_format_absolute` — string, optional. Absolute-mode folder template; `null` = global default.
* `episode_file_format_absolute` — string, optional. Absolute-mode file template; `null` = global default.
* `flatten_season_folders` — boolean, optional. Write files directly in the series root rather than season subdirectories.
* `path` — string, optional. New series path/folder; an empty string resets it. Resolved via `resolve_template`/`validate_path`.
* `monitor_mode` — enum, optional, one of `all`, `future`, `missing`, `existing`, `pilot`, `firstSeason`, `specials`, `none`. Applying a changed mode re-derives per-episode monitor status.
* `metadata_ids` — object of string→string, optional. Provider instance ID → external metadata ID. Changing/removing an ID prunes the matching `metadata_last_synced_at` entry.
* `metadata_last_synced_at` — object of string→string, optional, default `{}`. Per-provider last-sync timestamps (RFC 3339); normally left empty (server-managed).
* `rename_episodes` — boolean, optional. `null` = inherit the global rename setting; `true` = rename; `false` = keep the original filename.
* `search_format` — string, optional. Series-level search-key template for normal numbering; `null` = use the global default. A blank string searches by title alone.
* `search_format_absolute` — string, optional. Series-level search-key template for absolute numbering; `null` = use the global default.
* `last_known_dir_mtimes` — object of string→number, optional, default `{}`. Directory mtimes from the last scan; used to decide whether a rescan is needed.

`SeasonOverride` object (each entry of `season` / `season_absolute`):

* `season` — string, required. The season key.
* `episode_start` — integer, optional. Default `1`.
* `episode_end` — integer, optional. Default unbounded (`i32::MAX`).
* `cell_count` — integer, optional. Number of UI episode cells to render; `null` shows only DB episodes.
* `episode_offset` — integer, optional, default `0`. Bridge between source (release-title) and local (DB) episode numbers.
* `alias_season_number` — integer (unsigned), optional. Season to search instead of the real one.
* `search_format` — string, optional. Season-level search-key template; `null` inherits the series/global template, a blank string searches by title alone for that season.
* `aliases` — array of strings, optional, default `[]`.
* `reg_patterns` — array of strings, optional, default `[]`.

##### Output

```json
{
  "mode_switch_warning": false,
  "mode_switch_message": null
}
```

* `mode_switch_warning` — boolean. `true` when the numbering mode actually changed and the previous mode still had episode data worth preserving.
* `mode_switch_message` — string or `null`. Human-readable explanation shown when a warning is raised.

* On error: `400` for invalid quality profile, title, season aliases, paths, or season overrides; `404` when the series is not found; `409` when the series is locked during a path operation.

#### `DELETE /api/series/{id}`

**Scope:** `series:write`

Remove series. When neither flag is set the series is simply hidden (`hidden_in_library = true`) and all data is preserved — unhiding later restores everything. Setting `delete_configurations` + `delete_episodes` is equivalent to a full removal.

##### Input

```json
{
  "delete_configurations": false,
  "delete_episode_data": false,
  "delete_episodes": false
}
```

* `id` — path parameter; the series ID.
* `delete_configurations` — boolean, optional, default `false`. Removes series settings, season settings, and episode mappings from the DB (keeps episode metadata and files).
* `delete_episode_data` — boolean, optional, default `false`. Removes episode data (metadata, media scan fingerprints) from the DB but keeps the files on disk.
* `delete_episodes` — boolean, optional, default `false`. Removes episode data **and** deletes the episode files from disk; when set, `delete_episode_data` is implicitly true.

##### Output

No content (`204`).

* On error: `404` when the series is not found; `409` when the series is locked for a data/config deletion.

#### `DELETE /api/series/{id}/season/{season}`

**Scope:** `series:write`

Delete a season's episode data. Deletes the season's DB data and records a durable suppression (tombstone) so a later metadata sync does not recreate the season; in absolute numbering the single canonical season is un-suppressed (reset). Release-date estimation is re-run afterward.

##### Input

* `id` — path parameter; the series ID.
* `season` — path parameter; the season number (e.g. `2`, `S02`). Validated; a non-numeric label returns `400`.

##### Output

No content (`204`).

* On error: `400` for an invalid season number; `404` when the series is not found; `409` when the series is currently locked.

#### `POST /api/series/{id}/refresh`

**Scope:** `series:write`

Scan the series directory (and the download organizer roots) for new/changed files. If any episode is discovered without a title, a metadata fetch for the series is queued.

##### Input

* `id` — path parameter; the series ID.

##### Output

```json
{
  "series_found": 12,
  "downloads_found": 3,
  "total_episodes": 12
}
```

* `series_found` — number of episodes discovered in the series directory.
* `downloads_found` — number of series discovered across the download organizer roots.
* `total_episodes` — current episode count for the series from the DB.

* On error: `404` when the series is not found; `500` when the episode count cannot be read.

#### `POST /api/series/{id}/sync_metadata`

**Scope:** `series:write`

Sync episode metadata. Upserts the supplied metadata episodes for the series into the DB in a single transaction (all-or-nothing), then re-runs release-date estimation and re-applies monitor status. Episode IDs are generated in the series' active numbering mode. The request is rejected as a whole (`400`) if any episode is invalid or duplicated.

##### Input

```json
{
  "episodes": [
    {
      "season": 1,
      "episode": 1,
      "metadata_id": "12345-1-1",
      "title": "Pilot",
      "description": "The first episode.",
      "runtime": 45,
      "image_url": "https://example.com/1.jpg",
      "meta_date": "2020-01-01"
    }
  ]
}
```

* `id` — path parameter; the series ID.
* `episodes` — array of objects, required. The metadata episodes to upsert.
* `episodes[].season` — integer, required. Must not be negative; an invalid value returns `400`.
* `episodes[].episode` — integer, required. Validated; an invalid value returns `400`.
* `episodes[].metadata_id` — string, required. The provider's unique episode ID; a non-empty duplicate across the batch returns `400`.
* `episodes[].title` — string, required. Episode title.
* `episodes[].description` — string, optional. Episode summary.
* `episodes[].runtime` — integer, optional. Runtime in minutes.
* `episodes[].image_url` — string, optional. Episode thumbnail URL.
* `episodes[].meta_date` — string, optional. `YYYY-MM-DD` or an ISO/RFC 3339 datetime (parsed as UTC).

##### Output

Empty body (`200`).

* On error: `400` for a negative season, invalid episode number, or a duplicate `(season, episode)` / `metadata_id` in the payload; `404` when the series is not found.

#### `POST /api/series/{id}/fetch_metadata`

**Scope:** `series:write`

Trigger metadata fetch from provider. Fetches episode data server-side and upserts it into the DB through the metadata queue; the call waits for the queued fetch to complete. Optionally overrides the metadata ID to avoid races with unsaved form data.

##### Input

```json
{
  "metadata_id": "12345"
}
```

* `id` — path parameter; the series ID.
* `metadata_id` — string, optional. Overrides the metadata ID used for this fetch (the provider instance owning that ID is resolved). When absent the provider/ID stored on the mapping is used. The entire body may be omitted.

##### Output

```json
{
  "synced_at": "2024-01-01T12:00:00Z"
}
```

* `synced_at` — string or `null`. RFC 3339 timestamp of the successful sync, or `null` when the fetch did not produce one.

* On error: `404` when the series or provider mapping is not found; `429` when the provider rate-limits the request.

#### `POST /api/series/{id}/fetch_series_info`

**Scope:** `series:write`

Fetch series info from provider. Fetches the provider's canonical title/overview; when `update_title` is set the series title is replaced, and when `merge_aliases` is set the provider aliases (with the canonical title first) are merged into the series alias list, deduplicated. Freshly fetched data is written to the metadata cache.

##### Input

```json
{
  "update_title": true,
  "merge_aliases": true
}
```

* `id` — path parameter; the series ID.
* `provider_instance_id` — query parameter, optional. When set, targets that specific provider instance (using its config); otherwise the highest-priority enabled provider mapped to the series is used.
* `update_title` — boolean, optional, default `false`. Replace the series `target_title` with the provider's canonical name.
* `merge_aliases` — boolean, optional, default `false`. Merge provider aliases into the series alias list.

##### Output

```json
{
  "name": "My Show",
  "overview": "A series about...",
  "cached": false
}
```

* `name` — the provider's canonical series name.
* `overview` — the series overview, or `null` when the provider returns none.
* `cached` — always `false` for this endpoint (the DB cache is intentionally skipped so fresh localised data is fetched).

* On error: `400` when the provider does not implement series-info fetching; `404` when the series is not found; `429` when the provider rate-limits the request.

#### `POST /api/series/{id}/fetch_series_aliases`

**Scope:** `series:write`

Fetch series aliases from provider. Prefers the cached aliases and only calls the provider when none are cached; the provider's canonical title is injected as an alias so the series stays findable after aliases override the title as a search term. The merged, deduplicated list is persisted to the mapping and cache.

##### Input

* `id` — path parameter; the series ID.
* `provider_instance_id` — query parameter, optional. When set, targets that specific provider instance; otherwise the highest-priority enabled provider mapped to the series is used.

##### Output

```json
{
  "title": "My Show",
  "aliases": ["My Show", "Alternate Name"],
  "cached": false
}
```

* `title` — the provider's canonical title, or an empty string when it could not be resolved.
* `aliases` — the merged, deduplicated alias list stored on the series.
* `cached` — `true` when the aliases came from the metadata cache instead of a provider call.

* On error: `400` when the provider does not implement alias fetching; `404` when the series is not found; `429` when the provider rate-limits the request.

#### `DELETE /api/series/{id}/files`

**Scope:** `series:write`

Delete series files. Each path is removed from disk (if it exists) and its episode's `file_path` is cleared with status reset to `missing`; the media fingerprint is dropped so the file is not resurrected, and empty download folders are culled. Missing paths are skipped. Returns `409` when the series is locked by another modification. This handler returns the raw error message text rather than the JSON error envelope.

##### Input

```json
{
  "paths": [
    "/media/tv/My Show/Season 01/My Show - S01E01.mkv"
  ]
}
```

* `id` — path parameter; the series ID.
* `paths` — array of strings, required. File paths to delete from disk and disown from their episodes.

##### Output

Empty body (`200`).

* On error: `409` when the series is currently locked.

#### `POST /api/series/{id}/files/assign`

**Scope:** `series:write`

Manually assign a single file (or an episode range) to one or more episodes. The path is checked for traversal and existence, the season/episode values are validated at the API boundary, and a filename part suffix (`-pt2`, `-cd1`, `-part1`) routes the file to `episode_parts` instead of the main `file_path`. Fingerprinting is spawned in the background, so the API returns before hashing completes. Returns `409` if the series is locked by another modification (batch move, reorganize, etc.).

##### Input

```json
{
  "path": "/downloads/My Show/My.Show.S02E05.1080p.mkv",
  "season": "2",
  "episode": "5"
}
```

* `id` — path parameter; the series ID.
* `path` — string, required. Absolute path to the source file on disk; must exist and must not exceed 4096 bytes or contain `..`.
* `season` — string, required. Season number (e.g. `"2"`).
* `episode` — string, required. Episode number or range, e.g. `"5"` or `"1-3"`.

##### Output

Empty body (`200`).

* On error: `400` for an invalid/oversized/traversing path, invalid season, or invalid episode number/range; `404` when the series or source file is not found; `409` when the series is currently locked.

#### `POST /api/series/{id}/files/unassign`

**Scope:** `series:write`

Disown one or more files from their episodes without touching disk — the same loop as `delete_series_files` minus the `fs::remove_file` call, so each file can be re-assigned or deleted later. Every unassigned path is recorded in the durable `blocked_files` list so a rescan cannot re-adopt a file the user judged wrong.

##### Input

```json
{
  "paths": [
    "/media/tv/My Show/Season 02/My Show - S02E05.mkv"
  ]
}
```

* `id` — path parameter; the series ID.
* `paths` — array of strings, required. File paths to unassign; matching episodes have `file_path` cleared and status reset to `missing`.

##### Output

Empty body (`200`).

* On error: `409` when the series is currently locked.

#### `POST /api/series/{id}/files/batch-assign`

**Scope:** `series:write`

Bulk sequential assignment: given N files and a starting `(season, episode)` coordinate, `paths[i]` is assigned to `(start_season, start_episode + i)`. When the current season's episode count is known from metadata (`metadata_season_cache`), the sequence automatically rolls over the last episode of season S into S+01E01, so a multi-season batch import needs no manual season switching.

##### Input

```json
{
  "paths": [
    "/downloads/My Show/ep01.mkv",
    "/downloads/My Show/ep02.mkv"
  ],
  "start_season": "1",
  "start_episode": 1,
  "is_multipart": false
}
```

* `id` — path parameter; the series ID.
* `paths` — array of strings, required. Files to assign in order.
* `start_season` — string, required. Season for the first file.
* `start_episode` — integer, required. Episode number for the first file; subsequent files increment.
* `is_multipart` — boolean, optional, default `false`. Treat each file as a part of the same episode rather than advancing the episode number.

##### Output

Empty body (`200`).

* On error: `400` for an invalid `start_season`/`start_episode`; `404` when the series is not found; `409` when the series is currently locked.

#### `POST /api/series/{id}/clear-not-found-files`

**Scope:** `series:write`

Clear `file_path` for every episode in the series whose file no longer exists on disk, also deleting its `episode_parts` and `file_paths` rows. Monitor status is re-evaluated per affected episode, since a file going missing changes `has_file` for `Missing`/`Existing` modes.

##### Input

* `id` — path parameter; the series ID.

##### Output

```json
{
  "cleared": 3
}
```

* `cleared` — number of episodes whose missing file reference was cleared.

* On error: `409` when the series is currently locked.

#### `PUT /api/series/{id}/actions/monitor_episodes`

**Scope:** `series:write`

Apply a monitor mode to all episodes of a series. The mode is persisted to the series mapping before episode states are re-derived, so a crash mid-apply can never leave episodes out of step with an unpersisted mode. User per-episode overrides are cleared as part of the change.

##### Input

```json
{
  "mode": "missing"
}
```

* `id` — path parameter; the series ID.
* `mode` — enum, required. One of `all`, `future`, `missing`, `existing`, `pilot`, `firstSeason`, `specials`, `none`. Semantics match the monitor-mode help text (e.g. `missing` monitors episodes with no file or not yet aired).

##### Output

Empty body (`200`).

#### `POST /api/series/{id}/actions/reorganize`

**Scope:** `series:write`

Reorganize (rename/move) a single series' files according to the naming template. Builds the full source→destination plan, proactively scans missing media info, and executes it. A duplicate destination is a permanent misconfiguration and is marked failed (`500`); a partial failure also returns `500` so the frontend does not report false success.

##### Input

```json
{
  "target_absolute": false
}
```

* `id` — path parameter; the series ID.
* `target_absolute` — boolean, required. Whether to use absolute episode numbering for the target filenames; when it matches the mapping setting the cached rename plan is reused.

##### Output

Empty body (`200`).

* On error: `404` when the series is not found; `503` when the organizer is unavailable; `409` when the series is currently locked; `500` on a duplicate target or if any move failed.

#### `POST /api/series/actions/reorganize_all`

**Scope:** `series:write`

Synchronously reorganize every series with pending renames. Blocks the HTTP response until complete; use `reorganize_all_async` for progress notifications. If a reorganization is already running, the call is a no-op and still returns `200`.

##### Input

None.

##### Output

Empty body (`200`).

#### `POST /api/series/actions/reorganize_all_async`

**Scope:** `series:write`

Spawn a background reorganization of all series and return a `task_id` immediately. Poll `GET /api/series/actions/reorganize_all/{task_id}/status` to track progress.

##### Input

None.

##### Output

```json
{
  "results": [],
  "success_count": 0,
  "failure_count": 0,
  "task_id": "b3f1c2d4-5e6a-47b8-9c0d-1e2f3a4b5c6d"
}
```

* `results` — array; always empty on this spawn response (per-series results are not available yet).
* `success_count` — always `0` at spawn time.
* `failure_count` — always `0` at spawn time.
* `task_id` — the background task identifier to poll for progress.

#### `GET /api/series/actions/reorganize_all/{task_id}/status`

**Scope:** `series:write`

Poll the progress of a background reorganization started by `reorganize_all_async`. Returns `400` for an empty task ID and `404` when no task with that ID is known.

##### Input

* `task_id` — path parameter; the ID returned by the async reorganize endpoint.

##### Output

```json
{
  "operation_type": "reorganize",
  "total": 12,
  "completed": 5,
  "success_count": 14,
  "failed": 1,
  "finished": false,
  "errors": [
    "Failed to move '/media/tv/Show/ep01.mkv': permission denied"
  ]
}
```

* `operation_type` — the operation kind; `"reorganize"` (or `"batch_move"`) in snake_case.
* `total` — total number of series to process.
* `completed` — number of series processed so far.
* `success_count` — number of individual files moved successfully.
* `failed` — number of individual files that failed.
* `finished` — whether the task has completed (forced to `true` by `mark_finished`, which also sets `completed` to `total`).
* `errors` — human-readable messages for failures encountered.

#### `POST /api/episodes/{id}/scan_media`

**Scope:** `series:write`

Queue a high-priority media-info (ffprobe) scan for an episode's assigned file and return immediately; the scan runs in the background. Returns `400` when media info scanning is disabled in settings or FFmpeg/FFprobe is not installed, and `404` when the episode has no assigned file or the file is missing on disk (the fingerprint is marked failed so the UI can show a "file missing" indicator).

##### Input

* `id` — path parameter; the episode ID.

##### Output

Empty body (`200`).

#### `POST /api/episodes/{id}/est_date`

**Scope:** `series:write`

Update an episode's estimated release date. After writing, the series is re-estimated (the new date may anchor neighbouring episodes) and monitor status is refreshed, since `Future` mode considers the effective date. The date is a strict inbound timestamp.

##### Input

```json
{
  "date": "2026-06-15T00:00:00Z"
}
```

* `id` — path parameter; the episode ID.
* `date` — string, optional. An RFC 3339 timestamp with an explicit offset (e.g. `Z` or `±HH:MM`); `null` or omitted clears the estimated release date.

* On error: `400` for an invalid episode ID, or a timestamp that is not RFC 3339 with an explicit offset — zone-less (`2026-06-15T00:00:00`) and date-only (`2026-06-15`) values are rejected and never interpreted as UTC.

##### Output

Empty body (`200`).

#### `PUT /api/episodes/{id}/monitor`

**Scope:** `series:write`

Toggle a single episode's monitored flag. This is a direct write that sets `monitor_override=1`, so periodic sweeps respect the user's choice; the override is cleared when the series' monitor mode is changed, or self-heals once the mode's own evaluation matches the chosen value.

##### Input

```json
{
  "monitored": true
}
```

* `id` — path parameter; the episode ID.
* `monitored` — boolean, required. The desired monitored state for the episode.

##### Output

Empty body (`200`).

* On error: `400` for an invalid episode ID.

#### `POST /api/series/{id}/actions/delete_episode_data`

**Scope:** `series:write`

Delete all episode data for a series. Episode rows are removed and `metadata_last_synced_at` is cleared so the background refresh loop re-fetches metadata on its next tick. Returns `400` if the series ID is malformed and `404` if the series does not exist.

##### Input

* `id` — path parameter; the series ID.

##### Output

No content (`204`).

#### `POST /api/series/{id}/actions/reset_configuration`

**Scope:** `series:write`

Reset a series' configuration back to defaults without hiding it or touching episode data. Preserves `target_title`, `name`, `series_id`, `path`, and all episode data; resets `release_profile`, `qb_category`, `filters`, `scoring`, `quality_profile`, and `settings` (with `monitor_mode` reset to the default monitor mode). Returns `400` if the series ID is malformed and `404` if the series does not exist.

##### Input

* `id` — path parameter; the series ID.

##### Output

No content (`204`).

#### `POST /api/series/{id}/season/{season}/actions/delete_episode_data`

**Scope:** `series:write`

Delete all episode data for a single season of a series. The season is durably suppressed so it stays deleted across metadata resyncs, and `metadata_last_synced_at` is cleared so metadata is re-fetched on the next refresh tick. Returns `400` for a malformed series ID or an invalid season number, and `404` if the series does not exist.

##### Input

* `id` — path parameter; the series ID.
* `season` — path parameter; the season number, validated as `0`–`10000`.

##### Output

No content (`204`).

#### `POST /api/series/{id}/season/{season}/actions/reset_configuration`

**Scope:** `series:write`

Reset all overrides for a single season to defaults. Under normal numbering the season override entry is removed; under absolute numbering the canonical season's override is reset in place and the season is kept. Episode data is preserved in both cases. Returns `400` for a malformed series ID or an invalid season number, and `404` if the series does not exist.

##### Input

* `id` — path parameter; the series ID.
* `season` — path parameter; the season number, validated as `0`–`10000`.

##### Output

No content (`204`).

#### `POST /api/series/{id}/season/{season}/restore_metadata`

**Scope:** `series:write`

Restore a season's episode metadata from the per-provider cache, re-inserting it without calling the external API; falls back to a full metadata fetch via the queue on a cache miss. Clears the season's suppression first and re-applies monitor mode and release date estimation. Returns `400` for a malformed series ID or an invalid season number, and `404` if the series does not exist.

##### Input

* `id` — path parameter; the series ID.
* `season` — path parameter; the season number, validated as `0`–`10000`.

##### Output

No response body (`200`).

#### `POST /api/series/{id}/seasons`

**Scope:** `series:write`

Batch upsert seasons for a series. Each requested season is added to (or updated in) the series mapping with `cell_count` set to `episode_count`. Returns `400` for a malformed series ID, an empty `seasons` list, an `episode_count` below `1`, or an invalid season number, and `404` if the series does not exist.

##### Input

* `id` — path parameter; the series ID.

```json
{
  "seasons": [1, 2, 3],
  "episode_count": 10,
  "absolute_numbering": false
}
```

* `seasons` — array of integers, required. The season numbers to upsert; must be non-empty and each is validated as `0`–`10000`.
* `episode_count` — integer, required. Number of episode cells to render for each season; must be `>= 1`.
* `absolute_numbering` — boolean, required. When `true`, seasons are upserted into the absolute-numbering override map (`season_absolute`); otherwise into the normal `season` map.

##### Output

```json
["1", "2", "3"]
```

* `seasons[]` — each entry is an upserted season number serialized as a string (e.g. `"1"`), in request order.

#### `DELETE /api/series/{id}/metadata-cache`

**Scope:** `series:write`

Clear locally stored metadata snapshots for this series (the episode, season, and series caches plus the fetch log). When `provider` is supplied only that provider's cache is cleared; otherwise all providers' caches for the series are cleared. `metadata_last_synced_at` is reset accordingly so the next sync repopulates the cache. Returns `400` if the series ID is malformed and `404` if the series does not exist.

##### Input

* `id` — path parameter; the series ID.
* `provider` — optional query parameter; a provider instance ID. When provided, only that provider's cache entries are deleted.

##### Output

No content (`204`).

#### `PUT /api/series/{id}/episodes/{episode_id}/metadata`

**Scope:** `series:write`

Save custom metadata for a single episode. Provided values are written to the episode row and `metadata_source` is set to `'custom'` so the background poller will not overwrite them. Fields validated as noted below return `400`; `404` is returned if the series or the episode does not exist.

##### Input

* `id` — path parameter; the series ID.
* `episode_id` — path parameter; the episode ID.

```json
{
  "title": "The Long Night",
  "description": "Winter has come.",
  "runtime": 55,
  "image_url": "https://example.com/posters/ep1.jpg",
  "meta_date": "2026-06-18T20:30:00+09:00"
}
```

* `title` — string, optional. Episode title, max 500 characters. Written as-is: omitting it clears the stored title.
* `description` — string, optional. Episode description, max 5000 characters. When omitted, the existing value is kept.
* `runtime` — integer, optional. Runtime in minutes; must be between `0` and `1440`. When omitted, the existing value is kept.
* `image_url` — string, optional. Episode image URL; must start with `http://` or `https://` when non-empty. When omitted, the existing value is kept.
* `meta_date` — string, optional. Strict RFC 3339 timestamp with an explicit offset (e.g. `2026-06-18T20:30:00+09:00` or `2026-06-18T11:30:00Z`), normalized to UTC. An empty string is treated as absent. Zone-less or date-only values are rejected with `400`. When omitted, the existing value is kept.

##### Output

No response body (`200`).

#### `DELETE /api/series/{id}/episodes/{episode_id}/metadata`

**Scope:** `series:write`

Clear metadata for one episode. All metadata fields are set to `NULL` and `metadata_source` is set to `'cleared'` so the background poller will not re-populate them. Returns `400` for a malformed episode ID and `404` if the series or the episode does not exist.

##### Input

* `id` — path parameter; the series ID.
* `episode_id` — path parameter; the episode ID.

##### Output

No response body (`200`).

#### `POST /api/series/{id}/episodes/{episode_id}/restore_metadata`

**Scope:** `series:write`

Restore a single episode's metadata from the per-provider cache, re-inserting the cached values without calling the external API; falls back to a full metadata fetch via the queue on a cache miss. The episode's `metadata_source` is reset first so the cached values can overwrite it. Returns `400` if the episode has no season number or its ID cannot be parsed, and `404` if the series does not exist.

##### Input

* `id` — path parameter; the series ID.
* `episode_id` — path parameter; the episode ID.

##### Output

No response body (`200`).

#### `POST /api/series/{id}/season/{season}/match`

**Scope:** `series:write`

Match a season back to provider metadata: episode rows in the season are reset to provider state (metadata fields nulled, `metadata_source` set to `NULL`). The season's suppression is cleared, episodes the provider does not list are unassigned (their files durably blocked so a rescan cannot re-adopt them) and removed, and `metadata_last_synced_at` is reset so the background poller re-fetches metadata. In absolute-numbering mode the season resolves to the canonical absolute season and the label is not consulted. Returns `400` for an unresolvable season label and `404` if the series does not exist.

##### Input

* `id` — path parameter; the series ID.
* `season` — path parameter; the season label, resolved to a season number.

##### Output

No response body (`200`).

#### `POST /api/series/{id}/metadata/match`

**Scope:** `series:write`

Match an entire series back to provider metadata: all episode rows are reset to provider state (metadata fields nulled, `metadata_source` set to `NULL`), all season suppressions are cleared, and `metadata_last_synced_at` is reset so the background poller re-fetches metadata. Returns `400` if the series ID is malformed and `404` if the series does not exist.

##### Input

* `id` — path parameter; the series ID.

##### Output

No response body (`200`).

---

## 5. Search Endpoints

**Scope:** `search` — plus `queue:write` for any mode that queues downloads (`mode="auto_episode"` on `/api/search`, and every `/api/search/auto-season` endpoint). A missing `queue:write` returns the standard 403 (see §11).

### `POST /api/search`

**Scope:** `search` (plus `queue:write` for `mode="auto_episode"`)

`mode` selects the flow: `auto_episode` runs an alias-aware auto-search, queues the best match and returns it (needs `queue:write`); a missing/other mode runs the manual search and returns ranked results; `auto_season`/`season` is rejected with `400` — use `/api/search/auto-season`. Auto modes ignore the `query` field — the backend resolves aliases and builds the query. The manual `query` is validated: non-empty, ≤ 255 chars.

#### Input

```json
{
  "query": "Example Show",
  "mode": "auto_episode",
  "series_id": "550e8400-e29b-41d4-a716-446655440000",
  "episode_id": null,
  "season": "01",
  "episode_numbers": [1],
  "is_user_requested": true
}
```

* `query` — string, required. Search text for the manual flow; ignored by `auto_episode` (the backend builds the query from the series and its aliases). Manual mode validates it: non-empty, at most 255 characters.
* `mode` — string, optional. `auto_episode` runs alias-aware auto-search, queues the best match, and returns it (requires `queue:write`); omitted or any other value runs the manual ranked search; `auto_season`/`season` is rejected with `400`.
* `series_id` — string, optional. Series UUID; required by `auto_episode` (otherwise `400`) and used to load the profile-aware scoring and checks for manual search.
* `episode_id` — string, optional. Episode UUID; when set, manual search scopes the episode check to this episode. Not read by `auto_episode`.
* `season` — string, optional. Season label (e.g. `"01"`); required by `auto_episode` (otherwise `400`).
* `episode_numbers` — array of integers, optional. Episodes to search; required and non-empty for `auto_episode` (otherwise `400`). Each number must be positive.
* `is_user_requested` — boolean, optional, default `true`. Marks a user-initiated search; at organize time it temporarily enables upgrade evaluation.

#### Output

```json
[
  {
    "title": "Example.Show.S01E01.1080p.WEB-DL",
    "size": 1610612736,
    "seeders": 42,
    "leechers": 3,
    "link": "magnet:?xt=urn:btih:0f8fad5bd9cb469fa16570867728950e",
    "source": "Nyaa",
    "score": 120,
    "published": "2026-06-18T20:00:00+00:00",
    "is_season_pack": false,
    "download_id": "0f8fad5bd9cb469fa16570867728950e",
    "submitter": "SubsGroup",
    "release_checks": [],
    "queue_action": ""
  }
]
```

* `title` — release title.
* `size` — release size in bytes.
* `seeders` — seeders reported by the source, or `null`.
* `leechers` — leechers reported by the source, or `null`.
* `link` — download URL or magnet link, or `null`.
* `source` — source plugin instance name the result came from.
* `score` — computed release score (default `0`).
* `published` — source feed publish date, RFC 3339 with explicit UTC offset, or `null`.
* `is_season_pack` — `true` when the release is a season/complete pack (default `false`).
* `download_id` — unique download identifier (e.g. an infoHash); also accepted on input as `info_hash`, or `null`.
* `submitter` — release group / submitter, or `null`.
* `release_checks` — the auto-search acceptance gates evaluated for the result, in display order: `rejected`, `episode`, `profile`. Each entry is `{ "id", "label", "description", "passed" }`; a failing check carries a short reason in `description`. Manual search annotates failing results instead of hiding them, and only failures are shown in the UI. Empty for `auto_episode` results.
* `queue_action` — auto-queue action taken: `""`, `added`, `replaced`, `skipped`, or `merged`. Empty for manual search; set by `auto_episode`.

`auto_episode` returns a one-element array holding the selected release with `queue_action` set to the queue outcome, or an empty array when nothing suitable was found. Manual mode returns all ranked results (score ↓ → date ↓ → seeders ↓). A `400` is returned for an empty/oversized `query`, a missing `series_id`/`season`/`episode_numbers` (or empty `episode_numbers`) in `auto_episode`, an `auto_season`/`season` mode, or when no source plugins support manual search. A missing `queue:write` with `auto_episode` returns the standard `403` scope error.

### `POST /api/search/auto-season`

**Scope:** `search` + `queue:write`

Auto-search a whole season — spawns background retry, queues best matches.

#### Input

```json
{
  "query": "",
  "mode": "auto_season",
  "series_id": "550e8400-e29b-41d4-a716-446655440000",
  "season": "01",
  "episode_numbers": [1, 2, 3],
  "is_user_requested": true
}
```

* `query` — string, required by the payload but ignored; the backend resolves aliases and builds the search query.
* `mode` — string, required. Must be `auto_season` (legacy `season` also accepted); any other value returns `400`.
* `series_id` — string, required. Series UUID (otherwise `400`).
* `season` — string, required. Season label (e.g. `"01"`); a leading `S`/`s` is stripped and the remainder must be a valid season number (otherwise `400`).
* `episode_numbers` — array of integers, required. Non-empty; each episode must be positive (otherwise `400`).
* `is_user_requested` — boolean, optional, default `true`. Passed through to queued items so organize time treats them as user-requested.

#### Output

```json
[]
```

No results are returned inline (`[]`). The season auto-search runs asynchronously in the background — retrying with a 5-minute delay between rounds and queueing the best matches — so poll `GET /api/search/auto-season/{series_id}/{season}/status` for progress and `GET /api/queue` for the queued items. A `409` is returned when a search is already running for the same series + season.

### `GET /api/search/auto-season/{series_id}/{season}/status`

**Scope:** `search` + `queue:write`

Get auto-season search status.

#### Input

* `series_id` — path parameter, required. Series UUID.
* `season` — path parameter, required. Season number; validated (invalid → `400`).

#### Output

```json
{
  "running": true
}
```

* `running` — boolean, whether an auto-search task is currently active for this series + season. The frontend polls this to re-enable the button once it becomes `false`.

---

## 6. Queue & Download Endpoints

### Queue Read

**Scope:** `queue:read`

#### `GET /api/queue`

##### Input

None.

##### Output

```json
[
  {
    "id": 42,
    "downloaded_at": "2026-06-18T20:00:00+00:00",
    "media_name": "Example.Show.S01E01.1080p.WEB-DL",
    "media_link": "magnet:?xt=urn:btih:0f8fad5bd9cb469fa16570867728950e",
    "series_title": "Example Show",
    "season": "01",
    "episode": 1,
    "episode_end": null,
    "episode_id": "0f8fad5b-d9cb-469f-a165-70867728950e",
    "score": 120,
    "is_user_requested": true,
    "is_manual": false,
    "multi_targets": null,
    "status": "Downloading",
    "download_id": "0f8fad5bd9cb469fa16570867728950e",
    "downloader_id": "0f8fad5bd9cb469fa16570867728950e",
    "client_id": "qBittorrent",
    "progress": 0.42,
    "no_progress": false,
    "no_progress_minutes": null,
    "supports_pause_resume": true,
    "error_message": null,
    "series_id": "550e8400-e29b-41d4-a716-446655440000",
    "is_season_pack": false,
    "category": "Series",
    "retry_count": 0,
    "last_progress": 0.4,
    "no_progress_since": "2026-06-18T20:05:00+00:00",
    "next_retry_at": null,
    "episode_intentions": null,
    "scoring_size_bytes": 1610612736,
    "scoring_seeders": 42,
    "scoring_episode_count": null,
    "submitter": "SubsGroup",
    "quality_profile_id": "hd-1080p",
    "version": 1
  }
]
```

* `id` — queue row id.
* `downloaded_at` — RFC 3339 UTC. When the item was added to the queue.
* `media_name` — release/media name.
* `media_link` — download URL or magnet link.
* `series_title` — resolved series title (`"Manual Download"` for context-less items).
* `season` — season label, or `null` when smart-link resolves it from disk after completion.
* `episode` — known episode number, or `null` when the episode is not yet resolved.
* `episode_end` — last episode of a multi-episode range (e.g. `4` for `S01E01-04`), or `null` for a single episode.
* `episode_id` — episode UUID FK, or `null` while pending smart-link resolution.
* `score` — release score used for upgrade/duplicate decisions.
* `is_user_requested` — `true` when the user explicitly requested the download; at organize time it temporarily enables upgrade evaluation.
* `is_manual` — `true` when the user picked this specific release (link + download id), and exempt from no-progress autoresolve.
* `multi_targets` — JSON array of additional `(series_id, season, episode)` targets sharing this download, or `null` for a single-target download.
* `status` — queue status, e.g. `Queued`, `Downloading`, `Paused`, `Deleting`, or `Failed`.
* `download_id` — source plugin's download identifier (e.g. an infoHash), or `null`.
* `downloader_id` — the download client's torrent hash, or `null` before dispatch.
* `client_id` — stable id of the client that accepted the item (e.g. `qBittorrent`), or `null` for legacy items.
* `progress` — live download progress (`0.0`–`1.0`) for in-flight items; otherwise the stored value or `null`.
* `no_progress` — transient, computed at read time: `true` when a `Downloading` item has stalled past the configured threshold.
* `no_progress_minutes` — transient: minutes without progress, for the warning tooltip, or `null`.
* `supports_pause_resume` — transient: whether the assigned client supports pause/resume.
* `error_message` — last failure message, or `null` for non-failed items.
* `series_id` — series UUID linking this item to its mapping; empty string for legacy items.
* `is_season_pack` — `true` when the item is a season/complete pack.
* `category` — category passed to the download client; empty string means the client default.
* `retry_count` — content-path resolution retries; `0` means not currently in a retry cycle.
* `last_progress` — last sampled progress, or `null`.
* `no_progress_since` — RFC 3339 UTC. When progress last advanced; `null` until the first sample.
* `next_retry_at` — RFC 3339 UTC. Next allowed retry time, or `null`.
* `episode_intentions` — JSON array of `EpisodeIntention`, or `null` for legacy/manual items.
* `scoring_size_bytes` — raw size input stored for retroactive rescore, or `null`.
* `scoring_seeders` — raw seeders input stored for retroactive rescore, or `null`.
* `scoring_episode_count` — raw episode-count input stored for retroactive rescore, or `null`.
* `submitter` — release group / submitter, or `null`.
* `quality_profile_id` — quality profile UUID snapshotted from the series mapping at queue time, or `null`.
* `version` — parsed release version (default `1`).

Timestamps `downloaded_at`, `next_retry_at` are RFC 3339 UTC. `progress`, `no_progress`, `no_progress_minutes`, and `supports_pause_resume` are derived live at read time from the download client and are not persisted.

### Queue Write

**Scope:** `queue:write`

#### `POST /api/downloads`

Add a download to the queue — returns the queue id and outcome.

##### Input

```json
{
  "link": "magnet:?xt=urn:btih:0f8fad5bd9cb469fa16570867728950e",
  "download_id": "0f8fad5bd9cb469fa16570867728950e",
  "category": "Series",
  "episode_id": "0f8fad5b-d9cb-469f-a165-70867728950e",
  "tag": "1080p",
  "title": "Example.Show.S01E01.1080p.WEB-DL",
  "score": 120,
  "series_id": "550e8400-e29b-41d4-a716-446655440000",
  "is_season_pack": false,
  "is_user_requested": true,
  "size": 1610612736,
  "seeders": 42,
  "upload_date": "2026-06-18T20:00:00+00:00"
}
```

* `link` — string, required. Download URL or magnet link; validated (a magnet `magnet:?` or a URL with a protocol).
* `download_id` — string, required. Unique source identifier (e.g. Nyaa's infoHash); the system cannot track the download without it.
* `category` — string, optional, default `null`. Category passed to the download client; empty string means the client default.
* `episode_id` — string, optional, default `null`. Episode UUID; when present it resolves the episode context (series, season, episode).
* `tag` — string, optional, default `null`. Free-form tag; maximum 100 characters.
* `title` — string, optional, default `null`. Release title; defaults to `link` when absent; maximum 255 characters.
* `score` — integer, optional, default `0`. Release score; must not be negative.
* `series_id` — string, optional, default `null`. Series UUID; used when no `episode_id` is provided (series-level download).
* `is_season_pack` — boolean, optional, default `false`. Explicit pack flag carried from the search result.
* `is_user_requested` — boolean, optional, default `true`. Marks a user-requested download.
* `size` — integer, optional, default `null`. Release size in bytes, stored for scoring reconstruction.
* `seeders` — integer, optional, default `null`. Seeder count, stored for scoring reconstruction.
* `upload_date` — string, optional, default `null`. Must be strict RFC 3339 with an explicit offset; a malformed value returns `400`.

##### Output

```json
{
  "outcome": "added",
  "queue_id": 42
}
```

* `outcome` — one of `added`, `replaced`, `skipped`, or `merged`.
* `queue_id` — queue row id; `null` only when nothing was queued (`skipped`).

The request only *queues* the download — the download client is assigned later by the organizer, so `client_id` is not known here. Poll `GET /api/queue` with `queue_id` to read the assigned `client_id`. A `400` is returned for an empty/invalid link, a negative score, or an oversized title/tag (or an episode whose episode ID cannot be generated); a `404` when a provided `series_id` has no series mapping; and a `503` when no downloader is available.

#### `DELETE /api/queue/{id}`

Remove from queue.

##### Input

* `id` — path parameter, required. Queue row id (must be positive, otherwise `400`).

##### Output

No content (`200 OK`).

#### `POST /api/queue/{id}/pause`

Pause an active download.

##### Input

* `id` — path parameter, required. Queue row id (must be positive, otherwise `400`).

##### Output

No content (`200 OK`).

A `404` is returned when the id is not in the queue; a `400` when the item has no downloader id or its client does not support pause/resume; and a `503` when the client cannot be reached.

#### `POST /api/queue/{id}/resume`

Resume a paused download.

##### Input

* `id` — path parameter, required. Queue row id (must be positive, otherwise `400`).

##### Output

No content (`200 OK`).

A `404` is returned when the id is not in the queue; a `400` when the item has no downloader id or its client does not support pause/resume; and a `503` when the client cannot be reached.

#### `POST /api/queue/{id}/delete`

Remove from queue and optionally delete files.

##### Input

```json
{
  "delete_files": true
}
```

* `id` — path parameter, required. Queue row id (must be positive, otherwise `400`).
* `delete_files` — boolean, required. When `true`, the client also removes the downloaded files from disk.

##### Output

No content (`200 OK`).

A `404` is returned when the id is not in the queue; a `400` when the item has no downloader id; and a `503` when the client cannot be reached.

#### `POST /api/queue/{id}/retry`

Retry a failed download.

##### Input

* `id` — path parameter, required. Queue row id (must be positive, otherwise `400`).

##### Output

No content (`200 OK`).

A `404` is returned when the id is not in the queue, and a `400` when the item's status is not `Failed`.

---

## 7. System Endpoints

### System Read

#### `GET /api/system/health`

**Scope:** `system:read`

##### Input

None.

##### Output

```json
{
  "status": "Healthy",
  "uptime_seconds": 86400,
  "memory_used": 1073741824,
  "memory_total": 17179869184,
  "cpu_usage": 3.5,
  "db_integrity_status": "Healthy",
  "storage_health": [
    {
      "path": "/media/tv",
      "free_bytes": 536870912000,
      "total_bytes": 1099511627776
    }
  ],
  "dir_permissions": [
    {
      "path": "/media/tv",
      "can_read": true,
      "can_write": true
    }
  ],
  "ffmpeg_installed": true,
  "ffmpeg_version": "6.1.1",
  "downloader_is_disabled": false
}
```

* `status` — overall health string; the handler returns the literal `"Healthy"`.
* `uptime_seconds` — host uptime in seconds (`sysinfo::System::uptime`).
* `memory_used` — used system memory in bytes.
* `memory_total` — total system memory in bytes.
* `cpu_usage` — global CPU usage as a percentage (float).
* `db_integrity_status` — `"Healthy"` when `PRAGMA quick_check` returns `ok`, otherwise `Error: <detail>`.
* `storage_health` — per-path disk capacity, checked for each destination root, downloader path, and series parent directory (paths are normalized and deduplicated, and the list is sorted).
* `storage_health[].path` — normalized filesystem path.
* `storage_health[].free_bytes` — available bytes on the mount containing `path`.
* `storage_health[].total_bytes` — total bytes of the mount containing `path`.
* `dir_permissions` — read/write capability probe for each checked path.
* `dir_permissions[].path` — normalized filesystem path.
* `dir_permissions[].can_read` — `true` when the directory can be listed.
* `dir_permissions[].can_write` — `true` when a temporary file can be written and removed (a real write test, not just an access check).
* `ffmpeg_installed` — `true` when `ffprobe` is available on `PATH`.
* `ffmpeg_version` — `ffprobe` version string, or `null` when not installed.
* `downloader_is_disabled` — `true` when every download client is disabled (permanently or temporarily); `false` when no downloader is configured.

This endpoint runs expensive checks (disk I/O, DB integrity, ffprobe version), so liveness polling should use `GET /api/public/ping` instead. A caller whose key lacks `system:read` receives `403` with the `{"error": "..."}` envelope and an RFC 6750 `WWW-Authenticate` challenge.

#### `GET /api/system/plugins-metrics`

**Scope:** `system:read`

##### Input

None.

##### Output

```json
[
  {
    "type_id": "downloader.qbittorrent",
    "display_name": "qBittorrent",
    "healthy": true,
    "calls": 1523,
    "errors": 2,
    "restarts": 0,
    "queue_depth": 4
  }
]
```

* `type_id` — canonical external plugin type id (`category.plugin_type` form).
* `display_name` — human-readable type name.
* `healthy` — whether the type's shared process is currently healthy.
* `calls` — total RPC calls made to the type.
* `errors` — total errors recorded for the type.
* `restarts` — number of process restarts.
* `queue_depth` — pending request queue depth.

Per-**type** plugin process metrics, read in-process (no RPC to plugin processes). External types only — internal plugins have no process and are not listed.

#### `GET /api/system/about`

**Scope:** `system:read`

##### Input

None.

##### Output

```json
{
  "version": "1.2.3",
  "os": "linux",
  "git_commit": "9f8e7d6c5b4a3210",
  "build_date": "2026-06-18T20:00:00Z",
  "license": "MIT",
  "description": "Jumbie media server"
}
```

* `version` — `CARGO_PKG_VERSION`, baked in at compile time.
* `os` — target OS (`std::env::consts::OS`, e.g. `linux`).
* `git_commit` — commit hash injected at build time by `build.rs`.
* `build_date` — build timestamp injected at build time by `build.rs`.
* `license` — `CARGO_PKG_LICENSE`.
* `description` — `CARGO_PKG_DESCRIPTION`.

#### `GET /api/system/memory`

**Scope:** `system:read`

##### Input

None.

##### Output

```json
{
  "rss_bytes": 123456789,
  "peak_bytes": 234567890,
  "jemalloc": {
    "allocated_bytes": 100000000,
    "active_bytes": 120000000,
    "mapped_bytes": 200000000,
    "resident_bytes": 180000000,
    "overhead_bytes": 100000000,
    "decay_goal_ms": 5000
  },
  "logs": {
    "buffer_bytes": 1048576,
    "buffer_entries": 2048,
    "buffer_budget_bytes": 4194304,
    "disk_bytes": 8388608
  },
  "note": "jemalloc returns freed pages to the OS within ~5 seconds via background thread decay"
}
```

* `rss_bytes` — process RSS read from `/proc/self/status` (`VmRSS`) in bytes; `0` when unavailable.
* `peak_bytes` — process peak virtual memory (`VmPeak`) in bytes; `0` when unavailable.
* `jemalloc` — allocator breakdown from jemalloc `mallctl` stats (present because jemalloc is the global allocator).
* `jemalloc.allocated_bytes` — bytes currently allocated by the application.
* `jemalloc.active_bytes` — bytes in active pages.
* `jemalloc.mapped_bytes` — bytes mapped by the allocator.
* `jemalloc.resident_bytes` — resident bytes.
* `jemalloc.overhead_bytes` — `mapped_bytes − allocated_bytes` (saturating).
* `jemalloc.decay_goal_ms` — decay goal in milliseconds; currently the fixed value `5000`.
* `logs` — log subsystem footprint (in-memory buffer plus size-rolled files on disk).
* `logs.buffer_bytes` — estimated in-memory size of the parsed log ring buffer.
* `logs.buffer_entries` — number of entries currently held in the buffer.
* `logs.buffer_budget_bytes` — configured buffer byte budget (`LOG_BUFFER_BYTES`, 4 MiB).
* `logs.disk_bytes` — total size of the size-rolled log files on disk.
* `note` — fixed explanatory string about jemalloc's background page decay.

#### `GET /api/media-info-scan/counts`

**Scope:** `system:read`

##### Input

None.

##### Output

```json
{
  "550e8400-e29b-41d4-a716-446655440000": 3,
  "6ba7b810-9dad-11d1-80b4-00c04fd430c8": 1
}
```

* object keys — series ids.
* object values — count of active scan paths whose path starts with (component-wise) the series' configured path.

Only series with a configured path and at least one matching active scan path are included; the map is empty (`{}`) when no scan paths are active. Series without a configured path are skipped (no title-based matching).

#### `GET /api/media-info-scan/counts/{series_id}`

**Scope:** `system:read`

##### Input

* `series_id` — path parameter, required. Series UUID.

##### Output

```json
{
  "550e8400-e29b-41d4-a716-446655440000": 3
}
```

* object keys — the requested series id, present only when its count is greater than `0`.
* object values — count of active scan paths whose path starts with (component-wise) the series' configured path.

Returns an empty object (`{}`) when there are no active scan paths, the series is unknown, or the series has no configured path. A lightweight alternative to `GET /api/media-info-scan/counts` for showing progress on a single series.

#### `GET /api/status`

**Scope:** `system:read`

##### Input

None.

##### Output

```json
{
  "download_queue_has_failed": false,
  "download_queue_has_items": true,
  "rename_queue_has_failed": false,
  "rename_queue_populated": true,
  "downloader_disabled": false,
  "wanted_has_items": true,
  "locked_series": ["550e8400-e29b-41d4-a716-446655440000"],
  "active_operations": [
    {
      "id": "task-1",
      "operation_type": "batch_move",
      "total": 10,
      "completed": 4,
      "finished": false,
      "finished_at_ms": null,
      "success_count": 4,
      "failed": 0,
      "errors": []
    }
  ]
}
```

* `download_queue_has_failed` — `true` when any download queue item is in the `Failed` status.
* `download_queue_has_items` — `true` when the download queue is non-empty.
* `rename_queue_has_failed` — `true` when there are failed rename operations.
* `rename_queue_populated` — `true` when there are failed, processing, or pending rename operations.
* `downloader_disabled` — `true` when no download client is configured or all clients are unavailable.
* `wanted_has_items` — `true` when wanted episodes exist under the user's display preferences.
* `locked_series` — series ids currently locked by an in-progress modification; may be empty.
* `active_operations` — active batch operations; may be empty.
* `active_operations[].id` — operation id.
* `active_operations[].operation_type` — operation kind (e.g. `batch_move`).
* `active_operations[].total` — total item count.
* `active_operations[].completed` — completed item count.
* `active_operations[].finished` — whether the operation has finished.
* `active_operations[].finished_at_ms` — epoch milliseconds completion time, or `null` while running.
* `active_operations[].success_count` — number of successful items.
* `active_operations[].failed` — number of failed items.
* `active_operations[].errors` — collected error messages.

Lightweight indicator payload the frontend polls on a 60-second interval. Kept separate from `GET /api/system/health` because that endpoint runs expensive checks that should not fire every 60 seconds.

### Wanted Read

#### `GET /api/wanted`

**Scope:** `wanted:read`

##### Input

* `page` — optional, default `0`. Zero-based page index.
* `limit` — optional, default `50`. Page size.
* `sort` — optional. One of `series`, `episode`, `age`, `status`; falls back to the stored `wanted` table preference, then `age`.
* `order` — optional, `asc` or `desc`. Falls back to the stored preference, then descending.
* `search` — optional. Case-insensitive match on series title, episode title, and season/episode numbers (accepts plain `3`/`03` as well as `e3`/`s3`).

##### Output

```json
{
  "items": [
    {
      "series_id": "550e8400-e29b-41d4-a716-446655440000",
      "series_title": "Example Show",
      "episode_id": "0f8fad5b-d9cb-469f-a165-70867728950e",
      "season": "1",
      "episode": 3,
      "title": "The Third One",
      "eff_date": "2026-06-18T20:00:00+00:00",
      "dates": {
        "meta_date": "2026-06-18T20:00:00+00:00",
        "upload_date": "2026-06-18T22:00:00+00:00",
        "est_date": null
      },
      "status": "missing"
    }
  ],
  "total": 1,
  "page": 0,
  "page_size": 50
}
```

* `items` — wanted episodes for the requested page.
* `total` — total wanted episodes after filtering, before pagination.
* `page` — echoed zero-based page index.
* `page_size` — echoed page size (the resolved `limit`).
* `items[].series_id` — series UUID.
* `items[].series_title` — series display title.
* `items[].episode_id` — episode UUID.
* `items[].season` — season string, or `null`.
* `items[].episode` — episode number.
* `items[].title` — episode title, or `null`.
* `items[].eff_date` — server-resolved effective release date, RFC 3339 UTC (empty string when none is available).
* `items[].dates` — `meta_date` (provider airdate), `upload_date` (source feed), `est_date` (estimate); each RFC 3339 UTC or `null`.
* `items[].status` — wanted status string (e.g. `missing`).

A database error returns an empty page rather than an error response.

### Activity Read

#### `GET /api/activity`

**Scope:** `activity:read`

##### Input

* `page` — optional, default `0`. Zero-based page index.
* `limit` — optional, default `50`. Page size.
* `sort` — optional. One of `timestamp`, `type`, `title`, `status`; falls back to the stored `activity` table preference, then `timestamp`.
* `order` — optional, `asc` or `desc`. Falls back to the stored preference, then descending.
* `search` — optional. Case-insensitive match on `title`, `details`, and `status`.
* `types` — optional. Comma-separated lowercase activity type names (`download`, `import`, `metadata`, `reassign`, `assign`, `analyze`, `delete`, `unassign`); falls back to the stored activity filter. An empty or absent value means "show all".

##### Output

```json
{
  "items": [
    {
      "id": "activity-1",
      "timestamp": "2026-06-18T20:00:00+00:00",
      "activity_type": "download",
      "title": "Example Show S01E03",
      "details": "Downloaded 1080p release",
      "status": "Success"
    }
  ],
  "total": 1,
  "page": 0,
  "page_size": 50
}
```

* `items` — activity entries for the requested page.
* `total` — total entries after filtering, before pagination.
* `page` — echoed zero-based page index.
* `page_size` — echoed page size (the resolved `limit`).
* `items[].id` — activity entry id.
* `items[].timestamp` — event time, RFC 3339 UTC.
* `items[].activity_type` — one of `download`, `import`, `metadata`, `reassign`, `assign`, `analyze`, `delete`, `unassign`.
* `items[].title` — subject of the event (series/episode label).
* `items[].details` — optional detail text, or `null`.
* `items[].status` — result status string (e.g. `Success`).

Reads the most recent 1000 entries, then filters, sorts, and paginates in memory; a database error returns an empty page rather than an error response.

### Logs Read

#### `GET /api/system/logs`

**Scope:** `logs:read`

##### Input

* `page` — optional, default `0` (clamped to a minimum of `0`). Zero-based page index.
* `limit` — optional, default `100`, clamped to `1`–`1000`. Page size.
* `sort` — optional. One of `level`, `message`, `timestamp`; falls back to the stored `system_logs` table preference, then `timestamp`.
* `order` — optional, `asc` or `desc`. Falls back to the stored preference, then descending.
* `search` — optional. Case-insensitive substring match on the log `message`.
* `min_level` — optional. Minimum level, one of `TRACE`, `DEBUG`, `INFO`, `WARN`, `ERROR`; falls back to the stored `logs.min_level` preference. Unknown values are excluded by the priority filter.

##### Output

```json
{
  "items": [
    {
      "timestamp": "2026-06-19T08:10:27.123Z",
      "level": "INFO",
      "message": "jumbie: starting up"
    }
  ],
  "total": 1,
  "page": 0,
  "page_size": 100
}
```

* `items` — log entries for the requested page.
* `total` — total entries after filtering, before pagination.
* `page` — echoed zero-based page index.
* `page_size` — echoed page size (the resolved `limit`).
* `items[].timestamp` — log event time, RFC 3339 UTC.
* `items[].level` — one of `TRACE`, `DEBUG`, `INFO`, `WARN`, `ERROR`.
* `items[].message` — log message text.

Entries are served from the in-memory log buffer (seeded at startup from the log files, then appended by the tracing layer) — no disk I/O during normal operation.

#### `GET /api/system/logs/file`

**Scope:** `logs:read`

##### Input

* `head` — optional, integer. Return the first N matching lines (oldest file first). Mutually exclusive with `tail`.
* `tail` — optional, integer. Return the last N matching lines (newest file first, scanning backward). Mutually exclusive with `head`. When neither is given, the count defaults to `1000`.
* `level` — optional, default `INFO`. Minimum level to include (e.g. `debug`).

The requested count is clamped to `1`–`100000`.

##### Output

```text
2026-06-19T08:10:27.123Z INFO jumbie: starting up
2026-06-19T08:11:02.456Z WARN jumbie::api: slow request
```

* Each line — a raw on-disk log line in the form `<timestamp> <LEVEL> <message>`.
* Lines — read only from the size-rolled files (`jumbie.log`, `jumbie.log.N`), oldest file first, and joined with newlines (with a trailing newline when non-empty); the response `Content-Type` is `text/plain; charset=utf-8`.

Serves the merged on-disk history (8 MiB budget), bypassing the in-memory buffer, so it is useful for issues older than the buffer. Returns `400` when both `head` and `tail` are supplied, or when `level` is not a recognized level.

#### `GET /api/system/log-level`

**Scope:** `logs:read`

##### Input

None.

##### Output

```json
{
  "level": "INFO"
}
```

* `level` — base level extracted from the configured `EnvFilter` string (the last comma-separated segment, uppercased).

The frontend uses this to limit its level selector to levels at or above the backend's actual logging threshold.

### Files Read

#### `GET /api/system/organized_series`

**Scope:** `files:read`

##### Input

None.

##### Output

```json
[
  {
    "folder_name": "Example Show",
    "absolute_path": "/media/tv/Example Show",
    "is_tracked": true,
    "is_monitored": true,
    "monitor_mode": "all",
    "series_id": "550e8400-e29b-41d4-a716-446655440000",
    "hidden_in_library": false,
    "selected": false,
    "completion_status": "Partial",
    "total_episodes_expected": 20,
    "total_episodes_organized": 8,
    "season_counts": [
      { "season": 1, "organized": 8, "expected": 12 },
      { "season": 2, "organized": 0, "expected": 8 }
    ],
    "locked": false
  }
]
```

* `folder_name` — directory name.
* `absolute_path` — absolute filesystem path of the folder.
* `is_tracked` — `true` when a series mapping references the folder.
* `is_monitored` — `true` unless `monitor_mode` is `none`.
* `monitor_mode` — one of `all`, `future`, `missing`, `existing`, `pilot`, `firstSeason`, `specials`, `none`; `null` for untracked folders.
* `series_id` — series UUID, or `null` for untracked folders.
* `hidden_in_library` — whether the series is hidden from the library list.
* `selected` — always `false` (selection state is client-side).
* `completion_status` — `NotStarted`, `Partial`, or `Complete`; `null` for untracked folders.
* `total_episodes_expected` — expected episode count from season config and metadata.
* `total_episodes_organized` — episodes that have a file path on disk.
* `season_counts` — per-season `{ season, organized, expected }` counts, season-ascending; omitted for untracked folders. Summing `organized`/`expected` yields `total_episodes_organized`/`total_episodes_expected`.
* `locked` — `true` when an in-progress modification (batch move, reorganize, path update, delete, etc.) locks the series.

Lists every directory under all destination roots, annotated with tracking status, monitor state, and library visibility. Tracked series whose directory does not yet exist on disk are also listed, and the results are sorted case-insensitively by folder name. Untracked folders carry default values (`monitor_mode: null`, `series_id: null`, zeroed episode counts).

### Files Write

#### `POST /api/system/validate-path`

**Scope:** `files:write`

Validate a filesystem path for use before creating or editing a series. The raw path as entered is trimmed and resolved (template variables such as `${series}` are **not** expanded), checked for length, traversal (`..`), double slashes, directory-ness, and write access, then checked for a collision with an existing series mapping. In the create flow it also resolves folder-level collisions on disk. The endpoint always returns `200`: an invalid or colliding path is reported through `is_valid: false` / `message`, not an HTTP error.

##### Input

```json
{
  "path": "/media/tv/My Show",
  "series_id": "3f2b1c4e-8a7d-4c5b-9e10-2f6a1b7c8d9e",
  "resolve_collisions": true
}
```

* `path` — string, required. Path to validate. Rejected (via `is_valid: false`) when empty, longer than the max path length, contains `..` or `//`, is not a directory, or the nearest existing ancestor is not writable. Root containment is intentionally not enforced — write access to the nearest existing directory is the actual access test.
* `series_id` — string, optional. Series ID to exclude from collision checks, so an edit that saves the same path back does not flag itself. Omit for a new series.
* `resolve_collisions` — boolean, optional, default `false`. When `true` (Add Series form), the folder name is sanitized per the illegal-char policy and folder collisions are resolved per the organization config (rename → the suffixed path is returned in `resolved_path`; skip → invalid). When `false`, custom paths are honored verbatim, surfacing host-illegal characters as invalid.

##### Output

```json
{
  "is_valid": true,
  "message": "Valid path",
  "resolved_path": "/media/tv/My Show"
}
```

* `is_valid` — boolean. Whether the path is usable.
* `message` — string. Human-readable result; `"Valid path"` on success, otherwise the failure or collision reason (e.g. `"The path '/media/tv/My Show' is already used by series 'My Show'"`). The claiming series is identified by name here rather than by `series_id`.
* `resolved_path` — string, optional (omitted when absent). The effective path after sanitization / collision resolution (create flow only); `null`/omitted when no resolution was performed.

#### `PUT /api/system/organized_series/{series_id}/toggle`

**Scope:** `files:write`

Set the monitor mode for a single series mapping.

##### Input

```json
{
  "monitor_mode": "missing"
}
```

* `monitor_mode` — enum, required. One of `all`, `future`, `missing`, `existing`, `pilot`, `firstSeason`, `specials`, `none` (camelCase). Stored on the series mapping.

##### Output

```json
{
  "status": "ok"
}
```

* `status` — string. Always `"ok"` on success.

* `400` for an invalid series ID; `404` when no mapping exists for the series.

#### `PUT /api/system/organized_series/{series_id}/visibility`

**Scope:** `files:write`

Hide or show a series in the main library view. Visibility is independent of monitoring, so a completed series can be hidden while remaining monitored for upgrades. Unhiding (`hidden_in_library: false`) spawns a debounced auto-scan of the series path so new files appear without manual work; hiding cancels queued/in-progress scans for that series (`data` is preserved for a later unhide).

##### Input

```json
{
  "hidden_in_library": true
}
```

* `hidden_in_library` — boolean, required. `true` hides the series from the library; `false` unhides it.

##### Output

```json
{
  "status": "ok"
}
```

* `status` — string. Always `"ok"` on success.

* `400` for an invalid series ID; `404` when no mapping exists for the series.

#### `POST /api/system/organized_series/bulk`

**Scope:** `files:write`

Bulk import organized series from a preview. Items with `selected: false` are skipped. Each accepted item creates a new series mapping (generating a fresh UUID and series key), creates its directory if missing, and — when `all_files_in_root` was set at preview time — enables `flatten_season_folders`. A path already claimed by another mapping, a filesystem-root path, or a directory-creation error is logged and skips only that item; the request still returns `200`.

##### Input

```json
{
  "items": [
    {
      "path": "/media/tv/Breaking Bad",
      "original_folder_name": "Breaking Bad",
      "final_title": "Breaking Bad",
      "season_count": 5,
      "episode_count": 62,
      "selected": true,
      "already_exists": false,
      "all_files_in_root": false
    }
  ],
  "scan_for_existing": true,
  "monitor_mode": "all",
  "quality_profile": "HD-1080p",
  "release_profile": "Default"
}
```

* `items` — array of `PreviewSeriesItem`, required. Preview items to import; each has `path`, `original_folder_name`, `final_title`, `season_count`, `episode_count`, `selected`, `already_exists`, and `all_files_in_root` (optional, default `false`). Items with `selected: false` are skipped.
* `scan_for_existing` — boolean, required. When `true`, each newly created series is scanned for existing episodes (`import_scan_for_series`, matching by directory path and `SXXEXX` patterns; duplicate season/episode tuples are left unassigned for the user to resolve).
* `monitor_mode` — enum or `null`, optional. Applied to each created series after the scan; one of `all`, `future`, `missing`, `existing`, `pilot`, `firstSeason`, `specials`, `none`.
* `quality_profile` — string or `null`, optional. Quality profile stored on the new mapping.
* `release_profile` — string or `null`, optional. Release profile stored on the new mapping.

##### Output

No content (`200`).

#### `POST /api/system/organized_series/batch_edit`

**Scope:** `files:write`

Batch edit for the Manage Folders view. Supported operations:

* `add_to_library` — Track an untracked folder (creating a new series mapping and scanning it synchronously so sizes are correct on first render) or unhide a hidden one.
* `remove_from_library` — Mark a tracked series as hidden (soft delete).
* `delete` — Delete mapping + data, and files from disk when `delete_files` is set. Series currently locked by another modification are skipped.

The response is always `{ "status": "ok" }`; individual failures are logged and do not fail the batch.

##### Input

```json
{
  "paths": [
    "/media/tv/Breaking Bad"
  ],
  "operation": "add_to_library",
  "delete_files": false
}
```

* `paths` — array of strings, required. Source paths (as shown in `OrganizedSeriesItem.absolute_path`) to operate on.
* `operation` — string, required. One of `add_to_library`, `remove_from_library`, `delete`; any other value returns `400` with `{"error": "Unknown operation: <op>"}`.
* `delete_files` — boolean, optional, default `false`. For `delete`, also removes the files from disk; when `false` only DB data is removed. Ignored for other operations.

##### Output

```json
{
  "status": "ok"
}
```

* `status` — string. Always `"ok"` on success.

#### `POST /api/system/organized_series/preview`

**Scope:** `files:write`

Preview a series import without creating anything. Returns one entry per detected series folder so the user can confirm/edit titles before importing (see the `bulk` endpoint).

##### Input

```json
{
  "path": "/media/tv",
  "is_bulk": true
}
```

* `path` — string, required. Directory to scan. The path is validated (non-empty, no traversal, within length limits), must exist and be a directory, and must not be the Unix filesystem root `/`; a violation returns `400` with `{"error": "..."}`.
* `is_bulk` — boolean, required. When `true`, only the immediate child directories of `path` are enumerated (e.g. a root full of series folders such as a torrent watch-dir); when `false`, `path`'s own folder is evaluated directly.

##### Output

```json
[
  {
    "path": "/media/tv/Breaking Bad",
    "original_folder_name": "Breaking Bad",
    "final_title": "Breaking Bad",
    "season_count": 5,
    "episode_count": 62,
    "selected": true,
    "already_exists": false,
    "all_files_in_root": false
  }
]
```

* `path` — string. Absolute path to the detected series folder.
* `original_folder_name` — string. Folder name as found on disk.
* `final_title` — string. Proposed series title (defaults to `original_folder_name`, editable before confirming the import).
* `season_count` — integer. Number of distinct seasons found; only seasons actually declared in filenames are counted.
* `episode_count` — integer. Number of video files detected (directory traversal is capped at 5 levels deep).
* `selected` — boolean. Whether the item is pre-selected for import; `false` when `already_exists` is `true`.
* `already_exists` — boolean. `true` when an existing series mapping already claims this path (compared after canonicalization, so symlink-equivalent paths are detected).
* `all_files_in_root` — boolean. `true` when all detected video files reside directly in the series root with no season sub-directories; the import flow auto-enables `flatten_season_folders` for such items.

* `500` (`{"error": "Internal Server Error"}`) if the directory scan itself fails.

#### `POST /api/system/organized_series/batch_move_preview`

**Scope:** `files:write`

Validate a batch move without making any changes. Returns a (source → destination) mapping for every path so the frontend can show a confirmation table. Destination collisions are reported per item in `collision` rather than as an HTTP error.

##### Input

```json
{
  "paths": [
    "/media/tv/Breaking Bad"
  ],
  "target_root": "/media/archive",
  "custom_paths": {},
  "file_operation": "Move",
  "stop_tracking": false
}
```

* `paths` — array of strings, required. Source paths of the series to move (as displayed in `OrganizedSeriesItem.absolute_path`).
* `target_root` — string, required. Destination root directory. The series folder name is appended to this (`target_root=/media/tv` + series "My Show" → `/media/tv/My Show`). Must exist on disk when no custom path applies to an item, otherwise `400`.
* `custom_paths` — object of source-path → destination-path, optional, default `{}`. Per-series overrides that ignore both the root and the folder-name append. Explicit overrides are honored verbatim (not collision-resolved).
* `file_operation` — enum, optional, default `Move`. How to handle existing files at the destination: `Move`, `Copy`, `Delete`, or `DoNothing`.
* `stop_tracking` — boolean, optional, default `false`. After the file operation, delete all DB data for the series (`delete_series_data` + `delete_series_mapping` + `cleanup_orphaned_metadata`).

##### Output

```json
{
  "items": [
    {
      "source_path": "/media/tv/Breaking Bad",
      "destination_path": "/media/archive/Breaking Bad",
      "folder_name": "Breaking Bad",
      "series_id": "3f2b1c4e-8a7d-4c5b-9e10-2f6a1b7c8d9e",
      "is_tracked": true,
      "collision": null
    }
  ],
  "has_collisions": false
}
```

* `items` — array of `BatchMovePreviewItem`, one per input path.
* `items[].source_path` — string. The input source path.
* `items[].destination_path` — string. Resolved destination (after sanitization and any collision resolution).
* `items[].folder_name` — string. Folder name component of the source path.
* `items[].series_id` — string or `null`. The series mapping that claims the source path, if any.
* `items[].is_tracked` — boolean. Whether a series mapping claims the source path.
* `items[].collision` — string or `null`. Set when the destination is already claimed by a different series, is claimed twice within the batch, or is an untracked folder that could not be resolved; `null` otherwise.
* `has_collisions` — boolean. `true` when any item has a collision.

#### `POST /api/system/organized_series/batch_move`

**Scope:** `files:write`

Execute a batch move. The server always appends the series folder name to `target_root`, so API callers cannot accidentally dump every series into a single flat directory; use `custom_paths` for per-series overrides. The move runs asynchronously: the endpoint returns immediately with a `task_id` (empty `results`), and progress is polled via the status endpoint below. Intra-batch collisions (two series resolving to the same destination) and empty custom paths are rejected up front with `400` before any work starts; per-path failures during execution are collected on the task.

##### Input

```json
{
  "paths": [
    "/media/tv/Breaking Bad"
  ],
  "target_root": "/media/archive",
  "custom_paths": {
    "/media/tv/Breaking Bad": "/media/other/Breaking Bad"
  },
  "file_operation": "Move",
  "stop_tracking": false
}
```

* `paths` — array of strings, required. Source paths of the series to move.
* `target_root` — string, required. Destination root; the series folder name is appended unless a custom path is given.
* `custom_paths` — object of source-path → destination-path, optional, default `{}`. Per-series overrides; an empty value for a key is rejected with `400`, and two identical custom destinations are rejected with `400`.
* `file_operation` — enum, optional, default `Move`. `Move` renames the directory contents to the new location and deletes the source; `Copy` copies and keeps the source; `Delete` updates the DB path and deletes the source directory; `DoNothing` only updates the DB path.
* `stop_tracking` — boolean, optional, default `false`. After the file operation, remove the series from tracking (same DB sequence as `batch_edit` `delete`). Auto-scan is skipped when this is set.

##### Output

```json
{
  "results": [],
  "success_count": 0,
  "failure_count": 0,
  "task_id": "b1c2d3e4-5f60-4a7b-8c9d-0e1f2a3b4c5d"
}
```

* `results` — array of `BatchMoveResult`. Empty for the asynchronous response; per-path results are not returned inline (`BatchMoveResult` has `source_path`, `destination_path`, `success`, `error`).
* `success_count` — integer. `0` for the asynchronous response.
* `failure_count` — integer. `0` for the asynchronous response.
* `task_id` — string, optional. Identifier to poll via `GET /api/system/organized_series/batch_move/{task_id}/status`.

* `400` for intra-batch duplicate destinations or an empty custom path, with `{"error": "..."}`.

#### `GET /api/system/organized_series/batch_move/{task_id}/status`

**Scope:** `files:write`

Check the progress of a batch-move task started by `POST /api/system/organized_series/batch_move`.

##### Input

* `task_id` — path parameter, string, required. The `task_id` returned when the batch move started; must be non-empty (`400` otherwise).

##### Output

```json
{
  "operation_type": "batch_move",
  "total": 5,
  "completed": 5,
  "success_count": 4,
  "failed": 1,
  "finished": true,
  "errors": [
    "Destination '/media/archive/Show' is already claimed by another series"
  ]
}
```

* `operation_type` — enum. `batch_move` (or `reorganize`) in snake_case.
* `total` — integer. Total number of paths in the batch.
* `completed` — integer. Paths processed so far; equals `total` once finished.
* `success_count` — integer. Paths that completed successfully.
* `failed` — integer. Paths that failed.
* `finished` — boolean. Whether the task has completed (or failed).
* `errors` — array of strings. Up to 20 error messages collected during execution.

* `400` for an empty task ID; `404` (`{"error": "Batch move task not found"}`) when the task ID is unknown.

#### `GET /api/system/active_operations`

**Scope:** `files:write`

Active batch moves and currently-locked series. Exposes in-progress operations so other users/sessions can see what is happening and why some series are locked.

##### Input

None.

##### Output

```json
{
  "batch_moves": [
    {
      "id": "b1c2d3e4-5f60-4a7b-8c9d-0e1f2a3b4c5d",
      "operation_type": "batch_move",
      "total": 5,
      "completed": 3,
      "finished": false,
      "finished_at_ms": null,
      "success_count": 3,
      "failed": 0,
      "errors": []
    }
  ],
  "locked_series": [
    "3f2b1c4e-8a7d-4c5b-9e10-2f6a1b7c8d9e"
  ]
}
```

* `batch_moves` — array of `ActiveOperation`. In-progress or recently finished background operations (finished tasks are retained for up to 30 minutes).
* `batch_moves[].id` — string. Task identifier.
* `batch_moves[].operation_type` — string. `batch_move` or `reorganize`.
* `batch_moves[].total` — integer. Total items in the operation.
* `batch_moves[].completed` — integer. Items processed so far.
* `batch_moves[].finished` — boolean. Whether the operation has completed.
* `batch_moves[].finished_at_ms` — integer or `null`. Epoch milliseconds when finished; `null` while running.
* `batch_moves[].success_count` — integer. Items that completed successfully.
* `batch_moves[].failed` — integer. Items that failed.
* `batch_moves[].errors` — array of strings. Error messages collected during execution.
* `locked_series` — array of strings. Series IDs currently locked by an in-progress modification (batch move, reorganize, path update, delete, etc.).

### Rename Read

#### `GET /api/system/rename_queue`

**Scope:** `rename:read`

Rename queue overview. Returns one item per series with pending renames, including the number of affected episodes, collision count, and human-readable causes (e.g. `"Global/Series Episode File Format"`, `"Global/Series Season Folder Format"`). Hidden series are excluded; a series whose plan changed since it last failed is re-evaluated, and a series with no pending renames has its stale failure/processing flags cleared.

##### Input

None.

##### Output

```json
{
  "items": [
    {
      "series_id": "3f2b1c4e-8a7d-4c5b-9e10-2f6a1b7c8d9e",
      "series_title": "Breaking Bad",
      "affected_episodes": 62,
      "collision_count": 2,
      "causes": [
        "Global/Series Episode File Format"
      ],
      "absolute_numbering": false,
      "has_failed": false,
      "processing": false
    }
  ],
  "total_affected_episodes": 62,
  "has_failed_renames": false
}
```

* `items` — array of `RenameQueueItem`. One entry per series with pending renames.
* `items[].series_id` — string. Series mapping ID.
* `items[].series_title` — string. Series display title.
* `items[].affected_episodes` — integer. Number of planned renames for the series.
* `items[].collision_count` — integer. Destinations in the plan that collide with other files.
* `items[].causes` — array of strings. Sorted, human-readable reasons the plan has pending renames.
* `items[].absolute_numbering` — boolean. Whether the series uses absolute episode numbering.
* `items[].has_failed` — boolean, default `false`. Whether a previous apply failed and the plan is unchanged since.
* `items[].processing` — boolean, default `false`. Whether the series is currently being reorganized.
* `total_affected_episodes` — integer. Sum of `affected_episodes` across all items.
* `has_failed_renames` — boolean, default `false`. `true` when any item has `has_failed`.

#### `GET /api/system/rename_queue/{series_id}`

**Scope:** `rename:read`

Rename queue detail for one series: every planned rename (original → expected path), whether each has a collision, the collision mode, and the destination folders that will be created and the source folders that will become empty. Collision diagnosis is shared with execution, so the preview and the actual rename never drift.

##### Input

* `series_id` — path parameter, string, required. Series mapping ID; must be a valid ID (`400` otherwise).

##### Output

```json
{
  "series_id": "3f2b1c4e-8a7d-4c5b-9e10-2f6a1b7c8d9e",
  "series_title": "Breaking Bad",
  "renames": [
    {
      "original": "/media/tv/Breaking Bad/S1/Breaking.Bad.S01E01.mkv",
      "expected": "/media/tv/Breaking Bad/Season 01/Breaking Bad - S01E01 - Pilot.mkv",
      "has_collision": false,
      "part_number": null,
      "aux_kind": null
    }
  ],
  "collision_mode": "rename",
  "folder_creations": [
    "/media/tv/Breaking Bad/Season 01"
  ],
  "folder_deletions": [
    "/media/tv/Breaking Bad/S1"
  ]
}
```

* `series_id` — string. Series mapping ID.
* `series_title` — string. Series display title.
* `renames` — array of `RenameDetail`. One entry per planned move.
* `renames[].original` — string. Current file path (source).
* `renames[].expected` — string. Target file path (destination), adjusted per the collision mode when a collision is resolved.
* `renames[].has_collision` — boolean. Whether the destination is claimed by another assigned file.
* `renames[].part_number` — integer or `null`, default `null`. Part number when this entry is a part file (e.g. `pt1`, `cd2`); `null` for normal single/multi-episode files.
* `renames[].aux_kind` — string or `null`, default `null`. Sidecar kind (`subtitle` or `nfo`) for auxiliary files; `null` for a playable video file.
* `collision_mode` — string. The configured collision handling (`rename`, `skip`, or `overwrite`).
* `folder_creations` — array of strings. Destination folders that do not yet exist and will be created.
* `folder_deletions` — array of strings. Source folders that will become empty (all their video files are being moved out) and will be removed.

* `400` for an invalid series ID; `404` when the series mapping does not exist; `500` when the rename plan has a duplicate target destination.

### Rename Write

#### `POST /api/system/remediate`

**Scope:** `rename:write`

Run a remediation action. Currently only `cleanup_logs` is supported: it manually frees disk by culling old rotated log archives down to the configured maximum archive count. The current log file is never deleted, and unrelated files in the logs directory are ignored.

##### Input

```json
{
  "action": "cleanup_logs"
}
```

* `action` — string, required. Remediation action to run. Only `cleanup_logs` is handled; any other value returns `400` with `{"error": "Unknown remediation action"}`.

##### Output

No content (`200`).

---

## 8. Settings Endpoints

### Config Read

#### `GET /api/config`

**Scope:** `config:read`

##### Input

None.

##### Output

```json
{
  "database": "/var/lib/jumbie/jumbie.db",
  "unknown_files_tmp_dir": "/var/lib/jumbie/unknown",
  "plugins_dir": "/var/lib/jumbie/plugins",
  "logs_dir": "/var/lib/jumbie/logs",
  "organization": {
    "destination_roots": [
      { "path": "/media/tv", "include_subdirs_in_managed": true }
    ],
    "collision_handling": "rename",
    "season_folder_format": "S${season:auto2}",
    "episode_file_format": "${series} - S${season:auto2}E${episode:auto2}?{ - ${title}}",
    "season_folder_format_absolute": "S${season:auto2}",
    "episode_file_format_absolute": "${series} - S${season:auto2}E${episode:auto2}?{ - ${title}}",
    "search_format": "S${season:02}E${episode:02}",
    "search_format_absolute": "E${episode:02}",
    "rename_episodes": true,
    "auto_apply_renames": false,
    "illegal_char_policy": "underscore",
    "allow_platform_specific_chars": false,
    "collision_rename_suffix": "dot_numeric",
    "part_number_format": "pt"
  },
  "sources": {},
  "general": {
    "media_info_scan_enabled": true,
    "media_info_scan_interval": 15,
    "default_score_for_manual_files": null,
    "automatic_profiles": { "enabled": false, "categories": {} },
    "season_pack_strategy": "favorepisodes",
    "season_pack_replace_threshold": 50,
    "season_pack_score_modifier": null,
    "unexpected_files_handling": "delete",
    "unneeded_episodes_handling": "delete",
    "media_info_scan_concurrency": 1,
    "series_scan_enabled": true,
    "series_scan_interval": 10,
    "flatten_season_folders": false,
    "absolute_numbering": false,
    "metadata_fetch_cooldown_minutes": 60,
    "auto_search_wanted_enabled": false,
    "auto_search_wanted_interval": 60,
    "auto_search_wanted_min_wait": 120,
    "auto_search_wanted_max_age_days": 2
  },
  "proxy": { "enabled": false, "http": "", "https": "" },
  "auth": {
    "password": "********",
    "api_keys": [
      {
        "id": "550e8400-e29b-41d4-a716-446655440000",
        "name": "Automation",
        "key": "********",
        "prefix": "jb_aBcDeFgH",
        "scopes": ["config:read", "series:read"],
        "expires_at": null
      }
    ],
    "calendar_token": null,
    "calendar_tokens": [],
    "bypass_local_auth": false,
    "bypass_subnet_whitelist": false,
    "subnet_whitelist": [],
    "max_auth_fail_count": 0,
    "ban_duration_seconds": 300,
    "ban_increment_enabled": false,
    "ban_increment_factor": 2.0,
    "ban_increment_max_seconds": 31536000,
    "ban_count_reset_days": 30,
    "banned_ips": []
  },
  "security": {
    "clickjacking_protection": false,
    "csrf_protection": false,
    "host_header_validation": false,
    "allowed_domains": [],
    "use_custom_headers": false,
    "custom_headers": [],
    "trusted_proxies": [],
    "restrict_cors": false,
    "allowed_origins": [],
    "rate_limit_enabled": false,
    "rate_limit_per_minute": 60,
    "rate_limit_burst": 10,
    "require_plugin_signatures": false
  }
}
```

* `database` — path to the SQLite database file.
* `unknown_files_tmp_dir` — directory for quarantined/unorganized files.
* `plugins_dir` — directory scanned for external plugins.
* `logs_dir` — directory for log files.
* `organization` — library layout and naming rules: `destination_roots` (library roots, each `{ "path", "include_subdirs_in_managed" }`), `collision_handling`, the four format-template strings (`season_folder_format`, `episode_file_format`, `season_folder_format_absolute`, `episode_file_format_absolute`), the two search-template strings (`search_format`, `search_format_absolute`), `rename_episodes`, `auto_apply_renames`, `illegal_char_policy`, `allow_platform_specific_chars`, `collision_rename_suffix`, and `part_number_format`. In the format templates `${season}`/`${episode}` are emitted unpadded (`${episode}` → `5`), `:0N` zero-pads to a minimum width of N (`${episode:02}` → `05`), and `:auto` zero-pads to exactly the digits the highest value needs, with an optional minimum (`:autoN`) — `${episode:auto}` sizes to the episode's own season (`005` when that season reaches 105, `5` when it tops out at 9), `${episode:auto2}` is the same but never narrower than two digits, and `${season:auto}` sizes to the series' highest season (`03` for a 12-season series).
* `sources` — placeholder object; intentionally empty (source sync timing is per-plugin).
* `general` — scan and automation settings: media-info scan toggle/interval/concurrency (`media_info_scan_concurrency: 0` means no concurrency cap), `default_score_for_manual_files`, `automatic_profiles` (nested `enabled` + `categories`), `season_pack_strategy`, `season_pack_replace_threshold`, `season_pack_score_modifier` (`default_score_for_manual_files` and `season_pack_score_modifier` accept `null`, treated as `0`), `unexpected_files_handling`, `unneeded_episodes_handling`, series-scan toggle/interval, `flatten_season_folders`, `absolute_numbering`, `metadata_fetch_cooldown_minutes`, and the `auto_search_wanted_*` family.
* `proxy` — `enabled`, `http`, `https`.
* `auth` — authentication and ban settings. In every config response secret values are masked: `password` is the sentinel `"********"` when a password is set and `null` when auth is disabled; each `api_keys[].key` is replaced with `"********"` while `id`, `name`, `prefix`, `scopes`, and `expires_at` are preserved. `api_keys`, `calendar_tokens`, `banned_ips`, and `subnet_whitelist` are injected from the database (the DB is the source of truth). Remaining fields: `calendar_token`, `bypass_local_auth`, `bypass_subnet_whitelist`, `max_auth_fail_count`, `ban_duration_seconds`, `ban_increment_enabled`, `ban_increment_factor`, `ban_increment_max_seconds`, `ban_count_reset_days`.
* `security` — response headers, CORS, rate limiting, and plugin-signature settings: `clickjacking_protection`, `csrf_protection`, `host_header_validation`, `allowed_domains`, `use_custom_headers`, `custom_headers`, `trusted_proxies`, `restrict_cors`, `allowed_origins`, `rate_limit_enabled`, `rate_limit_per_minute`, `rate_limit_burst`, `require_plugin_signatures`.

The four path fields (`database`, `unknown_files_tmp_dir`, `plugins_dir`, `logs_dir`) are startup-only values from `config.toml`; the remaining sections are DB-managed. All sections are always present (missing values fall back to Rust defaults).

#### `GET /api/bootstrap`

**Scope:** `config:read`

##### Input

None.

##### Output

```json
{
  "config": { "...": "Config, same shape as GET /api/config" },
  "qualities": { "...": "same shape as GET /api/config/qualities" },
  "quality_profiles": { "...": "same shape as GET /api/config/quality_profiles" },
  "release_profiles": { "...": "same shape as GET /api/config/release_profiles" },
  "ui_preferences": { "...": "same shape as GET /api/config/ui_preferences" },
  "plugins_cfg": {
    "enabled": true,
    "directory": "/var/lib/jumbie/plugins",
    "downloader": {},
    "notifier": {},
    "source": {},
    "metadata": {}
  },
  "plugins": [],
  "available_plugins": [],
  "plugin_status": []
}
```

* `config` — full sanitized configuration, identical to `GET /api/config`.
* `qualities` — quality definitions, identical to `GET /api/config/qualities`.
* `quality_profiles` — quality profiles, identical to `GET /api/config/quality_profiles`.
* `release_profiles` — release profiles, identical to `GET /api/config/release_profiles`.
* `ui_preferences` — UI preferences, identical to `GET /api/config/ui_preferences`.
* `plugins_cfg` — plugin registry: `enabled`, `directory`, and the four category maps `downloader`, `notifier`, `source`, `metadata` (each `plugin_name` → `instance_id` → raw config).
* `plugins` — loaded plugin instances (type metadata plus backend-owned identity).
* `available_plugins` — available plugin types (immutable type metadata plus `plugin_id`).
* `plugin_status` — per-plugin health entries (`name`, `category`, `ok`, optional `message`).

Aggregates every read-only settings datum the frontend needs into a single round-trip.

#### `GET /api/config/qualities`

**Scope:** `config:read`

##### Input

None.

##### Output

```json
{
  "quality-1080p-uuid-00000000": {
    "name": "1080p",
    "tags": ["1080p", "1920x1080", "1080", "fhd"]
  },
  "quality-720p-uuid-000000000": {
    "name": "720p",
    "tags": ["720p", "1280x720", "720", "hd"]
  }
}
```

* object keys — stable quality ids (UUID-like so a quality keeps its id across renames).
* `name` — display name.
* `tags` — release-title substrings that match this quality.

#### `GET /api/config/quality_profiles`

**Scope:** `config:read`

##### Input

None.

##### Output

```json
{
  "high-def-profile-uuid-000000": {
    "name": "High Definition",
    "qualities": [
      "quality-4k-uuid-00000000000",
      "quality-1080p-uuid-00000000",
      "quality-720p-uuid-000000000"
    ],
    "upgrade_only_qualities": []
  }
}
```

* object keys — quality profile ids.
* `name` — profile display name.
* `qualities` — ordered quality ids accepted by this profile.
* `upgrade_only_qualities` — subset of `qualities` that may only replace an existing download (never satisfy a first grab).

#### `GET /api/config/release_profiles`

**Scope:** `config:read`

##### Input

None.

##### Output

```json
{
  "default-weights-uuid-0000000": {
    "min_score": 10,
    "name": "Default Weights",
    "terms": { "1080p": 10, "720p": 5, "BluRay": 15, "WebDL": 12 },
    "regex_terms": {},
    "submitters": {},
    "case_insensitive_terms": false,
    "case_insensitive_submitters": false,
    "size_score_per_gb": 0,
    "peers_score_per_peer": 0,
    "age_score_per_day": 0
  }
}
```

* object keys — release profile ids.
* `min_score` — minimum score a release must reach.
* `name` — profile display name.
* `terms` — substring → points map.
* `regex_terms` — regex pattern → points map.
* `submitters` — submitter → points map.
* `case_insensitive_terms` / `case_insensitive_submitters` — case-handling flags.
* `size_score_per_gb` — points per GB (normalized per episode for multi-episode releases).
* `peers_score_per_peer` — points per seeder.
* `age_score_per_day` — points per day since publication.

#### `GET /api/config/ui_preferences`

**Scope:** `config:read`

##### Input

None.

##### Output

```json
{
  "logs": { "min_level": "INFO" },
  "theme": "auto",
  "release_date_display": {
    "order": ["metadata", "source", "estimated"],
    "metadata_enabled": true,
    "source_enabled": true,
    "estimated_enabled": true
  },
  "table_sorts": {
    "series_library": { "column": "title", "ascending": true }
  },
  "view_modes": {},
  "activity": { "filter_types": [] },
  "time_format": "hour12"
}
```

* `logs.min_level` — minimum log level shown (`"INFO"` by default).
* `theme` — one of `"auto"`, `"light"`, `"dark"`.
* `release_date_display` — which release-date sources are shown and their priority: `order` (subset of `metadata`, `source`, `estimated`), `metadata_enabled`, `source_enabled`, `estimated_enabled`.
* `table_sorts` — table name → `{ "column", "ascending" }` persisted sort preferences.
* `view_modes` — page name → view-mode string.
* `activity.filter_types` — activity type names to filter by; empty means all.
* `time_format` — `"hour12"` or `"hour24"`.

#### `GET /api/automatic-profiles`

**Scope:** `config:read`

##### Input

None.

##### Output

```json
[
  {
    "submitter": "SomeGroup",
    "score": 3,
    "media_scan_count": 5
  }
]
```

* `submitter` — release-group / uploader name.
* `score` — accumulated offense score (higher is worse).
* `media_scan_count` — number of stored media scans recorded for this submitter.

Ordered by `score` ascending.

#### `GET /api/automatic-profiles/{submitter}/records`

**Scope:** `config:read`

##### Input

* `submitter` — path parameter, required. Release-group / uploader name.

##### Output

```json
[
  {
    "id": "1c2d3e4f-5a6b-7c8d-9e0f-1234567890ab",
    "submitter": "SomeGroup",
    "category": "Low Resolution",
    "score": 2,
    "description": "Resolution below minimum (1920x1080)",
    "date_added": "2026-06-18T20:00:00+00:00",
    "extension": null
  }
]
```

* `id` — record UUID.
* `submitter` — release-group / uploader name.
* `category` — the automatic-profile category that was violated.
* `score` — points applied for this offense (also accepted as `penalty` on input).
* `description` — human-readable reason.
* `date_added` — RFC 3339 UTC timestamp; the DB's naive-UTC value is normalized at the API boundary.
* `extension` — offending file extension for `unexpected files` offenses; omitted when not applicable.

Sorted by `date_added` descending.

### Config Write

#### `PUT /api/config`

**Scope:** `config:write`

##### Input

```json
{
  "general": {
    "absolute_numbering": true,
    "season_pack_replace_threshold": 50
  },
  "auth": {
    "password": "********",
    "api_keys": [
      {
        "id": "550e8400-e29b-41d4-a716-446655440000",
        "name": "Automation",
        "key": "********",
        "prefix": "jb_aBcDeFgH",
        "scopes": ["config:read"],
        "expires_at": "2026-12-31T23:59:59Z"
      }
    ],
    "banned_ips": [
      {
        "ip": "203.0.113.7",
        "fail_count": 5,
        "ban_count": 1,
        "banned_at": "2026-06-18T20:00:00+00:00",
        "banned_until": "2026-06-18T20:05:00+00:00"
      }
    ]
  }
}
```

* `organization` — optional `OrganizationConfig`. When provided, each `destination_roots` entry is validated and duplicate canonical roots are rejected with `400`.
* `sources` — optional `SourcesConfig`; intentionally empty.
* `general` — optional `GeneralConfig`; validated (automatic profiles, scan intervals, season-pack threshold, unexpected/unneeded handling, and the `auto_search_wanted_*` bounds).
* `proxy` — optional `ProxyConfig`; when `enabled`, a non-empty `http`/`https` must be a valid URL (`400` otherwise).
* `auth` — optional `AuthConfig` (see nested fields below).
* `security` — optional `SecurityConfig`; replaces the section verbatim.
* `auth.password` — optional. The sentinel `"********"` leaves the stored hash unchanged; a new non-empty value is hashed and stored; an empty value, or omitting `password` while sending `auth`, clears the password.
* `auth.api_keys` — optional. The full list: ids absent from it are deleted. A `key` other than `"********"` stores a freshly hashed key, while `"********"` keeps the existing hash and updates `name`, `scopes`, and `expires_at`.
* `auth.api_keys[].id` — required per key; stable key id used to match existing keys.
* `auth.api_keys[].name` — required per key; display name.
* `auth.api_keys[].prefix` — per key; the stored display prefix (ignored when `key` is a new value, which derives its own prefix).
* `auth.api_keys[].scopes` — required per key; must be non-empty.
* `auth.api_keys[].expires_at` — optional; must be RFC 3339 with an explicit offset (strict — see [§13](#timestamps)) or empty/`null` for no expiry.
* `auth.calendar_tokens` — optional. Full list: absent ids are deleted, provided entries are upserted.
* `auth.banned_ips` — optional. Replaces the entire ban list.
* `auth.banned_ips[].ip` — required per entry; the banned client IP.
* `auth.banned_ips[].fail_count` / `ban_count` — per entry; consecutive auth failures and total offenses recorded.
* `auth.banned_ips[].banned_at` / `banned_until` — must be RFC 3339 with an explicit offset when non-empty (strict); `banned_until` is `null` for a permanent ban.
* `auth.subnet_whitelist` — optional; replaces the stored whitelist.
* `auth.bypass_local_auth`, `auth.bypass_subnet_whitelist`, `auth.max_auth_fail_count`, `auth.ban_duration_seconds`, `auth.ban_increment_enabled`, `auth.ban_increment_factor`, `auth.ban_increment_max_seconds`, `auth.ban_count_reset_days` — optional ban/bypass settings.

**Partial update:** only the sections present in the payload are validated and saved. The four `config.toml` startup paths (`database`, `unknown_files_tmp_dir`, `plugins_dir`, `logs_dir`) are not part of this payload and can only be changed in the file before startup.

Request timestamps are strict: `auth.api_keys[].expires_at` and `auth.banned_ips[].banned_at` / `banned_until` must be RFC 3339 with an explicit `Z` or `±HH:MM` offset. Zone-less or date-only values are rejected with `400`.

##### Output

No content (`200 OK`).

#### `PUT /api/config/qualities`

**Scope:** `config:write`

##### Input

```json
{
  "quality-1080p-uuid-00000000": {
    "name": "1080p",
    "tags": ["1080p", "1920x1080", "1080", "fhd"]
  }
}
```

* object keys — quality ids (renaming a key changes its identity).
* `name` — required, non-empty, at most 100 characters, no control characters.
* `tags` — required array; each tag non-empty, at most 50 characters, no control characters.

Replaces all quality definitions with the provided map.

##### Output

No content (`200 OK`).

#### `PUT /api/config/quality_profiles`

**Scope:** `config:write`

##### Input

```json
{
  "high-def-profile-uuid-000000": {
    "name": "High Definition",
    "qualities": ["quality-1080p-uuid-00000000", "quality-720p-uuid-000000000"],
    "upgrade_only_qualities": ["quality-1080p-uuid-00000000"]
  }
}
```

* object keys — quality profile ids.
* `name` — required, non-empty, at most 100 characters, no control characters.
* `qualities` — required, must contain at least one quality id (`400` otherwise).
* `upgrade_only_qualities` — must be a subset of `qualities` (`400` otherwise).

Replaces all quality profiles with the provided map.

##### Output

No content (`200 OK`).

#### `PUT /api/config/release_profiles`

**Scope:** `config:write`

##### Input

```json
{
  "default-weights-uuid-0000000": {
    "min_score": 10,
    "name": "Default Weights",
    "terms": { "1080p": 10, "720p": 5, "BluRay": 15, "WebDL": 12 },
    "regex_terms": {},
    "submitters": {},
    "case_insensitive_terms": false,
    "case_insensitive_submitters": false,
    "size_score_per_gb": 0,
    "peers_score_per_peer": 0,
    "age_score_per_day": 0
  }
}
```

* object keys — release profile ids.
* `name` — required, non-empty, at most 100 characters, no control characters.
* `min_score` — minimum acceptable score (default `0`).
* `terms`, `regex_terms`, `submitters` — point maps; every `regex_terms` key must compile as a valid regex (`400` otherwise).
* `case_insensitive_terms`, `case_insensitive_submitters` — case-handling flags.
* `size_score_per_gb`, `peers_score_per_peer`, `age_score_per_day` — metadata-based score weights.

Replaces all release profiles with the provided map, then rescoring runs retroactively for every series referencing a saved profile (idempotent).

##### Output

No content (`200 OK`).

#### `PUT /api/config/ui_preferences`

**Scope:** `config:write`

##### Input

```json
{
  "logs": { "min_level": "INFO" },
  "theme": "dark",
  "release_date_display": {
    "order": ["metadata", "source", "estimated"],
    "metadata_enabled": true,
    "source_enabled": true,
    "estimated_enabled": true
  },
  "table_sorts": {
    "series_library": { "column": "title", "ascending": true }
  },
  "view_modes": {},
  "activity": { "filter_types": [] },
  "time_format": "hour24"
}
```

* `logs` — `{ min_level }`, minimum log level.
* `theme` — `"auto"`, `"light"`, or `"dark"`.
* `release_date_display` — `order` plus the three `*_enabled` toggles.
* `table_sorts` — table name → `{ column, ascending }`.
* `view_modes` — page name → view-mode string.
* `activity` — `{ filter_types }`; empty means all types.
* `time_format` — `"hour12"` or `"hour24"`.

The body is the full `UIConfig`; the whole object is saved, replacing the stored preferences.

##### Output

No content (`200 OK`).

#### `DELETE /api/automatic-profiles/{submitter}`

**Scope:** `config:write`

##### Input

* `submitter` — path parameter, required. Must be non-empty and at most 255 characters (`400` otherwise).

##### Output

No content (`200 OK`).

#### `POST /api/automatic-profiles/recalculate`

**Scope:** `config:write`

##### Input

None.

##### Output

No content (`200 OK`).

Cancels any pending debounced recalculation and immediately reapplies automatic profiles across the library. Returns `500` when the organizer is unavailable.

### Plugins Read

**Scope:** `plugins:read`

#### `GET /api/config/plugins_cfg`

**Scope:** `plugins:read`

##### Input

None.

##### Output

```json
{
  "enabled": true,
  "directory": "/config/plugins",
  "downloader": {
    "qbittorrent": {
      "a1b2c3d4e5f6": {
        "name": "qBittorrent",
        "enabled": true,
        "link": "http://localhost:8080",
        "username": "admin",
        "password": "",
        "download_path": "./downloads",
        "use_separate_paths": false,
        "default_category": "Series",
        "verify_ssl": false,
        "priority": 0
      }
    }
  },
  "notifier": {
    "discord": {
      "f6e5d4c3b2a1": {
        "name": "Discord",
        "enabled": true,
        "webhook_url": "https://discord.com/api/webhooks/000/xxx",
        "events": {
          "Download Started": true,
          "Download Completed": true,
          "Rename Queue": true,
          "Error": true
        }
      }
    }
  },
  "source": {
    "nyaa": {
      "112233445566": {
        "name": "Nyaa",
        "enabled": true,
        "base_url": "https://nyaa.si",
        "refresh_interval": 30
      }
    }
  },
  "metadata": {
    "tvdb": {
      "998877665544": {
        "name": "TVDB",
        "enabled": true,
        "api_key": "xxxxxxxx",
        "refresh_interval": 12
      }
    }
  }
}
```

* `enabled` — boolean. Whether the plugin subsystem is enabled; missing values deserialize to `false`.
* `directory` — string (path). Directory scanned for external plugins; missing values deserialize to an empty path.
* `downloader` — object. Map of `plugin_id` → `instance_id` → raw instance config object; instance config shapes are plugin-defined (`serde_json::Value`). Missing values deserialize to `{}`.
* `notifier` — object. Same nested-map shape as `downloader`, for notifier plugins.
* `source` — object. Same nested-map shape as `downloader`, for source plugins.
* `metadata` — object. Same nested-map shape as `downloader`, for metadata plugins.
* The read is a DB lookup; a storage failure yields `500` `{"error": "Internal Server Error"}`.

#### `GET /api/plugins`

**Scope:** `plugins:read`

##### Input

None.

##### Output

```json
[
  {
    "display_name": "qBittorrent",
    "version": "1.0.0",
    "author": "Jumbie",
    "description": "qBittorrent download client",
    "capabilities": ["downloader", "can_pause_resume", "can_seed"],
    "supported_protocols": ["torrent"],
    "series_identifier_label": null,
    "series_identifier_placeholder": null,
    "rate_limit": { "requests_per_minute": 300, "burst": 50 },
    "supports_test": true,
    "plugin_id": "jumbie.qbittorrent",
    "instance_id": "a1b2c3d4e5f6"
  }
]
```

* One entry per loaded instance (downloaders, notifiers, sources, metadata, and external hosts), sorted by `priority` descending then `instance_id` ascending.
* `display_name` — string. User-configured instance name when set, otherwise the plugin type's default display name.
* `version` — string. Plugin type version.
* `author` — string. Plugin type author.
* `description` — string. Plugin type description.
* `capabilities` — array of strings. One or more of `feed_provider`, `downloader`, `notifier`, `metadata_provider`, `metadata_provider_normal`, `metadata_provider_absolute`, `polling`, `manual_search`, `automatic_search`, `can_pause_resume`, `fetch_series_title`, `fetch_series_aliases`, `can_seed`.
* `supported_protocols` — array of strings or absent. Protocols the plugin handles (e.g. `torrent`); omitted when the type declares none.
* `series_identifier_label` — string or absent. Label for the series ID input field; omitted when unset.
* `series_identifier_placeholder` — string or absent. Placeholder for the series ID input field; omitted when unset.
* `rate_limit` — object or absent. `{ "requests_per_minute": <u32>, "burst": <u32> }`; omitted when the type declares no limit.
* `supports_test` — boolean. Whether the plugin implements an optional `test` method (drives the UI "Test" button).
* `plugin_id` — string or absent. Backend-derived type id (e.g. `jumbie.qbittorrent`); omitted only when the instance has no resolved type id.
* `instance_id` — string or absent. Backend-owned instance key (the config instance id); omitted when unresolved.

#### `GET /api/plugins/available`

**Scope:** `plugins:read`

##### Input

None.

##### Output

```json
[
  {
    "display_name": "qBittorrent",
    "version": "1.0.0",
    "author": "Jumbie",
    "description": "qBittorrent download client",
    "capabilities": ["downloader", "can_pause_resume", "can_seed"],
    "supported_protocols": ["torrent"],
    "series_identifier_label": null,
    "series_identifier_placeholder": null,
    "rate_limit": { "requests_per_minute": 300, "burst": 50 },
    "supports_test": true,
    "plugin_id": "jumbie.qbittorrent"
  }
]
```

* One entry per registered plugin TYPE (never per instance); there is no `instance_id` field.
* Sorted with built-in (`jumbie.*`) types first, then by display name case-insensitively with `plugin_id` as tie-break.
* `display_name` — string. Plugin type display name.
* `version` — string. Plugin type version.
* `author` — string. Plugin type author.
* `description` — string. Plugin type description.
* `capabilities` — array of strings. Same vocabulary as `GET /api/plugins`.
* `supported_protocols` — array of strings or absent. Omitted when the type declares none.
* `series_identifier_label` — string or absent. Omitted when unset.
* `series_identifier_placeholder` — string or absent. Omitted when unset.
* `rate_limit` — object or absent. `{ "requests_per_minute": <u32>, "burst": <u32> }`; omitted when unset.
* `supports_test` — boolean. Whether the type implements `test`.
* `plugin_id` — string or absent. Backend-derived type id (e.g. `jumbie.qbittorrent`).

#### `GET /api/plugins/schemas`

**Scope:** `plugins:read`

##### Input

None.

##### Output

```json
{
  "qbittorrent": {
    "$schema": "http://json-schema.org/draft-07/schema#",
    "title": "QBittorrentSettings",
    "type": "object",
    "required": ["link"],
    "properties": {
      "name": {
        "type": "string",
        "title": "Name",
        "description": "User-defined name for this plugin instance",
        "default": "qBittorrent",
        "order": -20
      },
      "enabled": {
        "type": "boolean",
        "title": "Enabled",
        "description": "Whether this plugin instance is active",
        "default": true,
        "order": -10
      },
      "link": {
        "type": "string",
        "title": "Link*",
        "description": "qBittorrent Web UI URL (e.g. http://localhost:8080)",
        "default": "http://localhost:8080",
        "order": 10
      },
      "username": { "type": "string", "title": "Username", "default": "admin", "order": 30 },
      "password": { "type": "string", "title": "Password", "order": 40 },
      "enable_seeding": {
        "type": "boolean",
        "title": "Enable Seeding",
        "description": "Keep files seeding after organize instead of removing torrent immediately",
        "default": true,
        "order": -2,
        "parent": "enabled"
      },
      "priority": {
        "type": "integer",
        "title": "Priority",
        "description": "Affects the order in which downloaders are used. Higher values are attempted first.",
        "default": 0,
        "order": 1000
      }
    }
  },
  "tvdb": {
    "$schema": "http://json-schema.org/draft-07/schema#",
    "title": "TVDBSettings",
    "type": "object",
    "properties": {
      "name": { "type": "string", "title": "Name", "default": "TVDB", "order": -20 },
      "enabled": { "type": "boolean", "title": "Enabled", "default": true, "order": -10 },
      "api_key": { "type": "string", "title": "API Key*", "description": "Your TVDB V4 API Key", "order": 1 },
      "refresh_interval": {
        "type": "integer",
        "format": "duration",
        "x-time-unit": "hours",
        "title": "Refresh Interval",
        "description": "How often to check for updated metadata (e.g. 12h, 1w)",
        "default": 12,
        "order": 1001,
        "minimum": 1
      }
    }
  }
}
```

* The response is an object keyed by short plugin type name; each value is that type's JSON Schema (draft-07).
* `$schema` / `title` / `type` / `required` / `properties` — standard JSON Schema fields supplied by the plugin; `required` lists mandatory field names and is absent when the plugin defines none.
* Every schema's `properties` has reserved fields injected by the backend (any plugin-supplied definitions of these are discarded, defaults preserved): `name` (`order` `-20`) and `enabled` (default `true`, `order` `-10`).
* `priority` — integer property (`order` `1000`) injected only for `downloader` types.
* `refresh_interval` — integer/duration property (`order` `1001`, with `x-time-unit`) injected only for `source` and `metadata` types; `metadata` also gets `minimum: 1` and metadata-specific description text.
* Capability toggles are injected only when the type declares the matching capability: `enable_manual_search`, `enable_automatic_search`, `enable_polling`, and `enable_seeding`. Each is a boolean with default `true`, a negative `order`, and `"parent": "enabled"`.

#### `GET /api/plugins/{name}/schema`

**Scope:** `plugins:read`

##### Input

* `name` — path parameter. Plugin type id; both short (`qbittorrent`) and full (`downloader.qbittorrent`) forms are accepted — it is normalized to the short, lowercased key before lookup.

##### Output

```json
{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "title": "QBittorrentSettings",
  "type": "object",
  "required": ["link"],
  "properties": {
    "name": { "type": "string", "title": "Name", "description": "User-defined name for this plugin instance", "default": "qBittorrent", "order": -20 },
    "enabled": { "type": "boolean", "title": "Enabled", "description": "Whether this plugin instance is active", "default": true, "order": -10 },
    "link": { "type": "string", "title": "Link*", "description": "qBittorrent Web UI URL (e.g. http://localhost:8080)", "default": "http://localhost:8080", "order": 10 },
    "priority": { "type": "integer", "title": "Priority", "description": "Affects the order in which downloaders are used. Higher values are attempted first.", "default": 0, "order": 1000 }
  }
}
```

* The response is a single JSON Schema (draft-07) for the requested plugin type, with the same reserved-field and capability-toggle injections described for `GET /api/plugins/schemas`.
* `properties` — object. Each property is a JSON Schema fragment with UI metadata (`type`, `title`, `description`, `default`, `order`, and optionally `placeholder`, `dependsOn`, `parent`).
* `404` `{"error": "No schema available for plugin type '<name>'}"` when no registered type matches.

#### `GET /api/plugins/status`

**Scope:** `plugins:read`

##### Input

None.

##### Output

```json
[
  {
    "name": "qBittorrent",
    "category": "downloader",
    "ok": true,
    "message": null
  },
  {
    "name": "Discord",
    "category": "notifier",
    "ok": false,
    "message": "connection refused"
  }
]
```

* `name` — string. Plugin display name for the status entry.
* `category` — string. Plugin category: `downloader`, `notifier`, `source`, `metadata`, or `plugin` for external plugins.
* `ok` — boolean. In-process health: for external instances this is the type host's last probe result, and it is `false` for instances in a failed state.
* `message` — string or `null`. Failure reason / cooldown text; `null` when the entry is healthy.
* Entries are ordered deterministically (by resolved type id, with backend-owned instance ordering); the endpoint performs no plugin RPCs.

### Plugins Write

**Scope:** `plugins:write`

#### `POST /api/config/plugins_cfg/{section}/{plugin_id}/instances`

**Scope:** `plugins:write`

##### Input

* `section` — path parameter. One of `downloader`, `notifier`, `source`, `metadata`; any other value returns `400`.
* `plugin_id` — path parameter. Plugin type id the new instance belongs to.

```json
{
  "name": "qBittorrent",
  "enabled": true,
  "link": "http://localhost:8080",
  "username": "admin",
  "password": "",
  "download_path": "./downloads",
  "use_separate_paths": false,
  "default_category": "Series",
  "verify_ssl": false,
  "priority": 0
}
```

* The request body is the raw instance config object, stored verbatim; its shape is defined by the plugin type's schema (`GET /api/plugins/{name}/schema`). Fields are plugin-specific and optional at this boundary.
* `enabled` — boolean, conventional. When `true` for a `metadata` section, any other enabled metadata instance is auto-disabled.
* The backend ignores any client-supplied instance id and generates a fresh 12-character lowercase-hex id (48 bits of randomness), returned as a key in the output config.

##### Output

```json
{
  "enabled": true,
  "directory": "/config/plugins",
  "downloader": {
    "qbittorrent": {
      "a1b2c3d4e5f6": {
        "name": "qBittorrent",
        "enabled": true,
        "link": "http://localhost:8080",
        "priority": 0
      }
    }
  },
  "notifier": {},
  "source": {},
  "metadata": {}
}
```

* The full persisted `PluginsConfig` after insertion (same schema as `GET /api/config/plugins_cfg`), with the new instance present under the given `plugin_id` and its generated instance id.
* `403` `{"error": "Insufficient scope: missing plugins:write"}` when the key lacks the scope; `400` `{"error": "Invalid plugin section: <section>"}` for an unknown section; `500` for a storage failure.

#### `DELETE /api/config/plugins_cfg/{section}/{plugin_id}/{instance_id}`

**Scope:** `plugins:write`

##### Input

* `section` — path parameter. One of `downloader`, `notifier`, `source`, `metadata`; any other value returns `400`.
* `plugin_id` — path parameter. Plugin type id owning the instance.
* `instance_id` — path parameter. Instance id to remove.

##### Output

```json
{
  "enabled": true,
  "directory": "/config/plugins",
  "downloader": {},
  "notifier": {},
  "source": {},
  "metadata": {}
}
```

* The full persisted `PluginsConfig` after deletion (same schema as `GET /api/config/plugins_cfg`); the removed instance no longer appears.
* Deleting a `metadata` instance also purges that instance's cached metadata (episodes, seasons, series, and fetch log).
* `403` `{"error": "Insufficient scope: missing plugins:write"}` when the key lacks the scope; `400` `{"error": "Invalid plugin section: <section>"}` for an unknown section; `500` for a storage failure.

#### `PUT /api/config/plugins_cfg/{section}`

**Scope:** `plugins:write`

##### Input

* `section` — path parameter. One of `downloader`, `notifier`, `source`, `metadata`; any other value returns `400`.

```json
{
  "qbittorrent": {
    "a1b2c3d4e5f6": {
      "name": "qBittorrent",
      "enabled": true,
      "link": "http://localhost:8080",
      "priority": 0
    }
  }
}
```

* Top-level key — `plugin_id`. Plugin type id.
* Nested key — `instance_id`. Value is that instance's config object (`serde_json::Value`), plugin-defined.
* The body replaces the entire named section. Any enabled instance missing a field listed in its schema's `required` array is auto-disabled and reported in `warnings`; schema defaults are filled for properties absent from the config.
* Instance `name` values must satisfy the plugin-name validation used as slug prefixes (e.g. `[a-zA-Z0-9 ]`); an invalid name returns `400`.
* `enabled` — boolean (inside each instance config). At most one enabled instance is kept for the `metadata` section; extras are auto-disabled and the config is re-saved.

##### Output

```json
{
  "config": {
    "enabled": true,
    "directory": "/config/plugins",
    "downloader": {},
    "notifier": {},
    "source": {},
    "metadata": {
      "tvdb": {
        "998877665544": {
          "name": "TVDB",
          "enabled": true,
          "api_key": "xxxxxxxx",
          "refresh_interval": 12
        }
      }
    }
  },
  "warnings": ["Disabled 'tvdb' instance 'TVDB': missing required field(s): api_key"]
}
```

* `config` — object. The full persisted `PluginsConfig` after the save (same schema as `GET /api/config/plugins_cfg`), including any metadata enforcement applied.
* `warnings` — array of strings. Human-readable messages for instances that were auto-disabled for missing required fields; empty when nothing was corrected.
* `403` `{"error": "Insufficient scope: missing plugins:write"}` when the key lacks the scope; `400` `{"error": "Invalid plugin section: <section>"}` for an unknown section or `{"error": "<name validation message>"}` for a bad instance name; `500` for a storage failure.

#### `POST /api/plugins/{name}/validate`

**Scope:** `plugins:write`

##### Input

* `name` — path parameter. Plugin instance/type key used to look up the loaded plugin; an unknown key returns `404`.

```json
{
  "name": "TVDB",
  "enabled": true,
  "api_key": "xxxxxxxx",
  "refresh_interval": 12
}
```

* The body is an arbitrary JSON value forwarded verbatim as the plugin's `validate_config` parameter; its shape is plugin-specific (typically a candidate instance config).

##### Output

```json
true
```

* Plugin-defined result, returned as arbitrary JSON. SDK plugins return `true` on success or `{"errors": ["..."]}` on failure. A plugin that does not implement `validate_config` surfaces as a `500`.
* `404` `{"error": "Plugin not found"}` when no loaded plugin matches `name`; `500` `{"error": "Internal Server Error"}` when the plugin call fails.

#### `POST /api/plugins/test`

**Scope:** `plugins:write`

##### Input

```json
{
  "category": "downloader",
  "plugin_type": "qbittorrent",
  "config": {
    "name": "qBittorrent",
    "enabled": true,
    "link": "http://localhost:8080",
    "username": "admin",
    "password": ""
  }
}
```

* `category` — string. Required. Plugin category, combined with `plugin_type` to form the registry key `{category}.{plugin_type.toLowerCase()}`.
* `plugin_type` — string. Required. Plugin type short name.
* `config` — object. Required. Full candidate instance config; the backend forces `enabled: true` for the duration of the test so disabled plugins can still be tested.

##### Output

```json
"Successfully connected to qBittorrent"
```

* The response is a JSON string: the plugin's `test` result when it returns a string, otherwise `Test passed for <plugin_type>`. The default implementation returns `Successfully connected to <display_name>`.
* Runs an operational connectivity check (`test`), distinct from the internal `health_check`; it is bounded by a 10-second timeout.
* `403` `{"error": "Insufficient scope: missing plugins:write"}` when the key lacks the scope; `400` `{"error": "Failed to create plugin: <reason>"}` or `{"error": "Test failed: <reason>"}`; `408` `{"error": "Test timed out"}`.

---

## 9. Series Import Endpoints

Series import is handled under `files:write` scope:

### `POST /api/system/organized_series/preview`

**Scope:** `files:write`

#### Input

```json
{
  "path": "/media/tv",
  "is_bulk": true
}
```

* `path` — string. Required. Directory to scan. The path is validated at the boundary (non-empty, not a traversal, within length limits), must exist and be a directory, and must not be the Unix filesystem root `/`; a violation returns `400` with `{"error": "..."}`.
* `is_bulk` — boolean. Required. When `true`, only the immediate child directories of `path` are enumerated (e.g. a root full of series folders such as a torrent watch-dir); when `false`, `path`'s own folder is evaluated directly.

#### Output

```json
[
  {
    "path": "/media/tv/Breaking Bad",
    "original_folder_name": "Breaking Bad",
    "final_title": "Breaking Bad",
    "season_count": 5,
    "episode_count": 62,
    "selected": true,
    "already_exists": false,
    "all_files_in_root": false
  }
]
```

* `path` — Absolute path to the detected series folder.
* `original_folder_name` — Folder name as found on disk.
* `final_title` — Proposed series title (defaults to `original_folder_name`, editable before confirming the import).
* `season_count` — Number of distinct seasons found; only seasons actually declared in filenames are counted.
* `episode_count` — Number of video files detected (directory traversal is capped at 5 levels deep).
* `selected` — Whether the item is pre-selected for import; `false` when `already_exists` is `true`.
* `already_exists` — `true` when an existing series mapping already claims this path (compared after canonicalization, so symlink-equivalent paths are detected).
* `all_files_in_root` — `true` when all detected video files reside directly in the series root with no season subdirectories; the import flow auto-enables `flatten_season_folders` for such items. Optional on input (defaults to `false`).

### `POST /api/system/organized_series/bulk`

**Scope:** `files:write`

#### Input

```json
{
  "items": [
    {
      "path": "/media/tv/Breaking Bad",
      "original_folder_name": "Breaking Bad",
      "final_title": "Breaking Bad",
      "season_count": 5,
      "episode_count": 62,
      "selected": true,
      "already_exists": false,
      "all_files_in_root": false
    }
  ],
  "scan_for_existing": true,
  "monitor_mode": "all",
  "quality_profile": "HD-1080p",
  "release_profile": "Default"
}
```

* `items` — array. Required. Preview items to import, each shaped like a `PreviewSeriesItem` (see the preview endpoint above). Items with `selected: false` are skipped.
* `scan_for_existing` — boolean. Required. When `true`, each newly created series is scanned for existing episodes (`import_scan_for_series`, matching by directory path and `SXXEXX` patterns; duplicate season/episode tuples are left unassigned for the user to resolve).
* `monitor_mode` — string or `null`. Optional. Monitoring mode applied to each created series after the scan: one of `all`, `future`, `missing`, `existing`, `pilot`, `firstSeason`, `specials`, `none`.
* `quality_profile` — string or `null`. Optional. Quality profile stored on the new mapping.
* `release_profile` — string or `null`. Optional. Release profile stored on the new mapping.

#### Output

`200 OK` with an empty body.

Per-item failures — a path already claimed by another mapping, a filesystem-root path, or a directory-creation error — are logged and skip only that item; the request still returns `200`.

---

## 10. Validation

The backend validates all inputs at the API boundary. Key validation rules:

| Field | Validator | Constraint |
|---|---|---|
| Search `query` | `validate_search_query` | Non-empty, ≤ 255 chars (manual search only — auto modes ignore `query`) |
| API key `scopes` | `validate_api_key_scopes` | At least one scope |
| `season_pack_replace_threshold` | `validate_season_pack_replace_threshold` | 0–100 |
| `media_info_scan_interval` | `validate_media_info_scan_interval` | ≥ 1 minute |
| `unexpected_files_handling` | `validate_unexpected_files_handling` | One of: `keep`, `delete` |
| Episode numbers | `validate_episode_numbers_not_empty` | Non-empty array |
| Auto-search interval | `validate_auto_search_wanted_interval` | Between 5–1440 minutes |
| Auto-search window | `validate_auto_search_wanted_window` | `min_wait` ≤ `max_age_days × 1440` |

Validation rules live in `crates/shared/src/validation/` and run on both form input and the API boundary.

---

## 11. API Key Scopes

Scopes follow a hierarchy: `*:write` implies `*:read`.

| Scope | Grants |
|---|---|
| `series:read` | View series, episodes, files |
| `series:write` | + Create, update, delete series |
| `config:read` | View settings, profiles, qualities |
| `config:write` | + Modify settings |
| `files:read` | View file listings, organized series |
| `files:write` | + File operations, imports |
| `queue:read` | View download queue |
| `queue:write` | + Add, remove, pause, resume downloads |
| `system:read` | View system health, about info |
| `auth:read` | View bans |
| `auth:write` | + Manage bans, generate API keys / calendar tokens |
| `logs:read` | View application logs |
| `plugins:read` | View plugins, schemas, status |
| `plugins:write` | + Test plugins, modify plugin config |
| `wanted:read` | View missing episodes |
| `activity:read` | View recent activity |
| `rename:read` | View rename queue |
| `rename:write` | + Execute renames, remediate |
| `search` | Search for media (standalone, no read/write split) |

An authenticated caller whose key lacks a required scope gets `403 Forbidden` with a
`WWW-Authenticate: Bearer error="insufficient_scope", scope="<required>"` header
and a JSON body naming the required scopes and the subset the key is missing. The
key's granted scopes are not echoed:

```json
{
  "error": "Insufficient scope: missing queue:write",
  "required_scopes": ["queue:write"],
  "missing_scopes": ["queue:write"]
}
```

Missing or invalid credentials return `401` instead, with a
`WWW-Authenticate: Basic realm="jumbie"` challenge.

---

## 12. Common Response Formats

### Paginated Response

```json
{
  "items": [...],
  "total": 100,
  "page": 1,
  "page_size": 25
}
```

### Error Response

Handler errors and scope failures use one envelope:

```json
{
  "error": "Description of what went wrong",
  "series_id": "…",
  "required_scopes": ["…"],
  "missing_scopes": ["…"]
}
```

Only `error` is always present:

- `series_id` — only on a path collision (add/move series), so the UI can link to
  the series claiming the path.
- `required_scopes` / `missing_scopes` — only on a `403` insufficient-scope
  failure. See [§11](#11-api-key-scopes) for the accompanying `WWW-Authenticate`
  header.

A `401` (missing or invalid credentials) is the exception: it has an **empty
body** plus a `WWW-Authenticate: Basic realm="jumbie"` challenge.

### Series List Sorting

Supports `sort` and `order` query parameters:
- `sort` — Field name (e.g., `title`, `date_added`, `size_on_disk`)
- `order` — `asc` or `desc`

### API Key Generation Response

```json
{
  "id": "550e8400-e29b-41d4-a716-446655440000",
  "key": "jb_aBcDeFgHiJkLmNoPqRsTuVwXyZ0123456789-abc",
  "prefix": "jb_aBcDeFgH"
}
```

Returned only once; see [API Keys](#api-keys).

### Calendar Token Generation Response

```json
{
  "id": "uuid",
  "token": "cal_aBcDeFgHiJkLmNoPqRsTuVw",
  "hide_unmonitored": false,
  "show_as_all_day": false
}
```

### Add Download Response

Returned by `POST /api/downloads`. The request only *queues* the download — the
download client is assigned later by the organizer, so `client_id` is not known
here. Poll `GET /api/queue` with `queue_id` to read the assigned `client_id`.

```json
{
  "outcome": "added",
  "queue_id": 42
}
```

`outcome` is one of `"added"`, `"replaced"`, `"skipped"`, or `"merged"`.
`queue_id` is `null` only when nothing was queued (`"skipped"`).

---

## 13. Notes

### Timestamps

Every timestamp in an API response is RFC 3339 with an explicit offset (UTC),
e.g. `2026-06-18T20:00:00+00:00`. Timestamps are never sent zone-less, so
clients should render them in the user's local timezone.

The database stores timestamps as naive UTC; conversion to RFC 3339 happens at
the API boundary — `jumbie_shared::serde_utc` for serde types, and
`UtcDateTime::to_rfc3339_utc` for string-typed fields. Both delegate to the
shared primitive `jumbie_shared::datetime::naive_utc_to_rfc3339`.

Response fields that carry timestamps:

- `GET /api/queue` — `downloaded_at`, `next_retry_at`
- `GET /api/series/{id}` and `POST /api/series/details/batch` — episode
  `created_at`, `file_acquired_at`
- `GET /api/automatic-profiles/{submitter}/records` — `date_added`
- `GET /api/activity` — `timestamp`
- `GET /api/system/logs` — `timestamp`
- `GET /api/wanted` and `GET /api/calendar` — `eff_date`, `dates.*`
- `GET /api/auth/bans` — `banned_at`, `banned_until`

#### Request timestamp format (strict)

**RFC 3339 requires an explicit offset.** Every inbound timestamp must be RFC 3339
with a `Z` or `±HH:MM` offset, e.g. `2026-06-18T20:30:00+09:00` or
`2026-06-18T11:30:00Z`. The backend normalizes it to UTC — the offset is applied,
never discarded.

Zone-less and date-only values are **rejected with `400`**:

- `2026-06-18T20:00:00` (naive, no offset) — a zone-less timestamp is ambiguous,
  so the client must state the zone.
- `2026-06-18` (date-only) — not a full timestamp.

This applies to every inbound date field: `PUT /api/episodes/{id}/est_date`,
`PUT /api/series/{id}/episodes/{episode_id}/metadata` (`meta_date`),
`POST /api/downloads` (`upload_date`), `GET /api/calendar`
(`start_date`/`end_date`), and `PUT /api/config` (`auth.api_keys[].expires_at`,
`auth.banned_ips[].banned_at` / `banned_until`).

Internally, values the backend already owns — DB rows and persisted config — are
read with a lenient parser that also accepts the DB-canonical
`YYYY-MM-DD HH:MM:SS` naive-UTC shape. That leniency never applies to request
input.
