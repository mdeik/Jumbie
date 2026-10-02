//! Media file extension definitions and validation.
//!
//! These lists deliberately omit container formats that are uncommon or obsolete in the
//! series release ecosystem (e.g., `.flv`, `.m4v`). Including them would introduce noise
//! with no practical benefit — media from organized sources (torrents, usenet) almost
//! exclusively uses the extensions listed below.

/// Supported video container formats.
///
/// `iso` is not a playable container but a valid remuxing input; `ts` covers
/// broadcast/DVR captures. `.flv` and `.m4v` are deliberately excluded (obsolete
/// or structurally identical to `.mp4`).
pub const VALID_VIDEO_EXTS: &[&str] = &["mkv", "mp4", "avi", "mov", "iso", "wmv", "webm", "ts"];

/// Default video extension used as a fallback when the source file has no
/// discernible extension (e.g., during template-based naming).
///
/// Derived from `VALID_VIDEO_EXTS` so that changes to the supported format list
/// automatically propagate here.
pub const DEFAULT_VIDEO_EXT: &str = VALID_VIDEO_EXTS[0];

/// Supported subtitle container formats (`sub`/`idx` are the VobSub pair).
pub const VALID_SUBTITLE_EXTS: &[&str] = &["srt", "ass", "vtt", "sub", "idx"];

/// File extensions commonly found in torrent downloads that are intentionally
/// ignored during unexpected-file classification. These are small metadata,
/// cover art, or OS-generated files that should never trigger an automatic
/// profile offense.
///
/// Users can override which extensions get flagged via the `file_patterns`
/// field on the `UnexpectedFiles` automatic profile rule.
pub const IGNORED_UNKNOWN_EXTS: &[&str] = &["nfo", "jpg", "jpeg", "png", "db", "ini"];

/// Returns `true` if `ext` is a known video container extension.
///
/// Comparison is case-insensitive because file systems and release naming
/// conventions vary (e.g., ".MKV" vs ".mkv").
pub fn is_video_ext(ext: &str) -> bool {
    VALID_VIDEO_EXTS.contains(&ext.to_lowercase().as_str())
}

/// Returns `true` if `ext` is a known subtitle extension.
///
/// Comparison is case-insensitive for the same reason as `is_video_ext`.
pub fn is_subtitle_ext(ext: &str) -> bool {
    VALID_SUBTITLE_EXTS.contains(&ext.to_lowercase().as_str())
}

/// Returns `true` if `ext` is a known video or subtitle extension.
pub fn is_valid_media_ext(ext: &str) -> bool {
    is_video_ext(ext) || is_subtitle_ext(ext)
}

/// Classification of a tracked file. `Video` is an episode's playable file;
/// `Subtitle` and `Nfo` are auxiliary sidecars attached to the episode.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum FileKind {
    #[default]
    Video,
    Subtitle,
    Nfo,
}

impl FileKind {
    /// True for sidecars that attach to an episode without being its playable file.
    pub fn is_auxiliary(self) -> bool {
        matches!(self, FileKind::Subtitle | FileKind::Nfo)
    }
}

/// Returns `true` if `ext` is an `.nfo` metadata sidecar.
pub fn is_nfo_ext(ext: &str) -> bool {
    ext.eq_ignore_ascii_case("nfo")
}

/// Returns `true` if `ext` belongs to a sidecar kind (subtitle or nfo).
pub fn is_auxiliary_ext(ext: &str) -> bool {
    is_subtitle_ext(ext) || is_nfo_ext(ext)
}

/// Classify a file extension. `None` for anything that is not video, subtitle,
/// or nfo. SSoT for the video/auxiliary split across the scanner, download
/// pipeline, and rename queue.
pub fn file_kind_for_ext(ext: &str) -> Option<FileKind> {
    if is_video_ext(ext) {
        Some(FileKind::Video)
    } else if is_subtitle_ext(ext) {
        Some(FileKind::Subtitle)
    } else if is_nfo_ext(ext) {
        Some(FileKind::Nfo)
    } else {
        None
    }
}

/// Classify a path by its extension. `None` when the path has no extension or
/// an unrecognized one.
pub fn file_kind_for_path(path: &std::path::Path) -> Option<FileKind> {
    path.extension()
        .and_then(|e| e.to_str())
        .and_then(file_kind_for_ext)
}

/// True when `path` is an auxiliary sidecar (subtitle/nfo). SSoT for the
/// `file_kind_for_path(..).map(FileKind::is_auxiliary)` pattern used by the
/// scanner, the organizer, and the assignment endpoints. `false` for videos and
/// unrecognized/absent extensions.
pub fn is_auxiliary_path(path: &std::path::Path) -> bool {
    file_kind_for_path(path)
        .map(FileKind::is_auxiliary)
        .unwrap_or(false)
}

/// Returns `true` if `ext` is a file type that should be silently ignored
/// during unexpected-file classification (metadata, cover art, OS files).
///
/// Comparison is case-insensitive.
pub fn is_ignored_unknown_ext(ext: &str) -> bool {
    IGNORED_UNKNOWN_EXTS.contains(&ext.to_lowercase().as_str())
}
