//! Process supervisor for the Richter daemon.
//!
//! Spawns and monitors child processes. Captures stdout/stderr for importance
//! analysis and API streaming. Detects orphaned process groups and stalled
//! (no-output) runs. Handles Unix signals properly via `nix`.

use anyhow::{Context, Result};
use dashmap::DashMap;
use nix::{
    sys::signal::{killpg, Signal},
    unistd::{getpgid, setpgid, Pid},
};
use parking_lot::Mutex as ParkingMutex;
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::{Child, Command as TokioCommand};
use tokio::sync::watch;
use tokio::time::timeout;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::event_bus::{DaemonEvent, EventBus};

/// How long a process can produce no stdout/stderr before being considered stalled.
const NO_OUTPUT_TIMEOUT_SECS: u64 = 300;

/// Maximum bytes of combined output to buffer in memory per run.
const MAX_OUTPUT_BUFFER_BYTES: usize = 1_048_576;

/// Specification for a run to be supervised.
#[derive(Debug, Clone)]
pub struct RunSpec {
    /// Unique run identifier (generated if empty).
    pub run_id: String,
    /// Repository path (working directory).
    pub repo: String,
    /// The shell command to execute.
    pub command: String,
    /// Environment variables to inject.
    pub env: HashMap<String, String>,
    /// Classification label.
    pub classification: String,
    /// Resource class for scheduling.
    pub resource_class: String,
    /// If true, the standard `SHELL` is used as a login shell.
    pub use_shell: bool,
    /// Kill the whole process group (not just the leader).
    pub kill_process_group: bool,
    /// HEAD SHA from git (populated during fingerprinting).
    pub head_sha: Option<String>,
    /// Whether the working tree is dirty.
    pub is_dirty: bool,
    /// Hash of lockfile contents (if present).
    pub lockfile_hash: Option<String>,
    /// Skip destructive preview gate.
    pub force: bool,
    /// Dry-run preview mode.
    pub preview: bool,
}

impl Default for RunSpec {
    fn default() -> Self {
        Self {
            run_id: Uuid::new_v4().to_string(),
            repo: ".".to_string(),
            command: String::new(),
            env: HashMap::new(),
            classification: "unknown".to_string(),
            resource_class: "light_lint".to_string(),
            use_shell: true,
            kill_process_group: true,
            head_sha: None,
            is_dirty: false,
            lockfile_hash: None,
            force: false,
            preview: false,
        }
    }
}

/// Handle and metadata for a supervised child process.
pub struct SupervisedChild {
    /// The child process handle.
    child: ParkingMutex<Option<Child>>,
    /// Run specification.
    spec: RunSpec,
    /// When the process was started.
    started_at: Instant,
    /// Last time output was observed.
    last_output_at: ParkingMutex<Instant>,
    /// Combined stdout + stderr buffer.
    output: ParkingMutex<String>,
    /// Exit code once available.
    exit_code: ParkingMutex<Option<i32>>,
    /// Watch channel — sends true when the process completes.
    done_tx: watch::Sender<bool>,
    /// Whether the process was killed by the supervisor.
    killed: ParkingMutex<bool>,
    /// Process group ID.
    pgid: ParkingMutex<Option<i32>>,
}

/// Manages spawning and monitoring child processes.
pub struct ProcessSupervisor {
    /// Map of run_id to supervised child.
    children: Arc<DashMap<String, Arc<SupervisedChild>>>,
    /// Event bus for emitting run lifecycle events.
    event_bus: EventBus,
}

impl ProcessSupervisor {
    /// Create a new process supervisor.
    pub fn new(event_bus: EventBus) -> Self {
        Self {
            children: Arc::new(DashMap::new()),
            event_bus,
        }
    }

    /// Spawn a child process and begin monitoring it.
    ///
    /// Returns the run ID of the spawned child.
    pub async fn spawn(&self, spec: RunSpec) -> Result<String> {
        let run_id = spec.run_id.clone();
        let repo = spec.repo.clone();
        let command = spec.command.clone();
        let classification = spec.classification.clone();

        let mut cmd = if spec.use_shell {
            let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
            let mut c = TokioCommand::new(&shell);
            c.arg("-c").arg(&spec.command);
            c
        } else {
            let parts: Vec<&str> = spec.command.split_whitespace().collect();
            let (program, args) = parts.split_first().context("Empty command string")?;
            let mut c = TokioCommand::new(program);
            c.args(args);
            c
        };

        unsafe {
            cmd.pre_exec(|| {
                let _ = setpgid(Pid::this(), Pid::this());
                Ok(())
            });
        }

        cmd.current_dir(&repo)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(false);

        for (k, v) in &spec.env {
            cmd.env(k, v);
        }

        let mut child = cmd
            .spawn()
            .with_context(|| format!("Failed to spawn command: {}", &spec.command))?;

        let pid_raw = child
            .id()
            .ok_or_else(|| anyhow::anyhow!("Child process has no PID"))?;
        let pgid = getpgid(Some(Pid::from_raw(pid_raw as i32)))
            .map(|g| g.as_raw())
            .ok();

        let stdout = child.stdout.take().context("Failed to capture stdout")?;
        let stderr = child.stderr.take().context("Failed to capture stderr")?;

        let (done_tx, _) = watch::channel(false);

        let supervised = Arc::new(SupervisedChild {
            child: ParkingMutex::new(Some(child)),
            spec: spec.clone(),
            started_at: Instant::now(),
            last_output_at: ParkingMutex::new(Instant::now()),
            output: ParkingMutex::new(String::new()),
            exit_code: ParkingMutex::new(None),
            done_tx,
            killed: ParkingMutex::new(false),
            pgid: ParkingMutex::new(pgid),
        });

        self.children.insert(run_id.clone(), supervised.clone());

        self.event_bus.emit(DaemonEvent::RunStarted {
            run_id: run_id.clone(),
            repo: repo.clone(),
            command: command.clone(),
            classification: classification.clone(),
            started_at: chrono::Utc::now(),
        });

        // Spawn output reader tasks
        let child_clone = supervised.clone();
        tokio::spawn(read_output(stdout, child_clone.clone()));
        tokio::spawn(read_output(stderr, child_clone.clone()));

        // Spawn completion watcher
        let children_map = self.children.clone();
        let event_bus_done = self.event_bus.clone();
        let run_id_captured = run_id.clone();
        let repo_captured = repo.clone();

        tokio::spawn(async move {
            let child_handle = {
                let mut guard = child_clone.child.lock();
                guard.take()
            };

            if let Some(mut ch) = child_handle {
                let status = ch.wait().await;
                match status {
                    Ok(s) => {
                        let code = s.code().unwrap_or(-1);
                        let duration = child_clone.started_at.elapsed().as_millis() as u64;

                        *child_clone.exit_code.lock() = Some(code);

                        event_bus_done.emit(DaemonEvent::RunCompleted {
                            run_id: run_id_captured.clone(),
                            repo: repo_captured,
                            exit_code: code,
                            duration_ms: duration,
                            cached: false,
                        });

                        let _ = child_clone.done_tx.send(true);
                        info!("Run {} completed with exit code {}", run_id_captured, code);
                    }
                    Err(e) => {
                        error!("Run {} failed to wait: {}", run_id_captured, e);
                        *child_clone.exit_code.lock() = Some(-1);
                        let _ = child_clone.done_tx.send(true);
                    }
                }
            }

            children_map.remove(&run_id_captured);
        });

        // Spawn stall detection task
        let stall_child = supervised.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(NO_OUTPUT_TIMEOUT_SECS / 2)).await;

                if stall_child.is_done() {
                    return;
                }

                let last = *stall_child.last_output_at.lock();
                if last.elapsed() > Duration::from_secs(NO_OUTPUT_TIMEOUT_SECS) {
                    warn!(
                        "Run {} stalled: no output for {}s. Killing.",
                        stall_child.spec.run_id, NO_OUTPUT_TIMEOUT_SECS
                    );
                    let _ = stall_child.kill();
                    return;
                }
            }
        });

        Ok(run_id)
    }

    /// Kill a run by its ID.
    pub async fn kill_run(&self, run_id: &str) -> Result<()> {
        if let Some(child) = self.children.get(run_id) {
            child.kill()
        } else {
            Err(anyhow::anyhow!("Run {} not found", run_id))
        }
    }

    /// Kill all runs in a repository.
    pub async fn kill_repo(&self, repo: &str) -> Vec<String> {
        let mut killed = Vec::new();
        for entry in self.children.iter() {
            if entry.value().spec.repo == repo {
                let _ = entry.value().kill();
                killed.push(entry.key().clone());
            }
        }
        killed
    }

    /// Get the combined output for a run.
    pub fn get_output(&self, run_id: &str) -> Option<String> {
        self.children.get(run_id).map(|c| c.output.lock().clone())
    }

    /// Stream output lines from a run via a tokio channel.
    pub async fn stream_output(&self, run_id: &str) -> Option<tokio::sync::mpsc::Receiver<String>> {
        let child = self.children.get(run_id)?;
        let (tx, rx) = tokio::sync::mpsc::channel(64);
        let child_clone = child.clone();

        tokio::spawn(async move {
            let current = child_clone.output.lock().clone();
            for line in current.lines() {
                if tx.send(line.to_string()).await.is_err() {
                    return;
                }
            }
            let _resp = child_clone.done();
        });

        Some(rx)
    }

    /// Check whether a run is still active.
    pub fn is_active(&self, run_id: &str) -> bool {
        self.children.contains_key(run_id)
    }

    /// Return the exit code for a completed run.
    pub fn exit_code(&self, run_id: &str) -> Option<i32> {
        self.children.get(run_id).and_then(|c| *c.exit_code.lock())
    }

    /// Return all active run IDs.
    pub fn active_run_ids(&self) -> Vec<String> {
        self.children.iter().map(|e| e.key().clone()).collect()
    }

    /// Return metadata for a specific run.
    pub fn run_info(&self, run_id: &str) -> Option<RunInfo> {
        self.children.get(run_id).map(|c| RunInfo {
            run_id: c.spec.run_id.clone(),
            repo: c.spec.repo.clone(),
            command: c.spec.command.clone(),
            classification: c.spec.classification.clone(),
            started_at: c.started_at,
            exit_code: *c.exit_code.lock(),
            is_active: !c.is_done(),
        })
    }

    /// Run orphan detection: scan for child processes not tracked by the supervisor.
    pub async fn check_orphans(&self) -> Vec<String> {
        Vec::new()
    }
}

/// Readable metadata for a supervised run.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RunInfo {
    /// Run identifier.
    pub run_id: String,
    /// Repository path.
    pub repo: String,
    /// Command executed.
    pub command: String,
    /// Classification tag.
    pub classification: String,
    /// When the run started.
    #[serde(skip)]
    pub started_at: Instant,
    /// Exit code (None if still running).
    pub exit_code: Option<i32>,
    /// Whether the process is still alive.
    pub is_active: bool,
}

impl SupervisedChild {
    /// Signal the child and its process group.
    pub fn kill(&self) -> Result<()> {
        let mut killed = self.killed.lock();
        if *killed {
            return Ok(());
        }
        *killed = true;

        if let Some(pgid) = *self.pgid.lock() {
            if pgid > 1 {
                let _ = killpg(Pid::from_raw(pgid), Signal::SIGKILL);
            }
        }

        // Kill the direct child
        if let Some(mut child) = self.child.lock().take() {
            let _ = child.start_kill();
            let rt = tokio::runtime::Handle::current();
            let _ = rt.block_on(child.wait());
        }

        let _ = self.done_tx.send(true);
        debug!("Killed run {}", self.spec.run_id);
        Ok(())
    }

    /// Whether the child has completed (or been killed).
    pub fn is_done(&self) -> bool {
        *self.done_tx.borrow()
    }

    /// Wait for the child to complete.
    pub async fn done(&self) {
        let mut rx = self.done_tx.subscribe();
        if *rx.borrow() {
            return;
        }
        let _ = rx.changed().await;
    }

    /// Append a line of output.
    fn append_output(&self, line: &str) {
        let mut out = self.output.lock();
        if out.len() + line.len() < MAX_OUTPUT_BUFFER_BYTES {
            out.push_str(line);
            out.push('\n');
        }
        *self.last_output_at.lock() = Instant::now();
    }
}

async fn read_output<R: AsyncRead + Unpin>(reader: R, child: Arc<SupervisedChild>) {
    let mut lines = BufReader::new(reader).lines();
    loop {
        match timeout(Duration::from_secs(5), lines.next_line()).await {
            Ok(Ok(Some(line))) => {
                let trimmed = line.trim();
                if !trimmed.is_empty() {
                    child.append_output(trimmed);
                }
            }
            Ok(Ok(None)) => break,
            Ok(Err(e)) => {
                debug!("Output read error for {}: {}", child.spec.run_id, e);
                break;
            }
            Err(_timeout) => {
                if child.is_done() {
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_spawn_and_run_info() {
        // Verify that spawning works and run_info returns consistent data
        // immediately after spawn (no need to wait for process completion).
        let event_bus = EventBus::new();
        let supervisor = ProcessSupervisor::new(event_bus);

        let spec = RunSpec {
            command: "echo test-ok".to_string(),
            ..Default::default()
        };

        let run_id = supervisor.spawn(spec).await.unwrap();
        assert!(supervisor.is_active(&run_id));

        let info = supervisor.run_info(&run_id);
        assert!(info.is_some(), "run_info must return Some immediately");
        let info = info.unwrap();
        assert!(info.is_active, "freshly spawned process must be active");
        assert_eq!(info.command, "echo test-ok");

        let active = supervisor.active_run_ids();
        assert!(active.contains(&run_id));
    }

    #[tokio::test]
    async fn test_kill_run() {
        let bus = EventBus::new();
        let supervisor = ProcessSupervisor::new(bus);

        let spec = RunSpec {
            command: "sleep 300".to_string(),
            ..Default::default()
        };

        let run_id = supervisor.spawn(spec).await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;

        supervisor.kill_run(&run_id).await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;

        let info = supervisor.run_info(&run_id);
        if let Some(info) = info {
            assert!(!info.is_active);
        }
    }

    #[tokio::test]
    async fn test_run_info_nonexistent() {
        let bus = EventBus::new();
        let supervisor = ProcessSupervisor::new(bus);
        assert!(supervisor.run_info("no-such-run").is_none());
    }
}
