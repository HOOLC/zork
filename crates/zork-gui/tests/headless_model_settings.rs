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
        let now = chrono::Utc::now().timestamp();
        fixture["profile"]["rateLimits"]["rateLimits"]["primary"]["resetsAt"] = json!(now + 3600);
        fixture["profile"]["rateLimits"]["rateLimits"]["secondary"]["resetsAt"] =
            json!(now + 3 * 86400);
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
            if id == "laptop" {
                zork_ui::device_name::DeviceStatus::Offline
            } else {
                zork_ui::device_name::DeviceStatus::Connected
            },
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
    // Provider marks are visible on the first settled frame, without another input.
    let snapshot = driver.snapshot(false);
    let pixels = cx.capture_screenshot(window.into())?;
    let marks: Vec<_> = snapshot
        .elements
        .iter()
        .filter(|e| e.id.starts_with("profile-provider-mark-") && e.visible)
        .collect();
    anyhow::ensure!(!marks.is_empty(), "No provider marks were mounted");
    for mark in marks {
        let b = &mark.visible_bounds;
        let scale = snapshot.scale_factor;
        let dark = (b.y * scale)..((b.y + b.height) * scale);
        let mut ink = 0;
        for y in dark.start as u32..dark.end as u32 {
            for x in (b.x * scale) as u32..((b.x + b.width) * scale) as u32 {
                let p = pixels.get_pixel(x, y);
                if p[0] < 80 && p[1] < 80 && p[2] < 80 {
                    ink += 1;
                }
            }
        }
        anyhow::ensure!(ink > 8, "Provider mark is blank: {}", mark.id);
    }

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
        let device_name = snapshot
            .elements
            .iter()
            .find(|e| e.id == format!("profile-device-{device}-fixture") && e.visible)
            .ok_or_else(|| anyhow::anyhow!("Missing {device} label inside Profile card"))?;
        anyhow::ensure!(
            device_name.label.contains(device)
                && device_name.label.contains(if device == "laptop" {
                    "离线"
                } else {
                    "已连接"
                })
                && device_name.bounds.x >= connection.bounds.x
                && device_name.bounds.y >= connection.bounds.y
                && device_name.bounds.x + device_name.bounds.width
                    <= connection.bounds.x + connection.bounds.width
                && device_name.bounds.y + device_name.bounds.height
                    <= connection.bounds.y + connection.bounds.height,
            "Device label is outside its Profile card: {device_name:?}"
        );
        // Billing is secondary: it stays in the row's accessible name only.
        anyhow::ensure!(
            snapshot.elements.iter().any(|e| {
                e.id == format!("profile-name-{device}-fixture")
                    && e.label.contains("OpenAI · ChatGPT 订阅 (Codex)")
            }),
            "Connection billing disappeared"
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
        for index in 0..2 {
            anyhow::ensure!(
                snapshot.elements.iter().any(|e| {
                    e.id == format!("profile-quota-window-{device}-fixture-{index}") && e.visible
                }),
                "{device} quota bar {index} is missing"
            );
        }
        anyhow::ensure!(
            snapshot.elements.iter().any(|e| {
                e.id == format!("profile-model-count-{device}-fixture") && e.label == "2 个模型"
            }),
            "Connection model count disappeared"
        );
        anyhow::ensure!(
            !snapshot
                .elements
                .iter()
                .any(|e| { e.id == format!("profile-verification-{device}-fixture") && e.visible }),
            "A verified connection must not show a status pill"
        );
    }
    cx.capture_screenshot(window.into())?
        .save(output.join("provider-profiles.png"))?;
    cx.update_window(window.into(), |_, w, cx| {
        driver.dispatch(
            serde_json::from_value(json!({
                "type":"move","target":{"element_id":"profile-quota-window-desktop-fixture-0"}
            }))?,
            w,
            cx,
        )
    })??;
    draw(&mut cx)?;
    let hover = driver.snapshot(false);
    anyhow::ensure!(
        hover.elements.iter().any(|e| {
            e.id == "control-hint-profile-quota-reset-desktop-fixture-0"
                && e.visible
                && e.label.contains("后重置")
        }),
        "5H quota hover did not show the reset time: {:?}",
        hover
            .elements
            .iter()
            .filter(|e| e.id.contains("quota") || e.id.contains("hint"))
            .map(|e| (&e.id, &e.label, e.visible))
            .collect::<Vec<_>>()
    );
    cx.capture_screenshot(window.into())?
        .save(output.join("provider-quota-tooltip.png"))?;
    cx.update_window(window.into(), |_, w, cx| {
        driver.dispatch(
            serde_json::from_value(json!({
                "type":"move","target":{"element_id":"model-provider-openai"}
            }))?,
            w,
            cx,
        )
    })??;
    draw(&mut cx)?;
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
            .any(|e| e.id == "profile-target-select" && e.visible),
        "Adding a connection did not offer the save-to device choice"
    );
    cx.capture_screenshot(window.into())?
        .save(output.join("add-connection-step-1.png"))?;
    click("profile-target-select", &mut cx)?;
    click("profile-target-1", &mut cx)?;
    anyhow::ensure!(
        driver
            .snapshot(false)
            .elements
            .iter()
            .any(|e| e.id == "profile-close-form" && e.visible),
        "Choosing another device did not keep the connection dialog open"
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
        "profile-device-desktop-fixture",
        "profile-device-laptop-fixture",
        "profile-quota-window-desktop-fixture-0",
        "profile-quota-window-laptop-fixture-1",
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

    // The same account on both devices is one card naming both devices; the
    // fresher successful sample (laptop) represents its quota.
    // Both stations report the same account identity for their copy.
    let mut merged_sources = vec![];
    for (index, (id, name, _, status)) in sources.iter().enumerate() {
        let mut fixture = zork_ui::stories::page_fixture();
        fixture["profile"]["account_key"] = json!("openai:a:0123456789abcdef");
        fixture["profile"]["checkedAt"] = json!(if index == 0 {
            "2026-09-26T00:00:00Z"
        } else {
            "2026-09-26T01:00:00Z"
        });
        let client = Arc::new(StationClient::fixture(fixture, provider_catalog.clone()));
        let profiles = Profiles::new(client);
        profiles.seed(ProfileData {
            providers: Arc::new(provider_catalog["providers"].as_array().unwrap().clone()),
            ..Default::default()
        });
        merged_sources.push((id.clone(), name.clone(), profiles, status.clone()));
    }
    view.update(&mut cx, |v, cx| v.set_sources(merged_sources, cx));
    draw(&mut cx)?;
    let snapshot = driver.snapshot(false);
    let cards: Vec<_> = snapshot
        .elements
        .iter()
        .filter(|e| e.id.starts_with("profile-detail-") && e.visible)
        .map(|e| e.id.clone())
        .collect();
    anyhow::ensure!(
        cards == ["profile-detail-account-desktop-fixture"],
        "Same account did not merge into one card: {cards:?}"
    );
    anyhow::ensure!(
        snapshot.elements.iter().any(|e| {
            e.id == "profile-devices-account-desktop-fixture"
                && e.visible
                && e.label == "desktop、laptop"
        }) && snapshot
            .elements
            .iter()
            .any(|e| e.id == "model-provider-openai" && e.label == "OpenAI · 1 个 Profile"),
        "Merged card lost its devices or count"
    );
    cx.capture_screenshot(window.into())?
        .save(output.join("same-account-merged.png"))?;
    // The fresher laptop sample stands for the account.
    anyhow::ensure!(
        view.read_with(&cx, |v, cx| v.headless_card_source(cx)) == Some("laptop".to_owned()),
        "The freshest successful sample did not represent the account"
    );
    click("profile-detail-account-desktop-fixture", &mut cx)?;
    cx.update_window(window.into(), |_, w, cx| {
        driver.dispatch(
            serde_json::from_value(json!({
                "type":"move","target":{"element_id":"model-account-name"}
            }))?,
            w,
            cx,
        )
    })??;
    draw(&mut cx)?;
    let snapshot = driver.snapshot(false);
    anyhow::ensure!(
        snapshot
            .elements
            .iter()
            .any(|e| e.id == "model-account-meta" && e.label.contains("desktop、laptop"))
            && ["desktop", "laptop"].iter().all(|device| {
                snapshot
                    .elements
                    .iter()
                    .any(|e| e.id == format!("profile-detail-{device}-fixture") && e.visible)
            }),
        "Opening the account did not list each device source"
    );
    cx.capture_screenshot(window.into())?
        .save(output.join("same-account-sources.png"))?;
    click("profile-detail-desktop-fixture", &mut cx)?;
    let selected = view.read_with(&cx, |v, cx| v.headless_selection(cx));
    anyhow::ensure!(
        selected["device"] == "desktop" && selected["editor"]["detail"] == "fixture",
        "A device source did not open its own connection: {selected}"
    );
    click("profile-detail-dialog-close", &mut cx)?;
    anyhow::ensure!(
        driver
            .snapshot(false)
            .elements
            .iter()
            .any(|e| e.id == "model-account-back" && e.visible),
        "Leaving a device source did not return to the account"
    );
    click("model-account-back", &mut cx)?;
    anyhow::ensure!(
        driver
            .snapshot(false)
            .elements
            .iter()
            .any(|e| e.id == "profile-detail-account-desktop-fixture" && e.visible),
        "Back from the account did not return to the list"
    );
    println!("PASS model settings: provider → Profile cards, quota/status, no model list or grouping switch, dialogs, multiple devices and profiles, source removal, same account merged across devices");
    Ok(())
}
