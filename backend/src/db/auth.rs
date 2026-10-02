// `password_hash` stores an Argon2id PHC string, never a plaintext password, so
// a DB compromise does not expose credentials (hashing lives in `auth_utils`).
use super::DbManager;
use anyhow::Result;

/// Parse a stored JSON scope list, dropping entries that are no longer known
/// (e.g. a scope removed in a later release) instead of failing the whole list —
/// otherwise one stale scope would silently strip every grant from the key.
fn parse_stored_scopes(raw: &str) -> Vec<jumbie_shared::auth::ApiScope> {
    serde_json::from_str::<Vec<String>>(raw)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|scope| scope.parse().ok())
        .collect()
}

impl DbManager {
    pub async fn get_user_password_hash(&self, username: &str) -> Result<Option<String>> {
        let hash: Option<String> =
            sqlx::query_scalar("SELECT password_hash FROM users WHERE username = ?")
                .bind(username)
                .fetch_optional(&self.pool)
                .await?;
        Ok(hash)
    }

    pub async fn set_user_password_hash(&self, username: &str, password_hash: &str) -> Result<()> {
        // UPSERT avoids a check-then-write race and the "does the user exist?" query.
        sqlx::query(
            "INSERT INTO users (username, password_hash) VALUES (?, ?) ON CONFLICT(username) DO UPDATE SET password_hash = excluded.password_hash",
        )
        .bind(username)
        .bind(password_hash)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn get_api_keys(&self) -> Result<Vec<jumbie_shared::config::ApiKey>> {
        #[derive(sqlx::FromRow)]
        struct ApiKeyRow {
            id: String,
            name: String,
            key_hash: String,
            prefix: String,
            scopes: String,
            expires_at: Option<String>,
        }

        let rows = sqlx::query_as::<_, ApiKeyRow>(
            "SELECT id, name, key_hash, prefix, scopes, expires_at FROM api_keys",
        )
        .fetch_all(&self.pool)
        .await?;

        let mut keys = Vec::with_capacity(rows.len());
        for row in rows {
            keys.push(jumbie_shared::config::ApiKey {
                id: row.id,
                name: row.name,
                key: row.key_hash,
                prefix: row.prefix,
                scopes: parse_stored_scopes(&row.scopes),
                // DB stores canonical naive UTC; the shared/wire type is RFC 3339.
                expires_at: row
                    .expires_at
                    .as_deref()
                    .map(crate::datetime::naive_utc_str_to_rfc3339),
            });
        }
        Ok(keys)
    }

    /// Look up a single API key by its stored hash. Returns `None` if no key
    /// matches. Backed by `idx_api_keys_key_hash`, so an authenticated request
    /// does not load and scan the whole table.
    pub async fn get_api_key_by_hash(
        &self,
        key_hash: &str,
    ) -> Result<Option<jumbie_shared::config::ApiKey>> {
        #[derive(sqlx::FromRow)]
        struct ApiKeyRow {
            id: String,
            name: String,
            key_hash: String,
            prefix: String,
            scopes: String,
            expires_at: Option<String>,
        }

        let row = sqlx::query_as::<_, ApiKeyRow>(
            "SELECT id, name, key_hash, prefix, scopes, expires_at FROM api_keys WHERE key_hash = ?",
        )
        .bind(key_hash)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|row| jumbie_shared::config::ApiKey {
            id: row.id,
            name: row.name,
            key: row.key_hash,
            prefix: row.prefix,
            scopes: parse_stored_scopes(&row.scopes),
            expires_at: row
                .expires_at
                .as_deref()
                .map(crate::datetime::naive_utc_str_to_rfc3339),
        }))
    }

    pub async fn get_api_key_hash(&self, id: &str) -> Result<Option<String>> {
        let hash: Option<String> = sqlx::query_scalar("SELECT key_hash FROM api_keys WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(hash)
    }

    pub async fn insert_api_key(
        &self,
        id: &str,
        name: &str,
        key_hash: &str,
        prefix: &str,
        scopes: &[jumbie_shared::auth::ApiScope],
        expires_at: Option<&str>,
    ) -> Result<()> {
        let scopes_json = serde_json::to_string(scopes).unwrap_or_else(|_| "[]".to_string());
        // Canonicalize to the DB format (naive UTC). The caller passes RFC 3339.
        let expires_at = expires_at.map(crate::datetime::canonicalize_to_db_string);
        sqlx::query(
            "INSERT INTO api_keys (id, name, key_hash, prefix, scopes, expires_at) VALUES (?, ?, ?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET name = excluded.name, key_hash = excluded.key_hash, prefix = excluded.prefix, scopes = excluded.scopes, expires_at = excluded.expires_at"
        )
        .bind(id)
        .bind(name)
        .bind(key_hash)
        .bind(prefix)
        .bind(scopes_json)
        .bind(expires_at.as_deref())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn delete_api_key(&self, id: &str) -> Result<()> {
        sqlx::query("DELETE FROM api_keys WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn get_calendar_tokens(&self) -> Result<Vec<jumbie_shared::config::CalendarToken>> {
        #[derive(sqlx::FromRow)]
        struct CalendarTokenRow {
            id: String,
            name: String,
            token: String,
            include_unmonitored: bool,
            show_as_all_day: bool,
        }

        let rows = sqlx::query_as::<_, CalendarTokenRow>(
            "SELECT id, name, token, include_unmonitored, show_as_all_day FROM calendar_tokens",
        )
        .fetch_all(&self.pool)
        .await?;

        let mut tokens = Vec::with_capacity(rows.len());
        for row in rows {
            tokens.push(jumbie_shared::config::CalendarToken {
                id: row.id,
                name: row.name,
                token: row.token,
                hide_unmonitored: !row.include_unmonitored,
                show_as_all_day: row.show_as_all_day,
            });
        }
        Ok(tokens)
    }

    pub async fn insert_calendar_token(
        &self,
        id: &str,
        name: &str,
        token: &str,
        include_unmonitored: bool,
        show_as_all_day: bool,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO calendar_tokens (id, name, token, include_unmonitored, show_as_all_day) VALUES (?, ?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET name = excluded.name, token = excluded.token, include_unmonitored = excluded.include_unmonitored, show_as_all_day = excluded.show_as_all_day",
        )
        .bind(id)
        .bind(name)
        .bind(token)
        .bind(include_unmonitored)
        .bind(show_as_all_day)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn delete_calendar_token(&self, id: &str) -> Result<()> {
        sqlx::query("DELETE FROM calendar_tokens WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    // Banned IPs use INSERT OR REPLACE semantics so the rate-limiter can call
    // insert_banned_ip on every failed auth attempt without a pre-check; fail_count
    // and ban_count update atomically, and bans are time-bounded by `banned_until`
    // (checked by the auth middleware before rejecting).

    pub async fn get_banned_ips(&self) -> Result<Vec<jumbie_shared::config::BannedIp>> {
        let rows = sqlx::query("SELECT ip, fail_count, ban_count, banned_at, banned_until FROM banned_ips ORDER BY banned_at DESC")
            .fetch_all(&self.pool)
            .await?;

        let mut ips = Vec::new();
        use sqlx::Row;
        for row in rows {
            let ip: String = row.get("ip");
            let fail_count: u32 = row.get("fail_count");
            let ban_count: u32 = row.get("ban_count");
            // DB stores canonical naive UTC; present RFC 3339 to the API/config.
            let banned_at =
                crate::datetime::naive_utc_str_to_rfc3339(&row.get::<String, _>("banned_at"));
            let banned_until: Option<String> = row
                .get::<Option<String>, _>("banned_until")
                .map(|s| crate::datetime::naive_utc_str_to_rfc3339(&s));
            ips.push(jumbie_shared::config::BannedIp {
                ip,
                fail_count,
                ban_count,
                banned_at,
                banned_until,
            });
        }
        Ok(ips)
    }

    pub async fn insert_banned_ip(&self, entry: &jumbie_shared::config::BannedIp) -> Result<()> {
        // Canonicalize to the DB format (naive UTC). The caller passes RFC 3339.
        let banned_at = crate::datetime::canonicalize_to_db_string(&entry.banned_at);
        let banned_until = entry
            .banned_until
            .as_deref()
            .map(crate::datetime::canonicalize_to_db_string);
        sqlx::query(
            "INSERT INTO banned_ips (ip, fail_count, ban_count, banned_at, banned_until) VALUES (?, ?, ?, ?, ?)
             ON CONFLICT(ip) DO UPDATE SET fail_count = excluded.fail_count, ban_count = excluded.ban_count, banned_at = excluded.banned_at, banned_until = excluded.banned_until"
        )
        .bind(&entry.ip)
        .bind(entry.fail_count)
        .bind(entry.ban_count)
        .bind(&banned_at)
        .bind(banned_until.as_deref())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Delete temporary bans whose `banned_until` is before `cutoff_rfc3339`.
    /// Permanent bans (`banned_until IS NULL`) are never deleted. Returns the
    /// number of rows removed. Used by the state reaper to keep the table bounded.
    pub async fn prune_expired_bans(&self, cutoff_rfc3339: &str) -> Result<u64> {
        let cutoff = crate::datetime::canonicalize_to_db_string(cutoff_rfc3339);
        let res = sqlx::query(
            "DELETE FROM banned_ips WHERE banned_until IS NOT NULL AND banned_until < ?",
        )
        .bind(cutoff)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected())
    }

    pub async fn delete_banned_ip(&self, ip: &str) -> Result<()> {
        sqlx::query("DELETE FROM banned_ips WHERE ip = ?")
            .bind(ip)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    // Subnet whitelist is a JSON array in `config_defaults` rather than its own
    // table: the list is small and always read/replaced whole as a singleton.
    pub async fn get_subnet_whitelist(&self) -> Result<Vec<String>> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT data FROM config_defaults WHERE key = ?")
                .bind("subnet_whitelist")
                .fetch_optional(&self.pool)
                .await?;
        if let Some((data,)) = row {
            Ok(serde_json::from_str(&data).unwrap_or_default())
        } else {
            Ok(Vec::new())
        }
    }

    pub async fn save_subnet_whitelist(&self, whitelist: &[String]) -> Result<()> {
        let data = serde_json::to_string(whitelist)?;
        sqlx::query(
            "INSERT INTO config_defaults (key, data) VALUES (?, ?)
             ON CONFLICT(key) DO UPDATE SET data = excluded.data",
        )
        .bind("subnet_whitelist")
        .bind(data)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}
