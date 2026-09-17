//! SQLite persistence layer for farmerbob.
//!
//! Wraps a `rusqlite::Connection` with schema management, migrations,
//! and CRUD operations for all domain entities. Every multi-statement
//! write runs inside a transaction.

pub mod migrate;

mod error;

use std::path::Path;

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use serde_json::Value;

use farmerbob_core as core;
use core::agent::{Agent, AgentKind};
use core::experiment::Experiment;
use core::grade::Grade;
use core::ids::{AgentId, RunId, TaskId};
use core::run::{Run, RunState};
use core::task::Task;

pub use error::StoreError;

const SCHEMA_SQL: &str = "\
    CREATE TABLE IF NOT EXISTS agents (\
        id TEXT PRIMARY KEY,\
        kind TEXT NOT NULL,\
        model TEXT NOT NULL,\
        provider TEXT NOT NULL\
    );\
    CREATE TABLE IF NOT EXISTS tasks (\
        id TEXT PRIMARY KEY,\
        name TEXT NOT NULL,\
        repo_path TEXT NOT NULL,\
        task_dir TEXT NOT NULL\
    );\
    CREATE TABLE IF NOT EXISTS runs (\
        id TEXT PRIMARY KEY,\
        task_id TEXT NOT NULL REFERENCES tasks(id),\
        agent_id TEXT NOT NULL REFERENCES agents(id),\
        worktree TEXT NOT NULL,\
        branch TEXT NOT NULL,\
        slot INTEGER NULL,\
        state TEXT NOT NULL,\
        state_data TEXT NULL,\
        created_at TEXT NOT NULL,\
        started_at TEXT NULL,\
        ended_at TEXT NULL\
    );\
    CREATE INDEX IF NOT EXISTS idx_runs_state ON runs(state);\
    CREATE TABLE IF NOT EXISTS leases (\
        id TEXT PRIMARY KEY,\
        resource TEXT NOT NULL,\
        class TEXT NOT NULL,\
        holder_run_id TEXT NOT NULL REFERENCES runs(id),\
        token TEXT NOT NULL,\
        acquired_at TEXT NOT NULL,\
        ttl_secs INTEGER NOT NULL\
    );\
    CREATE TABLE IF NOT EXISTS experiments (\
        id TEXT PRIMARY KEY,\
        run_id TEXT NOT NULL REFERENCES runs(id),\
        command TEXT NOT NULL,\
        metrics TEXT NOT NULL,\
        correct INTEGER NULL,\
        duration_ms INTEGER NULL,\
        quarantined INTEGER NOT NULL DEFAULT 0\
    );\
    CREATE INDEX IF NOT EXISTS idx_experiments_run_id ON experiments(run_id);\
    CREATE TABLE IF NOT EXISTS grades (\
        id TEXT PRIMARY KEY,\
        run_id TEXT NOT NULL REFERENCES runs(id),\
        grader TEXT NOT NULL,\
        quality INTEGER NOT NULL,\
        approach INTEGER NOT NULL,\
        adherence INTEGER NOT NULL,\
        autonomy INTEGER NOT NULL,\
        honesty INTEGER NOT NULL,\
        rationale TEXT NOT NULL,\
        created_at TEXT NOT NULL\
    );\
    CREATE INDEX IF NOT EXISTS idx_grades_run_id ON grades(run_id);\
    CREATE TABLE IF NOT EXISTS run_events (\
        id INTEGER PRIMARY KEY AUTOINCREMENT,\
        run_id TEXT NOT NULL REFERENCES runs(id),\
        at TEXT NOT NULL,\
        kind TEXT NOT NULL,\
        detail TEXT NOT NULL\
    );\
    CREATE INDEX IF NOT EXISTS idx_run_events_run_id_id ON run_events(run_id, id);\
";

/// All schema migrations, applied in order.
///
/// Each entry is `(version_name, creation_sql)`. The SQL is executed
/// inside a transaction, and the version name is recorded in
/// `schema_version` to make re-running idempotent.
pub const MIGRATIONS: &[(&str, &str)] = &[("001_initial_schema", SCHEMA_SQL)];

/// Per-agent summary for the leaderboard.
#[derive(Debug, Clone, PartialEq)]
pub struct LeaderboardEntry {
    /// The agent this entry describes.
    pub agent_id: AgentId,
    /// Total number of runs for this agent.
    pub run_count: usize,
    /// Number of runs that reached `Succeeded`.
    pub success_count: usize,
    /// Mean rubric grade across all grades for this agent's runs, if any exist.
    pub mean_grade: Option<f64>,
}

/// A SQLite-backed store for all farmerbob state.
pub struct Store {
    conn: Connection,
}

impl Store {
    /// Opens (or creates) a SQLite database at the given path.
    ///
    /// Sets WAL journal mode, foreign key enforcement, busy timeout,
    /// and synchronous mode, then applies all pending migrations.
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        let conn = Connection::open(path)?;
        Self::set_pragmas(&conn)?;
        let mut store = Self { conn };
        store.apply_migrations()?;
        Ok(store)
    }

    /// Opens an in-memory store, for tests.
    ///
    /// Sets the same pragmas as [`Self::open`] and applies all migrations.
    pub fn open_in_memory() -> Result<Self, StoreError> {
        let conn = Connection::open_in_memory()?;
        Self::set_pragmas(&conn)?;
        let mut store = Self { conn };
        store.apply_migrations()?;
        Ok(store)
    }

    fn set_pragmas(conn: &Connection) -> Result<(), StoreError> {
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;\
             PRAGMA foreign_keys=ON;\
             PRAGMA busy_timeout=5000;\
             PRAGMA synchronous=NORMAL;",
        )?;
        Ok(())
    }

    fn apply_migrations(&mut self) -> Result<(), StoreError> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_version (\
                version TEXT PRIMARY KEY,\
                applied_at TEXT NOT NULL\
            );",
        )?;

        for (version, sql) in MIGRATIONS {
            let count: i64 = self
                .conn
                .query_row(
                    "SELECT COUNT(*) FROM schema_version WHERE version = ?1",
                    [version],
                    |row| row.get(0),
                )
                .map_err(StoreError::Database)?;

            if count > 0 {
                continue;
            }

            let tx = self.conn.transaction()?;
            tx.execute_batch(sql)?;
            tx.execute(
                "INSERT INTO schema_version (version, applied_at) VALUES (?1, ?2)",
                params![version, Utc::now().to_rfc3339()],
            )?;
            tx.commit()?;
        }

        Ok(())
    }

    /// Returns all applied migration version names.
    pub fn applied_migrations(&self) -> Result<Vec<String>, StoreError> {
        let mut stmt = self
            .conn
            .prepare("SELECT version FROM schema_version ORDER BY version")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        let mut versions = Vec::new();
        for row in rows {
            versions.push(row.map_err(StoreError::Database)?);
        }
        Ok(versions)
    }

    /// Inserts an agent.
    pub fn insert_agent(&self, agent: &Agent) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO agents (id, kind, model, provider) VALUES (?1, ?2, ?3, ?4)",
            params![
                agent.id.as_uuid().to_string(),
                agent_kind_to_str(&agent.kind),
                agent.model,
                agent.provider,
            ],
        )?;
        Ok(())
    }

    /// Gets an agent by ID, or `None` if not found.
    pub fn get_agent(&self, id: AgentId) -> Result<Option<Agent>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, kind, model, provider FROM agents WHERE id = ?1",
        )?;
        let mut rows = stmt.query_map([id.as_uuid().to_string()], |row| {
            map_agent_row(row).map_err(store_to_rusqlite)
        })?;
        match rows.next() {
            Some(Ok(agent)) => Ok(Some(agent)),
            Some(Err(e)) => Err(StoreError::Database(e)),
            None => Ok(None),
        }
    }

    /// Lists all agents.
    pub fn list_agents(&self) -> Result<Vec<Agent>, StoreError> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, kind, model, provider FROM agents ORDER BY id")?;
        let rows = stmt.query_map([], |row| map_agent_row(row).map_err(store_to_rusqlite))?;
        let mut agents = Vec::new();
        for row in rows {
            agents.push(row.map_err(StoreError::Database)?);
        }
        Ok(agents)
    }

    /// Inserts a task.
    pub fn insert_task(&self, task: &Task) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO tasks (id, name, repo_path, task_dir) VALUES (?1, ?2, ?3, ?4)",
            params![
                task.id.as_uuid().to_string(),
                task.name,
                task.repo_path.to_string_lossy(),
                task.task_dir.to_string_lossy(),
            ],
        )?;
        Ok(())
    }

    /// Gets a task by ID, or `None` if not found.
    pub fn get_task(&self, id: TaskId) -> Result<Option<Task>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, repo_path, task_dir FROM tasks WHERE id = ?1",
        )?;
        let mut rows = stmt.query_map([id.as_uuid().to_string()], |row| {
            map_task_row(row).map_err(store_to_rusqlite)
        })?;
        match rows.next() {
            Some(Ok(task)) => Ok(Some(task)),
            Some(Err(e)) => Err(StoreError::Database(e)),
            None => Ok(None),
        }
    }

    /// Lists all tasks.
    pub fn list_tasks(&self) -> Result<Vec<Task>, StoreError> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, name, repo_path, task_dir FROM tasks ORDER BY id")?;
        let rows = stmt.query_map([], |row| map_task_row(row).map_err(store_to_rusqlite))?;
        let mut tasks = Vec::new();
        for row in rows {
            tasks.push(row.map_err(StoreError::Database)?);
        }
        Ok(tasks)
    }

    /// Inserts a run.
    pub fn insert_run(&self, run: &Run) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO runs (id, task_id, agent_id, worktree, branch, slot, state, state_data, created_at, started_at, ended_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                run.id.as_uuid().to_string(),
                run.task_id.as_uuid().to_string(),
                run.agent_id.as_uuid().to_string(),
                run.worktree.to_string_lossy(),
                run.branch,
                run.slot_index.map(|v| v as i64),
                run_state_name(&run.state),
                run_state_data_json(&run.state),
                run.created_at.to_rfc3339(),
                run.started_at.as_ref().map(|dt| dt.to_rfc3339()),
                run.ended_at.as_ref().map(|dt| dt.to_rfc3339()),
            ],
        )?;
        Ok(())
    }

    /// Gets a run by ID, or `None` if not found.
    pub fn get_run(&self, id: RunId) -> Result<Option<Run>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, task_id, agent_id, worktree, branch, slot, state, state_data, created_at, started_at, ended_at FROM runs WHERE id = ?1",
        )?;
        let mut rows = stmt.query_map([id.as_uuid().to_string()], |row| {
            map_run_row(row).map_err(store_to_rusqlite)
        })?;
        match rows.next() {
            Some(Ok(run)) => Ok(Some(run)),
            Some(Err(e)) => Err(StoreError::Database(e)),
            None => Ok(None),
        }
    }

    /// Lists all runs.
    pub fn list_runs(&self) -> Result<Vec<Run>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, task_id, agent_id, worktree, branch, slot, state, state_data, created_at, started_at, ended_at FROM runs ORDER BY id",
        )?;
        let rows = stmt.query_map([], |row| map_run_row(row).map_err(store_to_rusqlite))?;
        let mut runs = Vec::new();
        for row in rows {
            runs.push(row.map_err(StoreError::Database)?);
        }
        Ok(runs)
    }

    /// Updates a run's state and appends an audit event in a single transaction.
    ///
    /// `state_data` is a JSON string for states carrying data (`BlockedOnQuota`,
    /// `Failed`), or `None` otherwise. `at` is the timestamp for this state change.
    pub fn update_run_state(
        &mut self,
        run_id: RunId,
        state: &RunState,
        state_data: Option<&str>,
        at: DateTime<Utc>,
    ) -> Result<(), StoreError> {
        let tx = self.conn.transaction()?;

        let state_name = run_state_name(state);

        let updated = tx.execute(
            "UPDATE runs SET \
                state = ?1, \
                state_data = ?2, \
                started_at = CASE WHEN started_at IS NULL AND (?1 = 'Starting' OR ?1 = 'Running') THEN ?4 ELSE started_at END, \
                ended_at = CASE WHEN ?1 IN ('Succeeded', 'Failed', 'Killed', 'Abandoned') THEN ?4 ELSE ended_at END \
             WHERE id = ?3",
            params![state_name, state_data, run_id.as_uuid().to_string(), at],
        )?;

        if updated == 0 {
            tx.rollback()?;
            return Err(StoreError::NotFound(format!("run {}", run_id)));
        }

        let detail = serde_json::json!({
            "state": state_name,
            "state_data": state_data,
            "at": at.to_rfc3339(),
        })
        .to_string();

        tx.execute(
            "INSERT INTO run_events (run_id, at, kind, detail) VALUES (?1, ?2, ?3, ?4)",
            params![
                run_id.as_uuid().to_string(),
                at.to_rfc3339(),
                "state_change",
                detail,
            ],
        )?;

        tx.commit()?;
        Ok(())
    }

    /// Returns all runs in the given state.
    pub fn runs_by_state(&self, state: &RunState) -> Result<Vec<Run>, StoreError> {
        let state_name = run_state_name(state);
        let mut stmt = self.conn.prepare(
            "SELECT id, task_id, agent_id, worktree, branch, slot, state, state_data, created_at, started_at, ended_at FROM runs WHERE state = ?1 ORDER BY id",
        )?;
        let rows = stmt.query_map([state_name], |row| map_run_row(row).map_err(store_to_rusqlite))?;
        let mut runs = Vec::new();
        for row in rows {
            runs.push(row.map_err(StoreError::Database)?);
        }
        Ok(runs)
    }

    /// Returns all runs that are not in a terminal state.
    pub fn active_runs(&self) -> Result<Vec<Run>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, task_id, agent_id, worktree, branch, slot, state, state_data, created_at, started_at, ended_at FROM runs WHERE state NOT IN ('Succeeded', 'Failed', 'Killed', 'Abandoned') ORDER BY id",
        )?;
        let rows = stmt.query_map([], |row| map_run_row(row).map_err(store_to_rusqlite))?;
        let mut runs = Vec::new();
        for row in rows {
            runs.push(row.map_err(StoreError::Database)?);
        }
        Ok(runs)
    }

    /// Records an experiment.
    pub fn record_experiment(&self, experiment: &Experiment) -> Result<(), StoreError> {
        let metrics_json = serde_json::to_string(&experiment.metrics)
            .map_err(|e| StoreError::InvalidStateData(e.to_string()))?;
        self.conn.execute(
            "INSERT INTO experiments (id, run_id, command, metrics, correct, duration_ms, quarantined) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                experiment.id.as_uuid().to_string(),
                experiment.run_id.as_uuid().to_string(),
                experiment.command,
                metrics_json,
                experiment.correct,
                experiment.duration_ms,
                experiment.quarantined,
            ],
        )?;
        Ok(())
    }

    /// Records a grade.
    pub fn record_grade(&self, grade: &Grade) -> Result<(), StoreError> {
        let id = RunId::new().as_uuid().to_string();
        self.conn.execute(
            "INSERT INTO grades (id, run_id, grader, quality, approach, adherence, autonomy, honesty, rationale, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                id,
                grade.run_id.as_uuid().to_string(),
                grade.grader,
                grade.scores.quality as i64,
                grade.scores.approach as i64,
                grade.scores.adherence as i64,
                grade.scores.autonomy as i64,
                grade.scores.honesty as i64,
                grade.rationale,
                grade.created_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    /// Returns a per-agent leaderboard summary.
    pub fn leaderboard(&self) -> Result<Vec<LeaderboardEntry>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT r.agent_id, COUNT(*) as run_count, SUM(CASE WHEN r.state = 'Succeeded' THEN 1 ELSE 0 END) as success_count FROM runs r GROUP BY r.agent_id",
        )?;
        let rows = stmt.query_map([], |row| {
            let agent_id_str: String = row.get("agent_id")?;
            let agent_id = parse_agent_id(agent_id_str.as_str()).map_err(store_to_rusqlite)?;
            let run_count: i64 = row.get("run_count")?;
            let success_count: i64 = row.get("success_count")?;
            Ok((agent_id, run_count as usize, success_count as usize))
        })?;

        let mut entries: Vec<LeaderboardEntry> = Vec::new();
        for row in rows {
            let (agent_id, run_count, success_count) = row.map_err(StoreError::Database)?;
            entries.push(LeaderboardEntry {
                agent_id,
                run_count,
                success_count,
                mean_grade: None,
            });
        }

        let mut stmt2 = self.conn.prepare(
            "SELECT r.agent_id, AVG((g.quality + g.approach + g.adherence + g.autonomy + g.honesty) / 5.0) as mean_grade FROM grades g JOIN runs r ON r.id = g.run_id GROUP BY r.agent_id",
        )?;
        let grade_rows = stmt2.query_map([], |row| {
            let agent_id_str: String = row.get("agent_id")?;
            let mean_grade: f64 = row.get("mean_grade")?;
            Ok((agent_id_str, mean_grade))
        })?;

        for row in grade_rows {
            let (agent_id_str, mean_grade) = row.map_err(StoreError::Database)?;
            if let Some(entry) = entries.iter_mut().find(|e| {
                e.agent_id.as_uuid().to_string() == agent_id_str
            }) {
                entry.mean_grade = Some(mean_grade);
            }
        }

        Ok(entries)
    }
}

fn agent_kind_to_str(kind: &AgentKind) -> &'static str {
    match kind {
        AgentKind::Claude => "Claude",
        AgentKind::Codex => "Codex",
        AgentKind::Gemini => "Gemini",
        AgentKind::Glm => "Glm",
        AgentKind::Ifm => "Ifm",
        AgentKind::OpenRouter => "OpenRouter",
    }
}

fn run_state_data_json(state: &RunState) -> Option<String> {
    match state {
        RunState::BlockedOnQuota { reset_at } => Some(
            serde_json::json!({ "reset_at": reset_at.to_rfc3339() }).to_string(),
        ),
        RunState::Failed { reason } => {
            Some(serde_json::json!({ "reason": reason }).to_string())
        }
        _ => None,
    }
}

fn map_agent_row(row: &rusqlite::Row) -> Result<Agent, StoreError> {
    let id: AgentId = serde_json::from_str(&format!(
        "\"{}\"",
        row.get::<_, String>("id")?.as_str()
    ))
    .map_err(|e| StoreError::InvalidStateData(e.to_string()))?;
    Ok(Agent {
        id,
        kind: agent_kind_from_str(row.get::<_, String>("kind")?.as_str())?,
        model: row.get::<_, String>("model")?,
        provider: row.get::<_, String>("provider")?,
    })
}

fn map_task_row(row: &rusqlite::Row) -> Result<Task, StoreError> {
    let id: TaskId = serde_json::from_str(&format!(
        "\"{}\"",
        row.get::<_, String>("id")?.as_str()
    ))
    .map_err(|e| StoreError::InvalidStateData(e.to_string()))?;
    Ok(Task {
        id,
        name: row.get::<_, String>("name")?,
        repo_path: std::path::PathBuf::from(row.get::<_, String>("repo_path")?),
        task_dir: std::path::PathBuf::from(row.get::<_, String>("task_dir")?),
    })
}

fn map_run_row(row: &rusqlite::Row) -> Result<Run, StoreError> {
    let id: RunId = serde_json::from_str(&format!(
        "\"{}\"",
        row.get::<_, String>("id")?.as_str()
    ))
    .map_err(|e| StoreError::InvalidStateData(e.to_string()))?;
    let task_id: TaskId = serde_json::from_str(&format!(
        "\"{}\"",
        row.get::<_, String>("task_id")?.as_str()
    ))
    .map_err(|e| StoreError::InvalidStateData(e.to_string()))?;
    let agent_id: AgentId = serde_json::from_str(&format!(
        "\"{}\"",
        row.get::<_, String>("agent_id")?.as_str()
    ))
    .map_err(|e| StoreError::InvalidStateData(e.to_string()))?;
    let state_name = row.get::<_, String>("state")?;
    let state_data: Option<String> = row.get("state_data")?;
    let state = run_state_from(state_name.as_str(), state_data.as_deref())?;
    let created_at = DateTime::parse_from_rfc3339(row.get::<_, String>("created_at")?.as_str())
        .map_err(|e| StoreError::InvalidStateData(e.to_string()))?
        .with_timezone(&Utc);
    let started_at: Option<DateTime<Utc>> = row.get::<_, Option<String>>("started_at")?.map(|s| {
        DateTime::parse_from_rfc3339(s.as_str())
            .map_err(|e| StoreError::InvalidStateData(e.to_string()))
            .map(|dt| dt.with_timezone(&Utc))
    }).transpose()?;
    let ended_at: Option<DateTime<Utc>> = row.get::<_, Option<String>>("ended_at")?.map(|s| {
        DateTime::parse_from_rfc3339(s.as_str())
            .map_err(|e| StoreError::InvalidStateData(e.to_string()))
            .map(|dt| dt.with_timezone(&Utc))
    }).transpose()?;

    Ok(Run {
        id,
        task_id,
        agent_id,
        worktree: std::path::PathBuf::from(row.get::<_, String>("worktree")?),
        branch: row.get::<_, String>("branch")?,
        slot_index: row.get::<_, Option<i64>>("slot")?.map(|v| v as u32),
        state,
        created_at,
        started_at,
        ended_at,
    })
}

fn run_state_from(name: &str, data: Option<&str>) -> Result<RunState, StoreError> {
    match name {
        "Queued" => Ok(RunState::Queued),
        "Starting" => Ok(RunState::Starting),
        "Running" => Ok(RunState::Running),
        "BlockedOnLease" => Ok(RunState::BlockedOnLease),
        "BlockedOnQuota" => {
            let data = data.ok_or_else(|| {
                StoreError::InvalidStateData("BlockedOnQuota requires state_data".into())
            })?;
            let value: Value = serde_json::from_str(data)
                .map_err(|e| StoreError::InvalidStateData(e.to_string()))?;
            let reset_at_str = value["reset_at"].as_str().ok_or_else(|| {
                StoreError::InvalidStateData("BlockedOnQuota missing reset_at".to_string())
            })?;
            let reset_at = DateTime::parse_from_rfc3339(reset_at_str)
                .map_err(|e| StoreError::InvalidStateData(e.to_string()))?
                .with_timezone(&Utc);
            Ok(RunState::BlockedOnQuota { reset_at })
        }
        "Succeeded" => Ok(RunState::Succeeded),
        "Failed" => {
            let data = data.ok_or_else(|| {
                StoreError::InvalidStateData("Failed requires state_data".into())
            })?;
            let value: Value = serde_json::from_str(data)
                .map_err(|e| StoreError::InvalidStateData(e.to_string()))?;
            let reason = value["reason"].as_str().ok_or_else(|| {
                StoreError::InvalidStateData("Failed missing reason".to_string())
            })?;
            Ok(RunState::Failed { reason: reason.to_string() })
        }
        "Killed" => Ok(RunState::Killed),
        "Abandoned" => Ok(RunState::Abandoned),
        _ => Err(StoreError::UnknownState(name.to_string())),
    }
}

fn run_state_name(state: &RunState) -> &'static str {
    match state {
        RunState::Queued => "Queued",
        RunState::Starting => "Starting",
        RunState::Running => "Running",
        RunState::BlockedOnLease => "BlockedOnLease",
        RunState::BlockedOnQuota { .. } => "BlockedOnQuota",
        RunState::Succeeded => "Succeeded",
        RunState::Failed { .. } => "Failed",
        RunState::Killed => "Killed",
        RunState::Abandoned => "Abandoned",
    }
}

fn store_to_rusqlite(e: StoreError) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(Box::new(e))
}

fn agent_kind_from_str(s: &str) -> Result<AgentKind, StoreError> {
    match s {
        "Claude" => Ok(AgentKind::Claude),
        "Codex" => Ok(AgentKind::Codex),
        "Gemini" => Ok(AgentKind::Gemini),
        "Glm" => Ok(AgentKind::Glm),
        "Ifm" => Ok(AgentKind::Ifm),
        "OpenRouter" => Ok(AgentKind::OpenRouter),
        _ => Err(StoreError::UnknownState(s.to_string())),
    }
}

/// Parse a UUID string into a [`AgentId`] using JSON deserialization.
fn parse_agent_id(s: &str) -> Result<AgentId, StoreError> {
    serde_json::from_str(&format!("\"{}\"", s))
        .map_err(|e| StoreError::InvalidStateData(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn ts(secs: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(1_700_000_000 + secs, 0).unwrap()
    }

    fn make_agent() -> Agent {
        Agent {
            id: AgentId::new(),
            kind: AgentKind::Claude,
            model: "claude-3.5-sonnet".into(),
            provider: "openrouter".into(),
        }
    }

    fn make_task() -> Task {
        Task {
            id: TaskId::new(),
            name: "test-task".into(),
            repo_path: std::path::PathBuf::from("/srv/repo"),
            task_dir: std::path::PathBuf::from("/srv/repo/task"),
        }
    }

    fn make_run(agent: AgentId, task: TaskId) -> Run {
        Run {
            id: RunId::new(),
            task_id: task,
            agent_id: agent,
            worktree: std::path::PathBuf::from("/srv/worktrees/r1"),
            branch: "fb/test".into(),
            slot_index: None,
            state: RunState::Queued,
            created_at: ts(0),
            started_at: None,
            ended_at: None,
        }
    }

    #[test]
    fn migrations_are_idempotent() {
        let store = Store::open_in_memory().unwrap();
        let first = store.applied_migrations().unwrap();
        assert!(!first.is_empty(), "migrations should be applied");

        let store2 = Store::open_in_memory().unwrap();
        let second = store2.applied_migrations().unwrap();
        assert_eq!(first, second, "migrations should be same after re-apply");

        assert_eq!(first.len(), 1, "exactly one migration");
    }

    #[test]
    fn run_round_trips() {
        let store = Store::open_in_memory().unwrap();
        let agent = make_agent();
        let task = make_task();
        store.insert_agent(&agent).unwrap();
        store.insert_task(&task).unwrap();

        let run = make_run(agent.id, task.id);
        store.insert_run(&run).unwrap();

        let fetched = store.get_run(run.id).unwrap();
        assert_eq!(fetched, Some(run));
    }

    #[test]
    fn update_run_state_writes_both_run_and_event() {
        let mut store = Store::open_in_memory().unwrap();
        let agent = make_agent();
        let task = make_task();
        store.insert_agent(&agent).unwrap();
        store.insert_task(&task).unwrap();

        let run = make_run(agent.id, task.id);
        store.insert_run(&run).unwrap();

        let new_state = RunState::Running;
        let at = ts(10);
        store
            .update_run_state(run.id, &new_state, None, at)
            .unwrap();

        let fetched = store.get_run(run.id).unwrap();
        assert_eq!(fetched.unwrap().state, RunState::Running);

        let events_count: i64 = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM run_events WHERE run_id = ?1",
                [run.id.as_uuid().to_string()],
                |row| row.get(0),
            )
            .unwrap();
        assert!(events_count >= 1, "should have at least one event");
    }

    #[test]
    fn foreign_key_violation_rejected() {
        let store = Store::open_in_memory().unwrap();
        let agent = make_agent();
        store.insert_agent(&agent).unwrap();

        let bogus_task_id = TaskId::new();
        let run = Run {
            task_id: bogus_task_id,
            ..make_run(agent.id, bogus_task_id)
        };
        let result = store.insert_run(&run);
        assert!(result.is_err(), "inserting run with nonexistent task should fail");
    }

    #[test]
    fn runs_by_state_and_active_runs() {
        let store = Store::open_in_memory().unwrap();
        let agent = make_agent();
        let task = make_task();
        store.insert_agent(&agent).unwrap();
        store.insert_task(&task).unwrap();

        let r1 = make_run(agent.id, task.id);
        store.insert_run(&r1).unwrap();

        let r2 = Run {
            state: RunState::Succeeded,
            ..make_run(agent.id, task.id)
        };
        store.insert_run(&r2).unwrap();

        let queued = store.runs_by_state(&RunState::Queued).unwrap();
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].id, r1.id);

        let active = store.active_runs().unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].id, r1.id);
    }

    #[test]
    fn experiment_and_grade_recorded() {
        let store = Store::open_in_memory().unwrap();
        let agent = make_agent();
        let task = make_task();
        store.insert_agent(&agent).unwrap();
        store.insert_task(&task).unwrap();

        let run = make_run(agent.id, task.id);
        store.insert_run(&run).unwrap();

        let experiment = Experiment {
            id: super::core::ids::ExperimentId::new(),
            run_id: run.id,
            command: "python -m bench".into(),
            metrics: serde_json::json!({"accuracy": 0.9}),
            correct: Some(true),
            duration_ms: Some(100),
            quarantined: false,
        };
        store.record_experiment(&experiment).unwrap();

        let grade = Grade {
            run_id: run.id,
            grader: "judge-v1".into(),
            scores: core::grade::RubricScores::try_new(4, 5, 3, 4, 5).unwrap(),
            rationale: "Good job".into(),
            created_at: ts(5),
        };
        store.record_grade(&grade).unwrap();

        let lb = store.leaderboard().unwrap();
        assert!(!lb.is_empty());
        let entry = lb.iter().find(|e| e.agent_id == agent.id).unwrap();
        assert_eq!(entry.run_count, 1);
        assert_eq!(entry.success_count, 0);
        assert!(entry.mean_grade.is_some());
    }
}


