//! Full transcript messages and new-message positioning.
use gpui::{px, AppContext, HeadlessAppContext};
use serde_json::json;
use std::{sync::Arc, time::Duration};
use zork_gui::{
    api::Role,
    assets::EmbeddedAssets,
    automation::{AutomationRoot, HeadlessAutomation},
    views::{RootView, TranscriptLine},
};

fn main() -> anyhow::Result<()> {
    // The lazily mounted brand reads the same platform preference as the app.
    std::env::set_var("ZORK_GUI_TEST_REDUCE_MOTION", "1");
    let output = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../artifacts/message-presentation/native");
    std::fs::create_dir_all(&output)?;
    let directory = tempfile::tempdir()?;
    let store = Arc::new(zork_gui::desktop::store::ClientStore::open(
        directory.path(),
    )?);
    let source = format!("# 完整方案\n\n{}\n全文末尾：END-OF-FULL-MESSAGE", "保留现有消息样式，完整内容在侧边标签中阅读。\n\n```rust\nlet message = \"中文🐈\";\n```\n\n".repeat(30));
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
    let mut view = None;
    let window = cx.open_window(gpui::size(px(1280.), px(800.)), |_, cx| {
        let root = cx.new(|cx| RootView::render_benchmark_fixture(false, store, cx));
        view = Some(root.clone());
        cx.new(|_| AutomationRoot::new(root))
    })?;
    let view = view.unwrap();
    view.update(&mut cx, |v, cx| {
        v.benchmark_replace_messages(
            vec![
                TranscriptLine::Message {
                    role: Role::Assistant,
                    content: source.clone(),
                    metadata: serde_json::from_value(
                        json!({"id":"long-message","author_name":"产品 Leader"}),
                    )
                    .unwrap(),
                },
                TranscriptLine::Message {
                    role: Role::User,
                    content: "我先看预览。".into(),
                    metadata: serde_json::from_value(json!({"id":"short-message"})).unwrap(),
                },
            ],
            cx,
        )
    });
    let pump = |cx: &mut HeadlessAppContext| -> anyhow::Result<()> {
        for _ in 0..6 {
            cx.advance_clock(Duration::from_millis(16));
            cx.run_until_parked();
            cx.update_window(window.into(), |_, w, cx| {
                w.simulate_next_frame(cx);
            })?;
        }
        Ok(())
    };
    pump(&mut cx)?;
    let snapshot = driver.snapshot(false);
    anyhow::ensure!(
        !snapshot.elements.iter().any(|e| e.id == "message-expand-0"),
        "Transcript still offers message folding"
    );
    anyhow::ensure!(
        !snapshot.elements.iter().any(|e| e.id == "message-full-0"),
        "Transcript still offers the full-message dialog"
    );
    cx.capture_screenshot(window.into())?
        .save(output.join("full-message.png"))?;
    view.update(&mut cx, |v, cx| v.benchmark_follow_messages(cx));
    pump(&mut cx)?;
    let tail = driver.snapshot(false);
    anyhow::ensure!(
        !tail
            .elements
            .iter()
            .any(|e| e.id == "message-full-0" || e.id == "message-reader-dialog"),
        "Long message still has a reader footer or dialog"
    );
    cx.capture_screenshot(window.into())?
        .save(output.join("full-message-end.png"))?;
    cx.update_window(window.into(), |_, w, cx| {
        w.resize(gpui::size(px(900.), px(600.)));
        w.bounds_changed(cx);
    })?;
    pump(&mut cx)?;
    cx.capture_screenshot(window.into())?
        .save(output.join("compact.png"))?;
    println!("PASS full inline message without a reader footer at wide and compact sizes");
    // Exercise real incoming events and real wheel interruption through the
    // production observer, using wall-clock frames for the 200ms motion.
    view.update(&mut cx, |v, cx| {
        v.benchmark_replace_messages(
            (0..12)
                .map(|i| TranscriptLine::Message {
                    role: Role::Assistant,
                    content: format!(
                        "消息 {i}\n\n保留消息位置，平滑跟随新的内容。\n\n第二段正文。"
                    ),
                    metadata: serde_json::from_value(json!({"id":format!("motion-{i}")})).unwrap(),
                })
                .collect(),
            cx,
        )
    });
    let conversation = view.update(&mut cx, |v, cx| v.benchmark_bind_core(cx));
    pump(&mut cx)?;
    let mut connected = conversation.snapshot().as_ref().clone();
    connected.connected = true;
    conversation.seed(connected);
    pump(&mut cx)?;
    std::env::set_var("ZORK_GUI_TEST_REDUCE_MOTION", "0");
    cx.update(|cx| cx.set_reduce_motion(false));
    // A history refresh after connection readiness must not manufacture activity.
    let mut history = conversation.snapshot().as_ref().clone();
    history.lines.push(TranscriptLine::Message {
        role: Role::Assistant,
        content: "启动时补齐的历史消息".into(),
        metadata: serde_json::from_value(json!({"id":"restored-history"})).unwrap(),
    });
    conversation.seed(history);
    pump(&mut cx)?;
    let motion = view.update(&mut cx, |v, _| v.benchmark_message_motion());
    anyhow::ensure!(
        !motion.0 && motion.1 == 0,
        "History restoration replayed arrival activity"
    );
    view.update(&mut cx, |v, cx| v.benchmark_follow_messages(cx));
    pump(&mut cx)?;
    conversation.seed_event(&zork_client_core::api::SseEvent { name: "message".into(), data: json!({"type":"message","id":"motion-new","role":"assistant","content":"进入动画的新消息\n\n滚动应当平滑跟随。"}).to_string() });
    let mut offsets = Vec::new();
    for _ in 0..20 {
        std::thread::sleep(Duration::from_millis(16));
        cx.advance_clock(Duration::from_millis(16));
        cx.run_until_parked();
        cx.update_window(window.into(), |_, w, cx| {
            w.simulate_next_frame(cx);
            w.draw(cx).clear(cx)
        })?;
        let state = view.update(&mut cx, |v, _| v.benchmark_frame_state(false));
        offsets.push((state.1, state.2));
    }
    anyhow::ensure!(
        offsets.windows(2).any(|pair| pair[0] != pair[1]),
        "New-message scrolling did not move: {offsets:?}"
    );
    let motion = view.update(&mut cx, |v, _| v.benchmark_message_motion());
    anyhow::ensure!(
        !motion.0 && motion.2,
        "Short-message tail follow did not resume: {motion:?}"
    );
    let wheel = serde_json::from_value(
        json!({"type":"scroll","target":{"element_id":"conversation-transcript"},"delta_y":300}),
    )?;
    cx.update_window(window.into(), |_, w, cx| driver.dispatch(wheel, w, cx))??;
    pump(&mut cx)?;
    let position = view.update(&mut cx, |v, _| v.benchmark_frame_state(false));
    conversation.seed_event(&zork_client_core::api::SseEvent { name: "message".into(), data: json!({"type":"message","id":"motion-unread","role":"assistant","content":"用户正在读历史时，不抢滚动位置。"}).to_string() });
    pump(&mut cx)?;
    let held = view.update(&mut cx, |v, _| v.benchmark_frame_state(false));
    anyhow::ensure!(
        position.1 == held.1 && (position.2 - held.2).abs() < 1.,
        "Arrival stole the reading position"
    );
    anyhow::ensure!(
        driver
            .snapshot(false)
            .elements
            .iter()
            .any(|e| e.id == "messages-new"),
        "Missing new-message affordance while reading history"
    );
    std::fs::write(
        output.join("scroll-frames.json"),
        serde_json::to_vec_pretty(&offsets)?,
    )?;
    cx.capture_screenshot(window.into())?
        .save(output.join("new-message-affordance.png"))?;
    println!("PASS smooth tail follow and manual-scroll anchor preservation");
    // A message taller than the viewport must land at its first line. A second
    // quick arrival must target the second ID, even while the first is highlighted.
    std::env::set_var("ZORK_GUI_TEST_REDUCE_MOTION", "1");
    cx.update(|cx| cx.set_reduce_motion(true));
    view.update(&mut cx, |v, cx| v.benchmark_follow_messages(cx));
    pump(&mut cx)?;
    let first_index = view.update(&mut cx, |v, _| v.benchmark_transcript_len());
    let before_tall = view.update(&mut cx, |v, _| {
        (v.benchmark_frame_state(false), v.benchmark_message_motion())
    });
    anyhow::ensure!(
        before_tall.1 .2,
        "Fixture is not following before tall arrival: {before_tall:?}"
    );
    let tall = format!(
        "TALL-HEADER\n\n{}",
        "This line keeps the message taller than the window.\n".repeat(50)
    );
    conversation.seed_event(&zork_client_core::api::SseEvent {
        name: "message".into(),
        data: json!({"type":"message","id":"motion-tall","role":"assistant","content":tall})
            .to_string(),
    });
    for _ in 0..10 {
        pump(&mut cx)?;
        if view.update(&mut cx, |v, _| v.benchmark_transcript_len()) > first_index {
            break;
        }
    }
    let first = view.update(&mut cx, |v, _| v.benchmark_frame_state(false));
    anyhow::ensure!(
        first.1 == first_index && first.2.abs() < 1.,
        "Tall arrival did not show its header: index={first_index}, len={}, state={first:?}, before={before_tall:?}, motion={:?}",
        view.update(&mut cx, |v, _| v.benchmark_transcript_len()),
        view.update(&mut cx, |v, _| v.benchmark_message_motion())
    );
    conversation.seed_event(&zork_client_core::api::SseEvent {
        name: "message".into(),
        data: json!({"type":"message","id":"motion-after-tall","role":"assistant","content":"NEXT-HEADER\n\nThe next arrival starts here."}).to_string(),
    });
    for _ in 0..10 {
        pump(&mut cx)?;
        if view.update(&mut cx, |v, _| v.benchmark_transcript_len()) > first_index + 1 {
            break;
        }
    }
    let second = view.update(&mut cx, |v, _| v.benchmark_frame_state(false));
    let next_visible = driver.snapshot(false).elements.iter().any(|element| {
        element
            .id
            .starts_with(&format!("message-{}-", first_index + 1))
    });
    anyhow::ensure!(
        next_visible,
        "Consecutive arrival stayed on the previous message: index={first_index}, len={}, state={second:?}",
        view.update(&mut cx, |v, _| v.benchmark_transcript_len())
    );
    println!("PASS tall and consecutive arrivals land on each new message header");
    Ok(())
}
