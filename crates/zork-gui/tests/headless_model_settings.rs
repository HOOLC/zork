//! Cross-device model navigation uses the real settings and profile editor views.
use gpui::{
    div, prelude::*, px, rgb, size, AppContext, Context, Entity, HeadlessAppContext, Window,
};
use serde_json::json;
use std::{sync::Arc, time::Duration};
use zork_gui::{
    api::{ProfileData, Profiles, StationClient},
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
    for icon in [
        "icons/models.svg",
        "icons/group-by-provider.svg",
        "icons/group-by-model.svg",
    ] {
        anyhow::ensure!(
            gpui::AssetSource::load(&EmbeddedAssets, icon)?.is_some(),
            "{icon} is not embedded"
        );
    }
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
    let provider_catalog: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/provider_catalog.json"))?;
    for id in ["desktop", "laptop"] {
        let mut fixture = zork_ui::stories::page_fixture();
        fixture["profile"]["models"] = json!([{
            "id": "shared-model", "enabled": true, "api": "openai-responses",
            "limits": {"context_window_tokens": 128000, "max_output_tokens": 8192}, "thinking": ["low"], "default_thinking": "low"
        }, {"id": "old-model", "enabled": false, "api": "openai-responses"}]);
        let client = Arc::new(StationClient::fixture(fixture, provider_catalog.clone()));
        let profiles = Profiles::new(client);
        profiles.seed(ProfileData {
            providers: Arc::new(provider_catalog["providers"].as_array().unwrap().clone()),
            ..Default::default()
        });
        sources.push((
            id.into(),
            id.into(),
            profiles,
            zork_ui::device_name::DeviceStatus::Connected,
        ));
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
    let all_rows = || {
        driver
            .snapshot(true)
            .elements
            .into_iter()
            .filter(|e| e.id.starts_with("model-entry-"))
            .collect::<Vec<_>>()
    };
    let snapshot = driver.snapshot(false);
    for device in ["desktop", "laptop"] {
        let connection = snapshot
            .elements
            .iter()
            .find(|e| e.id == format!("profile-detail-{device}-fixture") && e.visible)
            .ok_or_else(|| anyhow::anyhow!("Missing {device} connection overview"))?;
        anyhow::ensure!(
            connection.label.contains(device),
            "Connection overview lost its device identity"
        );
        let billing = snapshot
            .elements
            .iter()
            .find(|e| e.id == format!("profile-billing-{device}-fixture"))
            .ok_or_else(|| anyhow::anyhow!("Missing {device} billing"))?;
        anyhow::ensure!(
            billing.label == "OpenAI · ChatGPT 订阅 (Codex)",
            "Connection billing disappeared: {}",
            billing.label
        );
        let quota = snapshot
            .elements
            .iter()
            .find(|e| e.id == format!("profile-quota-summary-{device}-fixture"))
            .ok_or_else(|| anyhow::anyhow!("Missing {device} quota"))?;
        anyhow::ensure!(
            quota.label.contains("5小时")
                && quota.label.contains("7天")
                && quota.label.matches("剩余").count() == 2,
            "Connection quota disappeared: {}",
            quota.label
        );
        anyhow::ensure!(
            snapshot.elements.iter().any(|e| {
                e.id == format!("profile-model-count-{device}-fixture") && e.label == "2 个模型"
            }),
            "Connection model count disappeared"
        );
        anyhow::ensure!(
            snapshot.elements.iter().any(|e| {
                e.id == format!("profile-verification-{device}-fixture") && e.label == "已验证"
            }),
            "Connection verification disappeared"
        );
    }
    anyhow::ensure!(
        rows().len() == 2,
        "Expected models from both devices: {:?}",
        rows().iter().map(|e| &e.label).collect::<Vec<_>>()
    );
    click("profile-detail-desktop-fixture", &mut cx)?;
    anyhow::ensure!(
        driver
            .snapshot(false)
            .elements
            .iter()
            .any(|e| e.id == "profile-detail-dialog-close" && e.visible),
        "Connection overview did not open the existing detail dialog"
    );
    click("profile-detail-dialog-close", &mut cx)?;
    view.update(&mut cx, |v, cx| {
        anyhow::ensure!(v.begin_onboarding("desktop", cx), "Local editor missing");
        Ok::<_, anyhow::Error>(())
    })?;
    draw(&mut cx)?;
    anyhow::ensure!(rows().len() == 1, "Onboarding showed another device");
    anyhow::ensure!(
        !driver
            .snapshot(false)
            .elements
            .iter()
            .any(|e| e.id == "profile-detail-laptop-fixture" && e.visible),
        "Onboarding showed another device's connection"
    );
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
    anyhow::ensure!(
        driver
            .snapshot(false)
            .elements
            .iter()
            .any(|e| e.id == "model-disabled-toggle"),
        "Disabled models need a collapsed entry"
    );
    cx.capture_screenshot(window.into())?
        .save(output.join("by-provider.png"))?;
    click("model-disabled-toggle", &mut cx)?;
    anyhow::ensure!(
        all_rows().len() == 4,
        "Disabled models did not expand: {} rows",
        all_rows().len()
    );
    click("model-disabled-toggle", &mut cx)?;
    anyhow::ensure!(
        all_rows().len() == 2,
        "Disabled models did not collapse: {} rows",
        all_rows().len()
    );
    click("models-add", &mut cx)?;
    anyhow::ensure!(
        driver
            .snapshot(false)
            .elements
            .iter()
            .any(|e| e.id == "model-add-device-laptop" && e.visible),
        "Adding a connection did not open the device dialog"
    );
    cx.capture_screenshot(window.into())?
        .save(output.join("add-device.png"))?;
    click("model-add-device-laptop", &mut cx)?;
    anyhow::ensure!(
        driver
            .snapshot(false)
            .elements
            .iter()
            .any(|e| e.id == "profile-close-form" && e.visible),
        "Selecting a device did not open the connection dialog"
    );
    click("profile-close-form", &mut cx)?;
    for label in [
        "按供应商分组 · 点击切换为按模型分组",
        "按模型分组 · 点击切换为按供应商分组",
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
    cx.update_window(window.into(), |_, w, cx| {
        w.resize(size(px(600.), px(760.)));
        w.bounds_changed(cx);
    })?;
    draw(&mut cx)?;
    anyhow::ensure!(rows().len() == 2, "Narrow layout lost a model source");
    for device in ["desktop", "laptop"] {
        let connection = driver
            .snapshot(false)
            .elements
            .into_iter()
            .find(|e| e.id == format!("profile-detail-{device}-fixture"))
            .ok_or_else(|| anyhow::anyhow!("Narrow layout lost {device} connection"))?;
        anyhow::ensure!(
            connection.visible
                && connection.bounds.x >= 0.
                && connection.bounds.x + connection.bounds.width <= 600.,
            "Narrow layout clipped {device} connection: {:?}",
            connection.bounds
        );
    }
    for row in rows() {
        anyhow::ensure!(
            row.visible && row.bounds.x >= 0. && row.bounds.x + row.bounds.width <= 600.,
            "Narrow layout clipped a model row: {:?}",
            row.bounds
        );
    }
    for id in [
        "models-add",
        "model-grouping-toggle",
        "model-disabled-toggle",
    ] {
        let element = driver
            .snapshot(false)
            .elements
            .into_iter()
            .find(|e| e.id == id)
            .ok_or_else(|| anyhow::anyhow!("Narrow layout lost {id}"))?;
        anyhow::ensure!(
            element.visible
                && element.bounds.x >= 0.
                && element.bounds.x + element.bounds.width <= 600.,
            "Narrow layout clipped {id}: {:?}",
            element.bounds
        );
    }
    cx.capture_screenshot(window.into())?
        .save(output.join("by-model-narrow.png"))?;
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
    click("models-add", &mut cx)?;
    anyhow::ensure!(
        driver
            .snapshot(false)
            .elements
            .iter()
            .any(|e| e.id == "profile-close-form" && e.visible),
        "Single-device add did not open the connection dialog"
    );
    anyhow::ensure!(
        !driver
            .snapshot(false)
            .elements
            .iter()
            .any(|e| e.id.starts_with("model-add-device-") && e.visible),
        "Single-device add showed an unnecessary chooser"
    );
    println!("PASS model settings: add dialog, disabled disclosure, icon tooltips, both grouping modes, same model on two devices, matching editor, source removal");
    Ok(())
}
