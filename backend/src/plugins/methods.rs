//! SSoT for plugin method name strings.
//!
//! Every plugin method name used in `handle_custom_method()` dispatch or
//! `PluginInstance::call()` call sites should be defined here. This prevents
//! silent mismatches caused by typos in string literals.

// Standard methods (handled by PluginInstance::call() default impl)
pub const METHOD_GET_INFO: &str = "get_info";
pub const METHOD_HEALTH_CHECK: &str = "health_check";
pub const METHOD_TEST: &str = "test";

// Source plugin methods
pub const METHOD_SEARCH: &str = "search";
pub const METHOD_FETCH_ENTRIES: &str = "fetch_entries";
pub const METHOD_AUTO_SEARCH: &str = "auto_search";

// Metadata plugin methods
pub const METHOD_FETCH_SERIES_METADATA: &str = "fetch_series_metadata";
pub const METHOD_FETCH_SERIES_INFO: &str = "fetch_series_info";
pub const METHOD_FETCH_SERIES_ALIASES: &str = "fetch_series_aliases";
pub const METHOD_GET_UPDATED_SERIES: &str = "get_updated_series";
pub const METHOD_USES_ABSOLUTE_NUMBERING: &str = "uses_absolute_episode_numbering";

// Downloader plugin methods
pub const METHOD_ADD_DOWNLOAD: &str = "add_download";
pub const METHOD_GET_COMPLETED_DOWNLOADS: &str = "get_completed_downloads";
pub const METHOD_GET_DOWNLOAD_PROGRESS: &str = "get_download_progress";
pub const METHOD_GET_DOWNLOAD_STATUS: &str = "get_download_status";
/// Optional: a download client reports a hard failure (with reason) for a
/// download that cannot proceed. Returning null means "no failure".
pub const METHOD_GET_DOWNLOAD_FAILURE: &str = "get_download_failure";
pub const METHOD_GET_DOWNLOAD_CONTENT_PATH: &str = "get_download_content_path";
pub const METHOD_PAUSE_DOWNLOAD: &str = "pause_download";
pub const METHOD_RESUME_DOWNLOAD: &str = "resume_download";
pub const METHOD_DELETE_DOWNLOAD: &str = "delete_download";
pub const METHOD_GET_DOWNLOAD_ID_BY_NAME: &str = "get_download_id_by_name";
pub const METHOD_RETRY: &str = "retry";
pub const METHOD_COMPLETE_DOWNLOAD: &str = "complete_download";
pub const METHOD_GET_DOWNLOAD_PATH: &str = "get_download_path";
pub const METHOD_GET_ORGANIZER_PATH: &str = "get_organizer_path";
pub const METHOD_TEST_CONNECTION: &str = "test_connection";

// Notifier plugin methods
pub const METHOD_NOTIFY: &str = "notify";
pub const METHOD_ON_TEST: &str = "on_test";
