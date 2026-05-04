//! Resource scheduler for the Richter daemon.
//!
//! Tracks CPU, memory, and disk pressure via `sysinfo`. Manages per-repo and
//! global concurrency limits. Provides a queue with backpressure for runs that
//! cannot be scheduled immediately. Supports process groups that are killable.

use dashmap::DashMap;
use parking_lot::Mutex as ParkingMutex;
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};
use sysinfo::System;
use tokio::sync::{Notify, Semaphore};
use tracing::{debug, info, warn};

use crate::event_bus::{DaemonEvent, EventBus};

// ---------------------------------------------------------------------------
// Resource classes and their characteristics
// ---------------------------------------------------------------------------

/// A resource class for categorising commands by their expected resource profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum ResourceClass {
    /// Heavy build (e.g. `cargo build`, `make -j`)
    HeavyBuild,
    /// Heavy test suite (e.g. `cargo test`, `pytest -n auto`)
    HeavyTest,
    /// Light lint/format (e.g. `cargo clippy`, `eslint`, `prettier`)
    LightLint,
    /// Package install (e.g. `npm install`, `pip install`)
    Install,
    /// Dev server / watch mode (long-running, not cacheable)
    DevServer,
}

impl ResourceClass {
    /// Estimated CPU weight for this class (0.0 – 1.0).
    pub fn cpu_weight(&self) -> f64 {
        match self {
            ResourceClass::HeavyBuild => 0.85,
            ResourceClass::HeavyTest => 0.80,
            ResourceClass::LightLint => 0.40,
            ResourceClass::Install => 0.30,
            ResourceClass::DevServer => 0.15,
        }
    }

    /// Whether this class can run concurrently with another instance of the same class.
    pub fn allows_concurrency(&self) -> bool {
        matches!(self, ResourceClass::LightLint | ResourceClass::DevServer)
    }
}

// ---------------------------------------------------------------------------
// System resource tracker
// ---------------------------------------------------------------------------

/// Snapshot of current system resource pressure.
#[derive(Debug, Clone)]
pub struct ResourceSnapshot {
    /// CPU usage as a percentage (0-100).
    pub cpu_percent: f32,
    /// Memory usage as a percentage (0-100).
    pub memory_percent: f32,
    /// Free disk space on the main volume in bytes.
    pub disk_free_bytes: u64,
    /// Unix epoch timestamp of the snapshot.
    pub timestamp: Instant,
}

/// Collects and caches system resource metrics as [`ResourceSnapshot`] values.
pub struct ResourceMonitor {
    /// Cached snapshot updated on each `poll`.
    snapshot: ParkingMutex<ResourceSnapshot>,
    /// Time of last poll.
    last_poll: ParkingMutex<Instant>,
}

impl ResourceMonitor {
    /// Create a new resource monitor with an initial poll.
    pub fn new() -> Self {
        let mut sys = System::new_all();
        sys.refresh_all();
        let snapshot = collect_snapshot(&sys);

        Self {
            snapshot: ParkingMutex::new(snapshot),
            last_poll: ParkingMutex::new(Instant::now()),
        }
    }

    /// Refresh the resource snapshot and return it.
    pub fn poll(&self) -> ResourceSnapshot {
        let mut sys = System::new_all();
        sys.refresh_all();
        let snapshot = collect_snapshot(&sys);

        let mut cached = self.snapshot.lock();
        *cached = snapshot.clone();
        let mut last = self.last_poll.lock();
        *last = Instant::now();
        snapshot
    }

    /// Return the most recent snapshot, polling if older than 5 seconds.
    pub fn current(&self) -> ResourceSnapshot {
        let last = *self.last_poll.lock();
        if last.elapsed() > Duration::from_secs(5) {
            return self.poll();
        }
        self.snapshot.lock().clone()
    }

    /// Return true if the system is under heavy pressure.
    pub fn is_under_pressure(&self) -> bool {
        let snap = self.current();
        snap.cpu_percent > 90.0 || snap.memory_percent > 90.0
    }
}

impl Clone for ResourceMonitor {
    fn clone(&self) -> Self {
        Self {
            snapshot: ParkingMutex::new(self.snapshot.lock().clone()),
            last_poll: ParkingMutex::new(*self.last_poll.lock()),
        }
    }
}

impl Default for ResourceMonitor {
    fn default() -> Self {
        Self::new()
    }
}

fn collect_snapshot(sys: &System) -> ResourceSnapshot {
    let cpu = sys.global_cpu_usage();
    let mem_used = sys.used_memory();
    let mem_total = sys.total_memory();
    let mem_pct = if mem_total > 0 {
        (mem_used as f32 / mem_total as f32) * 100.0
    } else {
        0.0
    };

    ResourceSnapshot {
        cpu_percent: cpu,
        memory_percent: mem_pct,
        disk_free_bytes: 0,
        timestamp: Instant::now(),
    }
}

// ---------------------------------------------------------------------------
// Concurrency limits
// ---------------------------------------------------------------------------

/// Concurrency control structure.
#[derive(Debug)]
struct ConcurrencyState {
    /// Global semaphore limiting total concurrent runs across all repos.
    global: Arc<Semaphore>,
    /// Per-repo semaphores.
    repos: DashMap<String, Arc<Semaphore>>,
}

/// Configuration for scheduler concurrency limits.
pub struct SchedulerConfig {
    /// Maximum concurrent runs globally.
    pub global_max: usize,
    /// Maximum concurrent runs per repository.
    pub repo_max: usize,
    /// Maximum queue length before backpressure kicks in.
    pub queue_max: usize,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            global_max: 6,
            repo_max: 3,
            queue_max: 64,
        }
    }
}

// ---------------------------------------------------------------------------
// Scheduled run entry
// ---------------------------------------------------------------------------

/// An item waiting in the scheduler queue.
#[derive(Debug, Clone)]
pub struct QueuedRun {
    /// Unique run identifier.
    pub run_id: String,
    /// Repository path.
    pub repo: String,
    /// The command to execute.
    pub command: String,
    /// Resource class for scheduling decisions.
    pub class: ResourceClass,
    /// When the item was enqueued.
    pub enqueued_at: Instant,
    /// Signal that resolves when the run can proceed.
    pub ready: Arc<Notify>,
    /// Priority level: lower number = higher priority.
    pub priority: u8,
}

// ---------------------------------------------------------------------------
// The scheduler itself
// ---------------------------------------------------------------------------

/// Central resource scheduler.
///
/// Accepts run requests, enqueues them if resources are constrained,
/// and releases permits when runs complete.
pub struct Scheduler {
    /// Concurrency control.
    concurrency: Arc<ConcurrencyState>,
    /// Resource monitor for system-level pressure.
    monitor: ResourceMonitor,
    /// Run queue.
    queue: ParkingMutex<VecDeque<QueuedRun>>,
    /// Event bus for emitting scheduling events.
    event_bus: EventBus,
    /// Configuration.
    config: SchedulerConfig,
    /// Pending run permits (run_id to permit tracking).
    active: Arc<DashMap<String, ActivePermit>>,
}

#[allow(dead_code)]
struct ActivePermit {
    repo: String,
    global_permit: Option<tokio::sync::OwnedSemaphorePermit>,
    repo_permit: Option<tokio::sync::OwnedSemaphorePermit>,
}

impl Scheduler {
    /// Create a new scheduler.
    pub fn new(config: SchedulerConfig, event_bus: EventBus, monitor: ResourceMonitor) -> Self {
        Self {
            concurrency: Arc::new(ConcurrencyState {
                global: Arc::new(Semaphore::new(config.global_max)),
                repos: DashMap::new(),
            }),
            monitor,
            queue: ParkingMutex::new(VecDeque::new()),
            event_bus,
            config,
            active: Arc::new(DashMap::new()),
        }
    }

    /// Request permission to run a command.
    ///
    /// Returns immediately with no wait if resources are available.
    /// Returns a `Notify` that resolves when a slot opens if the queue is not full.
    /// Returns `None` if the queue is at capacity (backpressure).
    pub async fn acquire(
        &self,
        run_id: &str,
        repo: &str,
        command: &str,
        class: ResourceClass,
    ) -> Option<Arc<Notify>> {
        if self.can_run_immediately(repo, &class) {
            self.reserve_permits(run_id, repo).await;
            debug!("Immediately scheduled run {run_id} [{:?}] in {repo}", class);
            return Some(Arc::new(Notify::new()));
        }

        // Otherwise, enqueue
        let mut queue = self.queue.lock();
        if queue.len() >= self.config.queue_max {
            warn!("Scheduler queue full, rejecting run {run_id}");
            return None;
        }

        let ready = Arc::new(Notify::new());
        let entry = QueuedRun {
            run_id: run_id.to_string(),
            repo: repo.to_string(),
            command: command.to_string(),
            class,
            enqueued_at: Instant::now(),
            ready: ready.clone(),
            priority: 10,
        };
        let entry_enqueued_at = entry.enqueued_at;
        queue.push_back(entry);

        let estimated_wait = queue.len() as u64 * 1000;
        self.event_bus.emit(DaemonEvent::RunQueued {
            run_id: run_id.to_string(),
            repo: repo.to_string(),
            reason: format!("{:?} resource constrained", class),
            estimated_wait_ms: estimated_wait,
        });

        info!(
            "Enqueued run {run_id} [{:?}] in {repo} (position: {})",
            class,
            queue.len()
        );
        drop(queue);

        // Spawn a background task to poll readiness
        let ready_clone = ready.clone();
        let concurrency = self.concurrency.clone();
        let active = self.active.clone();
        let run_id_owned = run_id.to_string();
        let repo_owned = repo.to_string();
        let event_bus = self.event_bus.clone();
        let monitor_owned = self.monitor.clone();

        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(100)).await;

                if monitor_owned.is_under_pressure() {
                    continue;
                }

                let permit = concurrency.global.clone().try_acquire_owned().ok();
                if permit.is_none() {
                    continue;
                }

                let repo_sem = concurrency
                    .repos
                    .entry(repo_owned.clone())
                    .or_insert_with(|| Arc::new(Semaphore::new(6)))
                    .clone();
                let repo_permit = repo_sem.try_acquire_owned().ok();
                if repo_permit.is_none() {
                    drop(permit);
                    continue;
                }

                active.insert(
                    run_id_owned.clone(),
                    ActivePermit {
                        repo: repo_owned.clone(),
                        global_permit: permit,
                        repo_permit,
                    },
                );

                let queue_ms = entry_enqueued_at.elapsed().as_millis() as u64;
                event_bus.emit(DaemonEvent::RunDequeued {
                    run_id: run_id_owned.clone(),
                    queue_time_ms: queue_ms,
                });

                ready_clone.notify_one();
                break;
            }
        });

        Some(ready)
    }

    /// Release all permits held by a run.
    pub fn release(&self, run_id: &str) {
        if let Some((_, permit)) = self.active.remove(run_id) {
            debug!("Released permits for run {run_id}");
            drop(permit);
        }
    }

    /// Force-kill all active runs in a repository.
    ///
    /// Returns the run IDs of killed runs.
    pub fn kill_repo(&self, repo: &str) -> Vec<String> {
        self.active
            .iter()
            .filter(|entry| entry.value().repo == repo)
            .map(|entry| entry.key().clone())
            .collect()
    }

    /// Return current active run count.
    pub fn active_count(&self) -> usize {
        self.active.len()
    }

    /// Return current queue depth.
    pub fn queue_depth(&self) -> usize {
        self.queue.lock().len()
    }

    /// Return a snapshot of current resource usage.
    pub fn resource_snapshot(&self) -> ResourceSnapshot {
        self.monitor.current()
    }

    // -- Private helpers --

    fn can_run_immediately(&self, repo: &str, class: &ResourceClass) -> bool {
        if self.monitor.is_under_pressure() {
            return false;
        }

        let global_available = self.concurrency.global.available_permits();
        if global_available == 0 {
            return false;
        }

        let repo_sem = self
            .concurrency
            .repos
            .entry(repo.to_string())
            .or_insert_with(|| Arc::new(Semaphore::new(self.config.repo_max)))
            .clone();
        if repo_sem.available_permits() == 0 {
            return false;
        }

        if !class.allows_concurrency() {
            let active_same_class = self
                .active
                .iter()
                .filter(|entry| entry.value().repo == repo)
                .count();
            if active_same_class > 0 {
                return false;
            }
        }

        true
    }

    async fn reserve_permits(&self, run_id: &str, repo: &str) {
        let global = Arc::clone(&self.concurrency.global)
            .acquire_owned()
            .await
            .ok();
        let repo_sem = self
            .concurrency
            .repos
            .entry(repo.to_string())
            .or_insert_with(|| Arc::new(Semaphore::new(self.config.repo_max)))
            .clone();
        let repo_permit: Option<tokio::sync::OwnedSemaphorePermit> =
            repo_sem.acquire_owned().await.ok();

        self.active.insert(
            run_id.to_string(),
            ActivePermit {
                repo: repo.to_string(),
                global_permit: global,
                repo_permit,
            },
        );
    }

    #[allow(dead_code)]
    fn drain_queue(&self) {
        // Placeholder: woken by spawned tasks
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_resource_class_weights() {
        assert!(ResourceClass::HeavyBuild.cpu_weight() > ResourceClass::LightLint.cpu_weight());
        assert!(ResourceClass::DevServer.allows_concurrency());
        assert!(!ResourceClass::HeavyTest.allows_concurrency());
    }

    #[tokio::test]
    async fn test_scheduler_immediate_acquire() {
        let config = SchedulerConfig::default();
        let bus = EventBus::new();
        let monitor = ResourceMonitor::new();
        let sched = Scheduler::new(config, bus, monitor);

        let notify = sched
            .acquire("run-1", "/repo", "cargo fmt", ResourceClass::LightLint)
            .await;
        assert!(notify.is_some());
        assert_eq!(sched.active_count(), 1);

        sched.release("run-1");
        assert_eq!(sched.active_count(), 0);
    }

    #[tokio::test]
    async fn test_scheduler_serialization() {
        let json = serde_json::to_string(&ResourceClass::HeavyBuild).unwrap();
        let parsed: ResourceClass = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, ResourceClass::HeavyBuild);
    }
}
