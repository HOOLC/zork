//! Real shared controls: selection, Unicode input, Enter, busy state and geometry.
use gpui::{px, size, AppContext, HeadlessAppContext};
use serde_json::{json, Value};
use std::{path::PathBuf, sync::Arc, time::Duration};
use zork_gui::{
    assets::EmbeddedAssets,
    automation::{AutomationRoot, HeadlessAutomation},
    desktop::stories::{Story, StoryHost},
};

fn main() -> anyhow::Result<()> {
    let output =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../artifacts/chat-first/native");
    std::fs::create_dir_all(&output)?;
    for width in [360., 900.] {
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
        let mut story = Story::new("new-chat", "新建 Chat", "draft", "", "new-chat");
        story.width = width;
        story.height = 700.;
        let mut host = None;
        let window = cx.open_window(size(px(width), px(700.)), |_, cx| {
            let view = cx.new(|cx| StoryHost::new(story, cx));
            host = Some(view.clone());
            cx.new(|_| AutomationRoot::new(view))
        })?;
        let host = host.unwrap();
        let draw = |cx: &mut HeadlessAppContext| -> anyhow::Result<()> {
            for _ in 0..4 {
                cx.run_until_parked();
                cx.advance_clock(Duration::from_millis(16));
                cx.update_window(window.into(), |_, w, cx| w.draw(cx).clear(cx))?;
            }
            Ok(())
        };
        draw(&mut cx)?;
        let snapshot = driver.snapshot(false);
        for id in [
            "new-chat-input",
            "new-chat-device",
            "new-chat-model",
            "new-chat-thinking",
            "new-chat-profile",
            "new-chat-send",
        ] {
            let element = snapshot
                .elements
                .iter()
                .find(|e| e.id == id)
                .ok_or_else(|| anyhow::anyhow!("missing {id}"))?;
            anyhow::ensure!(
                element.visible && element.bounds == element.visible_bounds,
                "clipped {id} at {width}"
            );
        }
        anyhow::ensure!(
            !snapshot
                .elements
                .iter()
                .find(|e| e.id == "new-chat-send")
                .unwrap()
                .enabled,
            "empty draft can be sent"
        );
        cx.capture_screenshot(window.into())?
            .save(output.join(format!("new-chat-{width}.png")))?;
        let action = |value: Value, cx: &mut HeadlessAppContext| -> anyhow::Result<()> {
            cx.update_window(window.into(), |_, w, cx| {
                driver.dispatch(serde_json::from_value(value)?, w, cx)
            })??;
            draw(cx)
        };
        for id in [
            "new-chat-device",
            "new-chat-device-1",
            "new-chat-model",
            "new-chat-model-1",
            "new-chat-thinking",
            "new-chat-thinking-1",
            "new-chat-profile",
            "new-chat-profile-1",
        ] {
            action(json!({"type":"click","target":{"element_id":id}}), &mut cx)?;
        }
        let state = host.read_with(&cx, |view, cx| view.inspect(cx));
        anyhow::ensure!(
            state["device"]["value"] == "remote"
                && state["model"]["value"] == "Demo fast"
                && state["thinking"]["value"] == "low"
                && state["profile"]["value"] == "personal",
            "selection did not reach core fixture: {state}"
        );
        action(
            json!({"type":"click","target":{"element_id":"new-chat-input"}}),
            &mut cx,
        )?;
        action(
            json!({"type":"type_text","text":"请检查中文输入 🦊"}),
            &mut cx,
        )?;
        action(json!({"type":"key","keystroke":"shift-enter"}), &mut cx)?;
        action(json!({"type":"type_text","text":"保留换行"}), &mut cx)?;
        let state = host.read_with(&cx, |view, cx| view.inspect(cx));
        anyhow::ensure!(
            state["text"] == "请检查中文输入 🦊\n保留换行" && state["can_submit"] == true,
            "draft/input mismatch: {state}"
        );
        cx.capture_screenshot(window.into())?
            .save(output.join(format!("new-chat-input-{width}.png")))?;
        action(json!({"type":"key","keystroke":"enter"}), &mut cx)?;
        let state = host.read_with(&cx, |view, cx| view.inspect(cx));
        anyhow::ensure!(
            state["busy"] == true && state["editable"] == false && state["can_submit"] == false,
            "Enter did not submit exactly one operation: {state}"
        );
        let snapshot = driver.snapshot(false);
        anyhow::ensure!(
            !snapshot
                .elements
                .iter()
                .find(|e| e.id == "new-chat-send")
                .unwrap()
                .enabled,
            "busy send still enabled"
        );
        std::fs::write(
            output.join(format!("new-chat-{width}.json")),
            serde_json::to_vec_pretty(&snapshot)?,
        )?;
        println!("PASS new Chat at {width}px: real selectors, Unicode/Shift+Enter, submit, busy and bounds");
    }
    Ok(())
}
