-- ── Kept-for-review ("unmatched") files ───────────────────────────────────
-- Files deliberately left in the download directory for manual review (pack
-- overspill that was kept, or files that could not be assigned). They have no
-- `episodes` row, so this table is their durable record.
--
-- WHY a dedicated table instead of a `file_fingerprints` sentinel: fingerprints
-- are a *scan cache* with their own pruning (`cleanup_missing_files` deletes rows
-- whose `episode_id` doesn't resolve), which would silently drop kept files from
-- the file browser. This table is the SSoT for the review list and carries the
-- series association needed to scope it per series (the download path itself is a
-- random UUID folder and does not encode the series).
--
-- `missing_since` tracks when the file was first seen missing so a row is only
-- removed after a grace period, mirroring the fingerprint stale-file cleanup.
CREATE TABLE IF NOT EXISTS unmatched_files (
    file_path     TEXT PRIMARY KEY,
    series_id     TEXT REFERENCES series_mappings(id) ON DELETE CASCADE,
    reason        TEXT,
    created_at    TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    last_seen_at  TIMESTAMP,
    missing_since TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_unmatched_files_series_id
    ON unmatched_files(series_id);
