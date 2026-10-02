-- Drop the series-level (mapping-wide) episode range.
--
-- `series_mappings.episode_start` / `episode_end` gated monitoring and scoped
-- auto-search for every season at once. It has been superseded by the per-season
-- `season_overrides.episode_start` / `episode_end` (with `cell_count`), which is
-- the single source of truth for a season's episode range and expected cell count.
-- The columns were never set by the API or frontend, so they were always NULL.
--
-- Drop the sync triggers and index first: SQLite refuses to drop a column that a
-- trigger or index still references.
DROP TRIGGER IF EXISTS trg_series_mappings_episode_start_insert;
DROP TRIGGER IF EXISTS trg_series_mappings_episode_start_update;
DROP TRIGGER IF EXISTS trg_series_mappings_episode_end_insert;
DROP TRIGGER IF EXISTS trg_series_mappings_episode_end_update;
DROP INDEX IF EXISTS idx_series_mappings_episode_range;
ALTER TABLE series_mappings DROP COLUMN episode_start;
ALTER TABLE series_mappings DROP COLUMN episode_end;
