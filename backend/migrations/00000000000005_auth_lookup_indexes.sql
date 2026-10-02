-- ── Auth lookup indexes ─────────────────────────────────────────────────────
--
-- The auth middleware resolves a Bearer token to a single API key by its stored
-- hash on every request. This index makes that lookup O(log n) instead of a full
-- table scan, and lets the middleware avoid loading every key into memory.

CREATE INDEX IF NOT EXISTS idx_api_keys_key_hash ON api_keys(key_hash);
