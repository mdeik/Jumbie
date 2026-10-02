-- Drop `episode_parts.media_info`.
--
-- Part media info is content-level data: it belongs to the file's fingerprint and
-- lives in `file_contents.media_info`, shared by every copy/part with the same
-- content. `episode_parts` now stores only the association (episode_id,
-- part_number, file_path, size, fingerprint); readers join `file_contents` by
-- fingerprint. The column was a denormalized copy written from the same ffprobe
-- result, so it is removed to keep media info single-sourced.
ALTER TABLE episode_parts DROP COLUMN media_info;
