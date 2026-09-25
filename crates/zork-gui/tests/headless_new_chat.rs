//! Real shared controls: selection, Unicode input, Enter, busy state and geometry.
use gpui::{px, size, AppContext, HeadlessAppContext};
use serde_json::{json, Value};
use std::{path::PathBuf, sync::Arc, time::Duration};
use zork_gui::{
    assets::EmbeddedAssets,
    automation::{AutomationRoot, HeadlessAutomation},
    desktop::stories::{Story, StoryHost},
};

/// A model served by another maker's router: the model row and trigger carry
/// the maker's mark, the connection list carries the provider's mark.
fn maker_and_provider_marks() -> anyhow::Result<()> {
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
    let mut story = Story::new("new-chat", "新建 Chat", "picker", "", "new-chat");
    story.width = 900.;
    story.height = 700.;
    let window = cx.open_window(size(px(900.), px(700.)), |_, cx| {
        let view = cx.new(|cx| StoryHost::new(story, cx));
        cx.new(|_| AutomationRoot::new(view))
    })?;
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
    let label = |id: &str| {
        driver
            .snapshot(false)
            .elements
            .into_iter()
            .find(|e| e.id == id && e.visible)
            .map(|e| e.label)
    };
    draw(&mut cx)?;
    anyhow::ensure!(
        label("new-chat-options-mark").as_deref() == Some("makers/openai.svg"),
        "trigger mark {:?}",
        label("new-chat-options-mark")
    );
    action(json!({"type":"click","target":{"element_id":"new-chat-options"}}), &mut cx)?;
    // Demo model is served by 个人账号 and API, both OpenAI.
    action(json!({"type":"click","target":{"element_id":"new-chat-connection"}}), &mut cx)?;
    anyhow::ensure!(label("new-chat-connection-mark-1").as_deref() == Some("providers/openai.svg"));
    anyhow::ensure!(label("new-chat-connection-mark-2").as_deref() == Some("providers/openai.svg"));
    action(json!({"type":"click","target":{"element_id":"new-chat-connection"}}), &mut cx)?;
    // The list scrolls; rows below the fold are still in the tree.
    let all = driver.snapshot(true).elements;
    let rows: Vec<(String, String)> = all
        .iter()
        .filter(|e| e.id.starts_with("new-chat-model-") && !e.id.contains("mark") && e.id != "new-chat-model-list")
        .filter_map(|e| {
            let index = e.id.trim_start_matches("new-chat-model-");
            let mark = all.iter().find(|m| m.id == format!("new-chat-model-mark-{index}"))?;
            Some((e.label.clone(), mark.label.clone()))
        })
        .collect();
    let mark = |model: &str| {
        rows.iter()
            .find(|(label, _)| label == model)
            .map(|(_, mark)| mark.as_str())
    };
    anyhow::ensure!(mark("deepseek-flash") == Some("makers/deepseek.svg"), "rows {rows:?}");
    anyhow::ensure!(mark("glm-5.1") == Some("makers/zhipu.svg"), "rows {rows:?}");
    anyhow::ensure!(mark("muse-spark") == Some("makers/generic.svg"), "rows {rows:?}");
    anyhow::ensure!(mark("Demo fast") == Some("makers/openai.svg"), "rows {rows:?}");
    // Choosing the router's DeepSeek model moves the trigger to the DeepSeek mark.
    let index = all
        .iter()
        .find(|e| e.id.starts_with("new-chat-model-") && e.label == "deepseek-flash")
        .map(|e| e.id.clone())
        .unwrap();
    for delta in [-400., 400.] {
        let visible = driver.snapshot(false).elements.iter().any(|e| e.id == index && e.visible && e.bounds == e.visible_bounds);
        if visible {
            break;
        }
        let row = all.iter().find(|e| e.id == "new-chat-model-0").unwrap().bounds;
        let (x, y) = (row.x + row.width / 2., row.y + row.height / 2.);
        action(json!({"type":"scroll","target":{"x":x,"y":y},"delta_y":delta}), &mut cx)?;
    }
    action(json!({"type":"click","target":{"element_id":index}}), &mut cx)?;
    anyhow::ensure!(
        label("new-chat-options-mark").as_deref() == Some("makers/deepseek.svg"),
        "trigger after choosing deepseek-flash {:?}",
        label("new-chat-options-mark")
    );
    // Only OpenCode Go serves it: automatic plus that one connection, with its provider's mark.
    action(json!({"type":"click","target":{"element_id":"new-chat-connection"}}), &mut cx)?;
    anyhow::ensure!(
        label("new-chat-connection-mark-1").as_deref() == Some("providers/opencode.svg"),
        "OpenCode Go connection {:?}",
        label("new-chat-connection-mark-1")
    );
    anyhow::ensure!(label("new-chat-connection-2").is_none(), "connections not serving the model are listed");
    println!("maker marks ok");
    Ok(())
}

/// Model and thinking are the choice; the connection is optional, automatic
/// by default, and can be pinned to a connection serving the model.
fn optional_connection() -> anyhow::Result<()> {
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
    story.width = 900.;
    story.height = 700.;
    let mut host = None;
    let window = cx.open_window(size(px(900.), px(700.)), |_, cx| {
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
    let action = |value: Value, cx: &mut HeadlessAppContext| -> anyhow::Result<()> {
        cx.update_window(window.into(), |_, w, cx| {
            driver.dispatch(serde_json::from_value(value)?, w, cx)
        })??;
        draw(cx)
    };
    let click = |id: &str, cx: &mut HeadlessAppContext| {
        action(json!({"type":"click","target":{"element_id":id}}), cx)
    };
    let key = |k: &str, cx: &mut HeadlessAppContext| action(json!({"type":"key","keystroke":k}), cx);
    let label = |id: &str| {
        driver
            .snapshot(false)
            .elements
            .into_iter()
            .find(|e| e.id == id && e.visible)
            .map(|e| e.label)
    };
    let selection = |cx: &mut HeadlessAppContext| {
        let state = host.read_with(cx, |view, cx| view.inspect(cx));
        (
            state["model"]["value"].as_str().unwrap_or_default().to_owned(),
            state["thinking"]["value"].as_str().unwrap_or_default().to_owned(),
            state["profile"]["value"].as_str().unwrap_or_default().to_owned(),
        )
    };
    let sel = |m: &str, t: &str, p: &str| (m.to_owned(), t.to_owned(), p.to_owned());
    draw(&mut cx)?;
    anyhow::ensure!(label("new-chat-options").as_deref() == Some("Demo model · high"), "trigger {:?}", label("new-chat-options"));
    anyhow::ensure!(label("new-chat-options-connection").is_none(), "automatic connection shown in the trigger");
    click("new-chat-options", &mut cx)?;
    // Demo model is offered by two connections and still listed once.
    let rows: Vec<String> = driver
        .snapshot(true)
        .elements
        .into_iter()
        .filter(|e| {
            e.id.strip_prefix("new-chat-model-")
                .is_some_and(|rest| rest.chars().all(|c| c.is_ascii_digit()))
        })
        .map(|e| e.label)
        .collect();
    anyhow::ensure!(rows == ["Demo model", "Demo fast"], "model rows {rows:?}");
    click("new-chat-model-1", &mut cx)?;
    click("new-chat-model-0", &mut cx)?;
    anyhow::ensure!(selection(&mut cx) == sel("Demo model", "high", "auto"), "picking a model keeps the connection automatic: {:?}", selection(&mut cx));
    // The connection row names the default and unfolds into the serving connections.
    anyhow::ensure!(label("new-chat-connection").as_deref() == Some("连接 · 自动"), "connection row {:?}", label("new-chat-connection"));
    anyhow::ensure!(label("new-chat-connection-0").is_none(), "connection list open before asked");
    click("new-chat-connection", &mut cx)?;
    for (id, text) in [
        ("new-chat-connection-0", "连接 · 自动"),
        ("new-chat-connection-1", "连接 · 个人账号"),
        ("new-chat-connection-2", "连接 · API"),
    ] {
        anyhow::ensure!(label(id).as_deref() == Some(text), "{id}: {:?}", label(id));
    }
    anyhow::ensure!(label("new-chat-connection-mark-0").as_deref() == Some("icons/sparkles.svg"));
    click("new-chat-connection-2", &mut cx)?;
    anyhow::ensure!(selection(&mut cx) == sel("Demo model", "high", "api"), "pinning API: {:?}", selection(&mut cx));
    anyhow::ensure!(label("new-chat-connection-0").is_none(), "connection list stayed open after pinning");
    anyhow::ensure!(label("new-chat-connection").as_deref() == Some("连接 · API"));
    anyhow::ensure!(label("new-chat-connection-mark").as_deref() == Some("providers/openai.svg"));
    anyhow::ensure!(label("new-chat-options").as_deref() == Some("Demo model · high · API"), "pinned trigger {:?}", label("new-chat-options"));
    anyhow::ensure!(label("new-chat-options-connection").as_deref() == Some("API"));
    // API does not serve Demo fast: switching resets the connection to automatic.
    click("new-chat-model-1", &mut cx)?;
    anyhow::ensure!(selection(&mut cx) == sel("Demo fast", "off", "auto"), "reset to automatic: {:?}", selection(&mut cx));
    anyhow::ensure!(label("new-chat-options").as_deref() == Some("Demo fast · 关"), "trigger after reset {:?}", label("new-chat-options"));
    anyhow::ensure!(label("new-chat-options-connection").is_none());
    // Keyboard: arrows move the model and thinking; a serving pin survives.
    click("new-chat-connection", &mut cx)?;
    click("new-chat-connection-1", &mut cx)?;
    anyhow::ensure!(selection(&mut cx).2 == "personal", "pinning 个人账号: {:?}", selection(&mut cx));
    key("up", &mut cx)?;
    anyhow::ensure!(selection(&mut cx) == sel("Demo model", "high", "personal"), "up: {:?}", selection(&mut cx));
    key("left", &mut cx)?;
    anyhow::ensure!(selection(&mut cx) == sel("Demo model", "medium", "personal"), "left: {:?}", selection(&mut cx));
    key("down", &mut cx)?;
    anyhow::ensure!(selection(&mut cx).0 == "Demo fast" && selection(&mut cx).2 == "personal", "down: {:?}", selection(&mut cx));
    key("right", &mut cx)?;
    anyhow::ensure!(selection(&mut cx) == sel("Demo fast", "low", "personal"), "right: {:?}", selection(&mut cx));
    anyhow::ensure!(label("new-chat-options").as_deref() == Some("Demo fast · low · 个人账号"));
    key("enter", &mut cx)?;
    anyhow::ensure!(label("new-chat-model-0").is_none(), "Enter did not close the panel");
    println!("PASS optional connection: listed once, automatic by default, pin, reset, trigger, keys");
    Ok(())
}

fn main() -> anyhow::Result<()> {
    maker_and_provider_marks()?;
    optional_connection()?;
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
            "new-chat-device-0",
            "new-chat-device-1",
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
        // Up to four devices show as tabs: each keeps its name and Mesh
        // status visible without opening a menu.
        for id in ["new-chat-device-name-0", "new-chat-device-name-1"] {
            anyhow::ensure!(
                snapshot.elements.iter().any(|e| e.id == id && e.visible),
                "device tab name {id} missing at {width}"
            );
        }
        anyhow::ensure!(
            snapshot
                .elements
                .iter()
                .find(|element| element.id == "new-chat-options")
                .is_some_and(|element| element.label.contains("Demo model · high")),
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
        let before = host.read_with(&cx, |view, cx| view.inspect(cx));
        action(
            json!({"type":"click","target":{"element_id":"new-chat-device-1"}}),
            &mut cx,
        )?;
        let after = host.read_with(&cx, |view, cx| view.inspect(cx));
        anyhow::ensure!(
            after["device"]["value"] != before["device"]["value"],
            "clicking a device tab did not select it: {after}"
        );
        action(
            json!({"type":"click","target":{"element_id":"new-chat-options"}}),
            &mut cx,
        )?;
        cx.capture_screenshot(window.into())?
            .save(output.join(format!("new-chat-picker-{width}.png")))?;
        // One panel: each model once, the selected model's own thinking
        // options below, the optional connection last, no second layer.
        let picker = driver.snapshot(false);
        for id in ["new-chat-model-0", "new-chat-model-1", "new-chat-connection"] {
            anyhow::ensure!(
                picker
                    .elements
                    .iter()
                    .any(|e| e.id == id && e.visible && e.bounds == e.visible_bounds),
                "{id} not visible in the model panel at {width}"
            );
        }
        anyhow::ensure!(
            !picker.elements.iter().any(|e| e.id == "new-chat-picker-back"),
            "the model panel must not have a second layer"
        );
        action(
            json!({"type":"click","target":{"element_id":"new-chat-model-1"}}),
            &mut cx,
        )?;
        action(
            json!({"type":"click","target":{"element_id":"new-chat-thinking-1"}}),
            &mut cx,
        )?;
        let state = host.read_with(&cx, |view, cx| view.inspect(cx));
        anyhow::ensure!(
            state["model"]["value"] == "Demo fast"
                && state["thinking"]["value"] == "low"
                && state["profile"]["value"] == "auto",
            "selection did not reach core fixture: {state}"
        );
        anyhow::ensure!(
            driver
                .snapshot(false)
                .elements
                .iter()
                .find(|element| element.id == "new-chat-options")
                .is_some_and(|element| element.label.contains("Demo fast · low")),
            "picker trigger did not show the selected model and thinking"
        );
        cx.capture_screenshot(window.into())?
            .save(output.join(format!("new-chat-picker-selected-{width}.png")))?;
        action(json!({"type":"key","keystroke":"escape"}), &mut cx)?;
        anyhow::ensure!(
            !driver
                .snapshot(false)
                .elements
                .iter()
                .any(|e| e.id == "new-chat-model-0" && e.visible),
            "Escape did not close picker"
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
    anyhow::ensure!(
        driver
            .snapshot(false)
            .elements
            .iter()
            .any(|e| e.id == "new-chat-model-39" && e.visible && e.bounds == e.visible_bounds),
        "opening a long model list did not reveal the current choice: {:?}",
        driver
            .snapshot(false)
            .elements
            .iter()
            .filter(|e| e.id.starts_with("new-chat"))
            .map(|e| (&e.id, e.visible, e.bounds.y))
            .collect::<Vec<_>>()
    );
    action(
        json!({"type":"click","target":{"element_id":"new-chat-model-0"}}),
        &mut cx,
    )
    .ok();
    action(json!({"type":"key","keystroke":"up"}), &mut cx)?;
    anyhow::ensure!(
        choices.borrow().last().is_some_and(|choice| choice == "model-38"),
        "arrow keys did not move the model choice: {:?}",
        choices.borrow()
    );
    println!("PASS long model selector: current item visible and arrow keys move the choice");
    Ok(())
}
