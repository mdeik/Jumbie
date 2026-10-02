-- ── Blocked files ─────────────────────────────────────────────────────────
-- A durable "do not auto-adopt this file" marker, created when the user
-- unassigns a file because it is wrong. The scanner's filename-derived
-- discovery consults it so a rescan cannot resurrect the association; an
-- explicit (re)assignment clears it, and a stale-entry sweep expires it.
--
-- WHY name + size/hash (not path): renaming a file to claim a different episode
-- is a deliberate act and must unblock it, so the *name* is part of the key.
-- The size/hash discriminates "the same file" from a newly-downloaded file that
-- happens to share the name — a differing size (or hash, when known) is treated
-- as a new file and is allowed back in.
--
-- Scoped per series (a copy into another series is unaffected). `last_seen_at`
-- is bumped whenever a scan still finds a matching file, so entries for files
-- that are gone (deleted/renamed/replaced) expire after a grace period.
CREATE TABLE IF NOT EXISTS blocked_files (
    series_id     TEXT NOT NULL REFERENCES series_mappings(id) ON DELETE CASCADE,
    file_name     TEXT NOT NULL,
    size          INTEGER NOT NULL,
    quick_hash    TEXT,
    created_at    TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    last_seen_at  TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (series_id, file_name, size)
);

CREATE INDEX IF NOT EXISTS idx_blocked_files_series
    ON blocked_files(series_id);
