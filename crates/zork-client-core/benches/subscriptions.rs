//! Build with --no-run, then run separately from compilation and UI load.
//! Measures the real Conversation reducer and native subscription, without IO.
#[path = "support/messages.rs"]
mod messages;
#[path = "support/wire.rs"]
mod wire;

use serde_json::{json, Value};
use std::{hint::black_box, sync::Arc, time::Instant};
use zork_client_core::{
    api::{StationClient, MessageMetadata, Role, SseEvent},
    state::{ConversationData, Device},
    transcript::TranscriptLine,
};

const SAMPLES: usize = 100;

fn line(index: usize) -> TranscriptLine {
    TranscriptLine::Message {
        role: if index % 2 == 0 {
            Role::User
        } else {
            Role::Assistant
        },
        content: messages::content(index),
        metadata: MessageMetadata {
            id: Some(format!("m{index}")),
            created_at: Some("2026-09-10T00:00:00Z".into()),
            author_name: Some("fixture".into()),
            ..Default::default()
        },
    }
}

fn event(index: usize, content: &str) -> SseEvent {
    SseEvent {
        name: "message".into(),
        data:
            json!({"type":"message","role":"assistant","id":format!("m{index}"),"content":content})
                .to_string(),
    }
}

fn distribution(mut samples: Vec<f64>) -> Value {
    samples.sort_by(f64::total_cmp);
    let percentile = |p: usize| samples[(samples.len() * p).div_ceil(100).saturating_sub(1)];
    json!({"samples":samples.len(),"p50_us":percentile(50),"p95_us":percentile(95),"p99_us":percentile(99)})
}

fn run(count: usize, readers: usize) -> Value {
    let device = Device::open(
        Arc::new(StationClient::new("http://127.0.0.1:9", None)),
        None,
        true,
    );
    let conversation = device.conversation("bench");
    conversation.seed(ConversationData {
        lines: (0..count).map(line).collect(),
        loaded: true,
        ..Default::default()
    });
    let mut subscriptions = (0..readers)
        .map(|_| conversation.subscribe())
        .collect::<Vec<_>>();
    for subscription in &mut subscriptions {
        black_box(subscription.snapshot());
    }
    let mut report = json!({"messages":count,"readers":readers});
    for workload in ["append", "replace_tail", "duplicate", "activity"] {
        let mut commits = Vec::with_capacity(SAMPLES);
        let mut reads = Vec::with_capacity(SAMPLES);
        let mut totals = Vec::with_capacity(SAMPLES);
        for sample in 0..SAMPLES {
            let incoming = match workload {
                "append" => event(count + sample, "new message"),
                "replace_tail" => event(count + SAMPLES - 1, &format!("stream revision {sample}")),
                "duplicate" => event(
                    count + SAMPLES - 1,
                    &format!("stream revision {}", SAMPLES - 1),
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
            conversation.seed_event(&incoming);
            let committed = start.elapsed().as_secs_f64() * 1_000_000.;
            let reading = Instant::now();
            for subscription in &mut subscriptions {
                black_box(subscription.snapshot());
            }
            reads.push(reading.elapsed().as_secs_f64() * 1_000_000.);
            totals.push(start.elapsed().as_secs_f64() * 1_000_000.);
            commits.push(committed);
        }
        report[workload] = json!({"commit":distribution(commits),"read_all":distribution(reads),"total":distribution(totals)});
    }
    assert_eq!(conversation.snapshot().lines.len(), count + SAMPLES);
    report
}

fn main() {
    let runs = [1_000, 10_000, 100_000]
        .into_iter()
        .flat_map(|count| [1, 8].map(|readers| run(count, readers)))
        .collect::<Vec<_>>();
    let wire_runs = [1_000, 10_000, 100_000]
        .into_iter()
        .flat_map(|count| [1, 8].map(|readers| wire::run(count, readers)))
        .collect::<Vec<_>>();
    let report = json!({
        "scope":"real core Conversation reducer + native subscription; no network, disk, JNI, rendering or frame clock",
        "fixture":"12 mixed message forms, long content, metadata; 100 samples per workload",
        "runs":runs,
        "wire_scope":"same core projection, requested last100 window; DTO prepare/JSON encode/decode/reference mirror apply, not JNI or Kotlin/device frames",
        "wire_runs":wire_runs
    });
    println!("{}", serde_json::to_string_pretty(&report).unwrap());
}
