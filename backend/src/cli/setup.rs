use std::path::PathBuf;
use std::sync::{Arc, RwLock};

/// Resolved values from the startup setup phase (config, logging, DB init).
/// The caller receives this after all one-time setup is complete and the
/// temporary DB manager has been dropped.
pub struct SetupOutput {
    pub config: jumbie_shared::config::Config,
    pub config_path: String,
    pub logs_dir: String,
    pub log_level: String,
    /// Keeps the non-blocking file-appender worker thread alive.
    /// MUST live as long as the application — dropping it silences all
    /// subsequent file log writes.
    pub _log_guard: tracing_appender::non_blocking::WorkerGuard,
    /// In-memory buffer of parsed log entries. Pre-populated from existing
    /// log files at startup, then appended to by a tracing layer on every
    /// log event. The API reads from this buffer — zero disk I/O at runtime.
    pub log_buffer: jumbie_shared::types::LogBuffer,
}

// Custom MakeWriter (not a Layer): fmt::Layer formats events identically
// regardless of writer, so capturing the formatted bytes and parsing them on drop
// reuses the file layer's format with no custom event-formatting code.

use std::io::Write;

/// Captures every formatted log line, parses it into a `LogEntry`, and pushes it
/// into a shared `LogBuffer`.
struct LogBufferWriter {
    buffer: jumbie_shared::types::LogBuffer,
}

impl tracing_subscriber::fmt::MakeWriter<'_> for LogBufferWriter {
    type Writer = LogLineCapture;

    fn make_writer(&self) -> Self::Writer {
        LogLineCapture {
            buffer: self.buffer.clone(),
            bytes: Vec::new(),
        }
    }
}

/// Per-event writer created by `LogBufferWriter::make_writer`.
struct LogLineCapture {
    buffer: jumbie_shared::types::LogBuffer,
    bytes: Vec<u8>,
}

impl Write for LogLineCapture {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.bytes.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Drop for LogLineCapture {
    fn drop(&mut self) {
        let line = std::str::from_utf8(&self.bytes).unwrap_or("");
        let trimmed = line.trim();
        if !trimmed.is_empty()
            && let Some(entry) = jumbie::logging::parse_log_line(trimmed.to_string())
            && entry.validate()
            && let Ok(mut buf) = self.buffer.write()
        {
            // LogRing evicts the oldest entries while over the byte budget.
            buf.push(entry);
        }
    }
}

/// Runs the full startup setup: config path resolution, logging init, config
/// loading, DB migration, seeding, and database maintenance commands.
///
/// Returns `Some(SetupOutput)` for normal startup flow, or `None` if a
/// maintenance command (`--db-stats`, `--vacuum`, `--analyze`, `--backup`,
/// `--restore`) was handled and the process should exit early.
pub async fn run_setup(args: &super::args::Args) -> anyhow::Result<Option<SetupOutput>> {
    // SSoT: jumbie_shared::config::default_config_path() handles all platform,
    // portable, and Docker logic internally.
    let args_config: PathBuf = args
        .config
        .clone()
        .unwrap_or_else(jumbie_shared::config::default_config_path);

    // Resolve logs directory before config is loaded (logging needs it first):
    //   1. Env var JUMBIE_LOGS_DIR
    //   2. Config.toml `logs_dir` field (if file exists)
    //   3. Rust default_logs_dir() (the SSoT — handles portable.txt, platform paths)
    let logs_dir = std::env::var("JUMBIE_LOGS_DIR").unwrap_or_else(|_| {
        let config_path = args_config.to_string_lossy().to_string();
        std::fs::read_to_string(&config_path)
            .ok()
            .and_then(|content| {
                content
                    .parse::<toml::Value>()
                    .ok()
                    .and_then(|v| v.get("logs_dir")?.as_str().map(String::from))
            })
            .unwrap_or_else(|| {
                let default = jumbie_shared::config::default_logs_dir();
                // Resolve relative paths against exe_dir (handles portable mode).
                if default.is_relative() {
                    jumbie_shared::config::win_exe_dir()
                        .join(&default)
                        .to_string_lossy()
                        .to_string()
                } else {
                    default.to_string_lossy().to_string()
                }
            })
    });

    if let Err(e) = std::fs::create_dir_all(&logs_dir) {
        // Pre-`tracing`-init warning: the subscriber is not installed yet, so
        // this goes through the sanctioned pre-logging stderr path.
        jumbie::logging::pre_init_warning(format_args!(
            "failed to create logs directory '{}': {}",
            logs_dir, e
        ));
    }

    // Buffer parsed log entries so the API reads them without any disk I/O.
    use jumbie_shared::types::{LOG_BUFFER_BYTES, LogBuffer};

    let log_buffer: LogBuffer = Arc::new(RwLock::new(jumbie_shared::types::LogRing::new()));

    // Pre-populate newest-first with a rolling window, so both entry count and
    // byte footprint stay bounded to LOG_BUFFER_BYTES without re-reading old logs.
    // The summary is emitted as a tracing event after the subscriber is installed
    // so it reaches the log file instead of the console.
    let (prepopulated_entries, prepopulated_bytes, prepopulated_files) = {
        // SSoT: `jumbie::logging::log_files` owns naming/enumeration and
        // `prepopulate_buffer` owns the byte-bounded, newest-first walk (returning
        // chronological entries so LogRing's eviction drops the oldest).
        let logs_path = std::path::Path::new(&logs_dir);
        let newest_first: Vec<PathBuf> = jumbie::logging::log_files(logs_path)
            .into_iter()
            .rev()
            .collect();
        let all_entries = jumbie::logging::prepopulate_buffer(&newest_first, LOG_BUFFER_BYTES);
        let entry_count = all_entries.len();
        let all_bytes: usize = all_entries.iter().map(|e| e.estimated_bytes()).sum();
        let file_count = newest_first.len();

        if let Ok(mut buf) = log_buffer.write() {
            for entry in all_entries {
                buf.push(entry);
            }
        }

        (entry_count, all_bytes, file_count)
    };

    // File output uses the size-rolled writer (2 MiB files, 8 MiB disk budget).
    let file_appender = jumbie::logging::SizeRoller::new(&logs_dir)?;
    let (non_blocking, log_guard) = tracing_appender::non_blocking(file_appender);

    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    // Console output is a development-only affordance, except for one-shot
    // maintenance commands which must report to the invoking user. The policy
    // and the sole sanctioned stdout writer live in `jumbie::logging` so the
    // release gate has exactly one home (SSoT).
    let console_layer = jumbie::logging::console_layer(args.is_maintenance_command());

    // Buffer layer: capture every formatted log line into the in-memory buffer
    // Buffer layer: capture every formatted log line into the in-memory buffer
    let buf_for_writer = log_buffer.clone();
    let buffer_writer = LogBufferWriter {
        buffer: buf_for_writer,
    };
    let buffer_layer = tracing_subscriber::fmt::layer()
        .with_writer(buffer_writer)
        .with_ansi(false);

    let file_layer = tracing_subscriber::fmt::layer()
        .with_writer(non_blocking)
        .with_ansi(false);

    let base_level = std::env::var("JUMBIE_LOG_LEVEL").unwrap_or_else(|_| {
        if args.verbose {
            "debug".to_string()
        } else {
            "info".to_string()
        }
    });

    let log_level =
        format!("h2::codec=off,hyper_util::client::legacy::pool=off,sqlx=info,{base_level}",);

    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(&log_level))
        .with(console_layer)
        .with(buffer_layer)
        .with(file_layer)
        .init();

    let config = jumbie_shared::config::Config::new(args_config.to_str().unwrap())?;
    let config_path = args_config.to_string_lossy().to_string();

    tracing::debug!(
        "Pre-populated log buffer with {} entries ({} bytes) from {} files",
        prepopulated_entries,
        prepopulated_bytes,
        prepopulated_files
    );
    tracing::info!("Starting Jumbie");

    // Handle database maintenance commands early (before creating full app).
    let temp_db_manager =
        Arc::new(jumbie::db::DbManager::new(std::path::Path::new(&config.database)).await?);

    if let Err(e) = temp_db_manager.seed_defaults().await {
        tracing::warn!("Failed to seed default data: {}", e);
    }

    if args.db_stats {
        let stats = temp_db_manager.stats().await?;
        let total_mb = stats.total_size_bytes as f64 / 1024.0 / 1024.0;
        let wasted_mb = stats.wasted_bytes as f64 / 1024.0 / 1024.0;
        let efficiency = 100.0 * (1.0 - stats.wasted_bytes as f64 / stats.total_size_bytes as f64);
        // One-shot CLI output: goes straight to the terminal regardless of build
        // profile — the user is not reading the log file. `logging::report` is
        // the sanctioned stdout writer.
        jumbie::logging::report("");
        jumbie::logging::report("Database Statistics:");
        jumbie::logging::report(format_args!("  Total Size:    {total_mb:.2} MB"));
        jumbie::logging::report(format_args!("  Wasted Space:  {wasted_mb:.2} MB"));
        jumbie::logging::report(format_args!("  Efficiency:    {efficiency:.1}%"));
        return Ok(None);
    }

    if args.vacuum {
        tracing::info!("Running VACUUM...");
        temp_db_manager.vacuum().await?;
        tracing::info!("✓ VACUUM completed successfully");
        return Ok(None);
    }

    if args.analyze {
        tracing::info!("Running ANALYZE...");
        temp_db_manager.analyze().await?;
        tracing::info!("✓ ANALYZE completed successfully");
        return Ok(None);
    }

    if let Some(backup_path) = &args.backup {
        tracing::info!("Creating backup to '{}'...", backup_path.display());
        if let Some(parent) = backup_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        temp_db_manager.backup(backup_path).await?;
        let size_mb = std::fs::metadata(backup_path)?.len() as f64 / 1024.0 / 1024.0;
        tracing::info!("✓ Backup created successfully ({:.2} MB)", size_mb);
        return Ok(None);
    }

    if let Some(restore_path) = &args.restore {
        tracing::info!("Restoring from '{}'...", restore_path.display());
        if !restore_path.exists() {
            anyhow::bail!("Backup file does not exist: '{}'", restore_path.display());
        }
        let current_backup = format!("{}.pre-restore", config.database);
        std::fs::copy(&config.database, &current_backup)?;
        drop(temp_db_manager);
        std::fs::copy(restore_path, &config.database)?;
        tracing::info!("✓ Database restored. Previous backup: {}", current_backup);
        return Ok(None);
    }

    // Drop the temp manager; the real app creates its own DB handle.
    drop(temp_db_manager);

    Ok(Some(SetupOutput {
        config,
        config_path,
        logs_dir,
        log_level,
        _log_guard: log_guard,
        log_buffer,
    }))
}
