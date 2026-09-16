//! CPU-only scene benchmark. Platform JNI, Canvas and GPU costs are separate.
use std::{
    alloc::{GlobalAlloc, Layout, System},
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::Instant,
};
use zork_liquid::scene::Scene;

struct Allocations;
static COUNTING: AtomicBool = AtomicBool::new(false);
static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);
unsafe impl GlobalAlloc for Allocations {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if COUNTING.load(Ordering::Relaxed) {
            ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        if COUNTING.load(Ordering::Relaxed) {
            ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.realloc(ptr, layout, size) }
    }
}
#[global_allocator]
static ALLOCATOR: Allocations = Allocations;

fn command(out: &mut Vec<u8>, id: u32, kind: u32, active: bool, phase: u32) {
    let mut words = [0_u32; 24];
    words[0] = id;
    words[1] = kind;
    words[2] = 1 | (u32::from(active) << 1);
    for (i, value) in [
        (4, 0.),
        (5, 0.),
        (6, 160.),
        (7, 48.),
        (8, 24.),
        (9, 0.),
        (10, 64.),
        (11, 320.),
        (12, 240.),
        (13, 32.),
        (14, if phase % 2 == 0 { 0.15 } else { 0.85 }),
        (15, 0.5),
        (18, 0.6),
        (19, 0.5),
    ] {
        words[i] = f32::to_bits(value);
    }
    words[16] = 3;
    words[17] = phase % 3;
    for word in words {
        out.extend_from_slice(&word.to_ne_bytes());
    }
}
fn header(bytes: &[u8], i: usize) -> u32 {
    u32::from_ne_bytes(bytes[i * 4..i * 4 + 4].try_into().unwrap())
}
fn percentile(samples: &mut [f64], quantile: f64) -> f64 {
    samples.sort_by(f64::total_cmp);
    samples[((samples.len() - 1) as f64 * quantile).round() as usize]
}
fn main() {
    let path = std::env::args().nth(1).expect("output JSON path");
    let mut report = Vec::new();
    for hz in [60, 120] {
        let mut scene = Scene::default();
        let mut input = Vec::with_capacity(16 * 96);
        let mut samples = Vec::with_capacity(1800);
        let mut allocations = Vec::with_capacity(1800);
        let mut bytes = 0_u64;
        for frame in 0..1800 {
            input.clear();
            if frame % 18 == 0 {
                for id in 1..=16 {
                    let kind = match id % 4 {
                        0 => 3,
                        1 => 1,
                        2 => 5,
                        _ => 8,
                    };
                    command(&mut input, id, kind, (frame / 18) % 2 == 0, frame / 18);
                }
            }
            ALLOCATIONS.store(0, Ordering::Relaxed);
            COUNTING.store(true, Ordering::Relaxed);
            let start = Instant::now();
            let output = scene.frame(&input, 1. / hz as f64, false).unwrap();
            let elapsed = start.elapsed().as_secs_f64() * 1000.;
            COUNTING.store(false, Ordering::Relaxed);
            assert_eq!(header(output, 3), 0, "frame errors at {frame}");
            if frame >= 120 {
                samples.push(elapsed);
                allocations.push(ALLOCATIONS.load(Ordering::Relaxed) as f64);
                bytes += output.len() as u64;
            }
        }
        for _ in 0..720 {
            scene.frame(&[], 1. / 120., false).unwrap();
        }
        ALLOCATIONS.store(0, Ordering::Relaxed);
        COUNTING.store(true, Ordering::Relaxed);
        for _ in 0..1000 {
            let output = scene.frame(&[], 1. / 120., false).unwrap();
            assert_eq!(output.len(), 16);
            assert_eq!(header(output, 2), 0);
        }
        COUNTING.store(false, Ordering::Relaxed);
        let idle_allocations = ALLOCATIONS.load(Ordering::Relaxed);
        assert_eq!(idle_allocations, 0, "settled scenes must not allocate");
        report.push(serde_json::json!({"cadence_hz": hz, "visible_controls":16,
            "frames": samples.len(), "cpu_p50_ms": percentile(&mut samples, 0.5),
            "cpu_p95_ms":percentile(&mut samples, 0.95), "cpu_p99_ms":percentile(&mut samples, 0.99),
            "allocations_p50": percentile(&mut allocations, 0.5), "allocations_p95":percentile(&mut allocations, 0.95),
            "bytes_per_frame":bytes / samples.len() as u64, "idle_frames":1000, "idle_allocations":idle_allocations}));
    }
    let result = serde_json::json!({"backend":"Rust CPU physics/contour/border/encoding; excludes JNI, Android Path and GPU", "scenarios":report});
    std::fs::write(path, serde_json::to_string_pretty(&result).unwrap() + "\n").unwrap();
    println!("{}", serde_json::to_string_pretty(&result).unwrap());
}
