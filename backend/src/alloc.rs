// Global allocator: jemalloc on non-Windows, system allocator on Windows.
// jemalloc's background threads purge freed pages, avoiding the unbounded RSS
// growth of glibc's per-thread arenas in a long-running multi-threaded process.
// Background threads are a Linux-only capability (jemalloc does not compile
// them for Mach-O), so macOS relies on the dirty/muzzy decay rates instead.

// jemalloc's C code requires MSVC and can fail to build on some Windows CI
// runners, so Windows falls back to the system allocator.
#[cfg(not(target_os = "windows"))]
pub use tikv_jemallocator::Jemalloc as Allocator;

#[cfg(not(target_os = "windows"))]
#[global_allocator]
pub static GLOBAL: Allocator = Allocator;

/// Configure jemalloc for aggressive page return at startup (no-op on Windows).
pub fn configure() {
    #[cfg(not(target_os = "windows"))]
    {
        // 5s decay (jemalloc default is 10s); silently ignore if the MIB entry
        // is unavailable (e.g. stats feature disabled).
        // SAFETY: valid jemalloc MIB names taking a uint64; these calls only mutate
        // allocator-internal state and run single-threaded before the tokio runtime starts.
        unsafe {
            let _ = tikv_jemalloc_ctl::raw::update(b"dirty_decay_ms\0", &5000u64);
            let _ = tikv_jemalloc_ctl::raw::update(b"muzzy_decay_ms\0", &5000u64);
        }
    }
}

/// Read jemalloc mallctl stats.
///
/// Returns `(allocated, active, mapped, resident)` in bytes.
/// On Windows (where jemalloc is not available), returns all zeros.
pub fn memory_stats() -> (u64, u64, u64, u64) {
    #[cfg(not(target_os = "windows"))]
    {
        (|| -> Result<_, String> {
            tikv_jemalloc_ctl::epoch::advance().map_err(|e| e.to_string())?;
            Ok((
                tikv_jemalloc_ctl::stats::allocated::read().map_err(|e| e.to_string())? as u64,
                tikv_jemalloc_ctl::stats::active::read().map_err(|e| e.to_string())? as u64,
                tikv_jemalloc_ctl::stats::mapped::read().map_err(|e| e.to_string())? as u64,
                tikv_jemalloc_ctl::stats::resident::read().map_err(|e| e.to_string())? as u64,
            ))
        })()
        .unwrap_or((0, 0, 0, 0))
    }

    #[cfg(target_os = "windows")]
    {
        (0, 0, 0, 0)
    }
}
