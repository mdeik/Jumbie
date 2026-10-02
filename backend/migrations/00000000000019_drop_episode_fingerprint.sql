-- Drop the episode origin anchor.
--
-- `episodes.fingerprint` stored the content fingerprint an episode was first
-- associated with (set-once). It has been superseded: the episode-details
-- `original_path` now resolves from the joined `file_contents` row for the
-- episode's *current* file (`ORIGINAL_PATH_EXPR` = `c.original_path`), so it
-- correctly follows reassignments, renames, and content replacement. The column
-- had become write-only, so remove it and its stamping machinery.
ALTER TABLE episodes DROP COLUMN fingerprint;
