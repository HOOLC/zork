//! Cross-device provider and profile navigation uses the real settings and editor views.
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
    for icon in ["icons/models.svg"] {
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
    let snapshot = driver.snapshot(false);
    anyhow::ensure!(
        snapshot.elements.iter().any(|e| {
            e.id == "model-provider-openai" && e.visible && e.label == "OpenAI · 2 个 Profile"
        }),
        "OpenAI provider group is missing"
    );
    anyhow::ensure!(
        !snapshot.elements.iter().any(|e| {
            e.id.starts_with("model-entry-")
                || e.id == "model-disabled-toggle"
                || e.id == "model-grouping-toggle"
        }),
        "A separate model list or grouping switch is still present"
    );
    for device in ["desktop", "laptop"] {
        let connection = snapshot
            .elements
            .iter()
            .find(|e| e.id == format!("profile-detail-{device}-fixture") && e.visible)
            .ok_or_else(|| anyhow::anyhow!("Missing {device} Profile card"))?;
        anyhow::ensure!(
            connection.label.contains(device),
            "Profile card lost its device identity"
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
    cx.capture_screenshot(window.into())?
        .save(output.join("provider-profiles.png"))?;
    click("profile-detail-desktop-fixture", &mut cx)?;
    anyhow::ensure!(
        driver
            .snapshot(false)
            .elements
            .iter()
            .any(|e| e.id == "profile-detail-dialog-close" && e.visible),
        "Profile card did not open its detail dialog"
    );
    let selected = view.read_with(&cx, |v, cx| v.headless_selection(cx));
    anyhow::ensure!(
        selected["device"] == "desktop" && selected["editor"]["detail"] == "fixture",
        "Wrong Profile detail/source: {selected}"
    );
    click("profile-detail-dialog-close", &mut cx)?;

    view.update(&mut cx, |v, cx| {
        anyhow::ensure!(v.begin_onboarding("desktop", cx), "Local editor missing");
        Ok::<_, anyhow::Error>(())
    })?;
    draw(&mut cx)?;
    anyhow::ensure!(
        !driver
            .snapshot(false)
            .elements
            .iter()
            .any(|e| { e.id == "profile-detail-laptop-fixture" && e.visible }),
        "Onboarding showed another device's Profile"
    );
    anyhow::ensure!(
        driver
            .snapshot(false)
            .elements
            .iter()
            .any(|e| e.id == "profile-close-form" && e.visible),
        "Onboarding did not open the connection editor"
    );
    click("profile-close-form", &mut cx)?;
    view.update(&mut cx, |v, cx| v.set_onboarding_local(None, cx));
    draw(&mut cx)?;

    click("models-add", &mut cx)?;
    anyhow::ensure!(
        driver
            .snapshot(false)
            .elements
            .iter()
            .any(|e| e.id == "model-add-device-laptop" && e.visible),
        "Adding a connection did not open the device chooser"
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

    cx.update_window(window.into(), |_, w, cx| {
        w.resize(size(px(600.), px(760.)));
        w.bounds_changed(cx);
    })?;
    draw(&mut cx)?;
    for id in [
        "models-add",
        "model-provider-openai",
        "profile-detail-desktop-fixture",
        "profile-detail-laptop-fixture",
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
        .save(output.join("provider-profiles-narrow.png"))?;

    view.update(&mut cx, |v, cx| v.set_sources(vec![sources[0].clone()], cx));
    draw(&mut cx)?;
    anyhow::ensure!(
        !driver
            .snapshot(false)
            .elements
            .iter()
            .any(|e| { e.id == "profile-detail-laptop-fixture" && e.visible }),
        "Removed device's Profile remains listed"
    );
    anyhow::ensure!(
        driver
            .snapshot(false)
            .elements
            .iter()
            .any(|e| { e.id == "model-provider-openai" && e.label == "OpenAI · 1 个 Profile" }),
        "Provider count did not follow source removal"
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
    click("profile-close-form", &mut cx)?;

    let mut state = (*sources[0].2.snapshot()).clone();
    let mut second = state.profiles[0].clone();
    second.profile_id = "team".into();
    second.name = Some("Team Profile".into());
    second.models.clear();
    Arc::make_mut(&mut state.profiles).push(second);
    sources[0].2.seed(state);
    draw(&mut cx)?;
    let snapshot = driver.snapshot(true);
    anyhow::ensure!(
        snapshot
            .elements
            .iter()
            .any(|e| { e.id == "model-provider-openai" && e.label == "OpenAI · 2 个 Profile" })
            && snapshot
                .elements
                .iter()
                .any(|e| { e.id == "profile-detail-desktop-team" && e.visible })
            && snapshot.elements.iter().any(|e| {
                e.id == "profile-model-count-desktop-team" && e.label == "待配置模型"
            }),
        "Same-provider or unconfigured Profile is missing"
    );
    anyhow::ensure!(
        !snapshot
            .elements
            .iter()
            .any(|e| e.id.starts_with("model-entry-")),
        "A separate model list reappeared"
    );
    cx.capture_screenshot(window.into())?
        .save(output.join("provider-multiple-profiles.png"))?;
    println!("PASS model settings: provider → Profile cards, quota/status, no model list or grouping switch, dialogs, multiple devices and profiles, source removal");
    Ok(())
}
