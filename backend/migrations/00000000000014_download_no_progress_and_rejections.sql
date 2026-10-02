-- No-progress detection for the download queue.

-- Last sampled progress fraction. NULL until the first sample after dispatch.
ALTER TABLE download_queue ADD COLUMN last_progress REAL;

-- When progress last advanced. NULL until the first sample. A Downloading item
-- whose (now - no_progress_since) exceeds the configured threshold is stalled.
ALTER TABLE download_queue ADD COLUMN no_progress_since DATETIME;

-- Releases rejected after a stalled download was removed. Auto-search skips these
-- until expires_at, so a re-search picks a different release instead of the
-- highest-scored one that already stalled. Also bounds autoresolve churn: the
-- count of active rows for a target is the number of alternatives already tried.
CREATE TABLE IF NOT EXISTS rejected_downloads (
    media_link  TEXT PRIMARY KEY,
    download_id TEXT,
    series_id   TEXT,
    season      INTEGER,
    episode     INTEGER,
    reason      TEXT NOT NULL,
    created_at  DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    expires_at  DATETIME NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_rejected_downloads_expires_at ON rejected_downloads(expires_at);
CREATE INDEX IF NOT EXISTS idx_rejected_downloads_target ON rejected_downloads(series_id, season, episode);
