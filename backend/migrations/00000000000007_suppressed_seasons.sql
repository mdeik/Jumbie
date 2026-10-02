-- ── Suppressed Seasons ────────────────────────────────────────────────────
-- A durable "user removed this season" marker.
--
-- WHY a separate table (not a column on `episodes`): a deleted season has its
-- `episodes` rows removed entirely, so there is nothing left to flag. The
-- provider snapshot caches (`metadata_*_cache`) are likewise left intact for
-- `restore_season_metadata`. This table is the tombstone the metadata sync
-- consults so a deleted season is NOT re-created on resync — the user must
-- deliberately restore (or match to provider) to bring it back.
--
-- Scoped by `numbering_mode` (0 = normal, 1 = absolute) because episode IDs
-- differ per mode; suppressing in one mode must not hide the other.
CREATE TABLE IF NOT EXISTS suppressed_seasons (
    series_id      TEXT NOT NULL REFERENCES series_mappings(id) ON DELETE CASCADE,
    season         INTEGER NOT NULL,
    numbering_mode INTEGER NOT NULL DEFAULT 0,
    created_at     TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (series_id, season, numbering_mode)
);

CREATE INDEX IF NOT EXISTS idx_suppressed_seasons_series
    ON suppressed_seasons(series_id, numbering_mode);
