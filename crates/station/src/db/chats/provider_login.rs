//! Login card state belongs to the Provider login operation. Private challenges
//! and callbacks are kept by its in-memory authorization session, never here.
use super::cards::{self, resolve, CardRecord};
use super::*;
use crate::{
    db::interaction_registry::{self as registry, Cleanup, Registration},
    node_access::Subject,
};
use zork_client_types::interaction::{Outcome, Request, Resolution};
pub(crate) const HANDLER: &str = zork_client_types::interaction::PROVIDER_LOGIN;
pub(super) const CARD: cards::Adapter = cards::Adapter {
    key: HANDLER,
    initialize,
    cleanup: crate::provider_login::cleanup,
    read,
    write_result,
};

pub(super) fn initialize(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS provider_login_cards(
        request_id TEXT PRIMARY KEY REFERENCES interaction_registrations(request_id),
        title TEXT NOT NULL,result TEXT);",
    )?;
    Ok(())
}

fn read(conn: &Connection, entry: Registration) -> Result<CardRecord> {
    let (title, result): (String, Option<String>) = conn
        .query_row(
            "SELECT title,result FROM provider_login_cards WHERE request_id=?1",
            [&entry.request_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .context("provider_login_card_not_found")?;
    Ok(CardRecord::new(
        entry,
        Request::OAuth { title },
        None,
        result.map(|s| serde_json::from_str(&s)).transpose()?,
    ))
}

fn write_result(conn: &Connection, record: &CardRecord, result: Resolution) -> Result<Resolution> {
    anyhow::ensure!(
        conn.execute(
            "UPDATE provider_login_cards SET result=?2 WHERE request_id=?1",
            params![record.request_id, serde_json::to_string(&result)?]
        )? == 1,
        "provider_login_card_not_found"
    );
    Ok(result)
}

impl StationDb {
    pub(crate) fn create_provider_login_card(
        &self,
        origin: &str,
        owner: &Subject,
        invocation: &str,
        title: &str,
    ) -> Result<CardRecord> {
        Request::OAuth {
            title: title.into(),
        }
        .validate()
        .map_err(anyhow::Error::msg)?;
        let mut conn = self.conn.lock().expect("db mutex");
        let tx = conn.transaction()?;
        if let Some(entry) = registry::find(&tx, owner, invocation, "login")? {
            anyhow::ensure!(entry.handler == HANDLER, "interaction_handler_mismatch");
            let record = read(&tx, entry)?;
            anyhow::ensure!(
                record.request
                    == Request::OAuth {
                        title: title.into()
                    },
                "idempotency_conflict"
            );
            return Ok(record);
        }
        let entry = registry::register_for_publication(
            &tx, origin, owner, invocation, "login", HANDLER, false,
        )?;
        tx.execute(
            "INSERT INTO provider_login_cards(request_id,title) VALUES(?1,?2)",
            params![entry.request_id, title],
        )?;
        let record = read(&tx, entry)?;
        tx.commit()?;
        self.flush_messages(&conn)?;
        self.chat_topics.publish([Topic::UserInteractionDelivery]);
        Ok(record)
    }

    pub(crate) fn provider_login_progress(&self, id: &str) -> Result<CardRecord> {
        let mut conn = self.conn.lock().expect("db mutex");
        let tx = conn.transaction()?;
        registry::require_active(&tx, id, HANDLER)?;
        let mut record = cards::read(&tx, id)?;
        if record.result.is_some() {
            return Ok(record);
        }
        let topics = resolve(
            &tx,
            &mut record,
            Resolution {
                request_message_id: String::new(),
                response_id: "operation".into(),
                revision: 1,
                outcome: Outcome::Pending,
                actor: "provider-login".into(),
                output: json!({}),
            },
        )?;
        tx.commit()?;
        self.flush_messages(&conn)?;
        self.chat_topics.publish(topics);
        Ok(record)
    }

    pub(crate) fn finish_provider_login(
        &self,
        key: &str,
        id: &str,
        outcome: Outcome,
        output: &Value,
    ) -> Result<()> {
        let mut conn = self.conn.lock().expect("db mutex");
        let tx = conn.transaction()?;
        let mut record = cards::read(&tx, id)?;
        anyhow::ensure!(record.handler == HANDLER, "interaction_handler_mismatch");
        if record.result.as_ref().is_some_and(|r| r.outcome.terminal()) {
            return Ok(());
        }
        let revision = record.result.as_ref().map_or(1, |r| r.revision + 1);
        let reason = if matches!(outcome, Outcome::Completed | Outcome::Unknown) {
            None
        } else {
            registry::cleanup_reason(&tx, id)?
        };
        let resolution = if let Some(reason) = reason {
            interrupted(&record, reason)
        } else {
            Resolution {
                request_message_id: String::new(),
                response_id: "operation".into(),
                revision,
                outcome,
                actor: "provider-login".into(),
                output: output.clone(),
            }
        };
        let topics = resolve(&tx, &mut record, resolution)?;
        super::finish(&tx, key, output)?;
        tx.commit()?;
        self.flush_messages(&conn)?;
        self.chat_topics.publish(topics);
        Ok(())
    }

    pub(crate) fn cleanup_provider_login(&self, id: &str, reason: Cleanup) -> Result<()> {
        let mut conn = self.conn.lock().expect("db mutex");
        let tx = conn.transaction()?;
        let mut record = cards::read(&tx, id)?;
        anyhow::ensure!(record.handler == HANDLER, "interaction_handler_mismatch");
        if record.result.as_ref().is_some_and(|r| r.outcome.terminal()) {
            return Ok(());
        }
        let result = interrupted(&record, reason);
        let topics = resolve(&tx, &mut record, result)?;
        tx.commit()?;
        self.flush_messages(&conn)?;
        self.chat_topics.publish(topics);
        Ok(())
    }
}

fn interrupted(record: &CardRecord, reason: Cleanup) -> Resolution {
    let revision = record.result.as_ref().map_or(1, |r| r.revision + 1);
    let unknown = record.result.is_some();
    let (outcome, output) = match reason {
        Cleanup::Cancelled => (Outcome::Cancelled, json!({"reason":"invocation_cancelled"})),
        Cleanup::OwnerLost => (
            if unknown {
                Outcome::Unknown
            } else {
                Outcome::Failed
            },
            json!({"reason":"runtime_interrupted","operation_replayed":false,"effects_may_have_occurred":unknown}),
        ),
    };
    Resolution {
        request_message_id: String::new(),
        response_id: format!("interrupted-{revision}"),
        revision,
        outcome,
        actor: "provider-login".into(),
        output,
    }
}
