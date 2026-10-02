-- File kind on every tracked path: an episode's playable `video`, or an
-- auxiliary sidecar (`subtitle`, `nfo`). Auxiliary rows are linked to their
-- episode via `episode_id` but never become `episodes.file_path`.
ALTER TABLE file_paths ADD COLUMN kind TEXT NOT NULL DEFAULT 'video';

-- At most one nfo per episode. Partial index so unlinked (orphan) nfo rows are
-- unconstrained; a video/subtitle is unaffected.
CREATE UNIQUE INDEX IF NOT EXISTS idx_file_paths_one_nfo_per_episode
    ON file_paths(episode_id) WHERE kind = 'nfo' AND episode_id IS NOT NULL;
