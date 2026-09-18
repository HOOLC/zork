//! Real History page ingestion, publication and native patch application.
//! Build first, then run this fixed binary without compiler or UI load.
use serde_json::{json, Value};
use std::{hint::black_box, sync::Arc, time::Instant};
use zork_client_core::{api::StationClient, state::Device};
#[path = "support/history.rs"]
mod input;
use input::{fixture, record};

const SAMPLES: usize = 100;

fn distribution(mut samples: Vec<f64>) -> Value {
    samples.sort_by(f64::total_cmp);
    let p = |percent: usize| samples[(samples.len() * percent).div_ceil(100) - 1];
    json!({"samples":samples.len(),"p50_us":p(50),"p95_us":p(95),"p99_us":p(99)})
}

fn run(count: usize, readers: usize) -> Value {
    let device = Device::open(
        Arc::new(StationClient::new("http://127.0.0.1:9", None)),
        None,
        true,
    );
    let history = device.conversation("history-bench").history();
    let initial = (0..count).map(fixture).collect();
    let cold = Instant::now();
    history.seed_records(initial, false);
    let cold_us = cold.elapsed().as_secs_f64() * 1e6;
    let mut subscriptions = (0..readers)
        .map(|_| history.subscribe())
        .collect::<Vec<_>>();
    let mut mirrors = subscriptions
        .iter_mut()
        .map(|reader| reader.snapshot().state.entries.clone())
        .collect::<Vec<_>>();
    let mut report = json!({"records":count,"readers":readers,"cold_us":cold_us,"initial_entries":mirrors[0].len()});
    for workload in ["append", "complete_far", "prepend_100", "duplicate"] {
        let mut commits = Vec::new();
        let mut consumes = Vec::new();
        let mut totals = Vec::new();
        let mut changed_rows = 0;
        for sample in 0..SAMPLES {
            let incoming = match workload {
                "append" => vec![record(format!("append{sample}"), json!({"kind":"input_appended","input":{"content":"new input","received_at_ms":(count+sample)*10}}))],
                "complete_far" => vec![record(format!("finish{sample}"), json!({"kind":"step_completed","step_id":format!("open{}", sample*53 % (count/10)),"completed_at_ms":(count+sample)*10,"assistant_text":"late completion","usage":{"input_tokens":42}}))],
                "prepend_100" => (0..100).map(|i| {
                    let position = -((sample + 1) as i64 * 100) + i;
                    record(format!("older{position}"), json!({"kind":"input_appended","input":{"content":"older input","received_at_ms":position*10}}))
                }).collect(),
                _ => vec![fixture(count / 2)],
            };
            let start = Instant::now();
            history.seed_records(incoming, workload == "prepend_100");
            commits.push(start.elapsed().as_secs_f64() * 1e6);
            let consuming = Instant::now();
            for (subscription, mirror) in subscriptions.iter_mut().zip(mirrors.iter_mut()) {
                let update = subscription.snapshot();
                if update.reset {
                    *mirror = update.state.entries.clone();
                } else if let Some(entries) = update.entries {
                    for edit in entries.edits {
                        changed_rows += edit.insert.len();
                        assert!(edit.apply(mirror));
                    }
                }
                black_box(&mirror);
            }
            consumes.push(consuming.elapsed().as_secs_f64() * 1e6);
            totals.push(start.elapsed().as_secs_f64() * 1e6);
        }
        for (subscription, mirror) in subscriptions.iter_mut().zip(&mirrors) {
            assert_eq!(*mirror, subscription.snapshot().state.entries);
        }
        report[workload] = json!({"commit":distribution(commits),"consume":distribution(consumes),"total":distribution(totals),"inserted_rows_all_readers":changed_rows});
    }
    report
}

fn main() {
    let runs = [1_000, 10_000, 100_000]
        .into_iter()
        .flat_map(|count| [1, 8].map(|readers| run(count, readers)))
        .collect::<Vec<_>>();
    println!("{}", serde_json::to_string_pretty(&json!({"scope":"core History production reducer, native publication and ordered patch application; no network, JSON wire, UI grouping or rendering","fixture":"10 event forms: model selections, starts/completions, tool arguments/results, input, wait/wakeup, usage and long bodies; 100 samples per workload","runs":runs})).unwrap());
}
