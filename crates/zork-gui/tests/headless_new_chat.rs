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
    long_model_selection()?;
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
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "missing {id}: {:?}",
                        snapshot
                            .elements
                            .iter()
                            .filter(|e| e.id.starts_with("new-chat-"))
                            .map(|e| e.id.as_str())
                            .collect::<Vec<_>>()
                    )
                })?;
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
        anyhow::ensure!(
            snapshot
                .elements
                .iter()
                .find(|e| e.id == "new-chat-device")
                .is_some_and(|e| e.label.contains('●') && !e.label.contains("直连")),
            "selected device must show a compact Mesh status icon"
        );
        anyhow::ensure!(
            snapshot
                .elements
                .iter()
                .find(|element| element.id == "new-chat-options")
                .is_some_and(|element| element.label.contains("Demo model · 高")),
            "picker trigger did not show the selected model and strength at {width}"
        );
        let initial_picker_width = snapshot
            .elements
            .iter()
            .find(|element| element.id == "new-chat-options")
            .unwrap()
            .bounds
            .width;
        cx.capture_screenshot(window.into())?
            .save(output.join(format!("new-chat-{width}.png")))?;
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
        action(
            json!({"type":"click","target":{"element_id":"new-chat-device-1"}}),
            &mut cx,
        )?;
        action(json!({"type":"key","keystroke":"enter"}), &mut cx)?;
        anyhow::ensure!(
            driver
                .snapshot(false)
                .elements
                .iter()
                .any(|e| e.id == "new-chat-device-1" && e.visible),
            "device dropdown did not reopen from the keyboard"
        );
        action(json!({"type":"key","keystroke":"escape"}), &mut cx)?;
        action(
            json!({"type":"click","target":{"element_id":"new-chat-options"}}),
            &mut cx,
        )?;
        cx.capture_screenshot(window.into())?
            .save(output.join(format!("new-chat-picker-max-{width}.png")))?;
        action(
            json!({"type":"click","target":{"element_id":"new-chat-thinking-keyboard"}}),
            &mut cx,
        )?;
        action(json!({"type":"key","keystroke":"home"}), &mut cx)?;
        action(json!({"type":"key","keystroke":"right"}), &mut cx)?;
        anyhow::ensure!(
            driver
                .snapshot(false)
                .elements
                .iter()
                .find(|element| element.id == "new-chat-options")
                .is_some_and(|element| element.label.contains("Demo model · 中")),
            "picker trigger did not follow the visible strength at {width}"
        );
        cx.capture_screenshot(window.into())?
            .save(output.join(format!("new-chat-picker-{width}.png")))?;
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
        let picker = driver.snapshot(false);
        let model_row = picker
            .elements
            .iter()
            .find(|element| element.id == "new-chat-model-0")
            .ok_or_else(|| anyhow::anyhow!("missing model choice at {width}"))?;
        let profile_row = picker
            .elements
            .iter()
            .find(|element| element.id == "new-chat-profile-0")
            .ok_or_else(|| anyhow::anyhow!("missing Profile choice at {width}"))?;
        anyhow::ensure!(
            model_row.visible
                && profile_row.visible
                && model_row.bounds == model_row.visible_bounds
                && profile_row.bounds == profile_row.visible_bounds
                && model_row.bounds.y == profile_row.bounds.y
                && model_row.bounds.x + model_row.bounds.width <= profile_row.bounds.x,
            "model and Profile choices are not visible side by side at {width}"
        );
        cx.capture_screenshot(window.into())?
            .save(output.join(format!("new-chat-models-{width}.png")))?;
        anyhow::ensure!(
            driver
                .snapshot(false)
                .elements
                .iter()
                .find(|element| element.id == "new-chat-profile-2")
                .is_some_and(|element| element.visible && element.bounds == element.visible_bounds),
            "last Profile option clipped at {width}"
        );
        cx.capture_screenshot(window.into())?
            .save(output.join(format!("new-chat-profiles-{width}.png")))?;
        action(
            json!({"type":"click","target":{"element_id":"new-chat-profile-1"}}),
            &mut cx,
        )?;
        anyhow::ensure!(
            driver
                .snapshot(false)
                .elements
                .iter()
                .any(|element| element.id == "new-chat-model-0" && element.visible),
            "choosing Profile hid the model choices at {width}"
        );
        action(
            json!({"type":"click","target":{"element_id":"new-chat-model-1"}}),
            &mut cx,
        )?;
        anyhow::ensure!(
            driver
                .snapshot(false)
                .elements
                .iter()
                .any(|element| element.id == "new-chat-profile-0" && element.visible),
            "choosing a model hid the Profile choices at {width}"
        );
        action(
            json!({"type":"click","target":{"element_id":"new-chat-picker-back"}}),
            &mut cx,
        )?;
        action(
            json!({"type":"click","target":{"element_id":"new-chat-thinking-keyboard"}}),
            &mut cx,
        )?;
        action(json!({"type":"key","keystroke":"end"}), &mut cx)?;
        action(
            json!({"type":"click","target":{"element_id":"new-chat-thinking-reset"}}),
            &mut cx,
        )?;
        anyhow::ensure!(
            host.read_with(&cx, |view, cx| view.inspect(cx))["thinking"]["value"] == "off",
            "reset did not use the selected model's default"
        );
        anyhow::ensure!(
            driver
                .snapshot(false)
                .elements
                .iter()
                .any(|element| element.id == "new-chat-thinking-keyboard" && element.visible),
            "reset unexpectedly left the strength picker at {width}"
        );
        action(
            json!({"type":"click","target":{"element_id":"new-chat-thinking-keyboard"}}),
            &mut cx,
        )?;
        action(json!({"type":"key","keystroke":"end"}), &mut cx)?;
        anyhow::ensure!(
            driver
                .snapshot(false)
                .elements
                .iter()
                .find(|element| element.id == "new-chat-options")
                .is_some_and(|element| element.label.contains("Demo fast · 低")),
            "picker trigger did not show the selected model and strength"
        );
        let selected_picker_width = driver
            .snapshot(false)
            .elements
            .iter()
            .find(|element| element.id == "new-chat-options")
            .unwrap()
            .bounds
            .width;
        anyhow::ensure!(
            selected_picker_width < initial_picker_width,
            "picker trigger did not shrink with its label at {width}: {initial_picker_width:?} -> {selected_picker_width:?}"
        );
        let state = host.read_with(&cx, |view, cx| view.inspect(cx));
        anyhow::ensure!(
            state["device"]["value"] == "remote"
                && state["model"]["value"] == "Demo fast"
                && state["thinking"]["value"] == "low"
                && state["profile"]["value"] == "personal",
            "selection did not reach core fixture: {state}"
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
    }
    Ok(())
}

fn long_model_selection() -> anyhow::Result<()> {
    use std::{cell::RefCell, rc::Rc};
    use zork_ui::{
        new_chat::{Event, Page},
        resources::Text,
    };
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
    let text = Text(Rc::new(|key| key.into()));
    let mut data = zork_client_core::new_chat::Fixture::new("draft").snapshot();
    let option = data.model.options[0].clone();
    data.model.options = (0..40)
        .map(|i| {
            let mut option = option.clone();
            option.value = format!("model-{i}");
            option.label = format!("Model {i}");
            option
        })
        .collect();
    data.model.value = "model-39".into();
    let mut page = None;
    let choices = Rc::new(RefCell::new(Vec::new()));
    let window = cx.open_window(size(px(900.), px(800.)), |_, cx| {
        let view = cx.new(|cx| {
            let mut view = Page::new(text.clone(), cx);
            view.configure(data.clone(), 600., text.clone(), cx);
            view
        });
        page = Some(view.clone());
        let choices = choices.clone();
        cx.subscribe(&view, move |_, event: &Event, _| {
            if let Event::Intent(zork_client_core::new_chat::Action::Model { value }) = event {
                choices.borrow_mut().push(value.clone());
            }
        })
        .detach();
        cx.new(|_| AutomationRoot::new(view))
    })?;
    let page = page.unwrap();
    let draw = |cx: &mut HeadlessAppContext| -> anyhow::Result<()> {
        for _ in 0..4 {
            cx.run_until_parked();
            cx.advance_clock(Duration::from_millis(16));
            cx.update_window(window.into(), |_, w, cx| w.draw(cx).clear(cx))?;
        }
        Ok(())
    };
    let action = |value: Value, cx: &mut HeadlessAppContext| -> anyhow::Result<()> {
        cx.update_window(window.into(), |_, w, cx| {
            driver.dispatch(serde_json::from_value(value)?, w, cx)
        })??;
        draw(cx)
    };
    draw(&mut cx)?;
    action(
        json!({"type":"click","target":{"element_id":"new-chat-options"}}),
        &mut cx,
    )?;
    let center = driver
        .snapshot(false)
        .elements
        .iter()
        .find(|e| e.id == "new-chat-thinking-label")
        .unwrap()
        .center;
    action(
        json!({"type":"click","target":{"x":center.x,"y":center.y}}),
        &mut cx,
    )?;
    anyhow::ensure!(
        driver
            .snapshot(false)
            .elements
            .iter()
            .any(|e| e.id == "new-chat-model-39" && e.visible && e.bounds == e.visible_bounds),
        "opening a long model list did not reveal the current choice"
    );
    action(
        json!({"type":"click","target":{"element_id":"new-chat-model-39"}}),
        &mut cx,
    )?;
    for (value, count) in [("model-10", 40), ("model-1", 2)] {
        data.model.options.truncate(count);
        data.model.value = value.into();
        cx.update(|cx| {
            page.update(cx, |page, cx| {
                page.configure(data.clone(), 600., text.clone(), cx)
            })
        });
        draw(&mut cx)?;
        action(json!({"type":"key","keystroke":"enter"}), &mut cx)?;
        anyhow::ensure!(
            choices
                .borrow()
                .last()
                .is_some_and(|choice| choice == value),
            "Enter used stale selection after model projection changed to {value}: {:?}",
            choices.borrow()
        );
    }
    println!("PASS long model selector: current item visible, external selection and shrinking options reconcile keyboard focus");
    Ok(())
}
