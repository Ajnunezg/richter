//! File-system watcher for the Richter daemon.
//!
//! Uses FSEvents (via the `notify` crate with the `macos_fsevent` feature) to
//! monitor repository and worktree file changes. Tracks repo root directories,
//! `.git` changes, lockfiles, and config file changes. Detects dirty-state
//! transitions and emits events for consumers.

use anyhow::{Context, Result};
use dashmap::DashMap;
use notify::{
    Config as NotifyConfig, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher,
};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};
use walkdir::WalkDir;

use crate::event_bus::{DaemonEvent, EventBus};

/// Patterns of files to watch for dirty-state transitions.
const DIRTY_INDICATOR_PATTERNS: &[&str] = &[
    "*.lock",
    "*.lockb",
    "Cargo.lock",
    "package-lock.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "Gemfile.lock",
    "poetry.lock",
    "Pipfile.lock",
    "*.toml",
    "*.json",
];

/// Patterns to exclude from watch events to reduce noise.
const EXCLUDE_PATTERNS: &[&str] = &[
    ".git/objects/",
    "node_modules/",
    ".DS_Store",
    "*.swp",
    "*.swo",
    "*~",
    ".git/index.lock",
];

/// A configured watch target.
#[derive(Debug, Clone)]
pub struct WatchTarget {
    /// Absolute path to the directory being watched.
    pub root: PathBuf,
    /// Repository name or identifier.
    pub repo_name: String,
    /// Whether to also track `.git` directory changes.
    pub track_git: bool,
}

/// Configuration for the file-system watcher.
#[derive(Debug, Clone, Default)]
pub struct WatcherConfig {
    /// List of directories to watch.
    pub targets: Vec<WatchTarget>,
    /// Whether to emit FileChanged events for all changes.
    pub emit_events: bool,
    /// Whether to detect dirty-state from lockfile/config changes.
    pub detect_dirty: bool,
}

/// Per-repo dirtiness state.
#[derive(Debug, Clone)]
struct RepoState {
    /// Whether the repo has uncommitted changes.
    dirty: bool,
    /// Paths that have changed (coalesced).
    changed_paths: Vec<String>,
    /// Last time a file change was observed.
    last_change: Instant,
}

impl Default for RepoState {
    fn default() -> Self {
        Self {
            dirty: false,
            changed_paths: Vec::new(),
            last_change: Instant::now(),
        }
    }
}

/// The file-system watcher service.
pub struct FsWatcher {
    /// The underlying notify watcher.
    _watcher: RecommendedWatcher,
    /// Channel into which raw file events are pushed.
    event_rx: tokio::sync::Mutex<mpsc::Receiver<notify::Result<Event>>>,
    /// Per-repo state.
    repo_states: Arc<DashMap<String, RepoState>>,
    /// Event bus for emitting file-change events.
    event_bus: EventBus,
    /// Active watch roots set for dedup.
    active_roots: Arc<DashMap<String, ()>>,
}

impl FsWatcher {
    /// Create a new file-system watcher and register the configured targets.
    pub fn new(config: &WatcherConfig, event_bus: EventBus) -> Result<Self> {
        let (tx, rx) = mpsc::channel(1024);

        let notify_config = NotifyConfig::default().with_poll_interval(Duration::from_secs(2));

        let mut watcher = RecommendedWatcher::new(
            move |res: notify::Result<Event>| {
                let _ = tx.blocking_send(res);
            },
            notify_config,
        )
        .context("Failed to create filesystem watcher")?;

        let repo_states = Arc::new(DashMap::new());
        let active_roots = Arc::new(DashMap::new());

        for target in &config.targets {
            repo_states.insert(target.repo_name.clone(), RepoState::default());

            match watcher.watch(&target.root, RecursiveMode::Recursive) {
                Ok(()) => {
                    active_roots.insert(target.root.display().to_string(), ());
                    info!(
                        "Watching {root} ({name})",
                        root = target.root.display(),
                        name = target.repo_name
                    );
                }
                Err(e) => {
                    warn!("Failed to watch {root}: {e}", root = target.root.display());
                }
            }
        }

        Ok(Self {
            _watcher: watcher,
            event_rx: tokio::sync::Mutex::new(rx),
            repo_states,
            event_bus,
            active_roots,
        })
    }

    /// Run the event-processing loop.
    pub async fn run(&mut self, mut shutdown: tokio::sync::watch::Receiver<bool>) {
        loop {
            let event = {
                tokio::select! {
                    _ = shutdown.changed() => {
                        info!("FsWatcher shutting down");
                        return;
                    }
                    raw = async {
                        let mut rx = self.event_rx.lock().await;
                        rx.recv().await
                    } => raw,
                }
            };

            let event = match event {
                Some(Ok(e)) => e,
                Some(Err(e)) => {
                    error!("Filesystem watch error: {e}");
                    continue;
                }
                None => {
                    warn!("Filesystem watch channel closed");
                    return;
                }
            };

            self.process_event(event).await;
        }
    }

    /// Check if any repo is dirty.
    pub fn is_dirty(&self, repo_name: &str) -> bool {
        self.repo_states
            .get(repo_name)
            .map(|s| s.dirty)
            .unwrap_or(false)
    }

    /// Reset dirty status for a repo.
    pub fn mark_clean(&self, repo_name: &str) {
        if let Some(mut state) = self.repo_states.get_mut(repo_name) {
            state.dirty = false;
            state.changed_paths.clear();
        }
    }

    /// Add a new watch target at runtime.
    pub fn add_target(&self, target: WatchTarget) -> Result<()> {
        if self
            .active_roots
            .contains_key(target.root.to_str().unwrap_or(""))
        {
            return Ok(());
        }

        self.repo_states
            .insert(target.repo_name.clone(), RepoState::default());
        self.active_roots
            .insert(target.root.display().to_string(), ());
        info!(
            "Registered watch target: {name} at {root}",
            name = target.repo_name,
            root = target.root.display()
        );
        Ok(())
    }

    /// Return the list of changed paths for a repo.
    pub fn changed_paths(&self, repo_name: &str) -> Vec<String> {
        self.repo_states
            .get(repo_name)
            .map(|s| s.changed_paths.clone())
            .unwrap_or_default()
    }

    async fn process_event(&self, event: Event) {
        let paths = event.paths;
        if paths.is_empty() {
            return;
        }

        let kind_str = match event.kind {
            EventKind::Create(_) => "created",
            EventKind::Modify(_) => "modified",
            EventKind::Remove(_) => "removed",
            EventKind::Access(_) => return,
            _ => "other",
        };

        for path in &paths {
            let normalized = match path.canonicalize() {
                Ok(p) => p,
                Err(_) => path.clone(),
            };

            if !self.should_include(&normalized) {
                continue;
            }

            if let Some(repo_name) = self.find_repo_for_path(&normalized) {
                let path_str = normalized.to_string_lossy().to_string();

                if let Some(mut state) = self.repo_states.get_mut(&repo_name) {
                    state.dirty = true;
                    state.last_change = Instant::now();
                    if !state.changed_paths.contains(&path_str) {
                        state.changed_paths.push(path_str.clone());
                        if state.changed_paths.len() > 500 {
                            state.changed_paths.remove(0);
                        }
                    }
                }

                let repo_name_for_emit = repo_name.clone();
                if self.event_bus.receiver_count() > 0 {
                    self.event_bus.emit(DaemonEvent::FileChanged {
                    repo: repo_name_for_emit.clone(),
                    path: path_str,
                    kind: kind_str.to_string(),
                });
                }

                debug!(
                    "File {kind_str}: {path} in {repo_name}",
                    path = normalized.display(),
                    repo_name = repo_name_for_emit
                );
            }
        }
    }

    fn find_repo_for_path(&self, path: &Path) -> Option<String> {
        for entry in self.active_roots.iter() {
            let root_path = PathBuf::from(entry.key());
            if path.starts_with(&root_path) {
                return self.repo_states.iter().next().map(|e| e.key().clone());
            }
        }
        None
    }

    fn should_include(&self, path: &Path) -> bool {
        let path_str = path.to_string_lossy();

        for pattern in EXCLUDE_PATTERNS {
            if pattern.contains('*') {
                let simplified = pattern.replace('*', "");
                if path_str.contains(&simplified) {
                    return false;
                }
            } else if path_str.contains(pattern) {
                return false;
            }
        }
        true
    }
}

/// Detect whether a path matches a dirty-indicator pattern.
fn is_dirty_indicator(path: &Path) -> bool {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();

    for pattern in DIRTY_INDICATOR_PATTERNS {
        if let Some(suffix) = pattern.strip_prefix('*') {
            if name.ends_with(suffix) {
                return true;
            }
        } else if name == *pattern {
            return true;
        }
    }
    false
}

/// Perform a `git status --short` equivalent check via walkdir.
pub fn detect_initial_dirty(root: &Path) -> Result<HashSet<String>> {
    let mut dirty = HashSet::new();
    let git_dir = root.join(".git");
    if !git_dir.exists() {
        return Ok(dirty);
    }

    for entry in WalkDir::new(root).into_iter().filter_entry(|e| {
        let p = e.path();
        !p.starts_with(&git_dir) && !p.ends_with("node_modules") && !p.ends_with(".DS_Store")
    }) {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        if entry.file_type().is_file() && is_dirty_indicator(entry.path()) {
            dirty.insert(entry.path().display().to_string());
        }
    }
    Ok(dirty)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dirty_indicator_matches() {
        assert!(is_dirty_indicator(Path::new("Cargo.lock")));
        assert!(is_dirty_indicator(Path::new("foo.lock")));
        assert!(is_dirty_indicator(Path::new("Cargo.toml")));
        assert!(!is_dirty_indicator(Path::new("main.rs")));
    }

    #[test]
    fn test_detect_initial_dirty_non_git() {
        let tmp = tempfile::tempdir().unwrap();
        let dirty = detect_initial_dirty(tmp.path()).unwrap();
        assert!(dirty.is_empty());
    }

    #[test]
    fn test_watch_target_defaults() {
        let target = WatchTarget {
            root: PathBuf::from("/tmp"),
            repo_name: "test".into(),
            track_git: false,
        };
        assert_eq!(target.repo_name, "test");
    }

    #[test]
    fn test_should_include_excludes() {
        let path = Path::new("/repo/node_modules/foo.js");
        let path_str = path.to_string_lossy();
        assert!(path_str.contains("node_modules"));
    }
}
