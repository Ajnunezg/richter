//! Richter daemon binary entry point.
//!
//! Wires together the API server, event bus, scheduler, run manager,
//! and process supervisor. Binds a Unix domain socket and blocks until
//! Ctrl-C. Optionally starts the LAN-facing mobile gateway when
//! RICHTER_MOBILE_ENABLED=true.

use anyhow::Context;
use parking_lot::Mutex as ParkingMutex;
use richter_daemon::api::{
    generate_auth_token, serve, verify_restrictive_permissions, AppState, AuthState, InstallStatus,
    ModelCallBudget, RunState, SystemState,
};
use richter_daemon::event_bus::EventBus;
use richter_daemon::metrics;
use richter_daemon::mobile::MobileGatewayState;
use richter_daemon::rate_limiter::RateLimiter;
use richter_daemon::run_manager::RunManager;
use richter_daemon::scheduler::{ResourceMonitor, Scheduler, SchedulerConfig};
use richter_daemon::supervisor::ProcessSupervisor;
use richter_daemon::watcher::{FsWatcher, WatchTarget, WatcherConfig};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::signal::unix::{signal as unix_signal, SignalKind};
use tokio::sync::watch;
use tracing::info;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

const DEFAULT_SOCKET_RELATIVE: &str = ".richter/daemon.sock";

fn home() -> String {
    std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // --- directories ---
    let data_dir = PathBuf::from(home()).join(".richter");
    std::fs::create_dir_all(&data_dir).context("Failed to create .richter data directory")?;

    // Ensure mobile TLS directory exists (for cert/key storage).
    richter_daemon::mobile::tls::ensure_mobile_dir(&data_dir)
        .context("Failed to create mobile TLS directory")?;

    // --- harden data directory permissions ---
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::metadata(&data_dir)
            .context("Failed to stat .richter directory")?
            .permissions();
        let mode = perms.mode() & 0o777;
        if mode != 0o700 {
            tracing::warn!(".richter directory has mode {:o}, correcting to 0700", mode);
            let mut p = perms.clone();
            p.set_mode(0o700);
            std::fs::set_permissions(&data_dir, p)
                .context("Failed to chmod 0700 .richter directory")?;
        }
    }

    // --- pidfile: prevent double-start + detect prior crash ---
    let pidfile_path = data_dir.join("daemon.pid");
    if pidfile_path.exists() {
        if let Ok(pid_str) = std::fs::read_to_string(&pidfile_path) {
            if let Ok(old_pid) = pid_str.trim().parse::<i32>() {
                // Check if the old process is still alive
                let running = unsafe { libc::kill(old_pid, 0) == 0 };
                if running {
                    anyhow::bail!(
                        "Another Richter daemon is already running (PID {})",
                        old_pid
                    );
                }
                tracing::warn!(
                    "Stale pidfile found for PID {} (process dead). Cleaning up.",
                    old_pid
                );
            }
        }
        let _ = std::fs::remove_file(&pidfile_path);
    }
    std::fs::write(&pidfile_path, std::process::id().to_string())
        .context("Failed to write pidfile")?;

    // --- tracing with log rotation ---
    let log_dir = data_dir.join("logs");
    std::fs::create_dir_all(&log_dir).context("Failed to create log directory")?;
    let file_appender = tracing_appender::rolling::daily(&log_dir, "richter");
    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);

    let json_format = std::env::var("RUST_LOG_FORMAT")
        .map(|v| v.eq_ignore_ascii_case("json"))
        .unwrap_or(false);

    if json_format {
        tracing_subscriber::fmt()
            .with_env_filter(
                EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| EnvFilter::new("richter_daemon=info,richter=info")),
            )
            .with_target(false)
            .with_writer(non_blocking)
            .json()
            .finish()
            .try_init()
            .context("Failed to set global tracing subscriber")?;
    } else {
        tracing_subscriber::fmt()
            .with_env_filter(
                EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| EnvFilter::new("richter_daemon=info,richter=info")),
            )
            .with_target(false)
            .with_writer(non_blocking)
            .finish()
            .try_init()
            .context("Failed to set global tracing subscriber")?;
    }

    let socket_path_str = std::env::var("RICHTER_SOCKET").unwrap_or_else(|_| {
        PathBuf::from(home())
            .join(DEFAULT_SOCKET_RELATIVE)
            .to_string_lossy()
            .to_string()
    });
    let socket_path = PathBuf::from(&socket_path_str);
    let _ = std::fs::remove_file(&socket_path);

    // --- database (open/create) ---
    let db_path = data_dir.join("richter.db");

    // Backup database before opening (best-effort)
    if db_path.exists() {
        let backup_path = data_dir.join("richter.db.backup");
        if let Err(e) = std::fs::copy(&db_path, &backup_path) {
            tracing::warn!("Failed to backup database: {}", e);
        }
    }

    let db = Arc::new(
        richter_core::db::Database::open(&db_path)
            .await
            .context("Failed to open SQLite database")?,
    );

    // --- verify encryption key management ---
    if let Err(e) = db.verify_encryption().await {
        tracing::warn!("Encryption key check failed (non-fatal): {:#}", e);
    }
    info!("Database encryption: {}", db.encryption_status());

    // --- auth token (always rotate on startup) ---
    let token_path = data_dir.join("auth_token");
    if token_path.exists() {
        // Remove old token before generating a fresh one.
        // This ensures tokens are never reused across daemon restarts.
        let _ = std::fs::remove_file(&token_path);
        info!("Removed previous auth token — generating fresh token on startup");
    }
    generate_auth_token(&token_path).context("Failed to generate auth token")?;
    verify_restrictive_permissions(&token_path)
        .context("Failed to verify auth token permissions")?;

    // --- verify database permissions ---
    verify_restrictive_permissions(&db_path).context("Failed to verify database permissions")?;

    // --- orphan reconciliation: mark stale runs from previous crash ---
    {
        use richter_core::models::RunStatus;
        match db.list_active_runs().await {
            Ok(orphans) if !orphans.is_empty() => {
                info!(
                    "Reconciling {} orphaned run(s) from previous daemon instance",
                    orphans.len()
                );
                for run in &orphans {
                    let now = chrono::Utc::now().to_rfc3339();
                    if let Err(e) = db
                        .update_run_status(
                            &run.id,
                            RunStatus::Failed,
                            Some(-1),
                            None,
                            Some(&now),
                            None,
                        )
                        .await
                    {
                        tracing::warn!("Failed to mark orphaned run {}: {}", run.id, e);
                    }
                }
            }
            Ok(_) => {} // no orphans
            Err(e) => tracing::warn!("Failed to query orphaned runs: {}", e),
        }
    }

    // --- core services ---
    let event_bus = EventBus::new();

    let scheduler = Scheduler::new(
        SchedulerConfig::default(),
        event_bus.clone(),
        ResourceMonitor::new(),
    );

    let supervisor = Arc::new(ProcessSupervisor::new(event_bus.clone()));

    let run_manager = Arc::new(RunManager::new(
        scheduler.clone(),
        supervisor.clone(),
        Some(db.clone()),
        event_bus.clone(),
    ));

    // --- filesystem watcher (best-effort) ---
    let (_shutdown_tx, shutdown_rx) = watch::channel(false);
    let watcher_healthy = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    if let Ok(mut watcher) = FsWatcher::new(
        &WatcherConfig {
            targets: vec![WatchTarget {
                root: PathBuf::from(home()),
                repo_name: "__home".into(),
                track_git: true,
            }],
            ..WatcherConfig::default()
        },
        event_bus.clone(),
    ) {
        watcher_healthy.store(true, std::sync::atomic::Ordering::Relaxed);
        tokio::spawn(async move { watcher.run(shutdown_rx).await });
    } else {
        tracing::warn!("Filesystem watcher failed to initialize — file change detection disabled");
    }

    // --- mobile gateway (optional, enabled via RICHTER_MOBILE_ENABLED) ---
    let mobile_enabled = std::env::var("RICHTER_MOBILE_ENABLED")
        .map(|v| v == "true" || v == "1")
        .unwrap_or(false);

    let mut mobile_state: Option<Arc<MobileGatewayState>> = None;
    // Shutdown guard for the mobile gateway — must live for the full daemon
    // lifetime to prevent premature shutdown of the gateway's watch channel.
    let _mobile_shutdown_guard: Option<tokio::sync::watch::Sender<bool>>;

    if mobile_enabled {
        let state = MobileGatewayState::new();
        state.config.write().enabled = true;

        // Phase 4.1: LAN access only via explicit opt-in
        let lan_gateway = std::env::var("RICHTER_MOBILE_LAN")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false);
        state.config.write().lan_gateway = lan_gateway;

        // Phase 4.1: TLS enabled by default, can be disabled for development
        let tls_enabled = std::env::var("RICHTER_MOBILE_TLS")
            .map(|v| v != "false" && v != "0")
            .unwrap_or(true);
        state.config.write().tls_enabled = tls_enabled;

        let state = state
            .with_event_bus(event_bus.clone())
            .with_run_manager(run_manager.clone())
            .with_resource_monitor(Arc::new(scheduler.resource_monitor().clone()))
            .with_db(db.clone());

        // Phase 4.6: Load persisted devices from SQLite
        if let Err(e) = state.load_devices_from_db().await {
            tracing::warn!("Failed to load mobile devices from SQLite: {e}");
        }

        let mobile_port: u16 = std::env::var("RICHTER_MOBILE_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(9777);

        // Phase 4.1: Bind to localhost only by default; LAN access requires explicit opt-in
        let bind_address = if lan_gateway {
            [0, 0, 0, 0]
        } else {
            [127, 0, 0, 1]
        };
        let bind_addr = SocketAddr::from((bind_address, mobile_port));

        let arc_state = Arc::new(state);
        let start_state = arc_state.clone();
        let mobile_data_dir = data_dir.clone();

        match start_state.start(bind_addr, &mobile_data_dir) {
            Ok(started_tx) => {
                _mobile_shutdown_guard = Some(started_tx);
                let protocol = if tls_enabled { "https" } else { "http" };
                info!(
                    "Mobile Gateway enabled on {protocol}://{}:{mobile_port} (LAN={lan_gateway}, TLS={tls_enabled})",
                    if lan_gateway { "0.0.0.0" } else { "127.0.0.1" }
                );
                mobile_state = Some(arc_state);
            }
            Err(e) => {
                _mobile_shutdown_guard = None;
                tracing::error!("Mobile Gateway failed to start: {e}");
            }
        }
    } else {
        _mobile_shutdown_guard = None;
    }

    // --- build AppState ---
    let shutdown_supervisor = supervisor.clone();
    let shutdown_run_manager = run_manager.clone();
    let auth_token = Arc::new(std::sync::OnceLock::new());
    let state = Arc::new(AppState {
        runs: RunState {
            run_manager,
            scheduler,
            supervisor: supervisor.clone(),
        },
        system: SystemState {
            db: db.clone(),
            event_bus: event_bus.clone(),
            settings: Arc::new(ParkingMutex::new(HashMap::new())),
            install_status: Arc::new(ParkingMutex::new(InstallStatus::default())),
            watcher_healthy,
        },
        auth: AuthState {
            token_path: token_path.clone(),
            auth_token: auth_token.clone(),
            auth_scope: Arc::new(ParkingMutex::new("write".to_string())),
        },
        mobile: mobile_state,
        model_call_budget: Arc::new(parking_lot::Mutex::new(ModelCallBudget::default())),
        repos: Arc::new(ParkingMutex::new(Vec::new())),
        rate_limiter: Arc::new(RateLimiter::default()),
        metrics: metrics::metrics(),
    });

    // Initialize cached auth token
    if let Ok(token_str) = std::fs::read_to_string(&token_path) {
        let _ = auth_token.get_or_init(|| token_str.trim().to_string());
    }

    // --- start API server ---
    info!(socket = %socket_path.display(), "Richter daemon starting");
    let api_socket = socket_path.clone();
    let api_handle = tokio::spawn(async move {
        if let Err(e) = serve(state, &api_socket).await {
            tracing::error!(error = %e, "API server exited");
        }
    });

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    info!("Richter daemon ready — socket {socket_path_str}");

    // --- log LLM/importance configuration ---
    {
        if let Ok(endpoint) = std::env::var("RICHTER_LLM_ENDPOINT") {
            if !endpoint.is_empty() {
                let model = std::env::var("RICHTER_LLM_MODEL")
                    .ok()
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "gpt-4o-mini".to_string());
                let max_tokens = std::env::var("RICHTER_LLM_MAX_TOKENS")
                    .ok()
                    .and_then(|v| v.parse::<u32>().ok())
                    .unwrap_or(256);
                let timeout = std::env::var("RICHTER_LLM_TIMEOUT")
                    .ok()
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(15);
                info!(
                    endpoint = %endpoint,
                    model = %model,
                    max_tokens = max_tokens,
                    timeout_secs = timeout,
                    "LLM importance boost configured (HTTP)"
                );
            } else {
                info!("LLM importance boost disabled (RICHTER_LLM_ENDPOINT is empty)");
            }
        } else if std::env::var("RICHTER_MODEL_BOOST_COMMAND").is_ok() {
            info!("LLM importance boost configured (shell command)");
        } else {
            info!("LLM importance boost disabled (no model configured)");
        }
    }

    // --- periodic cache eviction ---
    {
        let db = db.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(600)).await;
                match db.evict_expired_cache().await {
                    Ok(n) if n > 0 => tracing::info!("Evicted {n} expired cache entries"),
                    Err(e) => tracing::warn!("Cache eviction failed: {e}"),
                    _ => {}
                }
            }
        });
    }

    // --- health watchdog: periodic self-check ---
    {
        let db_watchdog = db.clone();
        let watchdog_interval = std::env::var("RICHTER_WATCHDOG_INTERVAL_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(60);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(watchdog_interval)).await;
                // Database liveness check
                match db_watchdog.list_active_runs().await {
                    Ok(_) => {}
                    Err(e) => {
                        tracing::error!("Health watchdog: database unreachable — {e:#}");
                    }
                }
                // Resource pressure warning
                let _ = sysinfo::System::new_all(); // Force refresh
            }
        });
    }

    // --- graceful shutdown (Ctrl-C or SIGTERM) ---
    let shutdown_signal = async {
        let ctrl_c = tokio::signal::ctrl_c();
        tokio::pin!(ctrl_c);
        let sigterm = async {
            #[cfg(unix)]
            {
                let mut sigterm = unix_signal(SignalKind::terminate())
                    .expect("failed to register SIGTERM handler");
                sigterm.recv().await;
            }
            #[cfg(not(unix))]
            std::future::pending::<()>().await;
        };
        tokio::select! {
            _ = &mut ctrl_c => info!("Received Ctrl-C"),
            _ = sigterm => info!("Received SIGTERM"),
        }
    };

    shutdown_signal.await;

    info!("Shutting down Richter daemon — entering drain mode…");

    // Drain: wait up to 30s for active runs to finish gracefully
    let drain_deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let active = shutdown_run_manager.active_runs();
        if active.is_empty() || tokio::time::Instant::now() >= drain_deadline {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    api_handle.abort();

    // Orphan reconciliation: kill remaining runs, mark as orphaned in DB
    let active_runs = shutdown_run_manager.active_runs();
    if !active_runs.is_empty() {
        info!("Reconciling {} orphaned run(s)…", active_runs.len());
        let now = chrono::Utc::now().to_rfc3339();
        for run_id in &active_runs {
            if let Err(e) = db
                .update_run_status(
                    run_id,
                    richter_core::models::RunStatus::Failed,
                    Some(-1),
                    None,
                    Some(&now),
                    None,
                )
                .await
            {
                tracing::warn!("Failed to mark run {} as orphaned in DB: {}", run_id, e);
            }
            if let Err(e) = shutdown_supervisor.kill_run(run_id).await {
                tracing::warn!("Failed to kill orphaned run {}: {}", run_id, e);
            }
        }
    }

    // WAL checkpoint before exit
    if let Err(e) = db.checkpoint_wal().await {
        tracing::warn!("Failed to checkpoint WAL on shutdown: {}", e);
    }

    let _ = std::fs::remove_file(&socket_path);
    let _ = std::fs::remove_file(&pidfile_path);
    info!("Richter daemon stopped — socket removed");
    Ok(())
}
