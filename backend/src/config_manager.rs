use std::sync::Arc;
use tokio::sync::RwLock;

use jumbie_shared::config::{
    AuthConfig, Config, OrganizationConfig, ProxyConfig, SecurityConfig, SourcesConfig,
};
use tracing::debug;

/// Manages the two-tier config system: a TOML file holding the startup-path
/// fields (database, plugins_dir, logs_dir, …) and the database holding the
/// runtime config sections (organization, sources, general, …).
pub struct ConfigManager {
    config: RwLock<Config>,
    db: Arc<crate::db::DbManager>,
    config_path: String,
}

impl ConfigManager {
    /// Load config from both TOML and DB, merging them into one in-memory Config.
    ///
    /// `Config::new()` reads the TOML fields; each DB section is then overlaid,
    /// falling back to Rust defaults if the DB read fails (e.g. first run).
    pub async fn new(db: Arc<crate::db::DbManager>, config_path: &str) -> anyhow::Result<Self> {
        let mut config = Config::new(config_path)?;

        config.organization = db
            .get_organization_config()
            .await
            .unwrap_or_else(|_| OrganizationConfig::default());
        config.sources = db
            .get_sources_config()
            .await
            .unwrap_or_else(|_| SourcesConfig::default());
        config.general = db
            .get_general_config()
            .await
            .unwrap_or_else(|_| jumbie_shared::config::general::GeneralConfig::default());
        config.proxy = db
            .get_proxy_config()
            .await
            .unwrap_or_else(|_| ProxyConfig::default());
        config.auth = db
            .get_auth_config()
            .await
            .unwrap_or_else(|_| AuthConfig::default());
        config.security = db
            .get_security_config()
            .await
            .unwrap_or_else(|_| SecurityConfig::default());

        debug!("Config merge complete: 6 DB sections merged over TOML defaults");
        Ok(Self {
            config: RwLock::new(config),
            db,
            config_path: config_path.to_string(),
        })
    }

    /// Read the merged in-memory config.
    pub async fn read(&self) -> tokio::sync::RwLockReadGuard<'_, Config> {
        self.config.read().await
    }

    /// Write-acquire the in-memory config for mutation; call `persist()` after
    /// mutating. `persist()` itself takes only a read lock, so it won't deadlock.
    pub async fn write(&self) -> tokio::sync::RwLockWriteGuard<'_, Config> {
        self.config.write().await
    }

    /// Persist the current in-memory config to both the TOML file and the database.
    ///
    /// Call this after making changes via `write()`:
    /// ```ignore
    /// let mut cfg = cm.write().await;
    /// cfg.general.media_info_scan_enabled = false;
    /// cm.persist().await?;
    /// ```
    ///
    /// Takes only a read lock internally, so it is safe to call after the write
    /// lock has been released.
    pub async fn persist(&self) -> anyhow::Result<()> {
        let config = self.config.read().await;
        self.persist_inner(&config).await
    }

    /// Replace the in-memory config entirely and persist. The flush to disk + DB
    /// happens before the in-memory swap, so the operation is atomic.
    /// Used by the settings API when the user submits a full config payload.
    pub async fn save(&self, new_config: Config) -> anyhow::Result<()> {
        self.persist_inner(&new_config).await?;
        *self.config.write().await = new_config;
        Ok(())
    }

    /// The config.toml file path.
    pub fn config_path(&self) -> &str {
        &self.config_path
    }

    pub fn db(&self) -> &Arc<crate::db::DbManager> {
        &self.db
    }

    /// Write DB-managed sections to the database (skips config.toml entirely).
    /// This is the primary persist path for the settings API — config.toml is a
    /// startup-only file and should never be written at runtime.
    pub async fn persist_db(&self) -> anyhow::Result<()> {
        let config = self.config.read().await;
        self.persist_db_inner(&config).await
    }

    /// Write TOML fields to file + DB sections to database.
    /// Used during migration/startup when both need updating.
    async fn persist_inner(&self, config: &Config) -> anyhow::Result<()> {
        // TOML file: only the startup-path fields
        config.save(&self.config_path)?;
        self.persist_db_inner(config).await
    }

    async fn persist_db_inner(&self, config: &Config) -> anyhow::Result<()> {
        self.db
            .save_organization_config(&config.organization)
            .await?;
        self.db.save_general_config(&config.general).await?;
        self.db.save_sources_config(&config.sources).await?;
        self.db.save_proxy_config(&config.proxy).await?;
        self.db.save_security_config(&config.security).await?;
        self.db.save_auth_config(&config.auth).await?;
        Ok(())
    }
}
