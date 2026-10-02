-- M3/M4: `episode_files` becomes the single episode↔file relationship; the legacy
-- ownership columns are dropped.
--
-- Re-sync first: code shipped before this migration could only write the legacy
-- columns (`episodes.file_path`, `episode_parts`, `file_paths.episode_id`/`kind`),
-- so `episode_files` (backfilled by migration 22) may be stale.
--
-- Precedence mirrors the old read join exactly (primary file > parts > video link),
-- so no episode changes which file it resolves to:
--   1. `episodes.file_path`        → main (non-part)
--   2. `episode_parts`             → main (part_number)
--   3. video `file_paths.episode_id` links not already covered → main (non-part)
--   4. subtitle/nfo `file_paths.episode_id` links → auxiliary
--
-- Legacy ownership columns carry no foreign key and may reference episodes that
-- no longer exist, so each legacy-sourced insert filters to live episodes before
-- satisfying the `episode_files.episode_id` foreign key.
DELETE FROM episode_files;

INSERT OR IGNORE INTO episode_files (episode_id, file_path_id, kind, part_number)
SELECT e.episode_id, fp.id, 'main', NULL
FROM episodes e
JOIN file_paths fp ON fp.file_path = e.file_path
WHERE e.file_path IS NOT NULL AND e.file_path <> '';

INSERT OR IGNORE INTO episode_files (episode_id, file_path_id, kind, part_number)
SELECT ep.episode_id, fp.id, 'main', ep.part_number
FROM episode_parts ep
JOIN episodes e    ON e.episode_id = ep.episode_id
JOIN file_paths fp ON fp.file_path = ep.file_path;

INSERT OR IGNORE INTO episode_files (episode_id, file_path_id, kind, part_number)
SELECT fp.episode_id, fp.id, 'main', NULL
FROM file_paths fp
WHERE fp.episode_id IS NOT NULL AND fp.episode_id <> ''
  AND fp.kind NOT IN ('subtitle', 'nfo')
  AND EXISTS (SELECT 1 FROM episodes e WHERE e.episode_id = fp.episode_id)
  AND NOT EXISTS (
      SELECT 1 FROM episode_files ef
      WHERE ef.episode_id = fp.episode_id AND ef.kind = 'main'
  );

INSERT OR IGNORE INTO episode_files (episode_id, file_path_id, kind, part_number)
SELECT fp.episode_id, fp.id, 'auxiliary', NULL
FROM file_paths fp
WHERE fp.episode_id IS NOT NULL AND fp.episode_id <> ''
  AND fp.kind IN ('subtitle', 'nfo')
  AND EXISTS (SELECT 1 FROM episodes e WHERE e.episode_id = fp.episode_id);

-- Drop the legacy storage.
DROP INDEX IF EXISTS idx_episodes_file_path;
ALTER TABLE episodes DROP COLUMN file_path;

DROP TABLE episode_parts;

DROP INDEX IF EXISTS idx_file_paths_one_nfo_per_episode;
DROP INDEX IF EXISTS idx_file_paths_episode_id;
ALTER TABLE file_paths DROP COLUMN episode_id;
ALTER TABLE file_paths DROP COLUMN kind;
