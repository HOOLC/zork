//! Command receipt, public projection and product mutation share one SQLite
//! transaction. Reusing an id with a different intent is always rejected.
use super::*;
use anyhow::ensure;
use zork_client_types::sync::{Action, Decision, Mutation, Outcome, Receipt, Record, Scope};

pub(super) fn initialize(conn: &Connection) -> Result<()> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS sync_receipts(id TEXT PRIMARY KEY,owner TEXT NOT NULL,intent TEXT NOT NULL,receipt TEXT NOT NULL,created TEXT NOT NULL);")?;
    Ok(())
}
impl StationDb {
    pub fn sync_receipt(&self, owner: &str, id: &str) -> Result<Option<Receipt>> {
        let value: Option<String> = self
            .conn
            .lock()
            .expect("db mutex")
            .query_row(
                "SELECT receipt FROM sync_receipts WHERE id=?1 AND owner=?2",
                params![id, owner],
                |r| r.get(0),
            )
            .optional()?;
        value.map(|v| Ok(serde_json::from_str(&v)?)).transpose()
    }
    pub fn sync_mutate(&self, owner: &str, request: &Mutation) -> Result<Receipt> {
        request.validate().map_err(anyhow::Error::msg)?;
        ensure!(
            request
                .owner
                .as_deref()
                .is_none_or(|expected| expected == owner),
            "sync_owner_mismatch"
        );
        let intent = serde_json::to_string(request)?;
        let mut conn = self.conn.lock().expect("db mutex");
        let tx = conn.transaction()?;
        let existing: Option<(String, String, String)> = tx
            .query_row(
                "SELECT owner,intent,receipt FROM sync_receipts WHERE id=?1",
                [&request.request_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        if let Some((old_owner, old_intent, receipt)) = existing {
            ensure!(
                old_owner == owner && old_intent == intent,
                "sync_request_id_reused"
            );
            self.sync_record_watermark(&tx)?;
            return Ok(serde_json::from_str(&receipt)?);
        }
        let epoch: String = tx.query_row("SELECT epoch FROM sync_meta", [], |r| r.get(0))?;
        let (kind, id) = request.action.entity();
        let current = |conn: &Connection| -> Result<Option<Record>> {
            let row: Option<(u64, Option<String>)> = conn
                .query_row(
                    "SELECT revision,value FROM sync_entities WHERE scope=?1 AND kind=?2 AND id=?3",
                    params![Scope::Catalog {}.key(), kind.key(), id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            row.map(|(revision, value)| {
                Ok(Record {
                    kind,
                    id: id.into(),
                    revision,
                    value: value.map(|v| serde_json::from_str(&v)).transpose()?,
                })
            })
            .transpose()
        };
        let entity = current(&tx)?;
        let (outcome, reason) = if epoch != request.epoch {
            (Outcome::Conflict, Some("sync_epoch_changed".to_owned()))
        } else if entity
            .as_ref()
            .is_none_or(|e| e.revision != request.expected_revision || e.value.is_none())
        {
            (
                Outcome::Conflict,
                Some("entity_revision_conflict".to_owned()),
            )
        } else {
            // A savepoint also makes future multi-statement actions safe when
            // validation fails after the first write.
            tx.execute_batch("SAVEPOINT sync_command_action")?;
            let result: Result<()> = match &request.action {
                Action::AgentAvatar { id, avatar } => {
                    tx.execute(
                        "UPDATE node_agents SET value=json_set(value,'$.avatar',?2) WHERE id=?1",
                        params![id, avatar],
                    )?;
                    Ok(())
                }
                Action::TaskDecision {
                    id,
                    expected_task_revision,
                    decision,
                } => {
                    let action = match decision {
                        Decision::Accept => TaskAction::Accept,
                        Decision::Reopen => TaskAction::Reopen,
                        Decision::Cancel => TaskAction::Cancel,
                    };
                    tasks::transition_task(&tx, id, *expected_task_revision, action).map(|_| ())
                }
            };
            match result {
                Ok(()) => {
                    tx.execute_batch("RELEASE sync_command_action")?;
                    (Outcome::Applied, None)
                }
                Err(error) => {
                    tx.execute_batch(
                        "ROLLBACK TO sync_command_action; RELEASE sync_command_action",
                    )?;
                    (Outcome::Rejected, Some(error.to_string()))
                }
            }
        };
        let receipt = Receipt {
            request_id: request.request_id.clone(),
            owner: owner.into(),
            epoch,
            outcome,
            reason,
            entity: current(&tx)?,
        };
        tx.execute(
            "INSERT INTO sync_receipts(id,owner,intent,receipt,created) VALUES(?1,?2,?3,?4,?5)",
            params![
                request.request_id,
                owner,
                intent,
                serde_json::to_string(&receipt)?,
                now_rfc3339()
            ],
        )?;
        self.sync_record_watermark(&tx)?;
        tx.commit()?;
        Ok(receipt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, StationDb, Mutation) {
        let dir = tempfile::tempdir().unwrap();
        let db = StationDb::open(dir.path(), &dir.path().join("workspaces")).unwrap();
        db.conn.lock().unwrap().execute("INSERT INTO node_agents(id,value) VALUES('a',?1)",[json!({"id":"a","name":"A","role":"leader","profile_id":"p","model":"m","thinking":"off","instructions":"","allowed_leaders":[],"avatar":"cat"}).to_string()]).unwrap();
        let (epoch,revision)=db.conn.lock().unwrap().query_row("SELECT m.epoch,e.revision FROM sync_meta m,sync_entities e WHERE e.kind='agent' AND e.id='a'",[],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
        let intent = Mutation {
            request_id: "operation".into(),
            owner: Some("owner".into()),
            epoch,
            expected_revision: revision,
            action: Action::AgentAvatar {
                id: "a".into(),
                avatar: "fox".into(),
            },
        };
        (dir, db, intent)
    }
    #[test]
    fn replay_is_identical_and_conflicting_clients_cannot_overwrite() {
        let (_dir, db, intent) = fixture();
        let first = db.sync_mutate("owner", &intent).unwrap();
        assert_eq!(first.outcome, Outcome::Applied);
        assert_eq!(db.sync_mutate("owner", &intent).unwrap(), first);
        let mut reused = intent.clone();
        reused.action = Action::AgentAvatar {
            id: "a".into(),
            avatar: "bear".into(),
        };
        assert!(db.sync_mutate("owner", &reused).is_err());
        reused.request_id = "competing".into();
        let conflict = db.sync_mutate("owner", &reused).unwrap();
        assert_eq!(conflict.outcome, Outcome::Conflict);
        assert_eq!(conflict.entity, first.entity);
        assert_eq!(db.sync_receipt("owner", "operation").unwrap(), Some(first));
        assert!(db.sync_receipt("other", "operation").unwrap().is_none());
        let mut foreign = intent.clone();
        foreign.request_id = "foreign-owner".into();
        foreign.owner = Some("other".into());
        assert!(db.sync_mutate("owner", &foreign).is_err());
    }
    #[test]
    fn epoch_change_does_not_execute_old_intent() {
        let (_dir, db, mut intent) = fixture();
        intent.epoch = "before-restore".into();
        let receipt = db.sync_mutate("owner", &intent).unwrap();
        assert_eq!(receipt.outcome, Outcome::Conflict);
        assert_eq!(receipt.entity.unwrap().value.unwrap()["avatar"], "cat");
    }
}

#[cfg(test)]
mod recovery_tests {
    use super::*;
    use axum::{extract::State, http::StatusCode, routing::post, Json, Router};
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    use zork_client_core::{api::StationClient, store::ClientStore, sync::Coordinator};
    use zork_client_types::sync::{Kind, Pull};
    #[tokio::test]
    async fn lost_ack_is_queried_after_restart_without_resubmitting() {
        let dir = tempfile::tempdir().unwrap();
        let db = Arc::new(StationDb::open(dir.path(), &dir.path().join("workspaces")).unwrap());
        db.conn.lock().unwrap().execute("INSERT INTO node_agents(id,value) VALUES('a',?1)",[json!({"id":"a","name":"A","role":"leader","profile_id":"p","model":"m","thinking":"off","instructions":"","allowed_leaders":[],"avatar":"cat"}).to_string()]).unwrap();
        let writes = Arc::new(AtomicUsize::new(0));
        let count = writes.clone();
        let router = Router::new()
            .route(
                "/v1/node/sync",
                post(
                    |State(db): State<Arc<StationDb>>, Json(p): Json<Pull>| async move {
                        Json(db.sync_pull("owner", &p).unwrap())
                    },
                ),
            )
            .route(
                "/v1/node/sync/commands",
                post(
                    move |State(db): State<Arc<StationDb>>, Json(intent): Json<Mutation>| {
                        let count = count.clone();
                        async move {
                            db.sync_mutate("owner", &intent).unwrap();
                            count.fetch_add(1, Ordering::SeqCst);
                            StatusCode::SERVICE_UNAVAILABLE
                        }
                    },
                ),
            )
            .route(
                "/v1/node/sync/receipt",
                post(
                    |State(db): State<Arc<StationDb>>, Json(value): Json<Value>| async move {
                        Json(
                            db.sync_receipt("owner", value["request_id"].as_str().unwrap())
                                .unwrap(),
                        )
                    },
                ),
            )
            .with_state(db);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let local = tempfile::tempdir().unwrap();
        let store = Arc::new(ClientStore::open(local.path()).unwrap());
        let client = Arc::new(StationClient::new(url, None));
        let coordinator =
            Coordinator::new(client.clone(), store.clone(), "peer".into(), "owner".into()).unwrap();
        coordinator.refresh(Scope::Catalog {}).await.unwrap();
        let cursor = store
            .replica_state("peer", &Scope::Catalog {})
            .unwrap()
            .cursor
            .unwrap();
        let revision = store
            .replica_record("peer", &Scope::Catalog {}, Kind::Agent, "a")
            .unwrap()
            .unwrap()
            .revision;
        let intent = Mutation {
            request_id: "lost-ack".into(),
            owner: Some("owner".into()),
            epoch: cursor.epoch.clone(),
            expected_revision: revision,
            action: Action::AgentAvatar {
                id: "a".into(),
                avatar: "fox".into(),
            },
        };
        assert!(coordinator.mutate(&intent).await.is_err());
        assert_eq!(
            store
                .replica_state("peer", &Scope::Catalog {})
                .unwrap()
                .cursor,
            Some(cursor)
        );
        assert_eq!(store.pending_operations("peer").unwrap().len(), 1);
        drop(coordinator);
        drop(store);
        let store = Arc::new(ClientStore::open(local.path()).unwrap());
        let coordinator =
            Coordinator::new(client, store.clone(), "peer".into(), "owner".into()).unwrap();
        coordinator.refresh(Scope::Catalog {}).await.unwrap();
        assert_eq!(writes.load(Ordering::SeqCst), 1);
        assert!(store.pending_operations("peer").unwrap().is_empty());
        assert_eq!(
            store
                .operation_receipt("peer", "lost-ack")
                .unwrap()
                .unwrap()
                .outcome,
            Outcome::Applied
        );
        assert_eq!(
            store
                .replica_record("peer", &Scope::Catalog {}, Kind::Agent, "a")
                .unwrap()
                .unwrap()
                .value
                .unwrap()["avatar"],
            "fox"
        );
        server.abort();
    }
}
