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
        "input_id":"input-0", "request_id":"fixture-user-message", "content":"查看执行历史，连续工具操作分组；等待独立显示。", "received_at_ms":NOW-90000}}),
    )];
    rows.push(record(
        1,
        json!({"kind":"step_started","step_id":"reply","purpose":"conversation","started_at_ms":NOW-89000}),
    ));
    rows.push(record(
        2,
        json!({"kind":"step_completed","step_id":"reply","purpose":"conversation","completed_at_ms":NOW-88000,
        "assistant_text":"我先读一遍历史投影，再合并连续的操作。历史页现在是 Cue 的活动页：概览在标题下方，记录行按操作分组，等待单独成行，回复是它自己的 Markdown 文档。我会逐行核对投影、分组、时长与绝对时钟，确认展开后仍能看到完整内容，然后把这次对齐的结论记录下来，并把它交给后续的评测与实现。如果记录行丢掉了来源、目标或状态，或者回复的文档被截断，这次对齐就不算完成；所以我在每一轮都重新读一遍投影，确认按时间排序的分组没有把等待错误地合并进常规操作，确认编辑与写入分开计数，确认发送与接收各有自己的措辞。页面的标题、执行记录滚动区、已到 Session 开始处、加载更早记录、正在加载记录和暂时无法加载都保持 Cue 的措辞，概览只统计整个会话的用量，绝不回到某一行。","invocations":[]}),
    ));
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
            "invocations":[{
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
    // Brand reads the platform preference on render; the page itself is static.
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
                    content: "查看执行历史，连续工具操作分组；等待独立显示。".into(),
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
    action(
        &mut cx,
        json!({"type":"scroll","target":{"element_id":"history-ledger"},"delta_y":10000}),
    )?;
    cx.capture_screenshot(window.into())?
        .save(out.join("initial.png"))?;
    std::fs::write(
        out.join("initial.json"),
        serde_json::to_vec_pretty(&driver.snapshot(true))?,
    )?;
    // Cue's page keeps the overview above the records, the paging state, the
    // rows and the reply disclosure. These replace the timeline-hover checks: a
    // page that loses its rows, its state or its disclosure fails here.
    let page = driver.snapshot(false);
    let older = page
        .elements
        .iter()
        .find(|e| e.id == "history-older")
        .expect("the page state control is missing");
    anyhow::ensure!(!older.enabled, "an idle page offered paging");
    anyhow::ensure!(
        page.elements
            .iter()
            .filter(|e| e.id.starts_with("history-record-"))
            .count()
            >= 3,
        "the page lost its records"
    );
    let disclosure_id = page
        .elements
        .iter()
        .find(|e| e.id.starts_with("history-output-disclosure-") && e.label == "展开更多")
        .expect("the reply disclosure is missing")
        .id
        .clone();
    let reply_id = page
        .elements
        .iter()
        .find(|e| e.id.starts_with("history-record-") && e.label.contains("我先读一遍历史投影"))
        .expect("assistant reply is a history row")
        .id
        .clone();
    let reply_label = || {
        driver
            .snapshot(false)
            .elements
            .into_iter()
            .find(|e| e.id == reply_id)
            .expect("assistant reply is a history row")
            .label
    };
    let reply_before = reply_label();
    std::thread::sleep(Duration::from_millis(1100));
    cx.advance_clock(Duration::from_millis(1100));
    pump(&mut cx)?;
    // Cue stamps the reply with an absolute clock, so the row holds still while
    // the page keeps ticking; a relative clock would rewrite the line.
    anyhow::ensure!(
        reply_label() == reply_before,
        "the reply row moved without new data: {reply_before} -> {}",
        reply_label()
    );
    action(
        &mut cx,
        json!({"type":"click","target":{"element_id":disclosure_id}}),
    )?;
    // The expanded reply is taller than the narrow window, so its disclosure and
    // clock sit below the fold there. Their state is what this checks, so read
    // the snapshot with hidden elements included.
    let expanded = driver.snapshot(true);
    anyhow::ensure!(
        expanded
            .elements
            .iter()
            .any(|e| e.id == disclosure_id && e.label == "收起"),
        "the reply disclosure did not open"
    );
    let clock = expanded
        .elements
        .iter()
        .find(|e| e.id.starts_with("history-output-clock-"))
        .map(|e| e.label.clone())
        .expect("the expanded reply hid its absolute clock");
    anyhow::ensure!(
        clock.len() == 8 && clock.matches(":").count() == 2,
        "the record clock is not absolute wall time: {clock}"
    );
    cx.capture_screenshot(window.into())?
        .save(out.join("expanded-reply.png"))?;
    // The expanded reply outgrows the narrow window, so its disclosure sits
    // below the fold there; scroll the page back onto it before toggling.
    let ledger = driver
        .snapshot(false)
        .elements
        .into_iter()
        .find(|e| e.id == "history-ledger")
        .map(|e| e.bounds)
        .expect("history ledger");
    action(
        &mut cx,
        json!({"type":"scroll","target":{"x":ledger.x + ledger.width * 0.5,"y":ledger.y + ledger.height * 0.5},"delta_y":-800}),
    )?;
    action(
        &mut cx,
        json!({"type":"click","target":{"element_id":disclosure_id}}),
    )?;
    // An empty snapshot is a page state, not an error: no rows, no paging.
    root_view.update(&mut cx, |v, cx| v.benchmark_story_history(vec![], NOW, cx));
    pump(&mut cx)?;
    let empty = driver.snapshot(false);
    anyhow::ensure!(
        empty
            .elements
            .iter()
            .all(|e| !e.id.starts_with("history-record-")),
        "an empty history kept rows"
    );
    anyhow::ensure!(
        empty
            .elements
            .iter()
            .any(|e| e.id == "history-older" && !e.enabled),
        "the empty page lost its state"
    );
    root_view.update(&mut cx, |v, cx| {
        v.benchmark_story_history(fixture(), NOW, cx)
    });
    pump(&mut cx)?;
    action(
        &mut cx,
        json!({"type":"scroll","target":{"element_id":"history-ledger"},"delta_y":10000}),
    )?;
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
    // Cue pads the scroll region 16px, so the page state spans the padded list
    // rather than the whole page.
    let activity = bounds("history-ledger");
    anyhow::ensure!(
        (older.width - activity.width).abs() < 34.,
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
            .any(|e| e.id.starts_with("history-record-")
                && e.label.contains("收到来自 测试用户 的消息")),
        "the user's message did not read its durable source receipt"
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
                && e.label.contains("执行 1 条命令")
        })
        .expect("routine summary visible");
    let group_id = group.id.clone();
    // Cue's reply row is its Markdown document, so it legitimately exceeds the
    // single-line row height; every other row still has to stay compact.
    let reply = snapshot
        .elements
        .iter()
        .find(|e| e.id.starts_with("history-record-") && e.label.contains("我先读一遍历史投影"))
        .expect("assistant reply is a history row");
    anyhow::ensure!(
        snapshot
            .elements
            .iter()
            .filter(|e| e.id.starts_with("history-record-") && e.id != reply.id)
            .all(|e| e.bounds.height <= 46.),
        "history item exceeded two lines"
    );
    anyhow::ensure!(
        reply.label.contains("我先读一遍历史投影") && !reply.label.starts_with("输出"),
        "the reply row does not read as its own text: {}",
        reply.label
    );
    anyhow::ensure!(
        snapshot
            .elements
            .iter()
            .any(|e| e.id.starts_with("history-record-") && e.label.contains("已等待 15s")),
        "wait duration did not end on mailbox wake"
    );
    if !snapshot.elements.iter().any(|e| {
        e.id.starts_with("history-record-")
            && e.label.contains("cargo test --locked")
            && e.label.contains("失败")
    }) {
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
                    && e.label.contains("cargo test --locked")
                    && e.label.contains("失败")),
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
    // Re-opening the collapsed block must reveal that exact invocation, not
    // the hidden model row at the old list index.
    anyhow::ensure!(
        !driver.snapshot(false).elements.iter().any(|e| {
            e.id.starts_with("history-record-")
                && e.label.starts_with("执行命令")
                && e.label.contains("rg -n history")
        }),
        "a collapsed group still exposed its members"
    );
    action(
        &mut cx,
        json!({"type":"click","target":{"element_id":group_id}}),
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
        .expect("expanding the group revealed the invocation");
    let ledger = driver
        .snapshot(false)
        .elements
        .into_iter()
        .find(|e| e.id == "history-ledger")
        .expect("history viewport")
        .bounds;
    // Expanding a group preserves its anchor, so a narrow viewport may leave
    // the last child below the fold. Reach it with the same scroll as a user.
    if shell.bounds.y + shell.bounds.height > ledger.y + ledger.height - 48.
        || shell.bounds.y < ledger.y
    {
        action(
            &mut cx,
            json!({"type":"scroll","target":{"element_id":"history-ledger"},
                "delta_y":ledger.y + ledger.height * 0.5 - shell.bounds.y - shell.bounds.height * 0.5}),
        )?;
    }
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
            .any(|e| e.id.starts_with("history-inline-detail-") && e.label.contains("matches")),
        "tool details did not expand inline"
    );
    cx.capture_screenshot(window.into())?
        .save(out.join("details.png"))?;
    action(&mut cx, json!({"type":"key","keystroke":"enter"}))?;
    anyhow::ensure!(
        !driver
            .snapshot(false)
            .elements
            .iter()
            .any(|e| e.id.starts_with("history-inline-detail-")),
        "Enter did not collapse the focused tool"
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
    action(
        &mut cx,
        json!({"type":"scroll","target":{"element_id":"history-ledger"},"delta_y":10000}),
    )?;
    std::fs::write(
        out.join("pagination-top.json"),
        serde_json::to_vec_pretty(&driver.snapshot(true))?,
    )?;
    let original_y = row_y("分页锚点 030")?;
    for start in [20, 10] {
        root_view.update(&mut cx, |v, cx| {
            v.benchmark_prepend_history(page_records(start), cx)
        });
        pump(&mut cx)?;
        std::fs::write(
            out.join(format!("pagination-{start}.json")),
            serde_json::to_vec_pretty(&driver.snapshot(true))?,
        )?;
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
    action(
        &mut cx,
        json!({"type":"scroll","target":{"element_id":"history-ledger"},"delta_y":10000}),
    )?;
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

    // A live source changes a mounted row: its absolute clock holds still while
    // the page keeps ticking, and new data rewrites the line.
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
    let row_before = live_row();
    std::thread::sleep(Duration::from_millis(1100));
    cx.advance_clock(Duration::from_millis(1100));
    pump(&mut cx)?;
    // Cue stamps a record with an absolute clock, so a mounted line holds still
    // until new data arrives; a relative clock would rewrite it on a tick.
    anyhow::ensure!(
        live_row() == row_before,
        "the absolute row clock moved without new data: {row_before} -> {}",
        live_row()
    );
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
    anyhow::ensure!(
        row_after.contains("失败"),
        "row missed its source update: {row_after}"
    );
    cx.capture_screenshot(window.into())?
        .save(out.join("live-update.png"))?;
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
            .any(|e| e.id == live_row_id),
        "a removed entry left its row behind"
    );

    // The emptied source leaves the page state and no records behind.
    let live_page = driver.snapshot(false);
    anyhow::ensure!(
        live_page.elements.iter().any(|e| e.id == "history-older"),
        "the live page lost its state control"
    );
    anyhow::ensure!(
        live_page
            .elements
            .iter()
            .all(|e| !e.id.starts_with("history-record-")),
        "an emptied source still painted records"
    );
    std::fs::write(
        out.join("live-checks.json"),
        serde_json::to_vec_pretty(&json!({
            "row_before":row_before,"row_after":row_after
        }))?,
    )?;
    println!(
        "history {width}x{height}: records, folding, page states, live sources, absolute clocks and the reply disclosure passed"
    );
    Ok(())
}
fn main() -> anyhow::Result<()> {
    run(1280., 800.)?;
    run(900., 600.)
}
