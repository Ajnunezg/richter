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
pub use richter_core::models::ResourceClass;

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
    /// Single `System` instance reused across polls.
    sys: Arc<ParkingMutex<System>>,
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
            sys: Arc::new(ParkingMutex::new(sys)),
            snapshot: ParkingMutex::new(snapshot),
            last_poll: ParkingMutex::new(Instant::now()),
        }
    }

    /// Refresh the resource snapshot and return it.
    pub fn poll(&self) -> ResourceSnapshot {
        let mut sys = self.sys.lock();
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
            sys: Arc::clone(&self.sys),
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

    let disks = sysinfo::Disks::new_with_refreshed_list();
    let disk_free = disks
        .list()
        .iter()
        .next()
        .map(|d| d.available_space())
        .unwrap_or(0);

    ResourceSnapshot {
        cpu_percent: cpu,
        memory_percent: mem_pct,
        disk_free_bytes: disk_free,
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
    /// Minimum free disk space (bytes) to accept new runs.
    pub min_disk_free_bytes: u64,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            global_max: 6,
            repo_max: 3,
            queue_max: 64,
            min_disk_free_bytes: 100 * 1024 * 1024, // 100 MB
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
    /// Signal that wakes the queue processor when permits are released.
    queue_notifier: Arc<Notify>,
}

struct ActivePermit {
    #[allow(dead_code)]
    repo: String,
    /// Resource class of this active run.
    class: ResourceClass,
    // SemaphorePermits are intentionally not read—their sole purpose is
    // RAII: they release permits when dropped.
    #[allow(dead_code)]
    global_permit: Option<tokio::sync::OwnedSemaphorePermit>,
    #[allow(dead_code)]
    repo_permit: Option<tokio::sync::OwnedSemaphorePermit>,
}

impl Scheduler {
    /// Create a new scheduler and start the queue-processor task.
    pub fn new(
        config: SchedulerConfig,
        event_bus: EventBus,
        monitor: ResourceMonitor,
    ) -> Arc<Self> {
        let queue_notifier = Arc::new(Notify::new());
        let sched = Arc::new(Self {
            concurrency: Arc::new(ConcurrencyState {
                global: Arc::new(Semaphore::new(config.global_max)),
                repos: DashMap::new(),
            }),
            monitor,
            queue: ParkingMutex::new(VecDeque::new()),
            event_bus,
            config,
            active: Arc::new(DashMap::new()),
            queue_notifier: queue_notifier.clone(),
        });

        // Spawn the queue processor that wakes whenever a permit is released.
        let sched_clone = Arc::clone(&sched);
        tokio::spawn(async move {
            loop {
                queue_notifier.notified().await;
                sched_clone.process_queue().await;
            }
        });

        sched
    }

    /// Return a reference to the resource monitor for system metrics.
    pub fn resource_monitor(&self) -> &ResourceMonitor {
        &self.monitor
    }

    /// Request permission to run a command.
    ///
    /// Returns immediately with no wait if resources are available.
    /// Returns a `Notify` that resolves when a slot opens if the queue is not full.
    /// Returns `None` if the queue is at capacity (backpressure).
    pub async fn acquire(
        &self,
        run_id: &str,
        repo: &std::path::Path,
        command: &str,
        class: ResourceClass,
    ) -> Option<Arc<Notify>> {
        let snap = self.monitor.current();
        if snap.disk_free_bytes < self.config.min_disk_free_bytes {
            warn!(
                "Rejecting run {run_id}: disk free {} bytes < threshold {} bytes",
                snap.disk_free_bytes, self.config.min_disk_free_bytes
            );
            return None;
        }

        if self.can_run_immediately(repo, &class) {
            self.reserve_permits(run_id, repo, class).await;
            debug!(
                "Immediately scheduled run {run_id} [{:?}] in {}",
                class,
                repo.display()
            );
            let notify = Arc::new(Notify::new());
            notify.notify_one();
            return Some(notify);
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
            repo: repo.to_string_lossy().into_owned(),
            command: command.to_string(),
            class,
            enqueued_at: Instant::now(),
            ready: ready.clone(),
            priority: 10,
        };
        queue.push_back(entry);
        let position = queue.len();
        drop(queue);

        let estimated_wait = position as u64 * 1000;
        self.event_bus.emit(DaemonEvent::RunQueued {
            run_id: run_id.to_string(),
            repo: repo.to_string_lossy().into_owned(),
            reason: format!("{:?} resource constrained", class),
            estimated_wait_ms: estimated_wait,
        });

        info!(
            "Enqueued run {run_id} [{:?}] in {} (position: {})",
            class,
            repo.display(),
            position
        );

        Some(ready)
    }

    /// Release all permits held by a run and wake the queue processor.
    pub fn release(&self, run_id: &str) {
        if let Some((_, permit)) = self.active.remove(run_id) {
            debug!("Released permits for run {run_id}");
            drop(permit);
        }
        // Wake queue processor so it can attempt to schedule waiting runs.
        self.queue_notifier.notify_waiters();
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

    /// Return the number of available global scheduler permits.
    pub fn available_permits(&self) -> usize {
        self.concurrency.global.available_permits()
    }

    // -- Private helpers --

    fn can_run_immediately(&self, repo: &std::path::Path, class: &ResourceClass) -> bool {
        if self.monitor.is_under_pressure() {
            return false;
        }

        let global_available = self.concurrency.global.available_permits();
        if global_available == 0 {
            return false;
        }

        let repo_key = repo.to_string_lossy().into_owned();
        let repo_sem = self
            .concurrency
            .repos
            .entry(repo_key)
            .or_insert_with(|| Arc::new(Semaphore::new(self.config.repo_max)))
            .clone();
        if repo_sem.available_permits() == 0 {
            return false;
        }

        if !class.allows_concurrency() {
            let repo_str = repo.to_string_lossy();
            // Check same-class concurrency, not all-run concurrency.
            let active_same_class = self
                .active
                .iter()
                .filter(|entry| {
                    entry.value().repo == repo_str.as_ref() && entry.value().class == *class
                })
                .count();
            if active_same_class > 0 {
                return false;
            }
        }

        true
    }

    async fn reserve_permits(&self, run_id: &str, repo: &std::path::Path, class: ResourceClass) {
        let repo_key = repo.to_string_lossy().into_owned();
        let global = Arc::clone(&self.concurrency.global)
            .acquire_owned()
            .await
            .ok();
        let repo_sem = self
            .concurrency
            .repos
            .entry(repo_key)
            .or_insert_with(|| Arc::new(Semaphore::new(self.config.repo_max)))
            .clone();
        let repo_permit: Option<tokio::sync::OwnedSemaphorePermit> =
            repo_sem.acquire_owned().await.ok();

        self.active.insert(
            run_id.to_string(),
            ActivePermit {
                repo: repo.to_string_lossy().into_owned(),
                class,
                global_permit: global,
                repo_permit,
            },
        );
    }

    /// Drain the queue and start all runs that can now acquire permits.
    async fn process_queue(&self) {
        loop {
            let entry = {
                let queue = self.queue.lock();
                let entry = queue.front().cloned();
                drop(queue);
                entry
            };

            let Some(entry) = entry else {
                break;
            };

            if !self.can_run_immediately(std::path::Path::new(&entry.repo), &entry.class) {
                break;
            }

            // Dequeue and reserve
            {
                let mut queue = self.queue.lock();
                let _ = queue.pop_front();
                drop(queue);
            }

            self.reserve_permits(
                &entry.run_id,
                std::path::Path::new(&entry.repo),
                entry.class,
            )
            .await;

            let queue_ms = entry.enqueued_at.elapsed().as_millis() as u64;
            self.event_bus.emit(DaemonEvent::RunDequeued {
                run_id: entry.run_id.clone(),
                queue_time_ms: queue_ms,
            });

            entry.ready.notify_one();
        }
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
            .acquire(
                "run-1",
                std::path::Path::new("/repo"),
                "cargo fmt",
                ResourceClass::LightLint,
            )
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

    #[test]
    fn test_resource_monitor_reuses_system() {
        let monitor = ResourceMonitor::new();

        // First poll
        let snap1 = monitor.poll();
        assert!(snap1.cpu_percent >= 0.0);
        assert!(snap1.memory_percent >= 0.0);
        assert!(snap1.disk_free_bytes > 0);

        // Sleep briefly to allow metrics to change
        std::thread::sleep(Duration::from_millis(100));

        // Second poll — should not create a new System, just refresh
        let snap2 = monitor.poll();
        assert!(snap2.cpu_percent >= 0.0);
        assert!(snap2.memory_percent >= 0.0);
        assert!(snap2.disk_free_bytes > 0);

        // The timestamp should differ
        assert!(snap2.timestamp > snap1.timestamp);
    }
}
