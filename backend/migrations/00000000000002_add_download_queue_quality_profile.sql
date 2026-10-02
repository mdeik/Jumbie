-- SSoT: the quality profile is snapshotted onto the queue item at queue time
-- (from the series mapping via download_winner / enrich_and_enqueue) so it
-- survives to organize time, where it is written to the episode row.  The
-- mapping can change between queue and completion, so the queue snapshot is
-- the authoritative attribution for this download.
ALTER TABLE download_queue ADD COLUMN quality_profile_id TEXT;
