//! Compact history, group navigation, identity links and full details use the
//! production view. Everything is disconnected and has explicit fixture data.
use gpui::{px, AppContext, HeadlessAppContext};
use serde_json::{json, Value};
use std::{path::PathBuf, sync::Arc, time::Duration};
use zork_gui::{
    assets::EmbeddedAssets,
    automation::{protocol::UserAction, AutomationRoot, HeadlessAutomation},
    views::RootView,
};
use zork_ui::history::Record;

const NOW: i64 = 1_800_000_600_000;
fn record(id: usize, event: Value) -> Record {
    Record {
        event_id: format!("{id:016x}"),
        event,
        metadata: Default::default(),
    }
}
fn fixture() -> Vec<Record> {
    let mut rows = vec![record(
        0,
        json!({"kind":"input_appended", "input":{
        "input_id":"input-0", "request_id":"fixture-user-message", "content":"保留时间线，连续常规操作合并；等待独立显示。", "received_at_ms":NOW-90000}}),
    )];
    for (i, name, args, time, state, data) in [
        (
            1,
            "file.read",
            json!({"path":"crates/zork-client-types/src/history.rs"}),
            NOW - 85000,
            "succeeded",
            json!({"content":"source"}),
        ),
        (
            2,
            "shell.run",
            json!({"command":"rg -n history crates/zork-ui"}),
            NOW - 83000,
            "succeeded",
            json!({"stdout":"matches"}),
        ),
        (
            3,
            "chat.post_message",
            json!({"text":"我会调整条目层级，保留明确的消息来源和目标。","kind":"progress"}),
            NOW - 75000,
            "succeeded",
            json!({"ok":true}),
        ),
        (
            4,
            "wait",
            json!({"seconds":20,"reason":"等待后台任务的新消息"}),
            NOW - 60000,
            "succeeded",
            json!({"until_ms":NOW-39990}),
        ),
        (
            5,
            "shell.run",
            json!({"command":"cargo test --locked -p zork-client-types"}),
            NOW - 25000,
            "failed",
            json!({"error":"fixture failure: history_empty_state"}),
        ),
    ] {
        rows.push(record(i*3, json!({"kind":"step_started","step_id":format!("s{i}"),"purpose":"conversation","started_at_ms":time-1000})));
        rows.push(record(i*3+1, json!({"kind":"step_completed","step_id":format!("s{i}"),"completed_at_ms":time,
            "assistant_text":"INTERNAL-MODEL-TEXT-MUST-NOT-BE-A-MESSAGE", "invocations":[{
                "invocation_id":format!("tool-{i}"),"tool":name,"arguments":args,"started_at_ms":time}]})));
        rows.push(record(i*3+2, json!({"kind":"tool_result","result":{
            "invocation_id":format!("tool-{i}"),"tool":name,"outcome":state,"data":data,"finished_at_ms":time+10}})));
        if name == "wait" {
            rows.push(record(100, json!({"kind":"input_appended","input":{"input_id":"wake", "content":"后台任务已完成。","received_at_ms":NOW-45000}})));
            rows.push(record(101, json!({"kind":"step_started","step_id":"wake","consumed_inputs":["wake"],"started_at_ms":NOW-44990})));
        }
    }
    rows
}
fn run(width: f32, height: f32) -> anyhow::Result<()> {
    let out = std::env::var("ZORK_HISTORY_STATISTICS_OUTPUT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../artifacts/history-statistics")
        })
        .join(format!("native-{}", width as u32));
    std::fs::create_dir_all(&out)?;
    let directory = tempfile::tempdir()?;
    let store = Arc::new(zork_gui::desktop::store::ClientStore::open(
        directory.path(),
    )?);
    let mut cx = HeadlessAppContext::with_platform(
        gpui_platform::current_platform(true).text_system(),
        Arc::new(EmbeddedAssets),
        gpui_platform::current_headless_renderer,
    );
    let driver = cx.update(|cx| {
        zork_gui::assets::init_fonts(cx);
        zork_gui::components::init(cx);
        cx.set_reduce_motion(true);
        HeadlessAutomation::install(cx)
    });
    let mut root_view = None;
    let window = cx.open_window(gpui::size(px(width), px(height)), |_, cx| {
        let root = cx.new(|cx| {
            let mut view = RootView::render_benchmark_fixture(false, store, cx);
            view.benchmark_replace_messages(
                vec![zork_gui::views::TranscriptLine::Message {
                    role: zork_gui::api::Role::User,
                    content: "保留时间线，连续常规操作合并；等待独立显示。".into(),
                    metadata: serde_json::from_value(
                        json!({"id":"fixture-user-message","author_name":"测试用户"}),
                    )
                    .unwrap(),
                }],
                cx,
            );
            view.benchmark_story_history(fixture(), NOW, cx);
            view
        });
        root_view = Some(root.clone());
        cx.new(|_| AutomationRoot::new(root))
    })?;
    let root_view = root_view.unwrap();
    let pump = |cx: &mut HeadlessAppContext| -> anyhow::Result<()> {
        for _ in 0..8 {
            cx.advance_clock(Duration::from_millis(16));
            cx.run_until_parked();
            cx.update_window(window.into(), |_, w, cx| {
                w.simulate_next_frame(cx);
                w.draw(cx).clear(cx)
            })?;
        }
        Ok(())
    };
    let action = |cx: &mut HeadlessAppContext, value: Value| -> anyhow::Result<()> {
        let action: UserAction = serde_json::from_value(value)?;
        cx.update_window(window.into(), |_, w, cx| driver.dispatch(action, w, cx))??;
        pump(cx)
    };
    pump(&mut cx)?;
    let capture = |cx: &mut HeadlessAppContext, name: &str| -> anyhow::Result<()> {
        cx.capture_screenshot(window.into())?.save(out.join(name))?;
        Ok(())
    };
    capture(&mut cx, "unknown.png")?;
    let runtime: zork_client_core::state::HistoryRuntime = serde_json::from_value(json!({
        "model":"gpt-5.4", "thinking":"high", "context_tokens":128432, "context_limit":256000,
        "profile":{"profile_id":"work", "name":"工作账号", "provider":"openai", "checkedAt":"2026-09-10T08:00:00Z",
            "rateLimits":{"rateLimits":{"primary":{"usedPercent":24,"windowDurationMins":300},"secondary":{"usedPercent":62,"windowDurationMins":10080}}}}
    }))?;
    let mut records = fixture();
    records.push(record(
        200,
        json!({"kind":"step_started","step_id":"usage","started_at_ms":NOW-1000}),
    ));
    records.push(record(
        201,
        json!({"kind":"step_completed","step_id":"usage","completed_at_ms":NOW,
        "usage":{"input_tokens":128432,"output_tokens":2400,"cached_input_tokens":96000}}),
    ));
    root_view.update(&mut cx, |v, cx| {
        v.benchmark_story_history(records, NOW, cx);
        v.benchmark_history_runtime(runtime.clone(), cx);
    });
    pump(&mut cx)?;
    let snapshot = driver.snapshot(false);
    let find = |id: &str| {
        snapshot
            .elements
            .iter()
            .find(|e| e.id == id)
            .unwrap_or_else(|| panic!("missing {id}"))
    };
    let stats = find("history-usage-overview");
    if std::env::var("ZORK_GUI_LOCALE").as_deref() == Ok("en") {
        assert!(
            stats.label.starts_with("Tokens:"),
            "fixture ignored English locale: {}",
            stats.label
        );
    }
    let model_element = find("history-model");
    assert!(!snapshot
        .elements
        .iter()
        .any(|e| e.id == "history-activity-toolbar"));
    let ledger = find("history-ledger");
    let ledger_y = ledger.bounds.y + ledger.bounds.height * 0.6;
    assert!(
        stats.label.contains("130832") && stats.label.contains("74.7%"),
        "incorrect totals: {}",
        stats.label
    );
    assert!(model_element.label.contains("gpt-5.4"));
    assert!(
        stats.bounds.y + stats.bounds.height <= ledger.bounds.y + 0.1,
        "the overview must precede the records"
    );
    assert!(
        ledger.bounds.y + ledger.bounds.height <= height + 1.,
        "the records must fill the page under the overview"
    );
    assert!(
        ledger.bounds.height >= height * 0.35,
        "records lost their primary reading space: {}",
        ledger.bounds.height
    );
    // Identity, model and total tokens share one line; the breakdown and the
    // cache rate wait behind 用量.
    let tokens = find("history-tokens").bounds;
    assert!((model_element.bounds.y - tokens.y).abs() < 1.);
    assert!(!snapshot.elements.iter().any(|e| e.id == "history-cache"));
    std::fs::write(
        out.join("layout.json"),
        serde_json::to_vec_pretty(&json!({
            "statistics":stats.bounds,"ledger":ledger.bounds
        }))?,
    )?;
    assert!(stats.bounds.x >= 0. && stats.bounds.x + stats.bounds.width <= width);
    let handle = find("page-resize").bounds;
    let original_width = ledger.bounds.width;
    action(
        &mut cx,
        json!({"type":"drag",
        "from":{"x":handle.x+3.,"y":height*0.45},
        "to":{"x":handle.x-20.,"y":height*0.45},"steps":10}),
    )?;
    let resized = driver.snapshot(false);
    let resized_ledger = resized
        .elements
        .iter()
        .find(|e| e.id == "history-ledger")
        .unwrap()
        .bounds;
    assert!(
        resized_ledger.width >= original_width + 15.,
        "right panel did not widen after dragging its edge"
    );
    capture(&mut cx, "resized.png")?;
    let resized_handle = resized
        .elements
        .iter()
        .find(|e| e.id == "page-resize")
        .unwrap()
        .bounds;
    action(
        &mut cx,
        json!({"type":"drag",
        "from":{"x":resized_handle.x+3.,"y":height*0.45},
        "to":{"x":handle.x+3.,"y":height*0.45},"steps":10}),
    )?;
    let restored = driver.snapshot(false);
    let restored_width = restored
        .elements
        .iter()
        .find(|e| e.id == "history-ledger")
        .unwrap()
        .bounds
        .width;
    assert!(
        (restored_width - original_width).abs() < 2.,
        "right panel did not shrink back: {restored_width} != {original_width}"
    );
    capture(&mut cx, "statistics.png")?;
    action(
        &mut cx,
        json!({"type":"scroll","target":{"x":width-80.,"y":ledger_y},"delta_y":-600}),
    )?;
    capture(&mut cx, "scrolled.png")?;
    let details = root_view.update(&mut cx, |v, cx| v.benchmark_observe_history(cx));
    details.seed_records(
        vec![
            record(
                998,
                json!({"kind":"step_started","step_id":"older-usage","started_at_ms":NOW-200000}),
            ),
            record(
                999,
                json!({"kind":"step_completed","step_id":"older-usage","completed_at_ms":NOW-199000,
            "usage":{"input_tokens":900000,"output_tokens":100000}}),
            ),
        ],
        true,
    );
    pump(&mut cx)?;
    let paged = driver.snapshot(false);
    let loaded = paged
        .elements
        .iter()
        .find(|e| e.id == "history-usage-overview")
        .unwrap();
    assert!(
        loaded.label.contains("1130832"),
        "paging must include the older loaded call in the displayed scope: {}",
        loaded.label
    );
    assert!(
        loaded.label.ends_with(": —"),
        "partial cache reports must not imply a complete rate: {}",
        loaded.label
    );
    let records = (0..600).map(|i| record(i, json!({"kind":"input_appended","input":{
        "input_id":format!("scroll-{i}"),"content":format!("历史记录 {i} · rolling history"),"received_at_ms":NOW-600000+i as i64*1000
    }}))).collect();
    root_view.update(&mut cx, |v, cx| {
        v.benchmark_story_history(records, NOW, cx);
        v.benchmark_history_runtime(runtime.clone(), cx);
        v.benchmark_scroll_history(300, 0., cx);
    });
    pump(&mut cx)?;
    let before = cx.update_window(window.into(), |_, w, _| w.frame_duration_snapshot())?;
    let mut trace = Vec::new();
    for frame in 0..120 {
        root_view.update(&mut cx, |v, _| v.benchmark_begin_frame());
        let scroll: UserAction = serde_json::from_value(
            json!({"type":"scroll","target":{"x":width-80.,"y":ledger_y},"delta_y":if frame < 60 {-24} else {24}}),
        )?;
        cx.update_window(window.into(), |_, w, cx| driver.dispatch(scroll, w, cx))??;
        cx.advance_clock(Duration::from_millis(16));
        cx.run_until_parked();
        trace.push(root_view.read_with(&cx, |v, _| v.benchmark_frame_state(true)));
    }
    let mut after = cx.update_window(window.into(), |_, w, _| w.frame_duration_snapshot())?;
    after
        .draw_duration_histogram
        .subtract(&before.draw_duration_histogram)?;
    let histogram = after.draw_duration_histogram;
    let max_rows = trace.iter().map(|state| state.0).max().unwrap_or(0);
    assert!(
        max_rows > 0 && max_rows <= 50,
        "virtualized rows: {max_rows}"
    );
    assert!(histogram.len() >= 108, "scroll did not continuously draw");
    assert!(
        trace.iter().any(|state| state.1 != trace[0].1),
        "scroll did not move"
    );
    let p95 = histogram.value_at_quantile(0.95) as f64 / 1e6;
    let p99 = histogram.value_at_quantile(0.99) as f64 / 1e6;
    std::fs::write(
        out.join("performance.json"),
        serde_json::to_vec_pretty(
            &json!({"records":600,"frames":histogram.len(),"max_rows":max_rows,"p95_ms":p95,"p99_ms":p99}),
        )?,
    )?;
    assert!(p95 < 8.33, "history draw p95 exceeded budget: {p95}");
    println!(
        "history statistics {width}x{height}: loaded-call totals, model identity, layout, scrolling passed; p95 {p95:.2} ms / p99 {p99:.2} ms"
    );
    Ok(())
}
fn main() -> anyhow::Result<()> {
    run(1280., 800.)?;
    run(900., 600.)
}
