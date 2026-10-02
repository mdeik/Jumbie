-- Autoresolve budget now anchors on the LAST attempt (sliding window) and records
-- when the budget was hit, so the reset needs both the window and a short
-- post-exhaustion cooldown to elapse.
ALTER TABLE autoresolve_attempts RENAME COLUMN window_started_at TO last_attempt_at;
ALTER TABLE autoresolve_attempts ADD COLUMN exhausted_at DATETIME;
