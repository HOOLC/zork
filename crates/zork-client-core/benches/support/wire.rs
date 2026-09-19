use super::{distribution, event, line, messages, SAMPLES};
use futures_util::FutureExt;
use serde_json::{json, Value};
use std::{hint::black_box, sync::Arc, time::Instant};
use zork_client_core::{
    api::{SseEvent, StationClient},
    state::{ConversationData, Device},
    store::ClientStore,
    subscriptions::{Key, WireSubscription},
};

fn apply(rows: &mut Vec<Value>, value: &mut Value) {
    let state = &mut value["state"];
    if state["messages"].is_array() {
        *rows = state["messages"]
            .take()
            .as_array_mut()
            .unwrap()
            .drain(..)
            .collect();
    }
    if let Some(edits) = state["message_edits"].as_array_mut() {
        for edit in edits {
            let remove =
                edit["start"].as_u64().unwrap() as usize..edit["end"].as_u64().unwrap() as usize;
            let insert = match edit["insert"].take() {
                Value::Array(rows) => rows,
                _ => unreachable!(),
            };
            rows.splice(remove, insert);
        }
    }
}
pub(super) fn run(count: usize, readers: usize) -> Value {
    let directory = tempfile::tempdir().unwrap();
    let store = Arc::new(ClientStore::open(directory.path()).unwrap());
    let device = Device::open(
        Arc::new(StationClient::new("http://127.0.0.1:9", None)),
        None,
        true,
    );
    let chat = device.conversation("bench");
    chat.seed(ConversationData {
        lines: (0..count).map(line).collect(),
        loaded: true,
        ..Default::default()
    });
    let mut subscriptions = (0..readers)
        .map(|_| {
            WireSubscription::from_device(
                Key::Conversation {
                    peer: "bench".into(),
                    session: Some("bench".into()),
                },
                device.clone(),
                store.clone(),
            )
            .unwrap()
        })
        .collect::<Vec<_>>();
    let mut mirrors = vec![vec![]; readers];
    let mut signals = subscriptions
        .iter()
        .map(|s| s.signals())
        .collect::<Vec<_>>();
    for ((reader, rows), signal) in subscriptions.iter_mut().zip(&mut mirrors).zip(&mut signals) {
        signal.changed().now_or_never().unwrap().unwrap();
        let initial = reader.prepare().unwrap().unwrap();
        apply(rows, &mut initial.as_ref().clone());
        assert_eq!(rows.len(), 100);
        reader.finish(initial["batch"].as_u64().unwrap(), true);
    }
    let mut report = json!({"messages":count,"readers":readers,"window":100});
    for workload in ["append", "replace_tail", "duplicate", "activity"] {
        let mut commits = vec![];
        let mut prepare = vec![];
        let mut encode = vec![];
        let mut decode = vec![];
        let mut applied = vec![];
        let mut totals = vec![];
        let mut bytes = vec![];
        let mut wakes = 0;
        for sample in 0..SAMPLES {
            let incoming = match workload {
                "append" => event(count + sample, &messages::content(count + sample)),
                "replace_tail" | "duplicate" => event(
                    count + SAMPLES - 1,
                    &format!(
                        "revision {}\n{}",
                        if workload == "duplicate" {
                            SAMPLES - 1
                        } else {
                            sample
                        },
                        messages::content(count + SAMPLES - 1)
                    ),
                ),
                _ => SseEvent {
                    name: "status".into(),
                    data: if sample % 2 == 0 {
                        "{\"state\":\"thinking\"}"
                    } else {
                        "{\"state\":\"finished\"}"
                    }
                    .into(),
                },
            };
            let start = Instant::now();
            chat.seed_event(&incoming);
            commits.push(start.elapsed().as_secs_f64() * 1e6);
            let mut size = 0;
            let (mut p, mut e, mut d, mut a) = (0., 0., 0., 0.);
            for ((reader, rows), signal) in
                subscriptions.iter_mut().zip(&mut mirrors).zip(&mut signals)
            {
                if signal.changed().now_or_never().is_some() {
                    wakes += 1;
                }
                let start = Instant::now();
                let frame = reader.prepare().unwrap();
                p += start.elapsed().as_secs_f64() * 1e6;
                let Some(frame) = frame else {
                    continue;
                };
                let start = Instant::now();
                let wire = serde_json::to_vec(frame.as_ref()).unwrap();
                e += start.elapsed().as_secs_f64() * 1e6;
                size += wire.len();
                let start = Instant::now();
                let mut parsed: Value = serde_json::from_slice(&wire).unwrap();
                d += start.elapsed().as_secs_f64() * 1e6;
                let start = Instant::now();
                apply(rows, &mut parsed);
                black_box(&rows);
                a += start.elapsed().as_secs_f64() * 1e6;
                assert!(reader.finish(frame["batch"].as_u64().unwrap(), true));
            }
            totals.push(start.elapsed().as_secs_f64() * 1e6);
            prepare.push(p);
            encode.push(e);
            decode.push(d);
            applied.push(a);
            bytes.push(size);
        }
        bytes.sort_unstable();
        report[workload] = json!({"commit":distribution(commits),"prepare_all":distribution(prepare),"encode_all":distribution(encode),
            "decode_all":distribution(decode),"apply_all":distribution(applied),"total":distribution(totals),"wakes":wakes,
            "bytes_all":{"p50":bytes[SAMPLES/2],"p95":bytes[SAMPLES*95/100],"max":bytes[SAMPLES-1]}});
    }
    assert_eq!(mirrors[0].len(), 100);
    assert_eq!(
        mirrors[0].last().unwrap()["id"],
        format!("m{}", count + SAMPLES - 1)
    );
    assert!(mirrors.iter().all(|rows| rows == &mirrors[0]));
    report
}
