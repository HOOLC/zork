//! Durable Mesh intake, export journal and owner-side projections. No network
//! work runs under the SQLite lock. Acknowledgements follow transaction commit.
use super::*;
use anyhow::ensure;
use serde::{Deserialize, Serialize};
use zork_mesh::node::ObjectRef;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Assignment {
    pub assignment_id: String,
    pub task_id: String,
    pub owner_origin: String,
    pub executor_origin: String,
    pub workspace_id: String,
    pub goal: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker: Option<WorkerTarget>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorkerTarget {
    pub leader_id: String,
    pub worker_id: String,
}

impl Assignment {
    pub fn validate(&self) -> Result<()> {
        if let Some(worker) = &self.worker {
            for id in [&worker.leader_id, &worker.worker_id] {
                ensure!(
                    !id.is_empty()
                        && id.len() <= 96
                        && id
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
                    "invalid_mesh_agent_id"
                );
            }
        }
        for id in [&self.assignment_id, &self.task_id, &self.workspace_id] {
            ensure!(
                !id.is_empty()
                    && id.len() <= 128
                    && id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
                "invalid_mesh_id"
            );
        }
        ensure!(
            !self.goal.trim().is_empty() && self.goal.len() <= 32 * 1024,
            "invalid_mesh_goal"
        );
        ensure!(
            self.owner_origin != self.executor_origin,
            "mesh_requires_remote_executor"
        );
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Link {
    pub assignment: Assignment,
    pub role: String,
    pub local_task_id: Option<String>,
    pub session_key: Option<String>,
    pub state: String,
    pub cursor: i64,
    pub remote_status: Option<String>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum EventBody {
    Message {
        message_id: String,
        text: String,
        message_kind: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pages: Vec<zork_client_types::pages::DeliveredPage>,
    },
    Run {
        run_id: String,
        agent_session_id: String,
        turn_id: String,
        status: String,
        started_at_ms: i64,
        finished_at_ms: Option<i64>,
    },
    Artifact {
        artifact_id: String,
        name: String,
        source_path: String,
        media_type: String,
        caption: Option<String>,
        version: i64,
        object: Option<ObjectRef>,
    },
    Attention {
        message: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeshEvent {
    pub sequence: i64,
    pub assignment_id: String,
    pub body: EventBody,
}

pub(super) fn initialize(conn: &Connection) -> Result<()> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS mesh_links (
        assignment_id TEXT PRIMARY KEY, assignment_json TEXT NOT NULL,
        role TEXT NOT NULL CHECK(role IN ('owner','executor')),
        local_task_id TEXT UNIQUE REFERENCES product_tasks(task_id), session_key TEXT REFERENCES sessions(key),
        state TEXT NOT NULL, cursor INTEGER NOT NULL DEFAULT 0,
        remote_status TEXT, error TEXT,
        decision_json TEXT, decision_sent INTEGER NOT NULL DEFAULT 0,
        cancel_requested INTEGER NOT NULL DEFAULT 0, cancel_sent INTEGER NOT NULL DEFAULT 0
    );
    CREATE TABLE IF NOT EXISTS mesh_reworks(assignment_id TEXT NOT NULL REFERENCES mesh_links(assignment_id),request_id TEXT NOT NULL,goal TEXT NOT NULL,delivered INTEGER NOT NULL DEFAULT 0,PRIMARY KEY(assignment_id,request_id));
    CREATE TABLE IF NOT EXISTS mesh_runtime_sessions(assignment_id TEXT PRIMARY KEY REFERENCES mesh_links(assignment_id),runtime_id TEXT NOT NULL UNIQUE);
    CREATE TABLE IF NOT EXISTS mesh_exports (
        sequence INTEGER PRIMARY KEY AUTOINCREMENT,
        assignment_id TEXT NOT NULL REFERENCES mesh_links(assignment_id),
        event_key TEXT NOT NULL, payload TEXT NOT NULL,
        ready INTEGER NOT NULL DEFAULT 0,
        UNIQUE(assignment_id,event_key)
    );
    CREATE INDEX IF NOT EXISTS mesh_exports_assignment ON mesh_exports(assignment_id,sequence);
    CREATE TABLE IF NOT EXISTS mesh_artifact_file_refs (
        artifact_id TEXT PRIMARY KEY REFERENCES task_file_snapshots(artifact_id),
        assignment_id TEXT NOT NULL REFERENCES mesh_links(assignment_id),
        remote_artifact_id TEXT NOT NULL, remote_version INTEGER NOT NULL, object_json TEXT NOT NULL
    );")?;
    Ok(())
}

fn link_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Link> {
    let assignment_json: String = row.get(0)?;
    let assignment = serde_json::from_str(&assignment_json).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })?;
    Ok(Link {
        assignment,
        role: row.get(1)?,
        local_task_id: row.get(2)?,
        session_key: row.get(3)?,
        state: row.get(4)?,
        cursor: row.get(5)?,
        remote_status: row.get(6)?,
        error: row.get(7)?,
    })
}
const LINK_SELECT: &str = "SELECT assignment_json,role,local_task_id,session_key,state,cursor,remote_status,error FROM mesh_links";
fn get_link(conn: &Connection, assignment: &str) -> Result<Option<Link>> {
    Ok(conn
        .query_row(
            &format!("{LINK_SELECT} WHERE assignment_id=?1"),
            [assignment],
            link_row,
        )
        .optional()?)
}

fn insert_message(
    conn: &Connection,
    session: &SessionRow,
    id: &str,
    role: &str,
    text: &str,
    kind: Option<&str>,
) -> Result<Vec<super::chats::Topic>> {
    let inserted = conn.execute("INSERT OR IGNORE INTO visible_messages(message_id,session_key,connection_id,conversation_id,root_message_id,role,text,kind,created_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)", params![id,session.key,session.connection_id,session.channel_id,session.root_thread_ts,role,text,kind,now_rfc3339()])?;
    if inserted > 0 {
        let message = conn.query_row("SELECT sequence,message_id,session_key,role,text,kind,created_at FROM visible_message_content WHERE message_id=?1", [id], map_visible_message_row)?;
        let topics = tasks::record_message(conn, &message)?;
        super::pages::record_file_handoff(conn, &message)?;
        return Ok(topics);
    }
    Ok(vec![])
}

fn export(conn: &Connection, assignment: &str, key: &str, body: &EventBody) -> Result<()> {
    let payload = serde_json::to_string(body)?;
    ensure!(
        payload.len() < zork_mesh::MAX_FRAME / 2,
        "mesh_event_too_large"
    );
    conn.execute(
        "INSERT OR IGNORE INTO mesh_exports(assignment_id,event_key,payload) VALUES (?1,?2,?3)",
        params![assignment, key, payload],
    )?;
    Ok(())
}

impl GatewayDb {
    pub fn mesh_queue_rework(
        &self,
        task_id: &str,
        request_id: &str,
        goal: &str,
        expected_revision: i64,
    ) -> Result<()> {
        let link = self
            .mesh_task_link(task_id)?
            .context("Remote task missing")?;
        ensure!(
            link.role == "owner" && link.assignment.worker.is_some(),
            "Remote Worker task required"
        );
        let key = link
            .session_key
            .as_deref()
            .context("Task session missing")?;
        let session = self.get_session(key)?.context("Task session missing")?;
        let mut conn = self.conn.lock().expect("db mutex");
        let tx = conn.transaction()?;
        let previous: Option<String> = tx
            .query_row(
                "SELECT goal FROM mesh_reworks WHERE assignment_id=?1 AND request_id=?2",
                params![link.assignment.assignment_id, request_id],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(previous) = previous {
            ensure!(
                previous == goal,
                "Rework request ID already has different content"
            );
            return Ok(());
        }
        let (revision, state): (i64, String) = tx.query_row(
            "SELECT revision,state FROM product_tasks WHERE task_id=?1",
            [task_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        ensure!(
            revision == expected_revision,
            "Task revision changed; inspect it again"
        );
        ensure!(
            !matches!(state.as_str(), "completed" | "cancelled"),
            "The user must reopen this closed task before rework"
        );
        let topics = insert_message(
            &tx,
            &session,
            &format!("rework-{}-{request_id}", link.assignment.assignment_id),
            "user",
            goal,
            None,
        )?;
        tx.execute(
            "INSERT INTO mesh_reworks(assignment_id,request_id,goal) VALUES (?1,?2,?3)",
            params![link.assignment.assignment_id, request_id, goal],
        )?;
        tx.commit()?;
        self.flush_messages(&conn)?;
        self.chat_topics.publish(topics);
        Ok(())
    }
    pub fn mesh_pending_reworks(&self, assignment: &str) -> Result<Vec<(String, String)>> {
        let conn = self.conn.lock().expect("db mutex");
        let rows=conn.prepare("SELECT request_id,goal FROM mesh_reworks WHERE assignment_id=?1 AND delivered=0 ORDER BY rowid")?.query_map([assignment],|r|Ok((r.get(0)?,r.get(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }
    pub fn mesh_rework_sent(&self, assignment: &str, request: &str) -> Result<()> {
        self.conn.lock().expect("db mutex").execute(
            "UPDATE mesh_reworks SET delivered=1 WHERE assignment_id=?1 AND request_id=?2",
            params![assignment, request],
        )?;
        Ok(())
    }
    pub fn mesh_runtime_id(&self, assignment: &str) -> Result<String> {
        let mut conn = self.conn.lock().expect("db mutex");
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT OR IGNORE INTO mesh_runtime_sessions(assignment_id,runtime_id) VALUES (?1,?2)",
            params![assignment, ulid::Ulid::new().to_string()],
        )?;
        let id = tx.query_row(
            "SELECT runtime_id FROM mesh_runtime_sessions WHERE assignment_id=?1",
            [assignment],
            |r| r.get(0),
        )?;
        tx.commit()?;
        self.flush_messages(&conn)?;
        Ok(id)
    }
    pub fn mesh_links(&self) -> Result<Vec<Link>> {
        let conn = self.conn.lock().expect("db mutex");
        let mut statement = conn.prepare(LINK_SELECT)?;
        let rows = statement
            .query_map([], link_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }
    pub fn mesh_link(&self, id: &str) -> Result<Option<Link>> {
        get_link(&self.conn.lock().expect("db mutex"), id)
    }
    pub fn mesh_task_link(&self, task_id: &str) -> Result<Option<Link>> {
        let conn = self.conn.lock().expect("db mutex");
        Ok(conn
            .query_row(
                &format!("{LINK_SELECT} WHERE local_task_id=?1"),
                [task_id],
                link_row,
            )
            .optional()?)
    }

    pub fn mesh_delegate(
        &self,
        assignment: &Assignment,
        session: &SessionRow,
        expected_revision: i64,
    ) -> Result<Link> {
        assignment.validate()?;
        let mut conn = self.conn.lock().expect("db mutex");
        let tx = conn.transaction()?;
        if let Some(existing) = get_link(&tx, &assignment.assignment_id)? {
            ensure!(
                existing.role == "owner" && existing.assignment == *assignment,
                "mesh_command_conflict"
            );
            return Ok(existing);
        }
        let (revision, state, session_key): (i64, String, String) = tx.query_row(
            "SELECT revision,state,session_key FROM product_tasks WHERE task_id=?1",
            [&assignment.task_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        ensure!(revision == expected_revision, "task_revision_conflict");
        ensure!(
            state == "open" && session_key == session.key,
            "mesh_requires_open_task"
        );
        let active: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM task_runs WHERE task_id=?1 AND status='running')",
            [&assignment.task_id],
            |r| r.get(0),
        )?;
        ensure!(!active, "task_run_active");
        tx.execute("INSERT INTO mesh_links(assignment_id,assignment_json,role,local_task_id,session_key,state,remote_status) VALUES (?1,?2,'owner',?3,?4,'queued','queued')",params![assignment.assignment_id,serde_json::to_string(assignment)?,assignment.task_id,session.key])?;
        let topics = insert_message(
            &tx,
            session,
            &format!("mesh-{}-goal", assignment.assignment_id),
            "user",
            &assignment.goal,
            None,
        )?;
        let link = get_link(&tx, &assignment.assignment_id)?.context("mesh link missing")?;
        tx.commit()?;
        self.flush_messages(&conn)?;
        self.chat_topics.publish(topics);
        Ok(link)
    }

    pub fn mesh_receive_assignment(&self, assignment: &Assignment) -> Result<Link> {
        assignment.validate()?;
        let mut conn = self.conn.lock().expect("db mutex");
        let tx = conn.transaction()?;
        if let Some(existing) = get_link(&tx, &assignment.assignment_id)? {
            ensure!(
                existing.role == "executor" && existing.assignment == *assignment,
                "mesh_command_conflict"
            );
            return Ok(existing);
        }
        let pending:i64=tx.query_row("SELECT COUNT(*) FROM mesh_links WHERE role='executor' AND state NOT IN ('settled','cancelled')",[],|r|r.get(0))?;
        ensure!(pending < 64, "mesh_executor_queue_full");
        tx.execute("INSERT INTO mesh_links(assignment_id,assignment_json,role,state) VALUES (?1,?2,'executor','queued')",params![assignment.assignment_id,serde_json::to_string(assignment)?])?;
        let link = get_link(&tx, &assignment.assignment_id)?.context("mesh link missing")?;
        tx.commit()?;
        self.flush_messages(&conn)?;
        Ok(link)
    }

    pub fn mesh_bind_executor(&self, assignment_id: &str, session: &SessionRow) -> Result<()> {
        let mut conn = self.conn.lock().expect("db mutex");
        let tx = conn.transaction()?;
        let task_id: String = tx.query_row(
            "SELECT task_id FROM product_tasks WHERE session_key=?1",
            [&session.key],
            |r| r.get(0),
        )?;
        tx.execute("UPDATE mesh_links SET session_key=?2,local_task_id=?3,state='preparing' WHERE assignment_id=?1 AND role='executor' AND state IN ('queued','preparing')",params![assignment_id,session.key,task_id])?;
        let link = get_link(&tx, assignment_id)?.context("mesh link missing")?;
        let topics = insert_message(
            &tx,
            session,
            &format!("mesh-{assignment_id}-goal"),
            "user",
            &link.assignment.goal,
            None,
        )?;
        tx.commit()?;
        self.flush_messages(&conn)?;
        self.chat_topics.publish(topics);
        Ok(())
    }

    pub fn mesh_state(&self, id: &str, state: &str, error: Option<&str>) -> Result<()> {
        self.conn.lock().expect("db mutex").execute(
            "UPDATE mesh_links SET state=?2,error=?3 WHERE assignment_id=?1",
            params![id, state, error],
        )?;
        Ok(())
    }

    /// An interrupted HTTP append has an uncertain outcome. Never replay it as a
    /// fresh mailbox input. Existing running sessions recover through Agent.
    pub fn mesh_recover_dispatch(&self) -> Result<()> {
        let mut conn = self.conn.lock().expect("db mutex");
        let tx = conn.transaction()?;
        let ids: Vec<String> = {
            let mut q = tx.prepare("SELECT assignment_id FROM mesh_links WHERE role='executor' AND state='dispatching'")?;
            let rows = q
                .query_map([], |r| r.get(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows
        };
        for id in ids {
            if get_link(&tx, &id)?.is_some_and(|l| l.assignment.worker.is_some()) {
                tx.execute(
                    "UPDATE mesh_links SET state='preparing',error=NULL WHERE assignment_id=?1",
                    [&id],
                )?;
                continue;
            }
            tx.execute("UPDATE mesh_links SET state='needs_attention',error='runtime_delivery_uncertain' WHERE assignment_id=?1",[&id])?;
            export(
                &tx,
                &id,
                "attention:dispatch",
                &EventBody::Attention {
                    message: "runtime_delivery_uncertain".into(),
                },
            )?;
        }
        tx.commit()?;
        self.flush_messages(&conn)?;
        Ok(())
    }

    pub fn mesh_attention(&self, id: &str, message: &str) -> Result<()> {
        let mut conn = self.conn.lock().expect("db mutex");
        let tx = conn.transaction()?;
        tx.execute(
            "UPDATE mesh_links SET state='needs_attention',error=?2 WHERE assignment_id=?1",
            params![id, message],
        )?;
        export(
            &tx,
            id,
            "attention:execution",
            &EventBody::Attention {
                message: message.into(),
            },
        )?;
        tx.commit()?;
        self.flush_messages(&conn)?;
        Ok(())
    }

    /// Capture from already durable product records. Artifacts precede explicit
    /// finals in this transaction; publishing and delivery resume from this log.
    pub fn mesh_capture(&self, id: &str) -> Result<()> {
        let mut conn = self.conn.lock().expect("db mutex");
        let tx = conn.transaction()?;
        let link = get_link(&tx, id)?.context("mesh link missing")?;
        ensure!(link.role == "executor", "not_mesh_executor");
        let Some(task_id) = link.local_task_id.as_ref() else {
            return Ok(());
        };
        let mut q=tx.prepare("SELECT artifact_id,name,source_path,media_type,caption,version FROM task_file_snapshots WHERE task_id=?1 UNION ALL SELECT a.artifact_id,a.name,a.source_path,a.media_type,a.caption,a.version FROM conversation_file_snapshots a WHERE a.session_key=(SELECT session_key FROM product_tasks WHERE task_id=?1) AND EXISTS(SELECT 1 FROM visible_message_content m WHERE m.session_key=a.session_key AND m.role='assistant' AND m.kind='file' AND instr(m.text,'\"id\":\"'||a.artifact_id||'\"')>0)")?;
        let artifacts = q
            .query_map([task_id], |r| {
                Ok(EventBody::Artifact {
                    artifact_id: r.get(0)?,
                    name: r.get(1)?,
                    source_path: r.get(2)?,
                    media_type: r.get(3)?,
                    caption: r.get(4)?,
                    version: r.get(5)?,
                    object: None,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(q);
        for body in artifacts {
            if let EventBody::Artifact { artifact_id, .. } = &body {
                export(&tx, id, &format!("artifact:{artifact_id}"), &body)?;
            }
        }
        let mut q=tx.prepare("SELECT message_id,text,kind FROM visible_message_content WHERE session_key=?1 AND role='assistant' ORDER BY sequence")?;
        let messages = q
            .query_map([&link.session_key], |r| {
                Ok(EventBody::Message {
                    message_id: r.get(0)?,
                    text: r.get(1)?,
                    message_kind: r.get(2)?,
                    pages: vec![],
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(q);
        for mut body in messages {
            if let EventBody::Message {
                message_id, pages, ..
            } = &mut body
            {
                *pages = super::pages::message_pages(&tx, message_id)?;
            }
            if let EventBody::Message { message_id, .. } = &body {
                export(&tx, id, &format!("message:{message_id}"), &body)?;
            }
        }
        let mut q=tx.prepare("SELECT run_id,agent_session_id,turn_id,status,started_at_ms,finished_at_ms FROM task_runs WHERE task_id=?1 ORDER BY rowid")?;
        let runs = q
            .query_map([task_id], |r| {
                Ok(EventBody::Run {
                    run_id: r.get(0)?,
                    agent_session_id: r.get(1)?,
                    turn_id: r.get(2)?,
                    status: r.get(3)?,
                    started_at_ms: r.get(4)?,
                    finished_at_ms: r.get(5)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(q);
        for body in runs {
            if let EventBody::Run { run_id, status, .. } = &body {
                export(&tx, id, &format!("run:{run_id}:{status}"), &body)?;
            }
        }
        tx.commit()?;
        self.flush_messages(&conn)?;
        Ok(())
    }

    pub fn mesh_events(&self, id: &str, after: i64, only_ready: bool) -> Result<Vec<MeshEvent>> {
        let conn = self.conn.lock().expect("db mutex");
        // Never skip an unpublished earlier event (especially a pending file).
        let mut q=conn.prepare("SELECT sequence,payload FROM mesh_exports WHERE assignment_id=?1 AND sequence>?2 ORDER BY sequence LIMIT 16")?;
        let rows = q
            .query_map(params![id, after], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut result = Vec::new();
        let mut bytes = 0;
        for (sequence, payload) in rows {
            if only_ready {
                let ready: bool = conn.query_row(
                    "SELECT ready FROM mesh_exports WHERE sequence=?1",
                    [sequence],
                    |r| r.get(0),
                )?;
                if !ready {
                    break;
                }
            }
            bytes += payload.len();
            if bytes > zork_mesh::MAX_FRAME - 2048 {
                break;
            }
            result.push(MeshEvent {
                sequence,
                assignment_id: id.into(),
                body: serde_json::from_str(&payload)?,
            });
        }
        Ok(result)
    }

    pub fn mesh_pending_exports(&self) -> Result<Vec<MeshEvent>> {
        let conn = self.conn.lock().expect("db mutex");
        let mut q=conn.prepare("SELECT sequence,assignment_id,payload FROM mesh_exports WHERE ready=0 ORDER BY sequence LIMIT 16")?;
        let rows = q
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .map(|(sequence, assignment_id, payload)| {
                Ok(MeshEvent {
                    sequence,
                    assignment_id,
                    body: serde_json::from_str(&payload)?,
                })
            })
            .collect()
    }
    pub fn mesh_export_ready(&self, event: &MeshEvent) -> Result<()> {
        self.conn.lock().expect("db mutex").execute(
            "UPDATE mesh_exports SET payload=?2,ready=1 WHERE sequence=?1",
            params![event.sequence, serde_json::to_string(&event.body)?],
        )?;
        Ok(())
    }

    pub fn mesh_import(
        &self,
        peer: &str,
        event: &MeshEvent,
        content: Option<&[u8]>,
    ) -> Result<bool> {
        let link = self
            .mesh_link(&event.assignment_id)?
            .context("unknown_mesh_assignment")?;
        ensure!(
            link.role == "owner" && link.assignment.executor_origin == peer,
            "mesh_event_unauthorized"
        );
        let session = self
            .get_session(
                link.session_key
                    .as_deref()
                    .context("mesh session missing")?,
            )?
            .context("mesh session missing")?;
        let task_id = link.local_task_id.as_deref().context("mesh task missing")?;
        let mut conn = self.conn.lock().expect("db mutex");
        let tx = conn.transaction()?;
        let current = get_link(&tx, &event.assignment_id)?.context("mesh link missing")?;
        if event.sequence <= current.cursor {
            return Ok(false);
        }
        let mut topics = vec![];
        {
            match &event.body {
                EventBody::Message {
                    message_id,
                    text,
                    message_kind,
                    pages,
                } => {
                    ensure!(
                        text.len() < zork_mesh::MAX_FRAME / 2,
                        "mesh_message_too_large"
                    );
                    let id = format!("mesh-{}-{message_id}", event.assignment_id);
                    let text = if let Some((body, mut files)) =
                        zork_client_types::files::decode(text)
                    {
                        ensure!(
                            zork_client_types::files::valid(&files),
                            "invalid_attachments"
                        );
                        for file in &mut files {
                            file.id = format!("mesh-{}-{}", event.assignment_id, file.id);
                            let (name,snapshot):(String,super::snapshots::Snapshot) = tx.query_row("SELECT name,snapshot FROM task_file_snapshots WHERE task_id=?1 AND artifact_id=?2",params![task_id,file.id],|r|Ok((r.get(0)?,r.get(1)?)))?;
                            ensure!(
                                file.valid()
                                    && name == file.name
                                    && snapshot.byte_len == file.byte_len
                                    && snapshot.root == file.content_root,
                                "attachment_reference_mismatch"
                            );
                        }
                        zork_client_types::files::compose(&body, &files)
                    } else {
                        text.clone()
                    };
                    topics.extend(insert_message(
                        &tx,
                        &session,
                        &id,
                        "assistant",
                        &text,
                        message_kind.as_deref(),
                    )?);
                    super::pages::record_pages(&tx, &session.key, &id, pages, &now_rfc3339())?;
                }
                EventBody::Run {
                    run_id,
                    agent_session_id,
                    turn_id,
                    status,
                    started_at_ms,
                    finished_at_ms,
                } => {
                    ensure!(
                        matches!(
                            status.as_str(),
                            "running" | "finished" | "failed" | "cancelled"
                        ),
                        "invalid_mesh_run_status"
                    );
                    let id = format!("mesh-{}-{run_id}", event.assignment_id);
                    tx.execute("INSERT INTO task_runs(run_id,task_id,agent_session_id,turn_id,status,started_at_ms,finished_at_ms) VALUES (?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(run_id) DO UPDATE SET status=excluded.status,finished_at_ms=excluded.finished_at_ms WHERE task_runs.status='running'",params![id,task_id,format!("{peer}:{agent_session_id}"),turn_id,status,started_at_ms,finished_at_ms])?;
                    tx.execute(
                        "UPDATE mesh_links SET remote_status=?2,error=NULL,state=CASE WHEN state='needs_attention' THEN 'received' ELSE state END WHERE assignment_id=?1",
                        params![event.assignment_id, status],
                    )?;
                    tx.execute("UPDATE product_tasks SET revision=revision+1,updated_at=?2 WHERE task_id=?1",params![task_id,now_rfc3339()])?;
                }
                EventBody::Artifact {
                    artifact_id,
                    name,
                    source_path,
                    media_type,
                    caption,
                    version,
                    object,
                } => {
                    let object = object.as_ref().context("mesh artifact missing object")?;
                    ensure!(object.origin == peer, "mesh_artifact_wrong_origin");
                    let bytes = content.context("mesh artifact not downloaded")?;
                    object.verify(bytes)?;
                    ensure!(
                        !name.is_empty()
                            && name.len() <= 255
                            && !name.contains(['/', '\\'])
                            && *version > 0,
                        "invalid_mesh_artifact_name"
                    );
                    let id = format!("mesh-{}-{artifact_id}", event.assignment_id);
                    let local_version:i64=tx.query_row("SELECT COALESCE(MAX(version),0)+1 FROM task_file_snapshots WHERE task_id=?1 AND source_path=?2",params![task_id,source_path],|r|r.get(0))?;
                    let snapshot = self.freeze_file(name, bytes)?;
                    tx.execute("INSERT OR IGNORE INTO task_file_snapshots(artifact_id,task_id,name,source_path,media_type,snapshot,version,created_at,caption,workspace) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",params![id,task_id,name,source_path,media_type,snapshot,local_version,now_rfc3339(),caption,format!("{}:{}",peer,link.assignment.workspace_id)])?;
                    tx.execute("INSERT OR IGNORE INTO mesh_artifact_file_refs(artifact_id,assignment_id,remote_artifact_id,remote_version,object_json) VALUES (?1,?2,?3,?4,?5)",params![id,event.assignment_id,artifact_id,version,serde_json::to_string(object)?])?;
                    tx.execute("UPDATE product_tasks SET revision=revision+1,updated_at=?2 WHERE task_id=?1",params![task_id,now_rfc3339()])?;
                }
                EventBody::Attention { message } => {
                    tx.execute("UPDATE mesh_links SET state='needs_attention',remote_status='unknown',error=?2 WHERE assignment_id=?1",params![event.assignment_id,message])?;
                }
            }
        }
        tx.execute(
            "UPDATE mesh_links SET cursor=?2 WHERE assignment_id=?1",
            params![event.assignment_id, event.sequence],
        )?;
        tx.commit()?;
        self.flush_messages(&conn)?;
        self.chat_topics.publish(topics);
        Ok(true)
    }

    pub fn mesh_cancel_request(&self, id: &str) -> Result<()> {
        self.conn.lock().expect("db mutex").execute(
            "UPDATE mesh_links SET cancel_requested=1,cancel_sent=0 WHERE assignment_id=?1",
            [id],
        )?;
        Ok(())
    }
    pub fn mesh_cancel_pending(&self, id: &str) -> Result<bool> {
        Ok(self.conn.lock().expect("db mutex").query_row(
            "SELECT cancel_requested=1 AND cancel_sent=0 FROM mesh_links WHERE assignment_id=?1",
            [id],
            |r| r.get(0),
        )?)
    }
    pub fn mesh_cancel_sent(&self, id: &str) -> Result<()> {
        self.conn.lock().expect("db mutex").execute(
            "UPDATE mesh_links SET cancel_sent=1 WHERE assignment_id=?1",
            [id],
        )?;
        Ok(())
    }
    pub fn mesh_remote_cancelled(&self, id: &str) -> Result<()> {
        self.conn.lock().expect("db mutex").execute("UPDATE mesh_links SET remote_status='cancelled' WHERE assignment_id=?1 AND role='owner'",[id])?;
        Ok(())
    }

    pub fn mesh_pending_decision(&self, id: &str) -> Result<Option<Value>> {
        Ok(self
            .conn
            .lock()
            .expect("db mutex")
            .query_row(
                "SELECT decision_json FROM mesh_links WHERE assignment_id=?1 AND decision_sent=0",
                [id],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten()
            .map(|s| serde_json::from_str(&s))
            .transpose()?)
    }
    pub fn mesh_decision_sent(&self, id: &str) -> Result<()> {
        self.conn.lock().expect("db mutex").execute(
            "UPDATE mesh_links SET decision_sent=1 WHERE assignment_id=?1",
            [id],
        )?;
        Ok(())
    }
    pub fn mesh_receive_decision(&self, id: &str, decision: &Value) -> Result<()> {
        let state = decision["state"]
            .as_str()
            .context("invalid_mesh_decision")?;
        ensure!(
            matches!(state, "completed" | "cancelled" | "open"),
            "invalid_mesh_decision"
        );
        let revision = decision["revision"]
            .as_i64()
            .context("invalid_mesh_revision")?;
        let mut conn = self.conn.lock().expect("db mutex");
        let tx = conn.transaction()?;
        let link = get_link(&tx, id)?.context("unknown_mesh_assignment")?;
        ensure!(link.role == "executor", "not_mesh_executor");
        let previous: Option<String> = tx.query_row(
            "SELECT decision_json FROM mesh_links WHERE assignment_id=?1",
            [id],
            |r| r.get(0),
        )?;
        if let Some(previous) = previous {
            let previous: Value = serde_json::from_str(&previous)?;
            let old = previous["revision"]
                .as_i64()
                .context("invalid saved decision")?;
            if revision < old {
                return Ok(());
            }
            if revision == old {
                ensure!(previous == *decision, "mesh_decision_conflict");
                return Ok(());
            }
        }
        if let Some(task_id) = link.local_task_id {
            let accepted_sequence = if state == "completed" {
                let message = decision["result_message_id"]
                    .as_str()
                    .context("mesh_decision_missing_result")?;
                let original = message
                    .strip_prefix(&format!("mesh-{id}-"))
                    .context("mesh_decision_wrong_result")?;
                Some(tx.query_row("SELECT sequence FROM visible_message_content WHERE message_id=?1 AND session_key=?2 AND kind='final'",params![original,link.session_key],|r|r.get::<_,i64>(0))?)
            } else {
                None
            };
            tx.execute("UPDATE product_tasks SET state=?2,revision=revision+1,updated_at=?3,result_sequence=CASE WHEN ?2='open' THEN NULL WHEN ?2='completed' THEN ?4 ELSE result_sequence END WHERE task_id=?1",params![task_id,state,now_rfc3339(),accepted_sequence])?;
        }
        tx.execute(
            "UPDATE mesh_links SET decision_json=?2,state='settled' WHERE assignment_id=?1",
            params![id, serde_json::to_string(decision)?],
        )?;
        tx.commit()?;
        self.flush_messages(&conn)?;
        Ok(())
    }
}

/// Called inside the product decision transaction, so a crash cannot strand an
/// accepted decision between local SQLite and the durable remote outbox.
pub(super) fn record_decision(
    conn: &Connection,
    task_id: &str,
    state: &str,
    revision: i64,
    result: Option<&str>,
) -> Result<()> {
    let artifacts: Vec<String> = {
        let mut q = conn.prepare(
            "SELECT artifact_id FROM task_file_snapshots WHERE task_id=?1 ORDER BY artifact_id",
        )?;
        let rows = q
            .query_map([task_id], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    conn.execute("UPDATE mesh_links SET decision_json=?2,decision_sent=0 WHERE local_task_id=?1 AND role='owner'",params![task_id,serde_json::to_string(&json!({"state":state,"revision":revision,"result_message_id":result,"artifact_ids":artifacts}))?])?;
    Ok(())
}

pub(super) fn validate_export_message(
    conn: &Connection,
    session_key: &str,
    role: &str,
    text: &str,
    kind: Option<&str>,
) -> Result<()> {
    let executor: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM mesh_links WHERE session_key=?1 AND role='executor')",
        [session_key],
        |r| r.get(0),
    )?;
    if executor && role == "assistant" {
        let body = EventBody::Message {
            message_id: "x".repeat(128),
            text: text.into(),
            message_kind: kind.map(str::to_owned),
            pages: vec![],
        };
        ensure!(
            serde_json::to_vec(&body)?.len() < zork_mesh::MAX_FRAME / 2,
            "mesh_message_too_large_submit_as_file"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_pages_survive_mesh_delivery_and_invalid_metadata_rolls_back() {
        let root = tempfile::tempdir().unwrap();
        let (db, session) = fixture(root.path());
        let assignment = assignment(&db, &session);
        db.mesh_delegate(&assignment, &session, 0).unwrap();
        let page =
            super::super::pages::page_link("Report", "https://example.test/result", "").unwrap();
        let event = MeshEvent {
            sequence: 1,
            assignment_id: assignment.assignment_id.clone(),
            body: EventBody::Message {
                message_id: "page-from-worker".into(),
                text: "[Report](https://example.test/result)".into(),
                message_kind: Some("page".into()),
                pages: vec![zork_client_types::pages::DeliveredPage { page: page.clone() }],
            },
        };
        let mut invalid = event.clone();
        if let EventBody::Message { pages, .. } = &mut invalid.body {
            pages[0].page.id = "forged".into();
        }
        assert!(db
            .mesh_import(&assignment.executor_origin, &invalid, None)
            .is_err());
        assert!(db.page_catalog().unwrap().references.is_empty());
        assert!(db
            .mesh_import(&assignment.executor_origin, &event, None)
            .unwrap());
        assert!(!db
            .mesh_import(&assignment.executor_origin, &event, None)
            .unwrap());
        assert!(
            db.page_catalog().unwrap().references.is_empty(),
            "unbound sessions are not client catalog entries"
        );
        db.set_agent_session(
            &session.key,
            "owner-session",
            &session.workspace_path,
            "fixture",
            "model",
            "off",
        )
        .unwrap();
        let catalog = db.page_catalog().unwrap();
        assert_eq!(catalog.references.len(), 1);
        assert_eq!(catalog.references[0].session_id, "owner-session");
        assert_eq!(catalog.references[0].page, page);
        assert!(catalog.applications.is_empty());
        let old = serde_json::json!({"kind":"message","message_id":"old","text":"compatible","message_kind":null});
        assert!(
            matches!(serde_json::from_value::<EventBody>(old).unwrap(), EventBody::Message {pages, ..} if pages.is_empty())
        );
    }
    fn fixture(root: &Path) -> (GatewayDb, SessionRow) {
        let workspace = root.join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let db = GatewayDb::open(&root.join("state"), &workspace).unwrap();
        let session = db
            .create_session_at_workspace(
                EnsureSession {
                    connection_id: "local_gui",
                    platform: "local_gui",
                    channel_id: "test",
                    root_thread_ts: "test",
                    channel_type: Some("desktop"),
                    initiator_user_id: Some("test-user"),
                    initiator_message_ts: None,
                },
                &workspace,
            )
            .unwrap();
        (db, session)
    }
    fn assignment(db: &GatewayDb, session: &SessionRow) -> Assignment {
        Assignment {
            assignment_id: "fixture-assignment".into(),
            task_id: db
                .product_task_for_session(&session.key)
                .unwrap()
                .unwrap()
                .task_id,
            owner_origin: "key:owner".into(),
            executor_origin: "key:executor".into(),
            workspace_id: "lab".into(),
            goal: "Prepare a report".into(),
            worker: None,
        }
    }
    #[test]
    fn durable_intake_rejects_reused_id_and_marks_uncertain_dispatch_without_replay() {
        let root = tempfile::tempdir().unwrap();
        let (db, session) = fixture(root.path());
        let a = assignment(&db, &session);
        db.mesh_receive_assignment(&a).unwrap();
        db.mesh_receive_assignment(&a).unwrap();
        let mut changed = a.clone();
        changed.goal = "Different operation".into();
        assert!(db.mesh_receive_assignment(&changed).is_err());
        db.mesh_bind_executor(&a.assignment_id, &session).unwrap();
        db.mesh_state(&a.assignment_id, "dispatching", None)
            .unwrap();
        drop(db);
        let db =
            GatewayDb::open(&root.path().join("state"), &root.path().join("workspace")).unwrap();
        db.mesh_recover_dispatch().unwrap();
        db.mesh_recover_dispatch().unwrap();
        assert_eq!(db.mesh_links().unwrap().len(), 1);
        assert_eq!(
            db.mesh_link(&a.assignment_id).unwrap().unwrap().state,
            "needs_attention"
        );
        assert_eq!(db.mesh_events(&a.assignment_id, 0, false).unwrap().len(), 1);
    }
    #[test]
    fn owner_imports_once_and_historical_decisions_do_not_discard_later_messages() {
        let root = tempfile::tempdir().unwrap();
        let (db, session) = fixture(root.path());
        let a = assignment(&db, &session);
        db.mesh_delegate(&a, &session, 0).unwrap();
        db.mesh_delegate(&a, &session, 0).unwrap();
        let run = MeshEvent {
            sequence: 1,
            assignment_id: a.assignment_id.clone(),
            body: EventBody::Run {
                run_id: "run-b".into(),
                agent_session_id: "session-b".into(),
                turn_id: "turn-b".into(),
                status: "finished".into(),
                started_at_ms: 1,
                finished_at_ms: Some(2),
            },
        };
        assert!(db.mesh_import("key:wrong", &run, None).is_err());
        assert!(db.mesh_import(&a.executor_origin, &run, None).unwrap());
        let task = db.product_task(&a.task_id).unwrap().unwrap();
        assert_eq!(task.run_count, 1);
        assert!(!db.mesh_import(&a.executor_origin, &run, None).unwrap());
        assert_eq!(
            db.product_task(&a.task_id).unwrap().unwrap().revision,
            task.revision
        );
        let final_result = MeshEvent {
            sequence: 2,
            assignment_id: a.assignment_id.clone(),
            body: EventBody::Message {
                message_id: "final-b".into(),
                text: "First candidate".into(),
                message_kind: Some("final".into()),
                pages: vec![],
            },
        };
        db.mesh_import(&a.executor_origin, &final_result, None)
            .unwrap();
        assert_eq!(
            db.product_task(&a.task_id).unwrap().unwrap().state,
            tasks::TaskState::Open
        );
        // A persisted historical decision remains auditable. New messages do
        // not create such a decision and are not discarded because of it.
        db.conn.lock().unwrap().execute("UPDATE product_tasks SET state='review',result_sequence=(SELECT sequence FROM visible_message_content WHERE message_id=?2) WHERE task_id=?1",params![a.task_id,format!("mesh-{}-final-b",a.assignment_id)]).unwrap();
        let review = db.product_task(&a.task_id).unwrap().unwrap();
        assert!(db
            .transition_task(&a.task_id, 0, tasks::TaskAction::Accept)
            .is_err());
        db.transition_task(&a.task_id, review.revision, tasks::TaskAction::Accept)
            .unwrap();
        let decision = db.mesh_pending_decision(&a.assignment_id).unwrap().unwrap();
        assert_eq!(decision["state"], "completed");
        let late = MeshEvent {
            sequence: 3,
            assignment_id: a.assignment_id.clone(),
            body: EventBody::Message {
                message_id: "late-b".into(),
                text: "Late candidate".into(),
                message_kind: Some("final".into()),
                pages: vec![],
            },
        };
        assert!(db.mesh_import(&a.executor_origin, &late, None).unwrap());
        let closed = db.product_task(&a.task_id).unwrap().unwrap();
        assert_eq!(closed.result_text.as_deref(), Some("First candidate"));
        assert_eq!(db.mesh_link(&a.assignment_id).unwrap().unwrap().cursor, 3);
        drop(db);
        let db =
            GatewayDb::open(&root.path().join("state"), &root.path().join("workspace")).unwrap();
        assert_eq!(
            db.mesh_pending_decision(&a.assignment_id).unwrap(),
            Some(decision)
        );
    }
    #[test]
    fn artifact_exports_precede_final_and_oversized_message_does_not_poison_journal() {
        let root = tempfile::tempdir().unwrap();
        let (db, session) = fixture(root.path());
        let a = assignment(&db, &session);
        db.mesh_receive_assignment(&a).unwrap();
        db.mesh_bind_executor(&a.assignment_id, &session).unwrap();
        let task_id = db
            .product_task_for_session(&session.key)
            .unwrap()
            .unwrap()
            .task_id;
        let file = Path::new(&session.workspace_path).join("report.md");
        std::fs::write(&file, b"Report").unwrap();
        db.register_artifact(&task_id, &file, None).unwrap();
        let submit = |id: &str, text: &str| {
            db.record_visible_message(
                id,
                &session.key,
                &session.connection_id,
                &session.channel_id,
                &session.root_thread_ts,
                "assistant",
                text,
                Some("final"),
            )
        };
        assert!(submit("oversized", &"x".repeat(zork_mesh::MAX_FRAME)).is_err());
        submit("valid", "Ready").unwrap();
        db.mesh_capture(&a.assignment_id).unwrap();
        db.mesh_capture(&a.assignment_id).unwrap();
        let events = db.mesh_events(&a.assignment_id, 0, false).unwrap();
        assert_eq!(events.len(), 2);
        assert!(matches!(events[0].body, EventBody::Artifact { .. }));
        assert!(matches!(events[1].body, EventBody::Message { .. }));
        assert!(db
            .mesh_events(&a.assignment_id, 0, true)
            .unwrap()
            .is_empty());
        let revision = db.product_task(&task_id).unwrap().unwrap().revision;
        assert!(db
            .transition_task(&task_id, revision, tasks::TaskAction::Accept)
            .is_err());
    }
}
