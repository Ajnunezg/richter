//! TLS certificate management for the mobile gateway.
//!
//! Generates self-signed ECDSA P-256 certificates on first startup,
//! stores them in <data_dir>/mobile/, and provides certificate pinning.
//!
//! # Security properties
//! - ECDSA P-256 keys (not RSA) for modern security
//! - TLS 1.3 only (no legacy protocol fallback)
//! - Certificate validity: 365 days
//! - Auto-regeneration on expiry
//! - Private key permissions: 0600
//! - SAN: localhost + 127.0.0.1

use anyhow::Context;
use rcgen::{CertificateParams, DistinguishedName, DnType, Ia5String, IsCa, KeyPair, SanType};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;
use tokio_rustls::TlsAcceptor;
use tracing::{info, warn};
use x509_parser::prelude::FromDer;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Subdirectory within the data directory for mobile TLS material.
const MOBILE_TLS_SUBDIR: &str = "mobile";

/// Certificate filename.
const CERT_FILE: &str = "server.crt";

/// Private key filename.
const KEY_FILE: &str = "server.key";

/// Certificate validity duration in days.
const CERT_VALIDITY_DAYS: u64 = 365;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Configured TLS acceptor + certificate fingerprint for the mobile gateway.
#[derive(Clone)]
pub struct MobileTlsConfig {
    /// TLS acceptor wrapping a rustls ServerConfig.
    pub acceptor: TlsAcceptor,
    /// SHA-256 hex fingerprint of the certificate (for pinning).
    pub cert_fingerprint: String,
}

// ---------------------------------------------------------------------------
// Directory setup
// ---------------------------------------------------------------------------

/// Ensure the mobile TLS directory exists under `data_dir`.
/// Returns the path to the mobile subdirectory.
pub fn ensure_mobile_dir(data_dir: &Path) -> anyhow::Result<PathBuf> {
    let mobile_dir = data_dir.join(MOBILE_TLS_SUBDIR);
    fs::create_dir_all(&mobile_dir).with_context(|| {
        format!(
            "Failed to create mobile TLS directory: {}",
            mobile_dir.display()
        )
    })?;

    // Lock down directory permissions (0700).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = fs::metadata(&mobile_dir)
            .context("Failed to stat mobile TLS directory")?
            .permissions();
        let mode = perms.mode() & 0o777;
        if mode != 0o700 {
            let mut p = perms;
            p.set_mode(0o700);
            fs::set_permissions(&mobile_dir, p)
                .context("Failed to chmod 0700 mobile TLS directory")?;
        }
    }

    Ok(mobile_dir)
}

// ---------------------------------------------------------------------------
// Certificate validation
// ---------------------------------------------------------------------------

/// Check whether the certificate at `cert_path` exists and has not expired.
fn cert_is_valid(cert_path: &Path) -> bool {
    let pem_bytes = match fs::read(cert_path) {
        Ok(b) => b,
        Err(_) => return false,
    };

    let pem = match pem::parse(pem_bytes) {
        Ok(p) => p,
        Err(_) => return false,
    };

    let cert = match x509_parser::certificate::X509Certificate::from_der(pem.contents()) {
        Ok((_, c)) => c,
        Err(_) => return false,
    };

    let now = x509_parser::time::ASN1Time::now();
    cert.validity().is_valid_at(now)
}

// ---------------------------------------------------------------------------
// Certificate generation
// ---------------------------------------------------------------------------

/// Generate a new self-signed ECDSA P-256 certificate and private key.
///
/// Writes `server.crt` and `server.key` into `mobile_dir`.
/// The private key is created with 0600 permissions.
fn generate_cert(mobile_dir: &Path) -> anyhow::Result<(PathBuf, PathBuf)> {
    let cert_path = mobile_dir.join(CERT_FILE);
    let key_path = mobile_dir.join(KEY_FILE);

    // Generate ECDSA P-256 keypair via ring.
    let key_pair = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)
        .context("Failed to generate ECDSA P-256 keypair")?;

    // Build certificate parameters.
    let mut params = CertificateParams::default();

    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, "Richter Mobile Gateway");
    dn.push(DnType::OrganizationName, "Richter");
    params.distinguished_name = dn;

    // Subject Alternative Names: localhost + loopback IP.
    params.subject_alt_names = vec![
        SanType::DnsName(Ia5String::try_from("localhost").unwrap()),
        SanType::IpAddress("127.0.0.1".parse().unwrap()),
    ];

    params.is_ca = IsCa::NoCa;

    // 365-day validity window.
    let now_unix = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let now = time::OffsetDateTime::from_unix_timestamp(now_unix).unwrap();
    params.not_before = now;
    params.not_after = now + time::Duration::days(CERT_VALIDITY_DAYS as i64);

    // Self-sign.
    let cert = params
        .self_signed(&key_pair)
        .context("Failed to generate self-signed certificate")?;

    // --- Write private key with 0600 permissions ---
    let key_pem = key_pair.serialize_pem();
    fs::write(&key_path, key_pem.as_bytes())
        .with_context(|| format!("Failed to write private key to {}", key_path.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&key_path)
            .context("Failed to stat private key for chmod")?
            .permissions();
        perms.set_mode(0o600);
        fs::set_permissions(&key_path, perms)
            .context("Failed to set 0600 permissions on private key")?;
    }

    // --- Write certificate ---
    let cert_pem = cert.pem();
    fs::write(&cert_path, cert_pem.as_bytes())
        .with_context(|| format!("Failed to write certificate to {}", cert_path.display()))?;

    info!(
        "Generated new self-signed TLS certificate (ECDSA P-256, {}d validity)",
        CERT_VALIDITY_DAYS
    );

    Ok((cert_path, key_path))
}

// ---------------------------------------------------------------------------
// Fingerprint
// ---------------------------------------------------------------------------

/// Compute the SHA-256 fingerprint of the certificate at `cert_path`.
pub fn compute_cert_fingerprint(cert_path: &Path) -> anyhow::Result<String> {
    let pem_bytes = fs::read(cert_path)
        .with_context(|| format!("Failed to read certificate: {}", cert_path.display()))?;

    let pem = pem::parse(pem_bytes).context("Failed to parse certificate PEM")?;

    let mut hasher = Sha256::new();
    hasher.update(pem.contents());
    let fingerprint = format!("{:x}", hasher.finalize());

    Ok(fingerprint)
}

// ---------------------------------------------------------------------------
// Main entry point
// ---------------------------------------------------------------------------

/// Set up TLS for the mobile gateway.
///
/// If a valid certificate already exists on disk, loads it.
/// Otherwise generates a new self-signed ECDSA P-256 certificate.
/// Returns the TLS acceptor (rustls ServerConfig -> TlsAcceptor)
/// and the certificate's SHA-256 fingerprint (for pinning and the /cert API).
pub fn setup_tls(data_dir: &Path) -> anyhow::Result<MobileTlsConfig> {
    let mobile_dir = ensure_mobile_dir(data_dir)?;
    let cert_path = mobile_dir.join(CERT_FILE);
    let key_path = mobile_dir.join(KEY_FILE);

    // Generate if missing or expired.
    if !cert_is_valid(&cert_path) {
        if cert_path.exists() {
            warn!("TLS certificate expired or invalid - regenerating");
        }
        generate_cert(&mobile_dir)?;
    }

    // Compute fingerprint.
    let cert_fingerprint = compute_cert_fingerprint(&cert_path)?;

    // --- Load cert and key for rustls ---
    let cert_pem_bytes = fs::read(&cert_path)
        .with_context(|| format!("Failed to read certificate: {}", cert_path.display()))?;
    let key_pem_bytes = fs::read(&key_path)
        .with_context(|| format!("Failed to read private key: {}", key_path.display()))?;

    let certs = rustls_pemfile::certs(&mut cert_pem_bytes.as_slice())
        .collect::<Result<Vec<_>, _>>()
        .context("Failed to parse certificate PEM for rustls")?;

    let key = rustls_pemfile::private_key(&mut key_pem_bytes.as_slice())
        .context("Failed to parse private key PEM for rustls")?
        .context("No private key found in PEM file")?;

    // Build rustls ServerConfig: ring crypto, TLS 1.3 only.
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_protocol_versions(&[&rustls::version::TLS13])
    .context("Failed to set TLS 1.3 as the only protocol version")?
    .with_no_client_auth()
    .with_single_cert(certs, key)
    .context("Failed to build TLS server config")?;

    let acceptor = TlsAcceptor::from(Arc::new(config));

    info!(
        fingerprint = %cert_fingerprint,
        "TLS configured - ECDSA P-256, TLS 1.3",
    );

    Ok(MobileTlsConfig {
        acceptor,
        cert_fingerprint,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_and_load_tls_config() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let config = setup_tls(tmp.path()).expect("setup_tls should succeed");

        // Fingerprint should be 64 hex chars (SHA-256).
        assert_eq!(config.cert_fingerprint.len(), 64);
        assert!(config
            .cert_fingerprint
            .chars()
            .all(|c| c.is_ascii_hexdigit()));

        // Cert and key files should exist.
        let cert_path = tmp.path().join(MOBILE_TLS_SUBDIR).join(CERT_FILE);
        let key_path = tmp.path().join(MOBILE_TLS_SUBDIR).join(KEY_FILE);
        assert!(cert_path.exists(), "server.crt should exist");
        assert!(key_path.exists(), "server.key should exist");

        // Verify content is PEM.
        let cert_pem = fs::read_to_string(&cert_path).expect("read cert");
        assert!(cert_pem.contains("-----BEGIN CERTIFICATE-----"));

        let key_pem = fs::read_to_string(&key_path).expect("read key");
        assert!(key_pem.contains("-----BEGIN PRIVATE KEY-----"));

        // Second call should reuse (no regeneration).
        let config2 = setup_tls(tmp.path()).expect("second setup_tls");
        assert_eq!(config.cert_fingerprint, config2.cert_fingerprint);

        // Fingerprint should match what compute_cert_fingerprint returns.
        let fp = compute_cert_fingerprint(&cert_path).expect("fingerprint");
        assert_eq!(fp, config.cert_fingerprint);
    }

    #[test]
    fn test_ensure_mobile_dir_creates_and_restricts_permissions() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mobile_dir = ensure_mobile_dir(tmp.path()).expect("ensure_mobile_dir");

        assert!(mobile_dir.exists());
        assert_eq!(mobile_dir, tmp.path().join(MOBILE_TLS_SUBDIR));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&mobile_dir)
                .expect("stat")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o700, "directory should be 0700");
        }
    }

    #[test]
    fn test_cert_is_valid_detects_missing() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cert_path = tmp.path().join("nonexistent.crt");
        assert!(!cert_is_valid(&cert_path));
    }
}
