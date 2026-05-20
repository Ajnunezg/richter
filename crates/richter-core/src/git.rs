//! Git repository and worktree detection.
//!
//! Uses `git rev-parse` and `git worktree list` to discover repositories
//! and worktrees. Returns structured data with HEAD SHA, branch name,
//! upstream tracking, and dirty state information.

use anyhow::{anyhow, Context, Result};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Information about a Git repository discovered on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitRepoInfo {
    /// Absolute path to the repository root (top-level).
    pub root: PathBuf,
    /// Absolute path to the git common directory.
    pub git_common_dir: PathBuf,
    /// The HEAD commit SHA.
    pub head_sha: Option<String>,
    /// The current branch name.
    pub branch: Option<String>,
    /// Upstream tracking branch (e.g. `origin/main`).
    pub upstream: Option<String>,
    /// Whether the working tree is dirty.
    pub is_dirty: bool,
    /// List of changed files (relative to repo root).
    pub changed_files: Vec<PathBuf>,
    /// List of untracked files (relative to repo root).
    pub untracked_files: Vec<PathBuf>,
}

/// Information about a Git worktree discovered via `git worktree list`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitWorktreeInfo {
    /// Absolute path to the worktree root.
    pub path: PathBuf,
    /// The HEAD SHA in this worktree.
    pub head_sha: Option<String>,
    /// The branch currently checked out (or "detached").
    pub branch: Option<String>,
    /// Whether this is the main (bare) worktree.
    pub is_main: bool,
    /// Whether the worktree is dirty.
    pub is_dirty: bool,
}

/// Detect whether a directory is inside a Git repository.
pub fn is_git_repo(dir: &Path) -> bool {
    Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .current_dir(dir)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Get the top-level directory of the Git repository containing `dir`.
pub fn git_toplevel(dir: &Path) -> Result<PathBuf> {
    let output = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(dir)
        .output()
        .context("git rev-parse --show-toplevel")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!(
            "git rev-parse --show-toplevel failed: {}",
            stderr.trim()
        ));
    }

    let path_str = String::from_utf8(output.stdout)
        .context("parse toplevel output as utf-8")?
        .trim()
        .to_string();

    Ok(std::fs::canonicalize(path_str)?)
}

/// Get the Git common directory for the repository containing `dir`.
pub fn git_common_dir(dir: &Path) -> Result<PathBuf> {
    let output = Command::new("git")
        .args(["rev-parse", "--git-common-dir"])
        .current_dir(dir)
        .output()
        .context("git rev-parse --git-common-dir")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!(
            "git rev-parse --git-common-dir failed: {}",
            stderr.trim()
        ));
    }

    let path_str = String::from_utf8(output.stdout)
        .context("parse common-dir output as utf-8")?
        .trim()
        .to_string();

    let path = PathBuf::from(path_str);
    if path.is_absolute() {
        Ok(path)
    } else {
        let root = git_toplevel(dir)?;
        Ok(root.join(path))
    }
}

/// Get the HEAD SHA of the repository containing `dir`.
pub fn head_sha(dir: &Path) -> Result<Option<String>> {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(dir)
        .output()
        .context("git rev-parse HEAD")?;

    if output.status.success() {
        let sha = String::from_utf8(output.stdout)
            .context("parse HEAD output as utf-8")?
            .trim()
            .to_string();
        if sha.is_empty() {
            Ok(None)
        } else {
            Ok(Some(sha))
        }
    } else {
        Ok(None)
    }
}

/// Get the current branch name for the repository containing `dir`.
pub fn current_branch(dir: &Path) -> Result<Option<String>> {
    let output = Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(dir)
        .output()
        .context("git rev-parse --abbrev-ref HEAD")?;

    if output.status.success() {
        let branch = String::from_utf8(output.stdout)
            .context("parse branch output as utf-8")?
            .trim()
            .to_string();
        if branch.is_empty() || branch == "HEAD" {
            Ok(None)
        } else {
            Ok(Some(branch))
        }
    } else {
        Ok(None)
    }
}

/// Get the upstream tracking branch (e.g. `origin/main`).
pub fn upstream_branch(dir: &Path) -> Result<Option<String>> {
    let current = match current_branch(dir)? {
        Some(b) => b,
        None => return Ok(None),
    };

    let output = Command::new("git")
        .args([
            "rev-parse",
            "--abbrev-ref",
            &format!("{current}@{{upstream}}"),
        ])
        .current_dir(dir)
        .output()
        .context("git rev-parse upstream")?;

    if output.status.success() {
        let upstream = String::from_utf8(output.stdout)
            .context("parse upstream output as utf-8")?
            .trim()
            .to_string();
        if upstream.is_empty() || upstream.contains("fatal:") {
            Ok(None)
        } else {
            Ok(Some(upstream))
        }
    } else {
        Ok(None)
    }
}

/// Check if the working tree has uncommitted changes (uses status --porcelain
/// which is more portable than diff-index for staged+unstaged).
pub fn is_dirty(dir: &Path) -> Result<bool> {
    let output = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(dir)
        .output()
        .context("git status --porcelain")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(!stdout.trim().is_empty())
}

/// Get the list of changed files (staged and unstaged) relative to repo root.
pub fn changed_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let output = Command::new("git")
        .args(["diff", "--name-only", "HEAD"])
        .current_dir(dir)
        .output()
        .context("git diff --name-only HEAD")?;

    parse_file_list(&output.stdout)
}

/// Get the list of staged files relative to repo root.
pub fn staged_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let output = Command::new("git")
        .args(["diff", "--name-only", "--cached", "HEAD"])
        .current_dir(dir)
        .output()
        .context("git diff --name-only --cached HEAD")?;

    parse_file_list(&output.stdout)
}

/// Get the list of unstaged files relative to repo root.
pub fn unstaged_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let output = Command::new("git")
        .args(["diff", "--name-only"])
        .current_dir(dir)
        .output()
        .context("git diff --name-only")?;

    parse_file_list(&output.stdout)
}

/// Get the list of untracked files relative to repo root.
pub fn untracked_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let output = Command::new("git")
        .args(["ls-files", "--others", "--exclude-standard"])
        .current_dir(dir)
        .output()
        .context("git ls-files --others --exclude-standard")?;

    parse_file_list(&output.stdout)
}

/// List all worktrees for the repository containing `dir`.
pub fn list_worktrees(dir: &Path) -> Result<Vec<GitWorktreeInfo>> {
    let output = Command::new("git")
        .args(["worktree", "list", "--porcelain"])
        .current_dir(dir)
        .output()
        .context("git worktree list --porcelain")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("git worktree list failed: {}", stderr.trim()));
    }

    let text = String::from_utf8(output.stdout).context("parse worktree list output as utf-8")?;

    parse_worktree_porcelain(&text)
}

/// Gather complete repository information for the directory.
pub fn inspect_repo(dir: &Path) -> Result<GitRepoInfo> {
    let root = git_toplevel(dir)?;
    let git_common_dir = git_common_dir(dir)?;
    let head_sha = head_sha(dir).ok().flatten();
    let branch = current_branch(dir).ok().flatten();
    let upstream = upstream_branch(dir).ok().flatten();
    let is_dirty = is_dirty(dir).unwrap_or(false);
    let changed = changed_files(dir).unwrap_or_default();
    let untracked = untracked_files(dir).unwrap_or_default();

    Ok(GitRepoInfo {
        root,
        git_common_dir,
        head_sha,
        branch,
        upstream,
        is_dirty,
        changed_files: changed,
        untracked_files: untracked,
    })
}

/// Discover all Git repositories under a root directory.
pub fn discover_repos(root: &Path) -> Result<Vec<GitRepoInfo>> {
    let mut repos = Vec::new();
    discover_repos_recursive(root, &mut repos, &mut HashSet::new())?;
    Ok(repos)
}

fn discover_repos_recursive(
    dir: &Path,
    repos: &mut Vec<GitRepoInfo>,
    seen: &mut HashSet<PathBuf>,
) -> Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }

    // Check if this directory is a git repo
    if dir.join(".git").exists() || dir.join(".git").is_symlink() {
        match inspect_repo(dir) {
            Ok(info) => {
                if seen.insert(info.root.clone()) {
                    repos.push(info);
                }
                return Ok(());
            }
            Err(_) => {
                // Not a valid git repo, continue scanning
            }
        }
    }

    // Don't recurse into .git directories
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return Ok(()),
    };

    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let path = entry.path();
        if path.is_dir() && path.file_name().is_some_and(|n| n != ".git") {
            discover_repos_recursive(&path, repos, seen)?;
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn parse_file_list(data: &[u8]) -> Result<Vec<PathBuf>> {
    let text = String::from_utf8(data.to_vec()).context("parse file list as utf-8")?;
    if text.trim().is_empty() {
        return Ok(vec![]);
    }
    Ok(text
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .map(PathBuf::from)
        .collect())
}

fn parse_worktree_porcelain(text: &str) -> Result<Vec<GitWorktreeInfo>> {
    let mut worktrees = Vec::new();
    let mut current_path: Option<PathBuf> = None;
    let mut current_head: Option<String> = None;
    let mut current_branch: Option<String> = None;
    let mut current_is_main = false;

    for line in text.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            if let Some(prev_path) = current_path.take() {
                worktrees.push(GitWorktreeInfo {
                    path: prev_path,
                    head_sha: current_head.take(),
                    branch: current_branch.take(),
                    is_main: current_is_main,
                    is_dirty: false,
                });
            }
            current_path = Some(PathBuf::from(path));
        } else if let Some(head) = line.strip_prefix("HEAD ") {
            current_head = Some(head.to_string());
        } else if let Some(branch) = line.strip_prefix("branch ") {
            let branch = branch.strip_prefix("refs/heads/").unwrap_or(branch);
            current_branch = Some(branch.to_string());
        } else if line == "bare" {
            current_is_main = true;
        }
    }

    if let Some(prev_path) = current_path.take() {
        worktrees.push(GitWorktreeInfo {
            path: prev_path,
            head_sha: current_head.take(),
            branch: current_branch.take(),
            is_main: current_is_main,
            is_dirty: false,
        });
    }

    Ok(worktrees)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use tempfile::TempDir;

    fn setup_test_repo() -> (TempDir, PathBuf) {
        let temp = TempDir::new().expect("create temp dir");
        let dir = temp.path().to_path_buf();
        Command::new("git")
            .args(["-c", "init.defaultBranch=main", "init"])
            .current_dir(&dir)
            .output()
            .expect("git init");
        Command::new("git")
            .args(["config", "user.email", "test@richter.local"])
            .current_dir(&dir)
            .output()
            .expect("git config email");
        Command::new("git")
            .args(["config", "user.name", "Richter Test"])
            .current_dir(&dir)
            .output()
            .expect("git config name");
        std::fs::write(dir.join("README.md"), "# test\n").expect("write file");
        Command::new("git")
            .args(["add", "."])
            .current_dir(&dir)
            .output()
            .expect("git add");
        Command::new("git")
            .args(["commit", "-m", "initial"])
            .current_dir(&dir)
            .output()
            .expect("git commit");
        (temp, dir)
    }

    #[test]
    fn test_is_git_repo() {
        let (_temp, dir) = setup_test_repo();
        assert!(is_git_repo(&dir));
        assert!(!is_git_repo(&std::env::temp_dir()));
    }

    #[test]
    fn test_git_toplevel() {
        let (_temp, dir) = setup_test_repo();
        let toplevel = git_toplevel(&dir).expect("toplevel");
        assert_eq!(
            std::fs::canonicalize(&toplevel).unwrap(),
            std::fs::canonicalize(&dir).unwrap()
        );
    }

    #[test]
    fn test_head_sha() {
        let (_temp, dir) = setup_test_repo();
        let sha = head_sha(&dir).expect("head sha");
        assert!(sha.is_some());
        assert_eq!(sha.unwrap().len(), 40);
    }

    #[test]
    fn test_current_branch() {
        let (_temp, dir) = setup_test_repo();
        let branch = current_branch(&dir).expect("branch");
        assert!(branch.is_some());
        let b = branch.unwrap();
        assert!(b == "main" || b == "master");
    }

    #[test]
    fn test_is_dirty_clean() {
        let (_temp, dir) = setup_test_repo();
        let dirty = is_dirty(&dir).expect("is_dirty");
        assert!(!dirty, "repo should be clean after initial commit");
    }

    #[test]
    fn test_is_dirty_after_change() {
        let (_temp, dir) = setup_test_repo();
        std::fs::write(dir.join("README.md"), "# modified\n").expect("write");
        let dirty = is_dirty(&dir).expect("is_dirty after change");
        assert!(dirty);
    }

    #[test]
    fn test_untracked_files() {
        let (_temp, dir) = setup_test_repo();
        std::fs::write(dir.join("untracked.txt"), "hello\n").expect("write");
        let untracked = untracked_files(&dir).expect("untracked");
        assert!(untracked
            .iter()
            .any(|p| p == &PathBuf::from("untracked.txt")));
    }

    #[test]
    fn test_list_worktrees() {
        let (_temp, dir) = setup_test_repo();
        let wts = list_worktrees(&dir).expect("list worktrees");
        assert!(!wts.is_empty());
        let wt_paths: Vec<&PathBuf> = wts.iter().map(|w| &w.path).collect();
        let canon_dir = std::fs::canonicalize(&dir).unwrap();
        assert!(wt_paths
            .iter()
            .any(|p| { std::fs::canonicalize(p).is_ok_and(|cp| cp == canon_dir) }));
    }

    #[test]
    fn test_inspect_repo() {
        let (_temp, dir) = setup_test_repo();
        let info = inspect_repo(&dir).expect("inspect");
        assert_eq!(
            std::fs::canonicalize(&info.root).unwrap(),
            std::fs::canonicalize(&dir).unwrap()
        );
        assert!(info.head_sha.is_some());
        assert!(info.branch.is_some());
    }

    #[test]
    fn test_parse_worktree_porcelain() {
        let sample = "worktree /path/to/repo\nHEAD abc123def456\nbranch refs/heads/main\n\nworktree /path/to/repo/other\nHEAD 789012abc345\nbranch refs/heads/feature\n";
        let wts = parse_worktree_porcelain(sample).expect("parse");
        assert_eq!(wts.len(), 2);
        assert_eq!(wts[0].path, PathBuf::from("/path/to/repo"));
        assert_eq!(wts[0].head_sha.as_deref(), Some("abc123def456"));
        assert_eq!(wts[0].branch.as_deref(), Some("main"));
        assert_eq!(wts[1].path, PathBuf::from("/path/to/repo/other"));
        assert_eq!(wts[1].branch.as_deref(), Some("feature"));
    }
}
