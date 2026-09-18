//! A publication grant is scoped to one destination Chat. Only the request
//! authority can redeem it and send phase facts. Mirrored requests never execute.
use super::*;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Grant {
    pub request_id: String,
    pub token: String,
    pub destination: String,
    pub chat: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Delivery {
    pub grant: Grant,
    pub chat: String,
    pub message: String,
    pub result: Resolution,
}

pub(super) fn initialize(conn: &Connection) -> Result<()> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS business_card_events(
        request_id TEXT NOT NULL, revision INTEGER NOT NULL, result TEXT NOT NULL,
        PRIMARY KEY(request_id,revision));
        CREATE TABLE IF NOT EXISTS business_card_exports(
        request_id TEXT NOT NULL, destination TEXT NOT NULL, requested_chat TEXT NOT NULL,
        token TEXT NOT NULL UNIQUE, chat_id TEXT, message_id TEXT, delivered INTEGER NOT NULL DEFAULT 0,
        PRIMARY KEY(request_id,destination,requested_chat));
        CREATE TABLE IF NOT EXISTS business_card_mirrors(
        request_id TEXT PRIMARY KEY, value TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS business_card_imports(
        request_id TEXT NOT NULL, chat_id TEXT NOT NULL, message_id TEXT NOT NULL, grant_json TEXT NOT NULL,
        PRIMARY KEY(request_id,chat_id));")?;
    Ok(())
}

pub(super) fn mirror(conn: &Connection, id: &str) -> Result<Option<CardRecord>> {
    let json: Option<String> = conn
        .query_row(
            "SELECT value FROM business_card_mirrors WHERE request_id=?1",
            [id],
            |r| r.get(0),
        )
        .optional()?;
    json.map(|s| serde_json::from_str(&s).map_err(Into::into))
        .transpose()
}

pub(super) fn record_event(conn: &Connection, id: &str, result: &Resolution) -> Result<()> {
    conn.execute(
        "INSERT INTO business_card_events VALUES(?1,?2,?3)",
        params![id, result.revision, serde_json::to_string(result)?],
    )?;
    Ok(())
}

impl StationDb {
    pub(crate) fn grant_user_publication(
        &self,
        id: &str,
        subject: &Subject,
        destination: &str,
        chat: &str,
    ) -> Result<Grant> {
        let conn = self.conn.lock().expect("db mutex");
        anyhow::ensure!(
            mirror(&conn, id)?.is_none(),
            "interaction_authority_required"
        );
        let request = read(&conn, id)?;
        anyhow::ensure!(request.owner == *subject, "interaction_owner_mismatch");
        conn.execute("INSERT OR IGNORE INTO business_card_exports(request_id,destination,requested_chat,token) VALUES(?1,?2,?3,?4)",
            params![id,destination,chat,ulid::Ulid::new().to_string()])?;
        let token = conn.query_row("SELECT token FROM business_card_exports WHERE request_id=?1 AND destination=?2 AND requested_chat=?3",
            params![id,destination,chat], |r| r.get(0))?;
        Ok(Grant {
            request_id: id.into(),
            token,
            destination: destination.into(),
            chat: chat.into(),
        })
    }

    pub(crate) fn redeem_user_publication(
        &self,
        grant: &Grant,
        chat: &str,
        root: &str,
        origin: &str,
    ) -> Result<CardRecord> {
        let mut conn = self.conn.lock().expect("db mutex");
        let tx = conn.transaction()?;
        let (id, destination, requested, prior_chat, prior_root):(String,String,String,Option<String>,Option<String>) = tx.query_row(
            "SELECT request_id,destination,requested_chat,chat_id,message_id FROM business_card_exports WHERE token=?1", [&grant.token],
            |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).context("interaction_publication_denied")?;
        anyhow::ensure!(
            id == grant.request_id && destination == grant.destination && requested == grant.chat,
            "interaction_publication_denied"
        );
        anyhow::ensure!(
            prior_chat.as_deref().is_none_or(|c| c == chat)
                && prior_root.as_deref().is_none_or(|r| r == root),
            "interaction_binding_conflict"
        );
        let mut request = read(&tx, &id)?;
        if prior_root.is_none() {
            tx.execute("UPDATE business_card_exports SET chat_id=?2,message_id=?3,delivered=?4 WHERE token=?1",
                params![grant.token,chat,root,request.result.as_ref().map_or(0,|r|r.revision)])?;
        }
        // The same actor has an explicit origin outside its home node.
        if request.owner.origin == "local" {
            request.owner.origin = origin.into();
        }
        tx.commit()?;
        self.flush_messages(&conn)?;
        self.chat_topics.publish([Topic::UserInteractionDelivery]);
        Ok(request)
    }

    pub(crate) fn import_user_publication(
        &self,
        grant: &Grant,
        chat: &str,
        root: &str,
        request: &CardRecord,
        actor: &str,
    ) -> Result<()> {
        anyhow::ensure!(
            request.request_id == grant.request_id
                && super::super::super::super::channels::request_actor(&request.owner) == actor,
            "interaction_owner_mismatch"
        );
        let mut conn = self.conn.lock().expect("db mutex");
        let tx = conn.transaction()?;
        let mut latest = request.clone();
        if let Some(old) = mirror(&tx, &request.request_id)? {
            anyhow::ensure!(
                old.request == request.request
                    && old.owner == request.owner
                    && old.handler == request.handler,
                "interaction_snapshot_mismatch"
            );
            if old.result.as_ref().map_or(0, |r| r.revision)
                > request.result.as_ref().map_or(0, |r| r.revision)
            {
                latest = old;
            }
        }
        tx.execute("INSERT INTO business_card_mirrors VALUES(?1,?2) ON CONFLICT(request_id) DO UPDATE SET value=excluded.value",
            params![request.request_id,serde_json::to_string(&latest)?])?;
        tx.execute(
            "INSERT OR IGNORE INTO business_card_imports VALUES(?1,?2,?3,?4)",
            params![
                request.request_id,
                chat,
                root,
                serde_json::to_string(grant)?
            ],
        )?;
        tx.commit()?;
        self.flush_messages(&conn)?;
        Ok(())
    }

    pub(crate) fn user_publication_grant(&self, chat: &str, root: &str) -> Result<Option<Grant>> {
        let conn = self.conn.lock().expect("db mutex");
        let value: Option<String> = conn
            .query_row(
                "SELECT grant_json FROM business_card_imports WHERE chat_id=?1 AND message_id=?2",
                params![chat, root],
                |r| r.get(0),
            )
            .optional()?;
        value
            .map(|v| serde_json::from_str(&v).map_err(Into::into))
            .transpose()
    }

    pub(crate) fn check_user_publication(
        &self,
        grant: &Grant,
        chat: &str,
        root: &str,
    ) -> Result<()> {
        let conn = self.conn.lock().expect("db mutex");
        let allowed: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM business_card_exports WHERE token=?1 AND request_id=?2 AND destination=?3 AND requested_chat=?4 AND chat_id=?5 AND message_id=?6)",
            params![grant.token,grant.request_id,grant.destination,grant.chat,chat,root],|r|r.get(0))?;
        anyhow::ensure!(allowed, "interaction_publication_denied");
        Ok(())
    }

    pub(crate) fn pending_user_publications(&self) -> Result<Vec<Delivery>> {
        let conn = self.conn.lock().expect("db mutex");
        let rows = conn.prepare("SELECT x.request_id,x.token,x.destination,x.requested_chat,x.chat_id,x.message_id,e.result
            FROM business_card_exports x JOIN business_card_events e ON e.request_id=x.request_id
            WHERE x.message_id IS NOT NULL AND e.revision=(SELECT MIN(revision) FROM business_card_events WHERE request_id=x.request_id AND revision>x.delivered)")?
            .query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?,r.get::<_,String>(5)?,r.get::<_,String>(6)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .map(
                |(id, token, destination, requested, chat, message, result)| {
                    Ok(Delivery {
                        grant: Grant {
                            request_id: id,
                            token,
                            destination,
                            chat: requested,
                        },
                        chat,
                        message,
                        result: serde_json::from_str(&result)?,
                    })
                },
            )
            .collect()
    }

    pub(crate) fn acknowledge_user_publication(&self, delivery: &Delivery) -> Result<()> {
        self.conn.lock().expect("db mutex").execute(
            "UPDATE business_card_exports SET delivered=MAX(delivered,?2) WHERE token=?1",
            params![delivery.grant.token, delivery.result.revision],
        )?;
        Ok(())
    }

    pub(crate) fn receive_user_publication(&self, delivery: &Delivery) -> Result<()> {
        let mut conn = self.conn.lock().expect("db mutex");
        let tx = conn.transaction()?;
        let grant: String = tx.query_row("SELECT grant_json FROM business_card_imports WHERE request_id=?1 AND chat_id=?2 AND message_id=?3",
            params![delivery.grant.request_id,delivery.chat,delivery.message],|r|r.get(0)).context("interaction_publication_not_ready")?;
        anyhow::ensure!(
            serde_json::from_str::<Grant>(&grant)?.token == delivery.grant.token,
            "interaction_publication_denied"
        );
        let mut request = mirror(&tx, &delivery.grant.request_id)?
            .context("interaction_publication_not_ready")?;
        if request
            .result
            .as_ref()
            .is_some_and(|r| r.revision >= delivery.result.revision)
        {
            return Ok(());
        }
        request.result = Some(delivery.result.clone());
        tx.execute(
            "UPDATE business_card_mirrors SET value=?2 WHERE request_id=?1",
            params![request.request_id, serde_json::to_string(&request)?],
        )?;
        let mut topics = vec![Topic::UserInteraction(request.request_id.clone())];
        // A phase can arrive after redemption but before the card commit. Hydrate
        // then includes this snapshot; later phases append normal source messages.
        let bindings = tx
            .prepare("SELECT chat_id,message_id FROM business_card_bindings WHERE request_id=?1")?
            .query_map([&request.request_id], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for (chat, root) in bindings {
            topics.extend(append_result(&tx, &request, &chat, &root)?.1);
        }
        tx.commit()?;
        self.flush_messages(&conn)?;
        self.chat_topics.publish(topics);
        Ok(())
    }
}
