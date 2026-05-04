//! Resource tracking and scheduling primitives.
//!
//! Tracks CPU, memory, and disk pressure. Defines resource classes for
//! scheduling (heavy_build, heavy_test, light_lint, install, dev_server)
//! and provides per-repo and global concurrency limits with a simple
//! queue abstraction.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use parking_lot::Mutex;

use crate::models::{RepoId, ResourceClass, ResourcePressure, RunId};

/// Compute the resource class for a given command class.
pub fn classify_resource(class: crate::models::CommandClass) -> ResourceClass {
    match class {
        crate::models::CommandClass::Build => ResourceClass::HeavyBuild,
        crate::models::CommandClass::Test => ResourceClass::HeavyTest,
        crate::models::CommandClass::Lint => ResourceClass::LightLint,
        crate::models::CommandClass::Typecheck => ResourceClass::LightLint,
        crate::models::CommandClass::Format => ResourceClass::LightLint,
        crate::models::CommandClass::Install => ResourceClass::Install,
        crate::models::CommandClass::DevServer => ResourceClass::DevServer,
        crate::models::CommandClass::Migration => ResourceClass::HeavyBuild,
        crate::models::CommandClass::Destructive => ResourceClass::Unknown,
        crate::models::CommandClass::Unknown => ResourceClass::Unknown,
    }
}

/// Whether a resource class is considered "heavy" for limit enforcement.
pub fn is_heavy_class(rc: ResourceClass) -> bool {
    matches!(rc, ResourceClass::HeavyBuild | ResourceClass::HeavyTest)
}

/// A snapshot of the current system resource state.
#[derive(Debug, Clone)]
pub struct SystemResources {
    /// CPU usage fraction (0.0–1.0).
    pub cpu_usage: f64,
    /// Memory usage fraction (0.0–1.0).
    pub memory_usage: f64,
    /// Number of active heavy builds globally.
    pub active_heavy_builds: usize,
    /// Number of active heavy tests globally.
    pub active_heavy_tests: usize,
    /// Number of active light runs globally.
    pub active_light_runs: usize,
    /// Number of active installs globally.
    pub active_installs: usize,
    /// Number of active dev servers globally.
    pub active_dev_servers: usize,
    /// Total active processes under Richter.
    pub total_active_processes: usize,
}

impl SystemResources {
    /// Convert to a `ResourcePressure` DTO for API responses.
    pub fn to_pressure(&self) -> ResourcePressure {
        ResourcePressure {
            cpu: self.cpu_usage,
            memory: self.memory_usage,
            active_heavy_builds: self.active_heavy_builds,
            active_heavy_tests: self.active_heavy_tests,
            total_active_processes: self.total_active_processes,
        }
    }

    /// Check whether the system is under high CPU pressure.
    pub fn cpu_under_pressure(&self, threshold: f64) -> bool {
        self.cpu_usage > threshold
    }

    /// Check whether the system is under high memory pressure.
    pub fn memory_under_pressure(&self, threshold: f64) -> bool {
        self.memory_usage > threshold
    }

    /// Check whether any pressure threshold is exceeded.
    pub fn any_pressure(&self, cpu_threshold: f64, memory_threshold: f64) -> bool {
        self.cpu_under_pressure(cpu_threshold) || self.memory_under_pressure(memory_threshold)
    }
}

impl Default for SystemResources {
    fn default() -> Self {
        SystemResources {
            cpu_usage: 0.0,
            memory_usage: 0.0,
            active_heavy_builds: 0,
            active_heavy_tests: 0,
            active_light_runs: 0,
            active_installs: 0,
            active_dev_servers: 0,
            total_active_processes: 0,
        }
    }
}

/// A lightweight queue for pending runs.
#[derive(Debug)]
pub struct RunQueue {
    /// Queued runs, in FIFO order.
    queue: VecDeque<QueuedRun>,
}

/// A run waiting in the queue.
#[derive(Debug, Clone)]
pub struct QueuedRun {
    /// The run identifier.
    pub run_id: RunId,
    /// The repo this run belongs to.
    pub repo_id: Option<RepoId>,
    /// The resource class.
    pub resource_class: ResourceClass,
    /// When the run was enqueued.
    pub enqueued_at: chrono::DateTime<chrono::Utc>,
    /// Optional priority (higher = more urgent).
    pub priority: u8,
}

impl RunQueue {
    /// Create a new empty queue.
    pub fn new() -> Self {
        RunQueue {
            queue: VecDeque::new(),
        }
    }

    /// Enqueue a run.
    pub fn enqueue(&mut self, run: QueuedRun) {
        // Insert maintaining priority order (higher priority first)
        let pos = self
            .queue
            .iter()
            .position(|r| r.priority < run.priority)
            .unwrap_or(self.queue.len());
        self.queue.insert(pos, run);
    }

    /// Dequeue the next run (highest priority, then FIFO).
    pub fn dequeue(&mut self) -> Option<QueuedRun> {
        self.queue.pop_front()
    }

    /// Peek at the next run without dequeuing.
    pub fn peek(&self) -> Option<&QueuedRun> {
        self.queue.front()
    }

    /// Number of queued runs.
    pub fn len(&self) -> usize {
        self.queue.len()
    }

    /// Whether the queue is empty.
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// Remove a specific run by ID.
    pub fn remove(&mut self, run_id: RunId) -> Option<QueuedRun> {
        if let Some(pos) = self.queue.iter().position(|r| r.run_id == run_id) {
            self.queue.remove(pos)
        } else {
            None
        }
    }

    /// List all queued runs.
    pub fn list(&self) -> Vec<&QueuedRun> {
        self.queue.iter().collect()
    }
}

impl Default for RunQueue {
    fn default() -> Self {
        Self::new()
    }
}

/// A scheduler that enforces concurrency limits per-repo and globally.
#[derive(Debug)]
pub struct ResourceScheduler {
    /// Per-repo active run counts.
    repo_active: HashMap<RepoId, usize>,
    /// Per-repo active heavy run counts.
    repo_heavy_active: HashMap<RepoId, usize>,
    /// Global active heavy run count.
    global_heavy_active: usize,
    /// Global active light run count.
    global_light_active: usize,
    /// The global run queue.
    queue: RunQueue,
    /// CPU pressure threshold.
    cpu_threshold: f64,
    /// Memory pressure threshold.
    memory_threshold: f64,
    /// Max heavy runs per repo.
    max_heavy_per_repo: usize,
    /// Max heavy runs global.
    max_heavy_global: usize,
    /// Max light runs per repo.
    max_light_per_repo: usize,
}

impl ResourceScheduler {
    /// Create a new scheduler with the given limits.
    pub fn new(
        max_heavy_per_repo: usize,
        max_heavy_global: usize,
        max_light_per_repo: usize,
        cpu_threshold: f64,
        memory_threshold: f64,
    ) -> Self {
        ResourceScheduler {
            repo_active: HashMap::new(),
            repo_heavy_active: HashMap::new(),
            global_heavy_active: 0,
            global_light_active: 0,
            queue: RunQueue::new(),
            cpu_threshold,
            memory_threshold,
            max_heavy_per_repo,
            max_heavy_global,
            max_light_per_repo,
        }
    }

    /// Try to admit a run. Returns `true` if admitted, `false` if queued.
    pub fn try_admit(
        &mut self,
        run_id: RunId,
        repo_id: Option<RepoId>,
        rc: ResourceClass,
        resources: &SystemResources,
    ) -> bool {
        // Install, dev server, and unknown classes always bypass pressure/limits.
        let always_admit = matches!(
            rc,
            ResourceClass::Install | ResourceClass::DevServer | ResourceClass::Unknown
        );

        if !always_admit {
            // Check system pressure (only applies to heavy and light)
            if resources.any_pressure(self.cpu_threshold, self.memory_threshold) {
                self.queue.enqueue(QueuedRun {
                    run_id,
                    repo_id,
                    resource_class: rc,
                    enqueued_at: chrono::Utc::now(),
                    priority: 0,
                });
                return false;
            }
        }

        // Check per-repo and global limits
        let can_admit = match rc {
            ResourceClass::HeavyBuild | ResourceClass::HeavyTest => {
                let repo_heavy = repo_id
                    .map(|rid| self.repo_heavy_active.get(&rid).copied().unwrap_or(0))
                    .unwrap_or(0);
                repo_heavy < self.max_heavy_per_repo
                    && self.global_heavy_active < self.max_heavy_global
            }
            ResourceClass::LightLint => {
                let repo_active = repo_id
                    .map(|rid| self.repo_active.get(&rid).copied().unwrap_or(0))
                    .unwrap_or(0);
                repo_active < self.max_light_per_repo
            }
            ResourceClass::Install | ResourceClass::DevServer | ResourceClass::Unknown => true,
        };

        if can_admit {
            self.record_admission(repo_id, rc);
            true
        } else {
            self.queue.enqueue(QueuedRun {
                run_id,
                repo_id,
                resource_class: rc,
                enqueued_at: chrono::Utc::now(),
                priority: 0,
            });
            false
        }
    }

    /// Record that a run has been admitted.
    fn record_admission(&mut self, repo_id: Option<RepoId>, rc: ResourceClass) {
        if let Some(rid) = repo_id {
            *self.repo_active.entry(rid).or_insert(0) += 1;
        }

        match rc {
            ResourceClass::HeavyBuild | ResourceClass::HeavyTest => {
                self.global_heavy_active += 1;
                if let Some(rid) = repo_id {
                    *self.repo_heavy_active.entry(rid).or_insert(0) += 1;
                }
            }
            ResourceClass::LightLint => {
                self.global_light_active += 1;
            }
            _ => {}
        }
    }

    /// Mark a run as completed and try to admit the next queued run.
    pub fn complete_run(
        &mut self,
        repo_id: Option<RepoId>,
        rc: ResourceClass,
        resources: &SystemResources,
    ) -> Vec<RunId> {
        // Decrement counters
        if let Some(rid) = repo_id {
            if let Some(count) = self.repo_active.get_mut(&rid) {
                *count = count.saturating_sub(1);
            }
        }

        match rc {
            ResourceClass::HeavyBuild | ResourceClass::HeavyTest => {
                self.global_heavy_active = self.global_heavy_active.saturating_sub(1);
                if let Some(rid) = repo_id {
                    if let Some(count) = self.repo_heavy_active.get_mut(&rid) {
                        *count = count.saturating_sub(1);
                    }
                }
            }
            ResourceClass::LightLint => {
                self.global_light_active = self.global_light_active.saturating_sub(1);
            }
            _ => {}
        }

        // Try to admit queued runs
        let mut admitted = Vec::new();
        let max_attempts = self.queue.len();
        for _ in 0..max_attempts {
            if let Some(queued) = self.queue.dequeue() {
                if self.try_admit(
                    queued.run_id,
                    queued.repo_id,
                    queued.resource_class,
                    resources,
                ) {
                    admitted.push(queued.run_id);
                } else {
                    // Put it back
                    self.queue.enqueue(queued);
                    break;
                }
            } else {
                break;
            }
        }

        admitted
    }

    /// Cancel a queued run.
    pub fn cancel_queued(&mut self, run_id: RunId) -> Option<QueuedRun> {
        self.queue.remove(run_id)
    }

    /// Get the current queue length.
    pub fn queue_len(&self) -> usize {
        self.queue.len()
    }

    /// Get active heavy count by repo.
    pub fn repo_heavy_count(&self, repo_id: RepoId) -> usize {
        self.repo_heavy_active.get(&repo_id).copied().unwrap_or(0)
    }

    /// Get global heavy active count.
    pub fn global_heavy_count(&self) -> usize {
        self.global_heavy_active
    }

    /// Get global light active count.
    pub fn global_light_count(&self) -> usize {
        self.global_light_active
    }

    /// Get queued runs as a list.
    pub fn queued_runs(&self) -> Vec<&QueuedRun> {
        self.queue.list()
    }
}

/// Thread-safe resource manager.
#[derive(Debug, Clone)]
pub struct ResourceManager {
    inner: Arc<Mutex<ResourceScheduler>>,
    current_resources: Arc<Mutex<SystemResources>>,
}

impl ResourceManager {
    /// Create a new resource manager with default limits.
    pub fn new() -> Self {
        ResourceManager {
            inner: Arc::new(Mutex::new(ResourceScheduler::new(1, 3, 4, 0.85, 0.90))),
            current_resources: Arc::new(Mutex::new(SystemResources::default())),
        }
    }

    /// Create a new resource manager with custom limits.
    pub fn with_limits(
        max_heavy_per_repo: usize,
        max_heavy_global: usize,
        max_light_per_repo: usize,
        cpu_threshold: f64,
        memory_threshold: f64,
    ) -> Self {
        ResourceManager {
            inner: Arc::new(Mutex::new(ResourceScheduler::new(
                max_heavy_per_repo,
                max_heavy_global,
                max_light_per_repo,
                cpu_threshold,
                memory_threshold,
            ))),
            current_resources: Arc::new(Mutex::new(SystemResources::default())),
        }
    }

    /// Update the current system resource snapshot.
    pub fn update_resources(&self, resources: SystemResources) {
        let mut current = self.current_resources.lock();
        *current = resources;
    }

    /// Get the current system pressure snapshot.
    pub fn get_pressure(&self) -> ResourcePressure {
        let current = self.current_resources.lock();
        current.to_pressure()
    }

    /// Try to admit a run.
    pub fn try_admit(&self, run_id: RunId, repo_id: Option<RepoId>, rc: ResourceClass) -> bool {
        let resources = self.current_resources.lock().clone();
        let mut scheduler = self.inner.lock();
        scheduler.try_admit(run_id, repo_id, rc, &resources)
    }

    /// Mark a run as completed.
    pub fn complete_run(&self, repo_id: Option<RepoId>, rc: ResourceClass) -> Vec<RunId> {
        let resources = self.current_resources.lock().clone();
        let mut scheduler = self.inner.lock();
        scheduler.complete_run(repo_id, rc, &resources)
    }

    /// Cancel a queued run.
    pub fn cancel_queued(&self, run_id: RunId) -> Option<QueuedRun> {
        self.inner.lock().cancel_queued(run_id)
    }

    /// Get queue length.
    pub fn queue_len(&self) -> usize {
        self.inner.lock().queue_len()
    }

    /// Get heavy count for a repo.
    pub fn repo_heavy_count(&self, repo_id: RepoId) -> usize {
        self.inner.lock().repo_heavy_count(repo_id)
    }

    /// Get queued runs.
    pub fn queued_runs(&self) -> Vec<QueuedRun> {
        self.inner
            .lock()
            .queued_runs()
            .into_iter()
            .cloned()
            .collect()
    }
}

impl Default for ResourceManager {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn new_run_id() -> RunId {
        Uuid::new_v4()
    }

    fn light_resources() -> SystemResources {
        SystemResources {
            cpu_usage: 0.2,
            memory_usage: 0.3,
            ..Default::default()
        }
    }

    fn heavy_resources() -> SystemResources {
        SystemResources {
            cpu_usage: 0.95,
            memory_usage: 0.5,
            ..Default::default()
        }
    }

    #[test]
    fn test_classify_resource() {
        use crate::models::CommandClass;
        assert_eq!(
            classify_resource(CommandClass::Build),
            ResourceClass::HeavyBuild
        );
        assert_eq!(
            classify_resource(CommandClass::Test),
            ResourceClass::HeavyTest
        );
        assert_eq!(
            classify_resource(CommandClass::Lint),
            ResourceClass::LightLint
        );
    }

    #[test]
    fn test_is_heavy() {
        assert!(is_heavy_class(ResourceClass::HeavyBuild));
        assert!(is_heavy_class(ResourceClass::HeavyTest));
        assert!(!is_heavy_class(ResourceClass::LightLint));
        assert!(!is_heavy_class(ResourceClass::Install));
    }

    #[test]
    fn test_system_resources_default() {
        let r = SystemResources::default();
        assert_eq!(r.cpu_usage, 0.0);
        assert_eq!(r.memory_usage, 0.0);
    }

    #[test]
    fn test_system_resources_pressure() {
        let r = SystemResources {
            cpu_usage: 0.9,
            memory_usage: 0.4,
            ..Default::default()
        };
        assert!(r.cpu_under_pressure(0.85));
        assert!(!r.memory_under_pressure(0.85));
        assert!(r.any_pressure(0.85, 0.85));
    }

    #[test]
    fn test_system_resources_to_pressure() {
        let r = SystemResources {
            cpu_usage: 0.75,
            memory_usage: 0.60,
            active_heavy_builds: 2,
            active_heavy_tests: 1,
            total_active_processes: 5,
            ..Default::default()
        };
        let p = r.to_pressure();
        assert_eq!(p.cpu, 0.75);
        assert_eq!(p.memory, 0.60);
        assert_eq!(p.active_heavy_builds, 2);
        assert_eq!(p.active_heavy_tests, 1);
        assert_eq!(p.total_active_processes, 5);
    }

    #[test]
    fn test_run_queue_enqueue_dequeue() {
        let mut q = RunQueue::new();
        let run_id = new_run_id();
        q.enqueue(QueuedRun {
            run_id,
            repo_id: None,
            resource_class: ResourceClass::HeavyBuild,
            enqueued_at: chrono::Utc::now(),
            priority: 0,
        });
        assert_eq!(q.len(), 1);
        let dequeued = q.dequeue().unwrap();
        assert_eq!(dequeued.run_id, run_id);
        assert!(q.is_empty());
    }

    #[test]
    fn test_run_queue_priority() {
        let mut q = RunQueue::new();
        let low_id = new_run_id();
        let high_id = new_run_id();
        q.enqueue(QueuedRun {
            run_id: low_id,
            repo_id: None,
            resource_class: ResourceClass::HeavyTest,
            enqueued_at: chrono::Utc::now(),
            priority: 1,
        });
        q.enqueue(QueuedRun {
            run_id: high_id,
            repo_id: None,
            resource_class: ResourceClass::HeavyBuild,
            enqueued_at: chrono::Utc::now(),
            priority: 10,
        });
        assert_eq!(q.dequeue().unwrap().run_id, high_id);
        assert_eq!(q.dequeue().unwrap().run_id, low_id);
    }

    #[test]
    fn test_run_queue_remove() {
        let mut q = RunQueue::new();
        let rid = new_run_id();
        q.enqueue(QueuedRun {
            run_id: rid,
            repo_id: None,
            resource_class: ResourceClass::HeavyBuild,
            enqueued_at: chrono::Utc::now(),
            priority: 0,
        });
        assert_eq!(q.len(), 1);
        let removed = q.remove(rid).unwrap();
        assert_eq!(removed.run_id, rid);
        assert!(q.is_empty());
    }

    #[test]
    fn test_scheduler_admit_heavy_under_pressure() {
        let mut s = ResourceScheduler::new(1, 3, 4, 0.85, 0.90);
        let rid = new_run_id();
        let admitted = s.try_admit(rid, None, ResourceClass::HeavyBuild, &heavy_resources());
        assert!(!admitted);
        assert_eq!(s.queue_len(), 1);
    }

    #[test]
    fn test_scheduler_admit_heavy_normal() {
        let mut s = ResourceScheduler::new(1, 3, 4, 0.85, 0.90);
        let rid = new_run_id();
        let admitted = s.try_admit(rid, None, ResourceClass::HeavyBuild, &light_resources());
        assert!(admitted);
        assert_eq!(s.global_heavy_count(), 1);
    }

    #[test]
    fn test_scheduler_global_heavy_limit() {
        let mut s = ResourceScheduler::new(1, 2, 4, 0.85, 0.90);
        let r1 = new_run_id();
        let r2 = new_run_id();
        let r3 = new_run_id();

        let res = light_resources();

        assert!(s.try_admit(r1, None, ResourceClass::HeavyBuild, &res));
        assert!(s.try_admit(r2, None, ResourceClass::HeavyTest, &res));
        assert!(!s.try_admit(r3, None, ResourceClass::HeavyBuild, &res));
        assert_eq!(s.queue_len(), 1);
    }

    #[test]
    fn test_scheduler_complete_and_admit_next() {
        let mut s = ResourceScheduler::new(1, 1, 4, 0.85, 0.90);
        let repo_id = Uuid::new_v4();
        let res = light_resources();

        let r1 = new_run_id();
        let r2 = new_run_id();

        assert!(s.try_admit(r1, Some(repo_id), ResourceClass::HeavyBuild, &res));
        assert!(!s.try_admit(r2, Some(repo_id), ResourceClass::HeavyBuild, &res));
        assert_eq!(s.queue_len(), 1);

        let admitted = s.complete_run(Some(repo_id), ResourceClass::HeavyBuild, &res);
        assert_eq!(admitted.len(), 1);
        assert_eq!(admitted[0], r2);
        assert_eq!(s.queue_len(), 0);
    }

    #[test]
    fn test_scheduler_light_lint() {
        let mut s = ResourceScheduler::new(1, 3, 2, 0.85, 0.90);
        let repo_id = Uuid::new_v4();
        let res = light_resources();

        let l1 = new_run_id();
        let l2 = new_run_id();
        let l3 = new_run_id();

        assert!(s.try_admit(l1, Some(repo_id), ResourceClass::LightLint, &res));
        assert!(s.try_admit(l2, Some(repo_id), ResourceClass::LightLint, &res));
        assert!(!s.try_admit(l3, Some(repo_id), ResourceClass::LightLint, &res));
        assert_eq!(s.queue_len(), 1);
    }

    #[test]
    fn test_scheduler_install_always_admits() {
        let mut s = ResourceScheduler::new(1, 1, 1, 0.85, 0.90);
        let rid = new_run_id();
        // Even under heavy pressure, installs are always admitted
        assert!(
            s.try_admit(rid, None, ResourceClass::Install, &heavy_resources()),
            "installs should always be admitted regardless of pressure"
        );
    }

    #[test]
    fn test_scheduler_cancel_queued() {
        let mut s = ResourceScheduler::new(1, 1, 4, 0.85, 0.90);
        let rid = new_run_id();
        let res = light_resources();

        assert!(s.try_admit(new_run_id(), None, ResourceClass::HeavyBuild, &res));
        assert!(!s.try_admit(rid, None, ResourceClass::HeavyBuild, &res));
        assert_eq!(s.queue_len(), 1);

        let cancelled = s.cancel_queued(rid);
        assert!(cancelled.is_some());
        assert_eq!(s.queue_len(), 0);
    }

    #[test]
    fn test_resource_manager_thread_safe() {
        let rm = ResourceManager::new();
        let rid = new_run_id();
        let admitted = rm.try_admit(rid, None, ResourceClass::HeavyBuild);
        assert!(admitted);
        assert_eq!(rm.queue_len(), 0);
    }

    #[test]
    fn test_resource_manager_update_resources() {
        let rm = ResourceManager::new();
        let initial = rm.get_pressure();
        assert_eq!(initial.cpu, 0.0);

        rm.update_resources(SystemResources {
            cpu_usage: 0.85,
            memory_usage: 0.6,
            ..Default::default()
        });

        let updated = rm.get_pressure();
        assert_eq!(updated.cpu, 0.85);
    }
}
