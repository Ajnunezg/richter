//! Richter daemon binary entry point.
//!
//! Wires together the API server, event bus, scheduler, run manager,
//! and process supervisor. Binds a Unix domain socket and blocks until
//! Ctrl-C. Optionally starts the LAN-facing mobile gateway when
//! RICHTER_MOBILE_ENABLED=true.

use anyhow::Context;
use parking_lot::Mutex as ParkingMutex;
use richter_daemon::api::{serve, DaemonState, InstallStatus, ModelCallBudget};
use richter_daemon::event_bus::EventBus;
use richter_daemon::mobile_gateway::MobileGatewayState;
use richter_daemon::run_manager::RunManager;
use richter_daemon::scheduler::{ResourceMonitor, Scheduler, SchedulerConfig};
use richter_daemon::supervisor::ProcessSupervisor;
use richter_daemon::watcher::{FsWatcher, WatchTarget, WatcherConfig};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::watch;
use tokio::signal::unix::{SignalKind, signal as unix_signal};
use tracing::info;
use tracing_subscriber::{fmt, EnvFilter};

const DEFAULT_SOCKET: &str = "/tmp/richter.sock";

fn home() -> String {
    std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // --- tracing ---
    fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("richter_daemon=info,richter=info")),
        )
        .with_target(false)
        .init();

    // --- directories ---
    let data_dir = PathBuf::from(home()).join(".richter");
    std::fs::create_dir_all(&data_dir).context("Failed to create .richter data directory")?;

    let socket_path_str =
        std::env::var("RICHTER_SOCKET").unwrap_or_else(|_| DEFAULT_SOCKET.to_string());
    let socket_path = PathBuf::from(&socket_path_str);
    let _ = std::fs::remove_file(&socket_path);

    // --- database (open/create) ---
    let db_path = data_dir.join("richter.db");
    let db = Arc::new(
        richter_core::db::Database::open(&db_path).context("Failed to open SQLite database")?,
    );

    // --- auth token ---
    let token_path = data_dir.join("auth_token");
    if !token_path.exists() {
        let token = uuid::Uuid::new_v4().to_string();
        std::fs::write(&token_path, &token).context("Failed to write auth token")?;
    }

    // --- core services ---
    let event_bus = EventBus::new();

    let scheduler = Arc::new(Scheduler::new(
        SchedulerConfig::default(),
        event_bus.clone(),
        ResourceMonitor::new(),
    ));

    let supervisor = Arc::new(ProcessSupervisor::new(event_bus.clone()));

    let run_manager = Arc::new(RunManager::new(
        scheduler.clone(),
        supervisor.clone(),
        Some(db.clone()),
        event_bus.clone(),
    ));

    // --- filesystem watcher (best-effort) ---
    let (_shutdown_tx, shutdown_rx) = watch::channel(false);
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
        tokio::spawn(async move { watcher.run(shutdown_rx).await });
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
        state.config.write().lan_gateway = true;
        let state = state
            .with_event_bus(event_bus.clone())
            .with_run_manager(run_manager.clone());

        let mobile_port: u16 = std::env::var("RICHTER_MOBILE_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(9777);
        let bind_addr = SocketAddr::from(([0, 0, 0, 0], mobile_port));

        let arc_state = Arc::new(state);
        let start_state = arc_state.clone();

        match start_state.start(bind_addr) {
            Ok(started_tx) => {
                _mobile_shutdown_guard = Some(started_tx);
                info!("Mobile Gateway enabled on port {mobile_port}");
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

    // --- build DaemonState ---
    let shutdown_supervisor = supervisor.clone();
    let shutdown_run_manager = run_manager.clone();
    let state = Arc::new(DaemonState {
        event_bus: event_bus.clone(),
        run_manager,
        scheduler,
        supervisor: supervisor.clone(),
        token_path,
        repos: ParkingMutex::new(Vec::new()),
        settings: ParkingMutex::new(HashMap::new()),
        install_status: ParkingMutex::new(InstallStatus::default()),
        mobile_state,
        model_call_budget: Arc::new(parking_lot::Mutex::new(ModelCallBudget::default())),
    });

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

    // --- periodic cache eviction ---
    {
        let db = db.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(600)).await;
                match db.evict_expired_cache() {
                    Ok(n) if n > 0 => tracing::info!("Evicted {n} expired cache entries"),
                    Err(e) => tracing::warn!("Cache eviction failed: {e}"),
                    _ => {}
                }
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

    // Drain: cancel pending API work, reconcile orphans
    api_handle.abort();

    // Orphan reconciliation: log any runs still active
    let active_runs = shutdown_run_manager.active_runs();
    if !active_runs.is_empty() {
        info!("Reconciling {} orphaned run(s)…", active_runs.len());
        for run_id in &active_runs {
            if let Err(e) = shutdown_supervisor.kill_run(run_id).await {
                tracing::warn!("Failed to kill orphaned run {}: {}", run_id, e);
            }
        }
    }

    let _ = std::fs::remove_file(&socket_path);
    info!("Richter daemon stopped — socket removed");
    Ok(())
}
