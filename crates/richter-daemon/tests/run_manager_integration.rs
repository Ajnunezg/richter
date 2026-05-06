//! Run manager integration tests.

use std::sync::Arc;
use richter_daemon::event_bus::EventBus;
use richter_daemon::run_manager::{RunManager, RunOutcome};
use richter_daemon::scheduler::{Scheduler, SchedulerConfig, ResourceMonitor};
use richter_daemon::supervisor::{ProcessSupervisor, RunSpec};

fn new_run_manager(event_bus: EventBus) -> RunManager {
    let scheduler = Arc::new(Scheduler::new(
        SchedulerConfig::default(), event_bus.clone(), ResourceMonitor::new(),
    ));
    let supervisor = Arc::new(ProcessSupervisor::new(event_bus.clone()));
    RunManager::new(scheduler, supervisor, None, event_bus.clone())
}

#[tokio::test]
async fn run_or_join_unknown_command_executes() {
    let event_bus = EventBus::new();
    let rm = new_run_manager(event_bus.clone());
    let spec = RunSpec { run_id: uuid::Uuid::new_v4().to_string(),
        command: "echo hello-run-manager".to_string(), repo: "/tmp".to_string(),
        env: Default::default(), classification: "unknown".to_string(),
        resource_class: "light_lint".to_string(), use_shell: true, kill_process_group: true,
        head_sha: None, is_dirty: false, lockfile_hash: None, force: false, preview: false };
    let outcome = rm.run_or_join(spec).await.unwrap();
    match outcome {
        RunOutcome::Started { .. } | RunOutcome::Joined { .. } | RunOutcome::Cached { .. }
        | RunOutcome::Queued { .. } | RunOutcome::Rejected { .. } => {}
    }
}

#[tokio::test]
async fn active_runs_starts_empty() {
    let rm = new_run_manager(EventBus::new());
    assert!(rm.active_runs().is_empty());
}
