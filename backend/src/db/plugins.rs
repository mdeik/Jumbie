// Plugins are stored as JSON blobs in a unified plugin_instances table rather than
// one table per category: the schema is identical for all types and the `category`
// column routes each row to the right map in PluginsConfig at read time.
//
// Number normalization: JSON doesn't distinguish `0` from `0.0`, and serde_json
// deserializes decimals as f64, which fails on i64 fields. Floats with a zero
// fraction are converted to integers at this DB load boundary.
use super::DbManager;
use anyhow::Result;
use serde_json;

/// Recursively walk a `serde_json::Value` and convert any float with zero fractional
/// part (e.g. `0.0`, `42.0`, `-3.0`) to its integer form.  This is applied at the
/// DB load boundary so that every plugin sees integer JSON numbers where the source
/// config stored an integer, regardless of whether the frontend/DB serialized it
/// as `0` or `0.0`.
fn normalize_json_numbers(val: serde_json::Value) -> serde_json::Value {
    match val {
        serde_json::Value::Number(n) => {
            if let Some(f) = n.as_f64() {
                // If the float is a whole number and fits in i64, convert to integer.
                // This handles `0.0` → `0`, `42.0` → `42`, etc.
                if f.fract() == 0.0 && f >= (i64::MIN as f64) && f <= (i64::MAX as f64) {
                    serde_json::Value::Number(serde_json::Number::from(f as i64))
                } else {
                    serde_json::Value::Number(n)
                }
            } else {
                serde_json::Value::Number(n)
            }
        }
        serde_json::Value::Array(arr) => {
            serde_json::Value::Array(arr.into_iter().map(normalize_json_numbers).collect())
        }
        serde_json::Value::Object(obj) => {
            let mut normalized = serde_json::Map::new();
            for (k, v) in obj {
                normalized.insert(k, normalize_json_numbers(v));
            }
            serde_json::Value::Object(normalized)
        }
        other => other,
    }
}

impl DbManager {
    pub async fn get_plugins_config(&self) -> Result<jumbie_shared::config::PluginsConfig> {
        let rows: Vec<(String, String, String, String)> =
            sqlx::query_as("SELECT category, plugin_id, instance_id, data FROM plugin_instances")
                .fetch_all(&self.pool)
                .await?;

        let mut config = jumbie_shared::config::PluginsConfig::default();
        for (category, plugin_id, instance_id, data) in rows {
            let val: serde_json::Value = match serde_json::from_str(&data) {
                Ok(v) => normalize_json_numbers(v),
                Err(e) => {
                    tracing::warn!(
                        "Failed to deserialize plugin {}/{}/{}: {}",
                        category,
                        plugin_id,
                        instance_id,
                        e
                    );
                    continue;
                }
            };
            match category.as_str() {
                "downloader" => {
                    config
                        .downloader
                        .entry(plugin_id)
                        .or_default()
                        .insert(instance_id, val);
                }
                "notifier" => {
                    config
                        .notifier
                        .entry(plugin_id)
                        .or_default()
                        .insert(instance_id, val);
                }
                "source" => {
                    config
                        .source
                        .entry(plugin_id)
                        .or_default()
                        .insert(instance_id, val);
                }
                "metadata" => {
                    config
                        .metadata
                        .entry(plugin_id)
                        .or_default()
                        .insert(instance_id, val);
                }
                _ => {}
            }
        }
        Ok(config)
    }

    // VALID_TABLES is a security boundary: it prevents SQL injection through the
    // `table` parameter. All three tables share the same schema, so one upsert fits all.
    async fn upsert_plugin_data(
        &self,
        table: &'static str,
        id: &str,
        plugin_id: &str,
        instance_id: &str,
        data: &serde_json::Value,
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    ) -> Result<()> {
        const VALID_TABLES: &[&str] = &["plugin_instances", "plugin_states", "plugin_configs"];
        if !VALID_TABLES.contains(&table) {
            return Err(anyhow::anyhow!("Invalid table name: {}", table));
        }

        let data_str = serde_json::to_string(data)?;

        // plugin_instances has an extra 'category' column handled in the wrapper.
        let query = format!(
            r#"
            INSERT INTO {} (id, plugin_id, instance_id, data)
            VALUES (?, ?, ?, ?)
            ON CONFLICT(id) DO UPDATE SET
                plugin_id = EXCLUDED.plugin_id,
                instance_id = EXCLUDED.instance_id,
                data = EXCLUDED.data
            "#,
            table
        );

        sqlx::query(&query)
            .bind(id)
            .bind(plugin_id)
            .bind(instance_id)
            .bind(data_str)
            .execute(&mut **tx)
            .await?;

        Ok(())
    }

    /// Specialized upsert for plugin_instances which includes the 'category' column
    pub async fn upsert_plugin_instance(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        category: &str,
        plugin_id: &str,
        instance_id: &str,
        val: &serde_json::Value,
    ) -> Result<()> {
        let id = format!("{}/{}/{}", category, plugin_id, instance_id);
        let data = serde_json::to_string(val)?;

        sqlx::query(
            r#"
            INSERT INTO plugin_instances (id, category, plugin_id, instance_id, data)
            VALUES (?, ?, ?, ?, ?)
            ON CONFLICT(id) DO UPDATE SET
                category = EXCLUDED.category,
                plugin_id = EXCLUDED.plugin_id,
                instance_id = EXCLUDED.instance_id,
                data = EXCLUDED.data
            "#,
        )
        .bind(&id)
        .bind(category)
        .bind(plugin_id)
        .bind(instance_id)
        .bind(data)
        .execute(&mut **tx)
        .await?;

        Ok(())
    }

    pub async fn upsert_plugin_state(
        &self,
        id: &str,
        plugin_id: &str,
        instance_id: &str,
        data: &serde_json::Value,
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    ) -> Result<()> {
        self.upsert_plugin_data("plugin_states", id, plugin_id, instance_id, data, tx)
            .await
    }

    pub async fn save_plugin_state(
        &self,
        id: &str,
        plugin_id: &str,
        instance_id: &str,
        data: &serde_json::Value,
    ) -> Result<()> {
        with_transaction!(self.pool, |mut tx| {
            self.upsert_plugin_state(id, plugin_id, instance_id, data, &mut tx)
                .await
        })
    }

    pub async fn get_plugin_state(&self, id: &str) -> Result<Option<serde_json::Value>> {
        let row: Option<(String,)> = sqlx::query_as("SELECT data FROM plugin_states WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;

        match row {
            Some((data,)) => Ok(Some(serde_json::from_str(&data)?)),
            None => Ok(None),
        }
    }

    pub async fn upsert_plugin_config(
        &self,
        id: &str,
        plugin_id: &str,
        instance_id: &str,
        data: &serde_json::Value,
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    ) -> Result<()> {
        self.upsert_plugin_data("plugin_configs", id, plugin_id, instance_id, data, tx)
            .await
    }

    // delete-then-INSERT (not individual upserts): the frontend always saves a
    // complete plugin config, so wiping and re-inserting avoids diffing. The
    // transaction means a crash leaves either the old or the new config.
    pub async fn save_plugins_config(
        &self,
        plugins: &jumbie_shared::config::PluginsConfig,
    ) -> Result<()> {
        with_transaction!(self.pool, |mut tx| {
            sqlx::query("DELETE FROM plugin_instances")
                .execute(&mut *tx)
                .await?;

            for (plugin_id, instances) in &plugins.downloader {
                for (instance_id, val) in instances {
                    self.upsert_plugin_instance(&mut tx, "downloader", plugin_id, instance_id, val)
                        .await?;
                }
            }
            for (plugin_id, instances) in &plugins.notifier {
                for (instance_id, val) in instances {
                    self.upsert_plugin_instance(&mut tx, "notifier", plugin_id, instance_id, val)
                        .await?;
                }
            }
            for (plugin_id, instances) in &plugins.source {
                for (instance_id, val) in instances {
                    self.upsert_plugin_instance(&mut tx, "source", plugin_id, instance_id, val)
                        .await?;
                }
            }
            for (plugin_id, instances) in &plugins.metadata {
                for (instance_id, val) in instances {
                    self.upsert_plugin_instance(&mut tx, "metadata", plugin_id, instance_id, val)
                        .await?;
                }
            }

            Ok(())
        })
    }

    pub async fn save_plugins_section(
        &self,
        section: &str,
        data: &std::collections::HashMap<
            String,
            std::collections::HashMap<String, serde_json::Value>,
        >,
    ) -> Result<()> {
        with_transaction!(self.pool, |mut tx| {
            sqlx::query("DELETE FROM plugin_instances WHERE category = ?")
                .bind(section)
                .execute(&mut *tx)
                .await?;

            for (plugin_id, instances) in data {
                for (instance_id, val) in instances {
                    self.upsert_plugin_instance(&mut tx, section, plugin_id, instance_id, val)
                        .await?;
                }
            }

            Ok(())
        })
    }

    pub async fn delete_plugin_instance(
        &self,
        section: &str,
        plugin_id: &str,
        instance_id: &str,
    ) -> Result<()> {
        let row_id = format!("{}/{}/{}", section, plugin_id, instance_id);
        sqlx::query("DELETE FROM plugin_instances WHERE id = ?")
            .bind(&row_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}
