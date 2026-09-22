//! Actual native controls driven by user input, backed by the portable core.
use gpui::{px, AppContext, HeadlessAppContext};
use serde_json::json;
use std::{sync::Arc, time::Duration};
use zork_gui::{
    assets::EmbeddedAssets,
    automation::{AutomationRoot, HeadlessAutomation},
    desktop::stories::{catalog, StoryHost},
};

fn main() -> anyhow::Result<()> {
    let output = std::env::var_os("ZORK_TEST_ARTIFACT_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../artifacts/user-interactions-rework/native")
        });
    std::fs::create_dir_all(&output)?;
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
    for story in catalog()
        .into_iter()
        .filter(|s| s.family == "message-interaction")
    {
        let mut root = None;
        let window = cx.open_window(gpui::size(px(story.width), px(story.height)), |_, cx| {
            let view = cx.new(|cx| StoryHost::new(story.clone(), cx));
            root = Some(view.clone());
            cx.new(|_| AutomationRoot::new(view))
        })?;
        let root = root.unwrap();
        let pump = |cx: &mut HeadlessAppContext| -> anyhow::Result<()> {
            for _ in 0..6 {
                cx.advance_clock(Duration::from_millis(16));
                cx.run_until_parked();
                cx.update_window(window.into(), |_, w, cx| {
                    w.simulate_next_frame(cx);
                    w.draw(cx).clear(cx)
                })?;
            }
            Ok(())
        };
        let action = |cx: &mut HeadlessAppContext, action| -> anyhow::Result<()> {
            cx.update_window(window.into(), |_, w, cx| {
                driver.dispatch(serde_json::from_value(action).unwrap(), w, cx)
            })??;
            pump(cx)
        };
        let click = |cx: &mut HeadlessAppContext, id| {
            action(cx, json!({"type":"click","target":{"element_id":id}}))
        };
        pump(&mut cx)?;
        std::fs::write(
            output.join(format!("{}.json", story.id)),
            serde_json::to_vec_pretty(&root.read_with(&cx, |v, cx| v.inspect(cx)))?,
        )?;
        anyhow::ensure!(
            driver
                .snapshot(false)
                .elements
                .iter()
                .any(|e| e.id == story.target && e.visible),
            "Missing card: {}",
            story.id
        );
        cx.capture_screenshot(window.into())?
            .save(output.join(format!("{}.png", story.id)))?;
        if story.state.starts_with("long") {
            click(&mut cx, "interaction-preview-/instructions-expand")?;
            cx.capture_screenshot(window.into())?
                .save(output.join(format!("{}-expanded.png", story.id)))?;
            click(&mut cx, "interaction-preview-/instructions-expand")?;
        } else if story.state.starts_with("input") {
            click(&mut cx, "interaction-preview-submit")?;
            let invalid = root.read_with(&cx, |v, cx| v.inspect(cx));
            anyhow::ensure!(
                invalid["fields"][0]["error_key"] == "input_required",
                "No core validation: {invalid}"
            );
            click(&mut cx, "interaction-preview-title")?;
            action(&mut cx, json!({"type":"type_text","text":"实现消息卡片 ✓"}))?;
            click(&mut cx, "interaction-preview-target")?;
            click(&mut cx, "interaction-preview-target-1")?;
            click(&mut cx, "interaction-preview-submit")?;
            let completed = root.read_with(&cx, |v, cx| v.inspect(cx));
            std::fs::write(
                output.join("input-accepted.json"),
                serde_json::to_vec_pretty(&completed)?,
            )?;
            anyhow::ensure!(
                completed["status_key"] == "interaction_completed"
                    && completed["fields"][0]["value"] == "实现消息卡片 ✓"
                    && completed["fields"][1]["value"] == "preview",
                "Input not accepted: {completed}"
            );
            anyhow::ensure!(
                !driver
                    .snapshot(false)
                    .elements
                    .iter()
                    .any(|e| e.id == "interaction-preview-submit"),
                "Completed card can be submitted twice"
            );
            cx.capture_screenshot(window.into())?
                .save(output.join(format!("{}-accepted.png", story.id)))?;
        } else if story.state.starts_with("create") {
            anyhow::ensure!(
                !driver
                    .snapshot(false)
                    .elements
                    .iter()
                    .any(|e| e.id == "interaction-preview-/allowed_leaders"),
                "Advanced settings should start collapsed"
            );
            click(&mut cx, "interaction-preview-/name")?;
            action(&mut cx, json!({"type":"key","keystroke":"cmd-a"}))?;
            action(&mut cx, json!({"type":"type_text","text":"新的队员"}))?;
            click(&mut cx, "interaction-preview-settings")?;
            click(&mut cx, "interaction-preview-/allowed_leaders")?;
            click(&mut cx, "interaction-preview-/allowed_leaders-0")?;
            anyhow::ensure!(
                driver
                    .snapshot(false)
                    .elements
                    .iter()
                    .any(|e| { e.id == "interaction-preview-/allowed_leaders-menu" && e.visible }),
                "Multi-choice menu closed after changing one option"
            );
            click(&mut cx, "interaction-preview-/name")?;
            click(&mut cx, "interaction-preview-settings")?;
            click(&mut cx, "interaction-preview-settings")?;
            click(&mut cx, "interaction-preview-submit")?;
            let completed = root.read_with(&cx, |v, cx| v.inspect(cx));
            anyhow::ensure!(
                completed["fields"][0]["value"] == "新的队员"
                    && completed["status_key"] == "interaction_agent_created",
                "Edited proposal was lost: {completed}"
            );
            anyhow::ensure!(
                completed["fields"][3]["value"] == "[]",
                "Collapsed multi-choice draft was lost: {completed}"
            );
        }
        cx.update_window(window.into(), |_, w, _| w.remove_window())?;
        cx.run_until_parked();
        println!("PASS native {}", story.id);
    }
    // Wire the actual conversation view to a real core/cache. Validation must
    // reach core through the production callback, and an incoming source result
    // must update the same row while staying hidden from the displayed list.
    let directory = tempfile::tempdir()?;
    let store = Arc::new(zork_client_core::store::ClientStore::open(
        directory.path(),
    )?);
    let request: zork_client_core::api::TranscriptMessage = serde_json::from_value(json!({
        "type":"message","id":"actual-request","chat_id":"render-fixture","role":"assistant",
        "content":"请选择执行目标。","interaction":{"version":5,"handler":"agent.configuration","request_id":"owner/actual-request","kind":"request","request":{
            "action":"input","title":"执行目标","fields":[{"id":"target","label":"目标","required":true}]
        }}
    }))?;
    store.cache_message_page(
        "mini1",
        "render-fixture",
        &zork_client_core::api::MessagePage {
            source_epoch: None,
            items: vec![request.clone()],
            older_cursor: None,
        },
        None,
    )?;
    let mut root = None;
    let window = cx.open_window(gpui::size(px(1280.), px(800.)), |_, cx| {
        let view =
            cx.new(|cx| zork_gui::views::RootView::render_benchmark_fixture(false, store, cx));
        root = Some(view.clone());
        cx.new(|_| AutomationRoot::new(view))
    })?;
    let root = root.unwrap();
    let conversation = root.update(&mut cx, |view, cx| {
        view.benchmark_replace_messages(
            vec![zork_client_core::transcript::transcript_line_from(&request).unwrap()],
            cx,
        );
        view.benchmark_bind_core(cx)
    });
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
    pump(&mut cx)?;
    let click = serde_json::from_value(
        json!({"type":"click","target":{"element_id":"interaction-actual-request-submit"}}),
    )?;
    cx.update_window(window.into(), |_, w, cx| driver.dispatch(click, w, cx))??;
    pump(&mut cx)?;
    let snapshot = conversation.snapshot();
    let zork_client_core::transcript::TranscriptLine::Message { metadata, .. } = &snapshot.lines[0];
    anyhow::ensure!(
        metadata.interaction_view.as_ref().unwrap().fields[0]
            .error_key
            .as_deref()
            == Some("input_required"),
        "Production action did not reach core validation"
    );
    cx.capture_screenshot(window.into())?
        .save(output.join("conversation-validation.png"))?;
    conversation.seed_event(&zork_client_core::api::SseEvent { name: "message".into(), data: json!({
        "type":"message","id":"actual-result","chat_id":"render-fixture","role":"assistant","author_kind":"system",
        "content":"Input accepted","reply_to":"actual-request","interaction":{"version":5,"handler":"agent.configuration","request_id":"owner/actual-request","kind":"result","result":{
            "request_message_id":"actual-request","response_id":"actual-response","revision":1,"outcome":"completed",
            "actor":"fixture-user","output":{"values":{"target":"测试环境\n".repeat(100)}}
        }}
    }).to_string() });
    pump(&mut cx)?;
    anyhow::ensure!(
        conversation.snapshot().lines.len() == 1,
        "Result became a second visible row"
    );
    anyhow::ensure!(
        !driver
            .snapshot(false)
            .elements
            .iter()
            .any(|e| e.id == "interaction-actual-request-submit"),
        "Resolved production card remains actionable"
    );
    cx.capture_screenshot(window.into())?
        .save(output.join("conversation-completed.png"))?;
    let collapsed = driver
        .snapshot(false)
        .elements
        .iter()
        .find(|e| e.id == "interaction-card-actual-request")
        .unwrap()
        .bounds
        .height;
    let expand = serde_json::from_value(
        json!({"type":"click","target":{"element_id":"interaction-actual-request-target-expand"}}),
    )?;
    cx.update_window(window.into(), |_, w, cx| driver.dispatch(expand, w, cx))??;
    pump(&mut cx)?;
    let expanded = driver
        .snapshot(false)
        .elements
        .iter()
        .find(|e| e.id == "interaction-card-actual-request")
        .unwrap()
        .bounds
        .height;
    anyhow::ensure!(
        expanded > collapsed && expanded < 600.,
        "Long card did not remeasure within a bounded viewport: {collapsed} -> {expanded}"
    );
    cx.capture_screenshot(window.into())?
        .save(output.join("conversation-expanded.png"))?;
    cx.update_window(window.into(), |_, w, _| w.remove_window())?;
    println!(
        "PASS production conversation callback, cache result folding and stable card identity"
    );
    Ok(())
}
