// Config sections store an entire JSON blob in a single `data` column rather than
// normalising across tables: each section is read as a unit, validated by serde,
// and replaced atomically, and normalising would require a migration per field.
// Config sections are small, so reading the whole blob is negligible.
use super::DbManager;
use anyhow::Result;
use jumbie_shared::config::defaults;
use jumbie_shared::config::{
    AuthConfig, DynamicProfilesConfig, GeneralConfig, OrganizationConfig, ProxyConfig,
    SecurityConfig, SourcesConfig, UIConfig,
};
use jumbie_shared::scoring::ReleaseProfile;
use jumbie_shared::types::{FilterRule, Quality, QualityProfile};
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::collections::HashMap;

impl DbManager {
    pub async fn get_system_state(&self, key: &str) -> Result<Option<String>> {
        let row: Option<(String,)> = sqlx::query_as("SELECT data FROM system_state WHERE key = ?")
            .bind(key)
            .fetch_optional(&self.pool)
            .await?;

        Ok(row.map(|(v,)| v))
    }

    pub async fn set_system_state(&self, key: &str, value: &str) -> Result<()> {
        exec_with_bindings!(
            self,
            "INSERT INTO system_state (key, data) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET data = excluded.data",
            key,
            value
        )
    }

    pub async fn get_release_profile(&self, id: &str) -> Result<Option<ReleaseProfile>> {
        let all = self.get_all_release_profiles().await?;
        Ok(all.get(id).cloned())
    }

    pub async fn get_effective_min_score(&self, profile_id: &str) -> Result<i32> {
        Ok(self
            .get_release_profile(profile_id)
            .await?
            .map(|p| p.min_score)
            .unwrap_or(0))
    }

    // Config-to-DB: Release Profiles

    pub async fn get_all_release_profiles(&self) -> Result<HashMap<String, ReleaseProfile>> {
        self.fetch_json_map("SELECT id, data FROM release_profiles")
            .await
    }

    pub async fn upsert_release_profile(&self, id: &str, profile: &ReleaseProfile) -> Result<()> {
        self.upsert_json_record("release_profiles", id, profile)
            .await
    }

    pub async fn delete_release_profile(&self, id: &str) -> Result<()> {
        exec_by_id!(self, "DELETE FROM release_profiles WHERE id = ?", id)
    }

    // Config-to-DB: Quality definitions

    pub async fn get_all_quality_definitions(&self) -> Result<HashMap<String, Quality>> {
        self.fetch_json_map("SELECT id, data FROM quality_definitions")
            .await
    }

    pub async fn upsert_quality_definition(&self, id: &str, quality: &Quality) -> Result<()> {
        self.upsert_json_record("quality_definitions", id, quality)
            .await
    }

    pub async fn delete_quality_definition(&self, id: &str) -> Result<()> {
        exec_by_id!(self, "DELETE FROM quality_definitions WHERE id = ?", id)
    }

    // Quality & Release Profile Default Resolvers

    // The "default" profile is the first map key (insertion order preserved by
    // indexmap) — there is no explicit `is_default` flag, so changing the default
    // is just a reorder in the UI, with no migration.
    pub async fn get_default_quality_profile(&self) -> String {
        self.get_all_quality_profiles()
            .await
            .unwrap_or_default()
            .keys()
            .next()
            .cloned()
            .unwrap_or_else(|| "Default".to_string())
    }

    pub async fn get_default_release_profile(&self) -> String {
        self.get_all_release_profiles()
            .await
            .unwrap_or_default()
            .keys()
            .next()
            .cloned()
            .unwrap_or_else(|| "Default".to_string())
    }

    /// SSoT: fetch both qualities and quality profiles in one call so callers
    /// load them from the same DB snapshot (avoids N+1 and inconsistency).
    pub async fn get_all_quality_data(
        &self,
    ) -> (HashMap<String, Quality>, HashMap<String, QualityProfile>) {
        let qualities = self.get_all_quality_definitions().await.unwrap_or_default();
        let profiles = self.get_all_quality_profiles().await.unwrap_or_default();
        (qualities, profiles)
    }

    pub async fn get_all_quality_profiles(&self) -> Result<HashMap<String, QualityProfile>> {
        self.fetch_json_map("SELECT id, data FROM quality_profiles")
            .await
    }

    pub async fn upsert_quality_profile(&self, id: &str, profile: &QualityProfile) -> Result<()> {
        self.upsert_json_record("quality_profiles", id, profile)
            .await
    }

    pub async fn delete_quality_profile(&self, id: &str) -> Result<()> {
        exec_by_id!(self, "DELETE FROM quality_profiles WHERE id = ?", id)
    }

    pub async fn save_all_quality_definitions(&self, map: &HashMap<String, Quality>) -> Result<()> {
        self.save_json_map("quality_definitions", map).await
    }

    pub async fn save_all_quality_profiles(
        &self,
        map: &HashMap<String, QualityProfile>,
    ) -> Result<()> {
        self.save_json_map("quality_profiles", map).await
    }

    pub async fn save_all_release_profiles(
        &self,
        map: &HashMap<String, ReleaseProfile>,
    ) -> Result<()> {
        self.save_json_map("release_profiles", map).await
    }

    // Config-to-DB: Singleton sections (global_filters, dynamic_profiles, ui)

    async fn get_singleton<T: DeserializeOwned + Default>(&self, key: &str) -> Result<T> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT data FROM config_defaults WHERE key = ?")
                .bind(key)
                .fetch_optional(&self.pool)
                .await?;
        if let Some((data,)) = row {
            Ok(serde_json::from_str(&data).unwrap_or_default())
        } else {
            Ok(T::default())
        }
    }

    async fn save_singleton<T: Serialize>(&self, key: &str, value: &T) -> Result<()> {
        let data = serde_json::to_string(value)?;
        sqlx::query(
            "INSERT INTO config_defaults (key, data) VALUES (?, ?)
             ON CONFLICT(key) DO UPDATE SET data = excluded.data",
        )
        .bind(key)
        .bind(data)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn get_global_filters(&self) -> Result<FilterRule> {
        self.get_singleton("global_filters").await
    }

    pub async fn save_global_filters(&self, filters: &FilterRule) -> Result<()> {
        self.save_singleton("global_filters", filters).await
    }

    pub async fn get_dynamic_profiles(&self) -> Result<DynamicProfilesConfig> {
        self.get_singleton("dynamic_profiles").await
    }

    pub async fn save_dynamic_profiles(&self, config: &DynamicProfilesConfig) -> Result<()> {
        self.save_singleton("dynamic_profiles", config).await
    }

    /// Read the UI preferences singleton.
    ///
    /// Cached in memory (see `ui_prefs_cache`): the first call loads the row,
    /// later calls clone the cached value.  `save_ui_preferences` refreshes it.
    pub async fn get_ui_preferences(&self) -> Result<UIConfig> {
        if let Some(cached) = self.ui_prefs_cache.read().await.clone() {
            return Ok(cached);
        }
        let ui: UIConfig = self.get_singleton("ui_preferences").await?;
        *self.ui_prefs_cache.write().await = Some(ui.clone());
        Ok(ui)
    }

    pub async fn save_ui_preferences(&self, ui: &UIConfig) -> Result<()> {
        self.save_singleton("ui_preferences", ui).await?;
        *self.ui_prefs_cache.write().await = Some(ui.clone());
        Ok(())
    }

    // Config-to-DB: Singleton sections (organization, general, proxy, etc.)

    pub async fn get_organization_config(&self) -> Result<OrganizationConfig> {
        self.get_singleton("organization").await
    }

    pub async fn save_organization_config(&self, cfg: &OrganizationConfig) -> Result<()> {
        self.save_singleton("organization", cfg).await
    }

    pub async fn get_general_config(&self) -> Result<GeneralConfig> {
        self.get_singleton("general").await
    }

    pub async fn save_general_config(&self, cfg: &GeneralConfig) -> Result<()> {
        self.save_singleton("general", cfg).await
    }

    pub async fn get_sources_config(&self) -> Result<SourcesConfig> {
        self.get_singleton("sources").await
    }

    pub async fn save_sources_config(&self, cfg: &SourcesConfig) -> Result<()> {
        self.save_singleton("sources", cfg).await
    }

    pub async fn get_proxy_config(&self) -> Result<ProxyConfig> {
        self.get_singleton("proxy").await
    }

    pub async fn save_proxy_config(&self, cfg: &ProxyConfig) -> Result<()> {
        self.save_singleton("proxy", cfg).await
    }

    pub async fn get_security_config(&self) -> Result<SecurityConfig> {
        self.get_singleton("security").await
    }

    pub async fn save_security_config(&self, cfg: &SecurityConfig) -> Result<()> {
        self.save_singleton("security", cfg).await
    }

    pub async fn get_auth_config(&self) -> Result<AuthConfig> {
        self.get_singleton("auth").await
    }

    pub async fn save_auth_config(&self, cfg: &AuthConfig) -> Result<()> {
        self.save_singleton("auth", cfg).await
    }

    // Default Seeding — populate an empty database with factory defaults

    // Idempotent per-table emptiness check, so "delete all data" re-seeds defaults
    // on restart while existing custom config is left untouched.
    /// Seed the database with default values if the relevant tables are empty.
    /// Called once on first startup after migrations.
    pub async fn seed_defaults(&self) -> Result<()> {
        if self.get_all_quality_definitions().await?.is_empty() {
            tracing::debug!("Seeding default qualities...");
            self.save_all_quality_definitions(&defaults::default_qualities())
                .await?;
        }

        if self.get_all_quality_profiles().await?.is_empty() {
            tracing::debug!("Seeding default quality profiles...");
            self.save_all_quality_profiles(&defaults::default_quality_profiles())
                .await?;
        }

        if self.get_all_release_profiles().await?.is_empty() {
            tracing::debug!("Seeding default release profiles...");
            self.save_all_release_profiles(&defaults::default_release_profiles())
                .await?;
        }

        // Fetch all existing config keys in one query, then seed only the missing ones.
        let existing_keys: std::collections::HashSet<String> =
            sqlx::query_scalar("SELECT key FROM config_defaults")
                .fetch_all(&self.pool)
                .await?
                .into_iter()
                .collect();

        macro_rules! seed_if_missing {
            ($key:expr, $default:expr) => {
                if !existing_keys.contains($key) {
                    tracing::debug!("Seeding default {}...", $key);
                    self.save_singleton($key, &$default()).await?;
                }
            };
        }

        seed_if_missing!("organization", OrganizationConfig::default);
        seed_if_missing!("general", GeneralConfig::default);
        seed_if_missing!("sources", SourcesConfig::default);
        seed_if_missing!("proxy", ProxyConfig::default);
        seed_if_missing!("security", SecurityConfig::default);
        seed_if_missing!("auth", || AuthConfig {
            max_auth_fail_count: AuthConfig::MAX_AUTH_FAIL_COUNT_SEEDED,
            ..Default::default()
        });
        seed_if_missing!("global_filters", FilterRule::default);
        seed_if_missing!("ui_preferences", defaults::default_ui_preferences);

        tracing::info!("Database default seeding complete.");
        Ok(())
    }
}
