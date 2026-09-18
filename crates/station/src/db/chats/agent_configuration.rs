//! Agent configuration reviews own their forms, submissions and receipts.
use super::cards::{self, resolve, CardRecord, SubmittedResponse};
use super::*;
use crate::{
    db::interaction_registry::{self as registry, Cleanup, Registration},
    node_access::{fingerprint, Subject},
};
use zork_client_types::interaction::{Outcome, Request, Resolution, Response};
pub(crate) const HANDLER: &str = zork_client_types::interaction::AGENT_CONFIGURATION;
pub(super) const CARD: cards::Adapter = cards::Adapter {
    key: HANDLER,
    initialize,
    cleanup: crate::agent_configuration::cleanup,
    read,
    write_result,
};

pub(super) fn initialize(conn: &Connection) -> Result<()> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS agent_configuration_cards(
        request_id TEXT PRIMARY KEY REFERENCES interaction_registrations(request_id),
        form TEXT NOT NULL,submission TEXT,result TEXT);
        CREATE TABLE IF NOT EXISTS agent_configuration_responses(
        response_id TEXT NOT NULL,request_id TEXT NOT NULL,fingerprint TEXT NOT NULL,state TEXT NOT NULL,error TEXT,
        PRIMARY KEY(request_id,response_id));")?;
    Ok(())
}

fn read(conn: &Connection, entry: Registration) -> Result<CardRecord> {
    let (form, submission, result): (String, Option<String>, Option<String>) = conn
        .query_row(
            "SELECT form,submission,result FROM agent_configuration_cards WHERE request_id=?1",
            [&entry.request_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .context("agent_configuration_review_not_found")?;
    Ok(CardRecord::new(
        entry,
        serde_json::from_str(&form)?,
        submission.map(|s| serde_json::from_str(&s)).transpose()?,
        result.map(|s| serde_json::from_str(&s)).transpose()?,
    ))
}

fn write_result(
    conn: &Connection,
    record: &CardRecord,
    mut result: Resolution,
) -> Result<Resolution> {
    // Only Agent configuration cards retain their accepted public field values.
    if let Some(values) = record.result.as_ref().and_then(|r| r.output.get("values")) {
        if result.output.is_null() {
            result.output = json!({});
        }
        if let Some(output) = result.output.as_object_mut() {
            output.entry("values").or_insert_with(|| values.clone());
        }
    }
    anyhow::ensure!(
        conn.execute(
            "UPDATE agent_configuration_cards SET result=?2 WHERE request_id=?1",
            params![record.request_id, serde_json::to_string(&result)?]
        )? == 1,
        "agent_configuration_review_not_found"
    );
    Ok(result)
}

impl StationDb {
    pub(crate) fn create_configuration_review(
        &self,
        origin: &str,
        owner: &Subject,
        invocation: &str,
        form: &Request,
    ) -> Result<CardRecord> {
        anyhow::ensure!(
            matches!(
                form,
                Request::AgentConfiguration { .. }
                    | Request::Input { .. }
                    | Request::Approval { .. }
            ),
            "invalid_agent_configuration_form"
        );
        form.validate().map_err(anyhow::Error::msg)?;
        let mut conn = self.conn.lock().expect("db mutex");
        let tx = conn.transaction()?;
        if let Some(entry) = registry::find(&tx, owner, invocation, "parameters")? {
            anyhow::ensure!(entry.handler == HANDLER, "interaction_handler_mismatch");
            let saved = read(&tx, entry)?;
            anyhow::ensure!(saved.request == *form, "idempotency_conflict");
            return Ok(saved);
        }
        let entry = registry::register(&tx, origin, owner, invocation, "parameters", HANDLER)?;
        tx.execute(
            "INSERT INTO agent_configuration_cards(request_id,form) VALUES(?1,?2)",
            params![entry.request_id, serde_json::to_string(form)?],
        )?;
        let record = read(&tx, entry)?;
        tx.commit()?;
        self.flush_messages(&conn)?;
        self.chat_topics.publish([Topic::UserInteractionDelivery]);
        Ok(record)
    }

    /// Admit input only. The waiting business tool validates and acknowledges it.
    pub(crate) fn submit_configuration_response(
        &self,
        id: &str,
        response: &Response,
        actor: &str,
    ) -> Result<CardRecord> {
        let mut conn = self.conn.lock().expect("db mutex");
        let tx = conn.transaction()?;
        let mut request = cards::read(&tx, id)?;
        anyhow::ensure!(request.handler == HANDLER, "interaction_handler_mismatch");
        let signature = fingerprint(&(response, actor))?;
        let prior: Option<String> = tx.query_row("SELECT fingerprint FROM agent_configuration_responses WHERE request_id=?1 AND response_id=?2",
            params![id,response.response_id], |r|r.get(0)).optional()?;
        if let Some(prior) = prior {
            anyhow::ensure!(prior == signature, "idempotency_conflict");
            return Ok(request);
        }
        if request
            .result
            .as_ref()
            .is_some_and(|r| r.outcome.terminal())
        {
            return Ok(request);
        }
        anyhow::ensure!(
            zork_client_types::interaction::valid_id(&response.response_id),
            "invalid_response_id"
        );
        registry::require_active(&tx, id, HANDLER)?;
        if response.accept {
            anyhow::ensure!(
                !matches!(request.request, Request::OAuth { .. }),
                "interaction_private_action_required"
            );
            request
                .request
                .validate_values(&response.values)
                .map_err(|e| {
                    anyhow::anyhow!(
                        "invalid_agent_configuration_input: {}",
                        serde_json::to_string(&e).unwrap_or_default()
                    )
                })?;
            anyhow::ensure!(
                request.submission.is_none(),
                "agent_configuration_submission_pending"
            );
        } else {
            anyhow::ensure!(response.values.is_empty(), "unexpected_decline_input");
        }
        tx.execute("INSERT INTO agent_configuration_responses(request_id,response_id,fingerprint,state) VALUES(?1,?2,?3,?4)",
            params![id,response.response_id,signature,if response.accept {"pending"} else {"accepted"}])?;
        let topics = if response.accept {
            let submitted = SubmittedResponse {
                response: response.clone(),
                actor: actor.into(),
            };
            tx.execute(
                "UPDATE agent_configuration_cards SET submission=?2 WHERE request_id=?1",
                params![id, serde_json::to_string(&submitted)?],
            )?;
            vec![Topic::UserInteraction(id.into())]
        } else {
            let revision = request.result.as_ref().map_or(1, |r| r.revision + 1);
            resolve(
                &tx,
                &mut request,
                Resolution {
                    request_message_id: String::new(),
                    response_id: response.response_id.clone(),
                    revision,
                    outcome: Outcome::Declined,
                    actor: actor.into(),
                    output: Value::Null,
                },
            )?
        };
        tx.commit()?;
        self.flush_messages(&conn)?;
        drop(conn);
        self.chat_topics.publish(topics);
        self.business_card(id)
    }

    pub(crate) fn configuration_response_state(
        &self,
        id: &str,
        response: &str,
    ) -> Result<Option<(String, Option<String>)>> {
        self.conn.lock().expect("db mutex").query_row("SELECT state,error FROM agent_configuration_responses WHERE request_id=?1 AND response_id=?2",
            params![id,response],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(Into::into)
    }

    pub(crate) fn reject_configuration_response(
        &self,
        id: &str,
        response: &str,
        error: &str,
    ) -> Result<()> {
        let mut conn = self.conn.lock().expect("db mutex");
        let tx = conn.transaction()?;
        registry::require_active(&tx, id, HANDLER)?;
        let request = cards::read(&tx, id)?;
        anyhow::ensure!(
            request
                .result
                .as_ref()
                .is_none_or(|r| !r.outcome.terminal()),
            "interaction_settled"
        );
        anyhow::ensure!(
            request
                .submission
                .as_ref()
                .is_some_and(|s| s.response.response_id == response),
            "interaction_response_mismatch"
        );
        tx.execute("UPDATE agent_configuration_responses SET state='rejected',error=?3 WHERE request_id=?1 AND response_id=?2",params![id,response,error])?;
        tx.execute(
            "UPDATE agent_configuration_cards SET submission=NULL WHERE request_id=?1",
            [id],
        )?;
        tx.commit()?;
        self.flush_messages(&conn)?;
        self.chat_topics
            .publish([Topic::UserInteraction(id.into())]);
        Ok(())
    }

    pub(crate) fn acknowledge_configuration_response(
        &self,
        id: &str,
        submitted: &SubmittedResponse,
        output: Value,
    ) -> Result<CardRecord> {
        let mut conn = self.conn.lock().expect("db mutex");
        let tx = conn.transaction()?;
        registry::require_active(&tx, id, HANDLER)?;
        let mut request = cards::read(&tx, id)?;
        anyhow::ensure!(
            request
                .result
                .as_ref()
                .is_none_or(|r| !r.outcome.terminal()),
            "interaction_settled"
        );
        anyhow::ensure!(
            request
                .submission
                .as_ref()
                .is_some_and(|s| s.response == submitted.response && s.actor == submitted.actor),
            "interaction_response_mismatch"
        );
        if request.result.is_some() {
            return Ok(request);
        }
        tx.execute("UPDATE agent_configuration_responses SET state='accepted' WHERE request_id=?1 AND response_id=?2",params![id,submitted.response.response_id])?;
        let topics = resolve(
            &tx,
            &mut request,
            Resolution {
                request_message_id: String::new(),
                response_id: submitted.response.response_id.clone(),
                revision: 1,
                outcome: Outcome::Pending,
                actor: submitted.actor.clone(),
                output,
            },
        )?;
        tx.commit()?;
        self.flush_messages(&conn)?;
        self.chat_topics.publish(topics);
        Ok(request)
    }

    pub(crate) fn finish_configuration_review(
        &self,
        id: &str,
        attempt: &str,
        outcome: Outcome,
        output: Value,
        actor: &str,
    ) -> Result<CardRecord> {
        let mut conn = self.conn.lock().expect("db mutex");
        let tx = conn.transaction()?;
        let mut record = cards::read(&tx, id)?;
        anyhow::ensure!(record.handler == HANDLER, "interaction_handler_mismatch");
        if record.result.as_ref().is_some_and(|r| r.outcome.terminal()) {
            return Ok(record);
        }
        let revision = record.result.as_ref().map_or(1, |r| r.revision + 1);
        let resolution = if let Some(reason) = registry::cleanup_reason(&tx, id)? {
            interrupted(&record, reason)
        } else {
            if let Some(old) = &record.result {
                anyhow::ensure!(old.response_id == attempt, "interaction_attempt_mismatch");
            }
            Resolution {
                request_message_id: String::new(),
                response_id: attempt.into(),
                revision,
                outcome,
                actor: actor.into(),
                output,
            }
        };
        let topics = resolve(&tx, &mut record, resolution)?;
        tx.commit()?;
        self.flush_messages(&conn)?;
        self.chat_topics.publish(topics);
        Ok(record)
    }

    pub(crate) fn cleanup_configuration_review(&self, id: &str, reason: Cleanup) -> Result<()> {
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
        actor: "agent-configuration".into(),
        output,
    }
}
