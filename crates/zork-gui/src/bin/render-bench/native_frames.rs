//! Complete frames from the production desktop view and native Metal renderer.
use gpui::{prelude::*, *};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    cell::RefCell,
    path::Path,
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};
use zork_gui::{
    assets::EmbeddedAssets,
    automation::{protocol::UserAction, AutomationRoot, HeadlessAutomation},
    views::RootView,
};
#[path = "native_frames/playground.rs"]
mod playground;

#[derive(Clone, Deserialize, Serialize)]
struct Config {
    viewport: [f32; 2],
    liquid_viewport: [f32; 2],
    #[serde(default)]
    offscreen: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    liquid_only: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    playground: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    playground_scroll: Option<PlaygroundScroll>,
    messages: usize,
    panel_pairs: usize,
    phase_ms: u64,
    scroll_px_per_second: f32,
}

#[derive(Clone, Deserialize, Serialize)]
struct PlaygroundScroll {
    px_per_second: f32,
    legs: u32,
    min_displacement_px: f32,
}

fn is_false(value: &bool) -> bool {
    !value
}

fn thread_cpu_time() -> Option<Duration> {
    static ENABLED: std::sync::LazyLock<bool> =
        std::sync::LazyLock::new(|| std::env::var_os("ZORK_BENCH_THREAD_CPU").is_some());
    if !*ENABLED {
        return None;
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        let mut time = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        if unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut time) } == 0 {
            return Some(Duration::new(time.tv_sec as u64, time.tv_nsec as u32));
        }
    }
    None
}

enum FrameInput {
    Idle,
    Action(UserAction),
    Scroll(f32),
    ScrollAt([f32; 2], f32),
    Jump(Entity<RootView>, usize),
    Refresh,
}

async fn frame(
    window: AnyWindowHandle,
    cx: &mut AsyncApp,
    driver: &HeadlessAutomation,
    input: FrameInput,
    origin: Instant,
    offscreen: bool,
) -> anyhow::Result<Option<Value>> {
    let started = Instant::now();
    let thread_started = thread_cpu_time();
    let continuous = matches!(
        input,
        FrameInput::Scroll(_)
            | FrameInput::ScrollAt(_, _)
            | FrameInput::Jump(_, _)
            | FrameInput::Refresh
    );
    window.update(cx, |_, window, cx| -> anyhow::Result<()> {
        match input {
            FrameInput::Idle => {}
            FrameInput::Action(action) => {
                driver.dispatch(action, window, cx)?;
            }
            FrameInput::Jump(root, index) => {
                root.update(cx, |view, cx| view.benchmark_jump_to(index, cx))
            }
            FrameInput::Refresh => window.refresh(),
            FrameInput::Scroll(delta) => {
                window.dispatch_event(
                    PlatformInput::ScrollWheel(ScrollWheelEvent {
                        position: point(px(700.), px(550.)),
                        delta: ScrollDelta::Pixels(point(px(0.), px(delta))),
                        modifiers: Modifiers::default(),
                        touch_phase: TouchPhase::Moved,
                    }),
                    cx,
                );
            }
            FrameInput::ScrollAt(position, delta) => {
                window.dispatch_event(
                    PlatformInput::ScrollWheel(ScrollWheelEvent {
                        position: point(px(position[0]), px(position[1])),
                        delta: ScrollDelta::Pixels(point(px(0.), px(delta))),
                        modifiers: Modifiers::default(),
                        touch_phase: TouchPhase::Moved,
                    }),
                    cx,
                );
            }
        }
        window.simulate_next_frame(cx);
        Ok(())
    })??;
    // The preceding App update flushes notifications before deciding to draw.
    let (receiver, continuing, input_counts) = window.update(cx, |_, window, cx| {
        let receiver = window.benchmark_frame(cx, offscreen)?;
        Ok::<_, anyhow::Error>((
            receiver,
            window.has_animation_frames(),
            window.benchmark_input_counts(),
        ))
    })??;
    let cpu_done = Instant::now();
    let thread_elapsed = thread_started
        .zip(thread_cpu_time())
        .map(|(start, end)| end.saturating_sub(start));
    let Some(receiver) = receiver else {
        return Ok(None);
    };
    let timeout = cx.background_executor().timer(Duration::from_secs(3));
    futures_util::pin_mut!(receiver, timeout);
    let completion = match futures_util::future::select(receiver, timeout).await {
        futures_util::future::Either::Left((result, _)) => result?,
        _ => anyhow::bail!("Metal did not report GPU completion"),
    };
    anyhow::ensure!(completion.succeeded, "Metal command buffer failed");
    let completed = completion.completed_at.max(cpu_done);
    Ok(Some(json!({
        "at": started.duration_since(origin).as_secs_f64() * 1000.,
        "cpuMs": cpu_done.duration_since(started).as_secs_f64() * 1000.,
        "threadCpuMs": thread_elapsed.map(|elapsed| elapsed.as_secs_f64() * 1000.),
        "completedAt": completed.duration_since(origin).as_secs_f64() * 1000.,
        "continuing": continuing || continuous,
        "drawableWaitMs": completion.drawable_wait.as_secs_f64() * 1000.,
        "encodingMs": completion.encoding.as_secs_f64() * 1000.,
        "retainedLayers": completion.render.retained_layers,
        "contentRedraws": completion.render.content_redraws,
        "maskRedraws": completion.render.mask_redraws,
        "retainedFallback": completion.render.retained_fallback,
        "inputTree": {
            "nodes": input_counts[0], "hitboxes": input_counts[1], "listeners": input_counts[2],
            "handlers": input_counts[3], "states": input_counts[4], "accesses": input_counts[5],
        },
    })))
}

async fn measure(
    window: AnyWindowHandle,
    root: Entity<RootView>,
    driver: HeadlessAutomation,
    config: Config,
    report: Rc<RefCell<Value>>,
    output: std::path::PathBuf,
    cx: &mut AsyncApp,
) -> anyhow::Result<()> {
    if config.liquid_only {
        window.update(cx, |_, window, _| window.remove_window())?;
        return measure_liquid(&driver, &config, &report, &output, cx).await;
    }
    let origin = Instant::now();
    // Initialize the ordinary client view before the first measured interaction.
    cx.background_executor()
        .timer(Duration::from_millis(1000))
        .await;
    for _ in 0..8 {
        frame(
            window,
            cx,
            &driver,
            FrameInput::Idle,
            origin,
            config.offscreen,
        )
        .await?;
        cx.background_executor()
            .timer(Duration::from_millis(8))
            .await;
    }
    let records = window.update(cx, |_, window, cx| {
        report.borrow_mut()["viewport"] = json!([
            window.viewport_size().width.as_f32(),
            window.viewport_size().height.as_f32()
        ]);
        report.borrow_mut()["scaleFactor"] = json!(window.scale_factor());
        root.read(cx).benchmark_record_count(false)
    })?;
    anyhow::ensure!(
        records == config.messages,
        "wrong client message workload: {records}"
    );
    report.borrow_mut()["records"] = json!(records);
    let mut cold_entries = Vec::new();
    for index in (0..RootView::benchmark_kind_count() * 2).chain(std::iter::once(0)) {
        if let Some(row) = frame(
            window,
            cx,
            &driver,
            FrameInput::Jump(root.clone(), index),
            origin,
            config.offscreen,
        )
        .await?
        {
            cold_entries.push(row);
        }
    }
    report.borrow_mut()["coldEntries"] = json!(cold_entries);
    for index in 0..config.panel_pairs * 2 {
        let opening = index % 2 == 0;
        let action: UserAction = serde_json::from_value(
            json!({"type":"click", "target":{"element_id":"conversation-browser"}}),
        )?;
        let before = window.update(cx, |_, _, cx| root.read(cx).benchmark_panel_width(cx))?;
        let begin = Instant::now();
        let mut first = Some(action);
        let mut frames = Vec::new();
        while begin.elapsed() < Duration::from_millis(config.phase_ms) {
            if let Some(row) = frame(
                window,
                cx,
                &driver,
                first.take().map_or(FrameInput::Idle, FrameInput::Action),
                origin,
                config.offscreen,
            )
            .await?
            {
                frames.push(row);
            } else {
                cx.background_executor()
                    .timer(Duration::from_millis(1))
                    .await;
            }
        }
        let after = window.update(cx, |_, _, cx| root.read(cx).benchmark_panel_width(cx))?;
        anyhow::ensure!(
            if opening {
                after > before + 50.
            } else {
                after < before - 50.
            },
            "client panel did not complete {opening}: {before} -> {after}"
        );
        if index == 0 {
            // Pixel evidence is outside the timed frame stream.
            window
                .update(cx, |_, window, _| window.render_to_image())??
                .save(output.join("client-panel-open.png"))?;
        }
        report.borrow_mut()["cases"]
            .as_array_mut()
            .unwrap()
            .push(json!({
                "name": if opening {"panel-open"} else {"panel-close"}, "index":index,
                "beforeWidth":before, "afterWidth":after, "frames":frames,
            }));
        std::fs::write(
            output.join("native-frames.json"),
            serde_json::to_vec_pretty(&*report.borrow())?,
        )?;
    }
    for (name, anchor, direction) in [
        ("scroll-start", 0, -1.),
        ("scroll-middle", config.messages / 2, -1.),
        ("scroll-end", config.messages.saturating_sub(40), 1.),
    ] {
        let begin = Instant::now();
        let mut previous = begin;
        let mut frames = Vec::new();
        if let Some(row) = frame(
            window,
            cx,
            &driver,
            FrameInput::Jump(root.clone(), anchor),
            origin,
            config.offscreen,
        )
        .await?
        {
            frames.push(row);
        }
        let before = window.update(cx, |_, _, cx| root.read(cx).benchmark_frame_state(false))?;
        while begin.elapsed() < Duration::from_millis(config.phase_ms) {
            let now = Instant::now();
            let delta = config.scroll_px_per_second
                * now.duration_since(previous).as_secs_f32().max(0.001)
                * direction;
            previous = now;
            if let Some(row) = frame(
                window,
                cx,
                &driver,
                FrameInput::Scroll(delta),
                origin,
                config.offscreen,
            )
            .await?
            {
                frames.push(row);
            }
        }
        let after = window.update(cx, |_, _, cx| root.read(cx).benchmark_frame_state(false))?;
        anyhow::ensure!(
            before.1 != after.1 || (before.2 - after.2).abs() > 1.,
            "client transcript did not scroll: {name}"
        );
        report.borrow_mut()["cases"].as_array_mut().unwrap().push(
            json!({"name":name,"anchor":anchor,"before":before,"after":after,"frames":frames}),
        );
        std::fs::write(
            output.join("native-frames.json"),
            serde_json::to_vec_pretty(&*report.borrow())?,
        )?;
    }
    report.borrow_mut()["coverage"] =
        window.update(cx, |_, _, cx| root.read(cx).benchmark_message_coverage())?;
    window.update(cx, |_, window, _| window.remove_window())?;
    measure_liquid(&driver, &config, &report, &output, cx).await
}

async fn measure_liquid(
    driver: &HeadlessAutomation,
    config: &Config,
    report: &Rc<RefCell<Value>>,
    output: &Path,
    cx: &mut AsyncApp,
) -> anyhow::Result<()> {
    use zork_ui::liquid_story::Gallery;
    let (window, gallery) = cx.update(|cx| -> anyhow::Result<_> {
        zork_gui::desktop::stories::install(cx);
        zork_ui::liquid_story::install_composer_fixture(cx, || {
            Box::new(zork_client_core::composer::fixture::Fixture::default())
        });
        let mut gallery = None;
        let window = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                    None,
                    size(px(config.liquid_viewport[0]), px(config.liquid_viewport[1])),
                    cx,
                ))),
                titlebar: Some(zork_gui::window_chrome::native_titlebar_options()),
                ..Default::default()
            },
            |window, cx| {
                window.prepare_benchmark_frames();
                let view = cx.new(Gallery::new);
                gallery = Some(view.clone());
                cx.new(|_| AutomationRoot::new(view))
            },
        )?;
        cx.activate(true);
        Ok((AnyWindowHandle::from(window), gallery.unwrap()))
    })?;
    let origin = Instant::now();
    for _ in 0..8 {
        frame(
            window,
            cx,
            driver,
            FrameInput::Idle,
            origin,
            config.offscreen,
        )
        .await?;
        cx.background_executor()
            .timer(Duration::from_millis(8))
            .await;
    }
    let viewport = window.update(cx, |_, window, _| {
        [
            window.viewport_size().width.as_f32(),
            window.viewport_size().height.as_f32(),
        ]
    })?;
    report.borrow_mut()["liquid"] = json!({"viewport":viewport,
        "component":"zork-ui::liquid::Dialog", "control":"liquid-library-dialog",
        "recipe":"static-content-contour-midpoint-backdrop", "cases":[], "status":"unverified"});
    anyhow::ensure!(
        viewport == config.liquid_viewport,
        "liquid viewport mismatch: {viewport:?} != {:?}",
        config.liquid_viewport
    );
    for index in 0..config.panel_pairs * 2 {
        let opening = index % 2 == 0;
        let action: UserAction = serde_json::from_value(if opening {
            json!({"type":"click","target":{"element_id":"liquid-library-toggle"}})
        } else {
            json!({"type":"key","keystroke":"escape"})
        })?;
        let before = window.update(cx, |_, _, cx| gallery.read(cx).inspect(cx))?;
        let begin = Instant::now();
        let mut first = Some(action);
        let mut frames = Vec::new();
        loop {
            if let Some(row) = frame(
                window,
                cx,
                driver,
                first.take().map_or(FrameInput::Idle, FrameInput::Action),
                origin,
                config.offscreen,
            )
            .await?
            {
                frames.push(row);
            } else {
                cx.background_executor()
                    .timer(Duration::from_millis(1))
                    .await;
            }
            let continuing = window.update(cx, |_, window, _| window.has_animation_frames())?;
            if begin.elapsed() >= Duration::from_millis(config.phase_ms) && !continuing {
                break;
            }
            anyhow::ensure!(
                begin.elapsed() < Duration::from_secs(7),
                "liquid presentation did not settle"
            );
        }
        let after = window.update(cx, |_, _, cx| gallery.read(cx).inspect(cx))?;
        anyhow::ensure!(
            after["dialog"]["backdropAlpha"] == if opening { 1. } else { 0. },
            "liquid phase did not include the complete backdrop transition"
        );
        anyhow::ensure!(
            if opening {
                after["panel"] == "library"
            } else {
                after["panel"].is_null()
            },
            "liquid directory input did not change its open state: {after}"
        );
        let before_frames = before["dialog"]["paintOnlyFrames"].as_u64().unwrap_or(0);
        let after_frames = after["dialog"]["paintOnlyFrames"].as_u64().unwrap_or(0);
        anyhow::ensure!(
            after_frames > before_frames,
            "liquid transition did not execute shared paint playback"
        );
        anyhow::ensure!(
            frames
                .iter()
                .any(|row| row["maskRedraws"].as_u64().unwrap_or(0) > 0)
                && frames
                    .iter()
                    .any(|row| row["retainedLayers"].as_u64().unwrap_or(0) > 0
                        && row["contentRedraws"] == 0),
            "native retained content/mask path was not exercised"
        );
        if index < 2 {
            window
                .update(cx, |_, window, _| window.render_to_image())??
                .save(output.join(if opening {
                    "liquid-open.png"
                } else {
                    "liquid-closed.png"
                }))?;
        }
        report.borrow_mut()["liquid"]["cases"]
            .as_array_mut()
            .unwrap()
            .push(json!({
            "name":if opening {"liquid-open"} else {"liquid-close"}, "frames":frames,
            "paintOnlyFrames":after_frames - before_frames,
            "before":before["dialog"], "after":after["dialog"]}));
    }
    report.borrow_mut()["liquid"]["status"] = "measured".into();
    // Readbacks and deliberately paced reversal gestures are correctness
    // evidence, outside every measured performance case above.
    let mut visual_frames = Vec::new();
    window
        .update(cx, |_, window, _| window.render_to_image())??
        .save(output.join("liquid-reference.png"))?;
    for (phase, opening, count) in [
        ("opening", true, 6),
        ("closing", false, 3),
        ("reversing", true, 12),
        ("returning", false, 28),
    ] {
        window
            .update(cx, |_, window, _| window.render_to_image())??
            .save(output.join(format!("liquid-{phase}-before-transfer.png")))?;
        let action = serde_json::from_value(if opening {
            json!({"type":"click","target":{"element_id":"liquid-library-toggle"}})
        } else {
            json!({"type":"key","keystroke":"escape"})
        })?;
        frame(
            window,
            cx,
            driver,
            FrameInput::Action(action),
            origin,
            config.offscreen,
        )
        .await?;
        window
            .update(cx, |_, window, _| window.render_to_image())??
            .save(output.join(format!("liquid-{phase}-after-transfer.png")))?;
        let destination = window.update(cx, |_, _, cx| gallery.read(cx).inspect(cx))?;
        anyhow::ensure!(
            destination["dialog"]["destinationLayer"] == if opening { "modal" } else { "source" },
            "layer did not change at the start of the operation: {destination}"
        );
        for index in 0..count {
            cx.background_executor()
                .timer(Duration::from_millis(25))
                .await;
            frame(
                window,
                cx,
                driver,
                FrameInput::Idle,
                origin,
                config.offscreen,
            )
            .await?;
            let state = window.update(cx, |_, _, cx| gallery.read(cx).inspect(cx))?;
            anyhow::ensure!(
                state["dialog"]["open"] == opening,
                "reversal lost its target"
            );
            anyhow::ensure!(
                driver
                    .snapshot(false)
                    .elements
                    .iter()
                    .filter(|element| element.id == "liquid-library-toggle")
                    .count()
                    == 1,
                "liquid source must keep one input identity"
            );
            let file = format!("liquid-{phase}-{index:02}.png");
            window
                .update(cx, |_, window, _| window.render_to_image())??
                .save(output.join(&file))?;
            visual_frames
                .push(json!({"phase":phase,"index":index,"file":file,"motion":state["dialog"]}));
        }
    }
    std::fs::write(
        output.join("liquid-visual.json"),
        serde_json::to_vec_pretty(&visual_frames)?,
    )?;
    if config.playground {
        report.borrow_mut()["playground"] =
            playground::measure(window, gallery, driver, config, &output, cx).await?;
    }
    window.update(cx, |_, window, _| window.remove_window())?;
    Ok(())
}

pub fn run(config_path: &Path, output: &Path) -> anyhow::Result<()> {
    #[cfg(target_os = "macos")]
    gpui_apple::metal_renderer::prepare_renderer();
    let config: Config = serde_json::from_slice(&std::fs::read(config_path)?)?;
    anyhow::ensure!(
        config.messages >= 100000 && config.panel_pairs > 0 && config.phase_ms >= 500,
        "invalid frame workload"
    );
    std::fs::create_dir_all(output)?;
    std::env::set_var("SEED", "0");
    std::env::set_var("TZ", "UTC");
    std::env::set_var("ZORK_GUI_TEST_REDUCE_MOTION", "0");
    std::env::set_var("ZORK_SCROLL_ALL_MESSAGES", "1");
    std::env::set_var("ZORK_BENCH_MESSAGE_COUNT", config.messages.to_string());
    let fixture = tempfile::tempdir()?;
    let preferences = fixture.path().join("preferences.json");
    std::fs::write(&preferences, "{}")?;
    std::env::set_var("ZORK_GUI_PREFERENCES_PATH", preferences);
    let store = Arc::new(zork_gui::desktop::store::ClientStore::open(fixture.path())?);
    let report = Rc::new(RefCell::new(
        json!({"fixture":config,"renderer":"native-metal","uncapped":config.offscreen,
        "profile":if cfg!(debug_assertions) {"dev"} else {"release"},
        "physicalPresentationMeasured":false,"gpuCompletionProbe":true,
        "threadCpuDiagnostic":std::env::var_os("ZORK_BENCH_THREAD_CPU").is_some(),
        "cases":[],"status":"unverified"}),
    ));
    let capture = report.clone();
    let output_owned = output.to_path_buf();
    gpui_platform::application()
        .with_assets(EmbeddedAssets)
        .run(move |cx| {
            zork_gui::assets::init_fonts(cx);
            zork_gui::components::init(cx);
            cx.set_reduce_motion(false);
            cx.set_window_appearance(Some(WindowAppearance::Light));
            let driver = HeadlessAutomation::install(cx);
            let mut root = None;
            let window: AnyWindowHandle = cx
                .open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                            None,
                            size(px(config.viewport[0]), px(config.viewport[1])),
                            cx,
                        ))),
                        titlebar: Some(zork_gui::window_chrome::native_titlebar_options()),
                        ..Default::default()
                    },
                    |window, cx| {
                        window.prepare_benchmark_frames();
                        window.on_window_should_close(cx, |_, cx| {
                            cx.quit();
                            true
                        });
                        let view =
                            cx.new(|cx| RootView::render_benchmark_fixture(false, store, cx));
                        root = Some(view.clone());
                        cx.new(|_| AutomationRoot::new(view))
                    },
                )
                .expect("native client benchmark window")
                .into();
            cx.activate(true);
            let root = root.unwrap();
            cx.spawn(async move |cx| {
                match measure(
                    window,
                    root,
                    driver,
                    config,
                    capture.clone(),
                    output_owned.clone(),
                    cx,
                )
                .await
                {
                    Ok(()) => capture.borrow_mut()["status"] = "measured".into(),
                    Err(error) => capture.borrow_mut()["error"] = format!("{error:#}").into(),
                }
                let _ = std::fs::write(
                    output_owned.join("native-frames.json"),
                    serde_json::to_vec_pretty(&*capture.borrow()).unwrap(),
                );
                let _ = cx.update(|cx| cx.quit());
            })
            .detach();
        });
    anyhow::ensure!(
        report.borrow()["status"] == "measured",
        "native frame measurement unavailable: {}",
        report.borrow()
    );
    Ok(())
}

/// A diagnostic lower bound for the same window, drawable and completion path.
/// This is never accepted as client workload evidence by the smoke evaluator.
pub fn calibrate(output: &Path, offscreen: bool) -> anyhow::Result<()> {
    struct Blank;
    impl Render for Blank {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().bg(rgb(0xffffff))
        }
    }
    let output = output.to_path_buf();
    let result = Rc::new(RefCell::new(Value::Null));
    let capture = result.clone();
    gpui_platform::application().run(move |cx| {
        let driver = HeadlessAutomation::install(cx);
        let window: AnyWindowHandle = cx.open_window(WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size(px(1280.), px(800.)), cx))),
            ..Default::default()
        }, |window, cx| { window.prepare_benchmark_frames(); cx.new(|_| Blank) }).unwrap().into();
        cx.activate(true);
        cx.spawn(async move |cx| {
            cx.background_executor().timer(Duration::from_millis(500)).await;
            let begin = Instant::now();
            let mut rows = Vec::new();
            let mut error = None;
            while begin.elapsed() < Duration::from_secs(2) {
                match frame(window, cx, &driver, FrameInput::Refresh, begin, offscreen).await {
                    Ok(Some(row)) => rows.push(row),
                    Ok(None) => {},
                    Err(e) => { error = Some(format!("{e:#}")); break; },
                }
            }
            *capture.borrow_mut() = json!({"kind":"empty-native-calibration","offscreen":offscreen,"frames":rows,"error":error});
            let _ = std::fs::write(&output, serde_json::to_vec_pretty(&*capture.borrow()).unwrap());
            let _ = cx.update(|cx| cx.quit());
        }).detach();
    });
    anyhow::ensure!(
        result.borrow()["frames"]
            .as_array()
            .is_some_and(|rows| rows.len() > 2),
        "native calibration unavailable"
    );
    Ok(())
}
