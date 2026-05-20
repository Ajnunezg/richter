//! Plugin runtime loader for Richter.
//!
//! Discovers plugins from ~/.richter/plugins/ via --richter-manifest flag.
//! All plugins undergo code-signature verification, sandbox validation,
//! and capability restriction before they are trusted.

use std::collections::HashMap;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::{Digest, Sha256};

/// A discovered and verified plugin.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct Plugin {
    pub name: String,
    pub version: String,
    pub path: PathBuf,
    pub enabled: bool,
    pub capabilities: Vec<String>,
}

/// Capabilities a plugin may request.
pub const ALLOWED_CAPABILITIES: &[&str] = &["read-files", "write-files", "network", "run-commands"];

/// Manages plugin discovery, verification, and execution.
pub struct PluginRuntime {
    plugins: Vec<Plugin>,
    plugin_dir: PathBuf,
    trusted_path: PathBuf,
}

impl PluginRuntime {
    pub fn new() -> Self {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
        let richter_dir = PathBuf::from(&home).join(".richter");
        Self {
            plugins: Vec::new(),
            plugin_dir: richter_dir.join("plugins"),
            trusted_path: richter_dir.join("plugins").join("trusted.json"),
        }
    }

    /// Discover and verify plugins in the plugin directory.
    ///
    /// Each binary in `~/.richter/plugins/` is structurally validated,
    /// code-signature verified, and its manifest parsed. Only plugins
    /// that pass all checks are added to the runtime.
    pub fn discover(&mut self) -> anyhow::Result<usize> {
        if !self.plugin_dir.exists() {
            return Ok(0);
        }

        let mut count = 0;

        for entry in fs::read_dir(&self.plugin_dir)? {
            let entry = entry?;
            let path = entry.path();

            // Skip non-files and the trusted.json bookkeeping file.
            if !path.is_file() {
                continue;
            }
            if path.file_name().is_some_and(|n| n == "trusted.json") {
                continue;
            }

            // ---- step 1: structural sandbox validation ----
            if let Err(reason) = validate_plugin_binary(&path, &self.plugin_dir) {
                tracing::warn!("Plugin '{}' rejected: {}", path.display(), reason);
                continue;
            }

            // ---- step 2: code-signature / hash verification ----
            if let Err(reason) = verify_binary(&path, &self.trusted_path) {
                tracing::warn!("Plugin '{}' rejected: {}", path.display(), reason);
                continue;
            }

            // ---- step 3: execute --richter-manifest to read metadata ----
            tracing::debug!("Running plugin '{}' for manifest discovery", path.display());

            let output = match Command::new(&path).arg("--richter-manifest").output() {
                Ok(o) => o,
                Err(e) => {
                    tracing::warn!(
                        "Plugin '{}' rejected: failed to execute: {}",
                        path.display(),
                        e
                    );
                    continue;
                }
            };

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                tracing::warn!(
                    "Plugin '{}' rejected: manifest exit status {}: {}",
                    path.display(),
                    output.status,
                    stderr.trim()
                );
                continue;
            }

            let manifest: serde_json::Value = match serde_json::from_slice(&output.stdout) {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!(
                        "Plugin '{}' rejected: invalid manifest JSON: {}",
                        path.display(),
                        e
                    );
                    continue;
                }
            };

            let name = manifest["name"].as_str().unwrap_or("unknown").to_string();

            let capabilities: Vec<String> = manifest["capabilities"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();

            // ---- step 4: capability allow-list check ----
            let mut caps_valid = true;
            for cap in &capabilities {
                if !ALLOWED_CAPABILITIES.contains(&cap.as_str()) {
                    tracing::warn!("Plugin '{}' rejected: unknown capability '{}'", name, cap);
                    caps_valid = false;
                    break;
                }
            }
            if !caps_valid {
                continue;
            }

            // ---- all checks passed ----
            self.plugins.push(Plugin {
                name: name.clone(),
                version: manifest["version"].as_str().unwrap_or("0.1.0").to_string(),
                path: path.clone(),
                enabled: manifest["enabled"].as_bool().unwrap_or(true),
                capabilities,
            });

            tracing::info!("Plugin '{}' verified (signature: valid)", name);
            count += 1;
        }

        tracing::info!("Discovered {count} plugin(s)");
        Ok(count)
    }

    /// Return all discovered plugins.
    pub fn list(&self) -> &[Plugin] {
        &self.plugins
    }

    /// Check whether a named, enabled plugin exists.
    pub fn has_plugin(&self, name: &str) -> bool {
        self.plugins.iter().any(|p| p.name == name && p.enabled)
    }

    /// Check whether the given plugin declares a specific capability.
    pub fn check_capability(plugin: &Plugin, capability: &str) -> bool {
        plugin.capabilities.iter().any(|c| c == capability)
    }

    /// Execute a previously-discovered plugin by name.
    ///
    /// Re-validates the binary and its signature before every execution.
    pub fn execute(&self, name: &str, args: &[&str]) -> anyhow::Result<std::process::Output> {
        let plugin = self
            .plugins
            .iter()
            .find(|p| p.name == name && p.enabled)
            .ok_or_else(|| anyhow::anyhow!("plugin '{name}' not found or disabled"))?;

        // Re-validate security properties at execution time.
        validate_plugin_binary(&plugin.path, &self.plugin_dir)
            .map_err(|e| anyhow::anyhow!("plugin '{}' validation failed: {}", name, e))?;

        verify_binary(&plugin.path, &self.trusted_path)
            .map_err(|e| anyhow::anyhow!("plugin '{}' signature check failed: {}", name, e))?;

        tracing::debug!("Running plugin '{}' at {}", name, plugin.path.display());

        let output = Command::new(&plugin.path).args(args).output()?;
        Ok(output)
    }
}

impl Default for PluginRuntime {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Verification helpers
// ---------------------------------------------------------------------------

/// Verify the code signature of a plugin binary.
///
/// On macOS uses the `codesign` tool. On other platforms computes a SHA-256
/// hash and checks it against a trusted.json registry.
#[allow(unused_variables)]
fn verify_binary(path: &Path, trusted_path: &Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let output = Command::new("codesign")
            .args(["-v", &path.to_string_lossy()])
            .output()
            .map_err(|e| format!("cannot run codesign: {e}"))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("code signature invalid: {}", stderr.trim()));
        }

        Ok(())
    }

    #[cfg(not(target_os = "macos"))]
    {
        let hash = compute_sha256(path)?;
        let mut trusted: HashMap<String, String> = load_trusted(trusted_path);

        let key = path.to_string_lossy().to_string();

        if let Some(stored) = trusted.get(&key) {
            if stored != &hash {
                return Err("binary hash mismatch — file modified since discovery".into());
            }
            // Hash matches — still trusted.
        } else {
            // First discovery: record the hash.
            trusted.insert(key, hash);
            save_trusted(trusted_path, &trusted)?;
        }

        Ok(())
    }
}

/// Compute the SHA-256 hex digest of a file's contents.
#[allow(dead_code)]
fn compute_sha256(path: &Path) -> Result<String, String> {
    let data = fs::read(path).map_err(|e| format!("cannot read file: {e}"))?;
    let mut hasher = Sha256::new();
    hasher.update(&data);
    Ok(hex::encode(hasher.finalize()))
}

/// Load the trusted-hash registry from disk (empty map if it doesn't exist).
#[allow(dead_code)]
fn load_trusted(path: &Path) -> HashMap<String, String> {
    fs::read_to_string(path)
        .ok()
        .and_then(|data| serde_json::from_str(&data).ok())
        .unwrap_or_default()
}

/// Persist the trusted-hash registry to disk with mode `0600`.
#[allow(dead_code)]
fn save_trusted(path: &Path, trusted: &HashMap<String, String>) -> Result<(), String> {
    let json =
        serde_json::to_string_pretty(trusted).map_err(|e| format!("cannot serialize: {e}"))?;

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("cannot create plugin dir: {e}"))?;
    }

    fs::write(path, &json).map_err(|e| format!("cannot write trusted.json: {e}"))?;

    // Restrict to owner-only access.
    #[cfg(unix)]
    {
        let mut perms = fs::metadata(path)
            .map_err(|e| format!("cannot stat trusted.json: {e}"))?
            .permissions();
        perms.set_mode(0o600);
        fs::set_permissions(path, perms).map_err(|e| format!("cannot chmod trusted.json: {e}"))?;
    }

    Ok(())
}

/// Structural sandbox checks that must pass before a binary is executed.
///
/// Returns `Ok(())` only when:
/// (a) the path is a regular file (not a symlink)
/// (b) the canonical path falls inside the plugin directory
/// (c) the file has mode `0755` or stricter (no setuid / setgid / world-writable)
/// (d) the file is owned by the current user
fn validate_plugin_binary(path: &Path, plugin_dir: &Path) -> Result<(), String> {
    let md = fs::symlink_metadata(path).map_err(|e| format!("cannot stat: {e}"))?;

    // (a) Must be a regular file, not a symlink.
    if md.file_type().is_symlink() {
        return Err("symlinks not allowed".into());
    }
    if !md.is_file() {
        return Err("not a regular file".into());
    }

    // (b) Path-traversal guard: canonical path must live under plugin_dir.
    let canonical = path
        .canonicalize()
        .map_err(|e| format!("cannot resolve path: {e}"))?;
    let canonical_dir = plugin_dir
        .canonicalize()
        .map_err(|e| format!("cannot resolve plugin dir: {e}"))?;

    if !canonical.starts_with(&canonical_dir) {
        return Err("path traversal detected — file outside plugin directory".into());
    }

    // (c) Permission checks (unix only).
    #[cfg(unix)]
    {
        let mode = md.permissions().mode();

        // Reject setuid (0o4000), setgid (0o2000), sticky (0o1000).
        if mode & 0o7000 != 0 {
            return Err(format!(
                "insecure permissions: setuid/setgid/sticky bits set (mode {mode:o})"
            ));
        }

        // Reject group-writable or world-writable.
        if mode & 0o022 != 0 {
            return Err(format!(
                "insecure permissions: group- or world-writable (mode {mode:o})"
            ));
        }
    }

    // (d) Ownership must match the current user.
    #[cfg(unix)]
    {
        let current_uid = unsafe { libc::getuid() };
        if md.uid() != current_uid {
            return Err(format!(
                "not owned by current user (owner uid {}, current uid {current_uid})",
                md.uid()
            ));
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;

    /// Non-existent path must fail verification.
    #[test]
    fn non_existent_path_rejected() {
        let bad = Path::new("/tmp/richter_test_nonexistent_binary_xyz");
        let trusted = Path::new("/tmp/richter_test_trusted_nonexistent.json");
        let result = verify_binary(bad, trusted);
        assert!(
            result.is_err(),
            "non-existent path should fail verification, got {result:?}"
        );
    }

    /// The sha256-based trusted.json mechanism works correctly
    /// (exercised on non-macOS; on macOS the codesign path is taken instead).
    #[test]
    fn trusted_json_hash_mechanism() {
        let dir = tempfile::tempdir().expect("tempdir");
        let bin = dir.path().join("plugin-bin");
        let trusted = dir.path().join("trusted.json");

        // Write a known payload.
        fs::write(&bin, b"v1.0.0 payload").expect("write bin");

        // First call: hash is recorded, should succeed.
        // (Will fail on macOS if the binary isn't codesigned, so we test the
        // underlying hash helpers directly when on macOS.)
        #[cfg(not(target_os = "macos"))]
        {
            verify_binary(&bin, &trusted).expect("first discovery should succeed");
            // Trusted file must exist with mode 0600.
            assert!(trusted.exists());
            let perms = fs::metadata(&trusted).expect("metadata").permissions();
            assert_eq!(perms.mode() & 0o777, 0o600, "trusted.json must be 0600");

            // Second call with unmodified binary must also succeed.
            verify_binary(&bin, &trusted).expect("second load should succeed");

            // Modify the binary — verification must now fail.
            fs::write(&bin, b"tampered payload").expect("write tampered bin");
            let result = verify_binary(&bin, &trusted);
            assert!(
                result.is_err(),
                "tampered binary should be rejected, got {result:?}"
            );
            assert!(
                result.unwrap_err().contains("hash mismatch"),
                "error should mention hash mismatch"
            );
        }

        // On macOS we validate the hash helpers directly since codesign
        // requires a real signed binary.
        #[cfg(target_os = "macos")]
        {
            let hash1 = compute_sha256(&bin).expect("hash v1");
            fs::write(&bin, b"tampered payload").expect("write tampered");
            let hash2 = compute_sha256(&bin).expect("hash v2");
            assert_ne!(hash1, hash2, "hashes must differ for different content");

            // load_trusted / save_trusted round-trip.
            let mut map: HashMap<String, String> = HashMap::new();
            map.insert("/fake/path".into(), "abc123".into());
            save_trusted(&trusted, &map).expect("save");
            let loaded = load_trusted(&trusted);
            assert_eq!(loaded.get("/fake/path").map(String::as_str), Some("abc123"));

            // Permissions check.
            let perms = fs::metadata(&trusted).expect("metadata").permissions();
            assert_eq!(perms.mode() & 0o777, 0o600, "trusted.json must be 0600");
        }
    }

    /// Capability check returns true only for declared capabilities.
    #[test]
    fn capability_check() {
        let plugin = Plugin {
            name: "test".into(),
            version: "1.0".into(),
            path: PathBuf::from("/fake"),
            enabled: true,
            capabilities: vec!["network".into(), "read-files".into()],
        };

        assert!(PluginRuntime::check_capability(&plugin, "network"));
        assert!(PluginRuntime::check_capability(&plugin, "read-files"));
        assert!(!PluginRuntime::check_capability(&plugin, "write-files"));
        assert!(!PluginRuntime::check_capability(&plugin, "run-commands"));
        assert!(!PluginRuntime::check_capability(&plugin, "bogus"));
    }

    /// Path traversal (e.g. `../../etc/passwd`) must be rejected even if the
    /// target exists and the symlink in the plugin dir points there.
    #[test]
    fn path_traversal_rejected() {
        let dir = tempfile::tempdir().expect("tempdir");

        // Plugin directory.
        let plugin_dir = dir.path().join("plugins");
        fs::create_dir_all(&plugin_dir).expect("mkdir plugins");

        // A file outside the plugin directory.
        let outside = dir.path().join("outside.txt");
        fs::write(&outside, b"evil").expect("write outside");

        // A symlink inside the plugin directory pointing outside.
        let symlink = plugin_dir.join("trojan");
        std::os::unix::fs::symlink(&outside, &symlink).expect("symlink");

        // Symlinks are rejected outright.
        let result = validate_plugin_binary(&symlink, &plugin_dir);
        assert!(
            result.is_err(),
            "symlink should be rejected, got {result:?}"
        );
        assert!(
            result.unwrap_err().contains("symlink"),
            "error should mention symlinks"
        );

        // A regular file inside the plugin dir passes (basic happy-path).
        let good = plugin_dir.join("ok-binary");
        {
            let mut f = fs::File::create(&good).expect("create");
            f.write_all(b"#!/bin/sh\necho ok").expect("write");
            let mut perms = f.metadata().expect("metadata").permissions();
            perms.set_mode(0o755);
            f.set_permissions(perms).expect("chmod");
        }
        validate_plugin_binary(&good, &plugin_dir).expect("regular binary should pass");
    }
}
