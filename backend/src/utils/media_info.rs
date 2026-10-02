use anyhow::{Context, Result};
use jumbie_shared::types::MediaInfo;
use std::path::Path;
use std::sync::OnceLock;
use tokio::io::AsyncReadExt;

use std::hash::Hasher;
use tracing::{debug, trace};
use xxhash_rust::xxh3::Xxh3;

/// Compute a fast, non-cryptographic xxHash of a file for duplicate detection.
///
/// xxHash is ~10× faster than SHA-256 and we only need bit-identical duplicate
/// detection, not tamper resistance; the ~2⁻⁶⁴ collision probability is acceptable
/// (a false match at worst yields a duplicate library entry, not data loss).
pub async fn calculate_xxhash(path: &Path) -> Result<String> {
    let file_size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    trace!(file_path = %path.display(), file_size, "Calculating xxhash for file");
    let mut file = tokio::fs::File::open(path)
        .await
        .context("Failed to open file for hashing")?;
    let mut hasher = Xxh3::default();
    let mut buffer = [0; 65536]; // Larger buffer for speed

    loop {
        let n = file.read(&mut buffer).await?;
        if n == 0 {
            break;
        }
        hasher.write(&buffer[..n]);
    }

    let digest = hasher.finish();
    Ok(format!("{:x}", digest))
}

/// Extract the `language` field from a stream's `tags` object, if present.
fn extract_stream_language(stream: &serde_json::Value) -> Option<String> {
    stream
        .get("tags")
        .and_then(|t| t.get("language"))
        .and_then(|l| l.as_str())
        .map(|s| s.to_string())
}

/// Return the ffprobe version string (e.g. "ffprobe version 7.1.1") if available;
/// `None` means ffprobe is not installed or couldn't execute.
/// SSoT — all backend paths that need ffprobe must use this.
///
/// Cached in a `OnceLock` — availability is static for the process lifetime.
pub fn ffprobe_version() -> Option<String> {
    static CACHE: OnceLock<Option<String>> = OnceLock::new();
    CACHE
        .get_or_init(|| {
            let output = std::process::Command::new("ffprobe")
                .arg("-version")
                .output()
                .ok()?;
            if !output.status.success() {
                return None;
            }
            let stdout = String::from_utf8_lossy(&output.stdout);
            stdout
                .lines()
                .next()
                .map(|line| line.trim().to_string())
                .filter(|s| !s.is_empty())
        })
        .clone()
}

/// Extract technical metadata from a media file by shelling out to `ffprobe`.
///
/// A subprocess (rather than an `ffmpeg-next`-style binding) tracks ffprobe's own
/// release cadence, supports any format ffprobe does without recompiling, and the
/// 10s timeout prevents a corrupt or network file from hanging the scanner.
pub async fn extract_media_info(path: &Path) -> Result<MediaInfo> {
    trace!(file_path = %path.display(), "Extracting media info via ffprobe");
    let mut cmd = tokio::process::Command::new("ffprobe");
    cmd.arg("-v")
        .arg("quiet")
        .arg("-print_format")
        .arg("json")
        .arg("-show_format")
        .arg("-show_streams")
        .arg("-show_chapters")
        .arg(path);

    let output = tokio::time::timeout(std::time::Duration::from_secs(10), cmd.output())
        .await
        .context("ffprobe timed out after 10 seconds")?
        .context("Failed to run ffprobe")?;

    if !output.status.success() {
        return Ok(MediaInfo {
            codec: None,
            resolution: None,
            bitrate: None,
            duration: None,
            audio: None,
            subtitles: None,
            has_chapters: false,
            audio_track_count: 0,
            subtitle_track_count: 0,
            video_track_count: 0,
            audio_languages: Vec::new(),
            subtitle_languages: Vec::new(),
            video_languages: Vec::new(),
            audio_channels: Vec::new(),
            width: 0,
            height: 0,
        });
    }

    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).context("Failed to parse ffprobe output")?;

    debug!(
        output_len = output.stdout.len(),
        stream_count = json
            .get("streams")
            .and_then(|s| s.as_array())
            .map(|a| a.len())
            .unwrap_or(0),
        "ffprobe completed successfully"
    );

    let mut video_codec = None;
    let mut audio_codec = None;
    let mut subtitle_codec = None;
    let mut width = None;
    let mut height = None;
    let mut duration_sec = 0.0;
    let mut bitrate_bps = 0;
    let mut video_track_count = 0;
    let mut audio_track_count = 0;
    let mut subtitle_track_count = 0;
    let mut video_languages = Vec::new();
    let mut audio_languages = Vec::new();
    let mut subtitle_languages = Vec::new();
    let mut audio_channels = Vec::new();

    if let Some(streams) = json.get("streams").and_then(|s| s.as_array()) {
        for stream in streams {
            let codec_type = stream
                .get("codec_type")
                .and_then(|s| s.as_str())
                .unwrap_or("");

            match codec_type {
                "video" => {
                    video_track_count += 1;
                    if video_codec.is_none() {
                        video_codec = stream
                            .get("codec_name")
                            .and_then(|s| s.as_str())
                            .map(|s| s.to_string());
                        width = stream.get("width").and_then(|v| v.as_i64());
                        height = stream.get("height").and_then(|v| v.as_i64());
                    }
                    if let Some(lang) = extract_stream_language(stream) {
                        video_languages.push(lang);
                    }
                }
                "audio" => {
                    audio_track_count += 1;
                    if audio_codec.is_none() {
                        audio_codec = stream
                            .get("codec_name")
                            .and_then(|s| s.as_str())
                            .map(|s| s.to_string());
                    }
                    if let Some(lang) = extract_stream_language(stream) {
                        audio_languages.push(lang);
                    }
                    if let Some(ch) = stream.get("channels").and_then(|c| c.as_u64()) {
                        audio_channels.push(ch as u32);
                    }
                }
                "subtitle" => {
                    subtitle_track_count += 1;
                    if subtitle_codec.is_none() {
                        subtitle_codec = stream
                            .get("codec_name")
                            .and_then(|s| s.as_str())
                            .map(|s| s.to_string());
                    }
                    if let Some(lang) = extract_stream_language(stream) {
                        subtitle_languages.push(lang);
                    }
                }
                _ => {}
            }
        }
    }

    let has_chapters = json
        .get("chapters")
        .and_then(|c| c.as_array())
        .map(|a| !a.is_empty())
        .unwrap_or(false);

    if let Some(format) = json.get("format") {
        if let Some(d) = format.get("duration").and_then(|v| v.as_str()) {
            duration_sec = d.parse().unwrap_or(0.0);
        }
        if let Some(b) = format.get("bit_rate").and_then(|v| v.as_str()) {
            bitrate_bps = b.parse().unwrap_or(0);
        }
    }

    let duration = if duration_sec > 0.0 {
        let mins = (duration_sec / 60.0) as u64;
        let secs = (duration_sec % 60.0) as u64;
        Some(format!("{}m {}s", mins, secs))
    } else {
        None
    };

    let resolution = if let (Some(w), Some(h)) = (width, height) {
        Some(format!("{}x{}", w, h))
    } else {
        None
    };

    let bitrate = if bitrate_bps > 0 {
        Some(format!("{} kb/s", bitrate_bps / 1000))
    } else {
        None
    };

    let codec = video_codec;
    let audio = match audio_track_count {
        0 => None,
        1 => audio_codec,
        n => Some(format!("{} tracks", n)),
    };
    let subtitles = match subtitle_track_count {
        0 => None,
        1 => subtitle_codec,
        n => Some(format!("{} tracks", n)),
    };

    Ok(MediaInfo {
        codec,
        resolution,
        bitrate,
        duration,
        audio,
        subtitles,
        has_chapters,
        audio_track_count,
        subtitle_track_count,
        video_track_count,
        audio_languages,
        subtitle_languages,
        video_languages,
        audio_channels,
        width: width.unwrap_or(0) as u32,
        height: height.unwrap_or(0) as u32,
    })
}
