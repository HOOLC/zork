//! Production form controls: failure recovery, selection, busy state and keyboard focus.
use gpui::{px, AppContext, HeadlessAppContext};
use serde_json::json;
use std::{sync::Arc, time::Duration};
use zork_gui::{
    assets::EmbeddedAssets,
    automation::{AutomationRoot, HeadlessAutomation},
    desktop::stories::{self, StoryHost},
};
fn main() -> anyhow::Result<()> {
    let output = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../artifacts/ui-unification/form-checks");
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
    let story = stories::catalog()
        .into_iter()
        .find(|s| s.id == "interaction-form")
        .unwrap();
    let window = cx.open_window(gpui::size(px(800.), px(760.)), |_, cx| {
        let host = cx.new(|cx| StoryHost::new(story, cx));
        cx.new(|_| AutomationRoot::new(host))
    })?;
    let pump = |cx: &mut HeadlessAppContext| -> anyhow::Result<()> {
        for _ in 0..4 {
            cx.advance_clock(Duration::from_millis(16));
            cx.run_until_parked();
            cx.update_window(window.into(), |_, w, cx| {
                w.simulate_next_frame(cx);
                w.draw(cx).clear(cx)
            })?;
        }
        Ok(())
    };
    let act = |cx: &mut HeadlessAppContext, value: serde_json::Value| -> anyhow::Result<()> {
        cx.update_window(window.into(), |_, w, cx| {
            driver.dispatch(serde_json::from_value(value).unwrap(), w, cx)
        })??;
        pump(cx)
    };
    let click = |id: &str| json!({"type":"click","target":{"element_id":id}});
    pump(&mut cx)?;
    let field_pixel = |cx: &mut HeadlessAppContext| -> anyhow::Result<[u8; 4]> {
        let snap = driver.snapshot(false);
        let e = snap.elements.iter().find(|e| e.id == "form-name").unwrap();
        let image = cx.capture_screenshot(window.into())?;
        Ok(image
            .get_pixel(
                ((e.bounds.x + e.bounds.width - 8.) * snap.scale_factor) as u32,
                ((e.bounds.y + e.bounds.height / 2.) * snap.scale_factor) as u32,
            )
            .0)
    };
    let original = field_pixel(&mut cx)?;
    act(&mut cx, click("form-name"))?;
    anyhow::ensure!(
        field_pixel(&mut cx)? == original,
        "Focus changed the normal field surface"
    );
    act(&mut cx, click("form-save"))?;
    if !driver
        .snapshot(false)
        .elements
        .iter()
        .any(|e| e.label == "请填写名称")
    {
        cx.capture_screenshot(window.into())?
            .save(output.join("validation-failure.png"))?;
        std::fs::write(
            output.join("validation-failure.json"),
            serde_json::to_vec_pretty(&driver.snapshot(false))?,
        )?;
    }
    anyhow::ensure!(
        driver
            .snapshot(false)
            .elements
            .iter()
            .any(|e| e.label == "请填写名称"),
        "Missing inline error"
    );
    let invalid = field_pixel(&mut cx)?;
    anyhow::ensure!(
        invalid[..3] == [255, 250, 250],
        "Invalid field lost its semantic surface: {invalid:?}"
    );
    cx.capture_screenshot(window.into())?
        .save(output.join("invalid-focused.png"))?;
    // Validation moved focus to the failed input: typing without a target must work.
    act(&mut cx, json!({"type":"type_text","text":"主力模型"}))?;
    act(&mut cx, click("form-connection"))?;
    act(&mut cx, click("form-option-1"))?;
    anyhow::ensure!(
        !driver
            .snapshot(false)
            .elements
            .iter()
            .any(|e| e.id == "form-connection-menu"),
        "Menu did not close after selection"
    );
    anyhow::ensure!(
        driver
            .snapshot(false)
            .elements
            .iter()
            .any(|e| e.id == "form-connection" && e.label == "API 连接"),
        "Wrong selected connection"
    );
    act(&mut cx, click("form-connection"))?;
    act(&mut cx, json!({"type":"key","keystroke":"escape"}))?;
    anyhow::ensure!(
        !driver
            .snapshot(false)
            .elements
            .iter()
            .any(|e| e.id == "form-connection-menu"),
        "Escape did not close the select"
    );
    // Escape restores focus to the select trigger; Enter should reopen it.
    act(&mut cx, json!({"type":"key","keystroke":"enter"}))?;
    anyhow::ensure!(
        driver
            .snapshot(false)
            .elements
            .iter()
            .any(|e| e.id == "form-connection-menu"),
        "Escape lost select trigger focus"
    );
    act(&mut cx, json!({"type":"key","keystroke":"escape"}))?;
    act(&mut cx, click("form-enabled"))?;
    anyhow::ensure!(
        driver
            .snapshot(false)
            .elements
            .iter()
            .any(|e| e.id == "form-enabled" && e.label.ends_with("关闭")),
        "Switch did not change"
    );
    act(&mut cx, json!({"type":"key","keystroke":"space"}))?;
    anyhow::ensure!(
        driver
            .snapshot(false)
            .elements
            .iter()
            .any(|e| e.id == "form-enabled" && e.label.ends_with("开启")),
        "Space did not activate the focused switch"
    );
    let before = driver
        .snapshot(false)
        .elements
        .into_iter()
        .find(|e| e.id == "form-save")
        .unwrap();
    act(&mut cx, click("form-save"))?;
    let busy = driver
        .snapshot(false)
        .elements
        .into_iter()
        .find(|e| e.id == "form-save")
        .unwrap();
    anyhow::ensure!(!busy.enabled, "Busy action is still enabled");
    anyhow::ensure!(
        driver
            .snapshot(false)
            .elements
            .iter()
            .any(|e| e.id == "form-reset" && !e.enabled),
        "Busy reset action is still enabled"
    );
    anyhow::ensure!(
        before.bounds.width == busy.bounds.width,
        "Busy label changed action width"
    );
    let image = cx.capture_screenshot(window.into())?;
    let scale = driver.snapshot(false).scale_factor;
    let center = busy.bounds.center();
    let mut foreground_pixels = 0;
    for y in ((center.y - 6.) * scale) as u32..((center.y + 6.) * scale) as u32 {
        for x in ((center.x - 6.) * scale) as u32..((center.x + 6.) * scale) as u32 {
            let pixel = image.get_pixel(x, y);
            if pixel[0] > 220 && pixel[1] > 220 && pixel[2] > 220 {
                foreground_pixels += 1;
            }
        }
    }
    anyhow::ensure!(
        foreground_pixels > 4,
        "Busy action has no visible label or indicator"
    );
    image.save(output.join("saving.png"))?;
    cx.advance_clock(Duration::from_secs(1));
    pump(&mut cx)?;
    anyhow::ensure!(
        driver
            .snapshot(false)
            .elements
            .iter()
            .any(|e| e.label == "示例已保存"),
        "Validation lost entered text or pending state did not complete"
    );
    cx.capture_screenshot(window.into())?
        .save(output.join("saved.png"))?;
    println!("PASS: stable field focus, inline error/focus recovery, select/escape, switch, busy geometry and successful retry");
    Ok(())
}
