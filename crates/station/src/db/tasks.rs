//! Local product tasks. A task owns its review decision; a runtime turn does not.
use super::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Open,
    Review,
    Completed,
    Cancelled,
}
impl TaskState {
    fn code(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Review => "review",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
        }
    }
    fn parse(value: String) -> rusqlite::Result<Self> {
        match value.as_str() {
            "open" => Ok(Self::Open),
            "review" => Ok(Self::Review),
            "completed" => Ok(Self::Completed),
            "cancelled" => Ok(Self::Cancelled),
            _ => Err(rusqlite::Error::InvalidQuery),
        }
    }
    pub fn is_closed(self) -> bool {
        matches!(self, Self::Completed | Self::Cancelled)
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ProductTask {
    pub task_id: String,
    pub session_id: Option<String>,
    pub conversation_id: String,
    pub title: String,
    pub goal: String,
    pub workspace: String,
    pub state: TaskState,
    pub revision: i64,
    pub result_message_id: Option<String>,
    pub result_text: Option<String>,
    pub last_run_status: Option<String>,
    pub run_count: i64,
    pub created_at: String,
    pub updated_at: String,
    pub mesh: Option<Value>,
}

#[derive(Clone, Debug, Serialize)]
pub struct TaskRun {
    pub run_id: String,
    pub agent_session_id: String,
    pub turn_id: String,
    pub status: String,
    pub started_at_ms: i64,
    pub finished_at_ms: Option<i64>,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskAction {
    Accept,
    Reopen,
    Cancel,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TaskTransitionError {
    NotFound,
    RevisionConflict,
    InvalidTransition,
    RunActive,
}
impl std::fmt::Display for TaskTransitionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NotFound => "task_not_found",
            Self::RevisionConflict => "task_revision_conflict",
            Self::InvalidTransition => "invalid_task_transition",
            Self::RunActive => "task_run_active",
        })
    }
}
impl std::error::Error for TaskTransitionError {}

const TASK_SELECT: &str = r#"
SELECT t.task_id, s.id, s.channel_id, t.title, s.workspace_path, t.state,
       t.revision, m.message_id, m.text,
       COALESCE((SELECT remote_status FROM mesh_links WHERE local_task_id=t.task_id AND role='owner'),
         (SELECT r.status FROM task_runs r WHERE r.task_id = t.task_id AND r.agent_session_id = s.id ORDER BY r.rowid DESC LIMIT 1)),
       (SELECT COUNT(*) FROM task_runs r WHERE r.task_id = t.task_id) +
       COALESCE((SELECT SUM(MAX(0, c.run_count - (SELECT COUNT(*) FROM task_runs r WHERE r.task_id=c.task_id AND r.agent_session_id=c.agent_session_id))) FROM task_event_cursors c WHERE c.task_id=t.task_id AND c.run_count IS NOT NULL),0),
       t.created_at, t.updated_at, t.goal,
       (SELECT json_object('role',role,'assignment_id',assignment_id,'state',state,'error',error,
          'owner_origin',json_extract(assignment_json,'$.owner_origin'),
          'executor_origin',json_extract(assignment_json,'$.executor_origin')) FROM mesh_links WHERE local_task_id=t.task_id)
FROM product_tasks t JOIN sessions s ON s.key = t.session_key
LEFT JOIN visible_message_content m ON m.sequence = t.result_sequence
"#;
fn map_task(row: &rusqlite::Row<'_>) -> rusqlite::Result<ProductTask> {
    Ok(ProductTask {
        task_id: row.get(0)?,
        session_id: row.get(1)?,
        conversation_id: row.get(2)?,
        title: row.get(3)?,
        goal: row.get(13)?,
        workspace: row.get(4)?,
        state: TaskState::parse(row.get(5)?)?,
        revision: row.get(6)?,
        result_message_id: row.get(7)?,
        result_text: row.get(8)?,
        last_run_status: row.get(9)?,
        run_count: row.get(10)?,
        created_at: row.get(11)?,
        updated_at: row.get(12)?,
        mesh: row
            .get::<_, Option<String>>(14)?
            .map(|s| serde_json::from_str(&s))
            .transpose()
            .map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    14,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })?,
    })
}

pub(super) fn initialize(conn: &Connection) -> Result<()> {
    conn.execute_batch(r#"
        CREATE TABLE IF NOT EXISTS product_tasks (
            task_id TEXT PRIMARY KEY,
            session_key TEXT NOT NULL UNIQUE REFERENCES sessions(key) ON DELETE CASCADE,
            title TEXT NOT NULL DEFAULT '',
            goal TEXT NOT NULL DEFAULT '',
            state TEXT NOT NULL DEFAULT 'open' CHECK(state IN ('open','review','completed','cancelled')),
            revision INTEGER NOT NULL DEFAULT 0,
            result_sequence INTEGER REFERENCES visible_messages(sequence) ON DELETE SET NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS task_runs (
            run_id TEXT PRIMARY KEY,
            task_id TEXT NOT NULL REFERENCES product_tasks(task_id) ON DELETE CASCADE,
            agent_session_id TEXT NOT NULL,
            turn_id TEXT NOT NULL,
            status TEXT NOT NULL CHECK(status IN ('running','finished','failed','cancelled')),
            started_at_ms INTEGER NOT NULL,
            finished_at_ms INTEGER,
            UNIQUE(agent_session_id, turn_id)
        );
        CREATE INDEX IF NOT EXISTS task_runs_task ON task_runs(task_id);
        CREATE TABLE IF NOT EXISTS task_event_cursors (
            task_id TEXT NOT NULL REFERENCES product_tasks(task_id) ON DELETE CASCADE,
            agent_session_id TEXT NOT NULL,
            event_id TEXT NOT NULL,
            PRIMARY KEY(task_id, agent_session_id)
        );
        CREATE TABLE IF NOT EXISTS task_decisions (
            task_id TEXT NOT NULL REFERENCES product_tasks(task_id) ON DELETE CASCADE,
            revision INTEGER NOT NULL,
            action TEXT NOT NULL,
            result_message_id TEXT,
            created_at TEXT NOT NULL,
            PRIMARY KEY(task_id, revision)
        );
    "#)?;
    let has_run_count = conn
        .prepare("PRAGMA table_info(task_event_cursors)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?
        .iter()
        .any(|name| name == "run_count");
    if !has_run_count {
        conn.execute_batch("ALTER TABLE task_event_cursors ADD COLUMN run_count INTEGER")?;
    }
    // Additive, idempotent migration: never infer acceptance from old idle sessions.
    let keys = conn
        .prepare("SELECT key FROM sessions WHERE platform = 'local_gui'")?
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for key in keys {
        ensure_task(conn, &key)?;
    }
    Ok(())
}

pub(super) fn ensure_task(conn: &Connection, session_key: &str) -> Result<()> {
    let inserted = conn.execute(
        r#"
        INSERT OR IGNORE INTO product_tasks(task_id, session_key, goal, created_at, updated_at)
        SELECT ?1, key, COALESCE((SELECT text FROM visible_message_content
          WHERE session_key = sessions.key AND role = 'user' ORDER BY sequence LIMIT 1), ''),
          created_at, updated_at FROM sessions WHERE key = ?2 AND platform = 'local_gui'
          AND COALESCE(channel_type,'') NOT IN ('channel','agent_control')
          AND NOT EXISTS (SELECT 1 FROM product_tasks WHERE session_key = sessions.key)
          AND NOT EXISTS (SELECT 1 FROM node_agents a WHERE a.session_key=sessions.key AND json_extract(a.value,'$.role')='leader')
    "#,
        params![format!("task-{}", ulid::Ulid::new()), session_key],
    )?;
    if inserted > 0 {
        let goal: String = conn.query_row(
            "SELECT goal FROM product_tasks WHERE session_key = ?1",
            [session_key],
            |r| r.get(0),
        )?;
        conn.execute(
            "UPDATE product_tasks SET title = ?1 WHERE session_key = ?2",
            params![title_from_goal(&goal), session_key],
        )?;
    }
    Ok(())
}

pub(super) fn transition_task(
    conn: &Connection,
    task_id: &str,
    expected_revision: i64,
    action: TaskAction,
) -> Result<ProductTask> {
    let task = conn
        .query_row(
            &format!("{TASK_SELECT} WHERE t.task_id = ?1"),
            [task_id],
            map_task,
        )
        .optional()?
        .ok_or(TaskTransitionError::NotFound)?;
    if task.revision != expected_revision {
        return Err(TaskTransitionError::RevisionConflict.into());
    }
    if let Some(mesh) = task.mesh.as_ref() {
        if mesh["role"] != "owner" {
            anyhow::bail!("mesh_task_owner_required");
        }
        if matches!(action, TaskAction::Reopen) {
            anyhow::bail!("mesh_create_new_task_for_next_run");
        }
        if !matches!(
            task.last_run_status.as_deref(),
            Some("finished" | "failed" | "cancelled")
        ) {
            return Err(TaskTransitionError::RunActive.into());
        }
    }
    let active: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM task_runs WHERE task_id = ?1 AND agent_session_id = ?2 AND status = 'running')", params![task_id, task.session_id], |r| r.get(0))?;
    if active {
        return Err(TaskTransitionError::RunActive.into());
    }
    let (next, label) = match (action, task.state) {
        (TaskAction::Accept, TaskState::Review) if task.result_message_id.is_some() => {
            (TaskState::Completed, "accept")
        }
        (TaskAction::Reopen, TaskState::Review | TaskState::Completed | TaskState::Cancelled) => {
            (TaskState::Open, "reopen")
        }
        (TaskAction::Cancel, TaskState::Open | TaskState::Review) => {
            (TaskState::Cancelled, "cancel")
        }
        _ => return Err(TaskTransitionError::InvalidTransition.into()),
    };
    let now = now_rfc3339();
    conn.execute("UPDATE product_tasks SET state = ?1, revision = revision + 1, updated_at = ?2, result_sequence = CASE WHEN ?1 = 'open' THEN NULL ELSE result_sequence END WHERE task_id = ?3", params![next.code(), now, task_id])?;
    conn.execute("INSERT INTO task_decisions(task_id, revision, action, result_message_id, created_at) VALUES (?1, ?2, ?3, ?4, ?5)", params![task_id, task.revision + 1, label, task.result_message_id, now])?;
    super::mesh::record_decision(
        conn,
        task_id,
        next.code(),
        task.revision + 1,
        task.result_message_id.as_deref(),
    )?;
    let updated = conn.query_row(
        &format!("{TASK_SELECT} WHERE t.task_id = ?1"),
        [task_id],
        map_task,
    )?;
    Ok(updated)
}

fn title_from_goal(goal: &str) -> String {
    let visible = zork_client_types::comments::display_text(goal);
    visible
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(160)
        .collect()
}

/// Compatibility entry points record the same channel facts. Message kinds
/// never create review, completion or cancellation decisions.
pub(super) fn record_message(
    conn: &Connection,
    message: &VisibleMessageRow,
) -> Result<Vec<super::chats::Topic>> {
    super::chats::record(conn, message, None, None, &[], true)
}

impl GatewayDb {
    /// Rebuild the actionable queue from durable task/run state, including detached tasks.
    pub fn list_inbox_tasks(&self) -> Result<Vec<ProductTask>> {
        Ok(self
            .list_product_task_summaries()?
            .into_iter()
            .filter(|task| {
                !task.state.is_closed()
                    && task.last_run_status.as_deref() != Some("running")
                    && (task.state == TaskState::Review
                        || task
                            .mesh
                            .as_ref()
                            .is_some_and(|mesh| mesh["state"] == "needs_attention")
                        || task.session_id.is_none()
                        || matches!(
                            task.last_run_status.as_deref(),
                            Some("failed" | "cancelled")
                        ))
            })
            .collect())
    }

    pub fn list_product_tasks(&self) -> Result<Vec<ProductTask>> {
        let conn = self.conn.lock().expect("db mutex");
        let mut query = conn.prepare(&format!(
            "{TASK_SELECT} ORDER BY t.updated_at DESC, t.task_id DESC"
        ))?;
        let rows = query
            .query_map([], map_task)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// The frequently polled GUI list must not load full prompts or result bodies.
    pub fn list_product_task_summaries(&self) -> Result<Vec<ProductTask>> {
        let conn = self.conn.lock().expect("db mutex");
        let select = TASK_SELECT
            .replace("m.text", "NULL")
            .replace("t.goal", "''");
        let mut query = conn.prepare(&format!(
            "{select} ORDER BY t.updated_at DESC, t.task_id DESC"
        ))?;
        let rows = query
            .query_map([], map_task)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn product_task(&self, task_id: &str) -> Result<Option<ProductTask>> {
        let conn = self.conn.lock().expect("db mutex");
        Ok(conn
            .query_row(
                &format!("{TASK_SELECT} WHERE t.task_id = ?1"),
                [task_id],
                map_task,
            )
            .optional()?)
    }

    pub fn product_task_for_session(&self, session_key: &str) -> Result<Option<ProductTask>> {
        let conn = self.conn.lock().expect("db mutex");
        Ok(conn
            .query_row(
                &format!("{TASK_SELECT} WHERE t.session_key = ?1"),
                [session_key],
                map_task,
            )
            .optional()?)
    }

    pub fn task_runs(&self, task_id: &str) -> Result<Vec<TaskRun>> {
        let conn = self.conn.lock().expect("db mutex");
        let mut query = conn.prepare("SELECT run_id, agent_session_id, turn_id, status, started_at_ms, finished_at_ms FROM task_runs WHERE task_id = ?1 ORDER BY rowid DESC LIMIT 100")?;
        let rows = query
            .query_map([task_id], |r| {
                Ok(TaskRun {
                    run_id: r.get(0)?,
                    agent_session_id: r.get(1)?,
                    turn_id: r.get(2)?,
                    status: r.get(3)?,
                    started_at_ms: r.get(4)?,
                    finished_at_ms: r.get(5)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn transition_task(
        &self,
        task_id: &str,
        expected_revision: i64,
        action: TaskAction,
    ) -> Result<ProductTask> {
        let mut conn = self.conn.lock().expect("db mutex");
        let tx = conn.transaction()?;
        let updated = transition_task(&tx, task_id, expected_revision, action)?;
        tx.commit()?;
        Ok(updated)
    }

    /// Durable Agent events are replayable; the local cursor and run changes commit together.
    /// Event IDs are fixed-width, per-session monotonic IDs from the Agent API.
    pub fn project_task_run(
        &self,
        session_key: &str,
        agent_session_id: &str,
        event_id: &str,
        event: &Value,
    ) -> Result<()> {
        if !matches!(
            event.get("kind").and_then(Value::as_str),
            Some("turn_started" | "turn_finished")
        ) {
            return Ok(());
        }
        let mut conn = self.conn.lock().expect("db mutex");
        let tx = conn.transaction()?;
        let task_id: Option<String> = tx
            .query_row(
                "SELECT task_id FROM product_tasks WHERE session_key = ?1",
                [session_key],
                |r| r.get(0),
            )
            .optional()?;
        let Some(task_id) = task_id else {
            return Ok(());
        };
        let cursor: Option<String> = tx.query_row("SELECT event_id FROM task_event_cursors WHERE task_id = ?1 AND agent_session_id = ?2", params![task_id, agent_session_id], |r| r.get(0)).optional()?;
        if cursor.as_deref().is_some_and(|cursor| event_id <= cursor) {
            return Ok(());
        }
        match event.get("kind").and_then(Value::as_str) {
            Some("turn_started") => {
                let turn_id = event
                    .get("turn_id")
                    .and_then(Value::as_str)
                    .context("turn_started lacks turn_id")?;
                let started = event
                    .get("started_at_ms")
                    .and_then(Value::as_i64)
                    .context("turn_started lacks timestamp")?;
                tx.execute("INSERT OR IGNORE INTO task_runs(run_id, task_id, agent_session_id, turn_id, status, started_at_ms) VALUES (?1, ?2, ?3, ?4, 'running', ?5)", params![format!("run-{}", ulid::Ulid::new()), task_id, agent_session_id, turn_id, started])?;
            }
            Some("turn_finished") => {
                let turn_id = event
                    .get("turn_id")
                    .and_then(Value::as_str)
                    .context("turn_finished lacks turn_id")?;
                let status = event
                    .get("outcome")
                    .and_then(Value::as_str)
                    .context("turn_finished lacks outcome")?;
                let finished = event
                    .get("finished_at_ms")
                    .and_then(Value::as_i64)
                    .context("turn_finished lacks timestamp")?;
                if !matches!(status, "finished" | "failed" | "cancelled") {
                    anyhow::bail!("invalid task run outcome");
                }
                tx.execute("UPDATE task_runs SET status = ?1, finished_at_ms = ?2 WHERE task_id = ?3 AND agent_session_id = ?4 AND turn_id = ?5", params![status, finished, task_id, agent_session_id, turn_id])?;
            }
            _ => {}
        }
        tx.execute("INSERT INTO task_event_cursors(task_id, agent_session_id, event_id) VALUES (?1, ?2, ?3) ON CONFLICT(task_id, agent_session_id) DO UPDATE SET event_id = excluded.event_id", params![task_id, agent_session_id, event_id])?;
        tx.commit()?;
        Ok(())
    }

    /// Product-task counters consume the authoritative session snapshot; they
    /// must not replay every old turn to reconstruct an aggregate after reconnect.
    pub fn project_task_snapshot(
        &self,
        session_key: &str,
        snapshot: &zork_agent_api::SessionSnapshot,
    ) -> Result<()> {
        let Some(cursor) = snapshot.cursor.as_deref() else {
            return Ok(());
        };
        let mut conn = self.conn.lock().expect("db mutex");
        let tx = conn.transaction()?;
        let task: Option<String> = tx
            .query_row(
                "SELECT task_id FROM product_tasks WHERE session_key=?1",
                [session_key],
                |row| row.get(0),
            )
            .optional()?;
        let Some(task) = task else {
            return Ok(());
        };
        let previous: Option<String> = tx
            .query_row(
                "SELECT event_id FROM task_event_cursors WHERE task_id=?1 AND agent_session_id=?2",
                params![task, snapshot.session_id],
                |row| row.get(0),
            )
            .optional()?;
        if previous
            .as_deref()
            .is_some_and(|previous| previous > cursor)
        {
            return Ok(());
        }
        for run in snapshot
            .aggregates
            .last_run
            .iter()
            .chain(snapshot.execution.active_turn.iter())
        {
            let status = run.outcome.as_deref().unwrap_or("running");
            anyhow::ensure!(
                matches!(status, "running" | "finished" | "failed" | "cancelled"),
                "invalid snapshot run status"
            );
            tx.execute("INSERT INTO task_runs(run_id,task_id,agent_session_id,turn_id,status,started_at_ms,finished_at_ms) VALUES(?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(agent_session_id,turn_id) DO UPDATE SET status=excluded.status,started_at_ms=excluded.started_at_ms,finished_at_ms=excluded.finished_at_ms WHERE status IS NOT excluded.status OR started_at_ms IS NOT excluded.started_at_ms OR finished_at_ms IS NOT excluded.finished_at_ms",
                params![format!("run-{}",ulid::Ulid::new()), task, snapshot.session_id, run.turn_id, status, run.started_at_ms, run.finished_at_ms])?;
        }
        let count = snapshot
            .aggregates
            .complete
            .then_some(snapshot.aggregates.run_count.min(i64::MAX as u64) as i64);
        tx.execute("INSERT INTO task_event_cursors(task_id,agent_session_id,event_id,run_count) VALUES(?1,?2,?3,?4) ON CONFLICT(task_id,agent_session_id) DO UPDATE SET event_id=excluded.event_id,run_count=COALESCE(excluded.run_count,run_count) WHERE event_id IS NOT excluded.event_id OR (excluded.run_count IS NOT NULL AND run_count IS NOT excluded.run_count)", params![task,snapshot.session_id,cursor,count])?;
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (tempfile::TempDir, GatewayDb, SessionRow) {
        let dir = tempfile::tempdir().unwrap();
        let db = GatewayDb::open(dir.path(), &dir.path().join("workspaces")).unwrap();
        let session = db
            .create_session_at_workspace(
                EnsureSession {
                    connection_id: "local_gui",
                    platform: "local_gui",
                    channel_id: "conversation",
                    root_thread_ts: "conversation",
                    channel_type: Some("desktop"),
                    initiator_user_id: Some("local-user"),
                    initiator_message_ts: None,
                },
                &dir.path().join("project"),
            )
            .unwrap();
        db.set_agent_session(
            &session.key,
            "agent-1",
            &session.workspace_path,
            "fixture",
            "model",
            "off",
        )
        .unwrap();
        (dir, db, session)
    }
    fn message(
        db: &GatewayDb,
        session: &SessionRow,
        id: &str,
        role: &str,
        text: &str,
        kind: Option<&str>,
    ) -> Result<VisibleMessageRow> {
        db.record_visible_message(
            id,
            &session.key,
            "local_gui",
            "conversation",
            "conversation",
            role,
            text,
            kind,
        )
    }
    fn task(db: &GatewayDb, session: &SessionRow) -> ProductTask {
        db.product_task_for_session(&session.key).unwrap().unwrap()
    }
    fn event(db: &GatewayDb, session: &SessionRow, seq: u32, value: Value) {
        db.project_task_run(&session.key, "agent-1", &format!("{seq:016}"), &value)
            .unwrap();
    }

    #[test]
    fn snapshot_seeds_total_runs_without_history_and_cursor_only_updates_are_quiet() {
        use zork_agent_api::{ExecutionRun, SessionAggregates, SessionSnapshot};
        use zork_client_types::sync::Scope;
        let (dir, db, session) = setup();
        message(&db, &session, "u1", "user", "Goal", None).unwrap();
        let mut snapshot = SessionSnapshot {
            session_id: "agent-1".into(),
            cursor: Some(format!("{:016}", 100)),
            server_time_ms: 100,
            runtime: serde_json::from_value(json!({})).unwrap(),
            aggregates: SessionAggregates {
                complete: true,
                run_count: 100,
                last_run: Some(ExecutionRun {
                    turn_id: "turn-100".into(),
                    started_at_ms: 99,
                    finished_at_ms: Some(100),
                    outcome: Some("finished".into()),
                }),
                ..Default::default()
            },
            execution: serde_json::from_value(
                json!({"generation":1,"status":"finished","tools":[]}),
            )
            .unwrap(),
        };
        db.project_task_snapshot(&session.key, &snapshot).unwrap();
        let current = task(&db, &session);
        assert_eq!(current.run_count, 100);
        assert_eq!(current.last_run_status.as_deref(), Some("finished"));
        assert_eq!(db.task_runs(&current.task_id).unwrap().len(), 1);
        let projected: i64 = db.conn.lock().unwrap().query_row(
            "SELECT json_extract(value,'$.run_count') FROM sync_entities WHERE kind='task' AND id=?1",
            [&current.task_id], |r| r.get(0),
        ).unwrap();
        assert_eq!(projected, 100);
        let before = db.sync_cursor("owner", Scope::Catalog {}).unwrap();
        let changes = db.realtime.subscribe();
        for seq in 100..110 {
            snapshot.cursor = Some(format!("{seq:016}"));
            snapshot.server_time_ms += 1;
            db.project_task_snapshot(&session.key, &snapshot).unwrap();
        }
        assert_eq!(db.sync_cursor("owner", Scope::Catalog {}).unwrap(), before);
        assert!(!changes.has_changed().unwrap());
        event(
            &db,
            &session,
            110,
            json!({"kind":"turn_started","turn_id":"turn-101","started_at_ms":110}),
        );
        snapshot.cursor = Some(format!("{:016}", 110));
        snapshot.aggregates.run_count = 101;
        snapshot.execution.active_turn = Some(ExecutionRun {
            turn_id: "turn-101".into(),
            started_at_ms: 110,
            finished_at_ms: None,
            outcome: None,
        });
        db.project_task_snapshot(&session.key, &snapshot).unwrap();
        assert_eq!(task(&db, &session).run_count, 101);
        assert_eq!(db.task_runs(&current.task_id).unwrap().len(), 2);
        assert!(changes.has_changed().unwrap());
        snapshot.cursor = Some(format!("{:016}", 99));
        snapshot.aggregates.run_count = 99;
        db.project_task_snapshot(&session.key, &snapshot).unwrap();
        assert_eq!(task(&db, &session).run_count, 101);
        drop(db);
        let db = GatewayDb::open(dir.path(), &dir.path().join("workspaces")).unwrap();
        assert_eq!(task(&db, &session).run_count, 101);
    }

    fn historical_review(db: &GatewayDb, session: &SessionRow, message: &str) {
        db.conn.lock().unwrap().execute("UPDATE product_tasks SET state='review',result_sequence=(SELECT sequence FROM visible_message_content WHERE message_id=?2),revision=revision+1 WHERE session_key=?1",params![session.key,message]).unwrap();
    }

    #[test]
    fn historical_identity_goal_and_review_decision_survive_restart() {
        let (dir, db, session) = setup();
        message(&db, &session, "u1", "user", "Review\nthe project", None).unwrap();
        message(&db, &session, "a1", "assistant", "Result", Some("final")).unwrap();
        assert_eq!(task(&db, &session).state, TaskState::Open);
        historical_review(&db, &session, "a1");
        let candidate = task(&db, &session);
        assert_eq!(candidate.title, "Review the project");
        assert_eq!(candidate.goal, "Review\nthe project");
        assert_eq!(candidate.state, TaskState::Review);
        assert_ne!(candidate.task_id, candidate.session_id.clone().unwrap());
        let done = db
            .transition_task(&candidate.task_id, candidate.revision, TaskAction::Accept)
            .unwrap();
        drop(db);
        let db = GatewayDb::open(dir.path(), &dir.path().join("workspaces")).unwrap();
        let restored = task(&db, &session);
        assert_eq!(restored.task_id, done.task_id);
        assert_eq!(restored.state, TaskState::Completed);
        assert_eq!(restored.result_message_id.as_deref(), Some("a1"));
        assert_eq!(restored.revision, done.revision);
        let decisions: i64 = db
            .conn
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM task_decisions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(decisions, 1);
    }

    #[test]
    fn historical_acceptance_keeps_its_recorded_result_revision() {
        let (_dir, db, session) = setup();
        message(&db, &session, "u1", "user", "Goal", None).unwrap();
        message(
            &db,
            &session,
            "a1",
            "assistant",
            "First result",
            Some("final"),
        )
        .unwrap();
        historical_review(&db, &session, "a1");
        let stale = task(&db, &session);
        message(
            &db,
            &session,
            "a2",
            "assistant",
            "Revised result",
            Some("final"),
        )
        .unwrap();
        historical_review(&db, &session, "a2");
        let error = db
            .transition_task(&stale.task_id, stale.revision, TaskAction::Accept)
            .unwrap_err();
        assert_eq!(
            error.downcast_ref::<TaskTransitionError>(),
            Some(&TaskTransitionError::RevisionConflict)
        );
        let current = task(&db, &session);
        let done = db
            .transition_task(&current.task_id, current.revision, TaskAction::Accept)
            .unwrap();
        assert_eq!(done.result_message_id.as_deref(), Some("a2"));
        assert!(db
            .transition_task(&current.task_id, current.revision, TaskAction::Accept)
            .is_err());
    }

    #[test]
    fn historical_closed_state_does_not_gate_new_channel_messages() {
        let (_dir, db, session) = setup();
        message(&db, &session, "u1", "user", "Original goal", None).unwrap();
        let current = task(&db, &session);
        db.transition_task(&current.task_id, current.revision, TaskAction::Cancel)
            .unwrap();
        message(&db, &session, "u2", "user", "A new channel message", None).unwrap();
        assert_eq!(
            db.list_visible_messages(&session.key, None, 100)
                .unwrap()
                .len(),
            2
        );
        message(
            &db,
            &session,
            "a1",
            "assistant",
            "Late result",
            Some("final"),
        )
        .unwrap();
        let cancelled = task(&db, &session);
        assert_eq!(cancelled.state, TaskState::Cancelled);
        db.transition_task(&cancelled.task_id, cancelled.revision, TaskAction::Reopen)
            .unwrap();
        message(&db, &session, "u3", "user", "Additional instructions", None).unwrap();
        let reopened = task(&db, &session);
        assert_eq!(reopened.state, TaskState::Open);
        assert_eq!(reopened.goal, "Original goal");
        assert!(reopened.result_message_id.is_none());
    }

    #[test]
    fn legacy_inbox_preserves_recorded_decisions_without_deriving_new_review() {
        let (dir, db, session) = setup();
        message(&db, &session, "u1", "user", "Goal", None).unwrap();
        assert!(db.list_inbox_tasks().unwrap().is_empty());
        event(
            &db,
            &session,
            1,
            json!({"kind":"turn_started","turn_id":"turn-1","started_at_ms":1}),
        );
        message(&db, &session, "a1", "assistant", "Result", Some("final")).unwrap();
        assert!(
            db.list_inbox_tasks().unwrap().is_empty(),
            "a candidate during a run is not actionable yet"
        );
        event(
            &db,
            &session,
            2,
            json!({"kind":"turn_finished","turn_id":"turn-1","outcome":"finished","finished_at_ms":2}),
        );
        assert!(
            db.list_inbox_tasks().unwrap().is_empty(),
            "a new final is only a message"
        );
        historical_review(&db, &session, "a1");
        let item = db.list_inbox_tasks().unwrap().remove(0);
        assert_eq!(item.state, TaskState::Review);
        assert!(item.goal.is_empty());
        assert!(item.result_text.is_none());
        drop(db);
        let db = GatewayDb::open(dir.path(), &dir.path().join("workspaces")).unwrap();
        assert_eq!(db.list_inbox_tasks().unwrap()[0].task_id, item.task_id);
        db.transition_task(&item.task_id, item.revision, TaskAction::Accept)
            .unwrap();
        assert!(db.list_inbox_tasks().unwrap().is_empty());
        let accepted = task(&db, &session);
        db.transition_task(&accepted.task_id, accepted.revision, TaskAction::Reopen)
            .unwrap();
        event(
            &db,
            &session,
            3,
            json!({"kind":"turn_started","turn_id":"turn-2","started_at_ms":3}),
        );
        event(
            &db,
            &session,
            4,
            json!({"kind":"turn_finished","turn_id":"turn-2","outcome":"failed","finished_at_ms":4}),
        );
        assert_eq!(db.list_inbox_tasks().unwrap()[0].state, TaskState::Open);
        event(
            &db,
            &session,
            5,
            json!({"kind":"turn_started","turn_id":"turn-3","started_at_ms":5}),
        );
        assert!(db.list_inbox_tasks().unwrap().is_empty());
        event(
            &db,
            &session,
            6,
            json!({"kind":"turn_finished","turn_id":"turn-3","outcome":"cancelled","finished_at_ms":6}),
        );
        assert_eq!(db.list_inbox_tasks().unwrap().len(), 1);
        db.clear_agent_session(&session.key).unwrap();
        let detached = db.list_inbox_tasks().unwrap().remove(0);
        assert_eq!(detached.task_id, item.task_id);
        assert!(detached.session_id.is_none());
        db.transition_task(&detached.task_id, detached.revision, TaskAction::Cancel)
            .unwrap();
        assert!(db.list_inbox_tasks().unwrap().is_empty());
    }

    #[test]
    fn runtime_finish_is_not_completion_and_replay_is_idempotent() {
        let (_dir, db, session) = setup();
        message(&db, &session, "u1", "user", "Goal", None).unwrap();
        let start = json!({"kind":"turn_started","turn_id":"turn-1","started_at_ms":1});
        event(&db, &session, 1, start.clone());
        message(&db, &session, "a1", "assistant", "Result", Some("final")).unwrap();
        let busy = task(&db, &session);
        assert_eq!(
            db.transition_task(&busy.task_id, busy.revision, TaskAction::Accept)
                .unwrap_err()
                .downcast_ref::<TaskTransitionError>(),
            Some(&TaskTransitionError::RunActive)
        );
        event(
            &db,
            &session,
            2,
            json!({"kind":"turn_finished","turn_id":"turn-1","outcome":"finished","finished_at_ms":2}),
        );
        let ready = task(&db, &session);
        assert_eq!(ready.state, TaskState::Open);
        assert!(ready.result_message_id.is_none());
        assert!(db
            .transition_task(&ready.task_id, ready.revision, TaskAction::Accept)
            .is_err());
        event(&db, &session, 1, start);
        assert_eq!(task(&db, &session).state, TaskState::Open);
        assert_eq!(db.task_runs(&ready.task_id).unwrap().len(), 1);
        assert_eq!(db.task_runs(&ready.task_id).unwrap()[0].status, "finished");
    }

    #[test]
    fn progress_and_duplicate_messages_do_not_request_review_or_change_revision() {
        let (_dir, db, session) = setup();
        message(&db, &session, "u1", "user", "Goal", None).unwrap();
        let revision = task(&db, &session).revision;
        message(&db, &session, "u1", "user", "Goal", None).unwrap();
        message(
            &db,
            &session,
            "a1",
            "assistant",
            "Still working",
            Some("progress"),
        )
        .unwrap();
        let current = task(&db, &session);
        assert_eq!(current.revision, revision);
        assert_eq!(current.state, TaskState::Open);
        assert!(db
            .transition_task(&current.task_id, current.revision, TaskAction::Accept)
            .is_err());
        event(
            &db,
            &session,
            1,
            json!({"kind":"turn_started","turn_id":"turn-1","started_at_ms":1}),
        );
        event(
            &db,
            &session,
            2,
            json!({"kind":"turn_finished","turn_id":"turn-1","outcome":"finished","finished_at_ms":2}),
        );
        assert_eq!(task(&db, &session).state, TaskState::Open);
    }

    #[test]
    fn summaries_omit_large_content_while_exact_channel_messages_remain_readable() {
        let (_dir, db, session) = setup();
        let goal = "large goal ".repeat(1000);
        let result = "large result ".repeat(1000);
        message(&db, &session, "u1", "user", &goal, None).unwrap();
        message(&db, &session, "a1", "assistant", &result, Some("final")).unwrap();
        let full = task(&db, &session);
        let summary = db.list_product_task_summaries().unwrap().remove(0);
        assert_eq!(summary.task_id, full.task_id);
        assert_eq!(summary.revision, full.revision);
        assert_eq!(summary.result_message_id, full.result_message_id);
        assert!(summary.goal.is_empty());
        assert!(summary.result_text.is_none());
        assert_eq!(full.goal, goal);
        assert!(full.result_text.is_none());
        let channel = db.chat(&session.key).unwrap();
        assert_eq!(
            db.chat_message(&channel.channel.chat_id, "a1")
                .unwrap()
                .text,
            result
        );
    }

    #[test]
    fn task_survives_runtime_replacement_and_keeps_distinct_run_history() {
        let (_dir, db, session) = setup();
        message(&db, &session, "u1", "user", "Persistent goal", None).unwrap();
        event(
            &db,
            &session,
            1,
            json!({"kind":"turn_started","turn_id":"turn-1","started_at_ms":1}),
        );
        event(
            &db,
            &session,
            2,
            json!({"kind":"turn_finished","turn_id":"turn-1","outcome":"finished","finished_at_ms":2}),
        );
        let original = task(&db, &session);
        db.clear_agent_session(&session.key).unwrap();
        let detached = db.list_product_tasks().unwrap();
        assert_eq!(detached.len(), 1);
        assert_eq!(detached[0].task_id, original.task_id);
        assert!(detached[0].session_id.is_none());
        db.set_agent_session(
            &session.key,
            "agent-2",
            &session.workspace_path,
            "fixture",
            "model",
            "off",
        )
        .unwrap();
        db.project_task_run(
            &session.key,
            "agent-2",
            "0000000000000001",
            &json!({"kind":"turn_started","turn_id":"turn-2","started_at_ms":3}),
        )
        .unwrap();
        db.project_task_run(&session.key, "agent-2", "0000000000000002", &json!({"kind":"turn_finished","turn_id":"turn-2","outcome":"finished","finished_at_ms":4})).unwrap();
        let replaced = task(&db, &session);
        assert_eq!(replaced.task_id, original.task_id);
        assert_eq!(replaced.goal, original.goal);
        assert_eq!(replaced.run_count, 2);
        let runs = db.task_runs(&original.task_id).unwrap();
        assert_eq!(runs[0].agent_session_id, "agent-2");
        assert_eq!(runs[1].agent_session_id, "agent-1");
        assert_ne!(runs[0].run_id, runs[1].run_id);
    }

    #[test]
    fn migrating_existing_sessions_preserves_messages_and_never_infers_acceptance() {
        let (dir, db, session) = setup();
        message(&db, &session, "u1", "user", "Existing goal", None).unwrap();
        message(
            &db,
            &session,
            "a1",
            "assistant",
            "Existing result",
            Some("final"),
        )
        .unwrap();
        db.conn.lock().unwrap().execute_batch("DROP TABLE task_decisions; DROP TABLE task_event_cursors; DROP TABLE task_runs; DROP TABLE product_tasks;").unwrap();
        drop(db);
        let db = GatewayDb::open(dir.path(), &dir.path().join("workspaces")).unwrap();
        let migrated = task(&db, &session);
        assert_eq!(migrated.goal, "Existing goal");
        assert_eq!(migrated.state, TaskState::Open);
        assert_eq!(
            db.list_visible_messages(&session.key, None, 100)
                .unwrap()
                .len(),
            2
        );
        drop(db);
        let db = GatewayDb::open(dir.path(), &dir.path().join("workspaces")).unwrap();
        assert_eq!(task(&db, &session).task_id, migrated.task_id);
    }
}
