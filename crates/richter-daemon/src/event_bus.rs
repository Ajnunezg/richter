//! Event bus for the Richter daemon.
//!
//! Uses `tokio::broadcast` channels to dispatch structured events across the daemon.
//! Producers emit typed events; consumers subscribe by event variant.
//! Includes event coalescing and rate-limiting to prevent log spam.

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::broadcast;
use tracing::debug;

/// Maximum number of queued events per channel before lagged consumers are dropped.
const BROADCAST_CAPACITY: usize = 256;

/// Minimum interval between coalesced events of the same type.
const COALESCE_WINDOW_MS: u64 = 250;

/// All structured event variants emitted by the daemon.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum DaemonEvent {
    /// A run has been dispatched to the process supervisor.
    RunStarted {
        /// Unique run identifier.
        run_id: String,
        /// Repository path the run belongs to.
        repo: String,
        /// Shell command being executed.
        command: String,
        /// Classification of the command.
        classification: String,
        /// When the run was started.
        started_at: DateTime<Utc>,
    },
    /// A run has completed successfully.
    RunCompleted {
        /// Unique run identifier.
        run_id: String,
        /// Repository path.
        repo: String,
        /// Exit code (0 for success).
        exit_code: i32,
        /// Duration of the run in milliseconds.
        duration_ms: u64,
        /// Whether the result was served from cache.
        cached: bool,
    },
    /// A run's result was served from the result cache.
    RunCached {
        /// Unique run identifier.
        run_id: String,
        /// Repository path.
        repo: String,
        /// The command that produced the cached result.
        command: String,
        /// Age of the cached result.
        cache_age: String,
    },
    /// A run has been queued due to resource constraints.
    RunQueued {
        /// Unique run identifier.
        run_id: String,
        /// Repository path.
        repo: String,
        /// Reason for queueing.
        reason: String,
        /// Estimated wait time in milliseconds.
        estimated_wait_ms: u64,
    },
    /// An important event (test failure, build break, etc.) was detected.
    ImportantEvent {
        /// Repository path.
        repo: String,
        /// Severity: low, medium, high, critical.
        severity: String,
        /// Human-readable reason.
        reason: String,
        /// Structured payload with parser-specific details.
        details: serde_json::Value,
    },
    /// Resource pressure detected (CPU, memory, disk).
    ResourcePressure {
        /// Resource type: cpu, memory, disk.
        resource: String,
        /// Pressure level as a percentage (0-100).
        level: u8,
        /// Human-readable description.
        description: String,
    },
    /// A conflicting command was detected.
    ConflictDetected {
        /// Repository path.
        repo: String,
        /// The existing command that conflicts.
        existing_command: String,
        /// The new command that conflicts.
        conflicting_command: String,
        /// Type of conflict.
        conflict_type: String,
    },
    /// A file-system watch event for a repo.
    FileChanged {
        /// Repository path.
        repo: String,
        /// Path of the changed file, relative to repo root.
        path: String,
        /// Kind of change: created, modified, removed.
        kind: String,
    },
    /// A daemon lifecycle event.
    DaemonStatus {
        /// Status: starting, running, stopping, error.
        status: String,
        /// Optional additional detail.
        detail: Option<String>,
    },
    /// A previously-queued run was dequeued and sent to the supervisor.
    RunDequeued {
        /// Unique run identifier.
        run_id: String,
        /// Time spent in the queue in milliseconds.
        queue_time_ms: u64,
    },
}

/// Subscription filter to select which event variants a consumer receives.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum EventFilter {
    /// Receive all events.
    All,
    /// Receive only events of a specific variant.
    Variant(String),
}

impl EventFilter {
    fn matches(&self, event: &DaemonEvent) -> bool {
        match self {
            EventFilter::All => true,
            EventFilter::Variant(v) => match event {
                DaemonEvent::RunStarted { .. } => v == "RunStarted",
                DaemonEvent::RunCompleted { .. } => v == "RunCompleted",
                DaemonEvent::RunCached { .. } => v == "RunCached",
                DaemonEvent::RunQueued { .. } => v == "RunQueued",
                DaemonEvent::ImportantEvent { .. } => v == "ImportantEvent",
                DaemonEvent::ResourcePressure { .. } => v == "ResourcePressure",
                DaemonEvent::ConflictDetected { .. } => v == "ConflictDetected",
                DaemonEvent::FileChanged { .. } => v == "FileChanged",
                DaemonEvent::DaemonStatus { .. } => v == "DaemonStatus",
                DaemonEvent::RunDequeued { .. } => v == "RunDequeued",
            },
        }
    }
}

/// Coalescence state for a single event variant.
#[derive(Debug)]
struct CoalescenceState {
    /// The last event that was emitted for this variant.
    last_event: DaemonEvent,
    /// When the last emit occurred.
    last_emit: Instant,
    /// Whether an event is pending coalescence.
    pending: bool,
    /// Whether the pending event is a duplicate of the last.
    is_duplicate: bool,
}

/// The central event bus.
///
/// Thread-safe. Clone is cheap (Arc).
#[derive(Clone)]
pub struct EventBus {
    /// Broadcast sender for all events.
    tx: broadcast::Sender<DaemonEvent>,
    /// Coalescence state per variant name, protected by a concurrent map.
    coalesce: Arc<DashMap<String, CoalescenceState>>,
}

impl EventBus {
    /// Create a new event bus with the configured broadcast capacity.
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(BROADCAST_CAPACITY);
        Self {
            tx,
            coalesce: Arc::new(DashMap::new()),
        }
    }

    /// Emit an event to all subscribers, with coalescence for rate-limited variants.
    ///
    /// Returns `true` if the event was actually broadcast (not coalesced away).
    pub fn emit(&self, event: DaemonEvent) -> bool {
        let variant = event_variant_name(&event);
        let now = Instant::now();

        // Check coalescence
        {
            let needs_insert = !self.coalesce.contains_key(variant);
            let mut state =
                self.coalesce
                    .entry(variant.to_string())
                    .or_insert_with(|| CoalescenceState {
                        last_event: event.clone(),
                        last_emit: now,
                        pending: false,
                        is_duplicate: false,
                    });

            // Don't coalesce if this is the first event of this variant
            if !needs_insert {
                let elapsed = now.duration_since(state.last_emit);
                let window = Duration::from_millis(COALESCE_WINDOW_MS);

                if elapsed < window {
                    let is_dup = events_are_coalescable(&state.last_event, &event);
                    if is_dup {
                        state.pending = true;
                        state.is_duplicate = true;
                        debug!("Coalesced duplicate {variant} event");
                        return false;
                    }
                }
            }

            state.last_event = event.clone();
            state.last_emit = now;
            state.pending = false;
            state.is_duplicate = false;
        }

        match self.tx.send(event) {
            Ok(n) => {
                debug!("Emitted event [{variant}] to {n} subscribers");
                true
            }
            Err(_e) => {
                false
            }
        }
    }

    /// Subscribe to all events.
    pub fn subscribe_all(&self) -> broadcast::Receiver<DaemonEvent> {
        self.tx.subscribe()
    }

    /// Subscribe to events matching a specific filter.
    pub fn subscribe_filtered(&self, filter: EventFilter) -> FilteredReceiver {
        FilteredReceiver {
            inner: self.tx.subscribe(),
            filter,
        }
    }

    /// Return the number of active receivers.
    pub fn receiver_count(&self) -> usize {
        self.tx.receiver_count()
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

/// A broadcast receiver that filters events by variant.
pub struct FilteredReceiver {
    inner: broadcast::Receiver<DaemonEvent>,
    filter: EventFilter,
}

impl FilteredReceiver {
    /// Receive the next event that matches the filter.
    pub async fn recv(&mut self) -> Result<DaemonEvent, broadcast::error::RecvError> {
        loop {
            let event = self.inner.recv().await?;
            if self.filter.matches(&event) {
                return Ok(event);
            }
        }
    }

    /// Try to receive an event without blocking.
    pub fn try_recv(&mut self) -> Result<DaemonEvent, broadcast::error::TryRecvError> {
        loop {
            let event = self.inner.try_recv()?;
            if self.filter.matches(&event) {
                return Ok(event);
            }
        }
    }
}

/// Return the variant name string for an event.
fn event_variant_name(event: &DaemonEvent) -> &'static str {
    match event {
        DaemonEvent::RunStarted { .. } => "RunStarted",
        DaemonEvent::RunCompleted { .. } => "RunCompleted",
        DaemonEvent::RunCached { .. } => "RunCached",
        DaemonEvent::RunQueued { .. } => "RunQueued",
        DaemonEvent::ImportantEvent { .. } => "ImportantEvent",
        DaemonEvent::ResourcePressure { .. } => "ResourcePressure",
        DaemonEvent::ConflictDetected { .. } => "ConflictDetected",
        DaemonEvent::FileChanged { .. } => "FileChanged",
        DaemonEvent::DaemonStatus { .. } => "DaemonStatus",
        DaemonEvent::RunDequeued { .. } => "RunDequeued",
    }
}

/// Determine whether two events of the same variant should be coalesced.
///
/// Coalescing logic merges events that convey identical state changes.
fn events_are_coalescable(prev: &DaemonEvent, next: &DaemonEvent) -> bool {
    match (prev, next) {
        (
            DaemonEvent::ResourcePressure { resource: r1, .. },
            DaemonEvent::ResourcePressure { resource: r2, .. },
        ) => r1 == r2,
        (DaemonEvent::FileChanged { path: p1, .. }, DaemonEvent::FileChanged { path: p2, .. }) => {
            p1 == p2
        }
        (
            DaemonEvent::ImportantEvent {
                repo: r1,
                reason: rs1,
                ..
            },
            DaemonEvent::ImportantEvent {
                repo: r2,
                reason: rs2,
                ..
            },
        ) => r1 == r2 && rs1 == rs2,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_emit_and_subscribe_all() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe_all();

        let event = DaemonEvent::DaemonStatus {
            status: "starting".to_string(),
            detail: None,
        };
        bus.emit(event.clone());

        let received = rx.recv().await.unwrap();
        assert_eq!(received, event);
    }

    #[tokio::test]
    async fn test_filtered_subscription() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe_filtered(EventFilter::Variant("RunCompleted".to_string()));

        bus.emit(DaemonEvent::RunStarted {
            run_id: "r1".into(),
            repo: "/test".into(),
            command: "cargo build".into(),
            classification: "build".into(),
            started_at: Utc::now(),
        });
        bus.emit(DaemonEvent::RunCompleted {
            run_id: "r1".into(),
            repo: "/test".into(),
            exit_code: 0,
            duration_ms: 1234,
            cached: false,
        });

        let received = rx.recv().await.unwrap();
        assert!(matches!(received, DaemonEvent::RunCompleted { .. }));
    }

    #[tokio::test]
    async fn test_coalescence() {
        // Build from explicit sender so the receiver is guaranteed active.
        let (tx, _) = tokio::sync::broadcast::channel::<DaemonEvent>(256);
        let mut rx = tx.subscribe();
        let bus = EventBus {
            tx,
            coalesce: std::sync::Arc::new(DashMap::new()),
        };

        bus.emit(DaemonEvent::RunStarted {
            run_id: "r0".into(),
            repo: "/test".into(),
            command: "cargo build".into(),
            classification: "build".into(),
            started_at: chrono::Utc::now(),
        });

        bus.emit(DaemonEvent::ResourcePressure {
            resource: "cpu".into(),
            level: 90,
            description: "high".into(),
        });

        let coalesced = bus.emit(DaemonEvent::ResourcePressure {
            resource: "cpu".into(),
            level: 92,
            description: "very high".into(),
        });
        assert!(
            !coalesced,
            "second ResourcePressure within window must coalesce"
        );

        let _ = rx.recv().await.unwrap();
        let second = rx.recv().await.unwrap();
        assert!(matches!(second, DaemonEvent::ResourcePressure { .. }));

        let third = tokio::time::timeout(Duration::from_millis(200), rx.recv()).await;
        assert!(third.is_err(), "third event must not arrive (coalesced)");
    }

    #[test]
    fn test_event_serialization() {
        let event = DaemonEvent::RunCached {
            run_id: "abc".into(),
            repo: "/repo".into(),
            command: "npm test".into(),
            cache_age: "5m".into(),
        };
        let json = serde_json::to_string(&event).unwrap();
        let parsed: DaemonEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(event, parsed);
    }
}
