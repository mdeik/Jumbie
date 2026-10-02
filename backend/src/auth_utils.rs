use anyhow::Result;
use argon2::{
    Argon2,
    password_hash::{PasswordHasher, PasswordVerifier, phc::PasswordHash},
};

// Passwords use Argon2id (OWASP-recommended): bcrypt truncates at 72 bytes and
// PBKDF2 is cheap to brute-force on GPUs/ASICs.

/// Hash a password with Argon2id (default parameters: 64 MiB memory,
/// 3 iterations, 4 parallel threads).
pub fn hash_password(password: &str) -> Result<String> {
    let argon2 = Argon2::default();
    let password_hash = argon2
        .hash_password(password.as_bytes())
        .map_err(|e| anyhow::anyhow!(e))?
        .to_string();
    Ok(password_hash)
}

/// Verify a password against an Argon2 PHC string.
/// Returns `false` (not `Err`) on parse failure — avoids leaking *why*
/// verification failed to callers that log the result.
pub fn verify_password(password: &str, hash: &str) -> bool {
    let parsed_hash = match PasswordHash::new(hash) {
        Ok(h) => h,
        Err(_) => return false,
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed_hash)
        .is_ok()
}

// API keys are high-entropy random strings, so a fast hash (Blake3) suffices;
// Argon2's deliberate slowness is only needed for low-entropy passwords.
//
// The "jb_" prefix is part of the API-key wire format (and is hashed as input),
// so it is hardcoded rather than configurable — changing it would silently
// invalidate every stored key hash.

/// Hash an API key with Blake3.
/// Returns a hex-encoded string suitable for storage and lookup.
pub fn hash_api_key(api_key: &str) -> String {
    let hash = blake3::hash(api_key.as_bytes());
    hash.to_hex().to_string()
}
