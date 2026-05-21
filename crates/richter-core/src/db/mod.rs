//! Richter database module: SQLite persistence for runs, claims, agents, events, and configuration.
//!
//! Provides a production-ready [`Database`] backed by [`sqlx::SqlitePool`] with WAL mode,
//! foreign keys enabled, a schema-version migration system, and typed async CRUD
//! methods for all 15+ tables.
//!
//! The pool is internally `Arc`-wrapped so [`Database`] can be cloned cheaply and
//! shared across tasks without an outer `Arc<Mutex>`.

pub mod migrations;
pub mod rows;

// Re-export row types for backward compatibility.
pub use rows::*;

use crate::models::{CommandClass, EventKind, ResourceClass, RunStatus};
use anyhow::Context;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::SqlitePool;
use std::path::Path;
use tracing;

use migrations::CURRENT_SCHEMA_VERSION;

// ---------------------------------------------------------------------------
// Public database handle
// ---------------------------------------------------------------------------

/// A SQLite connection pool with WAL mode and foreign keys enabled.
///
/// Construct via [`Database::open`], which runs all pending migrations
/// automatically before returning.
///
/// `Database` wraps a [`sqlx::SqlitePool`] which is internally `Arc`-wrapped,
/// so cloning is cheap and the handle can be freely shared across tasks.
pub struct Database {
    pool: SqlitePool,
    /// Directory where the encryption key file lives (same as db file parent).
    data_dir: std::path::PathBuf,
    /// Whether the database key has been loaded and encryption primitives are
    /// available. Set to `true` after a successful call to `generate_db_key()`.
    /// Full file-level encryption requires an encrypted VFS or SQLCipher.
    /// This implementation provides key management and the crypto primitives.
    /// For production, consider using SQLCipher or an encrypted filesystem.
    encrypted: bool,
}

impl Database {
    /// Opens (or creates) the SQLite database at `path`, enables WAL mode
    /// and foreign keys, then runs all pending schema migrations.
    pub async fn open(path: &Path) -> anyhow::Result<Self> {
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .foreign_keys(true)
            .busy_timeout(std::time::Duration::from_secs(5));

        let pool = SqlitePoolOptions::new()
            .max_connections(8)
            .connect_with(options)
            .await
            .context("failed to open SQLite database")?;

        // Integrity check
        let integrity: (String,) = sqlx::query_as("PRAGMA integrity_check")
            .fetch_one(&pool)
            .await
            .context("failed to run integrity check")?;
        if integrity.0 != "ok" {
            return Err(anyhow::anyhow!(
                "Database integrity check failed: {}",
                integrity.0
            ));
        }

        // Create a pre-migration backup if there are pending migrations.
        // The _schema_version table is created by run_migrations; check whether
        // it exists already (it will on an existing DB, but not on a fresh one).
        {
            let table_exists: (i64,) = sqlx::query_as(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='_schema_version'",
            )
            .fetch_one(&pool)
            .await
            .unwrap_or((0,));

            if table_exists.0 > 0 {
                let current: i64 =
                    sqlx::query_scalar("SELECT COALESCE(MAX(version), 0) FROM _schema_version")
                        .fetch_one(&pool)
                        .await
                        .unwrap_or(0);
                if (current as u32) < CURRENT_SCHEMA_VERSION && path.exists() {
                    let backup_path = path.with_file_name(format!(
                        "{}.backup-pre-v{}",
                        path.file_name().unwrap_or_default().to_string_lossy(),
                        current
                    ));
                    match std::fs::copy(path, &backup_path) {
                        Ok(bytes) => {
                            tracing::info!(
                                "Pre-migration backup created: {} ({} bytes, schema v{} → v{})",
                                backup_path.display(),
                                bytes,
                                current,
                                CURRENT_SCHEMA_VERSION,
                            );
                        }
                        Err(e) => {
                            tracing::warn!(
                                "Failed to create pre-migration backup at {}: {e:#}",
                                backup_path.display(),
                            );
                        }
                    }
                }
            }
        }

        migrations::run_migrations(&pool)
            .await
            .context("failed to run database migrations")?;

        // Initialize database encryption key management.
        // The key file lives alongside the database in the same directory.
        //
        // NOTE: The crypto module provides AES-256-GCM primitives and key
        // management for future VFS-layer encryption (SQLCipher, encrypted
        // filesystem, or custom VFS). The SQLite database file itself is NOT
        // currently encrypted at rest. The key is generated and a health check
        // is performed to ensure the crypto primitives are ready when a VFS
        // layer is integrated. Filesystem-level protection relies on 0600
        // file permissions and user-session isolation.
        let data_dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();
        let _key = crate::crypto::generate_db_key(&data_dir)
            .context("failed to initialize database encryption key")?;
        let encrypted = false;

        Ok(Self {
            pool,
            data_dir,
            encrypted,
        })
    }

    /// Returns a reference to the underlying connection pool.
    #[allow(unused)]
    pub(crate) fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Begin a new database transaction.
    ///
    /// Returns a `Transaction` that can be used to batch multiple operations
    /// atomically. Either call `.commit()` to persist or drop it to roll back.
    pub async fn transaction(&self) -> anyhow::Result<sqlx::Transaction<'static, sqlx::Sqlite>> {
        self.pool
            .begin()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to begin transaction: {e}"))
    }

    /// Perform a WAL checkpoint (TRUNCATE) — call on graceful shutdown.
    pub async fn checkpoint_wal(&self) -> anyhow::Result<()> {
        sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
            .execute(&self.pool)
            .await
            .context("failed to checkpoint WAL")?;
        Ok(())
    }

    /// Create a backup of the database file at the given path.
    pub async fn backup(&self, backup_path: &Path) -> anyhow::Result<()> {
        sqlx::query("VACUUM INTO ?1")
            .bind(backup_path.to_string_lossy().to_string())
            .execute(&self.pool)
            .await
            .context("failed to backup database")?;
        Ok(())
    }

    /// Whether the database encryption key has been loaded.
    ///
    /// Returns `true` if the database was opened with encryption key
    /// management enabled. The key is available for file-level encryption
    /// via the `crypto` module.
    pub fn is_encrypted(&self) -> bool {
        self.encrypted
    }

    /// Returns the encryption status of the database.
    ///
    /// Returns `"key-managed (vfs-pending)"` if key management is initialized
    /// but the database file is not encrypted at rest. Returns `"none"` if
    /// key management is not initialized.
    ///
    /// **Note:** Full at-rest encryption requires a VFS-layer integration
    /// (SQLCipher, encrypted filesystem, or custom VFS). The current
    /// implementation provides key management and AES-256-GCM primitives
    /// but does not encrypt the SQLite file itself. Protection relies on
    /// 0600 file permissions and user-session isolation.
    pub fn encryption_status(&self) -> String {
        if self.encrypted {
            "aes-256-gcm".to_string()
        } else {
            "key-managed (vfs-pending)".to_string()
        }
    }

    /// Verifies that the encryption key exists, is valid, and the crypto
    /// primitives are functional.
    ///
    /// Performs a round-trip encryption smoke test. Call this after opening
    /// the database to confirm encryption readiness.
    ///
    /// # Errors
    ///
    /// Returns an error if the key file is missing, corrupted, or if the
    /// AES-256-GCM primitives fail a self-test.
    pub async fn verify_encryption(&self) -> anyhow::Result<()> {
        if !self.encrypted {
            anyhow::bail!("database encryption is not enabled");
        }
        crate::crypto::verify_encryption_health(&self.data_dir)
            .context("encryption health check failed")
    }

    /// List all non-expired cache entries for startup population.
    pub async fn list_non_expired_cache(&self) -> anyhow::Result<Vec<CacheEntryRow>> {
        let rows = sqlx::query_as::<_, CacheEntryRow>(
            "SELECT id, fingerprint, run_id, exit_code, output_path, \
             cached_at, expires_at \
             FROM run_cache WHERE expires_at IS NULL \
             OR expires_at > datetime('now')",
        )
        .fetch_all(&self.pool)
        .await
        .context("failed to query non-expired cache")?;
        Ok(rows)
    }

    // -----------------------------------------------------------------------
    // runs
    // -----------------------------------------------------------------------

    /// Insert a new run row.
    pub async fn insert_run(
        &self,
        id: &str,
        repo_id: &str,
        command: &str,
        classification: CommandClass,
        fingerprint: &str,
        resource_class: ResourceClass,
    ) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO runs (id, repo_id, command, classification, fingerprint, \
                 status, resource_class, is_cached) \
                 VALUES (?1, ?2, ?3, ?4, ?5, 'queued', ?6, 0)",
        )
        .bind(id)
        .bind(repo_id)
        .bind(command)
        .bind(classification.to_string())
        .bind(fingerprint)
        .bind(resource_class.to_string())
        .execute(&self.pool)
        .await
        .context("failed to insert run")?;
        Ok(())
    }

    /// Update the status (and optionally exit code / timestamps) of a run.
    pub async fn update_run_status(
        &self,
        id: &str,
        status: RunStatus,
        exit_code: Option<i32>,
        started_at: Option<&str>,
        completed_at: Option<&str>,
        duration_ms: Option<i64>,
    ) -> anyhow::Result<()> {
        sqlx::query(
            "UPDATE runs SET status = ?2, exit_code = ?3, \
                 started_at = COALESCE(?4, started_at), \
                 completed_at = COALESCE(?5, completed_at), \
                 duration_ms = COALESCE(?6, duration_ms) \
                 WHERE id = ?1",
        )
        .bind(id)
        .bind(status.to_string())
        .bind(exit_code)
        .bind(started_at)
        .bind(completed_at)
        .bind(duration_ms)
        .execute(&self.pool)
        .await
        .context("failed to update run status")?;
        Ok(())
    }

    /// Fetch a single run by id.
    pub async fn get_run(&self, id: &str) -> anyhow::Result<Option<RunRow>> {
        let result = sqlx::query_as::<_, RunRow>(
            "SELECT id, repo_id, command, classification, fingerprint, status, \
                 exit_code, started_at, completed_at, duration_ms, is_cached, \
                 resource_class, output_path, error_path \
                 FROM runs WHERE id = ?1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .context("failed to fetch run")?;
        Ok(result)
    }

    /// List all runs for a given repo, newest first, with pagination.
    ///
    /// Default limit is 50, maximum is 500.
    pub async fn list_runs_by_repo(
        &self,
        repo_id: &str,
        limit: Option<usize>,
        offset: Option<usize>,
    ) -> anyhow::Result<Vec<RunRow>> {
        let limit = limit.unwrap_or(50).min(500);
        let offset = offset.unwrap_or(0);
        let rows = sqlx::query_as::<_, RunRow>(
            "SELECT id, repo_id, command, classification, fingerprint, status, \
             exit_code, started_at, completed_at, duration_ms, is_cached, \
             resource_class, output_path, error_path \
             FROM runs WHERE repo_id = ?1 \
             ORDER BY COALESCE(started_at, '0000-01-01T00:00:00Z') DESC \
             LIMIT ?2 OFFSET ?3",
        )
        .bind(repo_id)
        .bind(limit as i64)
        .bind(offset as i64)
        .fetch_all(&self.pool)
        .await
        .context("failed to query runs by repo")?;
        Ok(rows)
    }

    /// List all runs that are currently queued or running.
    pub async fn list_active_runs(&self) -> anyhow::Result<Vec<RunRow>> {
        let rows = sqlx::query_as::<_, RunRow>(
            "SELECT id, repo_id, command, classification, fingerprint, status, \
             exit_code, started_at, completed_at, duration_ms, is_cached, \
             resource_class, output_path, error_path \
             FROM runs WHERE status IN ('queued', 'running') \
             ORDER BY COALESCE(started_at, '0000-01-01T00:00:00Z') ASC",
        )
        .fetch_all(&self.pool)
        .await
        .context("failed to query active runs")?;
        Ok(rows)
    }

    // -----------------------------------------------------------------------
    // events
    // -----------------------------------------------------------------------

    /// Insert a new event.
    #[allow(clippy::too_many_arguments)]
    pub async fn insert_event(
        &self,
        id: &str,
        event_type: EventKind,
        run_id: Option<&str>,
        repo_id: Option<&str>,
        agent_id: Option<&str>,
        severity: Option<&str>,
        title: &str,
        summary: Option<&str>,
        details: Option<&str>,
        importance: i32,
        should_notify: bool,
        created_at: &str,
    ) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO events (id, event_type, run_id, repo_id, agent_id, severity, \
                 title, summary, details, importance, should_notify, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        )
        .bind(id)
        .bind(event_type.to_string())
        .bind(run_id)
        .bind(repo_id)
        .bind(agent_id)
        .bind(severity)
        .bind(title)
        .bind(summary)
        .bind(details)
        .bind(importance)
        .bind(should_notify as i32)
        .bind(created_at)
        .execute(&self.pool)
        .await
        .context("failed to insert event")?;
        Ok(())
    }

    /// List events, newest first, with optional entity filters and pagination.
    ///
    /// Default limit is 50, maximum is 500.
    pub async fn list_events(
        &self,
        run_id: Option<&str>,
        repo_id: Option<&str>,
        agent_id: Option<&str>,
        limit: Option<usize>,
        offset: Option<usize>,
    ) -> anyhow::Result<Vec<EventRow>> {
        let limit = limit.unwrap_or(50).min(500);
        let offset = offset.unwrap_or(0);
        let mut clauses: Vec<String> = Vec::new();
        let mut param_idx = 1;

        let mut sql = String::from(
            "SELECT id, event_type, run_id, repo_id, agent_id, severity, \
             title, summary, details, importance, should_notify, created_at \
             FROM events WHERE ",
        );

        if run_id.is_some() {
            clauses.push(format!("run_id = ?{param_idx}"));
            param_idx += 1;
        }
        if repo_id.is_some() {
            clauses.push(format!("repo_id = ?{param_idx}"));
            param_idx += 1;
        }
        if agent_id.is_some() {
            clauses.push(format!("agent_id = ?{param_idx}"));
            param_idx += 1;
        }

        let where_clause = if clauses.is_empty() {
            "1=1".to_string()
        } else {
            clauses.join(" AND ")
        };

        let limit_idx = param_idx;
        let offset_idx = param_idx + 1;
        sql.push_str(&where_clause);
        sql.push_str(&format!(
            " ORDER BY created_at DESC LIMIT ?{limit_idx} OFFSET ?{offset_idx}"
        ));

        let mut query = sqlx::query_as::<_, EventRow>(&sql);
        if let Some(rid) = run_id {
            query = query.bind(rid);
        }
        if let Some(rid) = repo_id {
            query = query.bind(rid);
        }
        if let Some(aid) = agent_id {
            query = query.bind(aid);
        }
        query = query.bind(limit as i64);
        query = query.bind(offset as i64);

        let rows = query
            .fetch_all(&self.pool)
            .await
            .context("failed to query events")?;
        Ok(rows)
    }

    /// List important events, newest first.
    pub async fn list_important_events(
        &self,
        limit: usize,
    ) -> anyhow::Result<Vec<ImportantEventRow>> {
        let rows = sqlx::query_as::<_, ImportantEventRow>(
            "SELECT id, event_id, importance, category, \
             recommended_action, acknowledged, created_at \
             FROM important_events \
             ORDER BY importance DESC, created_at DESC \
             LIMIT ?1",
        )
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await
        .context("failed to query important events")?;
        Ok(rows)
    }

    // -----------------------------------------------------------------------
    // run_cache
    // -----------------------------------------------------------------------

    /// Insert a cache entry for a completed run.
    #[allow(clippy::too_many_arguments)]
    pub async fn insert_cache_entry(
        &self,
        id: &str,
        fingerprint: &str,
        run_id: &str,
        exit_code: i32,
        output_path: Option<&str>,
        cached_at: &str,
        expires_at: Option<&str>,
    ) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO run_cache (id, fingerprint, run_id, exit_code, output_path, \
                 cached_at, expires_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )
        .bind(id)
        .bind(fingerprint)
        .bind(run_id)
        .bind(exit_code)
        .bind(output_path)
        .bind(cached_at)
        .bind(expires_at)
        .execute(&self.pool)
        .await
        .context("failed to insert cache entry")?;
        Ok(())
    }

    /// Look up a cache entry by fingerprint.
    pub async fn get_cache_entry(
        &self,
        fingerprint: &str,
    ) -> anyhow::Result<Option<CacheEntryRow>> {
        let result = sqlx::query_as::<_, CacheEntryRow>(
            "SELECT id, fingerprint, run_id, exit_code, output_path, \
                 cached_at, expires_at \
                 FROM run_cache WHERE fingerprint = ?1",
        )
        .bind(fingerprint)
        .fetch_optional(&self.pool)
        .await
        .context("failed to fetch cache entry")?;
        Ok(result)
    }

    /// Remove all cache entries whose `expires_at` is in the past.
    pub async fn evict_expired_cache(&self) -> anyhow::Result<usize> {
        let result = sqlx::query(
            "DELETE FROM run_cache WHERE expires_at IS NOT NULL \
                 AND expires_at < datetime('now')",
        )
        .execute(&self.pool)
        .await
        .context("failed to evict expired cache entries")?;
        Ok(result.rows_affected() as usize)
    }

    // -----------------------------------------------------------------------
    // repositories
    // -----------------------------------------------------------------------

    /// Insert or replace a repository row.
    #[allow(clippy::too_many_arguments)]
    pub async fn upsert_repo(
        &self,
        id: &str,
        name: &str,
        root: &str,
        branch: Option<&str>,
        head_sha: Option<&str>,
        is_dirty: bool,
        upstream: Option<&str>,
        created_at: &str,
        updated_at: &str,
    ) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO repositories (id, name, root, branch, head_sha, is_dirty, \
                 upstream, created_at, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9) \
                 ON CONFLICT(id) DO UPDATE SET \
                 name = excluded.name, root = excluded.root, \
                 branch = excluded.branch, head_sha = excluded.head_sha, \
                 is_dirty = excluded.is_dirty, upstream = excluded.upstream, \
                 updated_at = excluded.updated_at",
        )
        .bind(id)
        .bind(name)
        .bind(root)
        .bind(branch)
        .bind(head_sha)
        .bind(is_dirty as i32)
        .bind(upstream)
        .bind(created_at)
        .bind(updated_at)
        .execute(&self.pool)
        .await
        .context("failed to upsert repository")?;
        Ok(())
    }

    /// Fetch a repository by id.
    pub async fn get_repo(&self, id: &str) -> anyhow::Result<Option<RepoRow>> {
        let result = sqlx::query_as::<_, RepoRow>(
            "SELECT id, name, root, branch, head_sha, is_dirty, upstream, \
                 created_at, updated_at \
                 FROM repositories WHERE id = ?1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .context("failed to fetch repository")?;
        Ok(result)
    }

    /// List all registered repositories.
    pub async fn list_repos(&self) -> anyhow::Result<Vec<RepoRow>> {
        let rows = sqlx::query_as::<_, RepoRow>(
            "SELECT id, name, root, branch, head_sha, is_dirty, upstream, \
             created_at, updated_at \
             FROM repositories ORDER BY name ASC",
        )
        .fetch_all(&self.pool)
        .await
        .context("failed to query repositories")?;
        Ok(rows)
    }

    // -----------------------------------------------------------------------
    // agents
    // -----------------------------------------------------------------------

    /// Insert or replace an agent row.
    #[allow(clippy::too_many_arguments)]
    pub async fn upsert_agent(
        &self,
        id: &str,
        agent_type: &str,
        name: Option<&str>,
        cwd: Option<&str>,
        repo_id: Option<&str>,
        worktree_id: Option<&str>,
        active_command: Option<&str>,
        last_seen_at: &str,
        metadata: Option<&str>,
    ) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO agents (id, agent_type, name, cwd, repo_id, worktree_id, \
                 active_command, last_seen_at, metadata) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9) \
                 ON CONFLICT(id) DO UPDATE SET \
                 agent_type = excluded.agent_type, name = excluded.name, \
                 cwd = excluded.cwd, repo_id = excluded.repo_id, \
                 worktree_id = excluded.worktree_id, \
                 active_command = excluded.active_command, \
                 last_seen_at = excluded.last_seen_at, \
                 metadata = excluded.metadata",
        )
        .bind(id)
        .bind(agent_type)
        .bind(name)
        .bind(cwd)
        .bind(repo_id)
        .bind(worktree_id)
        .bind(active_command)
        .bind(last_seen_at)
        .bind(metadata)
        .execute(&self.pool)
        .await
        .context("failed to upsert agent")?;
        Ok(())
    }

    /// Fetch an agent by id.
    pub async fn get_agent(&self, id: &str) -> anyhow::Result<Option<AgentRow>> {
        let result = sqlx::query_as::<_, AgentRow>(
            "SELECT id, agent_type, name, cwd, repo_id, worktree_id, \
                 active_command, last_seen_at, metadata \
                 FROM agents WHERE id = ?1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .context("failed to fetch agent")?;
        Ok(result)
    }

    /// List all agents associated with a given repo.
    pub async fn list_agents_by_repo(&self, repo_id: &str) -> anyhow::Result<Vec<AgentRow>> {
        let rows = sqlx::query_as::<_, AgentRow>(
            "SELECT id, agent_type, name, cwd, repo_id, worktree_id, \
             active_command, last_seen_at, metadata \
             FROM agents WHERE repo_id = ?1 \
             ORDER BY last_seen_at DESC",
        )
        .bind(repo_id)
        .fetch_all(&self.pool)
        .await
        .context("failed to query agents by repo")?;
        Ok(rows)
    }

    // -----------------------------------------------------------------------
    // leases
    // -----------------------------------------------------------------------

    /// Acquire a lease (insert a new active lease row).
    #[allow(clippy::too_many_arguments)]
    pub async fn acquire_lease(
        &self,
        id: &str,
        agent_id: &str,
        path: &str,
        repo_id: &str,
        ttl_seconds: i64,
        acquired_at: &str,
        expires_at: &str,
    ) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO leases (id, agent_id, path, repo_id, ttl_seconds, \
                 acquired_at, expires_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )
        .bind(id)
        .bind(agent_id)
        .bind(path)
        .bind(repo_id)
        .bind(ttl_seconds)
        .bind(acquired_at)
        .bind(expires_at)
        .execute(&self.pool)
        .await
        .context("failed to acquire lease")?;
        Ok(())
    }

    /// Release a lease by setting `released_at`.
    pub async fn release_lease(&self, id: &str, released_at: &str) -> anyhow::Result<()> {
        sqlx::query("UPDATE leases SET released_at = ?2 WHERE id = ?1")
            .bind(id)
            .bind(released_at)
            .execute(&self.pool)
            .await
            .context("failed to release lease")?;
        Ok(())
    }

    /// List all leases that have not yet been released and are not expired.
    pub async fn list_active_leases(&self) -> anyhow::Result<Vec<LeaseRow>> {
        let rows = sqlx::query_as::<_, LeaseRow>(
            "SELECT id, agent_id, path, repo_id, ttl_seconds, acquired_at, \
             expires_at, released_at \
             FROM leases WHERE released_at IS NULL AND expires_at > datetime('now') \
             ORDER BY acquired_at ASC",
        )
        .fetch_all(&self.pool)
        .await
        .context("failed to query active leases")?;
        Ok(rows)
    }

    // -----------------------------------------------------------------------
    // settings
    // -----------------------------------------------------------------------

    /// Fetch a setting value by key.
    pub async fn get_setting(&self, key: &str) -> anyhow::Result<Option<String>> {
        let result: Option<(String,)> = sqlx::query_as("SELECT value FROM settings WHERE key = ?1")
            .bind(key)
            .fetch_optional(&self.pool)
            .await
            .context("failed to fetch setting")?;
        Ok(result.map(|(v,)| v))
    }

    /// Insert or replace a setting key-value pair.
    pub async fn set_setting(
        &self,
        key: &str,
        value: &str,
        updated_at: &str,
    ) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO settings (key, value, updated_at) \
                 VALUES (?1, ?2, ?3) \
                 ON CONFLICT(key) DO UPDATE SET \
                 value = excluded.value, updated_at = excluded.updated_at",
        )
        .bind(key)
        .bind(value)
        .bind(updated_at)
        .execute(&self.pool)
        .await
        .context("failed to set setting")?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // model_calls
    // -----------------------------------------------------------------------

    /// Record a call to an external LLM.
    #[allow(clippy::too_many_arguments)]
    pub async fn record_model_call(
        &self,
        id: &str,
        provider: &str,
        model: &str,
        purpose: &str,
        input_tokens: Option<i64>,
        output_tokens: Option<i64>,
        cost_cents: Option<f64>,
        duration_ms: Option<i64>,
        created_at: &str,
    ) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO model_calls (id, provider, model, purpose, \
                 input_tokens, output_tokens, cost_cents, duration_ms, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        )
        .bind(id)
        .bind(provider)
        .bind(model)
        .bind(purpose)
        .bind(input_tokens)
        .bind(output_tokens)
        .bind(cost_cents)
        .bind(duration_ms)
        .bind(created_at)
        .execute(&self.pool)
        .await
        .context("failed to record model call")?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // decisions
    // -----------------------------------------------------------------------

    /// Insert a run-or-join (or other) decision record.
    #[allow(clippy::too_many_arguments)]
    pub async fn insert_decision(
        &self,
        id: &str,
        run_id: &str,
        decision_type: &str,
        reason: Option<&str>,
        details: Option<&str>,
        model_used: Option<&str>,
        decided_at: &str,
    ) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO decisions (id, run_id, decision_type, reason, details, \
                 model_used, decided_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )
        .bind(id)
        .bind(run_id)
        .bind(decision_type)
        .bind(reason)
        .bind(details)
        .bind(model_used)
        .bind(decided_at)
        .execute(&self.pool)
        .await
        .context("failed to insert decision")?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // mobile_devices (Phase 4.6: persist device registrations in SQLite)
    // -----------------------------------------------------------------------

    /// Insert or update a mobile device registration.
    #[allow(clippy::too_many_arguments)]
    pub async fn upsert_mobile_device(
        &self,
        id: &str,
        display_name: &str,
        platform: &str,
        device_public_key: &str,
        scopes_json: &str,
        created_at: &str,
        last_seen_at: &str,
    ) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO mobile_devices (id, display_name, platform, device_public_key, \
                 scopes_json, created_at, last_seen_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) \
                 ON CONFLICT(id) DO UPDATE SET \
                 display_name = excluded.display_name, platform = excluded.platform, \
                 device_public_key = excluded.device_public_key, \
                 scopes_json = excluded.scopes_json, last_seen_at = excluded.last_seen_at",
        )
        .bind(id)
        .bind(display_name)
        .bind(platform)
        .bind(device_public_key)
        .bind(scopes_json)
        .bind(created_at)
        .bind(last_seen_at)
        .execute(&self.pool)
        .await
        .context("failed to upsert mobile device")?;
        Ok(())
    }

    /// Load all non-revoked mobile devices.
    pub async fn list_mobile_devices(&self) -> anyhow::Result<Vec<MobileDeviceRow>> {
        let rows = sqlx::query_as::<_, MobileDeviceRow>(
            "SELECT id, display_name, platform, device_public_key, scopes_json, \
             created_at, last_seen_at, revoked_at, revocation_reason, \
             push_enabled, relay_enabled, app_version, os_version \
             FROM mobile_devices WHERE revoked_at IS NULL \
             ORDER BY created_at ASC",
        )
        .fetch_all(&self.pool)
        .await
        .context("failed to list mobile devices")?;
        Ok(rows)
    }

    /// Update a mobile device's last_seen_at timestamp.
    pub async fn touch_mobile_device(&self, id: &str, last_seen_at: &str) -> anyhow::Result<()> {
        sqlx::query("UPDATE mobile_devices SET last_seen_at = ?2 WHERE id = ?1")
            .bind(id)
            .bind(last_seen_at)
            .execute(&self.pool)
            .await
            .context("failed to update mobile device last_seen_at")?;
        Ok(())
    }

    /// Insert a mobile gateway audit log entry.
    #[allow(clippy::too_many_arguments)]
    pub async fn insert_mobile_audit(
        &self,
        id: &str,
        device_id: Option<&str>,
        action: &str,
        target_type: Option<&str>,
        target_id: Option<&str>,
        risk_level: Option<&str>,
        allowed: bool,
        reason: Option<&str>,
        created_at: &str,
    ) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO mobile_gateway_audit_log \
                 (id, device_id, action, target_type, target_id, risk_level, \
                  allowed, reason, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        )
        .bind(id)
        .bind(device_id)
        .bind(action)
        .bind(target_type)
        .bind(target_id)
        .bind(risk_level)
        .bind(allowed as i32)
        .bind(reason)
        .bind(created_at)
        .execute(&self.pool)
        .await
        .context("failed to insert mobile audit log")?;
        Ok(())
    }
}

// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    struct TestDb {
        db: Database,
        _tmp: NamedTempFile,
    }

    async fn open_test_db() -> TestDb {
        let tmp = NamedTempFile::new().expect("tempfile");
        let db = Database::open(tmp.path()).await.expect("open");
        TestDb { db, _tmp: tmp }
    }

    fn now_iso() -> String {
        "2026-05-04T12:00:00Z".to_string()
    }

    // -----------------------------------------------------------------------
    // test_open
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_open() {
        let tmp = NamedTempFile::new().expect("tempfile");
        let db = Database::open(tmp.path()).await.expect("open works");

        let journal_mode: (String,) = sqlx::query_as("PRAGMA journal_mode")
            .fetch_one(&db.pool)
            .await
            .expect("pragma");
        assert_eq!(journal_mode.0, "wal");

        let version: (i64,) = sqlx::query_as("SELECT version FROM _schema_version LIMIT 1")
            .fetch_one(&db.pool)
            .await
            .expect("version query");
        assert_eq!(version.0 as u32, CURRENT_SCHEMA_VERSION);
    }

    // -----------------------------------------------------------------------
    // test_migration
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_migration() {
        let tmp = NamedTempFile::new().expect("tempfile");
        let path = tmp.path();

        // Open once — v1 migration runs.
        {
            let db = Database::open(path).await.expect("open 1");
            let cnt: (i64,) =
                sqlx::query_as("SELECT COUNT(*) FROM sqlite_master WHERE type='table'")
                    .fetch_one(&db.pool)
                    .await
                    .expect("count");
            // _schema_version + 15 data tables = 16
            assert!(cnt.0 >= 16, "expected at least 16 tables, got {}", cnt.0);
        }

        // Re-open — migration should be a no-op (version already 2).
        {
            let db = Database::open(path).await.expect("open 2");
            let version: (i64,) = sqlx::query_as("SELECT version FROM _schema_version LIMIT 1")
                .fetch_one(&db.pool)
                .await
                .expect("version");
            assert_eq!(version.0 as u32, CURRENT_SCHEMA_VERSION);
        }
    }

    // -----------------------------------------------------------------------
    // test_insert_and_get_run
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_insert_and_get_run() {
        let test_db = open_test_db().await;
        let db = &test_db.db;
        let now = now_iso();

        db.upsert_repo(
            "repo-1",
            "test-repo",
            "/tmp/test-repo",
            Some("main"),
            Some("abc123"),
            false,
            None,
            &now,
            &now,
        )
        .await
        .expect("upsert repo");

        db.insert_run(
            "run-1",
            "repo-1",
            "cargo test",
            CommandClass::Test,
            "fp-abc",
            ResourceClass::LightLint,
        )
        .await
        .expect("insert run");

        let run = db
            .get_run("run-1")
            .await
            .expect("get_run")
            .expect("present");
        assert_eq!(run.id, "run-1");
        assert_eq!(run.repo_id, "repo-1");
        assert_eq!(run.command, "cargo test");
        assert_eq!(run.classification(), CommandClass::Test);
        assert_eq!(run.fingerprint, "fp-abc");
        assert_eq!(run.status(), RunStatus::Queued);
        assert_eq!(run.exit_code, None);
        assert!(!run.is_cached());

        db.update_run_status("run-1", RunStatus::Running, None, Some(&now), None, None)
            .await
            .expect("update");
        let run = db
            .get_run("run-1")
            .await
            .expect("get_run")
            .expect("present");
        assert_eq!(run.status(), RunStatus::Running);
        assert_eq!(run.started_at.as_deref(), Some(&*now));

        let runs = db
            .list_runs_by_repo("repo-1", None, None)
            .await
            .expect("list");
        assert_eq!(runs.len(), 1);

        let active = db.list_active_runs().await.expect("active");
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].id, "run-1");

        db.update_run_status(
            "run-1",
            RunStatus::Passed,
            Some(0),
            None,
            Some(&now),
            Some(1500),
        )
        .await
        .expect("complete");
        let run = db
            .get_run("run-1")
            .await
            .expect("get_run")
            .expect("present");
        assert_eq!(run.status(), RunStatus::Passed);
        assert_eq!(run.exit_code, Some(0));
        assert_eq!(run.duration_ms, Some(1500));

        let active = db.list_active_runs().await.expect("active");
        assert!(active.is_empty());
    }

    // -----------------------------------------------------------------------
    // test_cache_eviction
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_cache_eviction() {
        let test_db = open_test_db().await;
        let db = &test_db.db;
        let now = now_iso();

        db.upsert_repo(
            "repo-1",
            "test-repo",
            "/tmp/test-repo",
            Some("main"),
            Some("abc123"),
            false,
            None,
            &now,
            &now,
        )
        .await
        .expect("upsert repo");

        db.insert_run(
            "run-1",
            "repo-1",
            "cargo build",
            CommandClass::Build,
            "fp-build",
            ResourceClass::HeavyBuild,
        )
        .await
        .expect("insert run");

        db.insert_cache_entry(
            "cache-1",
            "fp-build",
            "run-1",
            0,
            Some("/tmp/output.log"),
            &now,
            None,
        )
        .await
        .expect("insert cache");

        let entry = db
            .get_cache_entry("fp-build")
            .await
            .expect("get")
            .expect("present");
        assert_eq!(entry.exit_code, 0);

        let evicted = db.evict_expired_cache().await.expect("evict");
        assert_eq!(evicted, 0);
        assert!(db.get_cache_entry("fp-build").await.expect("get").is_some());

        db.insert_cache_entry(
            "cache-2",
            "fp-lint",
            "run-1",
            0,
            None,
            &now,
            Some("2020-01-01T00:00:00Z"),
        )
        .await
        .expect("insert cache 2");

        let evicted = db.evict_expired_cache().await.expect("evict");
        assert_eq!(evicted, 1);
        assert!(db.get_cache_entry("fp-lint").await.expect("get").is_none());
        assert!(db.get_cache_entry("fp-build").await.expect("get").is_some());
    }

    // -----------------------------------------------------------------------
    // test_lease_lifecycle
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_lease_lifecycle() {
        let test_db = open_test_db().await;
        let db = &test_db.db;
        let now = now_iso();
        let future = "2099-01-01T00:00:00Z".to_string();

        db.upsert_repo(
            "repo-1",
            "test-repo",
            "/tmp/test-repo",
            Some("main"),
            Some("abc123"),
            false,
            None,
            &now,
            &now,
        )
        .await
        .expect("upsert repo");

        db.upsert_agent(
            "agent-1",
            "claude",
            None,
            None,
            Some("repo-1"),
            None,
            None,
            &now,
            None,
        )
        .await
        .expect("upsert agent");

        db.acquire_lease(
            "lease-1",
            "agent-1",
            "/tmp/test-repo/src/main.rs",
            "repo-1",
            300,
            &now,
            &future,
        )
        .await
        .expect("acquire");

        let active = db.list_active_leases().await.expect("list");
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].id, "lease-1");
        assert_eq!(active[0].agent_id, "agent-1");

        db.release_lease("lease-1", &now).await.expect("release");
        let active = db.list_active_leases().await.expect("list");
        assert!(active.is_empty());
    }
}
