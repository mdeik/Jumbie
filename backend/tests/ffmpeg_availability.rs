//! Guard test: the media-info extraction tests degrade gracefully (skip or
//! assert `None`) when ffmpeg/ffprobe is absent, so a missing ffmpeg is
//! otherwise silent. ffmpeg is optional for the app itself, but these tests
//! need it to actually exercise extraction.

mod common;

#[test]
fn test_ffmpeg_available_for_media_info_tests() {
    assert!(
        common::ffprobe_available(),
        "ffmpeg/ffprobe not found on PATH at build time (checked `ffmpeg -version`). \
         Media-info tests (api_scan_queue, content_hash_dedup, api_series_extended) are \
         running DEGRADED — extraction is skipped or asserted None instead of exercised. \
         ffmpeg is optional for the app, but these tests need it: install ffmpeg and \
         rebuild to run them."
    );
}
