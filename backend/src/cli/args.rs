use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug, Clone)]
#[command(author, version, about, long_about = None)]
pub struct Args {
    // Database maintenance flags (--db-stats, --vacuum, --analyze, --backup,
    // --restore) exit immediately and are mutually exclusive with server mode.
    /// Config file (TOML only). Can be overridden via JUMBIE_CONFIG env var.
    /// Default: <exe_dir>/config/config.toml (Windows) or config/config.toml (Unix/Docker)
    #[arg(short, long, env = "JUMBIE_CONFIG")]
    pub config: Option<PathBuf>,

    /// Verbose output
    #[arg(short, long)]
    pub verbose: bool,

    /// Run without the system tray (headless)
    #[arg(long)]
    pub no_tray: bool,

    /// Perform database vacuum (reclaim space)
    #[arg(long)]
    pub vacuum: bool,

    /// Perform database analyze (update statistics)
    #[arg(long)]
    pub analyze: bool,

    /// Create database backup
    #[arg(long)]
    pub backup: Option<PathBuf>,

    /// Restore database from backup
    #[arg(long)]
    pub restore: Option<PathBuf>,

    /// Show database statistics
    #[arg(long)]
    pub db_stats: bool,
}

impl Args {
    /// A one-shot database maintenance command (`--db-stats`, `--vacuum`,
    /// `--analyze`, `--backup`, `--restore`). Such runs exit immediately, so
    /// they keep console output even in release builds — the invoking user
    /// needs to see the result (see `jumbie::logging::console_enabled`).
    pub fn is_maintenance_command(&self) -> bool {
        self.db_stats
            || self.vacuum
            || self.analyze
            || self.backup.is_some()
            || self.restore.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn no_tray_defaults_to_false() {
        let args = Args::try_parse_from(["jumbie"]).unwrap();
        assert!(!args.no_tray);
    }

    #[test]
    fn no_tray_flag_is_parsed() {
        let args = Args::try_parse_from(["jumbie", "--no-tray"]).unwrap();
        assert!(args.no_tray);
    }
}
