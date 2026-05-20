//! AES-256-GCM file-level encryption for the Richter SQLite database.
//!
//! Provides key management and authenticated encryption/decryption primitives.
//! The encryption key is derived from a machine-specific 32-byte seed stored
//! at `~/.richter/db.key` (generated on first run with 0600 permissions).
//!
//! **Design note:** Full file-level encryption requires either an encrypted VFS
//! or SQLCipher integration. This module provides key management and crypto
//! primitives that can be composed with a VFS layer in the future.
//! For production deployments, consider using SQLCipher or an encrypted
//! filesystem for the database file.

use aes_gcm::aead::{Aead, OsRng};
use aes_gcm::{AeadCore, Aes256Gcm, Key, KeyInit, Nonce};
use anyhow::Context;
use rand::RngCore;
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Filename for the machine-specific encryption key seed.
const DB_KEY_FILENAME: &str = "db.key";

/// Number of random bytes in the key seed.
const DB_KEY_BYTES: usize = 32;

/// Permissions for the key file: owner read/write only (0600).
const KEY_FILE_MODE: u32 = 0o600;

// ---------------------------------------------------------------------------
// Key management
// ---------------------------------------------------------------------------

/// Returns the path to the database encryption key file.
fn db_key_path(data_dir: &Path) -> PathBuf {
    data_dir.join(DB_KEY_FILENAME)
}

/// Generates a new 32-byte key seed or loads an existing one.
///
/// On first run, generates 32 cryptographically random bytes and writes them
/// to `$data_dir/db.key` with 0600 permissions. On subsequent runs, reads
/// the existing key file. Returns the 256-bit key ready for use with AES-256-GCM.
///
/// # Errors
///
/// Returns an error if the key file cannot be read (permissions, corruption)
/// or if the stored key is the wrong length.
pub fn generate_db_key(data_dir: &Path) -> anyhow::Result<[u8; 32]> {
    let key_path = db_key_path(data_dir);

    if key_path.exists() {
        let key_bytes = fs::read(&key_path).context("failed to read database key file")?;
        if key_bytes.len() != DB_KEY_BYTES {
            anyhow::bail!(
                "database key file has wrong length: expected {DB_KEY_BYTES} bytes, got {}",
                key_bytes.len()
            );
        }
        let mut key = [0u8; DB_KEY_BYTES];
        key.copy_from_slice(&key_bytes);

        // Verify restrictive permissions every time we load the key.
        verify_key_permissions(&key_path)?;

        Ok(key)
    } else {
        // First run: generate a new random key.
        let mut key = [0u8; DB_KEY_BYTES];
        OsRng.fill_bytes(&mut key);

        // Write with restrictive permissions from the start.
        let mut file = fs::File::create(&key_path).context("failed to create database key file")?;
        set_restrictive_permissions(&file, &key_path)?;
        file.write_all(&key)
            .context("failed to write database key file")?;
        // Ensure data is flushed to disk before we trust the key.
        file.sync_all()
            .context("failed to sync database key file")?;

        tracing::info!(
            "Generated new database encryption key at {}",
            key_path.display()
        );

        Ok(key)
    }
}

/// Sets file permissions to 0600 (owner read/write only).
fn set_restrictive_permissions(file: &fs::File, path: &Path) -> anyhow::Result<()> {
    let metadata = file.metadata().context("failed to get key file metadata")?;
    let mut perms = metadata.permissions();
    perms.set_mode(KEY_FILE_MODE);
    fs::set_permissions(path, perms).context("failed to set 0600 permissions on key file")?;
    Ok(())
}

/// Verifies the key file has 0600 permissions, warns and corrects if not.
fn verify_key_permissions(path: &Path) -> anyhow::Result<()> {
    let metadata = fs::metadata(path).context("failed to stat key file")?;
    let mode = metadata.permissions().mode() & 0o777;
    if mode != KEY_FILE_MODE {
        tracing::warn!(
            "Database key file has mode {:o}, correcting to {:o}",
            mode,
            KEY_FILE_MODE
        );
        let mut perms = metadata.permissions();
        perms.set_mode(KEY_FILE_MODE);
        fs::set_permissions(path, perms).context("failed to correct key file permissions")?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Encryption / decryption
// ---------------------------------------------------------------------------

/// Encrypts `plaintext` using AES-256-GCM with a random 96-bit nonce.
///
/// The output format is: `[nonce (12 bytes)] || [ciphertext + GCM tag]`.
/// The nonce is prepended so decryption can extract it.
///
/// # Panics
///
/// Panics if the plaintext is so large that the GCM counter would overflow
/// (~64 GiB). In practice, SQLite page buffers are orders of magnitude smaller.
pub fn encrypt_page(key: &[u8; 32], nonce: &[u8; 12], plaintext: &[u8]) -> Vec<u8> {
    let cipher_key = Key::<Aes256Gcm>::from_slice(key);
    let cipher = Aes256Gcm::new(cipher_key);
    let nonce = Nonce::from_slice(nonce);

    cipher
        .encrypt(nonce, plaintext)
        .expect("AES-256-GCM encryption failed (plaintext likely too large)")
}

/// Encrypts `plaintext` using AES-256-GCM and returns nonce-prepended ciphertext.
///
/// Generates a fresh random 96-bit nonce for each call. The returned buffer is:
/// `[nonce (12 bytes)] || [ciphertext + GCM authentication tag (16 bytes)]`.
pub fn encrypt_with_random_nonce(key: &[u8; 32], plaintext: &[u8]) -> Vec<u8> {
    let cipher_key = Key::<Aes256Gcm>::from_slice(key);
    let cipher = Aes256Gcm::new(cipher_key);
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);

    let ciphertext = cipher
        .encrypt(&nonce, plaintext)
        .expect("AES-256-GCM encryption failed");

    // Prepend nonce to ciphertext.
    let mut output = Vec::with_capacity(nonce.len() + ciphertext.len());
    output.extend_from_slice(&nonce);
    output.extend_from_slice(&ciphertext);
    output
}

/// Decrypts nonce-prepended ciphertext produced by [`encrypt_with_random_nonce`].
///
/// Returns `None` if the ciphertext is too short to contain a nonce, or if
/// authentication fails (wrong key, tampered data).
pub fn decrypt_with_nonce(key: &[u8; 32], data: &[u8]) -> Option<Vec<u8>> {
    if data.len() < 12 {
        return None;
    }

    let (nonce_bytes, ciphertext) = data.split_at(12);
    let nonce = Nonce::from_slice(nonce_bytes);

    let cipher_key = Key::<Aes256Gcm>::from_slice(key);
    let cipher = Aes256Gcm::new(cipher_key);

    cipher.decrypt(nonce, ciphertext).ok()
}

/// Decrypts a page of ciphertext using AES-256-GCM.
///
/// The caller must provide the original 96-bit nonce and the ciphertext
/// (which includes the GCM authentication tag). Returns `None` if
/// authentication fails (wrong key, tampered data).
pub fn decrypt_page(key: &[u8; 32], nonce: &[u8; 12], ciphertext: &[u8]) -> Option<Vec<u8>> {
    if nonce.len() < 12 {
        return None;
    }
    let cipher_key = Key::<Aes256Gcm>::from_slice(key);
    let cipher = Aes256Gcm::new(cipher_key);
    let nonce = Nonce::from_slice(nonce);

    cipher.decrypt(nonce, ciphertext).ok()
}

// ---------------------------------------------------------------------------
// Encryption health check
// ---------------------------------------------------------------------------

/// Verifies that the database encryption key exists, is valid, and the
/// encryption primitives are functional.
///
/// Performs:
/// 1. Key file existence and permission check.
/// 2. Key length validation.
/// 3. Round-trip encryption/decryption smoke test.
pub fn verify_encryption_health(data_dir: &Path) -> anyhow::Result<()> {
    let key = generate_db_key(data_dir)?;

    // Smoke test: encrypt a known plaintext and decrypt it back.
    let plaintext = b"richter-encryption-health-check";
    let encrypted = encrypt_with_random_nonce(&key, plaintext);
    let decrypted = decrypt_with_nonce(&key, &encrypted)
        .ok_or_else(|| anyhow::anyhow!("encryption health check: decryption returned None"))?;

    if decrypted != plaintext {
        anyhow::bail!("encryption health check: round-trip mismatch");
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// Round-trip: encrypt → decrypt with random nonce returns original plaintext.
    #[test]
    fn encrypt_decrypt_roundtrip() {
        let key = [0xABu8; 32];
        let plaintext = b"Hello, Richter! This is a test of AES-256-GCM encryption.";

        let encrypted = encrypt_with_random_nonce(&key, plaintext);
        assert!(
            encrypted.len() > plaintext.len(),
            "ciphertext should include nonce + tag"
        );

        let decrypted = decrypt_with_nonce(&key, &encrypted).expect("decryption should succeed");
        assert_eq!(decrypted, plaintext, "round-trip mismatch");
    }

    /// Decryption with the wrong key must fail.
    #[test]
    fn wrong_key_fails_decryption() {
        let key_a = [0xAAu8; 32];
        let key_b = [0xBBu8; 32];
        let plaintext = b"sensitive data";

        let encrypted = encrypt_with_random_nonce(&key_a, plaintext);
        let decrypted = decrypt_with_nonce(&key_b, &encrypted);

        assert!(
            decrypted.is_none(),
            "decryption with wrong key should return None"
        );
    }

    /// Tampered ciphertext must fail GCM authentication.
    #[test]
    fn tampered_ciphertext_fails_authentication() {
        let key = [0xCCu8; 32];
        let plaintext = b"tamper me";

        let mut encrypted = encrypt_with_random_nonce(&key, plaintext);

        // Flip a bit in the ciphertext portion (after the 12-byte nonce).
        if encrypted.len() > 13 {
            encrypted[13] ^= 0x01;
        } else {
            // If somehow the buffer is tiny, flip last byte.
            let last = encrypted.len() - 1;
            encrypted[last] ^= 0x01;
        }

        let decrypted = decrypt_with_nonce(&key, &encrypted);
        assert!(
            decrypted.is_none(),
            "tampered ciphertext should fail authentication"
        );
    }

    /// Truncated ciphertext (no nonce) must fail.
    #[test]
    fn too_short_ciphertext_fails() {
        let key = [0xDDu8; 32];
        let short = vec![0x00; 5]; // shorter than 12-byte nonce

        let result = decrypt_with_nonce(&key, &short);
        assert!(result.is_none(), "too-short input should return None");
    }

    /// Key generation creates a file with 0600 permissions.
    #[test]
    fn key_generation_creates_0600_file() {
        let tmp = tempfile::tempdir().expect("create temp dir");
        let data_dir = tmp.path();

        let key = generate_db_key(data_dir).expect("generate key");
        assert_eq!(key.len(), 32);
        // Key should be non-zero (random).
        assert!(key.iter().any(|&b| b != 0), "key should not be all zeros");

        let key_path = db_key_path(data_dir);
        assert!(key_path.exists(), "key file must exist after generation");

        let metadata = fs::metadata(&key_path).expect("stat key file");
        let mode = metadata.permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o600,
            "key file should have 0600 permissions, got {:o}",
            mode
        );
    }

    /// Loading an existing key returns the same value (idempotent).
    #[test]
    fn key_generation_is_idempotent() {
        let tmp = tempfile::tempdir().expect("create temp dir");
        let data_dir = tmp.path();

        let key1 = generate_db_key(data_dir).expect("generate first key");
        let key2 = generate_db_key(data_dir).expect("load existing key");

        assert_eq!(
            key1, key2,
            "loading an existing key should return the same value"
        );
    }

    /// Decrypt_page with explicit nonce parameter works correctly.
    #[test]
    fn decrypt_page_with_explicit_nonce() {
        let key = [0xEEu8; 32];
        let nonce = [0x42u8; 12];
        let plaintext = b"explicit nonce test data";

        let ciphertext = encrypt_page(&key, &nonce, plaintext);
        let decrypted =
            decrypt_page(&key, &nonce, &ciphertext).expect("decrypt_page should succeed");

        assert_eq!(decrypted, plaintext);
    }
}
