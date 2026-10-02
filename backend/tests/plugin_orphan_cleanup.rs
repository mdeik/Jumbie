//! Persistent PID registry — cross-process orphan cleanup.
//!
//! Must be an integration test, not a lib unit test: the registry's persistence
//! path is `#[cfg(not(test))]` in the lib, so it only compiles when the crate is
//! built as a dependency. Verifies the full crash cycle: startup → plugin
//! spawned (PID persisted) → host CRASHES (manager dropped, no cleanup) → next
//! startup sweeps the stale PID.
//!
//! The victim is a portable `python -c "time.sleep(...)"` child, so the kill
//! path runs on Unix (SIGTERM) and Windows (`taskkill /F`).

use std::path::PathBuf;
use std::time::Duration;

/// A portable victim process (a sleeping python interpreter). Owns its `Child`
/// so it is reaped on drop — never leaks a process, even on panic.
struct Victim(std::process::Child);

impl Victim {
    /// Spawn a sleeping python child. Returns `None` when python is
    /// unavailable.
    fn spawn() -> Option<(Self, u32)> {
        let python = jumbie_shared::plugin::python_command()?;
        let child = std::process::Command::new(python)
            .args(["-c", "import time; time.sleep(120)"])
            .spawn()
            .ok()?;
        let pid = child.id();
        Some((Self(child), pid))
    }

    fn alive(&mut self) -> bool {
        matches!(self.0.try_wait(), Ok(None))
    }
}

impl Drop for Victim {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[tokio::test]
async fn orphan_pids_are_swept_on_next_startup() {
    let Some((mut victim, pid)) = Victim::spawn() else {
        eprintln!("python not available — skipping orphan cleanup test");
        return;
    };

    let dir = tempfile::tempdir().expect("tempdir");
    let plugins_dir: PathBuf = dir.path().join("plugins");
    std::fs::create_dir_all(&plugins_dir).unwrap();
    let state_file = plugins_dir.join(".plugin_pids.json");

    // First "startup": nothing tracked yet, so no registry file is created.
    {
        let _manager = jumbie::plugins::PluginManager::new(plugins_dir.clone());
    }
    assert!(
        !state_file.exists(),
        "registry must be empty before any plugin spawns"
    );

    // Simulate a plugin spawn so its PID is persisted (as the host does).
    jumbie::plugins::sandbox::track_pid(pid);
    assert!(
        state_file.exists(),
        "track_pid must persist the PID to the registry"
    );
    assert!(victim.alive(), "victim must be alive before the sweep");

    // The host "crashes": the manager is dropped with no cleanup, leaving the
    // stale PID in the registry for the next startup to sweep.
    {
        let _manager = jumbie::plugins::PluginManager::new(plugins_dir.clone());
    }

    // The kill (SIGTERM / taskkill) lands asynchronously — poll briefly.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while victim.alive() && std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        !victim.alive(),
        "startup sweep must kill the stale plugin process (pid {pid})"
    );
    assert!(
        !state_file.exists(),
        "registry must be cleared after the sweep"
    );
}
