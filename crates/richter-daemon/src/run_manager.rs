//! Run-or-join manager: core orchestration logic for the Richter daemon.
//!
//! Implements command deduplication via fingerprint matching, join-existing-run
//! (subscribe and receive the same exit code), cached-result return when fresh,
//! queueing on resource conflict, pass-through for unknown/destructive commands,
//! dev-server detection, Ctrl-C handling, and superset/subset relation detection.

use anyhow::Result;
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use parking_lot::Mutex as ParkingMutex;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::watch;
use tracing::{debug, info};

use crate::event_bus::{DaemonEvent, EventBus};
use richter_core::db::Database;
use crate::scheduler::{ResourceClass, Scheduler};
use crate::supervisor::{ProcessSupervisor, RunSpec};

/// A command fingerprint used for deduplication.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct CommandFingerprint {
    /// Normalized command string.
    pub command: String,
    /// SHA-256 of the command.
    pub command_hash: String,
    /// SHA-256 of the working directory / repo path.
    pub context_hash: String,
    /// SHA-256 of relevant environment variables.
    pub env_hash: String,
    /// Classification tag.
    pub classification: String,
}

impl CommandFingerprint {
    /// Create a fingerprint from a run specification.
    pub fn from_spec(spec: &RunSpec) -> Self {
        use sha2::{Digest, Sha256};

        let mut cmd_hasher = Sha256::new();
        cmd_hasher.update(spec.command.as_bytes());
        let command_hash = hex::encode(cmd_hasher.finalize());

        // Context hash: repo path + HEAD SHA + dirty state
        let mut ctx_hasher = Sha256::new();
        ctx_hasher.update(spec.repo.as_bytes());
        if let Some(ref sha) = spec.head_sha {
            ctx_hasher.update(sha.as_bytes());
        }
        ctx_hasher.update(if spec.is_dirty { b"dirty" } else { b"clean" });
        let context_hash = hex::encode(ctx_hasher.finalize());

        let mut env_hasher = Sha256::new();
        let mut keys: Vec<_> = spec.env.keys().collect();
        keys.sort();
        for k in keys {
            env_hasher.update(k.as_bytes());
            env_hasher.update(spec.env.get(k).unwrap_or(&String::new()).as_bytes());
        }
        // Incorporate lockfile hash if present
        if let Some(ref lh) = spec.lockfile_hash {
            env_hasher.update(lh.as_bytes());
        }
        let env_hash = hex::encode(env_hasher.finalize());

        Self {
            command: spec.command.clone(),
            command_hash,
            context_hash,
            env_hash,
            classification: spec.classification.clone(),
        }
    }

    /// Build a cache key string from this fingerprint (for DB lookups).
    pub fn cache_key(&self) -> String {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(self.command.as_bytes());
        hasher.update(self.command_hash.as_bytes());
        hasher.update(self.context_hash.as_bytes());
        hasher.update(self.env_hash.as_bytes());
        hasher.update(self.classification.as_bytes());
        hex::encode(hasher.finalize())
    }

    /// Check if this fingerprint is a subset of another.
    pub fn is_subset_of(&self, other: &CommandFingerprint) -> bool {
        self.command_hash == other.command_hash
            && self.context_hash == other.context_hash
            && self.classification == other.classification
    }

    /// Check if this fingerprint is a superset of another.
    pub fn is_superset_of(&self, other: &CommandFingerprint) -> bool {
        other.is_subset_of(self)
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
    pub fn is_fresh(&self, max_age: Duration) -> bool {
        let age = Utc::now().signed_duration_since(self.cached_at);
        // Check time-based freshness
        if age.to_std().unwrap_or(Duration::ZERO) >= max_age {
            return false;
        }
        // Check if any changed files have been modified since cache time
        for file_path in &self.changed_files {
            if let Ok(meta) = std::fs::metadata(file_path) {
                if let Ok(mtime) = meta.modified() {
                    let mtime_age = mtime
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default();
                    let cached_age = self.cached_at
                        .timestamp_millis() as u64;
                    // If file was modified after cache, it's stale
                    if mtime_age.as_millis() as i64 > cached_age as i64 {
                        return false;
                    }
                }
            }
        }
        true
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

/// An active run entry in the run manager.
#[derive(Debug)]
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
}

/// Core run-or-join orchestrator.
pub struct RunManager {
    /// Active runs by fingerprint.
    active_by_fingerprint: Arc<DashMap<CommandFingerprint, Arc<ParkingMutex<ActiveRun>>>>,
    /// Active runs by run ID.
    active_by_id: Arc<DashMap<String, Arc<ParkingMutex<ActiveRun>>>>,
    /// Result cache.
    cache: Arc<DashMap<CommandFingerprint, CachedResult>>,
    /// Persistent SQLite database handle for cache persistence.
    db: Option<Arc<Database>>,
    /// "Richter Saved You" counter: cache hits today.
    cache_hits_today: Arc<std::sync::atomic::AtomicU64>,
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
}

impl RunManager {
    /// Create a new run manager.
    pub fn new(
        scheduler: Arc<Scheduler>,
        supervisor: Arc<ProcessSupervisor>,
        db: Option<Arc<Database>>,
        event_bus: EventBus,
    ) -> Self {
        Self {
            active_by_fingerprint: Arc::new(DashMap::new()),
            active_by_id: Arc::new(DashMap::new()),
            cache: Arc::new(DashMap::new()),
            db,
            cache_hits_today: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            duplicates_prevented: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            scheduler,
            supervisor,
            event_bus,
            cache_ttl: Duration::from_secs(300),
        }
    }
        }
        manager
    }

    /// Set the result cache TTL.
    pub fn with_cache_ttl(mut self, ttl: Duration) -> Self {
        self.cache_ttl = ttl;
        self
    }

    /// Run or join a command.
    pub async fn run_or_join(&self, spec: RunSpec) -> Result<RunOutcome> {
        let fingerprint = CommandFingerprint::from_spec(&spec);

        // 0. Unknown commands always pass through — no caching, dedup, or scheduling.
        if spec.classification == "unknown" {
            info!("Pass-through unknown command: {}", spec.command);
            let outcome = self.start_new(spec).await?;
            if let RunOutcome::Started { run_id, queue_time_ms, .. } = outcome {
                return Ok(RunOutcome::Started {
                    run_id,
                    queue_time_ms,
                    reason: Some("unknown command class — pass-through, no deduplication".into()),
                });
            }
            return Ok(outcome);
        }

        // 1. Pass-through destructive commands
        if self.is_destructive(&spec.command) {
            info!("Pass-through destructive command: {}", spec.command);
                        let outcome = self.start_new(spec.clone()).await?;
            // Attach reason to Started outcome
            if let RunOutcome::Started { run_id, queue_time_ms, .. } = outcome {
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
            if let RunOutcome::Started { run_id, queue_time_ms, .. } = outcome {
                return Ok(RunOutcome::Started {
                    run_id,
                    queue_time_ms,
                    reason: Some("dev server — pass-through, no deduplication".into()),
                });
            }
            return Ok(outcome);
        }

        // 3. Check in-memory cache
        if let Some(cached) = self.cache.get(&fingerprint) {
            if cached.is_fresh(self.cache_ttl) {
                self.cache_hits_today.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                debug!(
                    "Cache hit for fingerprint {hash}",
                    hash = fingerprint.command_hash
                );
                self.event_bus.emit(DaemonEvent::RunCached {
                    run_id: cached.run_id.clone(),
                    repo: spec.repo.clone(),
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

        // 3b. Check persistent DB cache
        if let Some(db) = self.db.as_ref() {
            let cache_key = fingerprint.cache_key();
            if let Ok(Some(entry)) = db.get_cache_entry(&cache_key) {
                self.cache_hits_today.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                debug!(
                    "DB cache hit for fingerprint {hash}",
                    hash = fingerprint.command_hash
                );
                self.event_bus.emit(DaemonEvent::RunCached {
                    run_id: entry.run_id.clone(),
                    repo: spec.repo.clone(),
                    command: spec.command.clone(),
                    cache_age: entry.cached_at.clone(),
                });
                return Ok(RunOutcome::Cached {
                    exit_code: entry.exit_code,
                    output: String::new(),
                    cache_age: format!("{} (DB)", entry.cached_at),
                    reason: Some("served from persistent database cache".into()),
                });
            }
        }

        // 4. Join existing equivalent run
        if let Some(active) = self.active_by_fingerprint.get(&fingerprint) {
            self.duplicates_prevented.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let (tx, mut rx) = watch::channel(-1);
            {
                let mut run = active.lock();
                run.subscribers.push(tx);
            }
            let _exit_code = rx.wait_for(|c| *c >= 0).await.map(|r| *r).unwrap_or(-1);
            let run_id = { active.lock().spec.run_id.clone() };
            return Ok(RunOutcome::Joined { run_id, reason: Some("exact fingerprint match — joining existing equivalent run".into()) });
        }

        // 5. Check superset/subset relations
        if let Some(parent_run) = self.find_superset(&fingerprint) {
            self.duplicates_prevented.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            debug!(
                "Joining superset run for {hash}",
                hash = fingerprint.command_hash
            );
            let (tx, mut rx) = watch::channel(-1);
            {
                let mut run = parent_run.lock();
                run.subscribers.push(tx);
            }
            let _exit_code = rx.wait_for(|c| *c >= 0).await.map(|r| *r).unwrap_or(-1);
            let run_id = { parent_run.lock().spec.run_id.clone() };
            return Ok(RunOutcome::Joined { run_id, reason: Some("exact fingerprint match — joining existing equivalent run".into()) });
        }

        // 6. Check resource availability
        let class = self.classify_resource(&spec);
        let _queue_start = std::time::Instant::now();
        let notify = self
            .scheduler
            .acquire(&spec.run_id, &spec.repo, &spec.command, class)
            .await;

        match notify {
            Some(ready) => {
                ready.notified().await;
                self.start_new(spec).await
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
        self.cache.remove(fingerprint);
    }

    /// Invalidate all cache entries for a repo.
    pub fn invalidate_repo_cache(&self, repo: &str) {
        self.cache.retain(|_fp, _| {
            !self
                .active_by_id
                .iter()
                .any(|e| e.value().lock().spec.repo == repo)
        });
    }

    /// Store a result in the cache after a run completes.
    pub fn cache_result(&self, fingerprint: CommandFingerprint, result: CachedResult) {
        self.cache.insert(fingerprint, result);
    }


    /// Get the number of cache hits today.
    pub fn cache_hits_today(&self) -> u64 {
        self.cache_hits_today.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Get the number of duplicate runs prevented.
    pub fn duplicates_prevented(&self) -> u64 {
        self.duplicates_prevented.load(std::sync::atomic::Ordering::Relaxed)
    }

    // -- Private helpers --

    async fn start_new(&self, spec: RunSpec) -> Result<RunOutcome> {
        let fingerprint = CommandFingerprint::from_spec(&spec);
        let run_id = spec.run_id.clone();

        let active = Arc::new(ParkingMutex::new(ActiveRun {
            spec: spec.clone(),
            started_at: Instant::now(),
            subscribers: Vec::new(),
            is_dev_server: self.is_dev_server(&spec),
        }));

        self.active_by_fingerprint
            .insert(fingerprint.clone(), active.clone());
        self.active_by_id.insert(run_id.clone(), active);

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
            loop {
                if let Some(code) = supervisor.exit_code(&run_id_for_cache) {
                    let output = supervisor.get_output(&run_id_for_cache).unwrap_or_default();

                    cache.insert(
                        fingerprint_for_cache.clone(),
                        CachedResult {
                            fingerprint: fingerprint_for_cache.clone(),
                            run_id: run_id_for_cache.clone(),
                            exit_code: code,
                            output: output.clone(),
                            cached_at: Utc::now(),
                            changed_files: Vec::new(),
                        },
                    );

                    // Persist to DB
                    if let Some(db) = db_for_completion.as_ref() {
                        let cache_key = fingerprint_for_cache.cache_key();
                        let now_iso = Utc::now().to_rfc3339();
                        if let Err(e) = db.insert_cache_entry(
                            &uuid::Uuid::new_v4().to_string(),
                            &cache_key,
                            &run_id_for_cache,
                            code,
                            None,
                            &now_iso,
                            None,
                        ) {
                            tracing::warn!("Failed to persist cache entry to DB: {e}");
                        }
                    }

                    if let Some(active) = active_by_fp.get(&fingerprint_for_cache) {
                        let mut run = active.value().lock();
                        for tx in run.subscribers.drain(..) {
                            let _ = tx.send(code);
                        }
                    }

                    scheduler.release(&run_id_for_cache);
                    active_by_id.remove(&run_id_for_cache);
                    active_by_fp.remove(&fingerprint_for_cache);
                    break;
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        });

        Ok(RunOutcome::Started {
            run_id: actual_run_id,
            queue_time_ms: None,
            reason: Some("new run started — no matching run found".into()),
        })
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
        spec.classification == "dev_server"
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

    fn classify_resource(&self, spec: &RunSpec) -> ResourceClass {
        match spec.resource_class.as_str() {
            "heavy_build" => ResourceClass::HeavyBuild,
            "heavy_test" => ResourceClass::HeavyTest,
            "light_lint" => ResourceClass::LightLint,
            "install" => ResourceClass::Install,
            "dev_server" => ResourceClass::DevServer,
            _ => {
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
        }
    }
}



#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fingerprint_deterministic() {
        let spec = RunSpec {
            command: "cargo build".into(),
            repo: "/test".into(),
            ..Default::default()
        };
        let fp1 = CommandFingerprint::from_spec(&spec);
        let fp2 = CommandFingerprint::from_spec(&spec);
        assert_eq!(fp1.command_hash, fp2.command_hash);
        assert_eq!(fp1, fp2);
    }

    #[test]
    fn test_fingerprint_subset() {
        let a = CommandFingerprint::from_spec(&RunSpec {
            command: "cargo build".into(),
            repo: "/x".into(),
            ..Default::default()
        });
        let b = CommandFingerprint::from_spec(&RunSpec {
            command: "cargo build".into(),
            repo: "/x".into(),
            ..Default::default()
        });
        assert!(a.is_subset_of(&b));
        assert!(a.is_superset_of(&b));
    }

    #[test]
    fn test_destructive_detection() {
        let bus = EventBus::new();
        let scheduler = Arc::new(Scheduler::new(
            crate::scheduler::SchedulerConfig::default(),
            bus.clone(),
            crate::scheduler::ResourceMonitor::new(),
        ));
        let supervisor = Arc::new(ProcessSupervisor::new(bus));
        let rm = RunManager::new(scheduler, supervisor, None, EventBus::new());

        assert!(rm.is_destructive("rm -rf /tmp"));
        assert!(rm.is_destructive("git push --force origin main"));
        assert!(!rm.is_destructive("cargo build"));
    }

    #[test]
    fn test_dev_server_detection() {
        let bus = EventBus::new();
        let scheduler = Arc::new(Scheduler::new(
            crate::scheduler::SchedulerConfig::default(),
            bus.clone(),
            crate::scheduler::ResourceMonitor::new(),
        ));
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

    #[test]
    fn test_cached_result_freshness() {
        let fp = CommandFingerprint::from_spec(&RunSpec::default());
        let result = CachedResult {
            fingerprint: fp,
            run_id: "r1".into(),
            exit_code: 0,
            output: String::new(),
            cached_at: Utc::now(),
            changed_files: vec![],
        };
        assert!(result.is_fresh(Duration::from_secs(600)));
        assert!(result.is_fresh(Duration::from_secs(1)));
    }
}
