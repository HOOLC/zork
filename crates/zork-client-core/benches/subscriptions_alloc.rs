//! Allocation instrumentation is a separate executable so it cannot change the
//! timing benchmark's allocator or invalidate its before/after comparison.
#[path = "support/history_alloc.rs"]
mod history;
#[path = "support/messages.rs"]
mod messages;
use serde_json::{json, Value};
use std::{
    alloc::{GlobalAlloc, Layout, System},
    hint::black_box,
    sync::{
        atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering::Relaxed},
        Arc,
    },
};
use zork_client_core::{
    api::{StationClient, MessageMetadata, Role, SseEvent},
    state::{ConversationData, Device},
    transcript::TranscriptLine,
};

struct Allocator;
static ACTIVE: AtomicBool = AtomicBool::new(false);
static CALLS: AtomicU64 = AtomicU64::new(0);
static BYTES: AtomicU64 = AtomicU64::new(0);
static LIVE: AtomicI64 = AtomicI64::new(0);
static PEAK: AtomicI64 = AtomicI64::new(0);
fn allocated(bytes: usize, change: i64) {
    let live = LIVE.fetch_add(change, Relaxed) + change;
    if ACTIVE.load(Relaxed) {
        CALLS.fetch_add(1, Relaxed);
        BYTES.fetch_add(bytes as u64, Relaxed);
        PEAK.fetch_max(live, Relaxed);
    }
}
unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = System.alloc(layout);
        if !ptr.is_null() {
            allocated(layout.size(), layout.size() as i64);
        }
        ptr
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let ptr = System.alloc_zeroed(layout);
        if !ptr.is_null() {
            allocated(layout.size(), layout.size() as i64);
        }
        ptr
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let ptr = System.realloc(ptr, layout, size);
        if !ptr.is_null() {
            allocated(size, size as i64 - layout.size() as i64);
        }
        ptr
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size() as i64, Relaxed);
        System.dealloc(ptr, layout);
    }
}
#[global_allocator]
static ALLOCATOR: Allocator = Allocator;

fn measure(work: impl FnOnce()) -> [u64; 3] {
    let baseline = LIVE.load(Relaxed);
    CALLS.store(0, Relaxed);
    BYTES.store(0, Relaxed);
    PEAK.store(baseline, Relaxed);
    ACTIVE.store(true, Relaxed);
    work();
    ACTIVE.store(false, Relaxed);
    [
        CALLS.load(Relaxed),
        BYTES.load(Relaxed),
        PEAK.load(Relaxed).saturating_sub(baseline).max(0) as u64,
    ]
}
fn distribution(samples: &[[u64; 3]], column: usize) -> Value {
    let mut values = samples.iter().map(|row| row[column]).collect::<Vec<_>>();
    values.sort_unstable();
    json!({"p50":values[values.len()/2],"p95":values[values.len()*95/100],"max":values[values.len()-1]})
}
fn event(id: usize, content: &str) -> SseEvent {
    SseEvent {
        name: "message".into(),
        data: json!({"type":"message","role":"user","id":format!("m{id}"),"content":content})
            .to_string(),
    }
}
fn run(count: usize, consumers: usize) -> Value {
    let device = Device::open(
        Arc::new(StationClient::new("http://127.0.0.1:9", None)),
        None,
        true,
    );
    let chat = device.conversation("bench");
    chat.seed(ConversationData {
        lines: (0..count)
            .map(|i| TranscriptLine::Message {
                role: Role::User,
                content: messages::content(i),
                metadata: MessageMetadata {
                    id: Some(format!("m{i}")),
                    ..Default::default()
                },
            })
            .collect(),
        loaded: true,
        ..Default::default()
    });
    let mut readers = (0..consumers).map(|_| chat.subscribe()).collect::<Vec<_>>();
    for reader in &mut readers {
        black_box(reader.snapshot());
    }
    let mut report = json!({"messages":count,"consumers":consumers});
    for workload in ["append", "replace_tail", "duplicate", "activity"] {
        let mut samples = Vec::new();
        for i in 0..100 {
            let input = match workload {
                "append" => event(count + i, "new message"),
                "replace_tail" => event(count + 99, &format!("revision {i}")),
                "duplicate" => event(count + 99, "revision 99"),
                _ => SseEvent {
                    name: "status".into(),
                    data: if i % 2 == 0 {
                        "{\"state\":\"thinking\"}"
                    } else {
                        "{\"state\":\"finished\"}"
                    }
                    .into(),
                },
            };
            samples.push(measure(|| {
                chat.seed_event(&input);
                for reader in &mut readers {
                    black_box(reader.snapshot());
                }
            }));
        }
        report[workload] = json!({"allocation_calls":distribution(&samples, 0),"allocated_bytes":distribution(&samples, 1),"additional_live_bytes_peak":distribution(&samples, 2)});
    }
    let mut slow = chat.subscribe();
    slow.snapshot();
    let burst = measure(|| {
        for i in 0..10_000 {
            chat.seed_event(&event(count + 100 + i, "burst"));
        }
    });
    let (records, bytes) = chat.subscription_journal_retained();
    assert!(records <= 512 && bytes <= 2 * 1024 * 1024);
    let recovered = slow.snapshot();
    assert!(recovered.reset);
    report["slow_burst"] = json!({"commits":10000,"allocation_calls":burst[0],"allocated_bytes":burst[1],
        "additional_live_bytes_peak":burst[2],"retained_journal_records":records,"estimated_journal_bytes":bytes,
        "note":"live-byte growth includes 10000 new authoritative rows; journal retention is measured separately"});
    report
}
fn main() {
    if std::env::args().any(|arg| arg == "--history") {
        history::run();
        return;
    }
    let runs = [1000, 10000, 100000]
        .into_iter()
        .flat_map(|count| [1, 8].map(|readers| run(count, readers)))
        .collect::<Vec<_>>();
    println!("{}", serde_json::to_string_pretty(&json!({"scope":"core reducer + synchronous native consumption, System allocator calls including realloc; not process RSS, JNI or rendering", "runs":runs})).unwrap());
}
