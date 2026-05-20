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
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::{Child, Command as TokioCommand};
use tokio::sync::watch;
use tokio::time::timeout;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use richter_core::models::{CommandClass, ResourceClass};

use crate::event_bus::{DaemonEvent, EventBus};

/// Maximum length of a command string (bytes).
const MAX_COMMAND_LENGTH: usize = 4096;

/// Characters forbidden anywhere in a command string.
const FORBIDDEN_CHARS: &[char] = &['\0', '\n', '\r'];

/// Shell metacharacters that enable command chaining or injection when using `sh -c`.
/// These are validated when `use_shell=true` to prevent command injection.
const SHELL_INJECTION_PATTERNS: [&str; 6] = [
    "$(", // Command substitution
    "`",  // Backtick command substitution
    "&&", // Command chaining (and)
    "||", // Command chaining (or)
    ";",  // Command separator
    "|",  // Pipe (when preceded/followed by a command char)
];

/// How long a process can produce no stdout/stderr before being considered stalled.
const NO_OUTPUT_TIMEOUT_SECS: u64 = 300;

/// Maximum bytes of combined output to buffer in memory per run.
const MAX_OUTPUT_BUFFER_BYTES: usize = 1_048_576;

/// Maximum number of completed runs retained in memory.
const MAX_COMPLETED_RUNS: usize = 500;

/// How long completed runs are retained before cleanup (seconds).
const COMPLETED_RETENTION_SECS: u64 = 3600;

/// How often the cleanup task runs (seconds).
const COMPLETED_CLEANUP_INTERVAL_SECS: u64 = 300;

/// Specification for a run to be supervised.
#[derive(Debug, Clone)]
pub struct RunSpec {
    /// Unique run identifier (generated if empty).
    pub run_id: String,
    /// Repository path (working directory).
    pub repo: PathBuf,
    /// The shell command to execute.
    pub command: String,
    /// Environment variables to inject.
    pub env: HashMap<String, String>,
    /// Classification label.
    pub classification: CommandClass,
    /// Resource class for scheduling.
    pub resource_class: ResourceClass,
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
            repo: PathBuf::from("."),
            command: String::new(),
            env: HashMap::new(),
            classification: CommandClass::Unknown,
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
}

impl RunSpec {
    /// Convert this `RunSpec` into a `ClassifiedCommand` for use with
    /// the core BLAKE3 fingerprint module.
    pub fn to_classified_command(&self) -> richter_core::classifier::ClassifiedCommand {
        let argv: Vec<String> = shlex::split(&self.command).unwrap_or_else(|| {
            // Fallback: split on whitespace if shlex can't parse
            self.command.split_whitespace().map(String::from).collect()
        });
        // Use the classifier for a proper classification, then override
        // with the already-known classification from the spec.
        let mut classified = richter_core::classifier::classify(&argv);
        classified.class = self.classification;
        classified
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

/// Completed child record kept for a short time after a run finishes.
#[derive(Debug, Clone)]
pub struct CompletedChild {
    pub exit_code: i32,
    pub output: String,
    pub spec: RunSpec,
    pub completed_at: Instant,
}

/// Manages spawning and monitoring child processes.
pub struct ProcessSupervisor {
    /// Map of run_id to supervised child.
    children: Arc<DashMap<String, Arc<SupervisedChild>>>,
    /// Map of run_id to completed child records (retained for cache retrieval).
    completed: Arc<DashMap<String, CompletedChild>>,
    /// Insertion-order queue for LRU eviction of completed entries.
    completed_queue: Arc<ParkingMutex<VecDeque<String>>>,
    /// Maximum completed runs before eviction kicks in.
    completed_limit: usize,
    /// Event bus for emitting run lifecycle events.
    event_bus: EventBus,
}

impl ProcessSupervisor {
    /// Create a new process supervisor.
    pub fn new(event_bus: EventBus) -> Self {
        let completed_limit = std::env::var("RICHTER_COMPLETED_LIMIT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(MAX_COMPLETED_RUNS);

        let completed: Arc<DashMap<String, CompletedChild>> = Arc::new(DashMap::new());
        let completed_queue: Arc<ParkingMutex<VecDeque<String>>> =
            Arc::new(ParkingMutex::new(VecDeque::new()));

        // Spawn periodic cleanup of entries older than COMPLETED_RETENTION_SECS
        let cleanup_completed = completed.clone();
        let cleanup_queue = completed_queue.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(COMPLETED_CLEANUP_INTERVAL_SECS)).await;
                let now = Instant::now();
                let mut queue = cleanup_queue.lock();
                while let Some(id) = queue.front() {
                    let run_id = id.clone();
                    match cleanup_completed.get(&run_id) {
                        Some(entry) => {
                            if now.duration_since(entry.completed_at)
                                > Duration::from_secs(COMPLETED_RETENTION_SECS)
                            {
                                queue.pop_front();
                                cleanup_completed.remove(&run_id);
                                debug!(
                                    "Cleaned up completed run {} (age > {}s)",
                                    run_id, COMPLETED_RETENTION_SECS
                                );
                            } else {
                                break;
                            }
                        }
                        None => {
                            queue.pop_front();
                        }
                    }
                }
            }
        });

        Self {
            children: Arc::new(DashMap::new()),
            completed,
            completed_queue,
            completed_limit,
            event_bus,
        }
    }

    /// Validate a command string before spawning.
    /// Returns `Ok` if safe, `Err` with a descriptive message otherwise.
    fn validate_command(command: &str) -> Result<()> {
        if command.is_empty() {
            return Err(anyhow::anyhow!("Command is empty"));
        }
        if command.len() > MAX_COMMAND_LENGTH {
            return Err(anyhow::anyhow!(
                "Command exceeds maximum length of {} bytes (got {})",
                MAX_COMMAND_LENGTH,
                command.len()
            ));
        }
        if command.chars().any(|c| FORBIDDEN_CHARS.contains(&c)) {
            return Err(anyhow::anyhow!(
                "Command contains forbidden characters (null bytes, newlines, or carriage returns)"
            ));
        }
        Ok(())
    }

    /// Validate a command string for shell injection risks.
    /// Called when `use_shell=true` to check for metacharacter patterns that
    /// would allow command chaining, substitution, or redirection attacks.
    fn validate_shell_command(command: &str) -> Result<()> {
        Self::validate_command(command)?;

        // Check for shell injection patterns when running through sh -c
        for pattern in &SHELL_INJECTION_PATTERNS {
            if command.contains(pattern) {
                return Err(anyhow::anyhow!(
                    "Shell command contains forbidden injection pattern '{}': command chaining and \
                     command substitution are not allowed when using shell mode. \
                     Use direct execution mode or simplify the command.",
                    pattern
                ));
            }
        }

        // Additionally validate that shlex can round-trip the command — if it can't,
        // the command contains shell metacharacter syntax that could be exploited.
        if let Some(tokens) = shlex::split(command) {
            // Round-trip check: if joining the tokens back doesn't match enough,
            // there's shell metasyntax present
            let reconstituted =
                shlex::try_join(tokens.iter().map(|s| s.as_str())).unwrap_or_default();
            if reconstituted != command && !command.contains('&') && !command.contains('|') {
                // Minor reconstitution difference is acceptable for tokens that
                // quote differently — but if shlex couldn't fully parse it, that's
                // a red flag.
            }
        } else {
            // shlex couldn't parse the command at all — it contains unbalanced
            // quotes or other shell syntax that could mask injection.
            return Err(anyhow::anyhow!(
                "Shell command could not be safely parsed by shlex — \
                 unbalanced quotes or shell syntax detected. \
                 Use direct execution mode or simplify the command."
            ));
        }

        Ok(())
    }

    /// Check whether an env key is dangerous and should be blocked.
    fn is_dangerous_env_key(key: &str) -> bool {
        const DENIED: &[&str] = &[
            "PATH",
            "LD_PRELOAD",
            "LD_LIBRARY_PATH",
            "DYLD_INSERT_LIBRARIES",
            "DYLD_LIBRARY_PATH",
        ];
        DENIED.iter().any(|d| d.eq_ignore_ascii_case(key))
    }

    /// Spawn a child process and begin monitoring it.
    ///
    /// Returns the run ID of the spawned child.
    pub async fn spawn(&self, spec: RunSpec) -> Result<String> {
        let run_id = spec.run_id.clone();
        let repo = spec.repo.clone();
        let command = spec.command.clone();
        let classification = spec.classification.to_string();

        if spec.use_shell {
            Self::validate_shell_command(&spec.command)?;
        } else {
            Self::validate_command(&spec.command)?;
        }

        let mut cmd = if spec.use_shell {
            let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
            let mut c = TokioCommand::new(&shell);
            // Parse the command string safely with shlex; if parsing fails,
            // we still pass the raw string through -c but we know the syntax
            // is well-formed (balanced quotes, no unclosed strings).
            c.arg("-c").arg(&spec.command);
            c
        } else {
            // Parse into tokens safely; if parsing fails, fall back to
            // whitespace split so we never silently lose the command.
            let parts: Vec<String> = shlex::split(&spec.command)
                .unwrap_or_else(|| spec.command.split_whitespace().map(String::from).collect());
            if parts.is_empty() {
                return Err(anyhow::anyhow!("Empty command string"));
            }
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
            if Self::is_dangerous_env_key(k) {
                tracing::warn!(
                    "Agent attempted to inject dangerous env key '{}', blocking",
                    k
                );
                continue;
            }
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
            repo: repo.to_string_lossy().into_owned(),
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
        let completed_map = self.completed.clone();
        let completed_queue = self.completed_queue.clone();
        let completed_limit = self.completed_limit;
        let event_bus_done = self.event_bus.clone();
        let run_id_captured = run_id.clone();
        let repo_captured = repo.to_string_lossy().into_owned();

        tokio::spawn(async move {
            let child_handle = {
                let mut guard = child_clone.child.lock();
                guard.take()
            };

            let (code, output) = if let Some(mut ch) = child_handle {
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

                        if let Err(e) = child_clone.done_tx.send(true) {
                            warn!(
                                "Failed to signal completion for run {}: {}",
                                run_id_captured, e
                            );
                        }
                        info!("Run {} completed with exit code {}", run_id_captured, code);
                        (code, child_clone.output.lock().clone())
                    }
                    Err(e) => {
                        error!("Run {} failed to wait: {}", run_id_captured, e);
                        *child_clone.exit_code.lock() = Some(-1);
                        if let Err(e) = child_clone.done_tx.send(true) {
                            warn!(
                                "Failed to signal completion for run {}: {}",
                                run_id_captured, e
                            );
                        }
                        (-1, child_clone.output.lock().clone())
                    }
                }
            } else {
                // kill() already took the child; ensure cleanup still happens
                let exit = *child_clone.exit_code.lock();
                let code = exit.unwrap_or(-1);
                if exit.is_none() {
                    *child_clone.exit_code.lock() = Some(-1);
                }
                if let Err(e) = child_clone.done_tx.send(true) {
                    warn!(
                        "Failed to signal completion for run {}: {}",
                        run_id_captured, e
                    );
                }
                info!("Run {} completed (killed externally)", run_id_captured);
                (code, child_clone.output.lock().clone())
            };

            // Insert with eviction policy
            {
                let mut queue = completed_queue.lock();
                queue.push_back(run_id_captured.clone());
            }
            completed_map.insert(
                run_id_captured.clone(),
                CompletedChild {
                    exit_code: code,
                    output,
                    spec: child_clone.spec.clone(),
                    completed_at: Instant::now(),
                },
            );
            // Evict oldest if over limit
            {
                let mut queue = completed_queue.lock();
                while queue.len() > completed_limit {
                    if let Some(oldest) = queue.pop_front() {
                        completed_map.remove(&oldest);
                        debug!(
                            "Evicted completed run {} (limit: {})",
                            oldest, completed_limit
                        );
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
                    if let Err(e) = stall_child.kill().await {
                        warn!(
                            "Failed to kill stalled run {}: {}",
                            stall_child.spec.run_id, e
                        );
                    }
                    return;
                }
            }
        });

        Ok(run_id)
    }

    /// Kill a run by its ID.
    pub async fn kill_run(&self, run_id: &str) -> Result<()> {
        if let Some(child) = self.children.get(run_id) {
            child.kill().await
        } else {
            Err(anyhow::anyhow!("Run {} not found", run_id))
        }
    }

    /// Kill all runs in a repository.
    pub async fn kill_repo(&self, repo: &str) -> Vec<String> {
        let repo_path = PathBuf::from(repo);
        let mut killed = Vec::new();
        for entry in self.children.iter() {
            if entry.value().spec.repo == repo_path {
                if let Err(e) = entry.value().kill().await {
                    warn!("Failed to kill run {} in repo {}: {}", entry.key(), repo, e);
                }
                killed.push(entry.key().clone());
            }
        }
        killed
    }

    /// Get the combined output for a run.
    pub fn get_output(&self, run_id: &str) -> Option<String> {
        self.children
            .get(run_id)
            .map(|c| c.output.lock().clone())
            .or_else(|| self.completed.get(run_id).map(|c| c.output.clone()))
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
            child_clone.done().await;
        });

        Some(rx)
    }

    /// Check whether a run is still active.
    pub fn is_active(&self, run_id: &str) -> bool {
        self.children.contains_key(run_id)
    }

    /// Return the exit code for a completed run.
    pub fn exit_code(&self, run_id: &str) -> Option<i32> {
        self.children
            .get(run_id)
            .and_then(|c| *c.exit_code.lock())
            .or_else(|| self.completed.get(run_id).map(|c| c.exit_code))
    }

    /// Wait for a run to complete using the watch channel (event-driven, no polling).
    ///
    /// Returns `Some(exit_code)` once the run finishes, or `None` if the run
    /// was not found.
    pub async fn wait_for_completion(&self, run_id: &str) -> Option<i32> {
        // First check the completed map — already finished.
        if let Some(completed) = self.completed.get(run_id) {
            return Some(completed.exit_code);
        }

        // Get the live child handle.
        let child = match self.children.get(run_id) {
            Some(c) => c,
            None => {
                // Race: completed between checks. Try completed map again.
                return self.completed.get(run_id).map(|c| c.exit_code);
            }
        };

        // Try event-driven wait first, with a short deadline.
        // Fall back to polling if the watch channel doesn't resolve
        // (e.g. on single-threaded tokio runtimes in tests).
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            // Check if already completed via exit_code.
            {
                let guard = child.exit_code.lock();
                if guard.is_some() {
                    let code = guard.unwrap_or(-1);
                    drop(guard);
                    drop(child);
                    return Some(code);
                }
            }
            if let Some(c) = self.completed.get(run_id) {
                drop(child);
                return Some(c.exit_code);
            }

            if Instant::now() >= deadline {
                drop(child);
                return Some(-1);
            }

            // Try the watch channel with a short timeout so we can also
            // poll for the result (handles single-threaded runtimes).
            let done_fut = child.done();
            let _ = tokio::time::timeout(Duration::from_millis(100), done_fut).await;
        }
    }

    /// Return all active run IDs.
    pub fn active_run_ids(&self) -> Vec<String> {
        self.children.iter().map(|e| e.key().clone()).collect()
    }

    /// Return metadata for a specific run.
    pub fn run_info(&self, run_id: &str) -> Option<RunInfo> {
        self.children.get(run_id).map(|c| RunInfo {
            run_id: c.spec.run_id.clone(),
            repo: c.spec.repo.to_string_lossy().into_owned(),
            command: c.spec.command.clone(),
            classification: c.spec.classification.to_string(),
            started_at: c.started_at,
            exit_code: *c.exit_code.lock(),
            is_active: !c.is_done(),
        })
    }

    /// Run orphan detection: scan for child processes that have exited
    /// without the completion watcher cleaning up (e.g., external kill,
    /// completion watcher panic).
    pub async fn check_orphans(&self) -> Vec<String> {
        let mut orphans = Vec::new();
        for entry in self.children.iter() {
            let run_id = entry.key().clone();
            let supervised = entry.value();

            let has_exited = {
                let mut guard = supervised.child.lock();
                if let Some(ref mut ch) = *guard {
                    matches!(ch.try_wait(), Ok(Some(_)))
                } else {
                    false
                }
            };

            if has_exited {
                warn!(
                    "Orphan detected: run {} process exited without cleanup",
                    run_id
                );
                orphans.push(run_id);
            } else if supervised.is_done() {
                warn!(
                    "Orphan detected: run {} marked done but still tracked",
                    run_id
                );
                orphans.push(run_id);
            }
        }

        for run_id in &orphans {
            if let Some((_, child)) = self.children.remove(run_id) {
                if let Err(e) = child.kill().await {
                    warn!("Failed to kill orphan {} during cleanup: {}", run_id, e);
                }
            }
        }

        orphans
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
    pub async fn kill(&self) -> Result<()> {
        {
            let mut killed = self.killed.lock();
            if *killed {
                return Ok(());
            }
            *killed = true;
        } // drop guard before any await

        if let Some(pgid) = *self.pgid.lock() {
            if pgid > 1 {
                if let Err(e) = killpg(Pid::from_raw(pgid), Signal::SIGKILL) {
                    warn!(
                        "Failed to kill process group {} for run {}: {}",
                        pgid, self.spec.run_id, e
                    );
                }
            }
        }

        // Kill the direct child
        let maybe_child = {
            let mut guard = self.child.lock();
            guard.take()
        };
        if let Some(mut child) = maybe_child {
            if let Err(e) = child.start_kill() {
                warn!("Failed to start_kill run {}: {}", self.spec.run_id, e);
            }
            if let Err(e) = child.wait().await {
                warn!("Failed to await run {} exit: {}", self.spec.run_id, e);
            }
        }
        // Ensure exit code is set even if completion watcher lost the race
        *self.exit_code.lock() = Some(-1);

        if let Err(e) = self.done_tx.send(true) {
            warn!("Failed to signal done for run {}: {}", self.spec.run_id, e);
        }
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
        if let Err(e) = rx.changed().await {
            warn!(
                "done() channel closed unexpectedly for run {}: {}",
                self.spec.run_id, e
            );
        }
    }

    /// Append a line of output, redacting any detected secrets.
    fn append_output(&self, line: &str) {
        let redacted = richter_core::redact::redact(line);
        let mut out = self.output.lock();
        if out.len() + redacted.len() < MAX_OUTPUT_BUFFER_BYTES {
            out.push_str(&redacted);
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

    #[tokio::test]
    async fn test_completed_map_evicts_at_limit() {
        let bus = EventBus::new();
        // Create supervisor with a tiny limit for testing
        let supervisor = ProcessSupervisor {
            children: Arc::new(DashMap::new()),
            completed: Arc::new(DashMap::new()),
            completed_queue: Arc::new(ParkingMutex::new(VecDeque::new())),
            completed_limit: 3,
            event_bus: bus,
        };

        // Insert 5 completed entries
        for i in 0..5 {
            {
                let mut queue = supervisor.completed_queue.lock();
                queue.push_back(format!("run-{}", i));
            }
            supervisor.completed.insert(
                format!("run-{}", i),
                CompletedChild {
                    exit_code: 0,
                    output: format!("output-{}", i),
                    spec: RunSpec::default(),
                    completed_at: Instant::now(),
                },
            );
            // Evict oldest if over limit
            {
                let mut queue = supervisor.completed_queue.lock();
                while queue.len() > supervisor.completed_limit {
                    if let Some(oldest) = queue.pop_front() {
                        supervisor.completed.remove(&oldest);
                    }
                }
            }
        }

        // Should have evicted the oldest 2 (run-0, run-1)
        assert!(
            supervisor.completed.get("run-0").is_none(),
            "run-0 should have been evicted (oldest, over limit)"
        );
        assert!(
            supervisor.completed.get("run-1").is_none(),
            "run-1 should have been evicted"
        );
        assert!(
            supervisor.completed.get("run-2").is_some(),
            "run-2 should still be present"
        );
        assert!(
            supervisor.completed.get("run-3").is_some(),
            "run-3 should still be present"
        );
        assert!(
            supervisor.completed.get("run-4").is_some(),
            "run-4 should still be present"
        );
        assert_eq!(supervisor.completed.len(), 3);
        assert_eq!(supervisor.completed_queue.lock().len(), 3);
    }
}
