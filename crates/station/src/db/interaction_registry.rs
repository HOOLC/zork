//! Durable registration identities and their lifecycle. Business state is stored
//! by the registered owner; no card, submission or business result enters here.
use super::{params, Connection, Context, OptionalExtension, Result, StationDb};
use crate::{db::chats::Topic, node_access::Subject};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Registration {
    pub request_id: String,
    pub owner: Subject,
    pub invocation_id: String,
    pub request_key: String,
    pub handler: String,
    pub active: bool,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Cleanup {
    Cancelled,
    OwnerLost,
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Notice {
    pub sequence: u64,
    pub request_id: String,
    pub owner: Subject,
    pub invocation_id: String,
    pub origin: String,
}

#[derive(Clone)]
pub(crate) struct OutgoingCall {
    pub subject: Subject,
    pub invocation_id: String,
    pub target: String,
}

pub(super) fn initialize(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS interaction_registrations(
         request_id TEXT PRIMARY KEY, owner TEXT NOT NULL, invocation_id TEXT NOT NULL,
         request_key TEXT NOT NULL, handler TEXT NOT NULL, active INTEGER NOT NULL DEFAULT 1,
         cleanup TEXT, UNIQUE(owner,invocation_id,request_key));
         CREATE TABLE IF NOT EXISTS interaction_registration_cancellations(
         owner TEXT NOT NULL, invocation_id TEXT NOT NULL, PRIMARY KEY(owner,invocation_id));
         CREATE TABLE IF NOT EXISTS interaction_registration_calls(
         owner TEXT NOT NULL, invocation_id TEXT NOT NULL, target TEXT NOT NULL,
         finished INTEGER NOT NULL DEFAULT 0, cleanup INTEGER NOT NULL DEFAULT 0,
         PRIMARY KEY(owner,invocation_id));
         CREATE TABLE IF NOT EXISTS interaction_registration_call_cancellations(
         agent TEXT NOT NULL, session TEXT NOT NULL, invocation_id TEXT NOT NULL,
         PRIMARY KEY(agent,session,invocation_id));
         CREATE TABLE IF NOT EXISTS interaction_registration_notices(
         sequence INTEGER PRIMARY KEY AUTOINCREMENT, request_id TEXT NOT NULL,
         owner TEXT NOT NULL, invocation_id TEXT NOT NULL, origin TEXT NOT NULL,
         delivered INTEGER NOT NULL DEFAULT 0);
         CREATE INDEX IF NOT EXISTS interaction_registration_notice_delivery
         ON interaction_registration_notices(delivered,sequence);",
    )?;
    Ok(())
}

pub(crate) fn read(conn: &Connection, id: &str) -> Result<Registration> {
    let (owner, invocation_id, request_key, handler, active): (String, String, String, String, bool) =
        conn.query_row(
            "SELECT owner,invocation_id,request_key,handler,active FROM interaction_registrations WHERE request_id=?1",
            [id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?)),
        ).context("interaction_registration_not_found")?;
    Ok(Registration {
        request_id: id.into(),
        owner: serde_json::from_str(&owner)?,
        invocation_id,
        request_key,
        handler,
        active,
    })
}

pub(crate) fn find(
    conn: &Connection,
    owner: &Subject,
    invocation: &str,
    key: &str,
) -> Result<Option<Registration>> {
    let id: Option<String> = conn.query_row(
        "SELECT request_id FROM interaction_registrations WHERE owner=?1 AND invocation_id=?2 AND request_key=?3",
        params![serde_json::to_string(owner)?,invocation,key], |r| r.get(0),
    ).optional()?;
    id.map(|id| read(conn, &id)).transpose()
}

/// The caller's transaction may also write its own business data. Registration
/// only knows the owner, stable slot and registered handler identity.
pub(crate) fn register(
    conn: &Connection,
    origin: &str,
    owner: &Subject,
    invocation: &str,
    key: &str,
    handler: &str,
) -> Result<Registration> {
    register_for_publication(conn, origin, owner, invocation, key, handler, true)
}

pub(crate) fn register_for_publication(
    conn: &Connection,
    origin: &str,
    owner: &Subject,
    invocation: &str,
    key: &str,
    handler: &str,
    notify_owner: bool,
) -> Result<Registration> {
    anyhow::ensure!(
        !origin.is_empty()
            && !invocation.is_empty()
            && invocation.len() <= 256
            && !key.is_empty()
            && key.len() <= 256
            && !handler.is_empty()
            && handler.len() <= 256,
        "invalid_interaction_registration",
    );
    if let Some(entry) = find(conn, owner, invocation, key)? {
        anyhow::ensure!(
            entry.handler == handler,
            "interaction_registration_conflict"
        );
        return Ok(entry);
    }
    let who = serde_json::to_string(owner)?;
    let cancelled: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM interaction_registration_cancellations WHERE owner=?1 AND invocation_id=?2)",
        params![who,invocation], |r| r.get(0),
    )?;
    anyhow::ensure!(!cancelled, "interaction_cancelled");
    let id = format!("{origin}/{}", ulid::Ulid::new());
    conn.execute(
        "INSERT INTO interaction_registrations(request_id,owner,invocation_id,request_key,handler) VALUES(?1,?2,?3,?4,?5)",
        params![id,who,invocation,key,handler],
    )?;
    if notify_owner {
        conn.execute(
        "INSERT INTO interaction_registration_notices(request_id,owner,invocation_id,origin) VALUES(?1,?2,?3,?4)",
        params![id,who,invocation,origin],
    )?;
    }
    read(conn, &id)
}

pub(crate) fn require_active(conn: &Connection, id: &str, handler: &str) -> Result<()> {
    let entry = read(conn, id)?;
    anyhow::ensure!(entry.handler == handler, "interaction_handler_mismatch");
    anyhow::ensure!(entry.active, "interaction_settled");
    Ok(())
}

pub(crate) fn retire(conn: &Connection, id: &str) -> Result<()> {
    conn.execute(
        "UPDATE interaction_registrations SET active=0 WHERE request_id=?1",
        [id],
    )?;
    Ok(())
}

pub(crate) fn cleanup_reason(conn: &Connection, id: &str) -> Result<Option<Cleanup>> {
    let reason: Option<String> = conn.query_row(
        "SELECT cleanup FROM interaction_registrations WHERE request_id=?1",
        [id],
        |r| r.get(0),
    )?;
    reason
        .map(|reason| match reason.as_str() {
            "cancelled" => Ok(Cleanup::Cancelled),
            "owner_lost" => Ok(Cleanup::OwnerLost),
            _ => anyhow::bail!("invalid_registration_cleanup"),
        })
        .transpose()
}

impl StationDb {
    pub(crate) fn interaction_registration(&self, id: &str) -> Result<Registration> {
        read(&self.conn.lock().expect("db mutex"), id)
    }

    pub(crate) fn begin_user_call(
        &self,
        owner: &Subject,
        invocation: &str,
        target: &str,
    ) -> Result<()> {
        let conn = self.conn.lock().expect("db mutex");
        let cancelled: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM interaction_registration_call_cancellations WHERE agent=?1 AND session=?2 AND invocation_id=?3)",
            params![owner.agent,owner.session,invocation], |r| r.get(0),
        )?;
        anyhow::ensure!(!cancelled, "interaction_cancelled");
        let who = serde_json::to_string(owner)?;
        conn.execute(
            "INSERT OR IGNORE INTO interaction_registration_calls(owner,invocation_id,target) VALUES(?1,?2,?3)",
            params![who,invocation,target],
        )?;
        let previous: String = conn.query_row(
            "SELECT target FROM interaction_registration_calls WHERE owner=?1 AND invocation_id=?2",
            params![who, invocation],
            |r| r.get(0),
        )?;
        anyhow::ensure!(previous == target, "idempotency_conflict");
        Ok(())
    }

    pub(crate) fn finish_user_calls(&self, owner: &Subject, invocation: &str) -> Result<()> {
        // Finishing schedules orphan-registration cleanup without turning a
        // completed invocation into a cancelled one. Its receipt can be recovered.
        self.conn.lock().expect("db mutex").execute(
            "UPDATE interaction_registration_calls SET finished=1,cleanup=1 WHERE invocation_id=?1 AND json_extract(owner,'$.agent')=?2 AND json_extract(owner,'$.session')=?3",
            params![invocation,owner.agent,owner.session],
        )?;
        self.chat_topics.publish([Topic::UserInteractionDelivery]);
        Ok(())
    }

    pub(crate) fn cancel_user_calls(
        &self,
        owner: &Subject,
        invocation: &str,
    ) -> Result<Vec<OutgoingCall>> {
        let mut conn = self.conn.lock().expect("db mutex");
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT OR IGNORE INTO interaction_registration_call_cancellations VALUES(?1,?2,?3)",
            params![owner.agent, owner.session, invocation],
        )?;
        tx.execute("UPDATE interaction_registration_calls SET cleanup=1 WHERE invocation_id=?1 AND json_extract(owner,'$.agent')=?2 AND json_extract(owner,'$.session')=?3",
            params![invocation,owner.agent,owner.session])?;
        let rows = tx.prepare("SELECT owner,target FROM interaction_registration_calls WHERE invocation_id=?1 AND json_extract(owner,'$.agent')=?2 AND json_extract(owner,'$.session')=?3 AND cleanup=1")?
            .query_map(params![invocation,owner.agent,owner.session], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let calls = rows
            .into_iter()
            .map(|(subject, target)| {
                Ok(OutgoingCall {
                    subject: serde_json::from_str(&subject)?,
                    target,
                    invocation_id: invocation.into(),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        tx.commit()?;
        self.chat_topics.publish([Topic::UserInteractionDelivery]);
        Ok(calls)
    }

    pub(crate) fn pending_user_call_cancellations(&self) -> Result<Vec<OutgoingCall>> {
        let conn = self.conn.lock().expect("db mutex");
        let rows = conn.prepare("SELECT owner,invocation_id,target FROM interaction_registration_calls WHERE cleanup=1")?
            .query_map([], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .map(|(owner, invocation_id, target)| {
                Ok(OutgoingCall {
                    subject: serde_json::from_str(&owner)?,
                    invocation_id,
                    target,
                })
            })
            .collect()
    }

    pub(crate) fn acknowledge_user_call_cancellation(&self, call: &OutgoingCall) -> Result<()> {
        self.conn.lock().expect("db mutex").execute(
            "UPDATE interaction_registration_calls SET cleanup=0,finished=1 WHERE owner=?1 AND invocation_id=?2 AND target=?3",
            params![serde_json::to_string(&call.subject)?,call.invocation_id,call.target],
        )?;
        Ok(())
    }

    pub(crate) fn revoke_interaction_registrations(
        &self,
        owner: &Subject,
        invocation: &str,
    ) -> Result<()> {
        let mut conn = self.conn.lock().expect("db mutex");
        let tx = conn.transaction()?;
        let who = serde_json::to_string(owner)?;
        tx.execute(
            "INSERT OR IGNORE INTO interaction_registration_cancellations VALUES(?1,?2)",
            params![who, invocation],
        )?;
        tx.execute("UPDATE interaction_registrations SET active=0,cleanup=COALESCE(cleanup,'cancelled') WHERE owner=?1 AND invocation_id=?2 AND (active=1 OR cleanup IS NOT NULL)",params![who,invocation])?;
        tx.execute("UPDATE interaction_registration_notices SET delivered=1 WHERE request_id IN (SELECT request_id FROM interaction_registrations WHERE owner=?1 AND invocation_id=?2 AND cleanup IS NOT NULL)",params![who,invocation])?;
        tx.commit()?;
        self.chat_topics.publish([Topic::UserInteractionDelivery]);
        Ok(())
    }

    pub(crate) fn recover_interaction_registrations(&self) -> Result<()> {
        let mut conn = self.conn.lock().expect("db mutex");
        let tx = conn.transaction()?;
        tx.execute(
            "UPDATE interaction_registration_calls SET cleanup=1 WHERE finished=0",
            [],
        )?;
        tx.execute(
            "UPDATE interaction_registrations SET active=0,cleanup='owner_lost' WHERE active=1 OR cleanup IS NOT NULL",
            [],
        )?;
        tx.execute("UPDATE interaction_registration_notices SET delivered=1 WHERE request_id IN (SELECT request_id FROM interaction_registrations WHERE cleanup IS NOT NULL)", [])?;
        tx.commit()?;
        Ok(())
    }

    pub(crate) fn pending_registration_cleanup(&self) -> Result<Vec<(Registration, Cleanup)>> {
        let conn = self.conn.lock().expect("db mutex");
        let rows = conn.prepare("SELECT request_id,cleanup FROM interaction_registrations WHERE cleanup IS NOT NULL")?
            .query_map([], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .map(|(id, reason)| {
                Ok((
                    read(&conn, &id)?,
                    match reason.as_str() {
                        "cancelled" => Cleanup::Cancelled,
                        "owner_lost" => Cleanup::OwnerLost,
                        _ => anyhow::bail!("invalid_registration_cleanup"),
                    },
                ))
            })
            .collect()
    }

    pub(crate) fn acknowledge_registration_cleanup(&self, id: &str) -> Result<()> {
        self.conn.lock().expect("db mutex").execute(
            "UPDATE interaction_registrations SET cleanup=NULL WHERE request_id=?1",
            [id],
        )?;
        Ok(())
    }

    pub(crate) fn user_notice_owners(&self) -> Result<Vec<Subject>> {
        let conn = self.conn.lock().expect("db mutex");
        let owners = conn
            .prepare(
                "SELECT DISTINCT owner FROM interaction_registration_notices WHERE delivered=0",
            )?
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        owners
            .into_iter()
            .map(|s| serde_json::from_str(&s).map_err(Into::into))
            .collect()
    }

    pub(crate) fn next_user_notice(&self, owner: &Subject) -> Result<Option<Notice>> {
        let conn = self.conn.lock().expect("db mutex");
        let row: Option<(u64,String,String,String)> = conn.query_row(
            "SELECT sequence,request_id,invocation_id,origin FROM interaction_registration_notices WHERE delivered=0 AND owner=?1 ORDER BY sequence LIMIT 1",
            [serde_json::to_string(owner)?], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)),
        ).optional()?;
        Ok(
            row.map(|(sequence, request_id, invocation_id, origin)| Notice {
                sequence,
                request_id,
                owner: owner.clone(),
                invocation_id,
                origin,
            }),
        )
    }

    pub(crate) fn acknowledge_user_notice(&self, sequence: u64) -> Result<()> {
        self.conn.lock().expect("db mutex").execute(
            "UPDATE interaction_registration_notices SET delivered=1 WHERE sequence=?1",
            [sequence],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registration_needs_only_identity_and_a_handler() -> Result<()> {
        let mut conn = Connection::open_in_memory()?;
        initialize(&conn)?;
        let owner = Subject {
            origin: "local".into(),
            agent: "caller".into(),
            session: "session".into(),
        };
        let tx = conn.transaction()?;
        let entry = register(
            &tx,
            "node",
            &owner,
            "invocation",
            "slot",
            "an-independent-business",
        )?;
        assert_eq!(
            entry,
            register(
                &tx,
                "node",
                &owner,
                "invocation",
                "slot",
                "an-independent-business"
            )?
        );
        assert!(register(
            &tx,
            "node",
            &owner,
            "invocation",
            "slot",
            "another-business"
        )
        .is_err());
        require_active(&tx, &entry.request_id, "an-independent-business")?;
        assert!(require_active(&tx, &entry.request_id, "another-business").is_err());
        retire(&tx, &entry.request_id)?;
        assert!(!read(&tx, &entry.request_id)?.active);
        tx.commit()?;
        let tables: Vec<String> = conn
            .prepare(
                "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",
            )?
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        assert!(tables
            .iter()
            .all(|name| name.starts_with("interaction_registr")));
        let notices: usize = conn.query_row(
            "SELECT COUNT(*) FROM interaction_registration_notices",
            [],
            |r| r.get(0),
        )?;
        assert_eq!(notices, 1);
        Ok(())
    }
}
