-- ── Content-keyed fingerprints + per-path instances ───────────────────────
-- `file_fingerprints` was keyed by `file_path`, so content metadata (media_info)
-- was duplicated per path and a rename/copy created a second, unrelated row.
-- This splits it into:
--   * `file_contents` — SSoT for a piece of content, keyed by `fingerprint`
--                       (xxhash, or the identity-fallback hash).
--   * `file_paths`    — one row per path seen for that content.
-- `file_contents.original_path` is the first path ever seen for the content and
-- is never rewritten, so it survives unassign, move, rename, copy, and
-- reassignment. It is intentionally not cleared anywhere.
CREATE TABLE IF NOT EXISTS file_contents (
    fingerprint            TEXT PRIMARY KEY,
    media_info             TEXT,
    media_info_scan_failed INTEGER NOT NULL DEFAULT 0,
    original_path          TEXT,
    first_seen_at          TIMESTAMP,
    last_seen_at           TIMESTAMP
);

CREATE TABLE IF NOT EXISTS file_paths (
    file_path     TEXT PRIMARY KEY,
    fingerprint   TEXT NOT NULL,
    inode         INTEGER,
    device        INTEGER,
    size          INTEGER,
    mtime         REAL,
    state         TEXT DEFAULT 'pending',
    expected_path TEXT,
    episode_id    TEXT,
    last_seen_at  TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_file_paths_fingerprint   ON file_paths(fingerprint);
CREATE INDEX IF NOT EXISTS idx_file_paths_episode_id    ON file_paths(episode_id);
CREATE INDEX IF NOT EXISTS idx_file_paths_state         ON file_paths(state);
CREATE INDEX IF NOT EXISTS idx_file_contents_last_seen  ON file_contents(last_seen_at);
