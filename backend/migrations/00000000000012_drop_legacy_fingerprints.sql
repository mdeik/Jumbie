-- Fold the legacy path-keyed `file_fingerprints` into the content-keyed
-- `file_contents`/`file_paths`, then drop it. The backfill is idempotent
-- (INSERT OR IGNORE); a content's origin is the earliest row seen for it.
INSERT OR IGNORE INTO file_contents
    (fingerprint, media_info, media_info_scan_failed, original_path, first_seen_at, last_seen_at)
SELECT
    CASE WHEN quick_hash IS NULL OR quick_hash = ''
         THEN 'path:' || file_path ELSE quick_hash END,
    MAX(media_info),
    COALESCE(MAX(media_info_scan_failed), 0),
    substr(MIN(printf('%012d', id) || file_path), 13),
    MIN(last_seen_at),
    MAX(last_seen_at)
FROM file_fingerprints
GROUP BY CASE WHEN quick_hash IS NULL OR quick_hash = ''
              THEN 'path:' || file_path ELSE quick_hash END;

INSERT OR IGNORE INTO file_paths
    (file_path, fingerprint, inode, device, size, mtime, state, expected_path, episode_id, last_seen_at)
SELECT
    file_path,
    CASE WHEN quick_hash IS NULL OR quick_hash = ''
         THEN 'path:' || file_path ELSE quick_hash END,
    inode, device, size, mtime, state, expected_path, episode_id, last_seen_at
FROM file_fingerprints;

DROP TABLE IF EXISTS file_fingerprints;
