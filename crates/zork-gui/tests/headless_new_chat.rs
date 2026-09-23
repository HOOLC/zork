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
            "new-chat-options",
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
            snapshot
                .elements
                .iter()
                .find(|element| element.id == "new-chat-options")
                .is_some_and(|element| element.label == "Demo model · 高"),
            "the model picker trigger must identify the selected model at {width}"
        );
        let bounds = |id| {
            snapshot
                .elements
                .iter()
                .find(|element| element.id == id)
                .unwrap()
                .bounds
                .clone()
        };
        let surface = bounds("new-chat-composer-surface");
        let rail = bounds("new-chat-device-rail");
        let device = bounds("new-chat-device");
        anyhow::ensure!(
            rail.x > surface.x
                && rail.x + rail.width < surface.x + surface.width
                && rail.y < surface.y
                && rail.y + rail.height > surface.y
                && device.y >= rail.y
                && device.y + device.height <= surface.y,
            "device rail must be narrower than and overlap behind the composer at {width}"
        );
        anyhow::ensure!(
            (bounds("new-chat-options").height - bounds("new-chat-send").height).abs() < 0.1,
            "picker and send control heights differ at {width}"
        );
        anyhow::ensure!(
            !snapshot
                .elements
                .iter()
                .find(|e| e.id == "new-chat-send")
                .unwrap()
                .enabled,
            "empty draft can be sent"
        );
        anyhow::ensure!(
            snapshot
                .elements
                .iter()
                .find(|e| e.id == "new-chat-device")
                .is_some_and(|e| e.label.contains('●') && !e.label.contains("直连")),
            "selected device must show a compact Mesh status icon"
        );
        let screenshot = cx.capture_screenshot(window.into())?;
        let scale = screenshot.width() as f32 / width;
        let sample = |x: f32, y: f32| {
            screenshot
                .get_pixel((x * scale) as u32, (y * scale) as u32)
                .0
        };
        let middle = surface.x + surface.width / 2.;
        let header = sample(middle, rail.y + 4.);
        let editor = sample(middle, surface.y + 12.);
        anyhow::ensure!(
            u16::from(header[0]) + 4 < u16::from(editor[0])
                && u16::from(header[1]) + 4 < u16::from(editor[1]),
            "device rail lost its gray section at {width}: {header:?} / {editor:?}"
        );
        anyhow::ensure!(
            sample(surface.x + 4., rail.y + 10.)[..3]
                .iter()
                .all(|channel| *channel >= 250),
            "device rail unexpectedly fills the composer width at {width}"
        );
        for fraction in [0.15, 0.5, 0.85] {
            let x = surface.x + surface.width * fraction;
            for offset in [-1., 0., 1.] {
                let pixel = sample(x, surface.y + offset);
                anyhow::ensure!(
                    pixel[..3].iter().all(|channel| *channel < 250),
                    "canvas-colored seam between rail and composer at {width}: {pixel:?}"
                );
            }
        }
        let shoulder = sample(rail.x + 2., surface.y + 2.);
        anyhow::ensure!(
            shoulder[..3].iter().all(|channel| *channel < 250),
            "the rail was erased behind the composer corner at {width}: {shoulder:?}"
        );
        screenshot.save(output.join(format!("new-chat-{width}.png")))?;
        let action = |value: Value, cx: &mut HeadlessAppContext| -> anyhow::Result<()> {
            cx.update_window(window.into(), |_, w, cx| {
                driver.dispatch(serde_json::from_value(value)?, w, cx)
            })??;
            draw(cx)
        };
        action(
            json!({"type":"click","target":{"element_id":"new-chat-device"}}),
            &mut cx,
        )?;
        anyhow::ensure!(
            driver
                .snapshot(false)
                .elements
                .iter()
                .find(|e| e.id == "new-chat-device-1")
                .is_some_and(|e| e.label.contains('◌') && !e.label.contains("Mesh 准备中")),
            "remote device option must show a compact Mesh status icon"
        );
        for id in ["new-chat-device-1", "new-chat-options"] {
            action(json!({"type":"click","target":{"element_id":id}}), &mut cx)?;
        }
        cx.capture_screenshot(window.into())?
            .save(output.join(format!("new-chat-picker-max-{width}.png")))?;
        action(
            json!({"type":"click","target":{"element_id":"new-chat-thinking-thumb-0"}}),
            &mut cx,
        )?;
        action(json!({"type":"key","keystroke":"home"}), &mut cx)?;
        action(json!({"type":"key","keystroke":"right"}), &mut cx)?;
        let label_center = driver
            .snapshot(false)
            .elements
            .iter()
            .find(|e| e.id == "new-chat-thinking-label")
            .unwrap()
            .center;
        action(
            json!({"type":"click","target":{"x":label_center.x,"y":label_center.y}}),
            &mut cx,
        )?;
        cx.capture_screenshot(window.into())?
            .save(output.join(format!("new-chat-picker-{width}.png")))?;
        for id in [
            "new-chat-model",
            "new-chat-model-1",
            "new-chat-thinking-thumb-0",
        ] {
            action(json!({"type":"click","target":{"element_id":id}}), &mut cx)?;
        }
        action(json!({"type":"key","keystroke":"end"}), &mut cx)?;
        action(
            json!({"type":"click","target":{"element_id":"new-chat-thinking-reset"}}),
            &mut cx,
        )?;
        anyhow::ensure!(
            host.read_with(&cx, |view, cx| view.inspect(cx))["thinking"]["value"] == "off",
            "reset did not use the selected model's default"
        );
        action(
            json!({"type":"click","target":{"element_id":"new-chat-thinking-thumb-0"}}),
            &mut cx,
        )?;
        action(json!({"type":"key","keystroke":"end"}), &mut cx)?;
        for id in ["new-chat-model", "new-chat-profile", "new-chat-profile-1"] {
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
        anyhow::ensure!(
            driver
                .snapshot(false)
                .elements
                .iter()
                .find(|element| element.id == "new-chat-options")
                .is_some_and(|element| element.label == "Demo fast · 低"),
            "the model picker trigger did not follow the selection at {width}"
        );
        action(json!({"type":"key","keystroke":"escape"}), &mut cx)?;
        anyhow::ensure!(
            !driver
                .snapshot(false)
                .elements
                .iter()
                .any(|e| e.id == "new-chat-model" && e.visible),
            "Escape did not close picker"
        );
        action(json!({"type":"key","keystroke":"enter"}), &mut cx)?;
        anyhow::ensure!(
            driver
                .snapshot(false)
                .elements
                .iter()
                .any(|e| e.id == "new-chat-model" && e.visible),
            "close did not restore trigger focus"
        );
        action(json!({"type":"key","keystroke":"escape"}), &mut cx)?;
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
        if width == 900. {
            let mut closeup = Story::new("new-chat", "新建 Chat", "draft", "", "new-chat");
            closeup.width = width;
            closeup.height = 180.;
            let closeup_window = cx.open_window(size(px(width), px(180.)), |_, cx| {
                let view = cx.new(|cx| StoryHost::new(closeup, cx));
                cx.new(|_| AutomationRoot::new(view))
            })?;
            for _ in 0..4 {
                cx.run_until_parked();
                cx.advance_clock(Duration::from_millis(16));
                cx.update_window(closeup_window.into(), |_, window, cx| {
                    window.draw(cx).clear(cx)
                })?;
            }
            cx.capture_screenshot(closeup_window.into())?
                .save(output.join("new-chat-closeup.png"))?;
        }
    }
    Ok(())
}
