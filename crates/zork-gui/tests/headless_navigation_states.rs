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

fn main() -> anyhow::Result<()> {
    let output = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(
            std::env::var("ZORK_NAVIGATION_OUTPUT")
                .unwrap_or_else(|_| "artifacts/ui-unification/navigation-checks".into()),
        );
    std::fs::create_dir_all(&output)?;
    if std::env::args().any(|arg| arg == "--settings-tabs") {
        return settings_sidebar_checks(&output);
    }
    if std::env::args().any(|arg| arg == "--settings-process") {
        return settings_sidebar_process(&output);
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
            let story = stories::fixture(id).unwrap();
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
            let image = cx.capture_screenshot(window.into())?;
            let default_color = zork_ui::design::ZORK_UI.palette.sidebar.to_be_bytes();
            let selected_color = zork_ui::design::ZORK_UI.palette.selected.to_be_bytes();
            anyhow::ensure!(
                if selected {
                    normal[..3] == selected_color[1..]
                } else {
                    normal[..3] == default_color[1..]
                },
                "navigation selected surface does not match its current state: selected={selected}, pixel={normal:?}"
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
            act(
                &mut cx,
                json!({"type":"move","target":{"x":bounds.x+bounds.width+12.,"y":bounds.y+bounds.height+12.}}),
            )?;
            let switched = pixel(&mut cx)?;
            anyhow::ensure!(
                if selected {
                    switched[..3] == default_color[1..]
                } else {
                    switched[..3] == selected_color[1..]
                },
                "selected surface did not follow click: {switched:?}"
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
    settings_sidebar_process(&output)?;
    chat_hover_checks()?;
    composer_preview_checks()?;
    println!("PASS navigation input, geometry and CPU draw budget");
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
        machine: None,
        color: None,
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
                zork_ui::resources::Text(Rc::new(|key| {
                    zork_gui::i18n::Locale::ZhCn.text(key).into()
                })),
                cx,
            );
            view.set_data(vec![initial], Some("device-0".into()), 280., cx);
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
            navigation.set_data(vec![refreshed], Some("device-0".into()), 280., cx)
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
        let client_heading = bounds("client-settings-heading");
        let mesh_heading = bounds("mesh-settings-heading");
        let advanced_heading = bounds("advanced-settings-heading");
        // Documented order: 客户端 (账号 · 外观 · 通知 · 已归档的 Chat), Mesh
        // (模型连接 · devices · 连接设备), 高级 (数据).
        let order = [
            "client-settings-heading",
            "client_account",
            "client_appearance",
            "client_notifications",
            "client_archived",
            "mesh-settings-heading",
            "settings-models",
            "settings-device-fixture",
            "settings-add-device",
            "advanced-settings-heading",
            "client_data",
        ];
        for pair in order.windows(2) {
            anyhow::ensure!(
                bounds(pair[0]).y < bounds(pair[1]).y,
                "settings order: {} should precede {}",
                pair[0],
                pair[1]
            );
        }
        for heading in [mesh_heading, advanced_heading] {
            anyhow::ensure!(
                client_heading.height == heading.height
                    && client_heading.width == heading.width
                    && client_heading.x == heading.x,
                "settings section headings differ"
            );
        }
        for (heading, first) in [
            (client_heading, account),
            (mesh_heading, bounds("settings-models")),
            (advanced_heading, bounds("client_data")),
        ] {
            anyhow::ensure!(
                (first.y - heading.y - heading.height - 2.).abs() < 0.1,
                "settings section heading/tab spacing differs"
            );
        }
        for id in [
            "desktop-return",
            "client_account",
            "client_appearance",
            "client_notifications",
            "client_archived",
            "settings-models",
            "settings-device-fixture",
            "settings-add-device",
            "client_data",
        ] {
            let row = bounds(id);
            anyhow::ensure!(
                row.height == 32. && row.width == notifications.width,
                "settings tab geometry differs: {id}: {row:?}"
            );
        }
        for pair in [
            "client_account",
            "client_appearance",
            "client_notifications",
            "client_archived",
        ]
        .windows(2)
        {
            let (previous, next) = (bounds(pair[0]), bounds(pair[1]));
            anyhow::ensure!(
                (next.y - previous.y - previous.height - 2.).abs() < 0.1,
                "settings tab gap differs: {previous:?} -> {next:?}"
            );
        }
        // Archived Chats moved here from the Chat sidebar.
        anyhow::ensure!(
            !snapshot.elements.iter().any(|e| e.id == "chat-archive-filter"),
            "the sidebar still shows an archived filter"
        );
        let scale = snapshot.scale_factor;
        let selected_color = zork_ui::design::ZORK_UI.palette.selected.to_be_bytes();
        let default_color = zork_ui::design::ZORK_UI.palette.sidebar.to_be_bytes();
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
        let notifications_fill = sample(
            &mut cx,
            notifications.x + notifications.width - 12.,
            notifications.y + 16.,
        )?;
        anyhow::ensure!(
            notifications_fill[..3] == selected_color[1..],
            "notifications selected fill missing: {notifications_fill:?} at {notifications:?}"
        );
        anyhow::ensure!(
            sample(&mut cx, account.x + account.width - 12., account.y + 16.)?[..3]
                == default_color[1..],
            "unselected account tab has a selected fill"
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
            sample(&mut cx, account.x + account.width - 12., account.y + 16.)?[..3]
                == selected_color[1..],
            "account selection did not update its fill"
        );
        anyhow::ensure!(
            sample(
                &mut cx,
                notifications.x + notifications.width - 12.,
                notifications.y + 16.
            )?[..3]
                == default_color[1..],
            "old notifications fill remained"
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
                &json!({"width":width,"height":height,"notifications":notifications,"account":account,"client_heading":client_heading,"mesh_heading":mesh_heading,"idle_callbacks":pending}),
            )?,
        )?;
    }
    println!("PASS client settings sidebar: shared geometry, selected fill and idle state");
    Ok(())
}
