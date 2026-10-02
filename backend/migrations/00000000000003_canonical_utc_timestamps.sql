-- Canonical UTC timestamp storage.
--
-- Invariant: DB, backend, and API deal only in UTC; the frontend converts to
-- local time for display. Every timestamp column stores the canonical shape
-- `YYYY-MM-DD HH:MM:SS` (naive UTC). (An optional fractional part is tolerated
-- for legacy rows; the LIKE pattern below allows trailing characters.)
--
-- This migration:
--   1. Backfills existing non-canonical values (RFC 3339 with offset, date-only)
--      into the canonical shape.
--   2. Enforces the shape on every write via BEFORE INSERT/UPDATE triggers.
--
-- WHY triggers rather than column CHECK constraints: SQLite cannot
-- `ALTER TABLE ... ADD CONSTRAINT`, so a CHECK would require rebuilding all 16
-- timestamp tables (destructive, FK/index-heavy). A `BEFORE INSERT/UPDATE`
-- trigger with `RAISE(ABORT)` enforces the identical invariant non-destructively.
--
-- Generic containers (`system_state.data`, `series_mappings.data`) are not
-- timestamp columns and are intentionally excluded.

-- ── 1. Backfill non-canonical values ─────────────────────────────────────────
-- Only rows that do not already match the canonical shape are touched.
UPDATE episodes
   SET created_at = strftime('%Y-%m-%d %H:%M:%S', created_at)
 WHERE created_at IS NOT NULL AND created_at NOT LIKE '____-__-__ __:__:__%';
UPDATE episodes
   SET file_acquired_at = strftime('%Y-%m-%d %H:%M:%S', file_acquired_at)
 WHERE file_acquired_at IS NOT NULL AND file_acquired_at NOT LIKE '____-__-__ __:__:__%';
UPDATE episodes
   SET meta_date = strftime('%Y-%m-%d %H:%M:%S', meta_date)
 WHERE meta_date IS NOT NULL AND meta_date NOT LIKE '____-__-__ __:__:__%';
UPDATE episodes
   SET upload_date = strftime('%Y-%m-%d %H:%M:%S', upload_date)
 WHERE upload_date IS NOT NULL AND upload_date NOT LIKE '____-__-__ __:__:__%';
UPDATE episodes
   SET est_date = strftime('%Y-%m-%d %H:%M:%S', est_date)
 WHERE est_date IS NOT NULL AND est_date NOT LIKE '____-__-__ __:__:__%';

UPDATE file_fingerprints
   SET last_seen_at = strftime('%Y-%m-%d %H:%M:%S', last_seen_at)
 WHERE last_seen_at IS NOT NULL AND last_seen_at NOT LIKE '____-__-__ __:__:__%';

UPDATE file_event_log
   SET created_at = strftime('%Y-%m-%d %H:%M:%S', created_at)
 WHERE created_at IS NOT NULL AND created_at NOT LIKE '____-__-__ __:__:__%';

UPDATE retry_queue
   SET next_retry_at = strftime('%Y-%m-%d %H:%M:%S', next_retry_at)
 WHERE next_retry_at IS NOT NULL AND next_retry_at NOT LIKE '____-__-__ __:__:__%';
UPDATE retry_queue
   SET created_at = strftime('%Y-%m-%d %H:%M:%S', created_at)
 WHERE created_at IS NOT NULL AND created_at NOT LIKE '____-__-__ __:__:__%';
UPDATE retry_queue
   SET last_attempt_at = strftime('%Y-%m-%d %H:%M:%S', last_attempt_at)
 WHERE last_attempt_at IS NOT NULL AND last_attempt_at NOT LIKE '____-__-__ __:__:__%';

UPDATE download_queue
   SET downloaded_at = strftime('%Y-%m-%d %H:%M:%S', downloaded_at)
 WHERE downloaded_at IS NOT NULL AND downloaded_at NOT LIKE '____-__-__ __:__:__%';
UPDATE download_queue
   SET next_retry_at = strftime('%Y-%m-%d %H:%M:%S', next_retry_at)
 WHERE next_retry_at IS NOT NULL AND next_retry_at NOT LIKE '____-__-__ __:__:__%';
UPDATE download_queue
   SET source_pub_date = strftime('%Y-%m-%d %H:%M:%S', source_pub_date)
 WHERE source_pub_date IS NOT NULL AND source_pub_date NOT LIKE '____-__-__ __:__:__%';

UPDATE release_info
   SET created_at = strftime('%Y-%m-%d %H:%M:%S', created_at)
 WHERE created_at IS NOT NULL AND created_at NOT LIKE '____-__-__ __:__:__%';
UPDATE release_info
   SET upload_date = strftime('%Y-%m-%d %H:%M:%S', upload_date)
 WHERE upload_date IS NOT NULL AND upload_date NOT LIKE '____-__-__ __:__:__%';

UPDATE automatic_profile_records
   SET date_added = strftime('%Y-%m-%d %H:%M:%S', date_added)
 WHERE date_added IS NOT NULL AND date_added NOT LIKE '____-__-__ __:__:__%';

UPDATE automatic_profile_media_scans
   SET date_added = strftime('%Y-%m-%d %H:%M:%S', date_added)
 WHERE date_added IS NOT NULL AND date_added NOT LIKE '____-__-__ __:__:__%';

UPDATE metadata_series_cache
   SET fetched_at = strftime('%Y-%m-%d %H:%M:%S', fetched_at)
 WHERE fetched_at IS NOT NULL AND fetched_at NOT LIKE '____-__-__ __:__:__%';

UPDATE metadata_episodes_cache
   SET fetched_at = strftime('%Y-%m-%d %H:%M:%S', fetched_at)
 WHERE fetched_at IS NOT NULL AND fetched_at NOT LIKE '____-__-__ __:__:__%';
UPDATE metadata_episodes_cache
   SET meta_date = strftime('%Y-%m-%d %H:%M:%S', meta_date)
 WHERE meta_date IS NOT NULL AND meta_date NOT LIKE '____-__-__ __:__:__%';

UPDATE metadata_season_cache
   SET updated_at = strftime('%Y-%m-%d %H:%M:%S', updated_at)
 WHERE updated_at IS NOT NULL AND updated_at NOT LIKE '____-__-__ __:__:__%';

UPDATE metadata_fetch_log
   SET attempted_at = strftime('%Y-%m-%d %H:%M:%S', attempted_at)
 WHERE attempted_at IS NOT NULL AND attempted_at NOT LIKE '____-__-__ __:__:__%';

UPDATE api_keys
   SET created_at = strftime('%Y-%m-%d %H:%M:%S', created_at)
 WHERE created_at IS NOT NULL AND created_at NOT LIKE '____-__-__ __:__:__%';
UPDATE api_keys
   SET expires_at = strftime('%Y-%m-%d %H:%M:%S', expires_at)
 WHERE expires_at IS NOT NULL AND expires_at NOT LIKE '____-__-__ __:__:__%';

UPDATE calendar_tokens
   SET created_at = strftime('%Y-%m-%d %H:%M:%S', created_at)
 WHERE created_at IS NOT NULL AND created_at NOT LIKE '____-__-__ __:__:__%';

UPDATE banned_ips
   SET banned_at = strftime('%Y-%m-%d %H:%M:%S', banned_at)
 WHERE banned_at IS NOT NULL AND banned_at NOT LIKE '____-__-__ __:__:__%';
UPDATE banned_ips
   SET banned_until = strftime('%Y-%m-%d %H:%M:%S', banned_until)
 WHERE banned_until IS NOT NULL AND banned_until NOT LIKE '____-__-__ __:__:__%';

UPDATE activity_log
   SET created_at = strftime('%Y-%m-%d %H:%M:%S', created_at)
 WHERE created_at IS NOT NULL AND created_at NOT LIKE '____-__-__ __:__:__%';

-- ── 2. Enforce canonical format on write ─────────────────────────────────────
-- Each table gets a BEFORE INSERT and a BEFORE UPDATE trigger. The message is
-- uniform so any violation is immediately identifiable.

CREATE TRIGGER IF NOT EXISTS trg_episodes_ts_insert
BEFORE INSERT ON episodes FOR EACH ROW
WHEN (NEW.created_at IS NOT NULL AND NEW.created_at NOT LIKE '____-__-__ __:__:__%')
  OR (NEW.file_acquired_at IS NOT NULL AND NEW.file_acquired_at NOT LIKE '____-__-__ __:__:__%')
  OR (NEW.meta_date IS NOT NULL AND NEW.meta_date NOT LIKE '____-__-__ __:__:__%')
  OR (NEW.upload_date IS NOT NULL AND NEW.upload_date NOT LIKE '____-__-__ __:__:__%')
  OR (NEW.est_date IS NOT NULL AND NEW.est_date NOT LIKE '____-__-__ __:__:__%')
BEGIN SELECT RAISE(ABORT, 'episodes: timestamp must be canonical UTC (YYYY-MM-DD HH:MM:SS)'); END;
CREATE TRIGGER IF NOT EXISTS trg_episodes_ts_update
BEFORE UPDATE OF created_at, file_acquired_at, meta_date, upload_date, est_date ON episodes FOR EACH ROW
WHEN (NEW.created_at IS NOT NULL AND NEW.created_at NOT LIKE '____-__-__ __:__:__%')
  OR (NEW.file_acquired_at IS NOT NULL AND NEW.file_acquired_at NOT LIKE '____-__-__ __:__:__%')
  OR (NEW.meta_date IS NOT NULL AND NEW.meta_date NOT LIKE '____-__-__ __:__:__%')
  OR (NEW.upload_date IS NOT NULL AND NEW.upload_date NOT LIKE '____-__-__ __:__:__%')
  OR (NEW.est_date IS NOT NULL AND NEW.est_date NOT LIKE '____-__-__ __:__:__%')
BEGIN SELECT RAISE(ABORT, 'episodes: timestamp must be canonical UTC (YYYY-MM-DD HH:MM:SS)'); END;

CREATE TRIGGER IF NOT EXISTS trg_file_fingerprints_ts_insert
BEFORE INSERT ON file_fingerprints FOR EACH ROW
WHEN (NEW.last_seen_at IS NOT NULL AND NEW.last_seen_at NOT LIKE '____-__-__ __:__:__%')
BEGIN SELECT RAISE(ABORT, 'file_fingerprints: timestamp must be canonical UTC'); END;
CREATE TRIGGER IF NOT EXISTS trg_file_fingerprints_ts_update
BEFORE UPDATE OF last_seen_at ON file_fingerprints FOR EACH ROW
WHEN (NEW.last_seen_at IS NOT NULL AND NEW.last_seen_at NOT LIKE '____-__-__ __:__:__%')
BEGIN SELECT RAISE(ABORT, 'file_fingerprints: timestamp must be canonical UTC'); END;

CREATE TRIGGER IF NOT EXISTS trg_file_event_log_ts_insert
BEFORE INSERT ON file_event_log FOR EACH ROW
WHEN (NEW.created_at IS NOT NULL AND NEW.created_at NOT LIKE '____-__-__ __:__:__%')
BEGIN SELECT RAISE(ABORT, 'file_event_log: timestamp must be canonical UTC'); END;
CREATE TRIGGER IF NOT EXISTS trg_file_event_log_ts_update
BEFORE UPDATE OF created_at ON file_event_log FOR EACH ROW
WHEN (NEW.created_at IS NOT NULL AND NEW.created_at NOT LIKE '____-__-__ __:__:__%')
BEGIN SELECT RAISE(ABORT, 'file_event_log: timestamp must be canonical UTC'); END;

CREATE TRIGGER IF NOT EXISTS trg_retry_queue_ts_insert
BEFORE INSERT ON retry_queue FOR EACH ROW
WHEN (NEW.next_retry_at IS NOT NULL AND NEW.next_retry_at NOT LIKE '____-__-__ __:__:__%')
  OR (NEW.created_at IS NOT NULL AND NEW.created_at NOT LIKE '____-__-__ __:__:__%')
  OR (NEW.last_attempt_at IS NOT NULL AND NEW.last_attempt_at NOT LIKE '____-__-__ __:__:__%')
BEGIN SELECT RAISE(ABORT, 'retry_queue: timestamp must be canonical UTC'); END;
CREATE TRIGGER IF NOT EXISTS trg_retry_queue_ts_update
BEFORE UPDATE OF next_retry_at, created_at, last_attempt_at ON retry_queue FOR EACH ROW
WHEN (NEW.next_retry_at IS NOT NULL AND NEW.next_retry_at NOT LIKE '____-__-__ __:__:__%')
  OR (NEW.created_at IS NOT NULL AND NEW.created_at NOT LIKE '____-__-__ __:__:__%')
  OR (NEW.last_attempt_at IS NOT NULL AND NEW.last_attempt_at NOT LIKE '____-__-__ __:__:__%')
BEGIN SELECT RAISE(ABORT, 'retry_queue: timestamp must be canonical UTC'); END;

CREATE TRIGGER IF NOT EXISTS trg_download_queue_ts_insert
BEFORE INSERT ON download_queue FOR EACH ROW
WHEN (NEW.downloaded_at IS NOT NULL AND NEW.downloaded_at NOT LIKE '____-__-__ __:__:__%')
  OR (NEW.next_retry_at IS NOT NULL AND NEW.next_retry_at NOT LIKE '____-__-__ __:__:__%')
  OR (NEW.source_pub_date IS NOT NULL AND NEW.source_pub_date NOT LIKE '____-__-__ __:__:__%')
BEGIN SELECT RAISE(ABORT, 'download_queue: timestamp must be canonical UTC'); END;
CREATE TRIGGER IF NOT EXISTS trg_download_queue_ts_update
BEFORE UPDATE OF downloaded_at, next_retry_at, source_pub_date ON download_queue FOR EACH ROW
WHEN (NEW.downloaded_at IS NOT NULL AND NEW.downloaded_at NOT LIKE '____-__-__ __:__:__%')
  OR (NEW.next_retry_at IS NOT NULL AND NEW.next_retry_at NOT LIKE '____-__-__ __:__:__%')
  OR (NEW.source_pub_date IS NOT NULL AND NEW.source_pub_date NOT LIKE '____-__-__ __:__:__%')
BEGIN SELECT RAISE(ABORT, 'download_queue: timestamp must be canonical UTC'); END;

CREATE TRIGGER IF NOT EXISTS trg_release_info_ts_insert
BEFORE INSERT ON release_info FOR EACH ROW
WHEN (NEW.created_at IS NOT NULL AND NEW.created_at NOT LIKE '____-__-__ __:__:__%')
  OR (NEW.upload_date IS NOT NULL AND NEW.upload_date NOT LIKE '____-__-__ __:__:__%')
BEGIN SELECT RAISE(ABORT, 'release_info: timestamp must be canonical UTC'); END;
CREATE TRIGGER IF NOT EXISTS trg_release_info_ts_update
BEFORE UPDATE OF created_at, upload_date ON release_info FOR EACH ROW
WHEN (NEW.created_at IS NOT NULL AND NEW.created_at NOT LIKE '____-__-__ __:__:__%')
  OR (NEW.upload_date IS NOT NULL AND NEW.upload_date NOT LIKE '____-__-__ __:__:__%')
BEGIN SELECT RAISE(ABORT, 'release_info: timestamp must be canonical UTC'); END;

CREATE TRIGGER IF NOT EXISTS trg_automatic_profile_records_ts_insert
BEFORE INSERT ON automatic_profile_records FOR EACH ROW
WHEN (NEW.date_added IS NOT NULL AND NEW.date_added NOT LIKE '____-__-__ __:__:__%')
BEGIN SELECT RAISE(ABORT, 'automatic_profile_records: timestamp must be canonical UTC'); END;
CREATE TRIGGER IF NOT EXISTS trg_automatic_profile_records_ts_update
BEFORE UPDATE OF date_added ON automatic_profile_records FOR EACH ROW
WHEN (NEW.date_added IS NOT NULL AND NEW.date_added NOT LIKE '____-__-__ __:__:__%')
BEGIN SELECT RAISE(ABORT, 'automatic_profile_records: timestamp must be canonical UTC'); END;

CREATE TRIGGER IF NOT EXISTS trg_automatic_profile_media_scans_ts_insert
BEFORE INSERT ON automatic_profile_media_scans FOR EACH ROW
WHEN (NEW.date_added IS NOT NULL AND NEW.date_added NOT LIKE '____-__-__ __:__:__%')
BEGIN SELECT RAISE(ABORT, 'automatic_profile_media_scans: timestamp must be canonical UTC'); END;
CREATE TRIGGER IF NOT EXISTS trg_automatic_profile_media_scans_ts_update
BEFORE UPDATE OF date_added ON automatic_profile_media_scans FOR EACH ROW
WHEN (NEW.date_added IS NOT NULL AND NEW.date_added NOT LIKE '____-__-__ __:__:__%')
BEGIN SELECT RAISE(ABORT, 'automatic_profile_media_scans: timestamp must be canonical UTC'); END;

CREATE TRIGGER IF NOT EXISTS trg_metadata_series_cache_ts_insert
BEFORE INSERT ON metadata_series_cache FOR EACH ROW
WHEN (NEW.fetched_at IS NOT NULL AND NEW.fetched_at NOT LIKE '____-__-__ __:__:__%')
BEGIN SELECT RAISE(ABORT, 'metadata_series_cache: timestamp must be canonical UTC'); END;
CREATE TRIGGER IF NOT EXISTS trg_metadata_series_cache_ts_update
BEFORE UPDATE OF fetched_at ON metadata_series_cache FOR EACH ROW
WHEN (NEW.fetched_at IS NOT NULL AND NEW.fetched_at NOT LIKE '____-__-__ __:__:__%')
BEGIN SELECT RAISE(ABORT, 'metadata_series_cache: timestamp must be canonical UTC'); END;

CREATE TRIGGER IF NOT EXISTS trg_metadata_episodes_cache_ts_insert
BEFORE INSERT ON metadata_episodes_cache FOR EACH ROW
WHEN (NEW.fetched_at IS NOT NULL AND NEW.fetched_at NOT LIKE '____-__-__ __:__:__%')
  OR (NEW.meta_date IS NOT NULL AND NEW.meta_date NOT LIKE '____-__-__ __:__:__%')
BEGIN SELECT RAISE(ABORT, 'metadata_episodes_cache: timestamp must be canonical UTC'); END;
CREATE TRIGGER IF NOT EXISTS trg_metadata_episodes_cache_ts_update
BEFORE UPDATE OF fetched_at, meta_date ON metadata_episodes_cache FOR EACH ROW
WHEN (NEW.fetched_at IS NOT NULL AND NEW.fetched_at NOT LIKE '____-__-__ __:__:__%')
  OR (NEW.meta_date IS NOT NULL AND NEW.meta_date NOT LIKE '____-__-__ __:__:__%')
BEGIN SELECT RAISE(ABORT, 'metadata_episodes_cache: timestamp must be canonical UTC'); END;

CREATE TRIGGER IF NOT EXISTS trg_metadata_season_cache_ts_insert
BEFORE INSERT ON metadata_season_cache FOR EACH ROW
WHEN (NEW.updated_at IS NOT NULL AND NEW.updated_at NOT LIKE '____-__-__ __:__:__%')
BEGIN SELECT RAISE(ABORT, 'metadata_season_cache: timestamp must be canonical UTC'); END;
CREATE TRIGGER IF NOT EXISTS trg_metadata_season_cache_ts_update
BEFORE UPDATE OF updated_at ON metadata_season_cache FOR EACH ROW
WHEN (NEW.updated_at IS NOT NULL AND NEW.updated_at NOT LIKE '____-__-__ __:__:__%')
BEGIN SELECT RAISE(ABORT, 'metadata_season_cache: timestamp must be canonical UTC'); END;

CREATE TRIGGER IF NOT EXISTS trg_metadata_fetch_log_ts_insert
BEFORE INSERT ON metadata_fetch_log FOR EACH ROW
WHEN (NEW.attempted_at IS NOT NULL AND NEW.attempted_at NOT LIKE '____-__-__ __:__:__%')
BEGIN SELECT RAISE(ABORT, 'metadata_fetch_log: timestamp must be canonical UTC'); END;
CREATE TRIGGER IF NOT EXISTS trg_metadata_fetch_log_ts_update
BEFORE UPDATE OF attempted_at ON metadata_fetch_log FOR EACH ROW
WHEN (NEW.attempted_at IS NOT NULL AND NEW.attempted_at NOT LIKE '____-__-__ __:__:__%')
BEGIN SELECT RAISE(ABORT, 'metadata_fetch_log: timestamp must be canonical UTC'); END;

CREATE TRIGGER IF NOT EXISTS trg_api_keys_ts_insert
BEFORE INSERT ON api_keys FOR EACH ROW
WHEN (NEW.created_at IS NOT NULL AND NEW.created_at NOT LIKE '____-__-__ __:__:__%')
  OR (NEW.expires_at IS NOT NULL AND NEW.expires_at NOT LIKE '____-__-__ __:__:__%')
BEGIN SELECT RAISE(ABORT, 'api_keys: timestamp must be canonical UTC'); END;
CREATE TRIGGER IF NOT EXISTS trg_api_keys_ts_update
BEFORE UPDATE OF created_at, expires_at ON api_keys FOR EACH ROW
WHEN (NEW.created_at IS NOT NULL AND NEW.created_at NOT LIKE '____-__-__ __:__:__%')
  OR (NEW.expires_at IS NOT NULL AND NEW.expires_at NOT LIKE '____-__-__ __:__:__%')
BEGIN SELECT RAISE(ABORT, 'api_keys: timestamp must be canonical UTC'); END;

CREATE TRIGGER IF NOT EXISTS trg_calendar_tokens_ts_insert
BEFORE INSERT ON calendar_tokens FOR EACH ROW
WHEN (NEW.created_at IS NOT NULL AND NEW.created_at NOT LIKE '____-__-__ __:__:__%')
BEGIN SELECT RAISE(ABORT, 'calendar_tokens: timestamp must be canonical UTC'); END;
CREATE TRIGGER IF NOT EXISTS trg_calendar_tokens_ts_update
BEFORE UPDATE OF created_at ON calendar_tokens FOR EACH ROW
WHEN (NEW.created_at IS NOT NULL AND NEW.created_at NOT LIKE '____-__-__ __:__:__%')
BEGIN SELECT RAISE(ABORT, 'calendar_tokens: timestamp must be canonical UTC'); END;

CREATE TRIGGER IF NOT EXISTS trg_banned_ips_ts_insert
BEFORE INSERT ON banned_ips FOR EACH ROW
WHEN (NEW.banned_at IS NOT NULL AND NEW.banned_at NOT LIKE '____-__-__ __:__:__%')
  OR (NEW.banned_until IS NOT NULL AND NEW.banned_until NOT LIKE '____-__-__ __:__:__%')
BEGIN SELECT RAISE(ABORT, 'banned_ips: timestamp must be canonical UTC'); END;
CREATE TRIGGER IF NOT EXISTS trg_banned_ips_ts_update
BEFORE UPDATE OF banned_at, banned_until ON banned_ips FOR EACH ROW
WHEN (NEW.banned_at IS NOT NULL AND NEW.banned_at NOT LIKE '____-__-__ __:__:__%')
  OR (NEW.banned_until IS NOT NULL AND NEW.banned_until NOT LIKE '____-__-__ __:__:__%')
BEGIN SELECT RAISE(ABORT, 'banned_ips: timestamp must be canonical UTC'); END;

CREATE TRIGGER IF NOT EXISTS trg_activity_log_ts_insert
BEFORE INSERT ON activity_log FOR EACH ROW
WHEN (NEW.created_at IS NOT NULL AND NEW.created_at NOT LIKE '____-__-__ __:__:__%')
BEGIN SELECT RAISE(ABORT, 'activity_log: timestamp must be canonical UTC'); END;
CREATE TRIGGER IF NOT EXISTS trg_activity_log_ts_update
BEFORE UPDATE OF created_at ON activity_log FOR EACH ROW
WHEN (NEW.created_at IS NOT NULL AND NEW.created_at NOT LIKE '____-__-__ __:__:__%')
BEGIN SELECT RAISE(ABORT, 'activity_log: timestamp must be canonical UTC'); END;

-- ── 3. Canonicalize JSON-embedded timestamps ─────────────────────────────────
-- `series_mappings.data` → `settings.metadata_last_synced_at` (a JSON map).
--
-- `system_state.data` also holds timestamps but stores a generic key/value blob
-- (last-run markers, JSON payloads) with arbitrary keys, so it is read
-- tolerantly via `parse_input_utc` rather than rewritten here.
UPDATE series_mappings
   SET data = json_set(
       data,
       '$.settings.metadata_last_synced_at',
       (
           SELECT json_group_object(
                      je.key,
                      strftime('%Y-%m-%d %H:%M:%S', je.value)
                  )
             FROM json_each(json_extract(data, '$.settings.metadata_last_synced_at')) AS je
       )
   )
 WHERE json_type(data, '$.settings.metadata_last_synced_at') = 'object';
