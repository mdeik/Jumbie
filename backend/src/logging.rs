//! Size-bounded log file writing and reading.
//!
//! Rotation is by size, never by time: the current file rolls at
//! [`ROTATE_BYTES`] and archives are culled to [`DISK_BUDGET_BYTES`]. Files not
//! matching the scheme (e.g. `jumbie.log.bak`) are ignored entirely — never
//! read, never served, never culled.
//!
//! SSoT for the on-disk log scheme and line format ([`parse_log_line`] /
//! [`strip_ansi`] / [`prepopulate_buffer`]).

use std::collections::VecDeque;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use jumbie_shared::types::LogEntry;

/// Rotation threshold for the current log file (2 MiB).
pub const ROTATE_BYTES: u64 = 2 * 1024 * 1024;

/// Total disk budget for all log files (8 MiB). On rotation the oldest
/// archives are deleted while the total exceeds this.
pub const DISK_BUDGET_BYTES: u64 = 8 * 1024 * 1024;

/// Name of the current (actively written) log file.
pub const CURRENT_FILE: &str = "jumbie.log";

/// Maximum number of archive files kept alongside the current file
/// (`jumbie.log.1` = newest archive … `jumbie.log.{MAX_ARCHIVES}` = oldest).
pub const MAX_ARCHIVES: usize = 3;

/// Whether `name` belongs to the size-rolled scheme (`jumbie.log`,
/// `jumbie.log.1`, …). Arbitrary files in the logs directory are ignored —
/// never read, served, or culled.
pub fn is_log_file_name(name: &str) -> bool {
    name == CURRENT_FILE
        || name
            .strip_prefix(&format!("{CURRENT_FILE}."))
            .is_some_and(|rest| rest.parse::<u64>().is_ok())
}

/// Enumerate the current-scheme log files in `dir`, **oldest → newest**
/// (current file last). SSoT for the file naming scheme; arbitrary files are
/// never included.
pub fn log_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    // Descending so archives are ordered oldest → newest (.3, .2, .1).
    for i in (1..=MAX_ARCHIVES).rev() {
        let p = dir.join(format!("{CURRENT_FILE}.{i}"));
        if p.exists() {
            files.push(p);
        }
    }
    let current = dir.join(CURRENT_FILE);
    if current.exists() {
        files.push(current);
    }
    files
}

/// Whether console (stdout) logging is enabled for this run.
///
/// SSoT for the console-output policy. Application logs always go to the
/// size-rolled file and the in-memory buffer; stdout is a development-only
/// affordance:
/// - debug builds (`cargo run`, tests): enabled, so logs appear on the terminal;
/// - release builds, long-running server: disabled, so a supervised deployment
///   (systemd/Docker) writes application logs to the log file only;
/// - release builds, one-shot maintenance commands (`--vacuum`, `--backup`, …):
///   enabled, because the command exists solely to report to the invoking user.
pub fn console_enabled(one_shot_command: bool) -> bool {
    cfg!(debug_assertions) || one_shot_command
}

/// Build the console (stdout) layer for a `tracing_subscriber::registry`.
///
/// This is the **only** sanctioned place that constructs a stdout/stderr writer
/// (enforced by CI), so [`console_enabled`] is the single gate for console
/// output.
pub fn console_layer<S>(one_shot_command: bool) -> Option<impl tracing_subscriber::layer::Layer<S>>
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    console_enabled(one_shot_command)
        .then(|| tracing_subscriber::fmt::layer().with_writer(std::io::stdout))
}

/// Emit a warning straight to stderr **before the `tracing` subscriber is
/// installed**.
///
/// Events emitted before [`crate::logging`]'s subscriber exists (e.g. tray init
/// in `main`, or the logs-directory fallback in `cli::setup`) are silently
/// dropped, so those early failures must write to the terminal themselves. This
/// is the sanctioned pre-`tracing` diagnostic path; like [`console_layer`] it
/// constructs a raw stderr writer, which only this module may do — the crate
/// denies the `print_*` macros and CI greps for direct `std::io` writers.
pub fn pre_init_warning(message: impl std::fmt::Display) {
    // `writeln!` into an explicit writer, not `eprintln!`: the crate denies the
    // print macros everywhere, and the raw writer is the console SSoT here.
    let _ = writeln!(io::stderr(), "Warning: {message}");
}

/// Print a one-shot maintenance-command report straight to stdout.
///
/// Maintenance commands (`--db-stats`, `--vacuum`, …) exist to report to the
/// invoking user, so their result goes to the terminal regardless of build
/// profile — [`console_enabled`] keeps the console layer on for exactly these
/// runs. This is the sanctioned raw-stdout writer (see [`pre_init_warning`]).
pub fn report(message: impl std::fmt::Display) {
    let _ = writeln!(io::stdout(), "{message}");
}

/// Remove ANSI SGR escape sequences (color/style codes) from a string.
/// Handles only SGR codes (ending in 'm'), which is all that log output uses.
pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            if chars.peek() == Some(&'[') {
                chars.next(); // consume '['
                for ch in chars.by_ref() {
                    if ch.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Parse a raw log line into a [`LogEntry`]. SSoT for the on-disk line format
/// (shared by the runtime buffer writer and the startup pre-population).
pub fn parse_log_line(line: String) -> Option<LogEntry> {
    let mut tokens = line.split_ascii_whitespace();
    let first = tokens.next()?;
    // No digits ⇒ not a timestamp, so a bare message (e.g. plugin stdout); skip.
    if !first.chars().any(|c| c.is_ascii_digit()) {
        return None;
    }
    let timestamp = first.to_string();
    let level = tokens.next()?.to_string();
    // Joining with " " is lossy (tabs become spaces) but log messages are
    // primarily informational, so exact whitespace is not important.
    let raw_message = tokens.collect::<Vec<_>>().join(" ");
    let message = strip_ansi(&raw_message);
    Some(LogEntry {
        timestamp,
        level,
        message,
    })
}

/// Build the startup log buffer from the on-disk files.
///
/// `files_newest_first` must be ordered newest → oldest (current file first,
/// then `.1`, `.2`, …). The returned entries are **chronological** (oldest
/// first) so the buffer's byte-budget eviction drops the oldest.
///
/// Memory is bounded to `budget` bytes no matter how large the on-disk logs
/// are: each file contributes only its tail, and a single oversized line is
/// dropped. Regression guard for the historical ~1.5 GiB buffer blow-up.
pub fn prepopulate_buffer(files_newest_first: &[PathBuf], budget: usize) -> VecDeque<LogEntry> {
    let mut all_entries: VecDeque<LogEntry> = VecDeque::new();
    let mut all_bytes = 0usize;

    for path in files_newest_first {
        // Per-file rolling window: keep only this file's tail that fits the
        // budget, bounding memory even if a giant file appears in `dir`.
        let mut file_entries: VecDeque<LogEntry> = VecDeque::new();
        let mut file_bytes = 0usize;
        if let Ok(file) = File::open(path) {
            for line in BufReader::new(file).lines().map_while(Result::ok) {
                if let Some(entry) = parse_log_line(line)
                    && entry.validate()
                {
                    file_bytes += entry.estimated_bytes();
                    file_entries.push_back(entry);
                    while file_bytes > budget {
                        if let Some(front) = file_entries.pop_front() {
                            file_bytes = file_bytes.saturating_sub(front.estimated_bytes());
                        } else {
                            break;
                        }
                    }
                }
            }
        }

        // Prepending keeps chronological order across the newest-first walk.
        for entry in file_entries.into_iter().rev() {
            all_bytes += entry.estimated_bytes();
            all_entries.push_front(entry);
        }

        while all_bytes > budget {
            if let Some(front) = all_entries.pop_front() {
                all_bytes = all_bytes.saturating_sub(front.estimated_bytes());
            } else {
                break;
            }
        }

        // Optimization, not correctness: once the newest entries fill the
        // budget, older files cannot contribute.
        if all_bytes >= budget {
            break;
        }
    }

    // Release excess ring-buffer capacity so a verbose install doesn't pin its
    // largest-ever allocation (hundreds of MB to GBs).
    all_entries.shrink_to_fit();
    all_entries
}

/// `std::io::Write` that appends to the current log file and rolls it over by
/// size. Runs on the `tracing_appender::non_blocking` writer thread, so it is
/// owned by exactly one thread and needs no internal locking.
pub struct SizeRoller {
    dir: PathBuf,
    current: File,
    current_bytes: u64,
}

impl SizeRoller {
    /// Open (or create) the current log file in `dir`. Files that don't match
    /// the scheme are left untouched.
    pub fn new(dir: impl Into<PathBuf>) -> io::Result<Self> {
        let dir = dir.into();
        fs::create_dir_all(&dir)?;

        let current = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join(CURRENT_FILE))?;
        let current_bytes = current.metadata()?.len();
        Ok(Self {
            dir,
            current,
            current_bytes,
        })
    }

    fn archive_path(&self, n: usize) -> PathBuf {
        self.dir.join(format!("{CURRENT_FILE}.{n}"))
    }

    /// Roll the current file into the archive set: drop the oldest archive,
    /// shift the rest up, then start a fresh current file. Afterwards cull by
    /// the disk budget.
    fn rotate(&mut self) -> io::Result<()> {
        self.current.flush()?;
        let _ = fs::remove_file(self.archive_path(MAX_ARCHIVES));
        for i in (1..MAX_ARCHIVES).rev() {
            let src = self.archive_path(i);
            if src.exists() {
                let _ = fs::rename(&src, self.archive_path(i + 1));
            }
        }
        fs::rename(self.dir.join(CURRENT_FILE), self.archive_path(1))?;
        self.current = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.dir.join(CURRENT_FILE))?;
        self.current_bytes = 0;
        self.cull()
    }

    /// Delete oldest archives while the total on-disk footprint exceeds the
    /// budget. The current file is never deleted; it is bounded by
    /// `ROTATE_BYTES` plus at most one (oversized) write.
    fn cull(&mut self) -> io::Result<()> {
        let mut total = self.current_bytes;
        for i in 1..=MAX_ARCHIVES {
            if let Ok(m) = fs::metadata(self.archive_path(i)) {
                total += m.len();
            }
        }
        let mut i = MAX_ARCHIVES;
        while total > DISK_BUDGET_BYTES && i > 0 {
            let p = self.archive_path(i);
            if let Ok(m) = fs::metadata(&p)
                && fs::remove_file(&p).is_ok()
            {
                total = total.saturating_sub(m.len());
            }
            i -= 1;
        }
        Ok(())
    }
}

impl Write for SizeRoller {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.current.write(buf)?;
        self.current_bytes += n as u64;
        if self.current_bytes >= ROTATE_BYTES {
            self.rotate()?;
        }
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.current.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_entries(roller: &mut SizeRoller, n: usize, bytes: usize) {
        for _ in 0..n {
            roller.write_all(&vec![b'x'; bytes]).unwrap();
        }
    }

    fn dir_files(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn test_rotates_at_threshold_and_keeps_max_archives() {
        let dir = tempfile::tempdir().unwrap();
        let mut roller = SizeRoller::new(dir.path()).unwrap();

        // Fill well past the rotation threshold many times over.
        write_entries(&mut roller, (ROTATE_BYTES as usize / 1000) * 6, 1000);

        let files = dir_files(dir.path());
        assert!(files.contains(&CURRENT_FILE.to_string()));
        for i in 1..=MAX_ARCHIVES {
            assert!(files.contains(&format!("{CURRENT_FILE}.{i}")));
        }
        // No archive beyond the max.
        assert!(!files.contains(&format!("{CURRENT_FILE}.{}", MAX_ARCHIVES + 1)));
    }

    #[test]
    fn test_culls_to_disk_budget() {
        let dir = tempfile::tempdir().unwrap();
        {
            let mut roller = SizeRoller::new(dir.path()).unwrap();
            // Write ~6× the rotation size so rotation + culling are exercised.
            write_entries(&mut roller, (ROTATE_BYTES as usize / 1000) * 6, 1000);
        }
        let total: u64 = fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .filter(|e| is_log_file_name(&e.file_name().to_string_lossy()))
            .map(|e| e.metadata().unwrap().len())
            .sum();
        assert!(
            total <= DISK_BUDGET_BYTES,
            "disk footprint {total} exceeds budget {DISK_BUDGET_BYTES}"
        );
    }

    #[test]
    fn test_non_scheme_files_are_ignored_not_deleted() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("jumbie.log.2026-08-24"), b"old").unwrap();
        fs::write(dir.path().join("jumbie.log.2026-08-23"), b"older").unwrap();
        fs::write(dir.path().join("other.txt"), b"keep").unwrap();

        // Creating the roller must not touch files outside the scheme.
        let _roller = SizeRoller::new(dir.path()).unwrap();

        let files = dir_files(dir.path());
        assert!(files.contains(&"jumbie.log.2026-08-24".to_string()));
        assert!(files.contains(&"jumbie.log.2026-08-23".to_string()));
        assert!(files.contains(&"other.txt".to_string()));
        assert!(files.contains(&CURRENT_FILE.to_string()));

        // …but they are excluded from the enumeration the app actually uses.
        assert_eq!(log_files(dir.path()), vec![dir.path().join(CURRENT_FILE)]);
    }

    #[test]
    fn test_is_log_file_name() {
        assert!(is_log_file_name("jumbie.log"));
        assert!(is_log_file_name("jumbie.log.1"));
        assert!(is_log_file_name("jumbie.log.42"));
        assert!(!is_log_file_name("jumbie.log.2026-08-24"));
        assert!(!is_log_file_name("jumbie.log.1.txt"));
        assert!(!is_log_file_name("other.log"));
        assert!(!is_log_file_name("jumbie.log.bak"));
    }

    #[test]
    fn test_log_files_orders_oldest_to_newest_and_excludes_others() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("jumbie.log"), b"c").unwrap();
        fs::write(dir.path().join("jumbie.log.1"), b"b").unwrap();
        fs::write(dir.path().join("jumbie.log.2"), b"a").unwrap();
        fs::write(dir.path().join("jumbie.log.2026-08-24"), b"old").unwrap();
        fs::write(dir.path().join("other.txt"), b"x").unwrap();

        let files = log_files(dir.path());
        assert_eq!(
            files,
            vec![
                dir.path().join("jumbie.log.2"),
                dir.path().join("jumbie.log.1"),
                dir.path().join("jumbie.log"),
            ]
        );
    }

    #[test]
    fn test_strip_ansi() {
        assert_eq!(strip_ansi("hello"), "hello");
        assert_eq!(strip_ansi("\x1b[31mred\x1b[0m"), "red");
        assert_eq!(strip_ansi("normal\x1b[1mbold\x1b[22m"), "normalbold");
        assert_eq!(strip_ansi("\x1b[38;5;196mcolor\x1b[m"), "color");
    }

    #[test]
    fn test_parse_log_line_standard() {
        let entry =
            parse_log_line("2024-03-15T12:00:00.123Z  INFO source_processor: hello".to_string());
        let entry = entry.unwrap();
        assert_eq!(entry.timestamp, "2024-03-15T12:00:00.123Z");
        assert_eq!(entry.level, "INFO");
        assert_eq!(entry.message, "source_processor: hello");
        assert!(entry.validate());
    }

    #[test]
    fn test_parse_log_line_bare_message_no_digits() {
        let entry = parse_log_line("not supported by InternalSourcePlugin".to_string());
        assert!(entry.is_none(), "bare message should be filtered out");
    }

    #[test]
    fn test_parse_log_line_bare_message_with_digits() {
        // A line starting with a token that has digits but no timestamp format
        // should still be parsed as a standard entry (first=ts, second=level)
        let entry = parse_log_line("12345 SOME_LEVEL rest of line".to_string());
        assert!(entry.is_some());
        let entry = entry.unwrap();
        assert_eq!(entry.timestamp, "12345");
        assert_eq!(entry.level, "SOME_LEVEL");
        assert_eq!(entry.message, "rest of line");
    }

    #[test]
    fn test_parse_log_line_empty_line() {
        let entry = parse_log_line("".to_string());
        assert!(entry.is_none(), "empty line should be filtered out");
    }

    #[test]
    fn test_prepopulate_never_exceeds_byte_budget() {
        // Regression: the historical bug parsed ALL log files into memory
        // before capping, retaining ~1.5 GiB. The pre-population must stay
        // within `budget` bytes no matter how big the on-disk files are.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("jumbie.log");
        let mut content = String::new();
        for i in 0..200_000 {
            content.push_str(&format!("2026-01-01T00:00:00Z  INFO jumbie::a: line {i}\n"));
        }
        fs::write(&path, content).unwrap();

        let budget = 64 * 1024;
        let entries = prepopulate_buffer(&[path], budget);
        let bytes: usize = entries.iter().map(|e| e.estimated_bytes()).sum();
        assert!(
            bytes <= budget,
            "pre-populated buffer uses {bytes} bytes, budget is {budget}"
        );
    }

    #[test]
    fn test_prepopulate_drops_oversized_line() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("jumbie.log");
        // A single line far larger than the budget must be dropped, not
        // retained (this is the "user drops a giant file in" guarantee).
        fs::write(
            &path,
            format!(
                "2026-01-01T00:00:00Z  INFO jumbie::a: {}\n",
                "x".repeat(100_000)
            ),
        )
        .unwrap();

        let entries = prepopulate_buffer(&[path], 1024);
        assert!(entries.is_empty(), "oversized line is dropped");
    }

    #[test]
    fn test_prepopulate_chronological_and_newest_retained() {
        let dir = tempfile::tempdir().unwrap();
        let current = dir.path().join("jumbie.log");
        let archive = dir.path().join("jumbie.log.1");
        fs::write(
            &current,
            "2026-01-01T00:00:03Z  INFO jumbie::a: new1\n\
             2026-01-01T00:00:04Z  INFO jumbie::a: new2\n",
        )
        .unwrap();
        fs::write(
            &archive,
            "2026-01-01T00:00:01Z  INFO jumbie::a: old1\n\
             2026-01-01T00:00:02Z  INFO jumbie::a: old2\n",
        )
        .unwrap();

        // newest-first order (current, then .1) with an unbounded budget:
        // everything is retained, oldest first.
        let entries = prepopulate_buffer(&[current, archive], usize::MAX);
        let ts: Vec<String> = entries.iter().map(|e| e.timestamp.clone()).collect();
        assert_eq!(
            ts,
            vec![
                "2026-01-01T00:00:01Z".to_string(),
                "2026-01-01T00:00:02Z".to_string(),
                "2026-01-01T00:00:03Z".to_string(),
                "2026-01-01T00:00:04Z".to_string(),
            ]
        );
    }

    #[test]
    fn test_prepopulate_keeps_newest_when_over_budget() {
        let dir = tempfile::tempdir().unwrap();
        let current = dir.path().join("jumbie.log");
        let archive = dir.path().join("jumbie.log.1");
        fs::write(
            &current,
            "2026-01-01T00:00:03Z  INFO jumbie::a: new1\n\
             2026-01-01T00:00:04Z  INFO jumbie::a: new2\n",
        )
        .unwrap();
        fs::write(
            &archive,
            "2026-01-01T00:00:01Z  INFO jumbie::a: old1\n\
             2026-01-01T00:00:02Z  INFO jumbie::a: old2\n",
        )
        .unwrap();

        // Budget fits ~one entry: only the newest survives, and the oldest
        // (from .1) is evicted first.
        let entries = prepopulate_buffer(&[current, archive], 200);
        let ts: Vec<String> = entries.iter().map(|e| e.timestamp.clone()).collect();
        assert_eq!(ts, vec!["2026-01-01T00:00:04Z".to_string()]);
    }
}
