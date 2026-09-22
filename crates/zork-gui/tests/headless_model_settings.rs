//! Cross-device model navigation uses the real settings and profile editor views.
use gpui::{
    div, prelude::*, px, rgb, size, AppContext, Context, Entity, HeadlessAppContext, Window,
};
use serde_json::json;
use std::{sync::Arc, time::Duration};
use zork_gui::{
    api::{Profiles, StationClient},
    assets::EmbeddedAssets,
    automation::{AutomationRoot, HeadlessAutomation},
    desktop::HeadlessModelSettings,
};

struct SettingsShell(Entity<HeadlessModelSettings>);
impl Render for SettingsShell {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let p = zork_gui::design::ZORK_UI.palette;
        div()
            .size_full()
            .bg(rgb(p.canvas))
            .text_color(rgb(p.text))
            .text_size(px(13.))
            .font_family("Inter Variable")
            .child(zork_ui::controls::settings_content(self.0.clone()))
    }
}
fn main() -> anyhow::Result<()> {
    anyhow::ensure!(
        gpui::AssetSource::load(&EmbeddedAssets, "icons/grouping.svg")?.is_some(),
        "Grouping icon is not embedded"
    );
    let output =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../artifacts/model-settings");
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
    let mut sources = vec![];
    for id in ["desktop", "laptop"] {
        let mut fixture = zork_ui::stories::page_fixture();
        fixture["profile"]["models"] = json!([{
            "id": "shared-model", "enabled": true, "api": "openai-responses",
            "limits": {"context_window_tokens": 128000, "max_output_tokens": 8192}, "thinking": ["low"], "default_thinking": "low"
        }]);
        let client = Arc::new(StationClient::fixture(
            fixture,
            serde_json::from_str(include_str!("fixtures/provider_catalog.json"))?,
        ));
        sources.push((id.into(), id.into(), Profiles::new(client)));
    }
    let mut view = None;
    let window = cx.open_window(size(px(900.), px(760.)), |_, cx| {
        let settings = cx.new(|cx| {
            let mut v = HeadlessModelSettings::new(cx);
            v.set_sources(sources.clone(), cx);
            v
        });
        view = Some(settings.clone());
        let shell = cx.new(|_| SettingsShell(settings));
        cx.new(|_| AutomationRoot::new(shell))
    })?;
    let view = view.unwrap();
    let draw = |cx: &mut HeadlessAppContext| -> anyhow::Result<()> {
        for _ in 0..8 {
            cx.run_until_parked();
            cx.advance_clock(Duration::from_millis(16));
            cx.update_window(window.into(), |_, w, cx| w.draw(cx).clear(cx))?;
        }
        Ok(())
    };
    draw(&mut cx)?;
    let click = |id: &str, cx: &mut HeadlessAppContext| -> anyhow::Result<()> {
        cx.update_window(window.into(), |_, w, cx| {
            driver.dispatch(
                serde_json::from_value(json!({"type":"click","target":{"element_id":id}}))?,
                w,
                cx,
            )
        })??;
        draw(cx)
    };
    let rows = || {
        driver
            .snapshot(false)
            .elements
            .into_iter()
            .filter(|e| e.id.starts_with("model-entry-"))
            .collect::<Vec<_>>()
    };
    anyhow::ensure!(
        rows().len() == 2,
        "Expected models from both devices: {:?}",
        rows().iter().map(|e| &e.label).collect::<Vec<_>>()
    );
    view.update(&mut cx, |v, cx| {
        anyhow::ensure!(v.begin_onboarding("desktop", cx), "Local editor missing");
        Ok::<_, anyhow::Error>(())
    })?;
    draw(&mut cx)?;
    anyhow::ensure!(rows().len() == 1, "Onboarding showed another device");
    anyhow::ensure!(
        driver
            .snapshot(false)
            .elements
            .iter()
            .any(|e| e.id == "profile-close-form" && e.visible),
        "Onboarding did not open the real connection editor"
    );
    anyhow::ensure!(
        !driver
            .snapshot(false)
            .elements
            .iter()
            .any(|e| e.id.starts_with("model-add-device-")),
        "Onboarding offered a device chooser"
    );
    click("profile-close-form", &mut cx)?;
    anyhow::ensure!(
        !driver
            .snapshot(false)
            .elements
            .iter()
            .any(|e| e.id == "profile-close-form" && e.visible),
        "Connection editor remained visible after cancel"
    );
    view.update(&mut cx, |v, cx| v.set_onboarding_local(None, cx));
    draw(&mut cx)?;
    anyhow::ensure!(rows().len() == 2, "Normal settings lost a device");
    cx.capture_screenshot(window.into())?
        .save(output.join("by-provider.png"))?;
    for label in [
        "当前按供应商分组，点击切换为按模型分组",
        "当前按模型分组，点击切换为按供应商分组",
    ] {
        cx.update_window(window.into(), |_, w, cx| {
            driver.dispatch(
                serde_json::from_value(
                    json!({"type":"move", "target":{"element_id":"model-grouping-toggle"}}),
                )?,
                w,
                cx,
            )
        })??;
        draw(&mut cx)?;
        cx.advance_clock(Duration::from_millis(300));
        draw(&mut cx)?;
        let snapshot = driver.snapshot(false);
        let hint = snapshot
            .elements
            .iter()
            .find(|e| e.id == "control-hint-model-grouping-toggle" && e.visible)
            .ok_or_else(|| anyhow::anyhow!("Missing grouping tooltip: {label}"))?;
        anyhow::ensure!(hint.label == label, "Tooltip did not follow grouping state");
        anyhow::ensure!(
            hint.bounds.height <= 30.1 && hint.bounds == hint.visible_bounds,
            "Grouping tooltip must fit one line without clipping: {label}, {:?}",
            hint.bounds
        );
        anyhow::ensure!(
            snapshot
                .elements
                .iter()
                .filter(|e| e.id == "model-grouping-toggle")
                .count()
                == 1,
            "Grouping must use one control"
        );
        click("model-grouping-toggle", &mut cx)?;
    }
    // The two clicks above return to provider grouping; a third selects models.
    click("model-grouping-toggle", &mut cx)?;
    cx.capture_screenshot(window.into())?
        .save(output.join("grouping-tooltip.png"))?;
    anyhow::ensure!(rows().len() == 2, "Grouping merged distinct model sources");
    cx.capture_screenshot(window.into())?
        .save(output.join("by-model.png"))?;
    let laptop = rows()
        .into_iter()
        .find(|e| e.label.contains("laptop"))
        .unwrap();
    click(&laptop.id, &mut cx)?;
    let selected = view.read_with(&cx, |v, cx| v.headless_selection(cx));
    anyhow::ensure!(
        selected["device"] == "laptop"
            && selected["editor"]["model_id"] == "shared-model"
            && selected["editor"]["model_form"] == true,
        "Wrong model editor/source: {selected}"
    );
    cx.capture_screenshot(window.into())?
        .save(output.join("edit-model.png"))?;
    // Removing a source must remove its rows and an editor opened against it.
    view.update(&mut cx, |v, cx| v.set_sources(vec![sources[0].clone()], cx));
    draw(&mut cx)?;
    anyhow::ensure!(rows().len() == 1, "Removed source remains listed");
    anyhow::ensure!(
        view.read_with(&cx, |v, cx| v.headless_selection(cx))
            .is_null(),
        "Removed source retained its editor"
    );
    println!("PASS model settings: icon tooltips, both grouping modes, same model on two devices, matching editor, source removal");
    Ok(())
}
