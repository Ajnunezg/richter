//! Richter database module: SQLite persistence for runs, claims, agents, events, and configuration.
//!
//! Provides a production-ready [`Database`] backed by rusqlite with WAL mode,
//! foreign keys enabled, a schema-version migration system, and typed CRUD
//! methods for all 15+ tables.

use anyhow::Context;
use rusqlite::params;
use std::path::Path;

// ---------------------------------------------------------------------------
// Schema version
// ---------------------------------------------------------------------------

/// Current schema version. Increment and add a migration in [`run_migrations`]
/// whenever the schema changes.
const CURRENT_SCHEMA_VERSION: u32 = 2;

// ---------------------------------------------------------------------------
// Public database handle
// ---------------------------------------------------------------------------

/// A SQLite connection handle with WAL mode and foreign keys enabled.
///
/// Construct via [`Database::open`], which runs all pending migrations
/// automatically before returning.
///
/// The inner connection is wrapped in a [`parking_lot::Mutex`] to support
/// safe sharing across threads (required by the daemon).
pub struct Database {
    conn: parking_lot::Mutex<rusqlite::Connection>,
}

impl Database {
    /// Opens (or creates) the SQLite database at `path`, enables WAL mode
    /// and foreign keys, then runs all pending schema migrations.
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        let conn = rusqlite::Connection::open(path).context("failed to open SQLite database")?;

        conn.execute_batch("PRAGMA journal_mode=WAL;")
            .context("failed to enable WAL mode")?;
        conn.execute_batch("PRAGMA foreign_keys=ON;")
            .context("failed to enable foreign keys")?;

        run_migrations(&conn).context("failed to run database migrations")?;

        Ok(Self {
            conn: parking_lot::Mutex::new(conn),
        })
    }

    /// Returns a reference to the inner `rusqlite::Connection`.
    #[allow(unused)]
    pub(crate) fn conn(&self) -> parking_lot::MutexGuard<'_, rusqlite::Connection> {
        // Safety: this is used by tests; callers must ensure exclusive access
        // or only use it on single-threaded test paths.
        self.conn.lock()
    }
    
    /// Access the inner connection for tests (single-threaded).
    #[allow(unused)]
    pub(crate) fn conn_lock(&self) -> parking_lot::MutexGuard<'_, rusqlite::Connection> {
        self.conn.lock()
    }

    /// List all non-expired cache entries for startup population.
    pub fn list_non_expired_cache(&self) -> anyhow::Result<Vec<CacheEntryRow>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, fingerprint, run_id, exit_code, output_path,              cached_at, expires_at              FROM run_cache WHERE expires_at IS NULL              OR expires_at > datetime('now')",
        )
        .context("failed to prepare list_non_expired_cache")?;
        let rows = stmt
            .query_map([], row_to_cache_entry)
            .context("failed to query non-expired cache")?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row.context("failed to read cache entry row")?);
        }
        Ok(result)
    }


    // -----------------------------------------------------------------------
    // runs
    // -----------------------------------------------------------------------

    /// Insert a new run row.
    pub fn insert_run(
        &self,
        id: &str,
        repo_id: &str,
        command: &str,
        classification: &str,
        fingerprint: &str,
        resource_class: &str,
    ) -> anyhow::Result<()> {
        let conn = self.conn.lock();
        conn.execute(
                "INSERT INTO runs (id, repo_id, command, classification, fingerprint, \
                 status, resource_class, is_cached) \
                 VALUES (?1, ?2, ?3, ?4, ?5, 'queued', ?6, 0)",
                params![
                    id,
                    repo_id,
                    command,
                    classification,
                    fingerprint,
                    resource_class
                ],
            )
            .context("failed to insert run")
            .map(|_| ())
    }

    /// Update the status (and optionally exit code / timestamps) of a run.
    pub fn update_run_status(
        &self,
        id: &str,
        status: &str,
        exit_code: Option<i32>,
        started_at: Option<&str>,
        completed_at: Option<&str>,
        duration_ms: Option<i64>,
    ) -> anyhow::Result<()> {
        let conn = self.conn.lock();
        conn.execute(
                "UPDATE runs SET status = ?2, exit_code = ?3, \
                 started_at = COALESCE(?4, started_at), \
                 completed_at = COALESCE(?5, completed_at), \
                 duration_ms = COALESCE(?6, duration_ms) \
                 WHERE id = ?1",
                params![id, status, exit_code, started_at, completed_at, duration_ms],
            )
            .context("failed to update run status")
            .map(|_| ())
    }

    /// Fetch a single run by id.
    pub fn get_run(&self, id: &str) -> anyhow::Result<Option<RunRow>> {
        let conn = self.conn.lock();
        conn.query_row(
                "SELECT id, repo_id, command, classification, fingerprint, status, \
                 exit_code, started_at, completed_at, duration_ms, is_cached, \
                 resource_class, output_path, error_path \
                 FROM runs WHERE id = ?1",
                params![id],
                row_to_run,
            )
            .optional()
            .context("failed to fetch run")
    }

    /// List all runs for a given repo, newest first.
    pub fn list_runs_by_repo(&self, repo_id: &str) -> anyhow::Result<Vec<RunRow>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
                "SELECT id, repo_id, command, classification, fingerprint, status, \
                 exit_code, started_at, completed_at, duration_ms, is_cached, \
                 resource_class, output_path, error_path \
                 FROM runs WHERE repo_id = ?1 \
                 ORDER BY COALESCE(started_at, '0000-01-01T00:00:00Z') DESC",
            )
            .context("failed to prepare list_runs_by_repo")?;
        let rows = stmt
            .query_map(params![repo_id], row_to_run)
            .context("failed to query runs by repo")?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row.context("failed to read run row")?);
        }
        Ok(result)
    }

    /// List all runs that are currently queued or running.
    pub fn list_active_runs(&self) -> anyhow::Result<Vec<RunRow>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
                "SELECT id, repo_id, command, classification, fingerprint, status, \
                 exit_code, started_at, completed_at, duration_ms, is_cached, \
                 resource_class, output_path, error_path \
                 FROM runs WHERE status IN ('queued', 'running') \
                 ORDER BY COALESCE(started_at, '0000-01-01T00:00:00Z') ASC",
            )
            .context("failed to prepare list_active_runs")?;
        let rows = stmt
            .query_map([], row_to_run)
            .context("failed to query active runs")?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row.context("failed to read run row")?);
        }
        Ok(result)
    }

    // -----------------------------------------------------------------------
    // events
    // -----------------------------------------------------------------------

    /// Insert a new event.
    #[allow(clippy::too_many_arguments)]
    pub fn insert_event(
        &self,
        id: &str,
        event_type: &str,
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
        let conn = self.conn.lock();
        conn.execute(
                "INSERT INTO events (id, event_type, run_id, repo_id, agent_id, severity, \
                 title, summary, details, importance, should_notify, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                params![
                    id,
                    event_type,
                    run_id,
                    repo_id,
                    agent_id,
                    severity,
                    title,
                    summary,
                    details,
                    importance,
                    should_notify as i32,
                    created_at
                ],
            )
            .context("failed to insert event")
            .map(|_| ())
    }

    /// List events, newest first, with optional entity filters.
    pub fn list_events(
        &self,
        run_id: Option<&str>,
        repo_id: Option<&str>,
        agent_id: Option<&str>,
        limit: usize,
    ) -> anyhow::Result<Vec<EventRow>> {
        let mut clauses: Vec<String> = Vec::new();
        let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        if let Some(rid) = run_id {
            param_values.push(Box::new(rid.to_string()));
            clauses.push(format!("run_id = ?{}", param_values.len()));
        }
        if let Some(rid) = repo_id {
            param_values.push(Box::new(rid.to_string()));
            clauses.push(format!("repo_id = ?{}", param_values.len()));
        }
        if let Some(aid) = agent_id {
            param_values.push(Box::new(aid.to_string()));
            clauses.push(format!("agent_id = ?{}", param_values.len()));
        }

        let where_clause = if clauses.is_empty() {
            String::from("1=1")
        } else {
            clauses.join(" AND ")
        };
        param_values.push(Box::new(limit as i64));
        let limit_placeholder = param_values.len();

        let sql = format!(
            "SELECT id, event_type, run_id, repo_id, agent_id, severity, \
             title, summary, details, importance, should_notify, created_at \
             FROM events WHERE {where_clause} \
             ORDER BY created_at DESC LIMIT ?{limit_placeholder}"
        );

        let params_refs: Vec<&dyn rusqlite::types::ToSql> =
            param_values.iter().map(|b| b.as_ref()).collect();

        let conn = self.conn.lock();
        let mut stmt = conn.prepare(&sql)
            .context("failed to prepare list_events")?;
        let rows = stmt
            .query_map(params_refs.as_slice(), row_to_event)
            .context("failed to query events")?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row.context("failed to read event row")?);
        }
        Ok(result)
    }

    /// List important events, newest first.
    pub fn list_important_events(&self, limit: usize) -> anyhow::Result<Vec<ImportantEventRow>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
                "SELECT id, event_id, importance, category, \
                 recommended_action, acknowledged, created_at \
                 FROM important_events \
                 ORDER BY importance DESC, created_at DESC \
                 LIMIT ?1",
            )
            .context("failed to prepare list_important_events")?;
        let rows = stmt
            .query_map(params![limit as i64], row_to_important_event)
            .context("failed to query important events")?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row.context("failed to read important event row")?);
        }
        Ok(result)
    }

    // -----------------------------------------------------------------------
    // run_cache
    // -----------------------------------------------------------------------

    /// Insert a cache entry for a completed run.
    #[allow(clippy::too_many_arguments)]
    pub fn insert_cache_entry(
        &self,
        id: &str,
        fingerprint: &str,
        run_id: &str,
        exit_code: i32,
        output_path: Option<&str>,
        cached_at: &str,
        expires_at: Option<&str>,
    ) -> anyhow::Result<()> {
        let conn = self.conn.lock();
        conn.execute(
                "INSERT INTO run_cache (id, fingerprint, run_id, exit_code, output_path, \
                 cached_at, expires_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    id,
                    fingerprint,
                    run_id,
                    exit_code,
                    output_path,
                    cached_at,
                    expires_at
                ],
            )
            .context("failed to insert cache entry")
            .map(|_| ())
    }

    /// Look up a cache entry by fingerprint.
    pub fn get_cache_entry(&self, fingerprint: &str) -> anyhow::Result<Option<CacheEntryRow>> {
        let conn = self.conn.lock();
        conn.query_row(
                "SELECT id, fingerprint, run_id, exit_code, output_path, \
                 cached_at, expires_at \
                 FROM run_cache WHERE fingerprint = ?1",
                params![fingerprint],
                row_to_cache_entry,
            )
            .optional()
            .context("failed to fetch cache entry")
    }

    /// Remove all cache entries whose `expires_at` is in the past.
    pub fn evict_expired_cache(&self) -> anyhow::Result<usize> {
        let conn = self.conn.lock();
        conn.execute(
                "DELETE FROM run_cache WHERE expires_at IS NOT NULL \
                 AND expires_at < datetime('now')",
                [],
            )
            .context("failed to evict expired cache entries")
    }

    // -----------------------------------------------------------------------
    // repositories
    // -----------------------------------------------------------------------

    /// Insert or replace a repository row.
    #[allow(clippy::too_many_arguments)]
    pub fn upsert_repo(
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
        let conn = self.conn.lock();
        conn.execute(
                "INSERT INTO repositories (id, name, root, branch, head_sha, is_dirty, \
                 upstream, created_at, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9) \
                 ON CONFLICT(id) DO UPDATE SET \
                 name = excluded.name, root = excluded.root, \
                 branch = excluded.branch, head_sha = excluded.head_sha, \
                 is_dirty = excluded.is_dirty, upstream = excluded.upstream, \
                 updated_at = excluded.updated_at",
                params![
                    id,
                    name,
                    root,
                    branch,
                    head_sha,
                    is_dirty as i32,
                    upstream,
                    created_at,
                    updated_at
                ],
            )
            .context("failed to upsert repository")
            .map(|_| ())
    }

    /// Fetch a repository by id.
    pub fn get_repo(&self, id: &str) -> anyhow::Result<Option<RepoRow>> {
        let conn = self.conn.lock();
        conn.query_row(
                "SELECT id, name, root, branch, head_sha, is_dirty, upstream, \
                 created_at, updated_at \
                 FROM repositories WHERE id = ?1",
                params![id],
                row_to_repo,
            )
            .optional()
            .context("failed to fetch repository")
    }

    /// List all registered repositories.
    pub fn list_repos(&self) -> anyhow::Result<Vec<RepoRow>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
                "SELECT id, name, root, branch, head_sha, is_dirty, upstream, \
                 created_at, updated_at \
                 FROM repositories ORDER BY name ASC",
            )
            .context("failed to prepare list_repos")?;
        let rows = stmt
            .query_map([], row_to_repo)
            .context("failed to query repositories")?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row.context("failed to read repository row")?);
        }
        Ok(result)
    }

    // -----------------------------------------------------------------------
    // agents
    // -----------------------------------------------------------------------

    /// Insert or replace an agent row.
    #[allow(clippy::too_many_arguments)]
    pub fn upsert_agent(
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
        let conn = self.conn.lock();
        conn.execute(
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
                params![
                    id,
                    agent_type,
                    name,
                    cwd,
                    repo_id,
                    worktree_id,
                    active_command,
                    last_seen_at,
                    metadata
                ],
            )
            .context("failed to upsert agent")
            .map(|_| ())
    }

    /// Fetch an agent by id.
    pub fn get_agent(&self, id: &str) -> anyhow::Result<Option<AgentRow>> {
        let conn = self.conn.lock();
        conn.query_row(
                "SELECT id, agent_type, name, cwd, repo_id, worktree_id, \
                 active_command, last_seen_at, metadata \
                 FROM agents WHERE id = ?1",
                params![id],
                row_to_agent,
            )
            .optional()
            .context("failed to fetch agent")
    }

    /// List all agents associated with a given repo.
    pub fn list_agents_by_repo(&self, repo_id: &str) -> anyhow::Result<Vec<AgentRow>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
                "SELECT id, agent_type, name, cwd, repo_id, worktree_id, \
                 active_command, last_seen_at, metadata \
                 FROM agents WHERE repo_id = ?1 \
                 ORDER BY last_seen_at DESC",
            )
            .context("failed to prepare list_agents_by_repo")?;
        let rows = stmt
            .query_map(params![repo_id], row_to_agent)
            .context("failed to query agents by repo")?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row.context("failed to read agent row")?);
        }
        Ok(result)
    }

    // -----------------------------------------------------------------------
    // leases
    // -----------------------------------------------------------------------

    /// Acquire a lease (insert a new active lease row).
    #[allow(clippy::too_many_arguments)]
    pub fn acquire_lease(
        &self,
        id: &str,
        agent_id: &str,
        path: &str,
        repo_id: &str,
        ttl_seconds: i64,
        acquired_at: &str,
        expires_at: &str,
    ) -> anyhow::Result<()> {
        let conn = self.conn.lock();
        conn.execute(
                "INSERT INTO leases (id, agent_id, path, repo_id, ttl_seconds, \
                 acquired_at, expires_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    id,
                    agent_id,
                    path,
                    repo_id,
                    ttl_seconds,
                    acquired_at,
                    expires_at
                ],
            )
            .context("failed to acquire lease")
            .map(|_| ())
    }

    /// Release a lease by setting `released_at`.
    pub fn release_lease(&self, id: &str, released_at: &str) -> anyhow::Result<()> {
        let conn = self.conn.lock();
        conn.execute(
                "UPDATE leases SET released_at = ?2 WHERE id = ?1",
                params![id, released_at],
            )
            .context("failed to release lease")
            .map(|_| ())
    }

    /// List all leases that have not yet been released and are not expired.
    pub fn list_active_leases(&self) -> anyhow::Result<Vec<LeaseRow>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
                "SELECT id, agent_id, path, repo_id, ttl_seconds, acquired_at, \
                 expires_at, released_at \
                 FROM leases WHERE released_at IS NULL AND expires_at > datetime('now') \
                 ORDER BY acquired_at ASC",
            )
            .context("failed to prepare list_active_leases")?;
        let rows = stmt
            .query_map([], row_to_lease)
            .context("failed to query active leases")?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row.context("failed to read lease row")?);
        }
        Ok(result)
    }

    // -----------------------------------------------------------------------
    // settings
    // -----------------------------------------------------------------------

    /// Fetch a setting value by key.
    pub fn get_setting(&self, key: &str) -> anyhow::Result<Option<String>> {
        let conn = self.conn.lock();
        conn.query_row(
                "SELECT value FROM settings WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()
            .context("failed to fetch setting")
    }

    /// Insert or replace a setting key-value pair.
    pub fn set_setting(&self, key: &str, value: &str, updated_at: &str) -> anyhow::Result<()> {
        let conn = self.conn.lock();
        conn.execute(
                "INSERT INTO settings (key, value, updated_at) \
                 VALUES (?1, ?2, ?3) \
                 ON CONFLICT(key) DO UPDATE SET \
                 value = excluded.value, updated_at = excluded.updated_at",
                params![key, value, updated_at],
            )
            .context("failed to set setting")
            .map(|_| ())
    }

    // -----------------------------------------------------------------------
    // model_calls
    // -----------------------------------------------------------------------

    /// Record a call to an external LLM.
    #[allow(clippy::too_many_arguments)]
    pub fn record_model_call(
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
        let conn = self.conn.lock();
        conn.execute(
                "INSERT INTO model_calls (id, provider, model, purpose, \
                 input_tokens, output_tokens, cost_cents, duration_ms, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    id,
                    provider,
                    model,
                    purpose,
                    input_tokens,
                    output_tokens,
                    cost_cents,
                    duration_ms,
                    created_at
                ],
            )
            .context("failed to record model call")
            .map(|_| ())
    }

    // -----------------------------------------------------------------------
    // decisions
    // -----------------------------------------------------------------------

    /// Insert a run-or-join (or other) decision record.
    #[allow(clippy::too_many_arguments)]
    pub fn insert_decision(
        &self,
        id: &str,
        run_id: &str,
        decision_type: &str,
        reason: Option<&str>,
        details: Option<&str>,
        model_used: Option<&str>,
        decided_at: &str,
    ) -> anyhow::Result<()> {
        let conn = self.conn.lock();
        conn.execute(
                "INSERT INTO decisions (id, run_id, decision_type, reason, details, \
                 model_used, decided_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    id,
                    run_id,
                    decision_type,
                    reason,
                    details,
                    model_used,
                    decided_at
                ],
            )
            .context("failed to insert decision")
            .map(|_| ())
    }
}

// ---------------------------------------------------------------------------
// Row types
// ---------------------------------------------------------------------------

/// A row from the `runs` table.
#[derive(Debug, Clone)]
pub struct RunRow {
    /// Primary key.
    pub id: String,
    /// FK to repositories.
    pub repo_id: String,
    /// The raw command string.
    pub command: String,
    /// Classification label (build, test, lint, etc.).
    pub classification: String,
    /// Content-addressable fingerprint of the command + context.
    pub fingerprint: String,
    /// Current lifecycle status.
    pub status: String,
    /// Exit code (None while running).
    pub exit_code: Option<i32>,
    /// ISO 8601 timestamp when execution began.
    pub started_at: Option<String>,
    /// ISO 8601 timestamp when execution completed.
    pub completed_at: Option<String>,
    /// Wall-clock duration in milliseconds.
    pub duration_ms: Option<i64>,
    /// Whether this run was served from cache.
    pub is_cached: bool,
    /// Resource class for scheduling.
    pub resource_class: String,
    /// Path to captured stdout log.
    pub output_path: Option<String>,
    /// Path to captured stderr log.
    pub error_path: Option<String>,
}

fn row_to_run(row: &rusqlite::Row<'_>) -> rusqlite::Result<RunRow> {
    Ok(RunRow {
        id: row.get(0)?,
        repo_id: row.get(1)?,
        command: row.get(2)?,
        classification: row.get(3)?,
        fingerprint: row.get(4)?,
        status: row.get(5)?,
        exit_code: row.get(6)?,
        started_at: row.get(7)?,
        completed_at: row.get(8)?,
        duration_ms: row.get(9)?,
        is_cached: row.get::<_, i32>(10)? != 0,
        resource_class: row.get(11)?,
        output_path: row.get(12)?,
        error_path: row.get(13)?,
    })
}

/// A row from the `events` table.
#[derive(Debug, Clone)]
pub struct EventRow {
    /// Primary key.
    pub id: String,
    /// Kind of event.
    pub event_type: String,
    /// Optional FK to runs.
    pub run_id: Option<String>,
    /// Optional FK to repositories.
    pub repo_id: Option<String>,
    /// Optional FK to agents.
    pub agent_id: Option<String>,
    /// Severity label.
    pub severity: Option<String>,
    /// Short title.
    pub title: String,
    /// Optional human-readable summary.
    pub summary: Option<String>,
    /// Optional detailed payload (JSON).
    pub details: Option<String>,
    /// Importance score.
    pub importance: i32,
    /// Whether this should trigger a notification.
    pub should_notify: bool,
    /// ISO 8601 timestamp.
    pub created_at: String,
}

fn row_to_event(row: &rusqlite::Row<'_>) -> rusqlite::Result<EventRow> {
    Ok(EventRow {
        id: row.get(0)?,
        event_type: row.get(1)?,
        run_id: row.get(2)?,
        repo_id: row.get(3)?,
        agent_id: row.get(4)?,
        severity: row.get(5)?,
        title: row.get(6)?,
        summary: row.get(7)?,
        details: row.get(8)?,
        importance: row.get(9)?,
        should_notify: row.get::<_, i32>(10)? != 0,
        created_at: row.get(11)?,
    })
}

/// A row from the `important_events` table.
#[derive(Debug, Clone)]
pub struct ImportantEventRow {
    /// Primary key.
    pub id: String,
    /// FK to the underlying event.
    pub event_id: String,
    /// Importance score.
    pub importance: i32,
    /// Category label.
    pub category: Option<String>,
    /// Recommended action text.
    pub recommended_action: Option<String>,
    /// Whether the user acknowledged this event.
    pub acknowledged: bool,
    /// ISO 8601 timestamp.
    pub created_at: String,
}

fn row_to_important_event(row: &rusqlite::Row<'_>) -> rusqlite::Result<ImportantEventRow> {
    Ok(ImportantEventRow {
        id: row.get(0)?,
        event_id: row.get(1)?,
        importance: row.get(2)?,
        category: row.get(3)?,
        recommended_action: row.get(4)?,
        acknowledged: row.get::<_, i32>(5)? != 0,
        created_at: row.get(6)?,
    })
}

/// A row from the `run_cache` table.
#[derive(Debug, Clone)]
pub struct CacheEntryRow {
    /// Primary key.
    pub id: String,
    /// Content fingerprint for cache lookup.
    pub fingerprint: String,
    /// FK to the run that produced this result.
    pub run_id: String,
    /// Cached exit code.
    pub exit_code: i32,
    /// Path to cached output log.
    pub output_path: Option<String>,
    /// ISO 8601 timestamp when cached.
    pub cached_at: String,
    /// ISO 8601 timestamp when this entry expires.
    pub expires_at: Option<String>,
}

fn row_to_cache_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<CacheEntryRow> {
    Ok(CacheEntryRow {
        id: row.get(0)?,
        fingerprint: row.get(1)?,
        run_id: row.get(2)?,
        exit_code: row.get(3)?,
        output_path: row.get(4)?,
        cached_at: row.get(5)?,
        expires_at: row.get(6)?,
    })
}

/// A row from the `repositories` table.
#[derive(Debug, Clone)]
pub struct RepoRow {
    /// Primary key.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Absolute path to the repository root.
    pub root: String,
    /// Current branch name.
    pub branch: Option<String>,
    /// HEAD commit SHA.
    pub head_sha: Option<String>,
    /// Whether the working tree has uncommitted changes.
    pub is_dirty: bool,
    /// Upstream remote URL.
    pub upstream: Option<String>,
    /// ISO 8601 timestamp of creation.
    pub created_at: String,
    /// ISO 8601 timestamp of last update.
    pub updated_at: String,
}

fn row_to_repo(row: &rusqlite::Row<'_>) -> rusqlite::Result<RepoRow> {
    Ok(RepoRow {
        id: row.get(0)?,
        name: row.get(1)?,
        root: row.get(2)?,
        branch: row.get(3)?,
        head_sha: row.get(4)?,
        is_dirty: row.get::<_, i32>(5)? != 0,
        upstream: row.get(6)?,
        created_at: row.get(7)?,
        updated_at: row.get(8)?,
    })
}

/// A row from the `agents` table.
#[derive(Debug, Clone)]
pub struct AgentRow {
    /// Primary key.
    pub id: String,
    /// Agent program name (claude, codex, aider, etc.).
    pub agent_type: String,
    /// Optional human-readable name.
    pub name: Option<String>,
    /// Working directory.
    pub cwd: Option<String>,
    /// FK to repositories.
    pub repo_id: Option<String>,
    /// FK to worktrees.
    pub worktree_id: Option<String>,
    /// Currently executing command.
    pub active_command: Option<String>,
    /// ISO 8601 timestamp of last activity.
    pub last_seen_at: String,
    /// JSON-encoded metadata map.
    pub metadata: Option<String>,
}

fn row_to_agent(row: &rusqlite::Row<'_>) -> rusqlite::Result<AgentRow> {
    Ok(AgentRow {
        id: row.get(0)?,
        agent_type: row.get(1)?,
        name: row.get(2)?,
        cwd: row.get(3)?,
        repo_id: row.get(4)?,
        worktree_id: row.get(5)?,
        active_command: row.get(6)?,
        last_seen_at: row.get(7)?,
        metadata: row.get(8)?,
    })
}

/// A row from the `leases` table.
#[derive(Debug, Clone)]
pub struct LeaseRow {
    /// Primary key.
    pub id: String,
    /// FK to the agent holding the lease.
    pub agent_id: String,
    /// The path being claimed.
    pub path: String,
    /// FK to the repository.
    pub repo_id: String,
    /// Lease duration in seconds.
    pub ttl_seconds: i64,
    /// ISO 8601 timestamp when acquired.
    pub acquired_at: String,
    /// ISO 8601 timestamp when the lease expires.
    pub expires_at: String,
    /// ISO 8601 timestamp when released (None = still active).
    pub released_at: Option<String>,
}

fn row_to_lease(row: &rusqlite::Row<'_>) -> rusqlite::Result<LeaseRow> {
    Ok(LeaseRow {
        id: row.get(0)?,
        agent_id: row.get(1)?,
        path: row.get(2)?,
        repo_id: row.get(3)?,
        ttl_seconds: row.get(4)?,
        acquired_at: row.get(5)?,
        expires_at: row.get(6)?,
        released_at: row.get(7)?,
    })
}

// ---------------------------------------------------------------------------
// OptionalExt
// ---------------------------------------------------------------------------

/// Extension trait that converts `Result<T, E>` into `Result<Option<T>, E>`
/// by treating `rusqlite::Error::QueryReturnedNoRows` as `Ok(None)`.
trait OptionalExt<T, E> {
    /// Converts the error variant `QueryReturnedNoRows` into `Ok(None)`.
    fn optional(self) -> Result<Option<T>, E>;
}

impl<T> OptionalExt<T, rusqlite::Error> for Result<T, rusqlite::Error> {
    fn optional(self) -> Result<Option<T>, rusqlite::Error> {
        match self {
            Ok(value) => Ok(Some(value)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

// ---------------------------------------------------------------------------
// Migration engine
// ---------------------------------------------------------------------------

/// Ensures the schema is at [`CURRENT_SCHEMA_VERSION`] by running any
/// pending migrations in order.
fn run_migrations(conn: &rusqlite::Connection) -> anyhow::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS _schema_version (
            version INTEGER NOT NULL
        );",
    )
    .context("failed to create _schema_version table")?;

    let current: u32 = conn
        .query_row(
            "SELECT COALESCE(version, 0) FROM _schema_version LIMIT 1",
            [],
            |row| row.get::<_, i64>(0).map(|v| v as u32),
        )
        .unwrap_or(0);

    for v in (current + 1)..=CURRENT_SCHEMA_VERSION {
        if let Some(migrate) = migration(v) {
            migrate(conn).with_context(|| format!("failed to run migration v{v}"))?;
        }
        conn.execute("DELETE FROM _schema_version", [])
            .with_context(|| format!("failed to record schema version v{v}"))?;
        conn.execute(
            "INSERT INTO _schema_version (version) VALUES (?1)",
            params![v as i64],
        )
        .with_context(|| format!("failed to record schema version v{v}"))?;
    }

    Ok(())
}

/// Returns the migration function for the given schema version.
fn migration(version: u32) -> Option<fn(&rusqlite::Connection) -> anyhow::Result<()>> {
    match version {
        1 => Some(migration_v1),
        2 => Some(migration_v2),
        _ => None,
    }
}

/// Migration v1: create all initial tables and indexes.
fn migration_v1(conn: &rusqlite::Connection) -> anyhow::Result<()> {
    conn.execute_batch(
        "
        CREATE TABLE repositories (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            root TEXT NOT NULL UNIQUE,
            branch TEXT,
            head_sha TEXT,
            is_dirty INTEGER NOT NULL DEFAULT 0,
            upstream TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );

        CREATE TABLE worktrees (
            id TEXT PRIMARY KEY,
            repo_id TEXT NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
            path TEXT NOT NULL UNIQUE,
            branch TEXT,
            head_sha TEXT,
            is_locked INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL
        );

        CREATE TABLE agents (
            id TEXT PRIMARY KEY,
            agent_type TEXT NOT NULL,
            name TEXT,
            cwd TEXT,
            repo_id TEXT REFERENCES repositories(id),
            worktree_id TEXT REFERENCES worktrees(id),
            active_command TEXT,
            last_seen_at TEXT NOT NULL,
            metadata TEXT
        );

        CREATE TABLE command_invocations (
            id TEXT PRIMARY KEY,
            run_id TEXT NOT NULL REFERENCES runs(id),
            agent_id TEXT REFERENCES agents(id),
            repo_id TEXT NOT NULL REFERENCES repositories(id),
            command TEXT NOT NULL,
            classification TEXT NOT NULL,
            fingerprint TEXT NOT NULL,
            resource_class TEXT NOT NULL,
            use_shell INTEGER NOT NULL DEFAULT 1,
            invoked_at TEXT NOT NULL
        );

        CREATE TABLE runs (
            id TEXT PRIMARY KEY,
            repo_id TEXT NOT NULL REFERENCES repositories(id),
            command TEXT NOT NULL,
            classification TEXT NOT NULL,
            fingerprint TEXT NOT NULL,
            status TEXT NOT NULL DEFAULT 'queued',
            exit_code INTEGER,
            started_at TEXT,
            completed_at TEXT,
            duration_ms INTEGER,
            is_cached INTEGER NOT NULL DEFAULT 0,
            resource_class TEXT NOT NULL DEFAULT 'unknown',
            output_path TEXT,
            error_path TEXT
        );

        CREATE TABLE run_subscribers (
            run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
            agent_id TEXT REFERENCES agents(id),
            subscribed_at TEXT NOT NULL,
            detached_at TEXT,
            PRIMARY KEY (run_id, agent_id)
        );

        CREATE TABLE run_artifacts (
            id TEXT PRIMARY KEY,
            run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
            artifact_type TEXT NOT NULL,
            path TEXT NOT NULL,
            size_bytes INTEGER,
            sha256 TEXT,
            created_at TEXT NOT NULL
        );

        CREATE TABLE run_cache (
            id TEXT PRIMARY KEY,
            fingerprint TEXT NOT NULL UNIQUE,
            run_id TEXT NOT NULL REFERENCES runs(id),
            exit_code INTEGER NOT NULL,
            output_path TEXT,
            cached_at TEXT NOT NULL,
            expires_at TEXT
        );

        CREATE TABLE events (
            id TEXT PRIMARY KEY,
            event_type TEXT NOT NULL,
            run_id TEXT,
            repo_id TEXT,
            agent_id TEXT,
            severity TEXT,
            title TEXT,
            summary TEXT,
            details TEXT,
            importance INTEGER DEFAULT 0,
            should_notify INTEGER DEFAULT 0,
            created_at TEXT NOT NULL
        );

        CREATE TABLE important_events (
            id TEXT PRIMARY KEY,
            event_id TEXT NOT NULL REFERENCES events(id),
            importance INTEGER NOT NULL,
            category TEXT,
            recommended_action TEXT,
            acknowledged INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL
        );

        CREATE TABLE decisions (
            id TEXT PRIMARY KEY,
            run_id TEXT NOT NULL REFERENCES runs(id),
            decision_type TEXT NOT NULL,
            reason TEXT,
            details TEXT,
            model_used TEXT,
            decided_at TEXT NOT NULL
        );

        CREATE TABLE leases (
            id TEXT PRIMARY KEY,
            agent_id TEXT NOT NULL REFERENCES agents(id),
            path TEXT NOT NULL,
            repo_id TEXT NOT NULL REFERENCES repositories(id),
            ttl_seconds INTEGER NOT NULL,
            acquired_at TEXT NOT NULL,
            expires_at TEXT NOT NULL,
            released_at TEXT
        );

        CREATE TABLE model_calls (
            id TEXT PRIMARY KEY,
            provider TEXT NOT NULL,
            model TEXT NOT NULL,
            purpose TEXT NOT NULL,
            input_tokens INTEGER,
            output_tokens INTEGER,
            cost_cents REAL,
            duration_ms INTEGER,
            created_at TEXT NOT NULL
        );

        CREATE TABLE settings (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );

        CREATE TABLE plugin_manifests (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            version TEXT NOT NULL,
            enabled INTEGER NOT NULL DEFAULT 1,
            config TEXT,
            installed_at TEXT NOT NULL
        );

        CREATE INDEX idx_runs_repo_id ON runs(repo_id);
        CREATE INDEX idx_runs_fingerprint ON runs(fingerprint);
        CREATE INDEX idx_runs_status ON runs(status);
        CREATE INDEX idx_runs_started_at ON runs(started_at);
        CREATE INDEX idx_events_run_id ON events(run_id);
        CREATE INDEX idx_events_repo_id ON events(repo_id);
        CREATE INDEX idx_events_agent_id ON events(agent_id);
        CREATE INDEX idx_events_created_at ON events(created_at);
        CREATE INDEX idx_run_cache_fingerprint ON run_cache(fingerprint);
        CREATE INDEX idx_leases_agent_id ON leases(agent_id);
        CREATE INDEX idx_leases_path ON leases(path);
        CREATE INDEX idx_agents_repo_id ON agents(repo_id);
        ",
    )
    .context("failed to execute migration v1")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Migration v2: add mobile companion tables and indexes.
fn migration_v2(conn: &rusqlite::Connection) -> anyhow::Result<()> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS mobile_devices (
            id TEXT PRIMARY KEY,
            display_name TEXT NOT NULL,
            platform TEXT NOT NULL,
            device_public_key TEXT NOT NULL,
            scopes_json TEXT NOT NULL DEFAULT '[]',
            created_at TEXT NOT NULL,
            last_seen_at TEXT NOT NULL,
            revoked_at TEXT,
            revocation_reason TEXT,
            push_enabled INTEGER NOT NULL DEFAULT 0,
            relay_enabled INTEGER NOT NULL DEFAULT 0,
            app_version TEXT,
            os_version TEXT
        );

        CREATE TABLE IF NOT EXISTS mobile_pairing_sessions (
            id TEXT PRIMARY KEY,
            pairing_secret_hash TEXT NOT NULL,
            server_pubkey_fingerprint TEXT NOT NULL,
            requested_scopes_json TEXT NOT NULL DEFAULT '[]',
            expires_at TEXT NOT NULL,
            claimed_at TEXT,
            claimed_device_id TEXT,
            created_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS mobile_device_sessions (
            id TEXT PRIMARY KEY,
            device_id TEXT NOT NULL REFERENCES mobile_devices(id) ON DELETE CASCADE,
            token_hash TEXT NOT NULL,
            created_at TEXT NOT NULL,
            expires_at TEXT NOT NULL,
            revoked INTEGER NOT NULL DEFAULT 0
        );

        CREATE TABLE IF NOT EXISTS mobile_push_subscriptions (
            id TEXT PRIMARY KEY,
            device_id TEXT NOT NULL REFERENCES mobile_devices(id) ON DELETE CASCADE,
            push_token TEXT NOT NULL,
            provider TEXT NOT NULL,
            created_at TEXT NOT NULL,
            revoked_at TEXT
        );

        CREATE TABLE IF NOT EXISTS mobile_notification_deliveries (
            id TEXT PRIMARY KEY,
            event_id TEXT NOT NULL,
            device_id TEXT NOT NULL REFERENCES mobile_devices(id) ON DELETE CASCADE,
            importance INTEGER NOT NULL,
            category TEXT NOT NULL,
            push_payload TEXT,
            delivered_at TEXT NOT NULL,
            opened_at TEXT
        );

        CREATE TABLE IF NOT EXISTS mobile_gateway_audit_log (
            id TEXT PRIMARY KEY,
            device_id TEXT,
            action TEXT NOT NULL,
            target_type TEXT,
            target_id TEXT,
            risk_level TEXT,
            allowed INTEGER NOT NULL DEFAULT 0,
            reason TEXT,
            ip_address_hash TEXT,
            created_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS mobile_relay_sessions (
            id TEXT PRIMARY KEY,
            device_id TEXT NOT NULL REFERENCES mobile_devices(id) ON DELETE CASCADE,
            relay_id TEXT NOT NULL,
            channel_id TEXT NOT NULL,
            established_at TEXT NOT NULL,
            closed_at TEXT
        );

        CREATE TABLE IF NOT EXISTS mobile_capability_grants (
            id TEXT PRIMARY KEY,
            device_id TEXT NOT NULL REFERENCES mobile_devices(id) ON DELETE CASCADE,
            capability TEXT NOT NULL,
            granted_at TEXT NOT NULL,
            expires_at TEXT,
            revoked INTEGER NOT NULL DEFAULT 0
        );

        CREATE TABLE IF NOT EXISTS mobile_security_events (
            id TEXT PRIMARY KEY,
            device_id TEXT,
            event_type TEXT NOT NULL,
            severity TEXT NOT NULL,
            description TEXT NOT NULL,
            ip_address_hash TEXT,
            created_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS mobile_device_tokens (
            id TEXT PRIMARY KEY,
            device_id TEXT NOT NULL REFERENCES mobile_devices(id) ON DELETE CASCADE,
            token_type TEXT NOT NULL,
            token_hash TEXT NOT NULL,
            created_at TEXT NOT NULL,
            expires_at TEXT,
            revoked INTEGER NOT NULL DEFAULT 0
        );

        CREATE INDEX IF NOT EXISTS idx_mobile_devices_public_key ON mobile_devices(device_public_key);
        CREATE INDEX IF NOT EXISTS idx_mobile_pairing_sessions_expires ON mobile_pairing_sessions(expires_at);
        CREATE INDEX IF NOT EXISTS idx_mobile_device_sessions_device ON mobile_device_sessions(device_id);
        CREATE INDEX IF NOT EXISTS idx_mobile_push_subscriptions_device ON mobile_push_subscriptions(device_id);
        CREATE INDEX IF NOT EXISTS idx_mobile_gateway_audit_device ON mobile_gateway_audit_log(device_id);
        CREATE INDEX IF NOT EXISTS idx_mobile_security_events_device ON mobile_security_events(device_id);
        CREATE INDEX IF NOT EXISTS idx_mobile_device_tokens_device ON mobile_device_tokens(device_id);
        "
    )
    .context("migration v2: failed to create mobile companion tables")?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    fn open_test_db() -> Database {
        let tmp = NamedTempFile::new().expect("tempfile");
        Database::open(tmp.path()).expect("open")
    }

    fn now_iso() -> String {
        "2026-05-04T12:00:00Z".to_string()
    }

    // -----------------------------------------------------------------------
    // test_open
    // -----------------------------------------------------------------------

    #[test]
    fn test_open() {
        let tmp = NamedTempFile::new().expect("tempfile");
        let db = Database::open(tmp.path()).expect("open works");

        let journal_mode: String = db
            .conn.lock().pragma_query_value(None, "journal_mode", |row| row.get(0))
            .expect("pragma");
        assert_eq!(journal_mode, "wal");

        let fk: bool = db
            .conn.lock().pragma_query_value(None, "foreign_keys", |row| row.get(0))
            .expect("pragma");
        assert!(fk);

        let version: i64 = db
            .conn.lock().query_row("SELECT version FROM _schema_version LIMIT 1", [], |row| {
                row.get(0)
            })
            .expect("version query");
        assert_eq!(version as u32, CURRENT_SCHEMA_VERSION); // v2 with mobile tables
    }

    // -----------------------------------------------------------------------
    // test_migration
    // -----------------------------------------------------------------------

    #[test]
    fn test_migration() {
        let tmp = NamedTempFile::new().expect("tempfile");
        let path = tmp.path();

        // Open once — v1 migration runs.
        {
            let db = Database::open(path).expect("open 1");
            let cnt: i64 = db
                .conn.lock().query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table'",
                    [],
                    |row| row.get(0),
                )
                .expect("count");
            // _schema_version + 15 data tables = 16
            assert!(cnt >= 16, "expected at least 16 tables, got {cnt}");
        }

        // Re-open — migration should be a no-op (version already 1).
        {
            let db = Database::open(path).expect("open 2");
            let version: i64 = db
                .conn.lock().query_row("SELECT version FROM _schema_version LIMIT 1", [], |row| {
                    row.get(0)
                })
                .expect("version");
            assert_eq!(version as u32, CURRENT_SCHEMA_VERSION); // v2 with mobile tables
        }
    }

    // -----------------------------------------------------------------------
    // test_insert_and_get_run
    // -----------------------------------------------------------------------

    #[test]
    fn test_insert_and_get_run() {
        let db = open_test_db();
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
        .expect("upsert repo");

        db.insert_run("run-1", "repo-1", "cargo test", "test", "fp-abc", "light")
            .expect("insert run");

        let run = db.get_run("run-1").expect("get_run").expect("present");
        assert_eq!(run.id, "run-1");
        assert_eq!(run.repo_id, "repo-1");
        assert_eq!(run.command, "cargo test");
        assert_eq!(run.classification, "test");
        assert_eq!(run.fingerprint, "fp-abc");
        assert_eq!(run.status, "queued");
        assert_eq!(run.exit_code, None);
        assert!(!run.is_cached);

        db.update_run_status("run-1", "running", None, Some(&now), None, None)
            .expect("update");
        let run = db.get_run("run-1").expect("get_run").expect("present");
        assert_eq!(run.status, "running");
        assert_eq!(run.started_at.as_deref(), Some(&*now));

        let runs = db.list_runs_by_repo("repo-1").expect("list");
        assert_eq!(runs.len(), 1);

        let active = db.list_active_runs().expect("active");
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].id, "run-1");

        db.update_run_status("run-1", "passed", Some(0), None, Some(&now), Some(1500))
            .expect("complete");
        let run = db.get_run("run-1").expect("get_run").expect("present");
        assert_eq!(run.status, "passed");
        assert_eq!(run.exit_code, Some(0));
        assert_eq!(run.duration_ms, Some(1500));

        let active = db.list_active_runs().expect("active");
        assert!(active.is_empty());
    }

    // -----------------------------------------------------------------------
    // test_cache_eviction
    // -----------------------------------------------------------------------

    #[test]
    fn test_cache_eviction() {
        let db = open_test_db();
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
        .expect("upsert repo");

        db.insert_run(
            "run-1",
            "repo-1",
            "cargo build",
            "build",
            "fp-build",
            "heavy_build",
        )
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
        .expect("insert cache");

        let entry = db
            .get_cache_entry("fp-build")
            .expect("get")
            .expect("present");
        assert_eq!(entry.exit_code, 0);

        let evicted = db.evict_expired_cache().expect("evict");
        assert_eq!(evicted, 0);
        assert!(db.get_cache_entry("fp-build").expect("get").is_some());

        db.insert_cache_entry(
            "cache-2",
            "fp-lint",
            "run-1",
            0,
            None,
            &now,
            Some("2020-01-01T00:00:00Z"),
        )
        .expect("insert cache 2");

        let evicted = db.evict_expired_cache().expect("evict");
        assert_eq!(evicted, 1);
        assert!(db.get_cache_entry("fp-lint").expect("get").is_none());
        assert!(db.get_cache_entry("fp-build").expect("get").is_some());
    }

    // -----------------------------------------------------------------------
    // test_lease_lifecycle
    // -----------------------------------------------------------------------

    #[test]
    fn test_lease_lifecycle() {
        let db = open_test_db();
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
        .expect("upsert repo");

        db.upsert_agent(
            "agent-1",
            "claude",
            Some("Claude"),
            Some("/tmp/cwd"),
            Some("repo-1"),
            None,
            None,
            &now,
            Some(r#"{"version":"1.0"}"#),
        )
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
        .expect("acquire lease");

        let active = db.list_active_leases().expect("list");
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].id, "lease-1");
        assert_eq!(active[0].agent_id, "agent-1");

        db.release_lease("lease-1", &now).expect("release");
        let active = db.list_active_leases().expect("list");
        assert!(active.is_empty());
    }
}
