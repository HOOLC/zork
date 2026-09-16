//! Node-local Agent definitions and canonical session allocations.
use super::*;
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentRole {
    Leader,
    Worker,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NodeAgent {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub avatar: Option<String>,
    pub role: AgentRole,
    pub profile_id: String,
    pub model: String,
    pub thinking: String,
    pub instructions: String,
    #[serde(default)]
    pub skill_paths: Vec<std::path::PathBuf>,
    pub allowed_leaders: Vec<String>,
    pub session_key: Option<String>,
    pub session_id: Option<String>,
}
pub(super) fn initialize(conn: &Connection) -> Result<()> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS node_agents(id TEXT PRIMARY KEY,value TEXT NOT NULL,session_key TEXT UNIQUE,session_id TEXT UNIQUE); CREATE TABLE IF NOT EXISTS worker_tasks(request_id TEXT NOT NULL,leader_id TEXT NOT NULL,worker_id TEXT NOT NULL,session_key TEXT NOT NULL UNIQUE,session_id TEXT NOT NULL UNIQUE,goal TEXT NOT NULL,state TEXT NOT NULL DEFAULT 'allocated',PRIMARY KEY(leader_id,request_id)); CREATE TABLE IF NOT EXISTS leader_notifications(id TEXT PRIMARY KEY,leader_id TEXT NOT NULL,content TEXT NOT NULL,delivered INTEGER NOT NULL DEFAULT 0);")?;
    Ok(())
}
impl GatewayDb {
    pub fn agent_id_for_session(&self, session_id: &str) -> Result<Option<String>> {
        Ok(self.conn.lock().expect("db mutex").query_row(
            "SELECT id FROM node_agents WHERE session_id=?1 UNION ALL SELECT worker_id FROM worker_tasks WHERE session_id=?1
             UNION ALL SELECT a.id FROM mesh_runtime_sessions r JOIN mesh_links l ON l.assignment_id=r.assignment_id JOIN node_agents a ON a.id=json_extract(l.assignment_json,'$.worker.worker_id') WHERE r.runtime_id=?1 AND l.role='executor' LIMIT 1",
            [session_id], |r| r.get(0)).optional()?)
    }

    /// Read the execution node's current Agent definition, including Mesh Workers.
    pub fn selection_for_session(
        &self,
        session_id: &str,
    ) -> Result<Option<zork_agent::session::wire::SessionSelection>> {
        let conn = self.conn.lock().expect("db mutex");
        let value: Option<String> = conn.query_row(
            "SELECT value FROM node_agents WHERE session_id=?1
             UNION ALL SELECT a.value FROM worker_tasks w JOIN node_agents a ON a.id=w.worker_id WHERE w.session_id=?1
             UNION ALL SELECT a.value FROM mesh_runtime_sessions r
             JOIN mesh_links l ON l.assignment_id=r.assignment_id
             JOIN node_agents a ON a.id=json_extract(l.assignment_json,'$.worker.worker_id')
             WHERE r.runtime_id=?1 AND l.role='executor' LIMIT 1",
            [session_id], |r| r.get(0)).optional()?;
        value
            .map(|value| {
                let agent: NodeAgent = serde_json::from_str(&value)?;
                Ok(zork_agent::session::wire::SessionSelection {
                    profile_id: agent.profile_id,
                    model: agent.model,
                    thinking: agent.thinking,
                })
            })
            .transpose()
    }

    pub fn update_agent_skill_paths(
        &self,
        id: &str,
        paths: Vec<std::path::PathBuf>,
    ) -> Result<NodeAgent> {
        zork_config::validate_skill_paths(&paths)?;
        let mut conn = self.conn.lock().expect("db mutex");
        let tx = conn.transaction()?;
        let value: String =
            tx.query_row("SELECT value FROM node_agents WHERE id=?1", [id], |r| {
                r.get(0)
            })?;
        let mut agent: NodeAgent = serde_json::from_str(&value)?;
        agent.skill_paths = paths;
        tx.execute(
            "UPDATE node_agents SET value=?2 WHERE id=?1",
            params![id, serde_json::to_string(&agent)?],
        )?;
        tx.commit()?;
        self.flush_messages(&conn)?;
        Ok(agent)
    }

    /// Resolve on the execution node, including recovered Worker sessions.
    pub fn skill_paths_for_session(&self, session_id: &str) -> Result<Vec<std::path::PathBuf>> {
        let conn = self.conn.lock().expect("db mutex");
        let value: Option<String> = conn.query_row(
            "SELECT value FROM node_agents WHERE session_id=?1
             UNION ALL SELECT a.value FROM worker_tasks w JOIN node_agents a ON a.id=w.worker_id WHERE w.session_id=?1
             UNION ALL SELECT a.value FROM mesh_runtime_sessions r
             JOIN mesh_links l ON l.assignment_id=r.assignment_id
             JOIN node_agents a ON a.id=json_extract(l.assignment_json,'$.worker.worker_id')
             WHERE r.runtime_id=?1 AND l.role='executor' LIMIT 1",
            [session_id], |r| r.get(0)).optional()?;
        Ok(value
            .map(|value| serde_json::from_str::<NodeAgent>(&value))
            .transpose()?
            .map(|a| a.skill_paths)
            .unwrap_or_default())
    }

    /// Serialize source list edits with other updates to the same Agent definition.

    pub fn update_agent_model(
        &self,
        id: &str,
        profile: &str,
        model: &str,
        thinking: &str,
    ) -> Result<NodeAgent> {
        let mut conn = self.conn.lock().expect("db mutex");
        let tx = conn.transaction()?;
        let value: String =
            tx.query_row("SELECT value FROM node_agents WHERE id=?1", [id], |r| {
                r.get(0)
            })?;
        let mut agent: NodeAgent = serde_json::from_str(&value)?;
        agent.profile_id = profile.to_owned();
        agent.model = model.to_owned();
        agent.thinking = thinking.to_owned();
        tx.execute(
            "UPDATE node_agents SET value=?2 WHERE id=?1",
            params![id, serde_json::to_string(&agent)?],
        )?;
        tx.execute(
            "UPDATE sessions SET profile_id=?2,model=?3,thinking=?4 WHERE id IN (
             SELECT session_id FROM node_agents WHERE id=?1
             UNION ALL SELECT session_id FROM worker_tasks WHERE worker_id=?1
             UNION ALL SELECT r.runtime_id FROM mesh_runtime_sessions r
             JOIN mesh_links l ON l.assignment_id=r.assignment_id
             WHERE l.role='executor' AND json_extract(l.assignment_json,'$.worker.worker_id')=?1)",
            params![id, profile, model, thinking],
        )?;
        tx.commit()?;
        self.flush_messages(&conn)?;
        Ok(agent)
    }

    /// A human comment and its Leader inbox item commit together. Comments do
    /// not start a Worker turn or alter a Task's review state.
    #[cfg(test)]
    pub fn record_task_comment(
        &self,
        session: &SessionRow,
        message_id: &str,
        text: &str,
    ) -> Result<VisibleMessageRow> {
        self.record_task_comment_with_input(session, message_id, text, text)
    }
    #[cfg(test)]
    pub fn record_task_comment_with_input(
        &self,
        session: &SessionRow,
        message_id: &str,
        text: &str,
        agent_input: &str,
    ) -> Result<VisibleMessageRow> {
        let mut conn = self.conn.lock().expect("db mutex");
        let tx = conn.transaction()?;
        let (leader, task, state): (String,String,String) = tx.query_row(
            "SELECT w.leader_id,t.task_id,t.state FROM worker_tasks w JOIN product_tasks t ON t.session_key=w.session_key JOIN node_agents a ON a.id=w.leader_id WHERE w.session_key=?1",
            [&session.key], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
        anyhow::ensure!(
            !matches!(state.as_str(), "completed" | "cancelled"),
            "task_closed_reopen_required"
        );
        let existing: Option<(String, String)> = tx
            .query_row(
                "SELECT session_key,text FROM visible_message_content WHERE message_id=?1",
                [message_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((key, content)) = existing {
            anyhow::ensure!(
                key == session.key && content == text,
                "request_id already has different content"
            );
        } else {
            let now = now_rfc3339();
            tx.execute("INSERT INTO visible_messages(message_id,session_key,connection_id,conversation_id,root_message_id,role,text,kind,created_at) VALUES (?1,?2,?3,?4,?5,'user',?6,'comment',?7)", params![message_id,session.key,session.connection_id,session.channel_id,session.root_thread_ts,text,now])?;
            let content = format!(
                "Human comment on Task {task} (message {message_id}):\n{agent_input}\n\nReview this comment in the context of the Task. Use agent.tasks and agent.rework if further Worker work is needed."
            );
            tx.execute(
                "INSERT INTO leader_notifications(id,leader_id,content) VALUES (?1,?2,?3)",
                params![format!("comment-{message_id}"), leader, content],
            )?;
            tx.execute(
                "UPDATE sessions SET updated_at=?2 WHERE key=?1",
                params![session.key, now],
            )?;
        }
        let message = tx.query_row("SELECT sequence,message_id,session_key,role,text,kind,created_at FROM visible_message_content WHERE message_id=?1", [message_id], map_visible_message_row)?;
        tx.commit()?;
        self.flush_messages(&conn)?;
        Ok(message)
    }

    pub fn conversation_read_markers(&self) -> Result<Vec<Value>> {
        let conn = self.conn.lock().expect("db mutex");
        // Match ImEntryService::message_json: assignment/rework IDs belong to
        // the owning Leader only when a canonical Worker Task ownership exists.
        // GLOB is case-sensitive, like Rust starts_with; human client- IDs stay user.
        let mut stmt = conn.prepare(
            "SELECT s.id,m.message_id,m.created_at,
                CASE WHEN m.role='user'
                    AND (m.message_id GLOB 'assignment-*' OR m.message_id GLOB 'rework-*')
                    AND EXISTS(SELECT 1 FROM worker_tasks w JOIN product_tasks t
                        ON t.session_key=w.session_key WHERE w.session_key=s.key)
                THEN 'assistant' ELSE m.role END
             FROM sessions s JOIN visible_message_content m
                ON m.sequence=(SELECT MAX(v.sequence) FROM visible_message_content v WHERE v.session_key=s.key)
             WHERE s.platform='local_gui' AND s.id IS NOT NULL",
        )?;
        let rows = stmt.query_map([], |r| Ok(json!({"session_id":r.get::<_,String>(0)?,"last_message_id":r.get::<_,String>(1)?,"created_at":r.get::<_,String>(2)?,"role":r.get::<_,String>(3)?})))?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn update_agent_avatar(&self, id: &str, avatar: &str) -> Result<NodeAgent> {
        let mut conn = self.conn.lock().expect("db mutex");
        let tx = conn.transaction()?;
        let value: String =
            tx.query_row("SELECT value FROM node_agents WHERE id=?1", [id], |r| {
                r.get(0)
            })?;
        let mut agent: NodeAgent = serde_json::from_str(&value)?;
        agent.avatar = Some(avatar.to_owned());
        tx.execute(
            "UPDATE node_agents SET value=?2 WHERE id=?1",
            params![id, serde_json::to_string(&agent)?],
        )?;
        tx.commit()?;
        self.flush_messages(&conn)?;
        Ok(agent)
    }
    pub fn update_worker_grants(
        &self,
        id: &str,
        expected: &[String],
        grants: &[String],
    ) -> Result<NodeAgent> {
        let mut conn = self.conn.lock().expect("db mutex");
        let tx = conn.transaction()?;
        let value: String =
            tx.query_row("SELECT value FROM node_agents WHERE id=?1", [id], |r| {
                r.get(0)
            })?;
        let mut agent: NodeAgent = serde_json::from_str(&value)?;
        anyhow::ensure!(
            agent.role == AgentRole::Worker,
            "Only Worker grants can be changed"
        );
        anyhow::ensure!(
            agent.allowed_leaders == expected,
            "Worker grants changed; refresh and try again"
        );
        agent.allowed_leaders = grants.to_vec();
        tx.execute(
            "UPDATE node_agents SET value=?2 WHERE id=?1",
            params![id, serde_json::to_string(&agent)?],
        )?;
        tx.commit()?;
        self.flush_messages(&conn)?;
        Ok(agent)
    }
    pub fn node_agents(&self) -> Result<Vec<NodeAgent>> {
        let conn = self.conn.lock().expect("db mutex");
        let rows = conn
            .prepare("SELECT value FROM node_agents ORDER BY rowid")?
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .map(|s| Ok(serde_json::from_str(&s)?))
            .collect()
    }
    pub fn node_agent(&self, id: &str) -> Result<Option<NodeAgent>> {
        let value: Option<String> = self
            .conn
            .lock()
            .expect("db mutex")
            .query_row("SELECT value FROM node_agents WHERE id=?1", [id], |r| {
                r.get(0)
            })
            .optional()?;
        value
            .map(|v| serde_json::from_str(&v).map_err(Into::into))
            .transpose()
    }
    pub fn agent_for_session(&self, key: &str) -> Result<Option<NodeAgent>> {
        let value: Option<String> = self
            .conn
            .lock()
            .expect("db mutex")
            .query_row(
                "SELECT value FROM node_agents WHERE session_key=?1",
                [key],
                |r| r.get(0),
            )
            .optional()?;
        value
            .map(|v| serde_json::from_str(&v).map_err(Into::into))
            .transpose()
    }
    /// Insert the Agent before creating its binding. Its role is authoritative
    /// even if Gateway stops between allocation and runtime acknowledgement.
    pub fn insert_node_agent(&self, agent: &NodeAgent) -> Result<()> {
        self.conn.lock().expect("db mutex").execute(
            "INSERT INTO node_agents(id,value,session_key,session_id) VALUES (?1,?2,?3,?4)",
            params![
                agent.id,
                serde_json::to_string(agent)?,
                agent.session_key,
                agent.session_id
            ],
        )?;
        Ok(())
    }
    pub fn worker_task_allocation(
        &self,
        leader: &str,
        request: &str,
        worker: &str,
        goal: &str,
    ) -> Result<(String, String, String)> {
        let conn = self.conn.lock().expect("db mutex");
        let existing=conn.query_row("SELECT session_key,session_id,state,worker_id,goal FROM worker_tasks WHERE leader_id=?1 AND request_id=?2",params![leader,request],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?))).optional()?;
        if let Some((key, id, state, existing_worker, existing_goal)) = existing {
            anyhow::ensure!(
                existing_worker == worker && existing_goal == goal,
                "request_id already has a different assignment"
            );
            return Ok((key, id, state));
        }
        let conversation = format!("worker-task-{}", ulid::Ulid::new());
        let key = format!("local_gui:{conversation}:{conversation}");
        let id = ulid::Ulid::new().to_string();
        conn.execute("INSERT INTO worker_tasks(request_id,leader_id,worker_id,session_key,session_id,goal) VALUES (?1,?2,?3,?4,?5,?6)",params![request,leader,worker,key,id,goal])?;
        Ok((key, id, "allocated".into()))
    }
    pub fn set_worker_task_state(&self, key: &str, state: &str) -> Result<()> {
        self.conn.lock().expect("db mutex").execute(
            "UPDATE worker_tasks SET state=?1 WHERE session_key=?2",
            params![state, key],
        )?;
        Ok(())
    }
    pub fn ensure_product_task(&self, key: &str) -> Result<()> {
        tasks::ensure_task(&self.conn.lock().expect("db mutex"), key)
    }
}

impl GatewayDb {
    pub fn tasks_for_leader(&self, leader: &str) -> Result<Vec<super::tasks::ProductTask>> {
        let keys = {
            let conn = self.conn.lock().expect("db mutex");
            let rows = conn
                .prepare("SELECT session_key FROM worker_tasks WHERE leader_id=?1")?
                .query_map([leader], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows
        };
        keys.into_iter()
            .filter_map(|key| self.product_task_for_session(&key).transpose())
            .collect()
    }
}

impl GatewayDb {
    pub fn pending_leader_notifications(&self) -> Result<Vec<(String, String, String)>> {
        let conn = self.conn.lock().expect("db mutex");
        let rows=conn.prepare("SELECT id,leader_id,content FROM leader_notifications WHERE delivered=0 ORDER BY rowid LIMIT 32")?.query_map([],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }
    pub fn finish_leader_notification(&self, id: &str) -> Result<()> {
        self.conn.lock().expect("db mutex").execute(
            "UPDATE leader_notifications SET delivered=1 WHERE id=?1",
            [id],
        )?;
        Ok(())
    }
    pub fn pending_worker_tasks(&self) -> Result<Vec<(String, String, String, String)>> {
        let conn = self.conn.lock().expect("db mutex");
        let rows=conn.prepare("SELECT leader_id,request_id,worker_id,goal FROM worker_tasks WHERE state!='sent' ORDER BY rowid LIMIT 32")?.query_map([],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }
    pub fn worker_task_owner(&self, task_id: &str) -> Result<Option<(String, String, String)>> {
        Ok(self.conn.lock().expect("db mutex").query_row("SELECT w.leader_id,w.worker_id,w.session_key FROM worker_tasks w JOIN product_tasks t ON w.session_key=t.session_key WHERE t.task_id=?1",[task_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?)
    }
}
impl GatewayDb {
    pub fn has_message_receipt(&self, id: &str, key: &str, content: &str) -> Result<bool> {
        let conn = self.conn.lock().expect("db mutex");
        let previous: Option<(String, String)> = conn
            .query_row(
                "SELECT session_key,text FROM visible_message_content WHERE message_id=?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((previous_key, previous_content)) = previous {
            anyhow::ensure!(
                previous_key == key && previous_content == content,
                "request_id already has different content"
            );
            return Ok(true);
        }
        Ok(false)
    }
}

#[cfg(test)]
mod avatar_tests {
    use super::NodeAgent;
    #[test]
    fn existing_agent_definition_without_avatar_remains_readable() {
        let agent: NodeAgent = serde_json::from_value(serde_json::json!({
            "id":"legacy", "name":"Legacy", "role":"leader", "profile_id":"profile",
            "model":"model", "thinking":"off", "instructions":"", "allowed_leaders":[],
            "session_key":"key", "session_id":"session"
        }))
        .unwrap();
        assert!(agent.avatar.is_none());
        assert_eq!(agent.session_id.as_deref(), Some("session"));
    }
}

#[cfg(test)]
mod gui_contract_tests {
    use super::*;
    fn definition(id: &str, role: AgentRole) -> NodeAgent {
        NodeAgent {
            id: id.into(),
            name: id.into(),
            avatar: Some("fox".into()),
            role,
            profile_id: "old".into(),
            model: "old-model".into(),
            thinking: "high".into(),
            instructions: "keep instructions".into(),
            skill_paths: Vec::new(),
            allowed_leaders: vec!["leader".into()],
            session_key: None,
            session_id: None,
        }
    }
    #[test]
    fn remote_worker_skill_paths_resolve_on_executor_after_recovery() {
        use crate::db::mesh::{Assignment, WorkerTarget};
        let dir = tempfile::tempdir().unwrap();
        let db = GatewayDb::open(dir.path(), &dir.path().join("workspaces")).unwrap();
        db.insert_node_agent(&definition("worker", AgentRole::Worker))
            .unwrap();
        db.update_agent_skill_paths("worker", vec!["executor-only".into()])
            .unwrap();
        let assignment = Assignment {
            assignment_id: "assignment".into(),
            task_id: "task".into(),
            owner_origin: "owner".into(),
            executor_origin: "executor".into(),
            workspace_id: "worker-workspace".into(),
            goal: "goal".into(),
            worker: Some(WorkerTarget {
                leader_id: "leader".into(),
                worker_id: "worker".into(),
            }),
        };
        db.mesh_receive_assignment(&assignment).unwrap();
        let session = db.mesh_runtime_id("assignment").unwrap();
        db.update_agent_model("worker", "new", "remote-new-model", "off")
            .unwrap();
        assert_eq!(
            db.skill_paths_for_session(&session).unwrap(),
            vec![std::path::PathBuf::from("executor-only")]
        );
        drop(db);
        let db = GatewayDb::open(dir.path(), &dir.path().join("workspaces")).unwrap();
        assert_eq!(
            db.selection_for_session(&session).unwrap().unwrap().model,
            "remote-new-model"
        );
        assert_eq!(
            db.skill_paths_for_session(&session).unwrap(),
            vec![std::path::PathBuf::from("executor-only")]
        );
        db.update_agent_skill_paths(
            "worker",
            vec!["executor-only".into(), "remote-extra".into()],
        )
        .unwrap();
        assert_eq!(
            db.skill_paths_for_session(&session).unwrap(),
            vec![
                std::path::PathBuf::from("executor-only"),
                std::path::PathBuf::from("remote-extra")
            ]
        );
        db.update_agent_skill_paths("worker", vec![]).unwrap();
        assert!(db.skill_paths_for_session(&session).unwrap().is_empty());
    }

    #[test]
    fn skill_paths_persist_and_worker_sessions_use_executor_configuration() {
        let dir = tempfile::tempdir().unwrap();
        let db = GatewayDb::open(dir.path(), &dir.path().join("workspaces")).unwrap();
        let mut leader = definition("leader", AgentRole::Leader);
        leader.session_id = Some("leader-session".into());
        leader.session_key = Some("leader-key".into());
        db.insert_node_agent(&leader).unwrap();
        db.insert_node_agent(&definition("worker", AgentRole::Worker))
            .unwrap();
        db.update_agent_skill_paths("leader", vec!["leader-only".into()])
            .unwrap();
        db.update_agent_skill_paths("worker", vec!["device/skills".into()])
            .unwrap();
        let (_, worker_session, _) = db
            .worker_task_allocation("leader", "request", "worker", "goal")
            .unwrap();
        assert_eq!(
            db.skill_paths_for_session(&worker_session).unwrap(),
            vec![std::path::PathBuf::from("device/skills")]
        );
        assert_eq!(
            db.skill_paths_for_session("leader-session").unwrap(),
            vec![std::path::PathBuf::from("leader-only")]
        );
        assert!(db.skill_paths_for_session("unknown").unwrap().is_empty());
        assert!(db
            .update_agent_skill_paths("worker", vec!["".into()])
            .is_err());
        drop(db);
        let db = GatewayDb::open(dir.path(), &dir.path().join("workspaces")).unwrap();
        assert_eq!(
            db.skill_paths_for_session(&worker_session).unwrap(),
            vec![std::path::PathBuf::from("device/skills")]
        );
        let updated = db
            .update_agent_skill_paths("worker", vec!["updated".into()])
            .unwrap();
        assert_eq!(updated.instructions, "keep instructions");
        assert_eq!(
            db.skill_paths_for_session(&worker_session).unwrap(),
            vec![std::path::PathBuf::from("updated")]
        );
    }

    #[test]
    fn model_change_updates_worker_selection_and_keeps_identity_grants_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let db = GatewayDb::open(dir.path(), &dir.path().join("workspaces")).unwrap();
        let worker = definition("worker", AgentRole::Worker);
        db.insert_node_agent(&worker).unwrap();
        let (key, id, _) = db
            .worker_task_allocation("leader", "request", "worker", "goal")
            .unwrap();
        let channel = key.split(':').nth(1).unwrap();
        let session = db
            .ensure_session(EnsureSession {
                connection_id: "local_gui",
                platform: "local_gui",
                channel_id: channel,
                root_thread_ts: channel,
                channel_type: Some("worker_task"),
                initiator_user_id: None,
                initiator_message_ts: None,
            })
            .unwrap();
        db.set_agent_session(
            &key,
            &id,
            &session.workspace_path,
            "old",
            "old-model",
            "high",
        )
        .unwrap();
        db.update_agent_avatar("worker", "owl").unwrap();
        let updated = db
            .update_agent_model("worker", "new", "new-model", "off")
            .unwrap();
        assert_eq!(updated.avatar.as_deref(), Some("owl"));
        assert_eq!(updated.allowed_leaders, worker.allowed_leaders);
        assert_eq!(updated.instructions, worker.instructions);
        assert!(updated.session_id.is_none());
        let existing = db.get_session(&key).unwrap().unwrap();
        assert_eq!(existing.model.as_deref(), Some("new-model"));
        let selection = db.selection_for_session(&id).unwrap().unwrap();
        assert_eq!(selection.model, "new-model");
        assert_eq!(selection.profile_id, "new");
        assert_eq!(selection.thinking, "off");
        assert!(db.selection_for_session("unowned").unwrap().is_none());
        assert_eq!(existing.workspace_path, session.workspace_path);
    }
    #[test]
    fn task_comment_is_atomic_idempotent_and_only_queues_its_own_leader() {
        let dir = tempfile::tempdir().unwrap();
        let db = GatewayDb::open(dir.path(), &dir.path().join("workspaces")).unwrap();
        db.insert_node_agent(&definition("leader", AgentRole::Leader))
            .unwrap();
        let (key, id, _) = db
            .worker_task_allocation("leader", "request", "remote/worker", "goal")
            .unwrap();
        let channel = key.split(':').nth(1).unwrap();
        let session = db
            .ensure_session(EnsureSession {
                connection_id: "local_gui",
                platform: "local_gui",
                channel_id: channel,
                root_thread_ts: channel,
                channel_type: Some("worker_task"),
                initiator_user_id: None,
                initiator_message_ts: None,
            })
            .unwrap();
        db.set_agent_session(
            &key,
            &id,
            &session.workspace_path,
            "profile",
            "model",
            "off",
        )
        .unwrap();
        db.ensure_product_task(&key).unwrap();
        let before = db.product_task_for_session(&key).unwrap().unwrap();
        let mut revisions = db.realtime.subscribe();
        revisions.borrow_and_update();
        let first = db
            .record_task_comment(&session, "comment-1", "please explain this result")
            .unwrap();
        let duplicate = db
            .record_task_comment(&session, "comment-1", "please explain this result")
            .unwrap();
        assert_eq!(first.sequence, duplicate.sequence);
        assert!(db
            .record_task_comment(&session, "comment-1", "different")
            .is_err());
        let after = db.product_task_for_session(&key).unwrap().unwrap();
        assert_eq!(before.state, after.state);
        assert_eq!(before.revision, after.revision);
        let pending = db.pending_leader_notifications().unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].1, "leader");
        assert!(pending[0].2.contains(&before.task_id));
        assert_eq!(db.list_visible_messages(&key, None, 100).unwrap().len(), 1);
        let markers = db.conversation_read_markers().unwrap();
        assert_eq!(markers[0]["session_id"], id);
        assert_eq!(markers[0]["last_message_id"], first.message_id);
        assert_eq!(markers[0]["created_at"], first.created_at);
        assert!(revisions.has_changed().unwrap());
        db.conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE product_tasks SET state='completed' WHERE session_key=?1",
                [&key],
            )
            .unwrap();
        assert!(db
            .record_task_comment(&session, "comment-2", "cannot reopen by comment")
            .is_err());
        assert_eq!(db.pending_leader_notifications().unwrap().len(), 1);
    }
    #[test]
    fn read_markers_project_only_owned_leader_assignment_and_rework_ids() {
        let dir = tempfile::tempdir().unwrap();
        let db = GatewayDb::open(dir.path(), &dir.path().join("workspaces")).unwrap();
        db.insert_node_agent(&definition("leader", AgentRole::Leader))
            .unwrap();
        let (key, id, _) = db
            .worker_task_allocation("leader", "request", "worker", "goal")
            .unwrap();
        let channel = key.split(':').nth(1).unwrap();
        let session = db
            .ensure_session(EnsureSession {
                connection_id: "local_gui",
                platform: "local_gui",
                channel_id: channel,
                root_thread_ts: channel,
                channel_type: Some("worker_task"),
                initiator_user_id: None,
                initiator_message_ts: None,
            })
            .unwrap();
        db.set_agent_session(
            &key,
            &id,
            &session.workspace_path,
            "profile",
            "model",
            "off",
        )
        .unwrap();
        db.ensure_product_task(&key).unwrap();
        for (message_id, expected_role) in [
            ("assignment-task", "assistant"),
            ("rework-task", "assistant"),
            ("client-comment", "user"),
            ("Assignment-case-sensitive", "user"),
        ] {
            db.record_visible_message(
                message_id,
                &key,
                "local_gui",
                channel,
                channel,
                "user",
                "same text",
                None,
            )
            .unwrap();
            let markers = db.conversation_read_markers().unwrap();
            let marker = markers.iter().find(|m| m["session_id"] == id).unwrap();
            assert_eq!(marker["last_message_id"], message_id);
            assert_eq!(marker["role"], expected_role);
            assert_eq!(
                db.list_visible_messages(&key, None, 100)
                    .unwrap()
                    .last()
                    .unwrap()
                    .role,
                "user"
            );
        }
        let orphan = db
            .ensure_session(EnsureSession {
                connection_id: "local_gui",
                platform: "local_gui",
                channel_id: "orphan",
                root_thread_ts: "orphan",
                channel_type: None,
                initiator_user_id: None,
                initiator_message_ts: None,
            })
            .unwrap();
        db.set_agent_session(
            &orphan.key,
            "orphan-runtime",
            &orphan.workspace_path,
            "profile",
            "model",
            "off",
        )
        .unwrap();
        db.record_visible_message(
            "assignment-without-owner",
            &orphan.key,
            "local_gui",
            "orphan",
            "orphan",
            "user",
            "ordinary user",
            None,
        )
        .unwrap();
        let markers = db.conversation_read_markers().unwrap();
        assert_eq!(
            markers
                .iter()
                .find(|m| m["session_id"] == "orphan-runtime")
                .unwrap()["role"],
            "user"
        );
    }
}
