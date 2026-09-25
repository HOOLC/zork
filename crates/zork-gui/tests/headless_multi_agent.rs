//! Several agents in one Chat, in the production desktop transcript: identity
//! discs and device names, header-line reply quotes with the omission rule,
//! the jump to the original, and load-then-jump for not-loaded originals.
//! Every rule comes from core `message_presentation`; this checks the wiring.
use anyhow::{ensure, Context as _, Result};
use gpui::{px, AppContext, HeadlessAppContext};
use serde_json::json;
use std::{sync::Arc, time::Duration};
use zork_gui::{
    api::Role,
    assets::EmbeddedAssets,
    automation::{protocol::ElementInfo, AutomationRoot, HeadlessAutomation},
    views::{RootView, TranscriptLine},
};

fn at(minutes_ago: i64) -> String {
    (chrono::Utc::now() - chrono::Duration::minutes(minutes_ago)).to_rfc3339()
}

fn line(role: Role, value: serde_json::Value, content: &str) -> TranscriptLine {
    TranscriptLine::Message {
        role,
        content: content.into(),
        metadata: serde_json::from_value(value).unwrap(),
    }
}

fn agent(id: &str, who: &str, minutes_ago: i64, content: &str) -> TranscriptLine {
    let (name, device, model) = match who {
        "planner" => ("Planner", "origin-a", "gpt-6-astra"),
        "builder" => ("Builder", "origin-b", "deepseek-flash"),
        _ => ("审阅助手", "origin-b", "claude-sonnet-5"),
    };
    line(
        Role::Assistant,
        json!({"id": id, "created_at": at(minutes_ago), "author_agent_id": who,
               "author_name": name, "device": device, "model": model}),
        content,
    )
}

fn reply(mut value: TranscriptLine, to: &str, quote: Option<(&str, &str)>) -> TranscriptLine {
    let TranscriptLine::Message { metadata, .. } = &mut value;
    metadata.reply_to = Some(to.into());
    if let Some((text, kind)) = quote {
        metadata.quote = Some(text.into());
        metadata.quote_kind = serde_json::from_value(json!(kind)).ok();
    }
    value
}

fn user(id: &str, minutes_ago: i64, content: &str) -> TranscriptLine {
    line(
        Role::User,
        json!({"id": id, "created_at": at(minutes_ago)}),
        content,
    )
}

fn main() -> Result<()> {
    std::env::set_var("ZORK_GUI_TEST_REDUCE_MOTION", "1");
    let output =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../artifacts/multi-agent/native");
    std::fs::create_dir_all(&output)?;
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
    let mut view = None;
    let window = cx.open_window(gpui::size(px(1280.), px(800.)), |_, cx| {
        let root = cx.new(|cx| RootView::render_benchmark_fixture(false, store, cx));
        view = Some(root.clone());
        cx.new(|_| AutomationRoot::new(root))
    })?;
    let view = view.unwrap();
    let pump = |cx: &mut HeadlessAppContext| -> Result<()> {
        for _ in 0..6 {
            cx.advance_clock(Duration::from_millis(16));
            cx.run_until_parked();
            cx.update_window(window.into(), |_, w, cx| w.simulate_next_frame(cx))?;
        }
        Ok(())
    };
    let find = |id: &str| -> Result<ElementInfo> {
        driver
            .snapshot(false)
            .elements
            .into_iter()
            .find(|e| e.id == id)
            .with_context(|| format!("missing element {id}"))
    };
    let absent = |id: &str| !driver.snapshot(false).elements.iter().any(|e| e.id == id);
    let long = "验收标准：首屏只保留账号、密码和一个主按钮；错误提示贴在对应字段下方，不弹窗；键盘可以走完全部流程；深色主题逐项核对对比度。";
    let mut rows = vec![
        user(
            "u1",
            90,
            "@Planner 把登录页改版拆成任务，审阅助手和 Builder 一起跟进。",
        ),
        agent("criteria", "planner", 88, long),
        agent(
            "b1",
            "builder",
            80,
            "骨架已推到 `ud/login-refresh`，表单和按钮先复用现有组件。",
        ),
        // Adjacent reply (only its own author in between): omitted within a screen.
        reply(
            agent("r1", "review", 70, "看过了：密码框缺少显示/隐藏切换。"),
            "b1",
            Some(("表单和按钮先复用现有组件", "excerpt")),
        ),
        user("u2", 60, "按审阅意见改，改完叫我看。"),
    ];
    for i in 0..6 {
        rows.push(agent(
            &format!("f{i}"),
            "builder",
            50 - i,
            &format!(
                "进度 {i}：{}",
                "这一段让列表超过一屏，好检验远处的引用。".repeat(4)
            ),
        ));
    }
    // Far reply with an excerpt: shown in the identity row, jumps on click.
    rows.push(reply(
        agent(
            "r2",
            "review",
            3,
            "按最初的验收标准复查：深色主题下错误提示的对比度还差一点。",
        ),
        "criteria",
        Some(("深色主题逐项核对对比度", "excerpt")),
    ));
    // Reply to an original older than the loaded rows.
    rows.push(reply(
        agent("r3", "planner", 1, "埋点按上周定的口径更新。"),
        "old",
        Some(("上周定的埋点口径", "summary")),
    ));
    let count = rows.len();
    view.update(&mut cx, |v, cx| {
        v.benchmark_replace_messages(rows, cx);
        v.benchmark_set_has_older(true, cx);
        v.benchmark_scroll_to_end(cx);
    });
    pump(&mut cx)?;
    pump(&mut cx)?;
    let r2 = count - 2;
    let r3 = count - 1;
    let identity = find(&format!("message-{r2}-identity"))?;
    ensure!(
        identity.label.starts_with("审阅助手") && identity.label.contains("claude-sonnet-5"),
        "identity detail is {:?}",
        identity.label
    );
    ensure!(
        find(&format!("message-{r2}-device"))?.label == "origin-b",
        "multi-device Chat did not show the device name"
    );
    let line = find(&format!("message-{r2}-reply"))?;
    ensure!(
        line.label == "回复 Planner 「深色主题逐项核对对比度」",
        "reply line reads {:?}",
        line.label
    );
    let not_loaded = find(&format!("message-{r3}-reply"))?;
    ensure!(
        not_loaded.label == "回复 更早的消息 · 尚未加载 · 点击加载",
        "not-loaded line reads {:?}",
        not_loaded.label
    );
    cx.capture_screenshot(window.into())?
        .save(output.join("transcript.png"))?;
    println!("PASS identity discs, device names and header-line reply quotes");

    // The jump: the original starts ~24 px below the top, with the wash.
    let click = serde_json::from_value(
        json!({"type":"click","target":{"element_id":format!("message-{r2}-reply")}}),
    )?;
    cx.update_window(window.into(), |_, w, cx| driver.dispatch(click, w, cx))??;
    pump(&mut cx)?;
    let (_, top, offset, _) = view.update(&mut cx, |v, _| v.benchmark_frame_state(false));
    ensure!(
        top == 0 || (top == 1 && offset < 30.),
        "jump did not reach the original: top={top} offset={offset}"
    );
    cx.capture_screenshot(window.into())?
        .save(output.join("jump-wash.png"))?;
    // The adjacent reply's original is on the same screen: its line is omitted.
    ensure!(
        absent("message-3-reply"),
        "adjacent reply within one screen kept its line"
    );
    println!("PASS reply jump with wash and the one-screen omission rule");

    // Not loaded: the first click only loads, then the line jumps.
    view.update(&mut cx, |v, cx| v.benchmark_scroll_to_end(cx));
    pump(&mut cx)?;
    let click = serde_json::from_value(
        json!({"type":"click","target":{"element_id":format!("message-{r3}-reply")}}),
    )?;
    cx.update_window(window.into(), |_, w, cx| driver.dispatch(click, w, cx))??;
    pump(&mut cx)?;
    ensure!(
        view.update(&mut cx, |v, _| v.benchmark_reply_loading())
            .as_deref()
            == Some("r3"),
        "first click did not start loading older history"
    );
    let older = vec![
        user(
            "old0",
            60 * 24 * 8,
            "上周的登录埋点口径先定下来：点击和展开分开统计。",
        ),
        agent(
            "old",
            "planner",
            60 * 24 * 8 - 3,
            "收到，口径写进了 `docs/metrics.md`，后续改版沿用。",
        ),
    ];
    view.update(&mut cx, |v, cx| v.benchmark_older_loaded(older, cx));
    pump(&mut cx)?;
    ensure!(
        find("transcript-hint")?.label == "已加载，再点引用可以跳转到原消息",
        "loading did not explain the next click"
    );
    let r3 = r3 + 2;
    let loaded = find(&format!("message-{r3}-reply"))?;
    ensure!(
        loaded.label == "回复 Planner 大意 上周定的埋点口径",
        "loaded line reads {:?}",
        loaded.label
    );
    cx.capture_screenshot(window.into())?
        .save(output.join("loaded-hint.png"))?;
    let click = serde_json::from_value(
        json!({"type":"click","target":{"element_id":format!("message-{r3}-reply")}}),
    )?;
    cx.update_window(window.into(), |_, w, cx| driver.dispatch(click, w, cx))??;
    pump(&mut cx)?;
    let (_, top, _, _) = view.update(&mut cx, |v, _| v.benchmark_frame_state(false));
    ensure!(
        top <= 1,
        "second click did not jump to the loaded original: top={top}"
    );
    println!("PASS not-loaded originals load first, then jump");

    // Narrow column: the device name moves into the hover detail.
    cx.update_window(window.into(), |_, w, cx| {
        w.resize(gpui::size(px(700.), px(800.)));
        w.bounds_changed(cx);
    })?;
    view.update(&mut cx, |v, cx| v.benchmark_scroll_to_end(cx));
    pump(&mut cx)?;
    pump(&mut cx)?;
    cx.capture_screenshot(window.into())?
        .save(output.join("narrow.png"))?;
    println!("PASS narrow layout rendered");
    Ok(())
}
