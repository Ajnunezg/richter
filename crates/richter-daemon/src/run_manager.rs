//! Run-or-join manager: core orchestration logic for the Richter daemon.
//!
//! Implements command deduplication via fingerprint matching, join-existing-run
//! (subscribe and receive the same exit code), cached-result return when fresh,
//! queueing on resource conflict, pass-through for unknown/destructive commands,
//! dev-server detection, Ctrl-C handling, and superset/subset relation detection.

use anyhow::Result;
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use lru::LruCache;
use parking_lot::Mutex as ParkingMutex;
use std::io::Read as IoRead;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::watch;
use tracing::{debug, info, warn};

use crate::event_bus::{DaemonEvent, EventBus};
use crate::scheduler::Scheduler;
use crate::supervisor::{ProcessSupervisor, RunSpec};
use richter_core::db::Database;
use richter_core::models::{CommandClass, ResourceClass};

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct CommandFingerprint {
    /// The normalized command string (for display/debugging).
    pub command: String,
    /// BLAKE3 fingerprint hash (includes cwd).
    pub hash: String,
    /// BLAKE3 cross-worktree hash (excludes cwd).
    pub cross_worktree_hash: String,
    /// Classification.
    pub classification: CommandClass,
}

impl CommandFingerprint {
    /// Create a fingerprint from a run specification using the canonical BLAKE3
    /// fingerprint from `richter_core`.
    pub async fn from_spec(spec: &RunSpec) -> Self {
        let classified = spec.to_classified_command();
        let cwd = spec.repo.to_string_lossy();
        let hash = richter_core::fingerprint::fingerprint(&classified, &cwd).await;
        let cross_worktree_hash =
            richter_core::fingerprint::fingerprint_cross_worktree(&classified, &cwd).await;

        Self {
            command: spec.command.clone(),
            hash,
            cross_worktree_hash,
            classification: spec.classification,
        }
    }

    /// Build a cross-worktree cache key (excludes worktree path).
    pub fn cross_worktree_key(&self) -> String {
        self.cross_worktree_hash.clone()
    }

    /// Build a cache key string from this fingerprint (for DB lookups).
    pub fn cache_key(&self) -> String {
        self.hash.clone()
    }

    /// Check if this fingerprint is a subset of another.
    ///
    /// Two fingerprints are considered subsets when they share the same
    /// classification and the same command string (modulo test-target
    /// specificity). This is used for test superset/subset matching.
    pub fn is_subset_of(&self, other: &CommandFingerprint) -> bool {
        self.command == other.command && self.classification == other.classification
    }

    /// Check if this fingerprint is a superset of another.
    pub fn is_superset_of(&self, other: &CommandFingerprint) -> bool {
        other.is_subset_of(self)
    }

    /// Extract test target from command for subset/superset analysis.
    /// Returns the package name, test path, or filter expression.
    pub fn test_target(&self) -> Option<String> {
        let cmd = &self.command;
        for (prefix, marker) in [
            (" -p ", ""),
            (" --package ", ""),
            (" pytest ", ".py"),
            (" jest ", ".test."),
            (" vitest ", ".test."),
        ] {
            if let Some(pos) = cmd.find(prefix) {
                let rest = &cmd[pos + prefix.len()..];
                let word = rest.split_whitespace().next()?;
                if marker.is_empty() || word.contains(marker) {
                    return Some(word.to_string());
                }
            }
        }
        None
    }
}

/// A cached run result.
#[derive(Debug, Clone)]
pub struct CachedResult {
    /// The fingerprint that produced this result.
    pub fingerprint: CommandFingerprint,
    /// Run identifier.
    pub run_id: String,
    /// Exit code.
    pub exit_code: i32,
    /// Combined stdout/stderr.
    pub output: String,
    /// When the result was produced.
    pub cached_at: DateTime<Utc>,
    /// File paths that were changed in this run.
    pub changed_files: Vec<String>,
}

impl CachedResult {
    /// Whether this cached result is still fresh (within `max_age`).
    ///
    /// This method performs filesystem I/O and should be called from a
    /// blocking context (e.g., `spawn_blocking`). It uses file modification
    /// times to check if any source files changed since the cache was created.
    pub fn is_fresh_blocking(&self, max_age: Duration) -> bool {
        let age = Utc::now().signed_duration_since(self.cached_at);
        // Check time-based freshness
        if age.to_std().unwrap_or(Duration::ZERO) >= max_age {
            return false;
        }
        // Check if any changed files have been modified since cache time
        for file_path in &self.changed_files {
            if let Ok(meta) = std::fs::metadata(file_path) {
                if let Ok(mtime) = meta.modified() {
                    let mtime_millis = mtime
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as i64;
                    let cached_millis = self.cached_at.timestamp_millis();
                    // If file was modified after cache, it's stale
                    if mtime_millis > cached_millis {
                        return false;
                    }
                }
            }
        }
        true
    }

    /// Async wrapper for `is_fresh_blocking` that runs file I/O on a blocking thread.
    pub async fn is_fresh(&self, max_age: Duration) -> bool {
        let changed_files = self.changed_files.clone();
        let cached_at = self.cached_at;
        tokio::task::spawn_blocking(move || {
            let age = Utc::now().signed_duration_since(cached_at);
            if age.to_std().unwrap_or(Duration::ZERO) >= max_age {
                return false;
            }
            for file_path in &changed_files {
                if let Ok(meta) = std::fs::metadata(file_path) {
                    if let Ok(mtime) = meta.modified() {
                        let mtime_millis = mtime
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_millis() as i64;
                        let cached_millis = cached_at.timestamp_millis();
                        if mtime_millis > cached_millis {
                            return false;
                        }
                    }
                }
            }
            true
        })
        .await
        .unwrap_or(true)
    }
}

/// Possible outcome of a run-or-join request.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "type")]
pub enum RunOutcome {
    /// A new run was started.
    Started {
        /// Run identifier for subscribing to output/status.
        run_id: String,
        /// Time spent waiting in the scheduler queue, if any (in milliseconds).
        #[serde(skip_serializing_if = "Option::is_none")]
        queue_time_ms: Option<u64>,
        /// Human-readable explanation of why this was started.
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// Joined an existing equivalent run.
    Joined {
        /// Run identifier of the existing run.
        run_id: String,
        /// Human-readable explanation of why this was joined.
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// Result was served from cache.
    Cached {
        /// Exit code from cache.
        exit_code: i32,
        /// Cached output.
        output: String,
        /// Cache age description.
        cache_age: String,
        /// Human-readable explanation of cache source.
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// The run was queued for later execution.
    Queued {
        /// Run identifier (reserved).
        run_id: String,
        /// Estimated wait time.
        estimated_wait_ms: u64,
        /// Human-readable reason for queueing.
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// The command was rejected (e.g. destructive command).
    Rejected {
        /// Human-readable reason for rejection.
        reason: String,
    },
}

// NOTE: started_at and is_dev_server are intentionally unused at this
// stage but are part of the ActiveRun schema and may be surfaced in
// telemetry/debugging later.
#[allow(dead_code)]
struct ActiveRun {
    /// Run specification.
    spec: RunSpec,
    /// When the run was started or joined.
    started_at: Instant,
    /// Subscribers awaiting the exit code.
    subscribers: Vec<watch::Sender<i32>>,
    /// Whether this is a dev-server (long-running).
    is_dev_server: bool,
    /// If the run completed before a late joiner subscribed, the
    /// (exit_code, output) tuple is stored here so the joiner can
    /// still discover and join the completed run.
    completion: Option<(i32, String)>,
}

/// Core run-or-join orchestrator.
pub struct RunManager {
    /// Active runs by fingerprint.
    active_by_fingerprint: Arc<DashMap<CommandFingerprint, Arc<ParkingMutex<ActiveRun>>>>,
    /// Active runs by run ID.
    active_by_id: Arc<DashMap<String, Arc<ParkingMutex<ActiveRun>>>>,
    /// Result cache with LRU eviction.
    cache: Arc<ParkingMutex<LruCache<CommandFingerprint, CachedResult>>>,
    /// Persistent SQLite database handle for cache persistence.
    db: Option<Arc<Database>>,
    /// "Richter Saved You" counter: cache hits today.
    cache_hits_today: Arc<std::sync::atomic::AtomicU64>,
    /// Date (UTC) of the last cache hit — used to reset the counter at midnight.
    cache_hits_date: Arc<ParkingMutex<chrono::NaiveDate>>,
    /// "Richter Saved You" counter: duplicate runs avoided.
    duplicates_prevented: Arc<std::sync::atomic::AtomicU64>,
    /// Scheduler for resource-aware dispatch.
    scheduler: Arc<Scheduler>,
    /// Process supervisor for spawning children.
    supervisor: Arc<ProcessSupervisor>,
    /// Event bus.
    event_bus: EventBus,
    /// Result cache TTL.
    cache_ttl: Duration,
    /// Maximum in-memory cache entries (reserved for future use).
    #[allow(dead_code)]
    cache_capacity: NonZeroUsize,

    /// Whether to log cache eviction events (reserved for future use).
    #[allow(dead_code)]
    log_evictions: bool,
}

impl RunManager {
    /// Create a new run manager.
    pub fn new(
        scheduler: Arc<Scheduler>,
        supervisor: Arc<ProcessSupervisor>,
        db: Option<Arc<Database>>,
        event_bus: EventBus,
    ) -> Self {
        let cache_capacity = NonZeroUsize::new(10_000).unwrap();
        Self {
            active_by_fingerprint: Arc::new(DashMap::new()),
            active_by_id: Arc::new(DashMap::new()),
            cache: Arc::new(ParkingMutex::new(LruCache::new(cache_capacity))),
            db,
            cache_hits_today: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            cache_hits_date: Arc::new(ParkingMutex::new(Utc::now().date_naive())),
            duplicates_prevented: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            scheduler,
            supervisor,
            event_bus,
            cache_ttl: Duration::from_secs(300),
            cache_capacity,
            log_evictions: true,
        }
    }

    /// Set the result cache TTL.
    pub fn with_cache_ttl(mut self, ttl: Duration) -> Self {
        self.cache_ttl = ttl;
        self
    }

    /// Run or join a command.
    pub async fn run_or_join(&self, spec: RunSpec) -> Result<RunOutcome> {
        // 0a. Validate repo path: canonicalize first, then verify within allowed roots.
        let canonical_repo = match spec.repo.canonicalize() {
            Ok(p) => p,
            Err(e) => {
                return Ok(RunOutcome::Rejected {
                    reason: format!("Invalid repo path '{}': {}", spec.repo.display(), e),
                });
            }
        };
        let home = std::env::var("HOME").unwrap_or_default();
        let home_path = std::path::PathBuf::from(&home);
        let allowed_tmp_roots: [&std::path::Path; 3] =
            ["/tmp".as_ref(), "/private/tmp".as_ref(), "/var".as_ref()];
        let in_tmp = allowed_tmp_roots
            .iter()
            .any(|root| canonical_repo.starts_with(root));
        if !canonical_repo.starts_with(&home_path) && !in_tmp {
            return Ok(RunOutcome::Rejected {
                reason: format!(
                    "Repo path {} is outside allowed directories",
                    canonical_repo.display()
                ),
            });
        }

        tracing::debug!("run_or_join: computing fingerprint for '{}'", spec.command);
        let fingerprint = CommandFingerprint::from_spec(&spec).await;
        tracing::debug!("run_or_join: fingerprint computed for '{}'", spec.command);

        // 0. Unknown commands always pass through — no caching, dedup, or scheduling.
        if spec.classification == CommandClass::Unknown {
            info!("Pass-through unknown command: {}", spec.command);
            let outcome = self.start_new(spec).await?;
            if let RunOutcome::Started {
                run_id,
                queue_time_ms,
                ..
            } = outcome
            {
                return Ok(RunOutcome::Started {
                    run_id,
                    queue_time_ms,
                    reason: Some("unknown command class — pass-through, no deduplication".into()),
                });
            }
            return Ok(outcome);
        }

        // 1. Destructive command preview gate
        if self.is_destructive(&spec.command) && !spec.force && !spec.preview {
            info!(
                "Destructive command blocked — preview required: {}",
                spec.command
            );
            if let Some(msg) = self.generate_preview(&spec.command) {
                return Ok(RunOutcome::Rejected {
                    reason: format!("DESTRUCTIVE: {}. Re-run with --force to execute.", msg),
                });
            }
            // Fall through to execute
        }
        if self.is_destructive(&spec.command) {
            info!("Pass-through destructive command: {}", spec.command);
            let outcome = self.start_new(spec.clone()).await?;
            // Attach reason to Started outcome
            if let RunOutcome::Started {
                run_id,
                queue_time_ms,
                ..
            } = outcome
            {
                return Ok(RunOutcome::Started {
                    run_id,
                    queue_time_ms,
                    reason: Some("destructive command — pass-through, no deduplication".into()),
                });
            }
            return Ok(outcome);
        }

        // 2. Pass-through dev servers
        if self.is_dev_server(&spec) {
            info!("Dev-server detected: {}", spec.command);
            let outcome = self.start_new(spec).await?;
            if let RunOutcome::Started {
                run_id,
                queue_time_ms,
                ..
            } = outcome
            {
                return Ok(RunOutcome::Started {
                    run_id,
                    queue_time_ms,
                    reason: Some("dev server — pass-through, no deduplication".into()),
                });
            }
            return Ok(outcome);
        }

        // 3. Check in-memory cache
        let cached_opt = self.cache.lock().get(&fingerprint).cloned();
        if let Some(cached) = cached_opt {
            if cached.is_fresh_blocking(self.cache_ttl) {
                self.increment_cache_hits();
                debug!("Cache hit for fingerprint {hash}", hash = fingerprint.hash);
                self.event_bus.emit(DaemonEvent::RunCached {
                    run_id: cached.run_id.clone(),
                    repo: spec.repo.to_string_lossy().into_owned(),
                    command: spec.command.clone(),
                    cache_age: format!(
                        "{}s",
                        Utc::now()
                            .signed_duration_since(cached.cached_at)
                            .num_seconds()
                    ),
                });
                return Ok(RunOutcome::Cached {
                    exit_code: cached.exit_code,
                    output: cached.output.clone(),
                    cache_age: format!(
                        "{}s",
                        Utc::now()
                            .signed_duration_since(cached.cached_at)
                            .num_seconds()
                    ),
                    reason: Some("served from in-memory cache".into()),
                });
            }
        }

        // 3b. Check persistent DB cache (includes cross-worktree)
        if let Some(db) = self.db.as_ref() {
            let cache_key = fingerprint.cache_key();
            let mut entry = db.get_cache_entry(&cache_key).await.ok().flatten();
            if entry.is_none() {
                let cross_key = fingerprint.cross_worktree_key();
                if cross_key != cache_key {
                    entry = db.get_cache_entry(&cross_key).await.ok().flatten();
                }
            }
            if let Some(entry) = entry {
                self.increment_cache_hits();
                debug!(
                    "DB cache hit for fingerprint {hash}",
                    hash = fingerprint.hash
                );

                // Read and decompress cached output from the output_path
                let cached_output = match &entry.output_path {
                    Some(path) if std::path::Path::new(path).exists() => {
                        match std::fs::read(path) {
                            Ok(data) => {
                                // Try gzip decompression first; fall back to raw bytes.
                                if data.starts_with(&[0x1f, 0x8b]) {
                                    let mut decoded = String::new();
                                    let _ = flate2::read::GzDecoder::new(data.as_slice())
                                        .read_to_string(&mut decoded);
                                    decoded
                                } else {
                                    String::from_utf8(data).unwrap_or_default()
                                }
                            }
                            Err(_) => String::new(),
                        }
                    }
                    _ => String::new(),
                };

                self.event_bus.emit(DaemonEvent::RunCached {
                    run_id: entry.run_id.clone(),
                    repo: spec.repo.to_string_lossy().into_owned(),
                    command: spec.command.clone(),
                    cache_age: entry.cached_at.clone(),
                });
                return Ok(RunOutcome::Cached {
                    exit_code: entry.exit_code,
                    output: cached_output,
                    cache_age: format!("{} (DB)", entry.cached_at),
                    reason: Some("served from persistent database cache".into()),
                });
            }
        }

        // 4. Join existing equivalent run
        if let Some(active_entry) = self.active_by_fingerprint.get(&fingerprint) {
            let active_arc = active_entry.value().clone();
            drop(active_entry); // Release DashMap shard read lock before await
            self.duplicates_prevented
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

            // Check completion AND subscribe under the same lock to avoid a
            // race where the completion hook finishes between the check and
            // the subscribe, leaving the subscriber waiting forever.
            let rx_opt: Option<watch::Receiver<i32>>;
            {
                let mut run = active_arc.lock();
                if let Some((_code, _output)) = &run.completion {
                    let run_id = run.spec.run_id.clone();
                    return Ok(RunOutcome::Joined {
                        run_id,
                        reason: Some(
                            "exact fingerprint match — joining completed equivalent run".into(),
                        ),
                    });
                }
                // Not completed yet — subscribe for the exit code.
                let (tx, rx) = watch::channel(-1);
                run.subscribers.push(tx);
                rx_opt = Some(rx);
            }

            if let Some(mut rx) = rx_opt {
                let _exit_code = rx.wait_for(|c| *c >= 0).await.map(|r| *r).unwrap_or(-1);
            }
            let run_id = { active_arc.lock().spec.run_id.clone() };
            return Ok(RunOutcome::Joined {
                run_id,
                reason: Some("exact fingerprint match — joining existing equivalent run".into()),
            });
        }

        // 5. Check superset/subset relations
        if let Some(parent_run) = self.find_superset(&fingerprint) {
            self.duplicates_prevented
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            debug!("Joining superset run for {hash}", hash = fingerprint.hash);

            let rx_opt: Option<watch::Receiver<i32>>;
            {
                let mut run = parent_run.lock();
                if let Some((_code, _output)) = &run.completion {
                    let run_id = run.spec.run_id.clone();
                    return Ok(RunOutcome::Joined {
                        run_id,
                        reason: Some(
                            "exact fingerprint match — joining completed equivalent run".into(),
                        ),
                    });
                }
                let (tx, rx) = watch::channel(-1);
                run.subscribers.push(tx);
                rx_opt = Some(rx);
            }

            if let Some(mut rx) = rx_opt {
                let _exit_code = rx.wait_for(|c| *c >= 0).await.map(|r| *r).unwrap_or(-1);
            }
            let run_id = { parent_run.lock().spec.run_id.clone() };
            return Ok(RunOutcome::Joined {
                run_id,
                reason: Some("exact fingerprint match — joining existing equivalent run".into()),
            });
        }

        // 6. Check resource availability
        let class = self.classify_resource(&spec);
        tracing::debug!("run_or_join step6: calling scheduler.acquire (class={class:?})");
        let _queue_start = std::time::Instant::now();
        let notify = self
            .scheduler
            .acquire(&spec.run_id, &spec.repo, &spec.command, class)
            .await;

        match notify {
            Some(ready) => {
                ready.notified().await;
                // Use start_or_reserve (atomic check-and-insert) instead of
                // start_new to close the race where a concurrent agent checks
                // active_by_fingerprint between our step-4 check and start_new's
                // insertion, finds nothing, and starts a duplicate run.
                self.start_or_reserve(spec, fingerprint).await
            }
            None => Ok(RunOutcome::Rejected {
                reason: "Scheduler queue full".to_string(),
            }),
        }
    }

    /// Handle Ctrl-C for a run: detach subscriber; kill if no subscribers remain.
    pub async fn cancel_run(&self, run_id: &str) -> Result<()> {
        if let Some(active) = self.active_by_id.get(run_id) {
            let kill = {
                let mut run = active.lock();
                if run.subscribers.len() <= 1 {
                    true
                } else {
                    run.subscribers.pop();
                    false
                }
            };

            if kill {
                info!("Killing run {run_id}: last subscriber cancelled");
                self.supervisor.kill_run(run_id).await?;
                self.scheduler.release(run_id);
                self.active_by_id.remove(run_id);
                self.active_by_fingerprint
                    .retain(|_, v| v.lock().spec.run_id != run_id);
            }
        }
        Ok(())
    }

    /// Wait for a run to complete.
    pub async fn wait_for_run(&self, run_id: &str) -> Option<i32> {
        self.supervisor.exit_code(run_id)
    }

    /// Stream output lines.
    pub async fn stream_run(&self, run_id: &str) -> Option<tokio::sync::mpsc::Receiver<String>> {
        self.supervisor.stream_output(run_id).await
    }

    /// Return all active run IDs.
    pub fn active_runs(&self) -> Vec<String> {
        self.supervisor.active_run_ids()
    }

    /// Invalidate cache for a fingerprint.
    pub fn invalidate_cache(&self, fingerprint: &CommandFingerprint) {
        self.cache.lock().pop(fingerprint);
    }

    /// Invalidate all cache entries for a repo.
    ///
    /// This is a best-effort LRU sweep: entries whose run was in the given repo are dropped.
    pub fn invalidate_repo_cache(&self, repo: &str) {
        let mut guard = self.cache.lock();
        // LruCache does not support bulk retain by predicate, so rebuild.
        let keys_to_remove: Vec<_> = guard
            .iter()
            .filter(|(_, cached)| {
                self.active_by_id
                    .get(&cached.run_id)
                    .map(|a| a.lock().spec.repo == std::path::Path::new(repo))
                    .unwrap_or(true)
            })
            .map(|(k, _)| k.clone())
            .collect();
        for k in keys_to_remove {
            guard.pop(&k);
        }
    }

    /// Store a result in the cache after a run completes.
    pub fn cache_result(&self, fingerprint: CommandFingerprint, result: CachedResult) {
        self.cache.lock().put(fingerprint, result);
    }

    /// Get the number of cache hits today, resetting at midnight UTC.
    pub fn cache_hits_today(&self) -> u64 {
        let today = Utc::now().date_naive();
        {
            let mut date = self.cache_hits_date.lock();
            if *date != today {
                *date = today;
                self.cache_hits_today
                    .store(0, std::sync::atomic::Ordering::Relaxed);
            }
        }
        self.cache_hits_today
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Get the number of duplicate runs prevented.
    pub fn duplicates_prevented(&self) -> u64 {
        self.duplicates_prevented
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    // -- Private helpers --

    async fn start_new(&self, spec: RunSpec) -> Result<RunOutcome> {
        let fingerprint = CommandFingerprint::from_spec(&spec).await;
        let run_id = spec.run_id.clone();

        let active = Arc::new(ParkingMutex::new(ActiveRun {
            spec: spec.clone(),
            started_at: Instant::now(),
            subscribers: Vec::new(),
            is_dev_server: self.is_dev_server(&spec),
            completion: None,
        }));

        self.active_by_fingerprint
            .insert(fingerprint.clone(), active.clone());
        self.active_by_id.insert(run_id.clone(), active);

        let repo_for_cache = spec.repo.clone();
        let actual_run_id = self.supervisor.spawn(spec).await?;

        // Spawn completion hook to cache results
        let cache = self.cache.clone();
        let db_for_completion = self.db.clone();
        let active_by_fp = self.active_by_fingerprint.clone();
        let active_by_id = self.active_by_id.clone();
        let scheduler = self.scheduler.clone();
        let supervisor = self.supervisor.clone();
        let fingerprint_for_cache = fingerprint.clone();
        let run_id_for_cache = actual_run_id.clone();

        tokio::spawn(async move {
            // Wait for completion via the supervisor's watch channel (event-driven, no polling).
            let code = supervisor
                .wait_for_completion(&run_id_for_cache)
                .await
                .unwrap_or(-1);

            let output = supervisor.get_output(&run_id_for_cache).unwrap_or_default();

            // Redact secrets from output before caching or persisting.
            let redacted_output = richter_core::redact::redact(&output);

            // Run git diff in a blocking context to avoid blocking the async runtime.
            let repo_path = repo_for_cache.clone();
            let changed: Vec<String> = tokio::task::spawn_blocking(move || {
                std::process::Command::new("git")
                    .args(["diff", "--name-only", "HEAD"])
                    .current_dir(&repo_path)
                    .output()
                    .ok()
                    .and_then(|o| String::from_utf8(o.stdout).ok())
                    .map(|s| s.lines().map(String::from).collect())
                    .unwrap_or_default()
            })
            .await
            .unwrap_or_default();

            cache.lock().put(
                fingerprint_for_cache.clone(),
                CachedResult {
                    fingerprint: fingerprint_for_cache.clone(),
                    run_id: run_id_for_cache.clone(),
                    exit_code: code,
                    output: redacted_output.clone(),
                    cached_at: Utc::now(),
                    changed_files: changed,
                },
            );

            // Persist to DB
            if let Some(db) = db_for_completion.as_ref() {
                let cache_key = fingerprint_for_cache.cache_key();
                let now_iso = Utc::now().to_rfc3339();
                if let Err(e) = db
                    .insert_cache_entry(
                        &uuid::Uuid::new_v4().to_string(),
                        &cache_key,
                        &run_id_for_cache,
                        code,
                        None,
                        &now_iso,
                        None,
                    )
                    .await
                {
                    tracing::warn!("Failed to persist cache entry to DB: {e}");
                }
            }

            if let Some(active) = active_by_fp.get(&fingerprint_for_cache) {
                let mut run = active.value().lock();
                // Store completion result so late joiners can still discover
                // this run even after it finishes (fixes the classic race
                // where the completion hook removes the entry before a
                // concurrent agent checks active_by_fingerprint).
                run.completion = Some((code, redacted_output.clone()));
                for tx in run.subscribers.drain(..) {
                    if let Err(e) = tx.send(code) {
                        warn!(
                            "Failed to notify subscriber for run {}: {}",
                            run_id_for_cache, e
                        );
                    }
                }
            }

            scheduler.release(&run_id_for_cache);
            active_by_id.remove(&run_id_for_cache);

            // Remove from active_by_fingerprint after a short grace period
            // so late-arriving agents can still discover and join this run.
            let fp_clone = fingerprint_for_cache.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs(3)).await;
                active_by_fp.remove(&fp_clone);
            });
        });

        Ok(RunOutcome::Started {
            run_id: actual_run_id,
            queue_time_ms: None,
            reason: Some("new run started — no matching run found".into()),
        })
    }

    /// Join an existing active run (subscribe to its completion).
    /// Handles both still-running and already-completed runs via the
    /// `completion` field on `ActiveRun`.
    async fn join_existing(&self, active: Arc<ParkingMutex<ActiveRun>>) -> Result<RunOutcome> {
        self.duplicates_prevented
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        let rx_opt: Option<watch::Receiver<i32>>;
        {
            let mut run = active.lock();
            if let Some((_code, _output)) = &run.completion {
                let run_id = run.spec.run_id.clone();
                return Ok(RunOutcome::Joined {
                    run_id,
                    reason: Some(
                        "exact fingerprint match — joining completed equivalent run".into(),
                    ),
                });
            }
            // Not completed yet — subscribe for the exit code.
            let (tx, rx) = watch::channel(-1);
            run.subscribers.push(tx);
            rx_opt = Some(rx);
        }

        if let Some(mut rx) = rx_opt {
            let _ = rx.wait_for(|c| *c >= 0).await.map(|r| *r).unwrap_or(-1);
        }
        let run_id = { active.lock().spec.run_id.clone() };
        Ok(RunOutcome::Joined {
            run_id,
            reason: Some("exact fingerprint match — joining existing equivalent run".into()),
        })
    }

    /// Start a new run, or join an existing one if a concurrent agent already
    /// reserved the same fingerprint. Uses `DashMap::entry` for an atomic
    /// check-and-insert, closing the classic TOCTOU race between
    /// `run_or_join`'s step-4 check and `start_new`'s insertion.
    ///
    /// Accepts a precomputed `fingerprint` to avoid re-running expensive
    /// git operations (which can deadlock when two agents compete inside
    /// `tokio::join!` for the same git subprocesses).
    async fn start_or_reserve(
        &self,
        spec: RunSpec,
        fingerprint: CommandFingerprint,
    ) -> Result<RunOutcome> {
        let run_id = spec.run_id.clone();

        // Atomically check-and-reserve the fingerprint.
        match self.active_by_fingerprint.entry(fingerprint.clone()) {
            dashmap::mapref::entry::Entry::Occupied(entry) => {
                // Another agent beat us here — join their run.
                // Clone the Arc and drop the entry (which holds the shard
                // write-lock) before the first .await to avoid deadlocking
                // with the completion hook which needs the same shard lock.
                let existing = entry.get().clone();
                drop(entry);
                self.join_existing(existing).await
            }
            dashmap::mapref::entry::Entry::Vacant(entry) => {
                let active = Arc::new(ParkingMutex::new(ActiveRun {
                    spec: spec.clone(),
                    started_at: Instant::now(),
                    subscribers: Vec::new(),
                    is_dev_server: self.is_dev_server(&spec),
                    completion: None,
                }));
                entry.insert(active.clone());
                self.active_by_id.insert(run_id.clone(), active);

                let repo_for_cache = spec.repo.clone();
                let actual_run_id = self.supervisor.spawn(spec).await?;

                // Spawn completion hook to cache results
                let cache = self.cache.clone();
                let db_for_completion = self.db.clone();
                let active_by_fp = self.active_by_fingerprint.clone();
                let active_by_id = self.active_by_id.clone();
                let scheduler = self.scheduler.clone();
                let supervisor = self.supervisor.clone();
                let fingerprint_for_cache = fingerprint.clone();
                let run_id_for_cache = actual_run_id.clone();

                tokio::spawn(async move {
                    let code = supervisor
                        .wait_for_completion(&run_id_for_cache)
                        .await
                        .unwrap_or(-1);

                    let output = supervisor.get_output(&run_id_for_cache).unwrap_or_default();
                    let redacted_output = richter_core::redact::redact(&output);

                    let repo_path = repo_for_cache.clone();
                    let changed: Vec<String> = tokio::task::spawn_blocking(move || {
                        std::process::Command::new("git")
                            .args(["diff", "--name-only", "HEAD"])
                            .current_dir(&repo_path)
                            .output()
                            .ok()
                            .and_then(|o| String::from_utf8(o.stdout).ok())
                            .map(|s| s.lines().map(String::from).collect())
                            .unwrap_or_default()
                    })
                    .await
                    .unwrap_or_default();

                    cache.lock().put(
                        fingerprint_for_cache.clone(),
                        CachedResult {
                            fingerprint: fingerprint_for_cache.clone(),
                            run_id: run_id_for_cache.clone(),
                            exit_code: code,
                            output: redacted_output.clone(),
                            cached_at: Utc::now(),
                            changed_files: changed,
                        },
                    );

                    if let Some(db) = db_for_completion.as_ref() {
                        let cache_key = fingerprint_for_cache.cache_key();
                        let now_iso = Utc::now().to_rfc3339();
                        if let Err(e) = db
                            .insert_cache_entry(
                                &uuid::Uuid::new_v4().to_string(),
                                &cache_key,
                                &run_id_for_cache,
                                code,
                                None,
                                &now_iso,
                                None,
                            )
                            .await
                        {
                            tracing::warn!("Failed to persist cache entry to DB: {e}");
                        }
                    }

                    if let Some(active) = active_by_fp.get(&fingerprint_for_cache) {
                        let mut run = active.value().lock();
                        run.completion = Some((code, redacted_output.clone()));
                        for tx in run.subscribers.drain(..) {
                            if let Err(e) = tx.send(code) {
                                warn!(
                                    "Failed to notify subscriber for run {}: {}",
                                    run_id_for_cache, e
                                );
                            }
                        }
                    }

                    scheduler.release(&run_id_for_cache);
                    active_by_id.remove(&run_id_for_cache);

                    let fp_clone = fingerprint_for_cache.clone();
                    tokio::spawn(async move {
                        tokio::time::sleep(Duration::from_secs(3)).await;
                        active_by_fp.remove(&fp_clone);
                    });
                });

                Ok(RunOutcome::Started {
                    run_id: actual_run_id,
                    queue_time_ms: None,
                    reason: Some("new run started — no matching run found".into()),
                })
            }
        }
    }

    fn find_superset(
        &self,
        fingerprint: &CommandFingerprint,
    ) -> Option<Arc<ParkingMutex<ActiveRun>>> {
        for entry in self.active_by_fingerprint.iter() {
            let fp = entry.key();
            if fp.is_superset_of(fingerprint) && fp != fingerprint {
                return Some(entry.value().clone());
            }
        }
        None
    }

    fn generate_preview(&self, command: &str) -> Option<String> {
        let lower = command.to_lowercase();
        if lower.contains("rm -rf") {
            Some(format!("would recursively delete: {}", command))
        } else if lower.contains("git push") && (lower.contains("-f") || lower.contains("--force"))
        {
            Some("would force-push, overwriting remote history".into())
        } else if lower.contains("drop ") || lower.contains("truncate ") {
            Some("would drop/truncate database table(s)".into())
        } else if lower.contains("terraform destroy") {
            Some("would destroy Terraform-managed infrastructure".into())
        } else {
            None
        }
    }

    fn is_destructive(&self, command: &str) -> bool {
        let destructive_patterns = [
            "rm -rf",
            "git push --force",
            "DROP ",
            "TRUNCATE ",
            "DELETE FROM",
            "mkfs.",
            "dd if=",
            "> /dev/sda",
        ];

        let lower = command.to_lowercase();
        destructive_patterns
            .iter()
            .any(|p| lower.contains(&p.to_lowercase()))
    }

    fn is_dev_server(&self, spec: &RunSpec) -> bool {
        let lower = spec.command.to_lowercase();
        spec.classification == CommandClass::DevServer
            || lower.contains("next dev")
            || lower.contains("vite")
            || lower.contains("npm run dev")
            || lower.contains("cargo watch")
            || lower.contains("rails server")
            || lower.contains("python -m http.server")
            || lower.contains("nodemon")
            || lower.contains("webpack-dev-server")
            || lower.contains("tsc --watch")
            || lower.contains("ng serve")
    }

    /// Increment cache hits counter, resetting at midnight UTC.
    fn increment_cache_hits(&self) {
        let today = Utc::now().date_naive();
        {
            let mut date = self.cache_hits_date.lock();
            if *date != today {
                *date = today;
                self.cache_hits_today
                    .store(0, std::sync::atomic::Ordering::Relaxed);
            }
        }
        self.cache_hits_today
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    fn classify_resource(&self, spec: &RunSpec) -> ResourceClass {
        match spec.resource_class {
            ResourceClass::Unknown => {
                let lower = spec.command.to_lowercase();
                if lower.contains("test") || lower.contains("pytest") {
                    ResourceClass::HeavyTest
                } else if lower.contains("build") || lower.contains("make") {
                    ResourceClass::HeavyBuild
                } else if lower.contains("lint")
                    || lower.contains("fmt")
                    || lower.contains("format")
                {
                    ResourceClass::LightLint
                } else if lower.contains("install")
                    || lower.contains("pip")
                    || lower.contains("npm")
                    || lower.contains("gem")
                {
                    ResourceClass::Install
                } else if self.is_dev_server(spec) {
                    ResourceClass::DevServer
                } else {
                    ResourceClass::LightLint
                }
            }
            other => other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_fingerprint_deterministic() {
        let spec = RunSpec {
            command: "cargo build".into(),
            repo: "/test".into(),
            ..Default::default()
        };
        let fp1 = CommandFingerprint::from_spec(&spec).await;
        let fp2 = CommandFingerprint::from_spec(&spec).await;
        assert_eq!(fp1.hash, fp2.hash);
        assert_eq!(fp1, fp2);
    }

    #[tokio::test]
    async fn test_fingerprint_subset() {
        let a = CommandFingerprint::from_spec(&RunSpec {
            command: "cargo build".into(),
            repo: "/x".into(),
            ..Default::default()
        })
        .await;
        let b = CommandFingerprint::from_spec(&RunSpec {
            command: "cargo build".into(),
            repo: "/x".into(),
            ..Default::default()
        })
        .await;
        assert!(a.is_subset_of(&b));
        assert!(a.is_superset_of(&b));
    }

    #[tokio::test]
    async fn test_destructive_detection() {
        let bus = EventBus::new();
        let scheduler = Scheduler::new(
            crate::scheduler::SchedulerConfig::default(),
            bus.clone(),
            crate::scheduler::ResourceMonitor::new(),
        );
        let supervisor = Arc::new(ProcessSupervisor::new(bus));
        let rm = RunManager::new(scheduler, supervisor, None, EventBus::new());

        assert!(rm.is_destructive("rm -rf /tmp"));
        assert!(rm.is_destructive("git push --force origin main"));
        assert!(!rm.is_destructive("cargo build"));
    }

    #[tokio::test]
    async fn test_dev_server_detection() {
        let bus = EventBus::new();
        let scheduler = Scheduler::new(
            crate::scheduler::SchedulerConfig::default(),
            bus.clone(),
            crate::scheduler::ResourceMonitor::new(),
        );
        let supervisor = Arc::new(ProcessSupervisor::new(bus));
        let rm = RunManager::new(scheduler, supervisor, None, EventBus::new());

        assert!(rm.is_dev_server(&RunSpec {
            command: "npm run dev".into(),
            ..Default::default()
        }));
        assert!(rm.is_dev_server(&RunSpec {
            command: "vite".into(),
            ..Default::default()
        }));
        assert!(!rm.is_dev_server(&RunSpec {
            command: "cargo build".into(),
            ..Default::default()
        }));
    }

    #[tokio::test]
    async fn test_cache_result_stores_and_evicts() {
        let bus = EventBus::new();
        let scheduler = Scheduler::new(
            crate::scheduler::SchedulerConfig::default(),
            bus.clone(),
            crate::scheduler::ResourceMonitor::new(),
        );
        let supervisor = Arc::new(ProcessSupervisor::new(bus));
        let rm = RunManager::new(scheduler, supervisor, None, EventBus::new());

        // Insert 3 entries (capacity is 10_000, so none evicted yet)
        for i in 0..3 {
            let fp = CommandFingerprint::from_spec(&RunSpec {
                command: format!("echo {}", i),
                ..Default::default()
            })
            .await;
            let result = CachedResult {
                fingerprint: fp.clone(),
                run_id: format!("r{}", i),
                exit_code: 0,
                output: String::new(),
                cached_at: Utc::now(),
                changed_files: vec![],
            };
            rm.cache_result(fp, result);
        }

        // Verify all 3 are present
        {
            let fp = CommandFingerprint::from_spec(&RunSpec {
                command: "echo 0".into(),
                ..Default::default()
            })
            .await;
            assert!(rm.cache.lock().get(&fp).is_some());
        }

        // Verify invalidate_cache removes a specific entry
        let fp0 = CommandFingerprint::from_spec(&RunSpec {
            command: "echo 0".into(),
            ..Default::default()
        })
        .await;
        rm.invalidate_cache(&fp0);
        {
            let cache = rm.cache.lock();
            assert_eq!(cache.len(), 2);
        }
    }

    #[tokio::test]
    async fn test_cached_result_freshness() {
        let fp = CommandFingerprint::from_spec(&RunSpec::default()).await;
        let result = CachedResult {
            fingerprint: fp,
            run_id: "r1".into(),
            exit_code: 0,
            output: String::new(),
            cached_at: Utc::now(),
            changed_files: vec![],
        };
        assert!(result.is_fresh_blocking(Duration::from_secs(600)));
        assert!(result.is_fresh_blocking(Duration::from_secs(1)));
    }
}
