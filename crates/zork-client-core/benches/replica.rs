//! Run separately from builds/UI load: cargo bench --locked -p zork-client-core --bench replica
//! Measures product storage/query work, not GPU frames or native scrolling.
#[path = "support/messages.rs"]
mod fixture;
use fixture::content;
use serde_json::{json, Value};
use std::time::Instant;
use zork_client_core::{
    store::{ClientStore, SavedNode},
    Client, Command,
};
use zork_client_types::sync::{Cursor, Kind, Page, Record, Scope, MAX_PAGE_RECORDS};
const MESSAGES: usize = 100_000;
fn samples(client: &Client) -> Value {
    let mut samples = Vec::new();
    for _ in 0..200 {
        let start = Instant::now();
        let value = client
            .local()
            .execute(Command::Settings {
                peer: "peer".into(),
                cached_only: true,
            })
            .unwrap();
        assert_eq!(value["agents"].as_array().unwrap().len(), 64);
        assert_eq!(value["profiles"].as_array().unwrap().len(), 16);
        samples.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    samples.sort_by(f64::total_cmp);
    json!({"samples":samples.len(),"p50_ms":samples[99],"p95_ms":samples[189],"p99_ms":samples[197],"max_ms":samples[199]})
}
fn main() {
    let dir = tempfile::tempdir().unwrap();
    let store = ClientStore::open(dir.path()).unwrap();
    store
        .save_node(&SavedNode {
            machine_name: None, color_key: None,
            id: "peer".into(),
            name: "Fixture".into(),
            url: String::new(),
            token: None,
            local: false,
            mesh: None,
            group: None,
        })
        .unwrap();
    let mut records = vec![Record {
        kind: Kind::Device,
        id: "self".into(),
        revision: 1,
        value: Some(json!({"name":"Fixture"})),
    }];
    for i in 0..64 {
        records.push(Record{kind:Kind::Agent,id:format!("agent-{i}"),revision:records.len() as u64+1,value:Some(json!({"id":format!("agent-{i}"),"name":format!("队员 {i}"),"role":"worker","profile_id":"p0","model":"fixture"}))});
    }
    for i in 0..16 {
        records.push(Record {
            kind: Kind::Profile,
            id: format!("p{i}"),
            revision: records.len() as u64 + 1,
            value: Some(json!({"profile_id":format!("p{i}"),"provider":"fixture","models":[]})),
        });
    }
    let metadata_count = records.len();
    store
        .apply_replica_page(
            "peer",
            "owner",
            &Page {
                protocol: 1,
                batch_id: "catalog".into(),
                from: None,
                through: Cursor {
                    owner: "owner".into(),
                    epoch: "epoch".into(),
                    scope: Scope::Catalog {},
                    sequence: metadata_count as u64,
                },
                index: 0,
                last: true,
                records,
            },
        )
        .unwrap();
    let client = Client::open(dir.path()).unwrap();
    let before = samples(&client);
    drop(client);
    let scope = Scope::Conversation { id: "chat".into() };
    let through = Cursor {
        owner: "owner".into(),
        epoch: "epoch".into(),
        scope: scope.clone(),
        sequence: (metadata_count + MESSAGES) as u64,
    };
    let start = Instant::now();
    for offset in (0..MESSAGES).step_by(MAX_PAGE_RECORDS) {
        let records=(offset..(offset+MAX_PAGE_RECORDS).min(MESSAGES)).map(|i|Record{kind:Kind::Message,id:format!("message-{i:06}"),revision:(metadata_count+i+1) as u64,value:Some(json!({"message_id":format!("message-{i:06}"),"sequence":i+1,"role":if i%2==0{"user"}else{"assistant"},"text":content(i),"kind":if i%2==0{None}else{Some("final")},"created_at":"2026-09-08T00:00:00Z"}))}).collect();
        store
            .apply_replica_page(
                "peer",
                "owner",
                &Page {
                    protocol: 1,
                    batch_id: "messages".into(),
                    from: None,
                    through: through.clone(),
                    index: (offset / MAX_PAGE_RECORDS) as u32,
                    last: offset + MAX_PAGE_RECORDS >= MESSAGES,
                    records,
                },
            )
            .unwrap();
    }
    let bootstrap_ms = start.elapsed().as_secs_f64() * 1000.0;
    let total: usize = {
        let db = rusqlite::Connection::open(dir.path().join("client.db")).unwrap();
        db.query_row(
            "SELECT COUNT(*) FROM replica_entities WHERE kind='message'",
            [],
            |r| r.get(0),
        )
        .unwrap()
    };
    assert_eq!(total, MESSAGES);
    for i in [0, MESSAGES / 2, MESSAGES - 1] {
        assert!(store
            .replica_record("peer", &scope, Kind::Message, &format!("message-{i:06}"))
            .unwrap()
            .unwrap()
            .value
            .is_some());
    }
    let client = Client::open(dir.path()).unwrap();
    let after = samples(&client);
    let changed = Record {
        kind: Kind::Message,
        id: "message-050000".into(),
        revision: through.sequence + 1,
        value: Some(json!({"text":"edited record","sequence":50001})),
    };
    let delta = Page {
        protocol: 1,
        batch_id: "delta".into(),
        from: Some(through.clone()),
        through: Cursor {
            sequence: through.sequence + 1,
            ..through
        },
        index: 0,
        last: true,
        records: vec![changed],
    };
    let start = Instant::now();
    store.apply_replica_page("peer", "owner", &delta).unwrap();
    let delta_ms = start.elapsed().as_secs_f64() * 1000.0;
    assert_eq!(
        store.replica_catalog("peer").unwrap().unwrap().1.len(),
        metadata_count
    );
    assert!(
        after["p99_ms"].as_f64().unwrap() < 100.0,
        "cached navigation budget exceeded"
    );
    let report = json!({"messages":total,"content_variants":12,"roles":["user","assistant"],"catalog_records":metadata_count,"bootstrap_ms":bootstrap_ms,"single_entity_delta_ms":delta_ms,"settings_before_history":before,"settings_after_history":after,"validated_positions":[0,50000,99999],"scope":"storage and cached metadata reads; not rendering"});
    println!("{}", serde_json::to_string_pretty(&report).unwrap());
    if let Ok(path) = std::env::var("ZORK_REPLICA_BENCH_REPORT") {
        std::fs::write(path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    }
}
