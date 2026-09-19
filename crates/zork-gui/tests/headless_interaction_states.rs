//! Input-driven checks for shared action feedback, on both application surfaces.
use gpui::{
    px, AppContext, HeadlessAppContext, MouseButton, MouseDownEvent, MouseUpEvent, PlatformInput,
};
use serde_json::json;
use std::{sync::Arc, time::Duration};
use zork_gui::{
    assets::EmbeddedAssets,
    automation::{AutomationRoot, HeadlessAutomation},
    desktop::stories::{self, StoryHost},
};
fn main() -> anyhow::Result<()> {
    let output = std::env::var_os("ZORK_INTERACTION_OUTPUT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../artifacts/interaction-preview/interaction-checks")
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
    let story = stories::catalog()
        .into_iter()
        .find(|s| s.id == "interaction-overview")
        .unwrap();
    let window = cx.open_window(gpui::size(px(800.), px(620.)), |_, cx| {
        let host = cx.new(|cx| StoryHost::new(story, cx));
        cx.new(|_| AutomationRoot::new(host))
    })?;
    let pump = |cx: &mut HeadlessAppContext| -> anyhow::Result<()> {
        for _ in 0..12 {
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
    for surface in ["canvas", "sidebar"] {
        for role in ["chat", "browser", "settings"] {
            let id = format!("interaction-overview-{surface}-{role}");
            let initial = driver
                .snapshot(false)
                .elements
                .into_iter()
                .find(|e| e.id == id)
                .unwrap();
            anyhow::ensure!(
                initial.bounds.width == 28. && initial.bounds.height == 28.,
                "Compact action geometry diverged"
            );
            let action = serde_json::from_value(json!({"type":"move","target":{"element_id":id}}))?;
            cx.update_window(window.into(), |_, w, cx| driver.dispatch(action, w, cx))??;
            pump(&mut cx)?;
            let sample = |cx: &mut HeadlessAppContext| -> anyhow::Result<[u8; 4]> {
                let scale = driver.snapshot(false).scale_factor;
                let image = cx.capture_screenshot(window.into())?;
                Ok(image
                    .get_pixel(
                        ((initial.bounds.x + 3.) * scale) as u32,
                        ((initial.bounds.y + 14.) * scale) as u32,
                    )
                    .0)
            };
            let hover = sample(&mut cx)?;
            anyhow::ensure!(hover[..3] == [239, 238, 234], "{id}: hover is {hover:?}");
            let position = gpui::point(
                px(initial.bounds.x as f32 + 14.),
                px(initial.bounds.y as f32 + 14.),
            );
            cx.update_window(window.into(), |_, w, cx| {
                w.dispatch_event(
                    PlatformInput::MouseDown(MouseDownEvent {
                        button: MouseButton::Left,
                        position,
                        modifiers: Default::default(),
                        click_count: 1,
                        first_mouse: false,
                    }),
                    cx,
                )
            })?;
            pump(&mut cx)?;
            let pressed = sample(&mut cx)?;
            anyhow::ensure!(
                pressed[..3] == [234, 231, 225],
                "{id}: pressed is {pressed:?}"
            );
            cx.capture_screenshot(window.into())?
                .save(output.join(format!("{surface}-{role}-pressed.png")))?;
            cx.update_window(window.into(), |_, w, cx| {
                w.dispatch_event(
                    PlatformInput::MouseUp(MouseUpEvent {
                        button: MouseButton::Left,
                        position,
                        modifiers: Default::default(),
                        click_count: 1,
                    }),
                    cx,
                )
            })?;
            pump(&mut cx)?;
            let after = driver
                .snapshot(false)
                .elements
                .into_iter()
                .find(|e| e.id == id)
                .unwrap();
            anyhow::ensure!(
                initial.bounds == after.bounds,
                "Hover or pressed changed layout"
            );
        }
    }
    anyhow::ensure!(
        driver
            .snapshot(false)
            .elements
            .iter()
            .any(|e| e.label == "已触发 6 次操作"),
        "Action dispatch was lost or duplicated"
    );
    let edge_target = driver
        .snapshot(false)
        .elements
        .into_iter()
        .find(|element| element.id == "interaction-overview-canvas-chat")
        .unwrap();
    for y in [
        edge_target.bounds.y + 3.,
        edge_target.bounds.y + edge_target.bounds.height - 3.,
    ] {
        let action = serde_json::from_value(
            json!({"type":"click","target":{"x":edge_target.center.x,"y":y}}),
        )?;
        cx.update_window(window.into(), |_, w, cx| driver.dispatch(action, w, cx))??;
        pump(&mut cx)?;
    }
    anyhow::ensure!(
        driver
            .snapshot(false)
            .elements
            .iter()
            .any(|element| element.label == "已触发 8 次操作"),
        "the full control must respond outside the label's ink band"
    );
    cx.capture_screenshot(window.into())?
        .save(output.join("overview.png"))?;
    println!("PASS: six real controls, two surfaces, hover/pressed colors, stable bounds, one action per center/edge click");
    Ok(())
}
