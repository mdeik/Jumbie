// User-facing audit events (search history, downloads, deletions); prunable
// independently of the media pipeline.
pub mod activity;

// Processing artifacts (ReleaseCandidate, MediaEntry) — not DB row types. They are
// marshalled across the WebSocket API and stored in the download queue, so they must
// stay stable as the DB schema evolves.
pub mod media;
