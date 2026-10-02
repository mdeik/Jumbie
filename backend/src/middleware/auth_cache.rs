// Short-lived caches for the per-request costs in the auth middleware: reading the
// admin password hash from the DB, running Argon2 verification (tens of ms, resent
// on every Basic-auth call), and resolving a Bearer token to an API key.
//
//   • `password` memoises the DB lookup for a short TTL. Only a *set* password is
//     cached — the disabled (no/empty password) state is NOT, so enabling auth
//     out-of-band takes effect immediately instead of leaving a fail-open window. A
//     query error is never cached, so the caller can still fail closed.
//   • `verified` remembers *successful* verifications; failures are never cached,
//     and entries are keyed with a per-process random key so the in-memory map is
//     not a portable, offline-crackable record of credentials.
//   • `api_keys` caches positive token-hash → key lookups. Misses are not cached (a
//     newly created key works immediately); mutations call `invalidate_api_keys`.
//
// Password caches are dropped whenever the password changes.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use jumbie_shared::config::ApiKey;
use rand::Rng;
use tokio::sync::RwLock;

use crate::db::DbManager;

/// How long a cached password hash is trusted before re-reading the DB.
const PASSWORD_TTL: Duration = Duration::from_secs(60);
/// How long a successful password verification is reused (skips Argon2).
const VERIFIED_TTL: Duration = Duration::from_secs(300);
/// Upper bound on cached verifications (bounds memory).
const VERIFIED_MAX: usize = 256;
/// How long a cached API key lookup is reused.
const API_KEY_TTL: Duration = Duration::from_secs(60);
/// Upper bound on cached API keys (bounds memory).
const API_KEYS_MAX: usize = 1024;

struct PasswordEntry {
    /// Always a non-empty hash — the disabled state is not cached.
    hash: Arc<str>,
    loaded_at: Instant,
}

struct VerifiedEntry {
    /// Keyed digest of the stored password hash this verification was valid for.
    /// When the password changes the tag changes, so stale entries stop matching.
    tag: [u8; 32],
    at: Instant,
}

struct CachedApiKey {
    key: Arc<ApiKey>,
    at: Instant,
}

pub struct AuthCache {
    password: RwLock<Option<PasswordEntry>>,
    verified: Mutex<HashMap<[u8; 32], VerifiedEntry>>,
    api_keys: RwLock<HashMap<String, CachedApiKey>>,
    /// Per-process random key for keyed BLAKE3 digests.
    key: [u8; 32],
}

impl Default for AuthCache {
    fn default() -> Self {
        Self::new()
    }
}

impl AuthCache {
    pub fn new() -> Self {
        let mut key = [0u8; 32];
        rand::rng().fill_bytes(&mut key);
        Self {
            password: RwLock::new(None),
            verified: Mutex::new(HashMap::new()),
            api_keys: RwLock::new(HashMap::new()),
            key,
        }
    }

    /// Drop cached password state (hash + verified credentials). Call whenever the
    /// admin password changes.
    pub async fn invalidate(&self) {
        *self.password.write().await = None;
        if let Ok(mut map) = self.verified.lock() {
            map.clear();
        }
    }

    /// Drop cached API-key lookups. Call whenever API keys are added, edited or
    /// removed.
    pub async fn invalidate_api_keys(&self) {
        self.api_keys.write().await.clear();
    }

    /// Return the admin password hash, using a short-lived cache.
    ///
    /// Errors are propagated (and never cached) so callers can fail closed. The
    /// disabled state is intentionally not cached.
    pub async fn password_hash(&self, db: &DbManager) -> anyhow::Result<Option<Arc<str>>> {
        {
            let guard = self.password.read().await;
            if let Some(entry) = guard.as_ref()
                && entry.loaded_at.elapsed() < PASSWORD_TTL
            {
                return Ok(Some(entry.hash.clone()));
            }
        }

        let hash = db.get_user_password_hash("admin").await?;
        match hash.filter(|h| !h.is_empty()) {
            Some(h) => {
                tracing::trace!("auth cache: password hash miss (loaded from DB)");
                let hash: Arc<str> = Arc::from(h.as_str());
                *self.password.write().await = Some(PasswordEntry {
                    hash: hash.clone(),
                    loaded_at: Instant::now(),
                });
                Ok(Some(hash))
            }
            None => {
                // Do NOT cache the disabled state: an out-of-band password write
                // must take effect on the very next request (no fail-open window).
                *self.password.write().await = None;
                Ok(None)
            }
        }
    }

    /// Verify a submitted password, short-circuiting Argon2 for credentials that
    /// verified successfully within `VERIFIED_TTL`.
    pub fn verify_password(&self, password: &str, stored_hash: &str) -> bool {
        let tag = *blake3::keyed_hash(&self.key, stored_hash.as_bytes()).as_bytes();
        let digest = *blake3::keyed_hash(&self.key, password.as_bytes()).as_bytes();

        if let Ok(map) = self.verified.lock()
            && let Some(entry) = map.get(&digest)
            && entry.tag == tag
            && entry.at.elapsed() < VERIFIED_TTL
        {
            return true;
        }

        let ok = crate::auth_utils::verify_password(password, stored_hash);
        if ok && let Ok(mut map) = self.verified.lock() {
            map.retain(|_, e| e.at.elapsed() < VERIFIED_TTL);
            if map.len() >= VERIFIED_MAX {
                map.clear();
            }
            map.insert(
                digest,
                VerifiedEntry {
                    tag,
                    at: Instant::now(),
                },
            );
        }
        ok
    }

    /// Resolve an API key by its token hash, with a short-lived positive cache.
    ///
    /// Errors are propagated (never cached). Misses are not cached, so a newly
    /// created key works immediately even without an explicit invalidation.
    pub async fn api_key(
        &self,
        db: &DbManager,
        token_hash: &str,
    ) -> anyhow::Result<Option<Arc<ApiKey>>> {
        {
            let guard = self.api_keys.read().await;
            if let Some(entry) = guard.get(token_hash)
                && entry.at.elapsed() < API_KEY_TTL
            {
                return Ok(Some(entry.key.clone()));
            }
        }

        let key = db.get_api_key_by_hash(token_hash).await?;
        let mut map = self.api_keys.write().await;
        match key {
            Some(k) => {
                tracing::trace!("auth cache: api key miss (loaded from DB)");
                if map.len() >= API_KEYS_MAX {
                    map.clear();
                }
                let key = Arc::new(k);
                map.insert(
                    token_hash.to_string(),
                    CachedApiKey {
                        key: key.clone(),
                        at: Instant::now(),
                    },
                );
                Ok(Some(key))
            }
            None => {
                map.remove(token_hash);
                Ok(None)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verify_password_accepts_correct_and_rejects_wrong() {
        let cache = AuthCache::new();
        let hash = crate::auth_utils::hash_password("hunter2").unwrap();

        assert!(cache.verify_password("hunter2", &hash));
        // Second call is served from the verified cache.
        assert!(cache.verify_password("hunter2", &hash));
        // Wrong password is rejected and never cached as valid.
        assert!(!cache.verify_password("wrong", &hash));
    }

    #[tokio::test]
    async fn invalidate_clears_verified_cache() {
        let cache = AuthCache::new();
        let hash = crate::auth_utils::hash_password("hunter2").unwrap();

        assert!(cache.verify_password("hunter2", &hash));
        assert!(
            !cache.verified.lock().unwrap().is_empty(),
            "successful verification should be cached"
        );

        cache.invalidate().await;
        assert!(
            cache.verified.lock().unwrap().is_empty(),
            "invalidate must drop cached verifications"
        );
    }

    #[tokio::test]
    async fn invalidate_api_keys_clears_cache() {
        let cache = AuthCache::new();
        cache.api_keys.write().await.insert(
            "hash".to_string(),
            CachedApiKey {
                key: Arc::new(ApiKey {
                    id: "i".to_string(),
                    name: "n".to_string(),
                    key: "hash".to_string(),
                    prefix: "p".to_string(),
                    scopes: vec![],
                    expires_at: None,
                }),
                at: Instant::now(),
            },
        );
        assert!(!cache.api_keys.read().await.is_empty());

        cache.invalidate_api_keys().await;
        assert!(cache.api_keys.read().await.is_empty());
    }
}
