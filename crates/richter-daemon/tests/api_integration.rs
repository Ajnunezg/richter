//! Daemon HTTP API auth integration tests.

use axum::{
    body::Body,
    http::{Method, Request, StatusCode},
};
use parking_lot::Mutex as ParkingMutex;
use richter_daemon::api::{
    build_router, AppState, AuthState, InstallStatus, ModelCallBudget, RepoEntry, RunState,
    SystemState,
};
use richter_daemon::event_bus::EventBus;
use richter_daemon::rate_limiter::RateLimiter;
use richter_daemon::run_manager::RunManager;
use richter_daemon::scheduler::{ResourceMonitor, Scheduler, SchedulerConfig};
use richter_daemon::supervisor::ProcessSupervisor;
use std::collections::HashMap;
use std::sync::Arc;
use tower::ServiceExt;

async fn new_daemon_state() -> Arc<AppState> {
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
        None,
        event_bus.clone(),
    ));
    let auth_token = Arc::new(std::sync::OnceLock::new());
    let _ = auth_token.get_or_init(|| "test-token".to_string());

    let tmp = tempfile::TempDir::new_in("/tmp").unwrap();
    let db_path = tmp.path().join("test.db");
    std::mem::forget(tmp);
    let db = Arc::new(
        richter_core::db::Database::open(&db_path)
            .await
            .expect("failed to open test db"),
    );

    Arc::new(AppState {
        runs: RunState {
            run_manager,
            scheduler,
            supervisor,
        },
        system: SystemState {
            db,
            event_bus,
            settings: Arc::new(ParkingMutex::new(HashMap::new())),
            install_status: Arc::new(ParkingMutex::new(InstallStatus::default())),
            watcher_healthy: Arc::new(std::sync::atomic::AtomicBool::new(true)),
        },
        auth: AuthState {
            token_path: std::path::PathBuf::from("/dev/null"),
            auth_token,
            auth_scope: Arc::new(ParkingMutex::new("write".to_string())),
        },
        mobile: None,
        model_call_budget: Arc::new(parking_lot::Mutex::new(ModelCallBudget::default())),
        repos: Arc::new(ParkingMutex::new(Vec::<RepoEntry>::new())),
        rate_limiter: Arc::new(RateLimiter::default()),
        metrics: richter_daemon::metrics::metrics(),
    })
}

async fn unauth_get(uri: &str) -> StatusCode {
    build_router(new_daemon_state().await)
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn health_requires_auth() {
    assert_eq!(unauth_get("/health").await, StatusCode::UNAUTHORIZED);
}
#[tokio::test]
async fn status_requires_auth() {
    assert_eq!(unauth_get("/status").await, StatusCode::UNAUTHORIZED);
}
#[tokio::test]
async fn repos_requires_auth() {
    assert_eq!(unauth_get("/repos").await, StatusCode::UNAUTHORIZED);
}
#[tokio::test]
async fn agents_requires_auth() {
    assert_eq!(unauth_get("/agents").await, StatusCode::UNAUTHORIZED);
}
#[tokio::test]
async fn runs_requires_auth() {
    assert_eq!(unauth_get("/runs").await, StatusCode::UNAUTHORIZED);
}
#[tokio::test]
async fn nonexistent_route_requires_auth() {
    assert_eq!(unauth_get("/no-such").await, StatusCode::UNAUTHORIZED);
}
#[tokio::test]
async fn invalid_bearer_returns_401() {
    let res = build_router(new_daemon_state().await)
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/status")
                .header("Authorization", "Bearer bad-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}
