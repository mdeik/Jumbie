-- Autoresolve attempt budget, keyed by (series, season, episode).
--
-- Deliberately separate from `rejected_downloads`: the two have different
-- lifetimes. A rejection must persist for the exclusion window (48h) so
-- auto-search keeps skipping that release, while the attempt budget uses a
-- shorter window and is cleared as soon as a download for the episode succeeds.
CREATE TABLE IF NOT EXISTS autoresolve_attempts (
    series_id         TEXT NOT NULL,
    season            INTEGER NOT NULL,
    episode           INTEGER NOT NULL,
    attempts          INTEGER NOT NULL DEFAULT 0,
    window_started_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (series_id, season, episode)
);

CREATE INDEX IF NOT EXISTS idx_autoresolve_attempts_window
    ON autoresolve_attempts(window_started_at);
