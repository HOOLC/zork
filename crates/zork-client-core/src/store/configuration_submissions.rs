//! Durable submissions owned by Agent configuration review operations.
use super::*;
use crate::interactions::{Response, Submission};
use anyhow::{ensure, Context};
use std::collections::HashMap;

#[derive(Clone)]
pub(crate) struct ConfigurationDelivery {
    pub session: String,
    pub message_id: String,
    pub generation: u64,
    pub submission: Submission,
}

pub(super) fn initialize(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS agent_configuration_outbox(
        node TEXT NOT NULL, session TEXT NOT NULL, message_id TEXT NOT NULL,
        generation INTEGER NOT NULL, value TEXT NOT NULL,
        PRIMARY KEY(node,session,message_id));",
    )?;
    conn.execute("UPDATE agent_configuration_outbox SET value=json_set(value,'$.error','Submission interrupted; recover the original response.')
        WHERE json_extract(value,'$.attempted')=1 AND json_extract(value,'$.error') IS NULL", [])?;
    Ok(())
}

impl ClientStore {
    pub(crate) fn prepare_configuration_submission(
        &self,
        node: &str,
        session: &str,
        message_id: &str,
        response: Response,
        generation: u64,
    ) -> Result<()> {
        let mut conn = self.0.lock().expect("client database");
        let tx = conn.transaction()?;
        messages::authorized(&tx, node, generation)?;
        let old: Option<String> = tx.query_row("SELECT value FROM agent_configuration_outbox WHERE node=?1 AND session=?2 AND message_id=?3", params![node, session, message_id], |r| r.get(0)).optional()?;
        if let Some(old) = old {
            let old: Submission = serde_json::from_str(&old)?;
            if !old.rejected {
                ensure!(
                    old.response.accept == response.accept
                        && old.response.values == response.values,
                    "An earlier response is awaiting confirmation; recover it before changing the input."
                );
                return Ok(());
            }
        }
        let value = Submission {
            response,
            attempted: false,
            accepted: false,
            rejected: false,
            error: None,
        };
        tx.execute(
            "INSERT INTO agent_configuration_outbox VALUES(?1,?2,?3,?4,?5) ON CONFLICT(node,session,message_id) DO UPDATE SET generation=excluded.generation,value=excluded.value",
            params![
                node,
                session,
                message_id,
                generation,
                serde_json::to_string(&value)?
            ],
        )?;
        tx.commit()?;
        drop(conn);
        self.delivery_changed();
        Ok(())
    }

    pub(crate) fn configuration_deliveries(
        &self,
        node: &str,
    ) -> Result<Vec<ConfigurationDelivery>> {
        let conn = self.0.lock().expect("client database");
        let mut query = conn.prepare("SELECT session,message_id,generation,value FROM agent_configuration_outbox WHERE node=?1 ORDER BY rowid")?;
        let rows = query
            .query_map([node], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, u64>(2)?,
                    r.get::<_, String>(3)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .map(|(session, message_id, generation, value)| {
                Ok(ConfigurationDelivery {
                    session,
                    message_id,
                    generation,
                    submission: serde_json::from_str(&value)?,
                })
            })
            .collect()
    }

    pub(crate) fn configuration_submissions(
        &self,
        node: &str,
        session: &str,
        generation: u64,
    ) -> Result<HashMap<String, Submission>> {
        let conn = self.0.lock().expect("client database");
        messages::authorized(&conn, node, generation)?;
        let rows = conn.prepare("SELECT message_id,value FROM agent_configuration_outbox WHERE node=?1 AND session=?2 AND generation=?3")?
            .query_map(params![node, session, generation], |r| Ok((r.get::<_,String>(0)?, r.get::<_,String>(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .map(|(id, value)| Ok((id, serde_json::from_str(&value)?)))
            .collect()
    }

    pub(crate) fn change_configuration_submission(
        &self,
        node: &str,
        delivery: &ConfigurationDelivery,
        change: impl FnOnce(&mut Submission),
    ) -> Result<bool> {
        let mut conn = self.0.lock().expect("client database");
        let tx = conn.transaction()?;
        messages::authorized(&tx, node, delivery.generation)?;
        let old: Option<String> = tx.query_row("SELECT value FROM agent_configuration_outbox WHERE node=?1 AND session=?2 AND message_id=?3 AND generation=?4", params![node, delivery.session, delivery.message_id, delivery.generation], |r| r.get(0)).optional()?;
        let Some(old) = old else {
            return Ok(false);
        };
        let mut value: Submission = serde_json::from_str(&old)?;
        change(&mut value);
        let value = serde_json::to_string(&value)?;
        if old == value {
            return Ok(false);
        }
        tx.execute(
            "UPDATE agent_configuration_outbox SET value=?4 WHERE node=?1 AND session=?2 AND message_id=?3",
            params![node, delivery.session, delivery.message_id, value],
        )?;
        tx.commit()?;
        drop(conn);
        self.delivery_changed();
        Ok(true)
    }

    pub(crate) fn retry_configuration_submission(
        &self,
        node: &str,
        session: &str,
        message: &str,
        generation: u64,
    ) -> Result<()> {
        let delivery = self
            .configuration_deliveries(node)?
            .into_iter()
            .find(|d| d.session == session && d.message_id == message && d.generation == generation)
            .context("No response is awaiting recovery")?;
        ensure!(
            delivery.submission.error.is_some() && !delivery.submission.rejected,
            "The response is already being submitted"
        );
        self.change_configuration_submission(node, &delivery, |s| {
            s.attempted = false;
            s.error = None;
        })?;
        Ok(())
    }

    pub(crate) fn source_message_tail(
        &self,
        node: &str,
        session: &str,
        generation: u64,
    ) -> Result<Option<String>> {
        let conn = self.0.lock().expect("client database");
        messages::authorized(&conn, node, generation)?;
        Ok(conn.query_row("SELECT id FROM messages WHERE node=?1 AND session=?2 AND status='sent' ORDER BY position DESC LIMIT 1", params![node, session], |r| r.get(0)).optional()?)
    }
}
