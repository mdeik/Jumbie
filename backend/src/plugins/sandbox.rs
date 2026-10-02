// Plugin Process Sandboxing
//
// Per-process isolation for external plugins lives in TWO places:
//
//   • host.rs — spawn-time hardening applied to the CHILD via `pre_exec`
//     (Unix): per-plugin rlimits (RLIMIT_CPU/NOFILE/FSIZE/NPROC/DATA),
//     process-group membership (`process_group(0)`) so the whole group can be
//     killed, and environment isolation (`env_clear` + allowlist). Windows has
//     no pre-exec hook: the child gets no rlimits, and killing the child does
//     not kill grandchildren (TerminateProcess semantics).
//
//   • sandbox.rs (this file) — the persistent PID registry: every spawned
//     plugin PID is recorded to `<plugins_dir>/.plugin_pids.json` so that if
//     the HOST crashes, the next startup can sweep the stale plugin processes
//     a crashed host left behind (SIGTERM on Unix, `taskkill /F` on Windows).
//
// The in-app sandbox bounds abuse and secret access, not hostile-code
// confinement — for untrusted plugins, run Jumbie containerized (spec §8).

use std::sync::Mutex;

/// Global set of PIDs known to the host. Populated at startup by scanning
/// the plugins directory and cleaned up on graceful shutdown. If the host crashes,
/// stale PIDs remain as a record for the next startup to clean.
static ORPHAN_PID_BLACKLIST: std::sync::OnceLock<Mutex<Vec<u32>>> = std::sync::OnceLock::new();

/// State-file location for the persistent PID registry (set by the manager).
/// When `None` (unset, e.g. in tests), PIDs are tracked in memory only and no
/// file is written.
static STATE_PATH: std::sync::OnceLock<Option<std::path::PathBuf>> = std::sync::OnceLock::new();

/// Set the path of the persistent PID registry state file. Called once by
/// `PluginManager::new` with `<plugins_dir>/.plugin_pids.json`.
pub fn init_state_path(path: std::path::PathBuf) {
    let _ = STATE_PATH.set(Some(path));
}

#[cfg(not(test))]
fn state_path() -> Option<std::path::PathBuf> {
    STATE_PATH.get_or_init(|| None).clone()
}

/// Track a plugin process PID for orphan cleanup.
///
/// When a state path is configured (production) the PID is also persisted to
/// disk — the in-memory list alone dies with the host, which is exactly when the
/// registry is needed.
pub fn track_pid(pid: u32) {
    let lock = ORPHAN_PID_BLACKLIST.get_or_init(|| std::sync::Mutex::new(Vec::new()));
    let mut pids = lock.lock().unwrap();
    pids.push(pid);

    // Persist (production only — tests have no state path).
    #[cfg(not(test))]
    if let Some(path) = state_path()
        && let Ok(mut pids) = load_pids(&path)
        && !pids.contains(&pid)
    {
        pids.push(pid);
        let _ = save_pids(&path, &pids);
    }
}

#[cfg(not(test))]
fn load_pids(path: &std::path::Path) -> std::io::Result<Vec<u32>> {
    match std::fs::read_to_string(path) {
        Ok(s) => serde_json::from_str(&s).map_err(|e| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("bad pid file: {e}"),
            )
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e),
    }
}

#[cfg(not(test))]
fn save_pids(path: &std::path::Path, pids: &[u32]) -> std::io::Result<()> {
    let tmp = path.with_extension("json.tmp");
    let json = serde_json::to_string(pids)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, path)
}

/// Clean up known-stale plugin PIDs on startup.
///
/// Reads the persistent registry (the in-memory list is empty on a fresh
/// process — the file is what survives a crash), SIGTERMs each recorded PID,
/// then clears the registry. Best-effort — processes may have already exited,
/// or the PID may have been reused by a non-plugin process.
#[cfg(not(test))]
pub fn cleanup_orphan_pids() {
    let Some(path) = state_path() else {
        return;
    };
    let Ok(pids) = load_pids(&path) else {
        return;
    };
    if pids.is_empty() {
        return;
    }
    tracing::info!(
        "Cleaning up {} stale plugin process(es) from a previous run",
        pids.len()
    );
    for pid in pids {
        #[cfg(unix)]
        {
            let _ = unsafe { libc::kill(pid as i32, libc::SIGTERM) };
        }
        #[cfg(not(unix))]
        {
            // Windows: no signals — best-effort force-kill via taskkill.
            let _ = std::process::Command::new("taskkill")
                .args(["/F", "/PID", &pid.to_string()])
                .status();
        }
    }
    let _ = std::fs::remove_file(&path);
}
