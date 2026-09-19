//! Read-only links from a public Chat to an explicitly allocated execution.
use super::*;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct WorkNotice {
    pub assignment_id: String,
    /// The durable assignment path already accepts this initial input.
    pub initial: bool,
}

pub(super) fn notice(conn: &Connection, agent: &str, message: &str) -> Result<Option<WorkNotice>> {
    Ok(conn
        .query_row(
            "SELECT w.session_id,w.leader_id,w.request_id FROM worker_tasks w
        JOIN chat_channels c ON c.session_key=w.session_key
        JOIN chat_message_facts f ON f.chat_id=c.chat_id
        WHERE f.message_id=?1 AND w.worker_id=?2",
            params![message, agent],
            |r| {
                let session: String = r.get(0)?;
                let creator: String = r.get(1)?;
                let request: String = r.get(2)?;
                let assignment_id = format!("worker-{session}");
                Ok(WorkNotice {
                    initial: message == format!("assignment-{creator}-{request}")
                        || message == format!("mesh-{assignment_id}-goal"),
                    assignment_id,
                })
            },
        )
        .optional()?)
}

impl StationDb {
    /// Resolve an omitted publication destination from the actual execution,
    /// never from another assignment or a Worker's optional home channel.
    pub fn session_chat_destination(&self, runtime: &str) -> Result<Option<(String, String)>> {
        let Some(binding) = self.get_binding_by_id(runtime)? else {
            return Ok(None);
        };
        let conn = self.conn.lock().expect("db mutex");
        let remote: Option<String> = conn
            .query_row(
                "SELECT assignment_json FROM mesh_links WHERE role='executor' AND session_key=?1",
                [binding.key()],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(remote) = remote {
            let assignment: crate::db::mesh::Assignment = serde_json::from_str(&remote)?;
            return Ok(assignment
                .assignment_id
                .strip_prefix("worker-")
                .filter(|_| assignment.worker.is_some())
                .map(|chat| (assignment.owner_origin.clone(), chat.to_owned())));
        }
        let assigned: Option<String> = conn.query_row(
            "SELECT c.chat_id FROM worker_tasks w JOIN chat_channels c ON c.session_key=w.session_key WHERE w.session_key=?1",
            [binding.key()], |row| row.get(0),
        ).optional()?;
        if let Some(chat) = assigned {
            return Ok(Some(("local".into(), chat)));
        }
        drop(conn);
        if let Some(agent) = self.agent_id_for_session(runtime)? {
            return Ok(self
                .agent_home(&agent)?
                .map(|chat| ("local".into(), chat.chat_id)));
        }
        let conn = self.conn.lock().expect("db mutex");
        Ok(conn
            .query_row(
                "SELECT chat_id FROM chat_channels WHERE session_key=?1",
                [binding.key()],
                |row| row.get(0),
            )
            .optional()?
            .map(|chat| ("local".into(), chat)))
    }

    /// Returns only a recorded allocation. Calling this cannot allocate a runtime.
    pub fn chat_execution(
        &self,
        agent: &str,
        target: &str,
        chat: &str,
    ) -> Result<Option<SessionRow>> {
        let key: Option<String> = {
            let conn = self.conn.lock().expect("db mutex");
            if target == "local" {
                conn.query_row("SELECT w.session_key FROM worker_tasks w JOIN chat_channels c ON c.session_key=w.session_key
                    WHERE w.worker_id=?1 AND c.chat_id=?2", params![agent,chat], |r| r.get(0)).optional()?
            } else {
                // Worker assignment ids have always carried the owner session id.
                // Keep this alias so existing Mesh assignments need no wire rewrite.
                conn.query_row(
                    "SELECT l.session_key FROM mesh_links l WHERE l.role='executor'
                    AND l.assignment_id=?3 AND json_extract(l.assignment_json,'$.owner_origin')=?2
                    AND json_extract(l.assignment_json,'$.worker.worker_id')=?1",
                    params![agent, target, format!("worker-{chat}")],
                    |r| r.get(0),
                )
                .optional()?
                .flatten()
            }
        };
        key.map(|key| self.get_session(&key))
            .transpose()
            .map(Option::flatten)
    }

    pub fn chat_home_agent(&self, chat: &str) -> Result<Option<String>> {
        let conn = self.conn.lock().expect("db mutex");
        Ok(conn.query_row("SELECT a.id FROM node_agents a JOIN chat_agent_home h ON h.agent_id=a.id WHERE h.chat_id=?1
            UNION ALL SELECT a.id FROM node_agents a JOIN chat_channels c ON c.session_key=a.session_key WHERE c.chat_id=?1 LIMIT 1",
            [chat], |r| r.get(0)).optional()?)
    }

    pub fn chat_executor(&self, chat: &str) -> Result<Option<String>> {
        let conn = self.conn.lock().expect("db mutex");
        Ok(conn.query_row("SELECT w.worker_id FROM worker_tasks w JOIN chat_channels c ON c.session_key=w.session_key
            WHERE c.chat_id=?1", [chat], |r| r.get(0)).optional()?)
    }
}
