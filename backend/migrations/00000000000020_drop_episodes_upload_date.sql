-- Drop `episodes.upload_date`.
--
-- `upload_date` is the *file's* date, not the episode's: it is the current file's
-- source-feed publish date, or (for scanned files) an mtime anchor. Episode-level
-- dates are `meta_date` and `est_date`, which stay on the row.
--
-- The file's date already has a content-keyed SSoT: `release_info.upload_date`,
-- keyed by content hash. Readers resolve it through
-- `file_paths.fingerprint = release_info.quick_hash`, and the scanner records the
-- mtime fallback there too (only when no source-feed date exists). The episode
-- column was a denormalized mirror that drifted, so it is removed.
--
-- SQLite refuses to DROP a column referenced by a trigger, so the canonical-UTC
-- triggers are recreated without it first.
DROP TRIGGER IF EXISTS trg_episodes_ts_insert;
DROP TRIGGER IF EXISTS trg_episodes_ts_update;

ALTER TABLE episodes DROP COLUMN upload_date;

CREATE TRIGGER IF NOT EXISTS trg_episodes_ts_insert
BEFORE INSERT ON episodes FOR EACH ROW
WHEN (NEW.created_at IS NOT NULL AND NEW.created_at NOT LIKE '____-__-__ __:__:__%')
  OR (NEW.file_acquired_at IS NOT NULL AND NEW.file_acquired_at NOT LIKE '____-__-__ __:__:__%')
  OR (NEW.meta_date IS NOT NULL AND NEW.meta_date NOT LIKE '____-__-__ __:__:__%')
  OR (NEW.est_date IS NOT NULL AND NEW.est_date NOT LIKE '____-__-__ __:__:__%')
BEGIN SELECT RAISE(ABORT, 'episodes: timestamp must be canonical UTC (YYYY-MM-DD HH:MM:SS)'); END;
CREATE TRIGGER IF NOT EXISTS trg_episodes_ts_update
BEFORE UPDATE OF created_at, file_acquired_at, meta_date, est_date ON episodes FOR EACH ROW
WHEN (NEW.created_at IS NOT NULL AND NEW.created_at NOT LIKE '____-__-__ __:__:__%')
  OR (NEW.file_acquired_at IS NOT NULL AND NEW.file_acquired_at NOT LIKE '____-__-__ __:__:__%')
  OR (NEW.meta_date IS NOT NULL AND NEW.meta_date NOT LIKE '____-__-__ __:__:__%')
  OR (NEW.est_date IS NOT NULL AND NEW.est_date NOT LIKE '____-__-__ __:__:__%')
BEGIN SELECT RAISE(ABORT, 'episodes: timestamp must be canonical UTC (YYYY-MM-DD HH:MM:SS)'); END;
