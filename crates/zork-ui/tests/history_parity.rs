//! Mouse/keyboard evidence through the same complete component as the desktop.
use gpui::{px, size, AppContext, HeadlessAppContext};
use serde_json::{json, Value};
use std::{path::PathBuf, sync::Arc, time::Duration};
use zork_ui::{
    assets::EmbeddedAssets,
    automation::{
        element::{AutomationRegistry, AutomationRegistryGlobal},
        protocol::UserAction,
        AutomationRoot,
    },
};
struct HeadlessAutomation(AutomationRegistry);
impl HeadlessAutomation {
    fn install(cx: &mut gpui::App) -> Self {
        let registry = AutomationRegistry::new();
        cx.set_global(AutomationRegistryGlobal(registry.clone()));
        Self(registry)
    }
    fn snapshot(&self, hidden: bool) -> zork_ui::automation::protocol::UiSnapshot {
        self.0.snapshot(hidden)
    }
    fn dispatch(
        &self,
        action: UserAction,
        w: &mut gpui::Window,
        cx: &mut gpui::App,
    ) -> Result<(), String> {
        zork_ui::automation::driver::headless_action(action, w, cx, &self.0)
            .map(|_| ())
            .map_err(|e| format!("{}: {}", e.code, e.message))
    }
}
use zork_ui::history_page::stories::Story;

fn run(width: f32) -> Result<(), Box<dyn std::error::Error>> {
    let output = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../artifacts/session-history-parity/native");
    std::fs::create_dir_all(&output)?;
    let mut cx = HeadlessAppContext::with_platform(
        gpui_platform::current_platform(true).text_system(),
        Arc::new(EmbeddedAssets),
        gpui_platform::current_headless_renderer,
    );
    let driver = cx.update(|cx| {
        zork_ui::assets::init_fonts(cx);
        zork_ui::components::init(cx);
        cx.set_reduce_motion(true);
        HeadlessAutomation::install(cx)
    });
    let catalog: std::collections::HashMap<String, String> =
        serde_json::from_str(include_str!("../../zork-gui/locales/zh-CN.json"))?;
    let text = zork_ui::resources::Text(std::rc::Rc::new(move |key| {
        catalog.get(key).cloned().unwrap_or_else(|| key.into())
    }));
    let window = cx.open_window(size(px(width), px(600.)), |_, cx| {
        let story = cx.new(|cx| Story::with_text("collapsed", text, cx));
        cx.new(|_| AutomationRoot::new(story))
    })?;
    let pump = |cx: &mut HeadlessAppContext| -> Result<(), Box<dyn std::error::Error>> {
        for _ in 0..8 {
            cx.advance_clock(Duration::from_millis(16));
            cx.run_until_parked();
            cx.update_window(window.into(), |_, w, cx| w.draw(cx).clear(cx))?;
        }
        Ok(())
    };
    let action =
        |cx: &mut HeadlessAppContext, value: Value| -> Result<(), Box<dyn std::error::Error>> {
            cx.update_window(window.into(), |_, w, cx| {
                driver.dispatch(serde_json::from_value::<UserAction>(value).unwrap(), w, cx)
            })??;
            pump(cx)
        };
    let capture =
        |cx: &mut HeadlessAppContext, state: &str| -> Result<(), Box<dyn std::error::Error>> {
            cx.capture_screenshot(window.into())?
                .save(output.join(format!("native-{width}-{state}.png")))?;
            std::fs::write(
                output.join(format!("native-{width}-{state}.json")),
                serde_json::to_vec_pretty(&driver.snapshot(true))?,
            )?;
            Ok(())
        };
    let find = |prefix: &str, label: &str| {
        driver
            .snapshot(false)
            .elements
            .into_iter()
            .find(|e| e.id.starts_with(prefix) && e.label.contains(label))
    };
    pump(&mut cx)?;
    action(
        &mut cx,
        json!({"type":"scroll","target":{"element_id":"history-ledger"},"delta_x":0,"delta_y":10000}),
    )?;
    capture(&mut cx, "collapsed")?;
    if width <= 330. {
        let model = find("history-model", "gpt").unwrap().bounds;
        let tokens = find("history-tokens", "1.4").unwrap().bounds;
        let cache = find("history-cache", "50").unwrap().bounds;
        assert!(
            tokens.y > model.y
                && (tokens.y - cache.y).abs() < 1.
                && model.width > tokens.width * 1.9,
            "narrow overview must place model above tokens/cache"
        );
    }
    let group = find("history-record-", "读取 1 个文件").expect("group summary visible");
    let group_top = group.bounds.y;
    action(
        &mut cx,
        json!({"type":"click","target":{"element_id":group.id}}),
    )?;
    let expanded_group = find("history-record-", "读取 1 个文件").unwrap();
    assert!(
        (expanded_group.bounds.y - group_top).abs() < 1.,
        "group disclosure moved its reading anchor: {} -> {}",
        group_top,
        expanded_group.bounds.y
    );
    capture(&mut cx, "group")?;
    let command = find("history-record-", "pnpm test").expect("group contains the failed command");
    action(
        &mut cx,
        json!({"type":"click","target":{"element_id":command.id}}),
    )?;
    assert!(
        find("history-inline-detail-", "reader_anchor").is_some(),
        "tool output did not expand inline"
    );
    assert!(
        !driver
            .snapshot(false)
            .elements
            .iter()
            .any(|e| e.id == "history-detail-dialog"),
        "record opened a dialog"
    );
    capture(&mut cx, "details")?;
    action(&mut cx, json!({"type":"key","keystroke":"enter"}))?;
    capture(&mut cx, "keyboard-collapsed")?;
    assert!(
        find("history-inline-detail-", "reader_anchor").is_none(),
        "Enter did not collapse the focused command"
    );
    action(
        &mut cx,
        json!({"type":"click","target":{"element_id":expanded_group.id}}),
    )?;
    let input = find("history-record-", "检查 Session History").unwrap();
    action(
        &mut cx,
        json!({"type":"click","target":{"element_id":input.id}}),
    )?;
    assert!(
        find("history-inline-detail-", "完整执行结果").is_some(),
        "input text did not expand inline"
    );
    action(
        &mut cx,
        json!({"type":"click","target":{"element_id":input.id}}),
    )?;
    action(
        &mut cx,
        json!({"type":"scroll","target":{"element_id":"history-ledger"},"delta_x":0,"delta_y":-180}),
    )?;
    let disclosure = find("history-output-disclosure-", "展开更多").expect("Markdown disclosure");
    action(
        &mut cx,
        json!({"type":"click","target":{"element_id":disclosure.id}}),
    )?;
    capture(&mut cx, "markdown")?;
    action(
        &mut cx,
        json!({"type":"scroll","target":{"element_id":"history-ledger"},"delta_x":0,"delta_y":-360}),
    )?;
    capture(&mut cx, "code")?;
    assert!(
        find("history-follow-latest", "回到最新").is_some(),
        "disclosure did not lock follow"
    );
    action(
        &mut cx,
        json!({"type":"click","target":{"element_id":"history-follow-latest"}}),
    )?;
    assert!(
        find("history-follow-latest", "回到最新").is_none(),
        "follow button remained after activation"
    );
    assert!(
        find("history-record-", "思考").is_some(),
        "latest row is not visible"
    );
    capture(&mut cx, "latest")?;
    // At the tail, the follow control covers the active row. A click must not
    // also disclose that row and immediately disable following again.
    action(
        &mut cx,
        json!({"type":"scroll","target":{"element_id":"history-ledger"},"delta_y":1}),
    )?;
    let follow = find("history-follow-latest", "回到最新").unwrap();
    let active = find("history-record-", "思考").unwrap();
    assert!(
        follow.center.y >= active.bounds.y
            && follow.center.y < active.bounds.y + active.bounds.height,
        "fixture must exercise overlapping input targets"
    );
    action(
        &mut cx,
        json!({"type":"click","target":{"element_id":follow.id}}),
    )?;
    assert!(
        find("history-follow-latest", "回到最新").is_none(),
        "follow click reached the row underneath"
    );
    assert!(
        !driver
            .snapshot(false)
            .elements
            .iter()
            .any(|e| { e.visible && e.id.starts_with("history-inline-detail-") }),
        "follow click opened an underlying disclosure"
    );
    println!(
        "history parity {width}: inline, nested group, keyboard, stable anchor, Markdown, follow passed"
    );
    Ok(())
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    std::env::set_var("TZ", "UTC");
    for width in [900., 560., 320.] {
        run(width)?;
    }
    Ok(())
}
