-- M2: unified episode↔file association.
--
-- `file_paths` gains a stable surrogate `id` (the pathname stays a UNIQUE column).
-- Associations reference the id, so a move updates `file_paths.file_path` once and
-- every `episode_files` row follows without being rewritten.
--
-- `episode_files` is the single relationship for every shape:
--   single file        -> (kind='main', part_number NULL)
--   multipart          -> (kind='main', part_number 1..N)
--   multi-episode file -> one row per covered episode (kind='main', NULL)
--   auxiliary sidecar  -> (kind='auxiliary', NULL) — episode-scoped, never part-scoped
--
-- This migration is additive: legacy storage is retained for dual-write during the
-- cutover (dropped in a later migration once every reader is converted).

-- ── 1. file_paths: add a stable surrogate id ─────────────────────────────────
CREATE TABLE file_paths_new (
    id            INTEGER PRIMARY KEY,
    file_path     TEXT    NOT NULL UNIQUE,
    fingerprint   TEXT    NOT NULL,
    inode         INTEGER,
    device        INTEGER,
    size          INTEGER,
    mtime         REAL,
    state         TEXT DEFAULT 'pending',
    expected_path TEXT,
    episode_id    TEXT,
    kind          TEXT    NOT NULL DEFAULT 'video',
    last_seen_at  TIMESTAMP
);

INSERT INTO file_paths_new
    (id, file_path, fingerprint, inode, device, size, mtime, state, expected_path, episode_id, kind, last_seen_at)
SELECT rowid, file_path, fingerprint, inode, device, size, mtime, state, expected_path, episode_id, kind, last_seen_at
FROM file_paths;

DROP INDEX IF EXISTS idx_file_paths_one_nfo_per_episode;
DROP TABLE file_paths;
ALTER TABLE file_paths_new RENAME TO file_paths;

CREATE INDEX IF NOT EXISTS idx_file_paths_fingerprint ON file_paths(fingerprint);
CREATE INDEX IF NOT EXISTS idx_file_paths_episode_id  ON file_paths(episode_id);
CREATE INDEX IF NOT EXISTS idx_file_paths_state       ON file_paths(state);

-- Preserve the "at most one nfo per episode" constraint (legacy; removed with the
-- `kind`/`episode_id` columns once the association table is authoritative).
CREATE UNIQUE INDEX IF NOT EXISTS idx_file_paths_one_nfo_per_episode
    ON file_paths(episode_id) WHERE kind = 'nfo' AND episode_id IS NOT NULL;

-- ── 2. episode_files: the single association ─────────────────────────────────
CREATE TABLE episode_files (
    episode_id   TEXT    NOT NULL REFERENCES episodes(episode_id) ON DELETE CASCADE,
    file_path_id INTEGER NOT NULL REFERENCES file_paths(id)       ON DELETE CASCADE,
    kind         TEXT    NOT NULL,
    part_number  INTEGER,
    PRIMARY KEY (episode_id, file_path_id)
);

CREATE INDEX IF NOT EXISTS ix_episode_files_file ON episode_files(file_path_id);

-- At most one non-part main file per episode.
CREATE UNIQUE INDEX IF NOT EXISTS ux_episode_files_main_single
    ON episode_files(episode_id) WHERE kind = 'main' AND part_number IS NULL;

-- At most one file per multipart slot.
CREATE UNIQUE INDEX IF NOT EXISTS ux_episode_files_main_part
    ON episode_files(episode_id, part_number) WHERE kind = 'main' AND part_number IS NOT NULL;

CREATE INDEX IF NOT EXISTS ix_episode_files_aux
    ON episode_files(episode_id) WHERE kind = 'auxiliary';

-- ── 3. Backfill from legacy storage ──────────────────────────────────────────
-- Legacy ownership columns (`episodes.file_path`, `episode_parts.episode_id`,
-- `file_paths.episode_id`) carry no foreign key, so they can hold *dangling*
-- episode ids (e.g. a sidecar left behind after the episode was renumbered or
-- deleted). `episode_files.episode_id` DOES reference `episodes`, so every
-- backfill must filter to episodes that still exist — `INSERT OR IGNORE` only
-- suppresses uniqueness conflicts, not foreign-key violations.

-- Single + multi-episode files (episode side already joined, so always valid).
INSERT OR IGNORE INTO episode_files (episode_id, file_path_id, kind, part_number)
SELECT e.episode_id, fp.id, 'main', NULL
FROM episodes e
JOIN file_paths fp ON fp.file_path = e.file_path
WHERE e.file_path IS NOT NULL AND e.file_path <> '';

-- Multipart files.
INSERT OR IGNORE INTO episode_files (episode_id, file_path_id, kind, part_number)
SELECT ep.episode_id, fp.id, 'main', ep.part_number
FROM episode_parts ep
JOIN episodes e       ON e.episode_id = ep.episode_id
JOIN file_paths fp    ON fp.file_path = ep.file_path;

-- Auxiliary sidecars (episode-scoped).
INSERT OR IGNORE INTO episode_files (episode_id, file_path_id, kind, part_number)
SELECT fp.episode_id, fp.id, 'auxiliary', NULL
FROM file_paths fp
WHERE fp.episode_id IS NOT NULL AND fp.episode_id <> ''
  AND fp.kind IN ('subtitle', 'nfo')
  AND EXISTS (SELECT 1 FROM episodes e WHERE e.episode_id = fp.episode_id);
