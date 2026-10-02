// Plugin Manifest Signing
//
// Optional Ed25519 signature verification for external plugin executables. If a
// manifest.json contains `public_key` and `signature`, the executable's BLAKE3
// hash is verified against the signature before spawning the process.
//
// Ed25519 gives small (64-byte) signatures and fast verification via the
// already-present, well-audited `ring` crate. BLAKE3 hashes the executable as a
// single message using the raw binary hash (no hex encoding).
//
// Signature format in manifest.json:
//
// ```json
// {
//   "public_key": "base64url(32-byte Ed25519 public key)",
//   "signature": "base64url(64-byte Ed25519 signature of BLAKE3 hash)",
//   "executable": "plugin.py"
// }
// ```
//
// Both fields are optional. If absent, the plugin runs without signature
// verification (backward compatible). If present but invalid, the plugin
// fails to load with a clear error message.

use anyhow::{Context, Result};
use base64::Engine as _;
use ring::signature::{ED25519, Ed25519KeyPair, KeyPair, UnparsedPublicKey};
use std::path::Path;

/// Verify an Ed25519 signature of a file's BLAKE3 hash.
///
/// `public_key_b64url` is a base64url-encoded 32-byte Ed25519 key;
/// `signature_b64url` is a base64url-encoded 64-byte signature.
pub fn verify_plugin_signature(
    file_path: &Path,
    public_key_b64url: &str,
    signature_b64url: &str,
) -> Result<()> {
    let public_key_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(public_key_b64url)
        .context("Failed to decode public_key: invalid base64url encoding")?;

    if public_key_bytes.len() != 32 {
        anyhow::bail!(
            "Invalid public key length: expected 32 bytes, got {}",
            public_key_bytes.len()
        );
    }

    let signature_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(signature_b64url)
        .context("Failed to decode signature: invalid base64url encoding")?;

    if signature_bytes.len() != 64 {
        anyhow::bail!(
            "Invalid signature length: expected 64 bytes, got {}",
            signature_bytes.len()
        );
    }

    let file_bytes = std::fs::read(file_path)
        .with_context(|| format!("Failed to read executable: {}", file_path.display()))?;

    let hash = blake3::hash(&file_bytes);
    let hash_bytes = hash.as_bytes();

    let peer_public_key = UnparsedPublicKey::new(&ED25519, public_key_bytes);
    peer_public_key
        .verify(hash_bytes, &signature_bytes)
        .map_err(|_| anyhow::anyhow!(
            "Plugin signature verification FAILED for {}. The executable has been modified or the signature is invalid.",
            file_path.display()
        ))?;

    Ok(())
}

/// Generate a new Ed25519 key pair and sign a file's BLAKE3 hash — a packaging
/// utility for plugin authors, NOT used at runtime.
///
/// `private_key_b64url` is a base64url-encoded PKCS#8 v2 private key (as produced
/// by `Ed25519KeyPair::generate_pkcs8`). Returns `(public_key_b64url,
/// signature_b64url)`.
pub fn sign_plugin_executable(
    file_path: &Path,
    private_key_b64url: &str,
) -> Result<(String, String)> {
    let private_key_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(private_key_b64url)
        .context("Failed to decode private key: invalid base64url encoding")?;

    let key_pair = Ed25519KeyPair::from_pkcs8(&private_key_bytes)
        .map_err(|e| anyhow::anyhow!("Invalid Ed25519 PKCS#8 private key: {}", e))?;

    let file_bytes = std::fs::read(file_path)
        .with_context(|| format!("Failed to read executable: {}", file_path.display()))?;

    let hash = blake3::hash(&file_bytes);
    let hash_bytes = hash.as_bytes();

    let signature = key_pair.sign(hash_bytes);

    let public_key_b64 =
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(key_pair.public_key());
    let signature_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(signature.as_ref());

    Ok((public_key_b64, signature_b64))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::signature::KeyPair;
    use std::fs;
    use tempfile::NamedTempFile;

    #[test]
    fn test_sign_and_verify() {
        let rng = ring::rand::SystemRandom::new();
        let private_key = ring::signature::Ed25519KeyPair::generate_pkcs8(&rng).unwrap();
        let key_pair = ring::signature::Ed25519KeyPair::from_pkcs8(private_key.as_ref()).unwrap();
        let public_key = key_pair.public_key();

        let private_key_b64 =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(private_key.as_ref());
        let public_key_b64 =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(public_key.as_ref());

        let tmp = NamedTempFile::new().unwrap();
        fs::write(tmp.path(), b"#!/usr/bin/env python3\nprint('hello')").unwrap();

        let (pk, sig) = sign_plugin_executable(tmp.path(), &private_key_b64).unwrap();
        assert_eq!(pk, public_key_b64);

        verify_plugin_signature(tmp.path(), &public_key_b64, &sig).unwrap();

        // Tamper with the file — verification must fail.
        fs::write(tmp.path(), b"#!/usr/bin/env python3\nprint('evil')").unwrap();
        let result = verify_plugin_signature(tmp.path(), &public_key_b64, &sig);
        assert!(result.is_err(), "Tampered file should fail verification");
        assert!(
            result.unwrap_err().to_string().contains("FAILED"),
            "Error should mention signature verification failure"
        );
    }

    #[test]
    fn test_invalid_key() {
        let tmp = NamedTempFile::new().unwrap();
        fs::write(tmp.path(), b"some data").unwrap();

        let result = verify_plugin_signature(
            tmp.path(),
            "!!!invalid-base64!!!",
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_missing_file() {
        let result = verify_plugin_signature(
            Path::new("/nonexistent/plugin.py"),
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        );
        assert!(result.is_err());
    }
}
