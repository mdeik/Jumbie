-- Download queue origin: distinguishes a release the user explicitly picked
-- (is_manual = 1) from one the system selected by score. Manual items show a
-- `Manual` badge and are exempt from no-progress autoresolve.
ALTER TABLE download_queue ADD COLUMN is_manual BOOLEAN NOT NULL DEFAULT 0;
