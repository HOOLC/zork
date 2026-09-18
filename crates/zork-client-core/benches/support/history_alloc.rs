use super::{distribution, measure, LIVE};
#[path = "history.rs"]
mod input;
use input::{fixture, record};
use serde_json::{json, Value};
use std::sync::{atomic::Ordering::Relaxed, Arc};
use zork_client_core::{api::StationClient, state::Device};

fn scenario(count: usize, readers: usize) -> Value {
    let device = Device::open(
        Arc::new(StationClient::new("http://127.0.0.1:9", None)),
        None,
        true,
    );
    let history = device.conversation("history-alloc").history();
    let before = LIVE.load(Relaxed);
    history.seed_records((0..count).map(fixture).collect(), false);
    let retained = (LIVE.load(Relaxed) - before).max(0);
    let mut subscriptions = (0..readers)
        .map(|_| history.subscribe())
        .collect::<Vec<_>>();
    let mut mirrors = subscriptions
        .iter_mut()
        .map(|reader| reader.snapshot().state.entries.clone())
        .collect::<Vec<_>>();
    let mut report =
        json!({"records":count,"readers":readers,"retained_history_heap_bytes":retained});
    for workload in ["append", "complete_far", "prepend_100", "duplicate"] {
        let mut samples = Vec::new();
        for sample in 0..100 {
            let incoming = match workload {
                "append" => vec![record(format!("append{sample}"), json!({"kind":"input_appended","input":{"content":"new input","received_at_ms":(count+sample)*10}}))],
                "complete_far" => vec![record(format!("finish{sample}"), json!({"kind":"step_completed","step_id":format!("open{}", sample*53 % (count/10)),"completed_at_ms":(count+sample)*10,"assistant_text":"late completion","usage":{"input_tokens":42}}))],
                "prepend_100" => (0..100).map(|i| {
                    let position = -((sample + 1) as i64 * 100) + i;
                    record(format!("older{position}"), json!({"kind":"input_appended","input":{"content":"older input","received_at_ms":position*10}}))
                }).collect(),
                _ => vec![fixture(count / 2)],
            };
            samples.push(measure(|| {
                history.seed_records(incoming, workload == "prepend_100");
                for (reader, mirror) in subscriptions.iter_mut().zip(&mut mirrors) {
                    let update = reader.snapshot();
                    if update.reset {
                        *mirror = update.state.entries.clone();
                    } else if let Some(changes) = update.entries {
                        for edit in changes.edits {
                            assert!(edit.apply(mirror));
                        }
                    }
                }
            }));
        }
        for (reader, mirror) in subscriptions.iter_mut().zip(&mirrors) {
            assert_eq!(*mirror, reader.snapshot().state.entries);
        }
        report[workload] = json!({"allocation_calls":distribution(&samples,0),"allocated_bytes":distribution(&samples,1),"additional_live_bytes_peak":distribution(&samples,2)});
    }
    report
}

pub fn run() {
    let runs = [1000, 10_000, 100_000]
        .into_iter()
        .flat_map(|count| [1, 8].map(|readers| scenario(count, readers)))
        .collect::<Vec<_>>();
    println!("{}", serde_json::to_string_pretty(&json!({"scope":"History production reducer plus native patch application; same mixed fixture as history timing. System allocator calls including realloc; not process RSS or UI rendering. Retained heap includes authoritative records, entries and indexes, measured before subscribing.","runs":runs})).unwrap());
}
