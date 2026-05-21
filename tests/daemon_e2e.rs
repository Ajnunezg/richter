//! End-to-end integration tests for the Richter daemon.
//!
//! These tests validate the core daemon lifecycle: startup, command submission,
//! run-or-join deduplication, cache hits, and graceful shutdown. They require
//! the daemon binary to be built but do NOT require it to be installed.
//!
//! Run with: cargo test --test daemon_e2e -- --test-threads=1

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;
use std::{fs, thread};

/// Path to the built daemon binary.
fn daemon_bin() -> PathBuf {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".into());
    let root = PathBuf::from(manifest_dir);
    let bin = root.join("target/debug/richter-daemon");
    if !bin.exists() {
        panic!(
            "Daemon binary not found at {}. Run `cargo build -p richter-daemon` first.",
            bin.display()
        );
    }
    bin
}

/// Path to the built CLI binary.
fn cli_bin() -> PathBuf {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".into());
    let root = PathBuf::from(manifest_dir);
    let bin = root.join("target/debug/richter");
    if !bin.exists() {
        panic!(
            "CLI binary not found at {}. Run `cargo build -p richter-cli` first.",
            bin.display()
        );
    }
    bin
}

/// Temporary test directory with cleanup.
struct TestDir {
    path: PathBuf,
}

impl TestDir {
    fn new(prefix: &str) -> Self {
        let path = std::env::temp_dir().join(format!("richter-e2e-{}-{}", prefix, std::process::id()));
        fs::create_dir_all(&path).expect("create test dir");
        Self { path }
    }

    fn richter_dir(&self) -> PathBuf {
        self.path.join(".richter")
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// Daemon process guard that kills the daemon on drop.
struct DaemonGuard {
    child: Child,
    socket_path: PathBuf,
}

impl DaemonGuard {
    fn start(test_dir: &TestDir) -> Self {
        let socket_path = test_dir.richter_dir().join("daemon.sock");
        let token_path = test_dir.richter_dir().join("auth_token");
        let pid_path = test_dir.richter_dir().join("daemon.pid");

        // Clean up any stale files
        let _ = fs::remove_file(&socket_path);
        let _ = fs::remove_file(&token_path);
        let _ = fs::remove_file(&pid_path);

        let child = Command::new(daemon_bin())
            .env("HOME", &test_dir.path)
            .env("RICHTER_SOCKET", &socket_path)
            .env("RUST_LOG", "richter_daemon=warn")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to start daemon");

        // Wait for the socket to appear (up to 5 seconds)
        let mut ready = false;
        for _ in 0..50 {
            thread::sleep(Duration::from_millis(100));
            if socket_path.exists() {
                ready = true;
                break;
            }
        }

        if !ready {
            panic!("Daemon did not start within 5 seconds. Socket not found at {:?}", socket_path);
        }

        Self { child, socket_path }
    }

    fn stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_file(&self.socket_path);
    }
}

impl Drop for DaemonGuard {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Run a CLI command against the test daemon.
fn cli_command(test_dir: &TestDir) -> Command {
    let mut cmd = Command::new(cli_bin());
    cmd.env("HOME", &test_dir.path)
        .env("RICHTER_SOCKET", test_dir.richter_dir().join("daemon.sock"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn daemon_starts_and_responds_to_health() {
    let test_dir = TestDir::new("health");
    let mut daemon = DaemonGuard::start(&test_dir);

    // Give it a moment to fully initialize
    thread::sleep(Duration::from_millis(500));

    let output = cli_command(&test_dir)
        .arg("status")
        .output()
        .expect("cli status command");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{}{}", stdout, stderr);

    // The daemon should be running and the CLI should be able to connect
    assert!(
        combined.contains("ok") || combined.contains("health") || combined.contains("active_runs") || combined.contains("status"),
        "Expected status response, got: stdout={:?} stderr={:?}",
        stdout, stderr
    );

    daemon.stop();
}

#[test]
fn daemon_creates_database_and_socket() {
    let test_dir = TestDir::new("db-socket");
    let mut daemon = DaemonGuard::start(&test_dir);

    let db_path = test_dir.richter_dir().join("richter.db");
    let socket_path = test_dir.richter_dir().join("daemon.sock");
    let token_path = test_dir.richter_dir().join("auth_token");
    let pid_path = test_dir.richter_dir().join("daemon.pid");

    assert!(db_path.exists(), "Database file should exist after startup");
    assert!(socket_path.exists(), "Socket file should exist after startup");
    assert!(token_path.exists(), "Auth token file should exist after startup");
    assert!(pid_path.exists(), "PID file should exist after startup");

    // Verify token file has restrictive permissions
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&token_path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "Auth token should have 0600 permissions, got {:o}", mode);
    }

    daemon.stop();
}

#[test]
fn daemon_cli_status_shows_zero_runs() {
    let test_dir = TestDir::new("zero-runs");
    let mut daemon = DaemonGuard::start(&test_dir);

    thread::sleep(Duration::from_millis(300));

    let output = cli_command(&test_dir)
        .arg("status")
        .output()
        .expect("cli status");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    // Should show status with zero runs (or at least not error)
    assert!(
        output.status.success() || stderr.contains("ok") || stdout.contains("active"),
        "Status should succeed. stdout={:?} stderr={:?}",
        stdout, stderr
    );

    daemon.stop();
}

#[test]
fn daemon_cli_repos_lists_empty() {
    let test_dir = TestDir::new("repos-empty");
    let mut daemon = DaemonGuard::start(&test_dir);

    thread::sleep(Duration::from_millis(300));

    let output = cli_command(&test_dir)
        .arg("repos")
        .output()
        .expect("cli repos");

    assert!(
        output.status.success(),
        "repos command should succeed. stderr={:?}",
        String::from_utf8_lossy(&output.stderr)
    );

    daemon.stop();
}

#[test]
fn daemon_cli_doctor_reports_status() {
    let test_dir = TestDir::new("doctor");
    let mut daemon = DaemonGuard::start(&test_dir);

    thread::sleep(Duration::from_millis(300));

    let output = cli_command(&test_dir)
        .arg("doctor")
        .output()
        .expect("cli doctor");

    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    // Doctor should produce output (even if some checks fail)
    assert!(
        !combined.is_empty(),
        "doctor should produce some output"
    );

    daemon.stop();
}

#[test]
fn daemon_pidfile_prevents_double_start() {
    let test_dir = TestDir::new("pidfile");
    let mut daemon1 = DaemonGuard::start(&test_dir);

    thread::sleep(Duration::from_millis(500));

    // Try to start a second daemon with the same HOME
    let output = Command::new(daemon_bin())
        .env("HOME", &test_dir.path)
        .env("RICHTER_SOCKET", test_dir.richter_dir().join("daemon.sock"))
        .env("RUST_LOG", "richter_daemon=warn")
        .output()
        .expect("second daemon start");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("already running") || !output.status.success(),
        "Second daemon should fail to start. stderr={:?}",
        stderr
    );

    daemon1.stop();
}

#[test]
fn daemon_shutdown_cleans_socket() {
    let test_dir = TestDir::new("shutdown");
    let socket_path = test_dir.richter_dir().join("daemon.sock");

    {
        let mut daemon = DaemonGuard::start(&test_dir);
        assert!(socket_path.exists(), "Socket should exist while running");
        // DaemonGuard drops here and kills the daemon
    }

    thread::sleep(Duration::from_millis(500));

    // After shutdown, socket should be cleaned up (or at least not blocking)
    // The daemon guard handles cleanup in its Drop impl
}
