-- ── Metadata cache: provider_id → plugin_id ───────────────────────────────
-- The cache key was `(metadata_id, provider_id, instance_id)`, but every writer
-- stored `provider_id` = `instance_id` (a duplicate), while the clear-cache
-- endpoint passed a *type short name* ("tvdb") — so per-provider clears deleted
-- nothing. `plugin_id` now carries the backend-stamped TYPE id (e.g.
-- "jumbie.tvdb"), the same value as `PluginInstanceInfo.plugin_id`, giving
-- instance isolation (instance_id) plus provider-type keying (plugin_id).
--
-- Existing rows hold instance ids in the old column, which would never match the
-- new key, so the caches are cleared: they are disposable snapshots rebuilt on
-- the next fetch (and the fetch-log just gates retry cadence).
ALTER TABLE metadata_series_cache RENAME COLUMN provider_id TO plugin_id;
ALTER TABLE metadata_episodes_cache RENAME COLUMN provider_id TO plugin_id;
ALTER TABLE metadata_season_cache RENAME COLUMN provider_id TO plugin_id;
ALTER TABLE metadata_fetch_log RENAME COLUMN provider_id TO plugin_id;

DELETE FROM metadata_series_cache;
DELETE FROM metadata_episodes_cache;
DELETE FROM metadata_season_cache;
DELETE FROM metadata_fetch_log;
