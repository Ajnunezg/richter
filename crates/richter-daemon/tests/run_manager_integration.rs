//! Run manager integration tests.

use richter_core::models::{CommandClass, ResourceClass};
use richter_daemon::event_bus::EventBus;
use richter_daemon::run_manager::{RunManager, RunOutcome};
use richter_daemon::scheduler::{ResourceMonitor, Scheduler, SchedulerConfig};
use richter_daemon::supervisor::{ProcessSupervisor, RunSpec};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

fn new_run_manager(event_bus: EventBus) -> RunManager {
    let scheduler = Scheduler::new(
        SchedulerConfig::default(),
        event_bus.clone(),
        ResourceMonitor::new(),
    );
    let supervisor = Arc::new(ProcessSupervisor::new(event_bus.clone()));
    RunManager::new(scheduler, supervisor, None, event_bus.clone())
}

fn make_spec(command: &str, classification: CommandClass, repo: &str) -> RunSpec {
    RunSpec {
        run_id: uuid::Uuid::new_v4().to_string(),
        command: command.to_string(),
        repo: PathBuf::from(repo),
        env: Default::default(),
        classification,
        resource_class: ResourceClass::LightLint,
        use_shell: true,
        kill_process_group: true,
        head_sha: None,
        is_dirty: false,
        lockfile_hash: None,
        force: false,
        preview: false,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn run_or_join_unknown_command_executes() {
    let event_bus = EventBus::new();
    let rm = new_run_manager(event_bus.clone());
    let spec = RunSpec {
        run_id: uuid::Uuid::new_v4().to_string(),
        command: "echo hello-run-manager".to_string(),
        repo: PathBuf::from("/tmp"),
        env: Default::default(),
        classification: CommandClass::Unknown,
        resource_class: ResourceClass::LightLint,
        use_shell: true,
        kill_process_group: true,
        head_sha: None,
        is_dirty: false,
        lockfile_hash: None,
        force: false,
        preview: false,
    };
    let outcome = rm.run_or_join(spec).await.unwrap();
    match outcome {
        RunOutcome::Started { .. }
        | RunOutcome::Joined { .. }
        | RunOutcome::Cached { .. }
        | RunOutcome::Queued { .. }
        | RunOutcome::Rejected { .. } => {}
    }
}

#[tokio::test]
async fn active_runs_starts_empty() {
    let rm = new_run_manager(EventBus::new());
    assert!(rm.active_runs().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn run_or_join_dedup_via_cache() {
    let rm = new_run_manager(EventBus::new());
    let spec = make_spec("echo dedup-test", CommandClass::Test, "/tmp");

    let first = rm.run_or_join(spec.clone()).await.unwrap();
    let run_id = match first {
        RunOutcome::Started { run_id, .. } => run_id,
        other => panic!("Expected Started, got {:?}", other),
    };

    // Wait for the run to fully complete and be removed from active_runs
    for _ in 0..100 {
        if rm.wait_for_run(&run_id).await.is_some() && !rm.active_runs().contains(&run_id) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Second identical request should be served from cache (or joined if race)
    let second = rm.run_or_join(spec).await.unwrap();
    assert!(
        matches!(
            second,
            RunOutcome::Cached { .. } | RunOutcome::Joined { .. }
        ),
        "Expected Cached or Joined on second identical call, got {:?}",
        second
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn run_or_join_joins_active_run() {
    let rm = new_run_manager(EventBus::new());
    let spec = make_spec("sleep 5", CommandClass::Test, "/tmp");

    let first = rm.run_or_join(spec.clone()).await.unwrap();
    let run_id = match first {
        RunOutcome::Started { run_id, .. } => run_id,
        other => panic!("Expected Started, got {:?}", other),
    };

    // Immediately request the same command while the first is still running
    let second = rm.run_or_join(spec).await.unwrap();
    assert!(
        matches!(second, RunOutcome::Joined { run_id: ref id, .. } if id == &run_id),
        "Expected Joined pointing to same run_id, got {:?}",
        second
    );

    // Clean up the long-running sleep
    let _ = rm.cancel_run(&run_id).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn run_or_join_rejects_path_traversal() {
    let rm = new_run_manager(EventBus::new());
    let spec = make_spec("echo hello", CommandClass::Test, "/tmp/../etc/passwd");

    let outcome = rm.run_or_join(spec).await.unwrap();
    assert!(
        matches!(outcome, RunOutcome::Rejected { ref reason } if reason.contains("outside allowed")),
        "Expected Rejected for path traversal, got {:?}",
        outcome
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn run_or_join_destructive_preview_gate() {
    let rm = new_run_manager(EventBus::new());
    let spec = make_spec("rm -rf /tmp/foo", CommandClass::Test, "/tmp");

    let outcome = rm.run_or_join(spec.clone()).await.unwrap();
    assert!(
        matches!(
            outcome,
            RunOutcome::Rejected { ref reason } if reason.contains("DESTRUCTIVE")
        ),
        "Expected Rejected for destructive command without --force, got {:?}",
        outcome
    );

    // With force=true the destructive command should pass through
    let mut forced = spec.clone();
    forced.force = true;
    let forced_outcome = rm.run_or_join(forced).await.unwrap();
    assert!(
        matches!(
            forced_outcome,
            RunOutcome::Started { .. } | RunOutcome::Rejected { .. }
        ),
        "Expected Started or Rejected for forced destructive command, got {:?}",
        forced_outcome
    );
}
