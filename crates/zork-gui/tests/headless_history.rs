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
    // Brand reads the platform preference on render. Keep these endpoint
    // assertions reduced; the hover phases below explicitly enable motion.
    std::env::set_var("ZORK_GUI_TEST_REDUCE_MOTION", "1");
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!(
        "../../artifacts/history-ux/validation/native-{}",
        width as u32
    ));
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
            })?;
        }
        Ok(())
    };
    let action = |cx: &mut HeadlessAppContext, value: Value| -> anyhow::Result<()> {
        eprintln!("history {width}: action {value}");
        let action: UserAction = serde_json::from_value(value)?;
        cx.update_window(window.into(), |_, w, cx| driver.dispatch(action, w, cx))??;
        pump(cx)
    };
    pump(&mut cx)?;
    cx.capture_screenshot(window.into())?
        .save(out.join("initial.png"))?;
    std::fs::write(
        out.join("initial.json"),
        serde_json::to_vec_pretty(&driver.snapshot(true))?,
    )?;
    if std::env::var_os("ZORK_HISTORY_HOVER_CHECK").is_some() {
        root_view.update(&mut cx, |v, cx| {
            v.benchmark_story_history(fixture(), NOW, cx)
        });
        pump(&mut cx)?;
        std::env::set_var("ZORK_GUI_TEST_REDUCE_MOTION", "0");
        cx.update(|cx| cx.set_reduce_motion(false));
        let output =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../artifacts/history-hover");
        std::fs::create_dir_all(&output)?;
        let snapshot = driver.snapshot(false);
        std::fs::write(
            output.join(format!("{width}-initial.json")),
            serde_json::to_vec_pretty(&snapshot)?,
        )?;
        cx.capture_screenshot(window.into())?
            .save(output.join(format!("{width}-initial.png")))?;
        let mut bars: Vec<_> = snapshot
            .elements
            .iter()
            .filter(|e| e.id.starts_with("history-bar-") && e.bounds.width > 0.)
            .collect();
        bars.sort_by(|a, b| a.bounds.x.total_cmp(&b.bounds.x));
        let first = bars.first().expect("timeline nodes").bounds.clone();
        let last = bars.last().expect("timeline nodes").bounds.clone();
        let move_to = |cx: &mut HeadlessAppContext, x, y| -> anyhow::Result<()> {
            cx.update_window(window.into(), |_, w, cx| {
                driver.dispatch(
                    serde_json::from_value(json!({"type":"move","target":{"x":x,"y":y}}))?,
                    w,
                    cx,
                )
            })??;
            Ok(())
        };
        let tick = |cx: &mut HeadlessAppContext| -> anyhow::Result<f64> {
            std::thread::sleep(Duration::from_millis(16));
            cx.advance_clock(Duration::from_millis(16));
            cx.run_until_parked();
            let start = std::time::Instant::now();
            cx.update_window(window.into(), |_, w, cx| {
                w.simulate_next_frame(cx);
            })?;
            Ok(start.elapsed().as_secs_f64() * 1000.)
        };
        let pose = || {
            driver
                .snapshot(false)
                .elements
                .into_iter()
                .find(|e| e.id == "history-span-detail")
                .expect("hover card")
                .bounds
        };
        move_to(
            &mut cx,
            first.x + first.width / 2.,
            first.y + first.height / 2.,
        )?;
        tick(&mut cx)?;
        let initial = pose();
        let mut baseline = Vec::new();
        for _ in 0..20 {
            baseline.push(tick(&mut cx)?);
        }
        let card_width = initial.width + 2.;
        let destination_x =
            (last.x + last.width / 2. - card_width / 2.).clamp(12., width - card_width - 12.) + 1.;
        move_to(&mut cx, last.x + last.width / 2., last.y + last.height / 2.)?;
        tick(&mut cx)?;
        let switched = pose();
        anyhow::ensure!(
            (switched.x - initial.x).abs() < (destination_x - initial.x).abs() - 0.5,
            "new target jumped from the painted position: {} -> {}",
            initial.x,
            switched.x
        );
        let mut frames = Vec::new();
        let mut moving = Vec::new();
        for _ in 0..6 {
            moving.push(tick(&mut cx)?);
            frames.push(pose());
        }
        let midway = pose();
        anyhow::ensure!((midway.x - initial.x).abs() > 1., "popover did not slide");
        cx.capture_screenshot(window.into())?
            .save(output.join(format!("{width}-moving.png")))?;
        move_to(&mut cx, 10., 10.)?;
        tick(&mut cx)?;
        anyhow::ensure!(
            driver
                .snapshot(false)
                .elements
                .iter()
                .any(|e| e.id == "history-span-detail"),
            "short gap closed the shared surface"
        );
        let midway = pose();
        move_to(
            &mut cx,
            first.x + first.width / 2.,
            first.y + first.height / 2.,
        )?;
        tick(&mut cx)?;
        anyhow::ensure!(
            (pose().x - midway.x).abs() < (midway.x - initial.x).abs() - 0.5,
            "reversal jumped"
        );
        for _ in 0..40 {
            moving.push(tick(&mut cx)?);
        }
        anyhow::ensure!((pose().x - initial.x).abs() < 1., "reversal did not settle");
        cx.capture_screenshot(window.into())?
            .save(output.join(format!("{width}-settled.png")))?;
        std::env::set_var("ZORK_GUI_TEST_REDUCE_MOTION", "1");
        move_to(&mut cx, last.x + last.width / 2., last.y + last.height / 2.)?;
        tick(&mut cx)?;
        let reduced = pose();
        anyhow::ensure!(
            (reduced.x - initial.x).abs() > 1.,
            "reduced-motion did not snap"
        );
        move_to(&mut cx, 10., 10.)?;
        for _ in 0..10 {
            tick(&mut cx)?;
        }
        anyhow::ensure!(
            !driver
                .snapshot(false)
                .elements
                .iter()
                .any(|e| e.id == "history-span-detail"),
            "hover did not dismiss"
        );
        let pending = cx.update_window(window.into(), |_, w, cx| w.simulate_next_frame(cx))?;
        anyhow::ensure!(pending == 0, "idle hover still requests frames");
        baseline.sort_by(f64::total_cmp);
        moving.sort_by(f64::total_cmp);
        let percentile = |v: &[f64], p: f64| v[((v.len() - 1) as f64 * p).round() as usize];
        anyhow::ensure!(
            percentile(&moving, 0.95) < 8.33,
            "hover exceeds CPU frame budget"
        );
        std::fs::write(
            output.join(format!("{width}.json")),
            serde_json::to_vec_pretty(&json!({
                "initial":initial,"switched":switched,"frames":frames,"reduced":reduced,"idle_callbacks":pending,
                "settled_p95_ms":percentile(&baseline,0.95),"moving_p95_ms":percentile(&moving,0.95),"moving_p99_ms":percentile(&moving,0.99)
            }))?,
        )?;
        std::env::set_var("ZORK_GUI_TEST_REDUCE_MOTION", "0");
        println!("history hover {width}: slide, reverse, reduced motion, dismissal, idle and CPU budget passed");
        return Ok(());
    }
    let snapshot = driver.snapshot(false);
    let bounds = |id: &str| {
        snapshot
            .elements
            .iter()
            .find(|e| e.id == id)
            .unwrap_or_else(|| panic!("missing control {id}"))
            .bounds
            .clone()
    };
    for id in ["browser-expand", "conversation-browser"] {
        let control = bounds(id);
        anyhow::ensure!(
            (control.y + control.height / 2. - 22.).abs() < 1.,
            "{id} is not aligned with the browser header"
        );
        anyhow::ensure!(
            control.y >= 0. && control.y + control.height <= 44.,
            "{id} exceeds the browser header"
        );
    }
    let older = bounds("history-older");
    let activity = bounds("history-page");
    anyhow::ensure!(
        (older.width - activity.width).abs() < 1.,
        "paging status is not centered across the list"
    );
    if std::env::var_os("ZORK_SCROLL_ALL_MESSAGES").is_some() {
        let files = bounds("conversation-files-button");
        anyhow::ensure!(
            (files.y + files.height / 2. - 22.).abs() < 1.,
            "attachment button shifted the header"
        );
        anyhow::ensure!(
            files.x + files.width < activity.x,
            "attachment button overlaps the history panel"
        );
    }
    anyhow::ensure!(
        snapshot
            .elements
            .iter()
            .any(|e| e.id.starts_with("history-target-") && e.enabled && e.label == "测试用户"),
        "received message did not resolve its durable source receipt"
    );
    let page = snapshot
        .elements
        .iter()
        .find(|e| e.id == "history-page")
        .expect("history page visible");
    anyhow::ensure!(
        page.bounds.x >= 0. && page.bounds.x + page.bounds.width <= width + 1.,
        "history page outside viewport"
    );
    let group = snapshot
        .elements
        .iter()
        .find(|e| {
            e.id.starts_with("history-record-")
                && e.label.contains("读取 1 个文件")
                && e.label.contains("命令 1 次")
        })
        .expect("routine summary visible");
    let group_id = group.id.clone();
    anyhow::ensure!(
        snapshot
            .elements
            .iter()
            .filter(|e| e.id.starts_with("history-record-"))
            .all(|e| e.bounds.height <= 46.),
        "history item exceeded two lines"
    );
    anyhow::ensure!(!snapshot.elements.iter().any(|e|e.id.starts_with("history-record-")&&e.label.contains("INTERNAL-MODEL-TEXT")),"internal model text leaked");
    anyhow::ensure!(
        snapshot
            .elements
            .iter()
            .any(|e| e.id.starts_with("history-record-") && e.label.contains("等待 15s")),
        "wait duration did not end on mailbox wake"
    );
    if !snapshot
        .elements
        .iter()
        .any(|e| e.id.starts_with("history-record-") && e.label.contains("history_empty_state"))
    {
        action(
            &mut cx,
            json!({"type":"scroll","target":{"element_id":"history-ledger"},"delta_y":-400}),
        )?;
        anyhow::ensure!(
            driver
                .snapshot(false)
                .elements
                .iter()
                .any(|e| e.id.starts_with("history-record-")
                    && e.label.contains("history_empty_state")),
            "failed command was folded or unreachable"
        );
        cx.capture_screenshot(window.into())?
            .save(out.join("scrolled-failure.png"))?;
        action(
            &mut cx,
            json!({"type":"scroll","target":{"element_id":"history-ledger"},"delta_y":400}),
        )?;
    }
    cx.capture_screenshot(window.into())?
        .save(out.join("collapsed.png"))?;
    action(
        &mut cx,
        json!({"type":"click","target":{"element_id":group_id}}),
    )?;
    anyhow::ensure!(
        driver
            .snapshot(false)
            .elements
            .iter()
            .any(|e| e.id.starts_with("history-record-")
                && e.label.starts_with("执行命令")
                && e.label.contains("rg -n history")),
        "group did not expand"
    );
    action(
        &mut cx,
        json!({"type":"click","target":{"element_id":group_id}}),
    )?;
    // A raw tool span inside the collapsed block must expand and reveal that
    // exact invocation, not the hidden model row at the old list index.
    let timeline = driver
        .snapshot(false)
        .elements
        .into_iter()
        .find(|e| {
            e.id.starts_with("history-bar-")
                && e.label.contains("shell.run")
                && e.label.contains("rg -n history")
        })
        .expect("shell timeline marker");
    action(
        &mut cx,
        json!({"type":"click","target":{"element_id":timeline.id}}),
    )?;
    let shell = driver
        .snapshot(false)
        .elements
        .into_iter()
        .find(|e| {
            e.id.starts_with("history-record-")
                && e.label.starts_with("执行命令")
                && e.label.contains("rg -n history")
        })
        .expect("timeline revealed grouped tool");
    cx.capture_screenshot(window.into())?
        .save(out.join("expanded.png"))?;
    action(
        &mut cx,
        json!({"type":"click","target":{"element_id":shell.id}}),
    )?;
    anyhow::ensure!(
        driver
            .snapshot(false)
            .elements
            .iter()
            .any(|e| e.id == "history-detail-dialog"),
        "details did not open"
    );
    cx.capture_screenshot(window.into())?
        .save(out.join("details.png"))?;
    action(&mut cx, json!({"type":"key","keystroke":"escape"}))?;
    let modal_state = cx.update_window(window.into(), |_, w, cx| root_view.read(cx).benchmark_history_modal(w, cx))?;
    if driver.snapshot(false).elements.iter().any(|e| e.id == "history-detail-dialog") {
        cx.capture_screenshot(window.into())?.save(out.join("failed-detail-escape.png"))?;
        std::fs::write(out.join("failed-detail-escape.json"), serde_json::to_vec_pretty(&modal_state)?)?;
    }
    anyhow::ensure!(
        !driver
            .snapshot(false)
            .elements
            .iter()
            .any(|e| e.id == "history-detail-dialog"),
        "Escape did not close details: {modal_state}"
    );
    // Collapse first so the target stays in the compact viewport.
    action(
        &mut cx,
        json!({"type":"click","target":{"element_id":group_id}}),
    )?;
    let target = driver
        .snapshot(false)
        .elements
        .into_iter()
        .find(|e| e.id.starts_with("history-target-") && e.enabled && e.label == "当前会话")
        .expect("resolved conversation target is clickable");
    action(
        &mut cx,
        json!({"type":"click","target":{"element_id":target.id}}),
    )?;
    anyhow::ensure!(
        !driver
            .snapshot(false)
            .elements
            .iter()
            .any(|e| e.id == "history-detail-dialog"),
        "target click bubbled into row details"
    );
    anyhow::ensure!(
        !driver
            .snapshot(false)
            .elements
            .iter()
            .any(|e| e.id == "history-page"),
        "target did not navigate to the conversation"
    );
    // Loading older rows from the paging control must retain the first real
    // record, including its offset below that control. Repeat after a partial scroll.
    let page_records = |start: usize| {
        (start..60).map(|i| record(200+i, json!({
        "kind":"input_appended","input":{"input_id":format!("page-{i}"),
        "content":format!("分页锚点 {i:03}"),"received_at_ms":NOW-200000+i as i64*1000}
    }))).collect::<Vec<_>>()
    };
    root_view.update(&mut cx, |v, cx| {
        v.benchmark_story_history(page_records(30), NOW, cx)
    });
    pump(&mut cx)?;
    let row_y = |text: &str| -> anyhow::Result<f32> {
        driver
            .snapshot(false)
            .elements
            .iter()
            .find(|e| e.id.starts_with("history-record-") && e.label.contains(text))
            .map(|e| e.bounds.y)
            .ok_or_else(|| anyhow::anyhow!("anchor disappeared: {text}"))
    };
    let original_y = row_y("分页锚点 030")?;
    for start in [20, 10] {
        root_view.update(&mut cx, |v, cx| {
            v.benchmark_prepend_history(page_records(start), cx)
        });
        pump(&mut cx)?;
        anyhow::ensure!(
            (row_y("分页锚点 030")? - original_y).abs() < 0.5,
            "paging from the top moved the existing record"
        );
    }
    root_view.update(&mut cx, |v, cx| v.benchmark_scroll_history(10, 13.5, cx));
    pump(&mut cx)?;
    let partial_y = row_y("分页锚点 019")?;
    root_view.update(&mut cx, |v, cx| {
        v.benchmark_prepend_history(page_records(0), cx)
    });
    pump(&mut cx)?;
    anyhow::ensure!(
        (row_y("分页锚点 019")? - partial_y).abs() < 0.5,
        "prepend lost the partial-row pixel offset"
    );
    cx.capture_screenshot(window.into())?
        .save(out.join("pagination-anchor.png"))?;
    let writes = |include_older: bool| {
        let mut records = Vec::new();
        for i in if include_older { 0..2 } else { 1..2 } {
            let time = NOW - 202000 + i as i64 * 1000;
            records.push(record(400+i*3,json!({"kind":"step_started","step_id":format!("write-{i}"),"started_at_ms":time-10})));
            records.push(record(401+i*3,json!({"kind":"step_completed","step_id":format!("write-{i}"),"completed_at_ms":time,"invocations":[{
                "invocation_id":format!("write-{i}"),"tool":"file.write","arguments":{"path":format!("anchor-file-{i}.txt"),"content":"写入内容"},"started_at_ms":time
            }]})));
            records.push(record(402+i*3,json!({"kind":"tool_result","result":{
                "invocation_id":format!("write-{i}"),"tool":"file.write","outcome":"succeeded","data":{"bytes_written":12},"finished_at_ms":time+10
            }})));
        }
        records.extend(page_records(0));
        records
    };
    root_view.update(&mut cx, |v, cx| {
        v.benchmark_story_history(writes(false), NOW, cx)
    });
    pump(&mut cx)?;
    let write_y = row_y("anchor-file-1.txt")?;
    root_view.update(&mut cx, |v, cx| {
        v.benchmark_prepend_history(writes(true), cx)
    });
    pump(&mut cx)?;
    // The old standalone becomes an expanded child of the new routine group.
    let old_write = driver
        .snapshot(false)
        .elements
        .into_iter()
        .find(|e| {
            e.id.starts_with("history-record-")
                && e.label.starts_with("写入文件")
                && e.label.contains("anchor-file-1.txt")
        })
        .expect("reading write remained visible");
    anyhow::ensure!(
        (old_write.bounds.y - write_y).abs() < 0.5,
        "group extension moved the record being read"
    );

    // A live source changes both a mounted row and an already open popup.
    // No further mouse input is dispatched until after these assertions.
    let mut live_records = vec![
        record(
            900,
            json!({"kind":"step_started","step_id":"live","started_at_ms":NOW-8000}),
        ),
        record(
            901,
            json!({"kind":"step_completed","step_id":"live","completed_at_ms":NOW-7000,
            "invocations":[{"invocation_id":"live-tool","tool":"shell.run","arguments":{"command":"live-render-source"},"started_at_ms":NOW-7000}]}),
        ),
    ];
    eprintln!("history {width}: begin live subscription");
    let source = root_view.update(&mut cx, |v, cx| {
        v.benchmark_story_history(live_records.clone(), NOW, cx);
        v.benchmark_observe_history(cx)
    });
    pump(&mut cx)?;
    eprintln!("history {width}: begin live clock");
    root_view.update(&mut cx, |v, cx| v.benchmark_history_live_clock(cx));
    pump(&mut cx)?;
    eprintln!("history {width}: live clock ready");
    let live_bar = driver
        .snapshot(false)
        .elements
        .into_iter()
        .find(|e| e.id.starts_with("history-bar-") && e.label.contains("live-render-source"))
        .expect("live timeline bar");
    action(
        &mut cx,
        json!({"type":"move","target":{"element_id":live_bar.id}}),
    )?;
    let live_row_id = driver
        .snapshot(false)
        .elements
        .into_iter()
        .find(|e| e.id.starts_with("history-record-") && e.label.contains("live-render-source"))
        .expect("live history row")
        .id;
    let live_row = || {
        driver
            .snapshot(false)
            .elements
            .into_iter()
            .find(|e| e.id == live_row_id)
            .expect("live history row")
            .label
    };
    let popup = || {
        driver
            .snapshot(false)
            .elements
            .into_iter()
            .find(|e| e.id == "history-span-detail")
            .expect("live history popup")
    };
    let row_before = live_row();
    let popup_before = popup();
    let pointer = cx.update_window(window.into(), |_, w, _| w.mouse_position())?;
    std::thread::sleep(Duration::from_millis(1100));
    cx.advance_clock(Duration::from_millis(1100));
    pump(&mut cx)?;
    anyhow::ensure!(
        live_row() != row_before,
        "visible row clock waited for mouse input"
    );
    anyhow::ensure!(
        popup().label != popup_before.label,
        "hover duration waited for mouse input"
    );
    let clock_after = popup().label;
    live_records.push(record(
        902,
        json!({"kind":"tool_result","result":{
        "invocation_id":"live-tool","tool":"shell.run","outcome":"failed",
        "finished_at_ms":NOW+1200,"data":{"error":"changed-without-mouse"}}}),
    ));
    source.seed(zork_client_core::state::HistoryData {
        records: live_records.clone().into(),
        entries: zork_ui::history::entries(&live_records).into(),
        loaded: true,
        clock_offset_ms: NOW + 1200 - chrono::Utc::now().timestamp_millis(),
        ..Default::default()
    });
    pump(&mut cx)?;
    let row_after = live_row();
    let popup_after = popup();
    anyhow::ensure!(
        row_after.contains("失败"),
        "row missed its source update: {row_after}"
    );
    anyhow::ensure!(
        popup_after.label.contains("失败"),
        "open popup missed its source update"
    );
    anyhow::ensure!(
        cx.update_window(window.into(), |_, w, _| w.mouse_position())? == pointer,
        "data/clock regression moved the pointer"
    );
    cx.capture_screenshot(window.into())?
        .save(out.join("live-update-stationary-pointer.png"))?;
    source.seed(zork_client_core::state::HistoryData {
        loaded: true,
        ..Default::default()
    });
    pump(&mut cx)?;
    anyhow::ensure!(
        !driver
            .snapshot(false)
            .elements
            .iter()
            .any(|e| e.id == "history-span-detail"),
        "removed entry left a stale popup"
    );

    // Zoom first, then pan an adjacent span underneath a stationary pointer.
    let mut wheel_records = Vec::new();
    for i in 0..12 {
        let time = NOW - 12000 + i as i64 * 1000;
        wheel_records.push(record(
            1000 + i * 3,
            json!({"kind":"step_started","step_id":format!("wheel-{i}"),"started_at_ms":time-100}),
        ));
        wheel_records.push(record(1001+i*3, json!({"kind":"step_completed","step_id":format!("wheel-{i}"),"completed_at_ms":time,
            "invocations":[{"invocation_id":format!("wheel-{i}"),"tool":"shell.run","arguments":{"command":format!("wheel-target-{i}")},"started_at_ms":time}]})));
        wheel_records.push(record(1002+i*3, json!({"kind":"tool_result","result":{"invocation_id":format!("wheel-{i}"),"tool":"shell.run","outcome":"succeeded","finished_at_ms":time+400,"data":{}}})));
    }
    root_view.update(&mut cx, |v, cx| {
        v.benchmark_story_history(wheel_records, NOW, cx)
    });
    pump(&mut cx)?;
    action(
        &mut cx,
        json!({"type":"scroll","target":{"element_id":"history-timeline-panel"},"delta_y":120,"delta_x":0}),
    )?;
    let mut wheel_bars: Vec<_> = driver
        .snapshot(false)
        .elements
        .into_iter()
        .filter(|e| {
            e.id.starts_with("history-bar-")
                && e.label.contains("wheel-target-")
                && e.bounds.width > 2.
        })
        .collect();
    wheel_bars.sort_by(|a, b| a.center.x.total_cmp(&b.center.x));
    anyhow::ensure!(wheel_bars.len() >= 2, "zoom did not retain adjacent spans");
    let first = &wheel_bars[wheel_bars.len() / 2 - 1];
    let second = &wheel_bars[wheel_bars.len() / 2];
    action(
        &mut cx,
        json!({"type":"move","target":{"x":first.center.x,"y":first.center.y}}),
    )?;
    let wheel_before = popup();
    let pointer = cx.update_window(window.into(), |_, w, _| w.mouse_position())?;
    action(
        &mut cx,
        json!({"type":"scroll","target":{"x":first.center.x,"y":first.center.y},"delta_y":0,"delta_x":first.center.x-second.center.x}),
    )?;
    let wheel_after = popup();
    anyhow::ensure!(
        wheel_after.label != wheel_before.label,
        "wheel did not retarget the popup"
    );
    let expected = second.label.split(" · ").last().unwrap();
    anyhow::ensure!(
        wheel_after.label.contains(expected),
        "wheel selected the wrong span: {} vs {expected}",
        wheel_after.label
    );
    anyhow::ensure!(
        cx.update_window(window.into(), |_, w, _| w.mouse_position())? == pointer,
        "wheel regression synthesized a pointer move"
    );
    cx.capture_screenshot(window.into())?
        .save(out.join("wheel-stationary-pointer.png"))?;
    std::fs::write(
        out.join("live-checks.json"),
        serde_json::to_vec_pretty(&json!({
            "row_before":row_before,"row_after":row_after,"popup_before":popup_before.label,"clock_after":clock_after,"popup_after":popup_after.label,
            "wheel_before":wheel_before.label,"wheel_after":wheel_after.label,"pointer": {"x":pointer.x.as_f32(),"y":pointer.y.as_f32()}
        }))?,
    )?;
    println!(
        "history {width}x{height}: rows, folding, timeline, live sources, clocks and stationary-pointer wheel passed"
    );
    Ok(())
}
fn main() -> anyhow::Result<()> {
    run(1280., 800.)?;
    run(900., 600.)
}
