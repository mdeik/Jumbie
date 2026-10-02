// Platform-abstracted file identity and file removal.
//
// Unix exposes a stable (inode, device) pair that survives renames within the
// same filesystem. Windows' equivalent (FileId + VolumeSerialNumber) is
// nightly-only, so we synthesise a pseudo-inode as `creation_time XOR
// last_write_time`: stable across renames (creation_time never changes) and
// changes on replacement (last_write_time does), though two distinct files can
// collide. `dev_equiv` is always 0 on Windows, which is harmless since it is
// only a DB column and fallback hash input.

use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::time::Duration;

/// Returns `(inode_equiv, dev_equiv, mtime_secs)` for a file's metadata.
///
/// | Platform | inode_equiv                          | dev_equiv | mtime_secs          |
/// |----------|--------------------------------------|-----------|---------------------|
/// | Unix     | real inode number                    | real dev  | real mtime (f64 s)  |
/// | Windows  | creation_time XOR last_write_time    | 0         | last_write_time / 1e7 (f64 s) |
pub fn file_identity(meta: &std::fs::Metadata) -> (u64, u64, f64) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let inode = meta.ino();
        let dev = meta.dev();
        let mtime = meta.mtime() as f64;
        (inode, dev, mtime)
    }

    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        let creation = meta.creation_time();
        let write = meta.last_write_time();
        let inode_equiv = creation ^ write;
        // last_write_time is in 100-ns intervals since 1601-01-01.
        let mtime = write as f64 / 10_000_000.0;
        (inode_equiv, 0u64, mtime)
    }

    #[cfg(not(any(unix, windows)))]
    {
        // Exotic targets (e.g. wasm32) never reach this backend-only path.
        let _ = meta;
        (0u64, 0u64, 0.0f64)
    }
}

/// Returns `true` when the IO error indicates a cross-device (rename across
/// different filesystems) failure — the signal that a caller should fall back
/// from `rename` to a copy+delete. SSoT for the platform-specific errno; callers
/// must not compare `raw_os_error()` themselves.
///
/// | Platform | errno / OS error  |
/// |----------|-------------------|
/// | Linux    | 18 (EXDEV)        |
/// | macOS    | 18 (EXDEV)        |
/// | Windows  | 17 (ERROR_NOT_SAME_DEVICE) |
pub fn is_cross_device_error(e: &std::io::Error) -> bool {
    #[cfg(unix)]
    {
        e.raw_os_error() == Some(18) // EXDEV
    }

    #[cfg(windows)]
    {
        e.raw_os_error() == Some(17) // ERROR_NOT_SAME_DEVICE
    }

    #[cfg(not(any(unix, windows)))]
    {
        let msg = e.to_string().to_lowercase();
        msg.contains("cross-device") || msg.contains("crosses devices")
    }
}

/// Remove a file, retrying while another handle holds it open.
///
/// Unix `unlink` succeeds even while the file is open, so the first attempt wins.
/// Windows refuses with a sharing violation while any process has the file open
/// without `FILE_SHARE_DELETE` — e.g. a concurrent ffprobe media-info scan. The
/// holder is expected to be short-lived (the background scan releases the file as
/// soon as ffprobe returns), so a bounded retry converts the race into a wait.
/// Best-effort: the caller decides how to report an ultimate failure.
pub async fn remove_file_tolerating_locks(path: &Path) -> std::io::Result<()> {
    remove_tolerating_locks(path, |p| Box::pin(tokio::fs::remove_file(p))).await
}

/// Remove a directory tree, retrying while another handle holds something inside
/// it open (same policy as [`remove_file_tolerating_locks`]). Used to clean up a
/// source directory after a cross-device copy, where a concurrent scan may still
/// hold a file in the tree on Windows.
pub async fn remove_dir_all_tolerating_locks(path: &Path) -> std::io::Result<()> {
    remove_tolerating_locks(path, |p| Box::pin(tokio::fs::remove_dir_all(p))).await
}

/// Shared removal-retry policy (SSoT for the locking behavior): only
/// `PermissionDenied` — which is how Windows surfaces sharing/lock violations —
/// is retried; `NotFound` (already gone) is success for an idempotent removal, and
/// any other error is surfaced immediately.
async fn remove_tolerating_locks<F>(path: &Path, remove: F) -> std::io::Result<()>
where
    F: for<'p> Fn(&'p Path) -> Pin<Box<dyn Future<Output = std::io::Result<()>> + Send + 'p>>,
{
    const MAX_ATTEMPTS: u32 = 20;
    const RETRY_DELAY: Duration = Duration::from_millis(100);

    let mut attempt = 0;
    loop {
        match remove(path).await {
            Ok(()) => return Ok(()),
            // Already gone is success for an idempotent removal.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                attempt += 1;
                if attempt >= MAX_ATTEMPTS {
                    return Err(e);
                }
                tokio::time::sleep(RETRY_DELAY).await;
            }
            Err(e) => return Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn remove_file_tolerating_locks_removes_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("video.mkv");
        std::fs::write(&path, b"video").unwrap();

        remove_file_tolerating_locks(&path).await.unwrap();
        assert!(!path.exists());

        // A repeat removal is a no-op rather than an error (plain remove_file
        // returns NotFound here).
        remove_file_tolerating_locks(&path).await.unwrap();
    }

    #[tokio::test]
    async fn remove_dir_all_tolerating_locks_removes_tree_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let tree = dir.path().join("series/S01");
        std::fs::create_dir_all(&tree).unwrap();
        std::fs::write(tree.join("ep.mkv"), b"video").unwrap();

        remove_dir_all_tolerating_locks(&dir.path().join("series"))
            .await
            .unwrap();
        assert!(!dir.path().join("series").exists());

        remove_dir_all_tolerating_locks(&dir.path().join("series"))
            .await
            .unwrap();
    }
}
