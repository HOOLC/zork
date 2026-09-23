//! Real navigation row input and forced-draw CPU measurements.
use anyhow::Context as _;
use gpui::{
    px, AppContext, HeadlessAppContext, MouseButton, MouseDownEvent, MouseUpEvent, PlatformInput,
};
use serde_json::json;
use std::{
    cell::RefCell,
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};
use zork_gui::{
    assets::EmbeddedAssets,
    automation::{AutomationRoot, HeadlessAutomation},
    desktop::stories::{self, StoryHost},
};
#[path = "support/collapse.rs"]
mod collapse;
#[path = "support/sliding.rs"]
mod sliding;

fn main() -> anyhow::Result<()> {
    let output = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(
            std::env::var("ZORK_NAVIGATION_OUTPUT")
                .unwrap_or_else(|_| "artifacts/ui-unification/navigation-checks".into()),
        );
    std::fs::create_dir_all(&output)?;
    if std::env::args().any(|arg| arg == "--slide-motion" || arg == "--slide-baseline") {
        return sliding::run(
            &output,
            std::env::args().any(|arg| arg == "--slide-baseline"),
        );
    }
    if std::env::args().any(|arg| arg == "--settings-tabs") {
        return settings_sidebar_checks(&output);
    }
    if std::env::args().any(|arg| arg == "--settings-process") {
        return settings_sidebar_process(&output);
    }
    if std::env::args().any(|arg| arg == "--collapse") {
        return collapse::run(&output);
    }
    if std::env::args().any(|arg| arg == "--chat-hover") {
        chat_hover_checks()?;
        return composer_preview_checks();
    }
    let mut reports = Vec::new();
    for width in [400., 800.] {
        for selected in [false, true] {
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
            let id = if selected {
                "navigation-selected"
            } else {
                "navigation-default"
            };
            let story = stories::catalog().into_iter().find(|s| s.id == id).unwrap();
            let mut host_entity = None;
            let window = cx.open_window(gpui::size(px(width), px(360.)), |_, cx| {
                let host = cx.new(|cx| StoryHost::new(story, cx));
                host_entity = Some(host.clone());
                cx.new(|_| AutomationRoot::new(host))
            })?;
            let host = host_entity.unwrap();
            let pump = |cx: &mut HeadlessAppContext| -> anyhow::Result<()> {
                for _ in 0..4 {
                    cx.advance_clock(Duration::from_millis(16));
                    cx.run_until_parked();
                    cx.update_window(window.into(), |_, w, cx| {
                        w.simulate_next_frame(cx);
                    })?;
                }
                Ok(())
            };
            let act = |cx: &mut HeadlessAppContext, value| -> anyhow::Result<()> {
                cx.update_window(window.into(), |_, w, cx| {
                    driver.dispatch(serde_json::from_value(value).unwrap(), w, cx)
                })??;
                pump(cx)
            };
            pump(&mut cx)?;
            let bounds = driver
                .snapshot(false)
                .elements
                .into_iter()
                .find(|e| e.id == "story-nav")
                .unwrap()
                .bounds;
            anyhow::ensure!(
                bounds.width == 240. && bounds.height == 32.,
                "navigation geometry changed: {bounds:?}"
            );
            let pixel = |cx: &mut HeadlessAppContext| -> anyhow::Result<[u8; 4]> {
                let scale = driver.snapshot(false).scale_factor;
                Ok(cx
                    .capture_screenshot(window.into())?
                    .get_pixel(
                        ((bounds.x + bounds.width - 12.) * scale) as u32,
                        ((bounds.y + 16.) * scale) as u32,
                    )
                    .0)
            };
            let normal = pixel(&mut cx)?;
            let marker = |cx: &mut HeadlessAppContext| -> anyhow::Result<[u8; 4]> {
                let scale = driver.snapshot(false).scale_factor;
                Ok(cx
                    .capture_screenshot(window.into())?
                    .get_pixel(
                        ((bounds.x + 3.) * scale) as u32,
                        ((bounds.y + 16.) * scale) as u32,
                    )
                    .0)
            };
            let active_color = zork_ui::design::BRAND_ACCENT.to_be_bytes()[1..].to_vec();
            let image = cx.capture_screenshot(window.into())?;
            let scale = driver.snapshot(false).scale_factor;
            let background = image
                .get_pixel(
                    ((bounds.x + bounds.width - 12.) * scale) as u32,
                    ((bounds.y + bounds.height + 4.) * scale) as u32,
                )
                .0;
            anyhow::ensure!(normal == background, "active tab still has a selected fill");
            anyhow::ensure!(
                &marker(&mut cx)?[..3]
                    == if selected {
                        active_color.as_slice()
                    } else {
                        &normal[..3]
                    },
                "active marker missing or shown on inactive tab"
            );
            image.save(output.join(format!("{width}-{id}-default.png")))?;
            act(&mut cx, json!({"type":"key","keystroke":"tab"}))?;
            let scale = driver.snapshot(false).scale_factor;
            let focused = cx.capture_screenshot(window.into())?;
            let border = focused
                .get_pixel((bounds.x * scale) as u32, ((bounds.y + 16.) * scale) as u32)
                .0;
            anyhow::ensure!(
                border[..3] == [100, 105, 112],
                "{id}: missing focus border {border:?}"
            );
            anyhow::ensure!(
                pixel(&mut cx)? == normal,
                "focus replaced selection surface"
            );
            focused.save(output.join(format!("{width}-{id}-focus.png")))?;
            act(
                &mut cx,
                json!({"type":"move","target":{"element_id":"story-nav"}}),
            )?;
            let hover = pixel(&mut cx)?;
            let position = gpui::point(px((bounds.x + 120.) as f32), px((bounds.y + 16.) as f32));
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
            let pressed = pixel(&mut cx)?;
            let expected_hover = [239, 238, 234];
            let expected_pressed = [234, 231, 225];
            if selected {
                anyhow::ensure!(
                    marker(&mut cx)?[..3] == active_color,
                    "pressed feedback covered the active marker"
                );
            }
            anyhow::ensure!(hover[..3] == expected_hover, "{id}: hover {hover:?}");
            anyhow::ensure!(
                pressed[..3] == expected_pressed,
                "{id}: pressed {pressed:?}"
            );

            cx.capture_screenshot(window.into())?
                .save(output.join(format!("{width}-{id}-pressed.png")))?;
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
            anyhow::ensure!(
                driver
                    .snapshot(false)
                    .elements
                    .iter()
                    .find(|e| e.id == "story-nav")
                    .unwrap()
                    .bounds
                    == bounds,
                "input changed row bounds"
            );
            anyhow::ensure!(
                pixel(&mut cx)?[..3] == expected_hover,
                "selection changed hover color"
            );
            anyhow::ensure!(
                cx.update(|cx| host.read(cx).inspect(cx))["state"]
                    == if selected { "default" } else { "selected" },
                "click did not toggle exactly once"
            );
            anyhow::ensure!(
                (marker(&mut cx)?[..3] == active_color) != selected,
                "active marker did not follow selection"
            );
            act(&mut cx, json!({"type":"key","keystroke":"enter"}))?;
            anyhow::ensure!(
                cx.update(|cx| host.read(cx).inspect(cx))["state"]
                    == if selected { "selected" } else { "default" },
                "Enter did not restore selection"
            );
            act(&mut cx, json!({"type":"key","keystroke":"space"}))?;
            anyhow::ensure!(
                cx.update(|cx| host.read(cx).inspect(cx))["state"]
                    == if selected { "default" } else { "selected" },
                "Space did not activate row"
            );
            act(&mut cx, json!({"type":"move","target":{"x":0,"y":0}}))?;
            act(
                &mut cx,
                json!({"type":"move","target":{"element_id":"story-nav"}}),
            )?;
            let mut samples = Vec::new();
            for i in 0..210 {
                let start = Instant::now();
                cx.update_window(window.into(), |_, w, cx| {
                    w.refresh();
                    w.draw(cx).clear(cx)
                })?;
                if i >= 30 {
                    samples.push(start.elapsed().as_secs_f64() * 1000.);
                }
            }
            samples.sort_by(f64::total_cmp);
            let p95 = samples[171];
            let p99 = samples[178];
            anyhow::ensure!(p95 < 8.33, "row CPU draw exceeds budget: {p95}");
            reports.push(json!({"width":width,"story":id,"bounds":bounds,"normal":normal,"hover":hover,"pressed":pressed,"frames":samples.len(),"p95_cpu_ms":p95,"p99_cpu_ms":p99}));
        }
    }
    std::fs::write(
        output.join("report.json"),
        serde_json::to_vec_pretty(&reports)?,
    )?;
    collapse::run(&output)?;
    sliding_hover_checks(&output)?;
    gap_surface_checks(&output)?;
    settings_sidebar_process(&output)?;
    chat_hover_checks()?;
    composer_preview_checks()?;
    println!("PASS navigation input, sliding hover, geometry and CPU draw budget");
    Ok(())
}

fn chat_hover_checks() -> anyhow::Result<()> {
    let chat = |id: &str| zork_client_core::state::NavigationChat {
        chat_id: id.into(),
        title: id.into(),
        updated_at: "2026-09-23T12:00:00Z".into(),
        in_preview: true,
        ..Default::default()
    };
    let device = zork_ui::chat_navigation::Device {
        id: "device-0".into(),
        name: "测试设备".into(),
        online: Some(true),
        status: zork_ui::device_name::DeviceStatus::Direct,
        direct: true,
        public: false,
        chats: Arc::new(vec![chat("chat-0"), chat("chat-1")]),
        selected_session: Some("chat-0".into()),
        chatting: true,
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
    let events = Rc::new(RefCell::new(Vec::new()));
    let observed = events.clone();
    let initial = device.clone();
    let mut navigation_entity = None;
    let window = cx.open_window(gpui::size(px(800.), px(600.)), |_, cx| {
        let navigation = cx.new(|cx| {
            let mut view = zork_ui::chat_navigation::Navigation::new(
                Default::default(),
                zork_ui::resources::Text(Rc::new(|key| {
                    zork_gui::i18n::Locale::ZhCn.text(key).into()
                })),
                cx,
            );
            view.set_data(vec![initial], Some("device-0".into()), false, 280., cx);
            view
        });
        navigation_entity = Some(navigation.clone());
        cx.subscribe(
            &navigation,
            move |_, event: &zork_ui::chat_navigation::Action, _| {
                if let zork_ui::chat_navigation::Action::Preview {
                    node,
                    session,
                    hovered,
                } = event
                {
                    observed
                        .borrow_mut()
                        .push((node.clone(), session.clone(), *hovered));
                }
            },
        )
        .detach();
        cx.new(|_| AutomationRoot::new(navigation))
    })?;
    let pump = |cx: &mut HeadlessAppContext| -> anyhow::Result<()> {
        for _ in 0..4 {
            cx.advance_clock(Duration::from_millis(16));
            cx.run_until_parked();
            cx.update_window(window.into(), |_, window, cx| {
                window.simulate_next_frame(cx)
            })?;
        }
        Ok(())
    };
    pump(&mut cx)?;
    anyhow::ensure!(
        driver
            .snapshot(false)
            .elements
            .iter()
            .any(|e| e.id == "chat-device-0-chat-1"),
        "Chat hover target was not rendered"
    );
    let move_to = |cx: &mut HeadlessAppContext, target| -> anyhow::Result<()> {
        cx.update_window(window.into(), |_, window, cx| {
            driver.dispatch(
                serde_json::from_value(json!({"type":"move","target":target})).unwrap(),
                window,
                cx,
            )
        })??;
        pump(cx)?;
        Ok(())
    };
    move_to(&mut cx, json!({"element_id":"chat-device-0-chat-1"}))?;
    let mut refreshed = device;
    refreshed.chats = Arc::new(refreshed.chats.as_ref().clone());
    navigation_entity
        .unwrap()
        .update(&mut cx, |navigation, cx| {
            navigation.set_data(vec![refreshed], Some("device-0".into()), false, 280., cx)
        });
    pump(&mut cx)?;
    move_to(&mut cx, json!({"x":700,"y":100}))?;
    anyhow::ensure!(
        events.borrow().as_slice()
            == &[
                ("device-0".into(), "chat-1".into(), true),
                ("device-0".into(), "chat-1".into(), false)
            ],
        "Chat hover did not enter and leave exactly once: {:?}",
        events.borrow()
    );
    println!("PASS Chat sidebar hover enters and leaves the preview target");
    Ok(())
}

fn composer_preview_checks() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let store = Arc::new(zork_gui::desktop::store::ClientStore::open(
        directory.path(),
    )?);
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
    let mut root = None;
    let window = cx.open_window(gpui::size(px(1000.), px(700.)), |_, cx| {
        let view = cx.new(|cx| {
            zork_gui::views::RootView::render_benchmark_fixture(false, store.clone(), cx)
        });
        root = Some(view.clone());
        cx.new(|_| AutomationRoot::new(view))
    })?;
    let pump = |cx: &mut HeadlessAppContext| -> anyhow::Result<()> {
        for _ in 0..4 {
            cx.advance_clock(Duration::from_millis(16));
            cx.run_until_parked();
            cx.update_window(window.into(), |_, window, cx| {
                window.simulate_next_frame(cx)
            })?;
        }
        Ok(())
    };
    let act = |cx: &mut HeadlessAppContext, value| -> anyhow::Result<()> {
        cx.update_window(window.into(), |_, window, cx| {
            driver.dispatch(serde_json::from_value(value).unwrap(), window, cx)
        })??;
        pump(cx)
    };
    let composer = |phase: &str| -> anyhow::Result<_> {
        let elements = driver.snapshot(false).elements;
        let input = elements
            .iter()
            .find(|element| element.id == "composer-input" && element.visible)
            .with_context(|| format!("{phase}: composer input disappeared"))?;
        let send = elements
            .iter()
            .find(|element| element.id == "send-button" && element.visible)
            .with_context(|| format!("{phase}: send button disappeared"))?;
        Ok((input.bounds, input.enabled, send.enabled))
    };
    cx.update_window(window.into(), |_, window, _| window.activate_window())?;
    let root = root.unwrap();
    let core = root.update(&mut cx, |view, cx| {
        let core = view.benchmark_core_device();
        core.edit_draft("render-fixture", "original draft".into())
            .unwrap();
        core.edit_draft("hovered", "hovered draft".into()).unwrap();
        core.edit_draft("another", "another draft".into()).unwrap();
        view.benchmark_restore_draft(cx);
        core
    });
    pump(&mut cx)?;
    let (original, _, _) = composer("original")?;
    act(
        &mut cx,
        json!({"type":"click","target":{"element_id":"composer-input"}}),
    )?;
    root.update(&mut cx, |view, cx| {
        assert!(view.benchmark_preview_session("hovered", cx));
    });
    pump(&mut cx)?;
    let (preview, editable, enabled) = composer("hovered")?;
    anyhow::ensure!(editable && enabled, "hovered Chat composer is not editable");
    anyhow::ensure!(
        (preview.y - original.y).abs() < 1. && (preview.height - original.height).abs() < 1.,
        "preview composer jumped: {original:?} -> {preview:?}"
    );
    let select_all = if cfg!(target_os = "macos") {
        "cmd-a"
    } else {
        "ctrl-a"
    };
    act(&mut cx, json!({"type":"key","keystroke":select_all}))?;
    act(&mut cx, json!({"type":"type_text","text":"edited hovered"}))?;
    anyhow::ensure!(
        core.draft("hovered").text == "edited hovered"
            && core.draft("render-fixture").text == "original draft",
        "preview input was saved to the wrong Chat"
    );
    root.update(&mut cx, |view, cx| {
        assert!(view.benchmark_preview_session("another", cx));
    });
    pump(&mut cx)?;
    let (_, editable, _) = composer("second preview")?;
    anyhow::ensure!(editable, "second Chat composer is not editable");
    act(&mut cx, json!({"type":"key","keystroke":select_all}))?;
    act(&mut cx, json!({"type":"type_text","text":"edited another"}))?;
    anyhow::ensure!(
        core.draft("another").text == "edited another"
            && core.draft("hovered").text == "edited hovered",
        "switching previews mixed two Chat drafts"
    );
    root.update(&mut cx, |view, cx| view.benchmark_restore_preview(cx));
    pump(&mut cx)?;
    composer("restored")?;
    anyhow::ensure!(
        core.draft("render-fixture").text == "original draft"
            && core.draft("hovered").text == "edited hovered"
            && core.draft("another").text == "edited another",
        "leaving preview changed a Chat draft"
    );
    root.update(&mut cx, |view, cx| {
        assert!(view.benchmark_preview_session("hovered", cx));
    });
    pump(&mut cx)?;
    act(&mut cx, json!({"type":"key","keystroke":"enter"}))?;
    anyhow::ensure!(
        store
            .outbox("mini1")?
            .iter()
            .any(|message| message.session_id == "hovered" && message.content == "edited hovered"),
        "sending from preview did not target the visible Chat"
    );
    root.update(&mut cx, |view, cx| view.benchmark_restore_preview(cx));
    pump(&mut cx)?;
    composer("restored after send")?;
    anyhow::ensure!(
        core.draft("render-fixture").text == "original draft",
        "sending from preview changed the original Chat draft"
    );
    println!(
        "PASS preview keeps the composer visible and routes edits and send to the visible Chat"
    );
    Ok(())
}

fn sliding_hover_checks(output: &std::path::Path) -> anyhow::Result<()> {
    let mut cx = HeadlessAppContext::with_platform(
        gpui_platform::current_platform(true).text_system(),
        Arc::new(EmbeddedAssets),
        gpui_platform::current_headless_renderer,
    );
    let driver = cx.update(|cx| {
        zork_gui::assets::init_fonts(cx);
        zork_gui::components::init(cx);
        cx.set_reduce_motion(false);
        HeadlessAutomation::install(cx)
    });
    let story = stories::catalog()
        .into_iter()
        .find(|s| s.id == "tooltip-hover")
        .unwrap();
    let window = cx.open_window(gpui::size(px(640.), px(400.)), |_, cx| {
        let host = cx.new(|cx| StoryHost::new(story, cx));
        cx.new(|_| AutomationRoot::new(host))
    })?;
    let pump = |cx: &mut HeadlessAppContext, frames: usize| -> anyhow::Result<()> {
        for _ in 0..frames {
            // Match the 120 Hz motion fixture: two 16 ms sleeps plus
            // rendering/readback can miss a short row transition entirely.
            let tick = Duration::from_nanos(8_333_333);
            std::thread::sleep(tick);
            cx.advance_clock(tick);
            cx.run_until_parked();
            cx.update_window(window.into(), |_, w, cx| {
                w.simulate_next_frame(cx);
            })?;
        }
        Ok(())
    };
    pump(&mut cx, 4)?;
    let elements = driver.snapshot(false).elements;
    let leader = elements
        .iter()
        .find(|e| e.id.ends_with("tooltip-leader-trigger"))
        .unwrap();
    let task = elements
        .iter()
        .find(|e| e.id.ends_with("tooltip-task-trigger"))
        .unwrap();
    anyhow::ensure!(
        (task.bounds.y - leader.bounds.y - leader.bounds.height - 2.).abs() < 0.1,
        "row gutter is not 2px"
    );
    let move_to = |cx: &mut HeadlessAppContext, x: f32, y: f32| -> anyhow::Result<()> {
        cx.update_window(window.into(), |_, w, cx| {
            driver.dispatch(
                serde_json::from_value(json!({"type":"move","target":{"x":x,"y":y}})).unwrap(),
                w,
                cx,
            )
        })??;
        Ok(())
    };
    let scale = driver.snapshot(false).scale_factor;
    let top = |cx: &mut HeadlessAppContext, name: &str| -> anyhow::Result<f32> {
        let image = cx.capture_screenshot(window.into())?;
        image.save(output.join(format!("sliding-{name}.png")))?;
        let x = ((leader.bounds.x + leader.bounds.width - 12.) * scale) as u32;
        let ys = ((leader.bounds.y * scale) as u32)
            ..(((task.bounds.y + task.bounds.height) * scale) as u32);
        let y = ys
            .into_iter()
            .find(|y| image.get_pixel(x, *y).0[..3] == [239, 238, 234]);
        Ok(y.map_or(-1., |y| y as f32 / scale))
    };
    let x = leader.bounds.x + 120.;
    move_to(&mut cx, x, leader.bounds.y + 16.)?;
    pump(&mut cx, 24)?;
    let first = top(&mut cx, "first")?;
    anyhow::ensure!(
        first >= leader.bounds.y && first <= leader.bounds.y + 3.,
        "first hover missing: {first}"
    );
    move_to(&mut cx, x, task.bounds.y + 16.)?;
    pump(&mut cx, 2)?;
    let moving = top(&mut cx, "moving")?;
    anyhow::ensure!(
        moving > first && moving < task.bounds.y,
        "hover teleported: {first} -> {moving}"
    );
    move_to(&mut cx, x, leader.bounds.y + 16.)?;
    pump(&mut cx, 1)?;
    let reversing = top(&mut cx, "reverse")?;
    anyhow::ensure!(
        reversing >= first && reversing < moving,
        "hover did not reverse continuously: {moving} -> {reversing}"
    );
    move_to(&mut cx, x, task.bounds.y + 16.)?;
    pump(&mut cx, 30)?;
    let settled = top(&mut cx, "settled")?;
    anyhow::ensure!(
        settled >= task.bounds.y && settled <= task.bounds.y + 3.,
        "hover did not settle: {settled}"
    );
    cx.update(|cx| cx.set_reduce_motion(true));
    move_to(&mut cx, x, leader.bounds.y + 16.)?;
    pump(&mut cx, 2)?;
    anyhow::ensure!(
        top(&mut cx, "reduced")? == first,
        "reduced motion did not reach target immediately"
    );
    move_to(&mut cx, 630., 390.)?;
    pump(&mut cx, 30)?;
    anyhow::ensure!(
        top(&mut cx, "leave")? == -1.,
        "hover remained after leaving"
    );
    let pending = cx.update_window(window.into(), |_, w, cx| w.simulate_next_frame(cx))?;
    anyhow::ensure!(
        pending == 0,
        "settled hover still schedules frames: {pending}"
    );
    std::fs::write(
        output.join("sliding.json"),
        serde_json::to_vec_pretty(
            &json!({"first":first,"moving":moving,"reversing":reversing,"settled":settled,"gutter_px":2,"idle_callbacks":pending}),
        )?,
    )?;
    Ok(())
}

fn settings_sidebar_process(output: &std::path::Path) -> anyhow::Result<()> {
    // A separate process supplies an isolated desktop store before GPUI starts;
    // no live device is restored and no local node is launched.
    let directory = tempfile::tempdir()?;
    let store = zork_gui::desktop::store::ClientStore::open(directory.path())?;
    store.set_local_node_enabled(false)?;
    store.save_node(&serde_json::from_value(json!({
        "id":"fixture", "name":"测试设备", "url":"http://127.0.0.1:9", "local":false
    }))?)?;
    drop(store);
    let services = directory.path().join("services.json");
    std::fs::write(
        &services,
        serde_json::to_vec(
            &json!({"relay_urls":["http://127.0.0.1:9"],"discovery_url":"http://127.0.0.1:9/pkarr"}),
        )?,
    )?;
    let status = std::process::Command::new(std::env::current_exe()?)
        .arg("--settings-tabs")
        .env("ZORK_CLIENT_DATA", directory.path())
        .env(
            "ZORK_GUI_PREFERENCES_PATH",
            directory.path().join("preferences.json"),
        )
        .env("ZORK_SERVICES_CONFIG", services)
        .env("ZORK_GUI_LOCALE", "zh-CN")
        .env("ZORK_GUI_TEST_REDUCE_MOTION", "0")
        .env("ZORK_NAVIGATION_OUTPUT", output)
        .status()?;
    anyhow::ensure!(
        status.success(),
        "actual settings sidebar regression failed"
    );
    Ok(())
}

fn settings_sidebar_checks(output: &std::path::Path) -> anyhow::Result<()> {
    for (width, height) in [(900., 600.), (1280., 800.)] {
        let mut cx = HeadlessAppContext::with_platform(
            gpui_platform::current_platform(true).text_system(),
            Arc::new(EmbeddedAssets),
            gpui_platform::current_headless_renderer,
        );
        let driver = cx.update(|cx| {
            zork_gui::assets::init_fonts(cx);
            zork_gui::components::init(cx);
            cx.set_reduce_motion(false);
            HeadlessAutomation::install(cx)
        });
        let window = cx.open_window(gpui::size(px(width), px(height)), |_, cx| {
            let desktop = cx.new(zork_gui::desktop::DesktopRoot::new);
            cx.new(|_| AutomationRoot::new(desktop))
        })?;
        let pump = |cx: &mut HeadlessAppContext, frames: usize| -> anyhow::Result<()> {
            for _ in 0..frames {
                std::thread::sleep(Duration::from_millis(16));
                cx.advance_clock(Duration::from_millis(16));
                cx.run_until_parked();
                cx.update_window(window.into(), |_, w, cx| {
                    w.simulate_next_frame(cx);
                })?;
            }
            Ok(())
        };
        let action = |cx: &mut HeadlessAppContext, value| -> anyhow::Result<()> {
            cx.update_window(window.into(), |_, w, cx| {
                driver.dispatch(serde_json::from_value(value).unwrap(), w, cx)
            })??;
            Ok(())
        };
        pump(&mut cx, 20)?;
        action(
            &mut cx,
            json!({"type":"click","target":{"element_id":"desktop-manage"}}),
        )?;
        pump(&mut cx, 20)?;
        let snapshot = driver.snapshot(false);
        anyhow::ensure!(
            !snapshot
                .elements
                .iter()
                .any(|element| element.id == "client_appearance"),
            "Removed appearance tab is still in the client settings sidebar"
        );
        let bounds = |id: &str| {
            snapshot
                .elements
                .iter()
                .find(|e| e.id == id)
                .unwrap_or_else(|| panic!("missing settings tab {id}"))
                .bounds
        };
        let notifications = bounds("client_notifications");
        let account = bounds("client_account");
        let data = bounds("client_data");
        let client_heading = bounds("client-settings-heading");
        let device_heading = bounds("device-settings-heading");
        let device_tab = bounds("settings-device-fixture");
        anyhow::ensure!(
            client_heading.height == device_heading.height
                && client_heading.width == device_heading.width
                && client_heading.x == device_heading.x,
            "settings section headings differ"
        );
        anyhow::ensure!(
            (notifications.y - client_heading.y - client_heading.height - 2.).abs() < 0.1
                && (device_tab.y - device_heading.y - device_heading.height - 2.).abs() < 0.1,
            "settings section heading/tab spacing differs"
        );
        for id in [
            "desktop-return",
            "client_notifications",
            "client_account",
            "client_data",
            "settings-device-fixture",
            "settings-add-device",
        ] {
            let row = bounds(id);
            anyhow::ensure!(
                row.height == 32. && row.width == notifications.width,
                "settings tab geometry differs: {id}: {row:?}"
            );
        }
        for (previous, next) in [(notifications, account), (account, data)] {
            anyhow::ensure!(
                (next.y - previous.y - previous.height - 2.).abs() < 0.1,
                "settings tab gap differs: {previous:?} -> {next:?}"
            );
        }
        let scale = snapshot.scale_factor;
        let color = zork_ui::design::BRAND_ACCENT.to_be_bytes()[1..].to_vec();
        let sample = |cx: &mut HeadlessAppContext, x: f32, y: f32| -> anyhow::Result<[u8; 4]> {
            Ok(cx
                .capture_screenshot(window.into())?
                .get_pixel((x * scale) as u32, (y * scale) as u32)
                .0)
        };
        action(
            &mut cx,
            json!({"type":"click","target":{"element_id":"client_notifications"}}),
        )?;
        action(
            &mut cx,
            json!({"type":"move","target":{"x":width-1.,"y":height-1.}}),
        )?;
        pump(&mut cx, 20)?;
        cx.capture_screenshot(window.into())?
            .save(output.join(format!("settings-{width}-active-notifications.png")))?;
        let notifications_marker = sample(&mut cx, notifications.x + 3., notifications.y + 16.)?;
        anyhow::ensure!(
            notifications_marker[..3] == color,
            "notifications active marker missing: {notifications_marker:?} at {notifications:?}"
        );
        anyhow::ensure!(
            sample(
                &mut cx,
                notifications.x + notifications.width - 12.,
                notifications.y + 16.
            )? == sample(&mut cx, account.x + account.width - 12., account.y + 16.)?,
            "selected settings tab retained a fill"
        );
        action(
            &mut cx,
            json!({"type":"click","target":{"element_id":"client_account"}}),
        )?;
        action(
            &mut cx,
            json!({"type":"move","target":{"x":width-1.,"y":height-1.}}),
        )?;
        pump(&mut cx, 20)?;
        anyhow::ensure!(
            sample(&mut cx, account.x + 3., account.y + 16.)?[..3] == color,
            "account selection did not move the marker"
        );
        anyhow::ensure!(
            sample(&mut cx, notifications.x + 3., notifications.y + 16.)?[..3] != color,
            "old active marker remained"
        );
        action(
            &mut cx,
            json!({"type":"move","target":{"x":width-1.,"y":height-1.}}),
        )?;
        pump(&mut cx, 20)?;
        let pending = cx.update_window(window.into(), |_, w, cx| w.simulate_next_frame(cx))?;
        anyhow::ensure!(pending == 0, "idle settings tab keeps drawing: {pending}");
        std::fs::write(
            output.join(format!("settings-{width}.json")),
            serde_json::to_vec_pretty(
                &json!({"width":width,"height":height,"notifications":notifications,"account":account,"client_heading":client_heading,"device_heading":device_heading,"idle_callbacks":pending}),
            )?,
        )?;
    }
    println!(
        "PASS client settings sidebar: shared geometry, independent active marker, selection, idle"
    );
    Ok(())
}

fn gap_surface_checks(output: &std::path::Path) -> anyhow::Result<()> {
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
        .find(|s| s.id == "navigation-gap")
        .unwrap();
    let window = cx.open_window(gpui::size(px(400.), px(360.)), |_, cx| {
        let host = cx.new(|cx| StoryHost::new(story, cx));
        cx.new(|_| AutomationRoot::new(host))
    })?;
    let pump = |cx: &mut HeadlessAppContext| -> anyhow::Result<()> {
        std::thread::sleep(Duration::from_millis(4));
        cx.advance_clock(Duration::from_millis(4));
        cx.run_until_parked();
        cx.update_window(window.into(), |_, w, cx| {
            w.simulate_next_frame(cx);
        })?;
        Ok(())
    };
    let action = |cx: &mut HeadlessAppContext, value| -> anyhow::Result<()> {
        cx.update_window(window.into(), |_, w, cx| {
            driver.dispatch(serde_json::from_value(value).unwrap(), w, cx)
        })??;
        Ok(())
    };
    pump(&mut cx)?;
    let snapshot = driver.snapshot(false);
    let first = snapshot
        .elements
        .iter()
        .find(|e| e.id == "gap-tab-first")
        .unwrap()
        .bounds;
    let second = snapshot
        .elements
        .iter()
        .find(|e| e.id == "gap-tab-second")
        .unwrap()
        .bounds;
    let scale = snapshot.scale_factor;
    let marker_x = ((first.x + 3.) * scale) as u32;
    let fill_x = ((first.x + first.width - 12.) * scale) as u32;
    let gap_y = (((first.y + first.height + second.y) / 2.) * scale) as u32;
    // A short marker can cross one scanline between readbacks. Observe the
    // entire gap, then verify that the captured surface has its full height.
    let gap_rows = (((first.y + first.height) * scale) as u32)..((second.y * scale) as u32);
    let active = zork_ui::design::BRAND_ACCENT.to_be_bytes()[1..].to_vec();
    action(
        &mut cx,
        json!({"type":"move","target":{"element_id":"gap-tab-first"}}),
    )?;
    pump(&mut cx)?;
    cx.update(|cx| cx.set_reduce_motion(false));
    action(
        &mut cx,
        json!({"type":"click","target":{"element_id":"gap-tab-second"}}),
    )?;
    // Hover follows pointer input, while the active marker follows selection.
    // Their first frames can differ; each surface must cross the gap intact.
    let mut active_crossing = None;
    let mut hover_crossing = None;
    let probe_started = Instant::now();
    let mut probes = Vec::new();
    for _ in 0..60 {
        pump(&mut cx)?;
        let image = cx.capture_screenshot(window.into())?;
        probes.push(json!({"ms":probe_started.elapsed().as_secs_f64()*1000.,
            "active_top":(0..image.height()).find(|y|image.get_pixel(marker_x,*y).0[..3]==active),
            "hover_top":(0..image.height()).find(|y|image.get_pixel(fill_x,*y).0[..3]==[239,238,234])}));
        if active_crossing.is_none()
            && gap_rows
                .clone()
                .any(|y| image.get_pixel(marker_x, y).0[..3] == active)
        {
            active_crossing = Some(image.clone());
        }
        if hover_crossing.is_none()
            && gap_rows
                .clone()
                .any(|y| image.get_pixel(fill_x, y).0[..3] == [239, 238, 234])
        {
            hover_crossing = Some(image);
        }
        if active_crossing.is_some() && hover_crossing.is_some() {
            break;
        }
    }
    std::fs::write(
        output.join("gap-probes.json"),
        serde_json::to_vec_pretty(
            &json!({"first":first,"second":second,"scale":scale,"gap_y":gap_y,"frames":probes}),
        )?,
    )?;
    let active_image = active_crossing.context("active disappeared in the 52px section gap")?;
    let hover_image = hover_crossing.context("hover disappeared in the 52px section gap")?;
    active_image.save(output.join("gap-active-crossing.png"))?;
    hover_image.save(output.join("gap-hover-crossing.png"))?;
    let ys = ((first.y * scale) as u32)..(((second.y + second.height) * scale) as u32);
    let marker_pixels = ys
        .clone()
        .filter(|y| active_image.get_pixel(marker_x, *y).0[..3] == active)
        .collect::<Vec<_>>();
    let hover_pixels = ys
        .clone()
        .filter(|y| hover_image.get_pixel(fill_x, *y).0[..3] == [239, 238, 234])
        .collect::<Vec<_>>();
    anyhow::ensure!(
        marker_pixels.len() >= (12. * scale) as usize
            && marker_pixels.windows(2).all(|p| p[1] == p[0] + 1),
        "active marker was clipped between tabs"
    );
    anyhow::ensure!(
        hover_pixels.len() >= (28. * scale) as usize
            && hover_pixels.windows(2).all(|p| p[1] == p[0] + 1),
        "hover surface was clipped between tabs"
    );
    let current = cx.capture_screenshot(window.into())?;
    let moving_top = ys
        .clone()
        .find(|y| current.get_pixel(marker_x, *y).0[..3] == active)
        .context("active marker disappeared before reversal")? as f32
        / scale;
    action(
        &mut cx,
        json!({"type":"click","target":{"element_id":"gap-tab-first"}}),
    )?;
    for _ in 0..3 {
        pump(&mut cx)?;
    }
    let reverse = cx.capture_screenshot(window.into())?;
    let reverse_top = ((first.y * scale) as u32..((second.y + second.height) * scale) as u32)
        .find(|y| reverse.get_pixel(marker_x, *y).0[..3] == active)
        .map(|y| y as f32 / scale)
        .unwrap_or(-1.);
    anyhow::ensure!(
        reverse_top >= first.y + 8. && reverse_top < moving_top,
        "active did not reverse from its displayed position: {moving_top} -> {reverse_top}"
    );
    reverse.save(output.join("gap-active-reverse.png"))?;
    cx.update(|cx| cx.set_reduce_motion(true));
    action(
        &mut cx,
        json!({"type":"click","target":{"element_id":"gap-tab-second"}}),
    )?;
    pump(&mut cx)?;
    let settled = cx.capture_screenshot(window.into())?;
    anyhow::ensure!(
        settled
            .get_pixel(marker_x, ((second.y + 16.) * scale) as u32)
            .0[..3]
            == active,
        "active did not snap with reduced motion"
    );
    settled.save(output.join("gap-active-settled.png"))?;
    std::fs::write(
        output.join("gap.json"),
        serde_json::to_vec_pretty(
            &json!({"gap_px":second.y-first.y-first.height,"moving_active_top":moving_top,"reversed_active_top":reverse_top,"active_pixel_height":marker_pixels.len() as f32/scale,"hover_pixel_height":hover_pixels.len() as f32/scale}),
        )?,
    )?;
    Ok(())
}
