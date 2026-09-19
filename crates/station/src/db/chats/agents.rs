use super::*;
use crate::db::agents::NodeAgent;

pub fn configuration_revision(agent: &NodeAgent) -> Result<String> {
    Ok(blake3::hash(&serde_json::to_vec(
        &json!({"name":agent.name,"avatar":agent.avatar,
        "profile_id":agent.profile_id,"model":agent.model,"thinking":agent.thinking,
        "instructions":agent.instructions,"skill_paths":agent.skill_paths,"role":agent.role,
        "allowed_leaders":agent.allowed_leaders}),
    )?)
    .to_hex()
    .to_string())
}

impl StationDb {
    pub fn client_agents(&self) -> Result<Vec<Value>> {
        self.node_agents()?
            .into_iter()
            .map(|agent| {
                let home = self.agent_home(&agent.id)?;
                let mut value = serde_json::to_value(&agent)?;
                value["session_id"] = json!(home.map(|h| h.chat_id));
                value["revision"] = json!(configuration_revision(&agent)?);
                Ok(value)
            })
            .collect()
    }
    pub fn agent_home(&self, id: &str) -> Result<Option<Channel>> {
        let conn = self.conn.lock().expect("db mutex");
        let chat:Option<String>=conn.query_row("SELECT chat_id FROM chat_agent_home WHERE agent_id=?1 UNION ALL SELECT c.chat_id FROM chat_channels c JOIN node_agents a ON a.session_key=c.session_key WHERE a.id=?1 LIMIT 1",[id],|r|r.get(0)).optional()?;
        chat.map(|id| {
            conn.query_row(
                &format!("{CHANNEL_SELECT} WHERE chat_id=?1"),
                [id],
                map_channel,
            )
            .map(|r| r.channel)
            .map_err(Into::into)
        })
        .transpose()
    }

    pub fn set_agent_home(&self, id: &str, chat: &str) -> Result<()> {
        self.conn.lock().expect("db mutex").execute(
            "INSERT OR IGNORE INTO chat_agent_home VALUES(?1,?2)",
            params![id, chat],
        )?;
        Ok(())
    }

    pub fn direct_agent_inputs(
        &self,
        agent: &str,
    ) -> Result<Vec<(i64, String, String, String, String)>> {
        let conn = self.conn.lock().expect("db mutex");
        let rows = conn.prepare("SELECT q.sequence,q.agent_id,q.author,q.content,(SELECT value FROM chat_metadata WHERE key='epoch') FROM chat_direct_inputs q WHERE q.delivered=0 AND q.agent_id=?1 ORDER BY q.sequence LIMIT 1")?
            .query_map([agent],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?)))?.collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    pub fn pending_chat_agents(&self) -> Result<Vec<String>> {
        let conn = self.conn.lock().expect("db mutex");
        let rows=conn.prepare("SELECT agent_id FROM chat_direct_inputs WHERE delivered=0 UNION SELECT agent_id FROM chat_mailbox WHERE delivered=0")?
            .query_map([],|r|r.get(0))?.collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }
    pub fn finish_direct_agent_input(&self, sequence: i64) -> Result<()> {
        self.conn.lock().expect("db mutex").execute(
            "UPDATE chat_direct_inputs SET delivered=1 WHERE sequence=?1 AND delivered=0",
            [sequence],
        )?;
        Ok(())
    }
    pub fn agent_configuration(
        &self,
        session: &str,
    ) -> Result<Option<zork_agent::session::runner::Configuration>> {
        let Some(id) = self.agent_id_for_session(session)? else {
            return Ok(None);
        };
        let agent = self.node_agent(&id)?.context("agent_not_found")?;
        Ok(Some(zork_agent::session::runner::Configuration {
            revision: configuration_revision(&agent)?,
            selection: zork_agent::session::wire::SessionSelection {
                profile_id: agent.profile_id.clone(),
                model: agent.model.clone(),
                thinking: agent.thinking.clone(),
            },
            system_prompt: Some(crate::node::agent_prompt(&agent)),
            end_turn_confirmation: Some(crate::agent::END_TURN_CONFIRMATION.into()),
        }))
    }

    pub(crate) fn save_channel_agent(
        &self,
        key: &str,
        agent: &NodeAgent,
        expected: Option<&str>,
        request: Option<&str>,
    ) -> Result<Value> {
        let mut conn = self.conn.lock().expect("db mutex");
        let tx = conn.transaction()?;
        command_active(&tx, key)?;
        if let Some(id) = request {
            crate::db::interaction_registry::require_active(&tx, id, agent_configuration::HANDLER)?;
        }
        let result = save_agent(&tx, agent, expected)?;
        finish(&tx, key, &result)?;
        let topics = if let Some(id) = request {
            let mut record = cards::read(&tx, id)?;
            let old = record.result.as_ref().context("interaction_not_accepted")?;
            anyhow::ensure!(
                old.outcome == zork_client_types::interaction::Outcome::Pending,
                "interaction_settled"
            );
            let resolution = zork_client_types::interaction::Resolution {
                request_message_id: String::new(),
                response_id: old.response_id.clone(),
                revision: old.revision + 1,
                outcome: zork_client_types::interaction::Outcome::Completed,
                actor: old.actor.clone(),
                output: result.clone(),
            };
            cards::resolve(&tx, &mut record, resolution)?
        } else {
            vec![]
        };
        tx.commit()?;
        self.flush_messages(&conn)?;
        self.chat_topics.publish(topics);
        Ok(result)
    }

    /// Allocate one Agent control context, independent of its channel subscriptions.
    /// Old canonical contexts remain in place; Worker histories stay readable.
    pub fn allocate_channel_agent(&self, id: &str) -> Result<NodeAgent> {
        let mut conn = self.conn.lock().expect("db mutex");
        let tx = conn.transaction()?;
        let value: String = tx
            .query_row("SELECT value FROM node_agents WHERE id=?1", [id], |r| {
                r.get(0)
            })
            .context("agent_not_found")?;
        let mut agent: NodeAgent = serde_json::from_str(&value)?;
        if agent.session_id.is_none() {
            let conversation = format!("agent-{id}");
            agent.session_key = Some(format!("local_gui:{conversation}:{conversation}"));
            agent.session_id = Some(ulid::Ulid::new().to_string());
            tx.execute(
                "UPDATE node_agents SET value=?2,session_key=?3,session_id=?4 WHERE id=?1",
                params![
                    id,
                    serde_json::to_string(&agent)?,
                    agent.session_key,
                    agent.session_id
                ],
            )?;
        }
        tx.commit()?;
        self.flush_messages(&conn)?;
        Ok(agent)
    }

    pub fn channel_agent_sessions(&self, id: &str) -> Result<Vec<String>> {
        Ok(self.conn.lock().expect("db mutex").prepare("SELECT session_id FROM node_agents WHERE id=?1 AND session_id IS NOT NULL
            UNION SELECT session_id FROM worker_tasks WHERE worker_id=?1
            UNION SELECT r.runtime_id FROM mesh_runtime_sessions r JOIN mesh_links l ON l.assignment_id=r.assignment_id WHERE l.role='executor' AND json_extract(l.assignment_json,'$.worker.worker_id')=?1")?
            .query_map([id],|r|r.get(0))?.collect::<rusqlite::Result<_>>()?)
    }
}

/// Agent configuration has one write contract, usable inside a larger message
/// transaction as well as by the existing management tools.
pub(super) fn save_agent(
    conn: &Connection,
    agent: &NodeAgent,
    expected: Option<&str>,
) -> Result<Value> {
    let previous: Option<String> = conn
        .query_row(
            "SELECT value FROM node_agents WHERE id=?1",
            [&agent.id],
            |r| r.get(0),
        )
        .optional()?;
    let encoded = serde_json::to_string(agent)?;
    if let Some(previous) = previous {
        let prior: NodeAgent = serde_json::from_str(&previous)?;
        anyhow::ensure!(
            expected == Some(configuration_revision(&prior)?.as_str()),
            "agent_configuration_conflict"
        );
        anyhow::ensure!(
            agent.session_id == prior.session_id && agent.session_key == prior.session_key,
            "agent_allocation_conflict"
        );
        conn.execute(
            "UPDATE node_agents SET value=?2 WHERE id=?1 AND value!=?2",
            params![agent.id, encoded],
        )?;
    } else {
        anyhow::ensure!(expected.is_none(), "agent_not_found");
        let count: u64 = conn.query_row("SELECT COUNT(*) FROM node_agents", [], |r| r.get(0))?;
        anyhow::ensure!(count < 64, "agent_limit");
        conn.execute(
            "INSERT INTO node_agents(id,value,session_key,session_id) VALUES(?1,?2,?3,?4)",
            params![agent.id, encoded, agent.session_key, agent.session_id],
        )?;
    }
    Ok(json!({"agent":agent,"revision":configuration_revision(agent)?}))
}
