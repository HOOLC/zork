//! Deterministic headless scrolling with real GPUI layout, shaping and paint.
use gpui::{
    AppContext, HeadlessAppContext, Modifiers, PlatformInput, ScrollDelta, ScrollWheelEvent,
    TouchPhase, point, px, size,
};
use std::{path::PathBuf, sync::Arc, time::Duration};
use zork_gui::{
    assets::EmbeddedAssets,
    automation::{AutomationRoot, HeadlessAutomation, protocol::UserAction},
    views::RootView,
};

#[cfg(feature = "frame-profiler")]
#[path = "render-bench/native.rs"]
mod native;

#[cfg(feature = "native-blur-bench")]
#[path = "render-bench/native_frames.rs"]
mod native_frames;

#[path = "render-bench/files.rs"]
mod files;

#[path = "render-bench/panel.rs"]
mod panel;

const FPS: u32 = 120;
const FRAMES: usize = 240;

fn workload(history: bool, output: &std::path::Path) -> anyhow::Result<serde_json::Value> {
    std::fs::create_dir_all(output)?;
    // The macOS brand view refreshes system motion preferences while rendering.
    // Use the same explicit fixture override as the native scrolling benchmark.
    std::env::set_var("ZORK_GUI_TEST_REDUCE_MOTION", "1");
    let all_types = !history && std::env::var_os("ZORK_SCROLL_ALL_MESSAGES").is_some();
    let anchor = std::env::var("ZORK_BENCH_ANCHOR")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(100);
    let setup_started = std::time::Instant::now();
    let stress = !history && std::env::var_os("ZORK_SCROLL_MARKDOWN_STRESS").is_some();
    let name = if all_types {
        format!("mixed-{anchor}")
    } else if history {
        "history".into()
    } else if stress {
        "markdown".into()
    } else {
        "chat".into()
    };
    let fixture = tempfile::tempdir()?;
    let store = Arc::new(zork_gui::desktop::store::ClientStore::open(fixture.path())?);
    let mut cx = HeadlessAppContext::with_platform(
        gpui_platform::current_platform(true).text_system(),
        Arc::new(EmbeddedAssets),
        gpui_platform::current_headless_renderer,
    );
    let driver = cx.update(|cx| {
        zork_gui::assets::init_fonts(cx);
        zork_gui::components::init(cx);
        // Measure the opened list, not the browser panel's entrance animation.
        // Both baseline and changed-tree replays use this same settled layout.
        cx.set_reduce_motion(true);
        HeadlessAutomation::install(cx)
    });
    let mut root = None;
    let handle = cx.open_window(size(px(1280.), px(800.)), |_, cx| {
        let view = cx.new(|cx| RootView::render_benchmark_fixture(history, store.clone(), cx));
        root = Some(view.clone());
        cx.new(|_| AutomationRoot::new(view))
    })?;
    let root = root.unwrap();
    // Root initialization applies platform preferences; this fixed-input
    // scrolling fixture measures the settled layout, as configured above.
    cx.update(|cx| cx.set_reduce_motion(true));
    cx.update_window(handle.into(), |_, window, cx| window.draw(cx).clear(cx))?;
    cx.run_until_parked();
    let cold = cx.update_window(handle.into(), |_, window, _| {
        window.frame_duration_snapshot()
    })?;

    let fixture_setup_ms = setup_started.elapsed().as_secs_f64() * 1000.;
    let records = root.read_with(&cx, |view, _| view.benchmark_record_count(history));
    let mut type_cold_draws = Vec::new();
    let mut file_list = serde_json::Value::Null;
    if all_types {
        // Exercise every kind in both delivered roles, including rarely hit
        // cases, before the start/middle/end continuous-scroll replays.
        for index in 0..(RootView::benchmark_kind_count() * 2).min(records) {
            let start = std::time::Instant::now();
            root.update(&mut cx, |view, cx| view.benchmark_jump_to(index, cx));
            cx.update_window(handle.into(), |_, w, cx| w.draw(cx).clear(cx))?;
            cx.run_until_parked();
            type_cold_draws.push(start.elapsed().as_secs_f64() * 1000.);
            if std::env::var_os("ZORK_BENCH_CAPTURE_KINDS").is_some()
                && anchor == 0
                && output.ends_with("replay-0")
            {
                cx.capture_screenshot(handle.into())?
                    .save(output.join(format!("kind-{index}.png")))?;
            }
        }
        file_list = files::verify(
            &mut cx,
            handle.into(),
            &root,
            &driver,
            output,
            anchor == 0 && output.ends_with("replay-0"),
        )?;
        for kind in [
            "png", "jpeg", "svg", "text", "markdown", "binary", "loading", "failed",
        ] {
            root.update(&mut cx, |view, cx| {
                view.benchmark_preview_artifact(kind, cx)
            });
            for _ in 0..3 {
                cx.update_window(handle.into(), |_, w, cx| w.draw(cx).clear(cx))?;
                cx.run_until_parked();
            }
            anyhow::ensure!(
                driver
                    .snapshot(false)
                    .elements
                    .iter()
                    .any(|e| e.id == "drive-close-preview"),
                "missing {kind} preview"
            );
            if anchor == 0 && output.ends_with("replay-0") {
                cx.capture_screenshot(handle.into())?
                    .save(output.join(format!("preview-{kind}.png")))?;
            }
            let action: UserAction = serde_json::from_value(
                serde_json::json!({"type":"click","target":{"element_id":"drive-close-preview"}}),
            )?;
            cx.update_window(handle.into(), |_, w, cx| driver.dispatch(action, w, cx))??;
            cx.update_window(handle.into(), |_, w, cx| w.draw(cx).clear(cx))?;
            cx.run_until_parked();
        }
        for (offset, state) in ["pending", "sending", "failed"].iter().enumerate() {
            root.update(&mut cx, |view, cx| {
                view.benchmark_jump_to(records - 3 + offset, cx);
                if *state == "failed" {
                    // Literal user text can make the final row taller than the viewport.
                    // Its retry control is at the bottom of that row.
                    view.benchmark_scroll_to_end(cx);
                }
            });
            for _ in 0..3 {
                cx.update_window(handle.into(), |_, w, cx| w.draw(cx).clear(cx))?;
                cx.run_until_parked();
            }
            if anchor == 0 && output.ends_with("replay-0") {
                cx.capture_screenshot(handle.into())?
                    .save(output.join(format!("delivery-{state}.png")))?;
            }
        }
        std::fs::write(
            output.join("delivery-elements.json"),
            serde_json::to_vec_pretty(&driver.snapshot(true))?,
        )?;
        let device = root.read_with(&cx, |view, _| view.benchmark_core_device());
        let before_resend = device.outbox();
        let original = before_resend
            .items
            .iter()
            .find(|message| message.request_id == "stress-failed")
            .unwrap();
        let action: UserAction = serde_json::from_value(
            serde_json::json!({"type":"click","target":{"element_id":"retry-queued-stress-failed"}}),
        )?;
        cx.update_window(handle.into(), |_, w, cx| driver.dispatch(action, w, cx))??;
        cx.run_until_parked();
        cx.update_window(handle.into(), |_, w, cx| w.draw(cx).clear(cx))?;
        cx.run_until_parked();
        let after_resend = device.outbox();
        let resent = after_resend
            .items
            .iter()
            .filter(|message| {
                !before_resend
                    .items
                    .iter()
                    .any(|old| old.request_id == message.request_id)
            })
            .collect::<Vec<_>>();
        anyhow::ensure!(
            resent.len() == 1,
            "resend must create exactly one new message"
        );
        anyhow::ensure!(
            resent[0].content == original.content
                && resent[0].error.is_none()
                && !resent[0].attempted,
            "resend did not preserve content in a fresh delivery"
        );
        anyhow::ensure!(
            after_resend.items.iter().any(|message| message == original),
            "resend changed the original failed message"
        );
        // Restore the fixed three-state fixture before measuring or replaying.
        // The successful resend is a fourth local row with a fresh identity.
        store.fail_delivery("mini1", &resent[0].request_id, "fixture cleanup")?;
        device.delete_failed_delivery(&resent[0].request_id)?;
        root.update(&mut cx, |view, cx| {
            view.benchmark_restore_delivery_failure(cx)
        });
        for state in 0..9 {
            let visible = root.update(&mut cx, |view, cx| view.benchmark_activity(state, cx));
            cx.update_window(handle.into(), |_, w, cx| w.draw(cx).clear(cx))?;
            cx.run_until_parked();
            anyhow::ensure!(
                visible == !matches!(state, 0 | 7 | 8),
                "activity state {state} visibility regression"
            );
            if anchor == 0 && output.ends_with("replay-0") {
                cx.capture_screenshot(handle.into())?
                    .save(output.join(format!("activity-{state}.png")))?;
            }
        }
        root.update(&mut cx, |view, cx| {
            view.benchmark_activity(0, cx);
        });
        root.update(&mut cx, |view, cx| view.benchmark_jump_to(anchor, cx));
        cx.update_window(handle.into(), |_, w, cx| w.draw(cx).clear(cx))?;
        cx.run_until_parked();
    }

    let step = |cx: &mut HeadlessAppContext, frame: usize| -> anyhow::Result<_> {
        root.update(cx, |view, _| view.benchmark_begin_frame());
        cx.advance_clock(Duration::from_nanos(1_000_000_000 / FPS as u64));
        cx.update_window(handle.into(), |_, window, cx| {
            let direction = if frame % 240 < 120 { 1. } else { -1. }
                * if all_types && anchor == 0 { -1. } else { 1. };
            window.dispatch_event(
                PlatformInput::ScrollWheel(ScrollWheelEvent {
                    position: point(px(if history { 1100. } else { 700. }), px(550.)),
                    delta: ScrollDelta::Pixels(point(
                        px(0.),
                        px(direction * if stress || all_types { 6000. } else { 2000. }
                            / FPS as f32),
                    )),
                    modifiers: Modifiers::default(),
                    touch_phase: TouchPhase::Moved,
                }),
                cx,
            );
        })?;
        cx.run_until_parked();
        Ok(root.read_with(cx, |view, _| view.benchmark_frame_state(history)))
    };
    anyhow::ensure!(
        root.read_with(&cx, |view, _| view.benchmark_record_count(history)) == records,
        "interaction preflight changed message count"
    );
    // Warm font, SVG and row caches with the same input sequence before timing.
    for frame in 0..FRAMES {
        step(&mut cx, frame)?;
    }
    let before = cx.update_window(handle.into(), |_, w, _| w.frame_duration_snapshot())?;
    let mut trace = Vec::new();
    for frame in 0..FRAMES {
        trace.push(step(&mut cx, frame)?);
    }
    let mut after = cx.update_window(handle.into(), |_, w, _| w.frame_duration_snapshot())?;
    after
        .draw_duration_histogram
        .subtract(&before.draw_duration_histogram)?;
    let draws = &after.draw_duration_histogram;
    let max_rows = trace.iter().map(|row| row.0).max().unwrap_or(0);
    let max_artifacts = trace.iter().map(|row| row.3).max().unwrap_or(0);
    if max_rows == 0 {
        cx.capture_screenshot(handle.into())?
            .save(output.join(format!("{name}-failed.png")))?;
        let regions = root.read_with(&cx, |view, cx| view.benchmark_region_counts(cx));
        std::fs::write(
            output.join(format!("{name}-failed.json")),
            serde_json::to_vec_pretty(
                &serde_json::json!({"trace": trace, "regions": regions, "elements": driver.snapshot(true)}),
            )?,
        )?;
    }
    if all_types {
        anyhow::ensure!(
            max_artifacts == 0,
            "collapsed historical files were rendered during message scrolling: {max_artifacts}"
        );
    }
    anyhow::ensure!(
        max_rows > 0 && max_rows <= 50,
        "{name}: virtual row bound failed: {max_rows}"
    );
    anyhow::ensure!(
        draws.len() >= FRAMES as u64 * 9 / 10,
        "{name}: no continuous drawing"
    );
    anyhow::ensure!(
        trace.windows(2).any(|p| p[0].1 != p[1].1),
        "{name}: scrolling did not move"
    );
    cx.capture_screenshot(handle.into())?
        .save(output.join(format!("{name}.png")))?;
    let coverage = root.read_with(&cx, |view, _| view.benchmark_message_coverage());
    if all_types {
        anyhow::ensure!(
            coverage["both_roles_measured"] == true,
            "not every message kind and role was exercised"
        );
        anyhow::ensure!(
            coverage["parsed_documents"].as_u64().unwrap() <= 2000,
            "message parsing escaped the visible window"
        );
    }
    let (layout_entries, layout_bytes, layout_accesses, layout_misses) =
        zork_ui::components::message::text_layout_cache_stats();
    anyhow::ensure!(
        layout_entries <= 1024 && layout_bytes <= 32 * 1024 * 1024,
        "text layout cache exceeded its bound"
    );
    let result = serde_json::json!({
        "layout_cache_accesses":layout_accesses,"layout_cache_misses":layout_misses,"layout_cache_entries":layout_entries,"layout_cache_estimated_bytes":layout_bytes,
        "workload":name,"all_message_types":all_types,"records":records,"coverage":coverage,"fixture_setup_ms":fixture_setup_ms,"type_cold_draw_ms":type_cold_draws,"peak_rss_mib":peak_rss_mib(),"input_frames":FRAMES,"virtual_fps":FPS,
        "seed":0,"viewport":[1280,800],"draws":draws.len(),"max_rendered_rows":max_rows,"max_rendered_artifacts":max_artifacts,"file_list":file_list,
        "cold_draw_ms":cold.draw_duration_histogram.value_at_quantile(1.0) as f64 / 1e6,
        "p95_draw_ms":draws.value_at_quantile(0.95) as f64 / 1e6,
        "p99_draw_ms":draws.value_at_quantile(0.99) as f64 / 1e6,
        "frame_budget_ms":1000. / FPS as f64,
        "measurement":"CPU layout and paint; fixed virtual clock/input; GPU and display FPS excluded",
        "trace":trace,
    });
    std::fs::write(
        output.join(format!("{name}.json")),
        serde_json::to_vec_pretty(&result)?,
    )?;
    Ok(result)
}

fn main() -> anyhow::Result<()> {
    #[cfg(feature = "native-blur-bench")]
    if std::env::args().nth(1).as_deref().is_some_and(|arg| matches!(arg, "--native-frame-calibration" | "--native-offscreen-calibration")) {
        return native_frames::calibrate(&PathBuf::from(std::env::args().nth(2).ok_or_else(|| anyhow::anyhow!("missing calibration output"))?), std::env::args().nth(1).as_deref() == Some("--native-offscreen-calibration"));
    }
    if std::env::args().nth(1).as_deref() == Some("--native-frames") {
        #[cfg(feature = "native-blur-bench")]
        return native_frames::run(
            &PathBuf::from(std::env::args().nth(2).ok_or_else(|| anyhow::anyhow!("missing configuration"))?),
            &PathBuf::from(std::env::args().nth(3).ok_or_else(|| anyhow::anyhow!("missing output directory"))?),
        );
        #[cfg(not(feature = "native-blur-bench"))]
        anyhow::bail!("--native-frames requires native-blur-bench");
    }
    if std::env::args().nth(1).as_deref() == Some("--native") {
        #[cfg(feature = "frame-profiler")]
        return native::run(
            &std::env::args().nth(2).unwrap_or_else(|| "chat".into()),
            &PathBuf::from(
                std::env::args()
                    .nth(3)
                    .unwrap_or_else(|| "/tmp/zork-native-render".into()),
            ),
        );
        #[cfg(not(feature = "frame-profiler"))]
        anyhow::bail!("--native requires the frame-profiler feature");
    }
    let output = PathBuf::from(
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| "/tmp/zork-headless-render".into()),
    );
    std::fs::create_dir_all(&output)?;
    std::env::set_var("SEED", "0");
    std::env::set_var("TZ", "UTC");
    std::env::set_var(
        "ZORK_GUI_PREFERENCES_PATH",
        output.join("fixture-preferences.json"),
    );
    std::fs::write(output.join("fixture-preferences.json"), "{}")?;
    if std::env::var_os("ZORK_BENCH_PANEL_MOTION").is_some() {
        return panel::run(&output);
    }
    let all_types = std::env::var_os("ZORK_SCROLL_ALL_MESSAGES").is_some();
    let count = std::env::var("ZORK_BENCH_MESSAGE_COUNT")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(100_000)
        .clamp(1, 1_000_000);
    let cases = if all_types {
        vec![
            (false, 0),
            (false, count / 2),
            (false, count.saturating_sub(40)),
        ]
    } else {
        vec![(true, 100), (false, 100)]
    };
    for (history, anchor) in cases {
        if all_types {
            std::env::set_var("ZORK_BENCH_ANCHOR", anchor.to_string());
        }
        let first = workload(history, &output.join("replay-0"))?;
        let mut result = workload(history, &output.join("replay-1"))?;
        anyhow::ensure!(
            first["trace"] == result["trace"],
            "fixed-input replay diverged"
        );
        let worst_p95 = first["p95_draw_ms"]
            .as_f64()
            .unwrap()
            .max(result["p95_draw_ms"].as_f64().unwrap());
        result["replay_verified"] = true.into();
        result["replay_runs"] = 2.into();
        result["p95_draw_ms"] = worst_p95.into();
        for metric in ["p99_draw_ms", "cold_draw_ms"] {
            result[metric] = first[metric]
                .as_f64()
                .unwrap()
                .max(result[metric].as_f64().unwrap())
                .into();
        }
        let name = result["workload"].as_str().unwrap();
        std::fs::write(
            output.join(format!("{name}.json")),
            serde_json::to_vec_pretty(&result)?,
        )?;
        std::fs::copy(
            output.join(format!("replay-1/{name}.png")),
            output.join(format!("{name}.png")),
        )?;
        println!(
            "{}: p95 CPU draw {:.2} ms, {} virtual frames, max {} rows",
            result["workload"].as_str().unwrap(),
            result["p95_draw_ms"].as_f64().unwrap(),
            FRAMES,
            result["max_rendered_rows"]
        );
        anyhow::ensure!(
            worst_p95 <= 1000. / FPS as f64,
            "CPU frame budget exceeded; see {}",
            output.display()
        );
    }
    Ok(())
}

fn peak_rss_mib() -> Option<f64> {
    #[cfg(unix)]
    {
        let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
        if unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) } != 0 {
            return None;
        }
        let usage = unsafe { usage.assume_init() };
        let bytes = usage.ru_maxrss as f64 * if cfg!(target_os = "macos") { 1. } else { 1024. };
        Some(bytes / (1024. * 1024.))
    }
    #[cfg(not(unix))]
    {
        None
    }
}
