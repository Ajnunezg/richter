//! Richter fingerprint: BLAKE3 deterministic command hashing.
//!
//! Incorporates cwd, argv, git HEAD, staged/unstaged diffs,
//! lockfile contents, toolchain versions, and dirty state per ADR-0002.
//!
//! All git and toolchain commands are run asynchronously via
//! `tokio::process::Command`. Independent git queries are dispatched
//! concurrently with `tokio::join!`. Toolchain versions are cached with
//! a 60-second TTL via a static `LazyLock`.

use crate::classifier::ClassifiedCommand;
use std::path::PathBuf;
use std::sync::LazyLock;
use std::sync::Mutex;
use tokio::process::Command;

/// Lockfile patterns to hash (first found).
const LOCKFILES: &[&str] = &[
    "Cargo.lock",
    "package-lock.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "poetry.lock",
    "Pipfile.lock",
    "go.sum",
];

/// Toolchain version cache with a 60-second TTL.
struct ToolchainCache {
    versions: Vec<(String, Vec<u8>)>,
    fetched_at: std::time::Instant,
}

static TOOLCHAIN_CACHE: LazyLock<Mutex<Option<ToolchainCache>>> =
    LazyLock::new(|| Mutex::new(None));

/// TTL for toolchain version cache.
const TOOLCHAIN_TTL: std::time::Duration = std::time::Duration::from_secs(60);

/// Fingerprint result cache: avoids redundant git process spawns when the same
/// command+cwd is fingerprinted within the cache TTL.
struct FingerprintCache {
    /// Map of (identity_hash + cwd) -> fingerprint string.
    entries: std::collections::HashMap<String, (String, std::time::Instant)>,
}

static FP_CACHE: LazyLock<Mutex<FingerprintCache>> = LazyLock::new(|| {
    Mutex::new(FingerprintCache {
        entries: std::collections::HashMap::new(),
    })
});

/// TTL for fingerprint cache entries (short — git state changes frequently).
const FP_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(5);

/// Maximum number of cached fingerprints before eviction.
const FP_CACHE_MAX_ENTRIES: usize = 256;

/// Build a cache key from identity hash + cwd.
fn fp_cache_key(command: &ClassifiedCommand, cwd: &str) -> String {
    use blake3::Hasher;
    let mut h = Hasher::new();
    h.update(command.class.to_string().as_bytes());
    for arg in &command.canonical {
        h.update(arg.as_bytes());
    }
    h.update(cwd.as_bytes());
    hex::encode(h.finalize().as_bytes())[..16].to_string()
}

/// Look up a cached fingerprint. Returns None on miss or expired entry.
fn fp_cache_get(key: &str) -> Option<String> {
    let mut cache = FP_CACHE.lock().unwrap();
    if let Some((fp, instant)) = cache.entries.get(key) {
        if instant.elapsed() < FP_CACHE_TTL {
            return Some(fp.clone());
        }
        cache.entries.remove(key);
    }
    None
}

/// Insert a fingerprint into the cache, evicting oldest entries if at capacity.
fn fp_cache_put(key: String, fingerprint: String) {
    let mut cache = FP_CACHE.lock().unwrap();
    // Evict expired entries first
    cache
        .entries
        .retain(|_, (_, instant)| instant.elapsed() < FP_CACHE_TTL);
    // If still at capacity, evict oldest
    if cache.entries.len() >= FP_CACHE_MAX_ENTRIES {
        if let Some(oldest_key) = cache
            .entries
            .iter()
            .min_by_key(|(_, (_, instant))| *instant)
            .map(|(k, _)| k.clone())
        {
            cache.entries.remove(&oldest_key);
        }
    }
    cache
        .entries
        .insert(key, (fingerprint, std::time::Instant::now()));
}

/// Compute a BLAKE3 256-bit fingerprint asynchronously.
///
/// Results are cached for 5 seconds to avoid redundant git process spawns
/// when the same command+cwd is fingerprinted repeatedly (common during
/// multi-agent bursts).
pub async fn fingerprint(command: &ClassifiedCommand, cwd: &str) -> String {
    let key = fp_cache_key(command, cwd);
    if let Some(cached) = fp_cache_get(&key) {
        return cached;
    }

    let mut h = blake3::Hasher::new();
    hash_identity(&mut h, command);
    hash_cwd(&mut h, cwd);
    hash_git_state(&mut h, cwd).await;
    let result = hex::encode(h.finalize().as_bytes());

    fp_cache_put(key, result.clone());
    result
}

/// BLAKE3 256-bit fingerprint that excludes the working directory path.
///
/// Use this for cross-worktree deduplication: two identical commands in
/// different worktrees of the same repo should match, but commands in
/// completely different repos should not.
pub async fn fingerprint_cross_worktree(command: &ClassifiedCommand, cwd: &str) -> String {
    let mut h = blake3::Hasher::new();
    hash_identity(&mut h, command);
    // Skip cwd — that's the whole point of cross-worktree.
    hash_git_state(&mut h, cwd).await;
    hex::encode(h.finalize().as_bytes())
}

/// Hash command identity (class + argv).
fn hash_identity(h: &mut blake3::Hasher, command: &ClassifiedCommand) {
    h.update(command.class.to_string().as_bytes());
    h.update(b" ");
    for arg in &command.canonical {
        h.update(arg.as_bytes());
        h.update(b" ");
    }
}

/// Hash the working directory path.
fn hash_cwd(h: &mut blake3::Hasher, cwd: &str) {
    h.update(b"CWD:");
    h.update(cwd.as_bytes());
}

/// Hash all git-derived state: HEAD, diffs, lockfiles, toolchains, dirty flag.
///
/// Runs independent git commands concurrently via `tokio::join!`.
async fn hash_git_state(h: &mut blake3::Hasher, cwd: &str) {
    // Run independent git commands concurrently.
    let head_fut = async {
        Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(cwd)
            .output()
            .await
            .ok()
            .filter(|o| o.status.success())
            .map(|o| o.stdout)
    };

    let staged_fut = async {
        Command::new("git")
            .args(["diff", "--cached", "HEAD"])
            .current_dir(cwd)
            .output()
            .await
            .ok()
            .filter(|o| o.status.success())
            .map(|o| o.stdout)
    };

    let unstaged_fut = async {
        Command::new("git")
            .args(["diff", "HEAD"])
            .current_dir(cwd)
            .output()
            .await
            .ok()
            .filter(|o| o.status.success())
            .map(|o| o.stdout)
    };

    let dirty_fut = async {
        Command::new("git")
            .args(["diff-index", "--quiet", "HEAD", "--"])
            .current_dir(cwd)
            .status()
            .await
            .ok()
    };

    let (head, staged, unstaged, dirty_status) =
        tokio::join!(head_fut, staged_fut, unstaged_fut, dirty_fut);

    // git HEAD
    if let Some(stdout) = head {
        h.update(&stdout);
    }

    // staged diff hash
    h.update(b"S:");
    if let Some(stdout) = staged {
        h.update(blake3::hash(&stdout).as_bytes().as_slice());
    }

    // unstaged diff hash
    h.update(b"U:");
    if let Some(stdout) = unstaged {
        h.update(blake3::hash(&stdout).as_bytes().as_slice());
    }

    // lockfile contents (synchronous file read — fast, local FS)
    h.update(b"L:");
    if let Some(path) = find_lockfile(cwd) {
        if let Ok(contents) = std::fs::read(&path) {
            h.update(blake3::hash(&contents).as_bytes().as_slice());
        }
    }

    // toolchain versions (cached with 60s TTL)
    h.update(b"T:");
    let cached_versions = {
        let guard = TOOLCHAIN_CACHE.lock().unwrap();
        if let Some(ref cache) = *guard {
            if cache.fetched_at.elapsed() < TOOLCHAIN_TTL {
                Some(cache.versions.clone())
            } else {
                None
            }
        } else {
            None
        }
    };

    let versions = if let Some(v) = cached_versions {
        v
    } else {
        let fetched = fetch_toolchain_versions().await;
        let mut guard = TOOLCHAIN_CACHE.lock().unwrap();
        *guard = Some(ToolchainCache {
            versions: fetched.clone(),
            fetched_at: std::time::Instant::now(),
        });
        fetched
    };

    for (tool, stdout) in &versions {
        h.update(tool.as_bytes());
        h.update(stdout);
    }

    // dirty indicator
    h.update(b"D:");
    if let Some(status) = dirty_status {
        h.update(if status.success() { b"clean" } else { b"dirty" });
    }
}

/// Fetch toolchain versions concurrently.
async fn fetch_toolchain_versions() -> Vec<(String, Vec<u8>)> {
    let rustc = async {
        Command::new("rustc")
            .arg("--version")
            .output()
            .await
            .ok()
            .filter(|o| o.status.success())
            .map(|o| ("rustc".to_string(), o.stdout.trim_ascii().to_vec()))
    };
    let node = async {
        Command::new("node")
            .arg("--version")
            .output()
            .await
            .ok()
            .filter(|o| o.status.success())
            .map(|o| ("node".to_string(), o.stdout.trim_ascii().to_vec()))
    };
    let go = async {
        Command::new("go")
            .arg("version")
            .output()
            .await
            .ok()
            .filter(|o| o.status.success())
            .map(|o| ("go".to_string(), o.stdout.trim_ascii().to_vec()))
    };

    let (r, n, g) = tokio::join!(rustc, node, go);
    [r, n, g].into_iter().flatten().collect()
}

fn find_lockfile(cwd: &str) -> Option<PathBuf> {
    for name in LOCKFILES {
        let p = std::path::Path::new(cwd).join(name);
        if p.exists() {
            return Some(p);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::classifier::ClassifiedCommand;
    use crate::models::CommandClass;

    fn cmd(cls: CommandClass, tool: &str, args: &[&str]) -> ClassifiedCommand {
        ClassifiedCommand {
            class: cls,
            tool: tool.into(),
            subcommand: args.first().map(|s| s.to_string()),
            is_interactive: false,
            is_destructive: false,
            canonical: std::iter::once(tool.to_string())
                .chain(args.iter().map(|s| s.to_string()))
                .collect(),
        }
    }

    #[tokio::test]
    async fn deterministic() {
        let c = cmd(CommandClass::Test, "cargo", &["test"]);
        assert_eq!(
            fingerprint(&c, "/home/dev/repo").await,
            fingerprint(&c, "/home/dev/repo").await
        );
    }

    #[tokio::test]
    async fn cwd_matters() {
        let c = cmd(CommandClass::Build, "cargo", &["build"]);
        assert_ne!(fingerprint(&c, "/a").await, fingerprint(&c, "/b").await);
    }

    #[tokio::test]
    async fn is_64_hex_chars() {
        let c = cmd(CommandClass::Test, "cargo", &["test"]);
        let fp = fingerprint(&c, "/tmp/test").await;
        assert_eq!(fp.len(), 64);
        assert!(fp.chars().all(|ch| ch.is_ascii_hexdigit()));
    }

    #[tokio::test]
    async fn cross_worktree_ignores_cwd() {
        let c = cmd(CommandClass::Build, "cargo", &["build"]);
        // cross-worktree fingerprints should be equal when only cwd differs
        // (in a test environment git state will be the same for both paths)
        let fp_a = fingerprint_cross_worktree(&c, "/a").await;
        let fp_b = fingerprint_cross_worktree(&c, "/b").await;
        assert_eq!(fp_a, fp_b);
    }

    #[tokio::test]
    async fn cross_worktree_differs_from_full() {
        let c = cmd(CommandClass::Build, "cargo", &["build"]);
        let full = fingerprint(&c, "/tmp/x").await;
        let cross = fingerprint_cross_worktree(&c, "/tmp/x").await;
        // They should differ because the full one includes CWD
        assert_ne!(full, cross);
    }

    #[tokio::test]
    async fn cross_worktree_is_64_hex_chars() {
        let c = cmd(CommandClass::Test, "cargo", &["test"]);
        let fp = fingerprint_cross_worktree(&c, "/tmp/test").await;
        assert_eq!(fp.len(), 64);
        assert!(fp.chars().all(|ch| ch.is_ascii_hexdigit()));
    }

    mod proptest_fingerprint {
        use super::*;
        use proptest::prelude::*;

        fn classified_cmd_strategy() -> impl Strategy<Value = ClassifiedCommand> {
            (
                "[a-z]{3,10}",
                prop::collection::vec("[a-z]{1,8}", 0..5),
                prop::sample::select(&[
                    CommandClass::Build,
                    CommandClass::Test,
                    CommandClass::Lint,
                    CommandClass::Install,
                    CommandClass::DevServer,
                ]),
            )
                .prop_map(|(tool, args, class)| ClassifiedCommand {
                    class,
                    subcommand: args.first().cloned(),
                    is_interactive: false,
                    is_destructive: false,
                    canonical: std::iter::once(tool.clone())
                        .chain(args.iter().map(|s| s.to_string()))
                        .collect(),
                    tool,
                })
        }

        proptest! {
            #[test]
            fn deterministic(cmd in classified_cmd_strategy(), cwd in "[a-z/]{5,20}") {
                let rt = tokio::runtime::Runtime::new().unwrap();
                let fp1 = rt.block_on(fingerprint(&cmd, &cwd));
                let fp2 = rt.block_on(fingerprint(&cmd, &cwd));
                prop_assert_eq!(fp1.clone(), fp2);
                prop_assert_eq!(fp1.len(), 64);
            }

            #[test]
            fn cwd_sensitive(
                cmd in classified_cmd_strategy(),
                cwd1 in "[a-z/]{5,20}",
                cwd2 in "[a-z/]{5,20}",
            ) {
                prop_assume!(cwd1 != cwd2);
                let rt = tokio::runtime::Runtime::new().unwrap();
                let fp1 = rt.block_on(fingerprint(&cmd, &cwd1));
                let fp2 = rt.block_on(fingerprint(&cmd, &cwd2));
                prop_assert_ne!(fp1, fp2);
            }

            #[test]
            fn arg_sensitive(
                tool in "[a-z]{3,10}",
                class in prop::sample::select(&[
                    CommandClass::Build,
                    CommandClass::Test,
                    CommandClass::Lint,
                    CommandClass::Install,
                    CommandClass::DevServer,
                ]),
                args1 in prop::collection::vec("[a-z]{1,8}", 1..4),
                args2 in prop::collection::vec("[a-z]{1,8}", 1..4),
                cwd in "[a-z/]{5,20}",
            ) {
                prop_assume!(args1 != args2);
                let c1 = ClassifiedCommand {
                    class,
                    subcommand: args1.first().cloned(),
                    is_interactive: false,
                    is_destructive: false,
                    canonical: std::iter::once(tool.clone())
                        .chain(args1.iter().map(|s| s.to_string()))
                        .collect(),
                    tool: tool.clone(),
                };
                let c2 = ClassifiedCommand {
                    class,
                    subcommand: args2.first().cloned(),
                    is_interactive: false,
                    is_destructive: false,
                    canonical: std::iter::once(tool.clone())
                        .chain(args2.iter().map(|s| s.to_string()))
                        .collect(),
                    tool,
                };
                let rt = tokio::runtime::Runtime::new().unwrap();
                let fp1 = rt.block_on(fingerprint(&c1, &cwd));
                let fp2 = rt.block_on(fingerprint(&c2, &cwd));
                prop_assert_ne!(fp1, fp2);
            }
        }
    }
}
