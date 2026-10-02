-- Origin anchor: the content fingerprint an episode was first associated with.
-- Set-once (see `stamp_origin_*` in db/fingerprints.rs) so the origin path
-- survives unassign, move, rename, and content replacement. Resolves to
-- `file_contents.original_path`, which is itself first-write-wins.
ALTER TABLE episodes ADD COLUMN fingerprint TEXT;

-- Backfill from the current file, or the first part for part-only episodes.
UPDATE episodes
   SET fingerprint = (SELECT fp.fingerprint FROM file_paths fp WHERE fp.file_path = episodes.file_path)
 WHERE fingerprint IS NULL AND file_path IS NOT NULL;

UPDATE episodes
   SET fingerprint = (
       SELECT fp.fingerprint
         FROM episode_parts ep
         JOIN file_paths fp ON fp.file_path = ep.file_path
        WHERE ep.episode_id = episodes.episode_id
        ORDER BY ep.part_number
        LIMIT 1)
 WHERE fingerprint IS NULL
   AND EXISTS (SELECT 1 FROM episode_parts ep WHERE ep.episode_id = episodes.episode_id);
