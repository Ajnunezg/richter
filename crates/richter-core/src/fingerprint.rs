//! Richter fingerprint module: deterministic command hashing for cache lookups.
//!
//! Computes a stable fingerprint from a classified command and its execution
//! context (working directory, environment variables, filesystem state).

use crate::classifier::ClassifiedCommand;
#[allow(unused_imports)]
use crate::models::CommandClass;
use sha2::{Digest, Sha256};

/// Computes a fingerprint for a classified command.
///
/// The fingerprint incorporates the canonical argument vector, the command
/// class, working directory, HEAD SHA, and dirty-tree status to produce a
/// stable hash suitable for cache lookup.
pub fn fingerprint(command: &ClassifiedCommand, cwd: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(cwd.as_bytes());
    hasher.update(b"\x00");
    hasher.update(command.class.to_string().as_bytes());
    hasher.update(b"\x00");
    for arg in &command.canonical {
        hasher.update(arg.as_bytes());
        hasher.update(b"\x00");
    }
    // Include git HEAD SHA if available
    if let Ok(head) = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(cwd)
        .output()
    {
        if head.status.success() {
            hasher.update(&head.stdout);
        }
    }
    // Include dirty tree indicator
    // We check via git diff-index to see if worktree is dirty
    if let Ok(diff) = std::process::Command::new("git")
        .args(["diff-index", "--quiet", "HEAD", "--"])
        .current_dir(cwd)
        .status()
    {
        hasher.update(if diff.success() { b"clean" } else { b"dirty" });
    }
    let result = hasher.finalize();
    hex::encode(&result[..8])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::classifier::ClassifiedCommand;

    #[test]
    fn test_same_command_same_fingerprint() {
        let cmd = ClassifiedCommand {
            class: CommandClass::Test,
            tool: "cargo".into(),
            subcommand: Some("test".into()),
            is_interactive: false,
            is_destructive: false,
            canonical: vec!["cargo".into(), "test".into()],
        };
        let fp1 = fingerprint(&cmd, "/home/dev/repo");
        let fp2 = fingerprint(&cmd, "/home/dev/repo");
        assert_eq!(fp1, fp2);
    }

    #[test]
    fn test_different_cwd_different_fingerprint() {
        let cmd = ClassifiedCommand {
            class: CommandClass::Build,
            tool: "cargo".into(),
            subcommand: Some("build".into()),
            is_interactive: false,
            is_destructive: false,
            canonical: vec!["cargo".into(), "build".into()],
        };
        let fp1 = fingerprint(&cmd, "/repo/a");
        let fp2 = fingerprint(&cmd, "/repo/b");
        assert_ne!(fp1, fp2);
    }
}
