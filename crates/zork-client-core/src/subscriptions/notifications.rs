use super::snapshot::SnapshotWire;
use crate::store::ClientStore;
use anyhow::Result;
use serde_json::Value;
use std::sync::Arc;
use zork_observe::ValueSource;

pub(super) struct NotificationsWire {
    pub wire: SnapshotWire,
    task: tokio::task::JoinHandle<()>,
    store: Arc<ClientStore>,
    peers: Vec<String>,
    settings: Value,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        notifications::{self, mobile::Action, Kind, Ledger, Notice},
        Command,
    };
    use serde_json::json;
    #[tokio::test]
    async fn notification_wire_redacts_invalidates_privacy_and_keeps_receipts_distinct_from_reading(
    ) {
        let root = tempfile::tempdir().unwrap();
        let client = crate::Client::open(root.path()).unwrap();
        let store = client.store.clone();
        store
            .save_node(&crate::store::SavedNode {
                machine_name: None,
                id: "node".into(),
                name: "workstation".into(),
                url: String::new(),
                token: None,
                local: false,
                mesh: None,
                group: None,
            })
            .unwrap();
        let notice = Notice {
            id: "event".into(),
            node: "node".into(),
            session: Some("conversation".into()),
            task: None,
            leader: None,
            title: "private conversation name".into(),
            kind: Kind::Reply,
            created_at_ms: crate::store::delivery_now_ms(),
        };
        let tag = notice.tag();
        let mut ledger = Ledger::default();
        ledger.pending.insert(tag.clone(), notice);
        store.put("node", notifications::KEY, &ledger).unwrap();
        let mut wire =
            crate::subscriptions::WireSubscription::from_notifications(store.clone()).unwrap();
        let first = wire.prepare().unwrap().unwrap();
        assert_eq!(first["snapshot"]["pending"][0]["title"], "Zork");
        assert!(!first.to_string().contains("private conversation"));
        client
            .local()
            .execute(Command::NotificationSettings {
                operation: Some(Action::Preview { value: true }),
            })
            .unwrap();
        assert!(
            !wire.valid(first["batch"].as_u64().unwrap()),
            "old privacy settings were accepted"
        );
        let revealed = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if let Some(frame) = wire.prepare().unwrap() {
                    if frame["snapshot"]["settings"]["preview"] == true {
                        break frame;
                    }
                    wire.finish(frame["batch"].as_u64().unwrap(), false);
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            revealed["snapshot"]["pending"][0]["title"],
            "private conversation name"
        );
        assert!(wire.finish(revealed["batch"].as_u64().unwrap(), true));
        client
            .local()
            .execute(Command::NotificationReceipt {
                peer: "node".into(),
                id: "event".into(),
            })
            .unwrap();
        assert!(store
            .get::<Ledger>("node", notifications::KEY)
            .unwrap()
            .unwrap()
            .pending
            .is_empty());
        assert!(store
            .get::<Value>("node", "navigation-seen")
            .unwrap()
            .is_none());
        let opened = client
            .local()
            .execute(Command::OpenNotification { tag: tag.clone() })
            .unwrap();
        assert_eq!(opened["session"], "conversation");
        client
            .local()
            .execute(Command::NotificationSettings {
                operation: Some(Action::Preview { value: false }),
            })
            .unwrap();
        assert_eq!(
            notifications::mobile::snapshot(&store).unwrap()["retained"],
            json!([])
        );
        store.revoke_replica("node").unwrap();
        assert!(client
            .local()
            .execute(Command::OpenNotification { tag })
            .is_err());
    }
    #[tokio::test]
    async fn replaced_service_cannot_be_stopped_by_the_previous_instances_cleanup() {
        let root = tempfile::tempdir().unwrap();
        let mut client = crate::Client::open(root.path()).unwrap();
        for (running, instance) in [(true, "old"), (true, "new"), (false, "old")] {
            client
                .execute(Command::BackgroundService {
                    running,
                    instance: instance.into(),
                })
                .await
                .unwrap();
        }
        assert_eq!(client.background_service.as_deref(), Some("new"));
        client
            .execute(Command::BackgroundService {
                running: false,
                instance: "new".into(),
            })
            .await
            .unwrap();
        assert!(client.background_service.is_none());
        client
            .execute(Command::HostVisibility {
                visible: false,
                generation: 2,
            })
            .await
            .unwrap();
        client
            .execute(Command::HostVisibility {
                visible: true,
                generation: 1,
            })
            .await
            .unwrap();
        assert!(
            !client.foreground,
            "a delayed old Activity restored foreground ownership"
        );
    }
}
impl Drop for NotificationsWire {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl NotificationsWire {
    pub fn new(store: Arc<ClientStore>) -> Result<Self> {
        let mut events = store.notification_events();
        events.snapshot();
        let source = Arc::new(ValueSource::new(crate::notifications::mobile::snapshot(
            &store,
        )?));
        let wire = SnapshotWire::new(&source);
        let db = store.clone();
        let task = tokio::spawn(async move {
            while events.changed().await.is_some() {
                if let Ok(value) = crate::notifications::mobile::snapshot(&db) {
                    source.publish(value);
                }
            }
        });
        Ok(Self {
            wire,
            task,
            store,
            peers: vec![],
            settings: Value::Null,
        })
    }
    pub fn valid(&self) -> bool {
        self.wire.valid()
            && self
                .peers
                .iter()
                .all(|peer| !self.store.replica_revoked(peer).unwrap_or(true))
            && crate::notifications::mobile::settings(&self.store)
                .ok()
                .as_ref()
                == Some(&self.settings)
    }
    pub fn prepare(&mut self) -> Result<Option<(Value, bool, bool)>> {
        let batch = self.wire.prepare()?;
        if let Some((value, _, _)) = &batch {
            self.settings = value["snapshot"]["settings"].clone();
            self.peers = value["snapshot"]["pending"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|n| n["peer"].as_str().map(str::to_owned))
                .collect();
        }
        Ok(batch)
    }
}
