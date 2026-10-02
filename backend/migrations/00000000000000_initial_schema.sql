-- ── Consolidated Initial Schema ─────────────────────────────────────────────
-- Replaces all prior incremental migrations with a single DDL that defines
-- every table in its final state.  Safe for both fresh databases (creates all)
-- and existing databases (CREATE TABLE IF NOT EXISTS is a no-op).
--
-- Migration history was squashed because the 108 prior incremental files
-- contained contradictory operations (add → rename → drop columns, create →
-- drop → recreate tables) that served no purpose going forward.

PRAGMA foreign_keys = ON;

-- ═══════════════════════════════════════════════════════════════════════════
-- 1. Core data tables
-- ═══════════════════════════════════════════════════════════════════════════

-- ── Episodes ─────────────────────────────────────────────────────────────
-- SSoT for episode metadata.  Columns that were dropped during migration
-- history (score, quality, release_title, submitter, download_id,
-- download_link, scoring_*) are intentionally absent — those fields now live
-- in release_info.
CREATE TABLE IF NOT EXISTS episodes (
    id                      INTEGER PRIMARY KEY AUTOINCREMENT,
    episode_id              TEXT UNIQUE NOT NULL,
    series_id               TEXT,
    season                  INTEGER,
    episode                 INTEGER NOT NULL,
    title                   TEXT,
    file_path               TEXT,
    status                  TEXT DEFAULT 'pending',
    monitored               INTEGER DEFAULT 1,
    numbering_mode          INTEGER NOT NULL DEFAULT 0,
    created_at              TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    file_acquired_at        TIMESTAMP,
    meta_date               TIMESTAMP,
    upload_date             TEXT,
    est_date                TIMESTAMP,
    metadata_ids            TEXT DEFAULT '{}',
    description             TEXT,
    runtime                 INTEGER,
    image_url               TEXT,
    absolute_override       INTEGER,
    metadata_source         TEXT,
    episode_cell_type       INTEGER,
    quality_profile_id      TEXT
);

CREATE INDEX IF NOT EXISTS idx_episodes_status           ON episodes(status);
CREATE INDEX IF NOT EXISTS idx_episodes_series_id        ON episodes(series_id);
CREATE INDEX IF NOT EXISTS idx_episodes_file_path        ON episodes(file_path);
CREATE INDEX IF NOT EXISTS idx_episodes_episode_id       ON episodes(episode_id);

-- ── Series Mappings ───────────────────────────────────────────────────────
-- JSON-backed config with materialized columns for indexed access.
CREATE TABLE IF NOT EXISTS series_mappings (
    id                  TEXT PRIMARY KEY,
    data                TEXT NOT NULL,
    target_title        TEXT NOT NULL DEFAULT '',
    absolute_numbering  INTEGER NOT NULL DEFAULT 0,
    total_size          INTEGER NOT NULL DEFAULT 0,
    episode_start       INTEGER,
    episode_end         INTEGER,
    release_profile     TEXT NOT NULL DEFAULT ''
);

CREATE INDEX IF NOT EXISTS idx_series_mappings_target_title
    ON series_mappings(target_title);
CREATE INDEX IF NOT EXISTS idx_series_mappings_episode_range
    ON series_mappings(episode_start, episode_end);
CREATE INDEX IF NOT EXISTS idx_series_mappings_release_profile
    ON series_mappings(release_profile);

-- Keep materialized columns in sync with the JSON data column.
CREATE TRIGGER IF NOT EXISTS trg_series_mappings_target_title_insert
    AFTER INSERT ON series_mappings FOR EACH ROW BEGIN
        UPDATE series_mappings SET target_title = COALESCE(json_extract(NEW.data, '$.target_title'), '') WHERE id = NEW.id;
    END;
CREATE TRIGGER IF NOT EXISTS trg_series_mappings_target_title_update
    AFTER UPDATE OF data ON series_mappings FOR EACH ROW BEGIN
        UPDATE series_mappings SET target_title = COALESCE(json_extract(NEW.data, '$.target_title'), '') WHERE id = NEW.id;
    END;
CREATE TRIGGER IF NOT EXISTS trg_series_mappings_abs_numbering_insert
    AFTER INSERT ON series_mappings FOR EACH ROW BEGIN
        UPDATE series_mappings SET absolute_numbering = CASE WHEN json_extract(NEW.data, '$.settings.absolute_numbering') = 'true' THEN 1 ELSE 0 END WHERE id = NEW.id;
    END;
CREATE TRIGGER IF NOT EXISTS trg_series_mappings_abs_numbering_update
    AFTER UPDATE OF data ON series_mappings FOR EACH ROW BEGIN
        UPDATE series_mappings SET absolute_numbering = CASE WHEN json_extract(NEW.data, '$.settings.absolute_numbering') = 'true' THEN 1 ELSE 0 END WHERE id = NEW.id;
    END;
CREATE TRIGGER IF NOT EXISTS trg_series_mappings_episode_start_insert
    AFTER INSERT ON series_mappings FOR EACH ROW BEGIN
        UPDATE series_mappings SET episode_start = CAST(json_extract(NEW.data, '$.episode_start') AS INTEGER) WHERE id = NEW.id;
    END;
CREATE TRIGGER IF NOT EXISTS trg_series_mappings_episode_start_update
    AFTER UPDATE OF data ON series_mappings FOR EACH ROW BEGIN
        UPDATE series_mappings SET episode_start = CAST(json_extract(NEW.data, '$.episode_start') AS INTEGER) WHERE id = NEW.id;
    END;
CREATE TRIGGER IF NOT EXISTS trg_series_mappings_episode_end_insert
    AFTER INSERT ON series_mappings FOR EACH ROW BEGIN
        UPDATE series_mappings SET episode_end = CAST(json_extract(NEW.data, '$.episode_end') AS INTEGER) WHERE id = NEW.id;
    END;
CREATE TRIGGER IF NOT EXISTS trg_series_mappings_episode_end_update
    AFTER UPDATE OF data ON series_mappings FOR EACH ROW BEGIN
        UPDATE series_mappings SET episode_end = CAST(json_extract(NEW.data, '$.episode_end') AS INTEGER) WHERE id = NEW.id;
    END;
CREATE TRIGGER IF NOT EXISTS trg_series_mappings_release_profile_insert
    AFTER INSERT ON series_mappings FOR EACH ROW BEGIN
        UPDATE series_mappings SET release_profile = COALESCE(json_extract(NEW.data, '$.release_profile'), '') WHERE id = NEW.id;
    END;
CREATE TRIGGER IF NOT EXISTS trg_series_mappings_release_profile_update
    AFTER UPDATE OF data ON series_mappings FOR EACH ROW BEGIN
        UPDATE series_mappings SET release_profile = COALESCE(json_extract(NEW.data, '$.release_profile'), '') WHERE id = NEW.id;
    END;

-- ── Episode Parts ─────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS episode_parts (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    episode_id  TEXT    NOT NULL,
    part_number INTEGER NOT NULL,
    file_path   TEXT    NOT NULL,
    size        INTEGER,
    fingerprint TEXT,
    media_info  TEXT,
    UNIQUE(episode_id, part_number),
    FOREIGN KEY (episode_id) REFERENCES episodes(episode_id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_episode_parts_episode_id ON episode_parts(episode_id);
CREATE INDEX IF NOT EXISTS idx_episode_parts_file_path  ON episode_parts(file_path);

-- ── Season Overrides ──────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS season_overrides (
    series_id      TEXT NOT NULL REFERENCES series_mappings(id) ON DELETE CASCADE,
    season         INTEGER NOT NULL,
    numbering_mode INTEGER NOT NULL DEFAULT 0,
    episode_start  INTEGER,
    episode_end    INTEGER,
    episode_offset INTEGER NOT NULL DEFAULT 0,
    cell_count     INTEGER,
    PRIMARY KEY (series_id, season, numbering_mode)
);

CREATE INDEX IF NOT EXISTS idx_season_overrides_lookup
    ON season_overrides(series_id, season, numbering_mode);

-- ═══════════════════════════════════════════════════════════════════════════
-- 2. File tracking
-- ═══════════════════════════════════════════════════════════════════════════

-- ── File Fingerprints ─────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS file_fingerprints (
    id                      INTEGER PRIMARY KEY AUTOINCREMENT,
    file_path               TEXT NOT NULL,
    inode                   INTEGER,
    device                  INTEGER,
    size                    INTEGER,
    mtime                   REAL,
    quick_hash              TEXT,
    state                   TEXT DEFAULT 'pending',
    last_seen_at            TIMESTAMP,
    expected_path           TEXT,
    episode_id              TEXT,
    media_info              TEXT,
    media_info_scan_failed  BOOLEAN NOT NULL DEFAULT 0
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_file_fingerprints_file_path
    ON file_fingerprints(file_path);
CREATE INDEX IF NOT EXISTS idx_file_fingerprints_episode_id
    ON file_fingerprints(episode_id);
CREATE INDEX IF NOT EXISTS idx_file_fingerprints_state
    ON file_fingerprints(state);
CREATE INDEX IF NOT EXISTS idx_file_fingerprints_quick_hash
    ON file_fingerprints(quick_hash);

-- ── File Event Log ────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS file_event_log (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    created_at       TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    event_type       TEXT NOT NULL,
    source_path      TEXT NOT NULL,
    destination_path TEXT,
    status           TEXT DEFAULT 'pending'
);

CREATE INDEX IF NOT EXISTS idx_file_event_log_status ON file_event_log(status);

-- ── Retry Queue ───────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS retry_queue (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    operation         TEXT NOT NULL,
    source_path       TEXT NOT NULL,
    destination_path  TEXT,
    episode_id        TEXT,
    error_message     TEXT,
    retry_count       INTEGER DEFAULT 0,
    max_retries       INTEGER DEFAULT 5,
    next_retry_at     TIMESTAMP,
    created_at        TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    last_attempt_at   TIMESTAMP,
    status            TEXT DEFAULT 'pending',
    FOREIGN KEY (episode_id) REFERENCES episodes(episode_id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_retry_queue_scheduling ON retry_queue(next_retry_at, status);
CREATE INDEX IF NOT EXISTS idx_retry_queue_episode_id  ON retry_queue(episode_id);

-- ═══════════════════════════════════════════════════════════════════════════
-- 3. Download / Release tracking
-- ═══════════════════════════════════════════════════════════════════════════

-- ── Download Queue ────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS download_queue (
    id                    INTEGER PRIMARY KEY AUTOINCREMENT,
    downloaded_at         DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    media_name            TEXT NOT NULL,
    media_link            TEXT NOT NULL,
    series_title          TEXT NOT NULL,
    season                TEXT,
    episode               INTEGER,
    episode_end           INTEGER,
    episode_id            TEXT,
    score                 INTEGER NOT NULL DEFAULT 0,
    is_user_requested     BOOLEAN NOT NULL DEFAULT 0,
    status                TEXT NOT NULL DEFAULT 'Queued',
    downloader_id         TEXT,
    client_id             TEXT,
    progress              REAL,
    error_message         TEXT,
    series_id             TEXT NOT NULL DEFAULT '',
    is_season_pack        INTEGER NOT NULL DEFAULT 0,
    category              TEXT NOT NULL DEFAULT '',
    retry_count           INTEGER NOT NULL DEFAULT 0,
    next_retry_at         DATETIME,
    multi_targets         TEXT,
    episode_intentions    TEXT,
    submitter             TEXT,
    download_id           TEXT,
    version               INTEGER NOT NULL DEFAULT 1,
    source_pub_date       TEXT,
    scoring_size_bytes    INTEGER,
    scoring_seeders       INTEGER,
    scoring_episode_count INTEGER,
    FOREIGN KEY (episode_id) REFERENCES episodes(episode_id) ON DELETE SET NULL
);

CREATE INDEX IF NOT EXISTS idx_download_queue_episode_id ON download_queue(episode_id);
CREATE INDEX IF NOT EXISTS idx_download_queue_status     ON download_queue(status);

-- ── Release Info ──────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS release_info (
    quick_hash            TEXT PRIMARY KEY,
    release_title         TEXT,
    submitter             TEXT,
    download_link         TEXT,
    created_at            TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    score                 INTEGER,
    download_id           TEXT,
    upload_date           DATETIME,
    version               INTEGER NOT NULL DEFAULT 1,
    scoring_size_bytes    INTEGER,
    scoring_seeders       INTEGER,
    scoring_episode_count INTEGER
);

-- ═══════════════════════════════════════════════════════════════════════════
-- 4. Configuration tables
-- ═══════════════════════════════════════════════════════════════════════════

-- ── Release Profiles ──────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS release_profiles (
    id   TEXT PRIMARY KEY,
    data TEXT NOT NULL
);

-- ── Quality Definitions ───────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS quality_definitions (
    id   TEXT PRIMARY KEY,
    data TEXT NOT NULL
);

-- ── Quality Profiles ─────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS quality_profiles (
    id   TEXT PRIMARY KEY,
    data TEXT NOT NULL
);

-- ── Config Defaults ──────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS config_defaults (
    key  TEXT PRIMARY KEY,
    data TEXT NOT NULL
);

-- ── Plugin Instances ─────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS plugin_instances (
    id          TEXT PRIMARY KEY,
    category    TEXT NOT NULL,
    plugin_id   TEXT NOT NULL,
    instance_id TEXT NOT NULL,
    data        TEXT NOT NULL,
    UNIQUE(category, plugin_id, instance_id)
);

CREATE INDEX IF NOT EXISTS idx_plugin_instances_category ON plugin_instances(category);

-- ═══════════════════════════════════════════════════════════════════════════
-- 5. Automatic Profiles (scoring system)
-- ═══════════════════════════════════════════════════════════════════════════

-- ── Automatic Profile ─────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS automatic_profile (
    submitter TEXT PRIMARY KEY,
    score     INTEGER NOT NULL
);

-- ── Automatic Profile Records ─────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS automatic_profile_records (
    id              TEXT PRIMARY KEY,
    submitter       TEXT NOT NULL,
    description     TEXT NOT NULL,
    date_added      DATETIME DEFAULT CURRENT_TIMESTAMP NOT NULL,
    category        TEXT NOT NULL DEFAULT 'General',
    score           INTEGER NOT NULL DEFAULT 0,
    source_identity TEXT,
    extension       TEXT,
    FOREIGN KEY (submitter) REFERENCES automatic_profile(submitter) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_auto_records_submitter ON automatic_profile_records(submitter);
CREATE INDEX IF NOT EXISTS idx_auto_records_path      ON automatic_profile_records(source_identity);
CREATE INDEX IF NOT EXISTS idx_auto_records_category  ON automatic_profile_records(category);

-- ── Automatic Profile Unknown Files ───────────────────────────────────────
CREATE TABLE IF NOT EXISTS automatic_profile_unknown_files (
    submitter TEXT NOT NULL,
    extension TEXT NOT NULL,
    count     INTEGER NOT NULL DEFAULT 1,
    PRIMARY KEY (submitter, extension),
    FOREIGN KEY (submitter) REFERENCES automatic_profile(submitter) ON DELETE CASCADE
);

-- ── Automatic Profile Media Scans ─────────────────────────────────────────
CREATE TABLE IF NOT EXISTS automatic_profile_media_scans (
    id          TEXT PRIMARY KEY,
    submitter   TEXT NOT NULL,
    filename    TEXT NOT NULL,
    series_id   TEXT,
    season      INTEGER,
    episode     INTEGER,
    media_info  TEXT NOT NULL,
    date_added  DATETIME DEFAULT CURRENT_TIMESTAMP NOT NULL,
    FOREIGN KEY (submitter) REFERENCES automatic_profile(submitter) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_auto_scans_submitter ON automatic_profile_media_scans(submitter);
CREATE INDEX IF NOT EXISTS idx_auto_scans_series    ON automatic_profile_media_scans(series_id, season);

-- ═══════════════════════════════════════════════════════════════════════════
-- 6. Metadata cache tables
-- ═══════════════════════════════════════════════════════════════════════════

-- ── Metadata Series Cache ─────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS metadata_series_cache (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    metadata_id     TEXT NOT NULL,
    provider_id     TEXT NOT NULL,
    instance_id     TEXT NOT NULL DEFAULT '',
    title           TEXT NOT NULL,
    overview        TEXT,
    language        TEXT,
    aliases         TEXT DEFAULT '[]',
    image_url       TEXT,
    fetched_at      TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(metadata_id, provider_id, instance_id)
);

-- ── Metadata Episodes Cache ───────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS metadata_episodes_cache (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    metadata_id     TEXT NOT NULL,
    provider_id     TEXT NOT NULL,
    instance_id     TEXT NOT NULL DEFAULT '',
    ordering_mode   TEXT NOT NULL DEFAULT 'normal',
    season_number   INTEGER NOT NULL,
    episode_number  INTEGER NOT NULL,
    unique_id       TEXT NOT NULL,
    title           TEXT NOT NULL,
    description     TEXT,
    runtime         INTEGER,
    image_url       TEXT,
    meta_date       TEXT,
    fetched_at      TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(metadata_id, provider_id, instance_id, ordering_mode, season_number, episode_number)
);

CREATE INDEX IF NOT EXISTS idx_metadata_episodes_cache_lookup
    ON metadata_episodes_cache(metadata_id, provider_id, instance_id, ordering_mode, season_number, episode_number);

-- ── Metadata Season Cache ─────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS metadata_season_cache (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    metadata_id     TEXT NOT NULL,
    provider_id     TEXT NOT NULL DEFAULT '',
    instance_id     TEXT NOT NULL DEFAULT '',
    ordering_mode   TEXT NOT NULL DEFAULT 'normal',
    season_number   TEXT NOT NULL,
    episode_count   INTEGER NOT NULL,
    updated_at      TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(metadata_id, provider_id, instance_id, ordering_mode, season_number)
);

CREATE INDEX IF NOT EXISTS idx_metadata_season_cache_lookup
    ON metadata_season_cache(metadata_id, provider_id, instance_id, ordering_mode);

-- ── Metadata Fetch Log ────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS metadata_fetch_log (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    metadata_id     TEXT NOT NULL,
    provider_id     TEXT NOT NULL,
    instance_id     TEXT NOT NULL DEFAULT '',
    ordering_mode   TEXT NOT NULL,
    attempted_at    TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(metadata_id, provider_id, instance_id, ordering_mode)
);

-- ═══════════════════════════════════════════════════════════════════════════
-- 7. Auth tables
-- ═══════════════════════════════════════════════════════════════════════════

-- ── Users ─────────────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS users (
    username      TEXT PRIMARY KEY,
    password_hash TEXT NOT NULL
);

-- ── API Keys ──────────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS api_keys (
    id         TEXT PRIMARY KEY,
    name       TEXT NOT NULL,
    key_hash   TEXT NOT NULL,
    scopes     TEXT NOT NULL,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    expires_at TEXT DEFAULT NULL,
    prefix     TEXT NOT NULL DEFAULT ''
);

-- ── Calendar Tokens ───────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS calendar_tokens (
    id                  TEXT PRIMARY KEY,
    name                TEXT NOT NULL,
    token               TEXT NOT NULL,
    created_at          DATETIME DEFAULT CURRENT_TIMESTAMP,
    include_unmonitored INTEGER NOT NULL DEFAULT 0,
    show_as_all_day     INTEGER NOT NULL DEFAULT 0
);

-- ═══════════════════════════════════════════════════════════════════════════
-- 8. Security / system tables
-- ═══════════════════════════════════════════════════════════════════════════

-- ── Banned IPs ────────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS banned_ips (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    ip          TEXT NOT NULL,
    fail_count  INTEGER NOT NULL DEFAULT 0,
    ban_count   INTEGER NOT NULL DEFAULT 0,
    banned_at   TEXT NOT NULL,
    banned_until TEXT,
    UNIQUE(ip)
);

CREATE INDEX IF NOT EXISTS idx_banned_ips_ip ON banned_ips(ip);

-- ── System State ──────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS system_state (
    key  TEXT PRIMARY KEY NOT NULL,
    data TEXT NOT NULL
);

-- ── Activity Log ──────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS activity_log (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    created_at   TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    event_type   TEXT NOT NULL,
    series_title TEXT NOT NULL DEFAULT '',
    season       TEXT,
    episode      INTEGER,
    episode_end  INTEGER,
    title        TEXT,
    details      TEXT,
    status       TEXT NOT NULL DEFAULT 'Success'
);

CREATE INDEX IF NOT EXISTS idx_activity_log_created_at ON activity_log(created_at DESC);

-- ═══════════════════════════════════════════════════════════════════════════
-- 9. Application-created tracking tables
-- ═══════════════════════════════════════════════════════════════════════════

-- No dead tables are created.  The following tables from earlier migration
-- history were dropped and are intentionally absent:
--   file_transitions    (dropped — never referenced by Rust code)
--   downloads           (dropped — replaced by download_queue + release_info)
--   rss_history         (dropped — never wired up; source plugins handle dedup)
--   unknown_files       (dropped — never wired up; filesystem walking covers this)
--   tvmaze_seasons      (dropped — superseded by metadata_season_cache)
