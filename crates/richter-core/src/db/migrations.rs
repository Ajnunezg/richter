//! Database schema migrations.
//!
//! Forward-only migration system with transactional guarantees. Each migration
//! runs inside an explicit SQLite transaction. The schema version is recorded
//! atomically within the same transaction, so a crash between the migration SQL
//! and the version bump rolls back cleanly.

use anyhow::Context;
use sqlx::SqlitePool;

// ---------------------------------------------------------------------------
// Schema version
// ---------------------------------------------------------------------------

/// Current schema version. Increment and add a migration in [`migration`]
/// whenever the schema changes.
pub const CURRENT_SCHEMA_VERSION: u32 = 2;

// ---------------------------------------------------------------------------
// Migration dispatch
// ---------------------------------------------------------------------------

/// Run all pending migrations. Called by `Database::open()`.
pub(super) async fn run_migrations(pool: &SqlitePool) -> anyhow::Result<()> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS _schema_version (
            version INTEGER NOT NULL
        )",
    )
    .execute(pool)
    .await
    .context("failed to create _schema_version table")?;

    let current: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(version), 0) FROM _schema_version")
        .fetch_one(pool)
        .await
        .unwrap_or(0);

    for v in ((current as u32) + 1)..=CURRENT_SCHEMA_VERSION {
        if let Err(e) = run_migration_in_transaction(pool, v).await {
            tracing::error!("Migration v{v} failed: {e:#}");
            return Err(e).with_context(|| format!("failed to run migration v{v}"));
        }
        tracing::info!("Applied database migration v{v}");
    }

    Ok(())
}

/// Run a single migration inside a transaction.
async fn run_migration_in_transaction(pool: &SqlitePool, version: u32) -> anyhow::Result<()> {
    let mut tx = pool
        .begin()
        .await
        .with_context(|| format!("failed to begin transaction for migration v{version}"))?;

    if let Some(migrate) = migration(version) {
        #[allow(clippy::explicit_auto_deref)]
        migrate(&mut *tx).await?;
    }

    sqlx::query("DELETE FROM _schema_version")
        .execute(&mut *tx)
        .await
        .with_context(|| format!("failed to clear schema version for v{version}"))?;
    sqlx::query("INSERT INTO _schema_version (version) VALUES (?1)")
        .bind(version as i64)
        .execute(&mut *tx)
        .await
        .with_context(|| format!("failed to record schema version v{version}"))?;

    tx.commit()
        .await
        .with_context(|| format!("failed to commit migration v{version}"))?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Migration function type and dispatch table
// ---------------------------------------------------------------------------

type MigrationFn = for<'a> fn(
    &'a mut sqlx::SqliteConnection,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send + 'a>,
>;

/// Returns the migration function for the given schema version.
fn migration(version: u32) -> Option<MigrationFn> {
    match version {
        1 => Some(migration_v1),
        2 => Some(migration_v2),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Migration v1: initial schema
// ---------------------------------------------------------------------------

fn migration_v1(
    conn: &mut sqlx::SqliteConnection,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send + '_>> {
    Box::pin(async move {
        let stmts: &[&str] = &[
            "CREATE TABLE repositories (id TEXT PRIMARY KEY, name TEXT NOT NULL, root TEXT NOT NULL UNIQUE, branch TEXT, head_sha TEXT, is_dirty INTEGER NOT NULL DEFAULT 0, upstream TEXT, created_at TEXT NOT NULL, updated_at TEXT NOT NULL)",
            "CREATE TABLE worktrees (id TEXT PRIMARY KEY, repo_id TEXT NOT NULL REFERENCES repositories(id) ON DELETE CASCADE, path TEXT NOT NULL UNIQUE, branch TEXT, head_sha TEXT, is_locked INTEGER NOT NULL DEFAULT 0, created_at TEXT NOT NULL)",
            "CREATE TABLE agents (id TEXT PRIMARY KEY, agent_type TEXT NOT NULL, name TEXT, cwd TEXT, repo_id TEXT REFERENCES repositories(id), worktree_id TEXT REFERENCES worktrees(id), active_command TEXT, last_seen_at TEXT NOT NULL, metadata TEXT)",
            "CREATE TABLE runs (id TEXT PRIMARY KEY, repo_id TEXT NOT NULL REFERENCES repositories(id), command TEXT NOT NULL, classification TEXT NOT NULL, fingerprint TEXT NOT NULL, status TEXT NOT NULL DEFAULT 'queued', exit_code INTEGER, started_at TEXT, completed_at TEXT, duration_ms INTEGER, is_cached INTEGER NOT NULL DEFAULT 0, resource_class TEXT NOT NULL DEFAULT 'unknown', output_path TEXT, error_path TEXT)",
            "CREATE TABLE command_invocations (id TEXT PRIMARY KEY, run_id TEXT NOT NULL REFERENCES runs(id), agent_id TEXT REFERENCES agents(id), repo_id TEXT NOT NULL REFERENCES repositories(id), command TEXT NOT NULL, classification TEXT NOT NULL, fingerprint TEXT NOT NULL, resource_class TEXT NOT NULL, use_shell INTEGER NOT NULL DEFAULT 1, invoked_at TEXT NOT NULL)",
            "CREATE TABLE run_subscribers (run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE, agent_id TEXT REFERENCES agents(id), subscribed_at TEXT NOT NULL, detached_at TEXT, PRIMARY KEY (run_id, agent_id))",
            "CREATE TABLE run_artifacts (id TEXT PRIMARY KEY, run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE, artifact_type TEXT NOT NULL, path TEXT NOT NULL, size_bytes INTEGER, sha256 TEXT, created_at TEXT NOT NULL)",
            "CREATE TABLE run_cache (id TEXT PRIMARY KEY, fingerprint TEXT NOT NULL UNIQUE, run_id TEXT NOT NULL REFERENCES runs(id), exit_code INTEGER NOT NULL, output_path TEXT, cached_at TEXT NOT NULL, expires_at TEXT)",
            "CREATE TABLE events (id TEXT PRIMARY KEY, event_type TEXT NOT NULL, run_id TEXT, repo_id TEXT, agent_id TEXT, severity TEXT, title TEXT, summary TEXT, details TEXT, importance INTEGER DEFAULT 0, should_notify INTEGER DEFAULT 0, created_at TEXT NOT NULL)",
            "CREATE TABLE important_events (id TEXT PRIMARY KEY, event_id TEXT NOT NULL REFERENCES events(id), importance INTEGER NOT NULL, category TEXT, recommended_action TEXT, acknowledged INTEGER NOT NULL DEFAULT 0, created_at TEXT NOT NULL)",
            "CREATE TABLE decisions (id TEXT PRIMARY KEY, run_id TEXT NOT NULL REFERENCES runs(id), decision_type TEXT NOT NULL, reason TEXT, details TEXT, model_used TEXT, decided_at TEXT NOT NULL)",
            "CREATE TABLE leases (id TEXT PRIMARY KEY, agent_id TEXT NOT NULL REFERENCES agents(id), path TEXT NOT NULL, repo_id TEXT NOT NULL REFERENCES repositories(id), ttl_seconds INTEGER NOT NULL, acquired_at TEXT NOT NULL, expires_at TEXT NOT NULL, released_at TEXT)",
            "CREATE TABLE model_calls (id TEXT PRIMARY KEY, provider TEXT NOT NULL, model TEXT NOT NULL, purpose TEXT NOT NULL, input_tokens INTEGER, output_tokens INTEGER, cost_cents REAL, duration_ms INTEGER, created_at TEXT NOT NULL)",
            "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL, updated_at TEXT NOT NULL)",
            "CREATE TABLE plugin_manifests (id TEXT PRIMARY KEY, name TEXT NOT NULL, version TEXT NOT NULL, enabled INTEGER NOT NULL DEFAULT 1, config TEXT, installed_at TEXT NOT NULL)",
            "CREATE INDEX idx_runs_repo_id ON runs(repo_id)",
            "CREATE INDEX idx_runs_fingerprint ON runs(fingerprint)",
            "CREATE INDEX idx_runs_status ON runs(status)",
            "CREATE INDEX idx_runs_started_at ON runs(started_at)",
            "CREATE INDEX idx_events_run_id ON events(run_id)",
            "CREATE INDEX idx_events_repo_id ON events(repo_id)",
            "CREATE INDEX idx_events_agent_id ON events(agent_id)",
            "CREATE INDEX idx_events_created_at ON events(created_at)",
            "CREATE INDEX idx_run_cache_fingerprint ON run_cache(fingerprint)",
            "CREATE INDEX idx_leases_agent_id ON leases(agent_id)",
            "CREATE INDEX idx_leases_path ON leases(path)",
            "CREATE INDEX idx_agents_repo_id ON agents(repo_id)",
        ];
        for (i, stmt) in stmts.iter().enumerate() {
            sqlx::query(stmt)
                .execute(&mut *conn)
                .await
                .with_context(|| {
                    format!("failed to execute migration v1 statement #{i}: {stmt}")
                })?;
        }
        Ok(())
    })
}

// ---------------------------------------------------------------------------
// Migration v2: mobile companion tables
// ---------------------------------------------------------------------------

fn migration_v2(
    conn: &mut sqlx::SqliteConnection,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send + '_>> {
    Box::pin(async move {
        let stmts: &[&str] = &[
            "CREATE TABLE IF NOT EXISTS mobile_devices (id TEXT PRIMARY KEY, display_name TEXT NOT NULL, platform TEXT NOT NULL, device_public_key TEXT NOT NULL, scopes_json TEXT NOT NULL DEFAULT '[]', created_at TEXT NOT NULL, last_seen_at TEXT NOT NULL, revoked_at TEXT, revocation_reason TEXT, push_enabled INTEGER NOT NULL DEFAULT 0, relay_enabled INTEGER NOT NULL DEFAULT 0, app_version TEXT, os_version TEXT)",
            "CREATE TABLE IF NOT EXISTS mobile_pairing_sessions (id TEXT PRIMARY KEY, pairing_secret_hash TEXT NOT NULL, server_pubkey_fingerprint TEXT NOT NULL, requested_scopes_json TEXT NOT NULL DEFAULT '[]', expires_at TEXT NOT NULL, claimed_at TEXT, claimed_device_id TEXT, created_at TEXT NOT NULL)",
            "CREATE TABLE IF NOT EXISTS mobile_device_sessions (id TEXT PRIMARY KEY, device_id TEXT NOT NULL REFERENCES mobile_devices(id) ON DELETE CASCADE, token_hash TEXT NOT NULL, created_at TEXT NOT NULL, expires_at TEXT NOT NULL, revoked INTEGER NOT NULL DEFAULT 0)",
            "CREATE TABLE IF NOT EXISTS mobile_push_subscriptions (id TEXT PRIMARY KEY, device_id TEXT NOT NULL REFERENCES mobile_devices(id) ON DELETE CASCADE, push_token TEXT NOT NULL, provider TEXT NOT NULL, created_at TEXT NOT NULL, revoked_at TEXT)",
            "CREATE TABLE IF NOT EXISTS mobile_notification_deliveries (id TEXT PRIMARY KEY, event_id TEXT NOT NULL, device_id TEXT NOT NULL REFERENCES mobile_devices(id) ON DELETE CASCADE, importance INTEGER NOT NULL, category TEXT NOT NULL, push_payload TEXT, delivered_at TEXT NOT NULL, opened_at TEXT)",
            "CREATE TABLE IF NOT EXISTS mobile_gateway_audit_log (id TEXT PRIMARY KEY, device_id TEXT, action TEXT NOT NULL, target_type TEXT, target_id TEXT, risk_level TEXT, allowed INTEGER NOT NULL DEFAULT 0, reason TEXT, ip_address_hash TEXT, created_at TEXT NOT NULL)",
            "CREATE TABLE IF NOT EXISTS mobile_relay_sessions (id TEXT PRIMARY KEY, device_id TEXT NOT NULL REFERENCES mobile_devices(id) ON DELETE CASCADE, relay_id TEXT NOT NULL, channel_id TEXT NOT NULL, established_at TEXT NOT NULL, closed_at TEXT)",
            "CREATE TABLE IF NOT EXISTS mobile_capability_grants (id TEXT PRIMARY KEY, device_id TEXT NOT NULL REFERENCES mobile_devices(id) ON DELETE CASCADE, capability TEXT NOT NULL, granted_at TEXT NOT NULL, expires_at TEXT, revoked INTEGER NOT NULL DEFAULT 0)",
            "CREATE TABLE IF NOT EXISTS mobile_security_events (id TEXT PRIMARY KEY, device_id TEXT, event_type TEXT NOT NULL, severity TEXT NOT NULL, description TEXT NOT NULL, ip_address_hash TEXT, created_at TEXT NOT NULL)",
            "CREATE TABLE IF NOT EXISTS mobile_device_tokens (id TEXT PRIMARY KEY, device_id TEXT NOT NULL REFERENCES mobile_devices(id) ON DELETE CASCADE, token_type TEXT NOT NULL, token_hash TEXT NOT NULL, created_at TEXT NOT NULL, expires_at TEXT, revoked INTEGER NOT NULL DEFAULT 0)",
            "CREATE INDEX IF NOT EXISTS idx_mobile_devices_public_key ON mobile_devices(device_public_key)",
            "CREATE INDEX IF NOT EXISTS idx_mobile_pairing_sessions_expires ON mobile_pairing_sessions(expires_at)",
            "CREATE INDEX IF NOT EXISTS idx_mobile_device_sessions_device ON mobile_device_sessions(device_id)",
            "CREATE INDEX IF NOT EXISTS idx_mobile_push_subscriptions_device ON mobile_push_subscriptions(device_id)",
            "CREATE INDEX IF NOT EXISTS idx_mobile_gateway_audit_device ON mobile_gateway_audit_log(device_id)",
            "CREATE INDEX IF NOT EXISTS idx_mobile_security_events_device ON mobile_security_events(device_id)",
            "CREATE INDEX IF NOT EXISTS idx_mobile_device_tokens_device ON mobile_device_tokens(device_id)",
        ];
        for stmt in stmts {
            sqlx::query(stmt)
                .execute(&mut *conn)
                .await
                .context("migration v2: failed to execute statement")?;
        }
        Ok(())
    })
}
