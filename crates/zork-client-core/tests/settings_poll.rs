use futures_util::FutureExt;
use serde_json::json;
use std::sync::Arc;
use zork_client_core::{
    api::StationClient,
    state::Device,
    store::{ClientStore, SavedNode},
    subscriptions::{Key, WireSubscription},
    Client, Command,
};
use zork_client_types::sync::{Cursor, Kind, Page, Record, Scope};

#[tokio::test]
async fn settings_subscription_notifies_only_committed_versions_and_hides_revoked_data() {
    let root = tempfile::tempdir().unwrap();
    let store = Arc::new(ClientStore::open(root.path()).unwrap());
    store
        .save_node(&SavedNode {
            machine_name: None,
            color_key: None,
            id: "peer".into(),
            name: "Peer".into(),
            url: String::new(),
            token: None,
            local: false,
            mesh: None,
            group: None,
        })
        .unwrap();
    store.put("peer", "draft:chat", &"own draft").unwrap();
    let cursor = Cursor {
        owner: "owner".into(),
        epoch: "epoch".into(),
        scope: Scope::Catalog {},
        sequence: 1,
    };
    let profile = |name: &str, revision| Record {
        kind: Kind::Profile,
        id: "p".into(),
        revision,
        value: Some(json!({"profile_id":"p","name":name,"provider":"fixture","models":[]})),
    };
    store
        .apply_replica_page(
            "peer",
            "owner",
            &Page {
                protocol: 1,
                batch_id: "one".into(),
                from: None,
                through: cursor.clone(),
                index: 0,
                last: true,
                records: vec![profile("Before", 1)],
            },
        )
        .unwrap();
    let client = Client::open(root.path()).unwrap();
    let device = Device::open(
        Arc::new(StationClient::new("http://127.0.0.1:9", None)),
        Some((store.clone(), "peer".into())),
        true,
    );
    let mut reader = WireSubscription::from_device(
        Key::Settings {
            peer: "peer".into(),
        },
        device,
        store.clone(),
    )
    .unwrap();
    let mut signals = reader.signals();
    signals.changed().await.unwrap();
    let initial = reader.prepare().unwrap().unwrap();
    assert_eq!(initial["snapshot"]["profiles"][0]["name"], "Before");
    assert!(reader.finish(initial["batch"].as_u64().unwrap(), true));
    assert!(reader.prepare().unwrap().is_none());
    let first = Page {
        protocol: 1,
        batch_id: "two".into(),
        from: Some(cursor.clone()),
        through: Cursor {
            sequence: 2,
            ..cursor
        },
        index: 0,
        last: false,
        records: vec![profile("After", 2)],
    };
    store.apply_replica_page("peer", "owner", &first).unwrap();
    tokio::task::yield_now().await;
    assert!(
        signals.changed().now_or_never().is_none(),
        "uncommitted page escaped staging"
    );
    store
        .apply_replica_page(
            "peer",
            "owner",
            &Page {
                index: 1,
                last: true,
                records: vec![],
                ..first.clone()
            },
        )
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), signals.changed())
        .await
        .unwrap()
        .unwrap();
    let updated = reader.prepare().unwrap().unwrap();
    assert_eq!(updated["snapshot"]["profiles"][0]["name"], "After");
    assert!(reader.finish(updated["batch"].as_u64().unwrap(), true));
    assert!(updated["snapshot"].get("cursor").is_none());
    store
        .apply_replica_page(
            "peer",
            "owner",
            &Page {
                protocol: 1,
                batch_id: "cursor-only".into(),
                from: Some(first.through.clone()),
                through: Cursor {
                    sequence: 3,
                    ..first.through
                },
                index: 0,
                last: true,
                records: vec![],
            },
        )
        .unwrap();
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(30), signals.changed())
            .await
            .is_err(),
        "storage checkpoint-only advancement woke UI"
    );
    assert!(reader.prepare().unwrap().is_none());
    store.revoke_replica("peer").unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), signals.changed())
        .await
        .unwrap()
        .unwrap();
    let revoked = reader.prepare().unwrap().unwrap();
    assert_eq!(revoked["snapshot"]["revoked"], true);
    assert!(revoked["snapshot"].get("profiles").is_none());
    assert_eq!(
        client
            .local()
            .execute(Command::Conversation {
                peer: "peer".into(),
                session: "chat".into()
            })
            .unwrap()["draft"],
        "own draft"
    );
}
