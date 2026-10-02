-- Built-in plugins now declare the "Jumbie" author and are namespaced under
-- `jumbie.` instead of the legacy `internal.` prefix. Rename persisted plugin
-- type ids so existing user configuration keeps resolving to the same built-in
-- plugin (config lookup is keyed by the derived type id).
UPDATE plugin_instances
SET plugin_id = 'jumbie.' || substr(plugin_id, length('internal.') + 1)
WHERE plugin_id LIKE 'internal.%';
