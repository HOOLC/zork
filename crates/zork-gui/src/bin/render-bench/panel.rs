//! Panel motion measured in the production conversation layout.
use gpui::{
    point, px, size, AppContext, Entity, HeadlessAppContext, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, PlatformInput,
};
use serde_json::json;
use std::{path::Path, sync::Arc, time::Duration};
use zork_gui::{
    assets::EmbeddedAssets,
    automation::{AutomationRoot, HeadlessAutomation},
    views::RootView,
};

pub fn run(output: &Path) -> anyhow::Result<()> {
    std::env::set_var("ZORK_GUI_TEST_REDUCE_MOTION", "0");
    let width = std::env::var("ZORK_BENCH_PANEL_WIDTH")
        .ok()
        .and_then(|s| s.parse::<f32>().ok())
        .unwrap_or(1280.);
    let fixture = tempfile::tempdir()?;
    let store = Arc::new(zork_gui::desktop::store::ClientStore::open(fixture.path())?);
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
    let mut root = None;
    let handle = cx.open_window(size(px(width), px(800.)), |_, cx| {
        let view = cx.new(|cx| RootView::render_benchmark_fixture(false, store, cx));
        root = Some(view.clone());
        cx.new(|_| AutomationRoot::new(view))
    })?;
    let root = root.unwrap();
    for _ in 0..40 {
        cx.advance_clock(Duration::from_nanos(1_000_000_000 / 120));
        cx.update_window(handle.into(), |_, w, cx| {
            w.simulate_next_frame(cx);
            w.draw(cx).clear(cx)
        })?;
        cx.run_until_parked();
    }
    if std::env::var_os("ZORK_BENCH_PANEL_TRACE").is_some() {
        return trace(&mut cx, handle.into(), &root, &driver, output);
    }
    let mut reports = Vec::new();
    let mut split_width = 0.;
    for (case, control, frames) in [
        ("open", "conversation-browser", 40),
        ("close", "conversation-browser", 40),
        ("open-repeat", "conversation-browser", 40),
        ("close-repeat", "conversation-browser", 40),
        ("reverse-start", "conversation-browser", 8),
        ("reverse", "conversation-browser", 40),
        ("open-for-expand", "conversation-browser", 40),
        ("expand", "browser-expand", 40),
        ("restore", "browser-expand", 40),
        ("expand-reverse-start", "browser-expand", 8),
        ("restore-reverse", "browser-expand", 40),
        ("close-after-restore", "conversation-browser", 40),
    ] {
        let start_width = root.read_with(&cx, |v, cx| v.benchmark_panel_width(cx));
        let before = cx.update_window(handle.into(), |_, w, _| w.frame_duration_snapshot())?;
        let counters = root.read_with(&cx, |v, cx| v.benchmark_region_counts(cx));
        cx.update_window(handle.into(), |_, w, cx| {
            driver.dispatch(
                serde_json::from_value(json!({"type":"click","target":{"element_id":control}}))
                    .unwrap(),
                w,
                cx,
            )
        })??;
        cx.run_until_parked();
        let mut widths = Vec::new();
        let mut page_widths = Vec::new();
        let mut max_rows = 0;
        for _ in 0..frames {
            root.update(&mut cx, |v, _| v.benchmark_begin_frame());
            cx.advance_clock(Duration::from_nanos(1_000_000_000 / 120));
            cx.update_window(handle.into(), |_, w, cx| {
                w.simulate_next_frame(cx);
                w.draw(cx).clear(cx)
            })?;
            cx.run_until_parked();
            max_rows = max_rows.max(root.read_with(&cx, |v, _| v.benchmark_frame_state(false).0));
            widths.push(root.read_with(&cx, |v, cx| v.benchmark_panel_width(cx)));
            if control == "browser-expand" {
                let page = driver
                    .snapshot(false)
                    .elements
                    .into_iter()
                    .find(|e| e.id == "browser-page")
                    .unwrap();
                page_widths.push(page.bounds.width);
                anyhow::ensure!(
                    (page.bounds.width - widths.last().unwrap()).abs() <= 2.,
                    "page jumped ahead of panel aperture: {case}"
                );
            }
        }
        let mut after = cx.update_window(handle.into(), |_, w, _| w.frame_duration_snapshot())?;
        after
            .draw_duration_histogram
            .subtract(&before.draw_duration_histogram)?;
        let mut delta = root.read_with(&cx, |v, cx| v.benchmark_region_counts(cx));
        for (name, counts) in &mut delta {
            if let Some(old) = counters.get(name) {
                for (value, old) in counts.iter_mut().zip(old) {
                    *value -= old;
                }
            }
        }
        anyhow::ensure!(
            widths.windows(2).any(|w| (w[1] - w[0]).abs() > 1.),
            "panel did not move: {case}"
        );
        if case.starts_with("close") || case == "reverse" {
            anyhow::ensure!(*widths.last().unwrap() == 0., "panel did not close: {case}");
        }
        if case.starts_with("open") {
            anyhow::ensure!(*widths.last().unwrap() > 200., "panel did not open: {case}");
            split_width = *widths.last().unwrap();
        }
        if case == "expand" {
            anyhow::ensure!(
                *widths.last().unwrap() > split_width + 100.,
                "panel did not expand"
            );
        }
        if case.starts_with("restore") {
            anyhow::ensure!(
                (widths.last().unwrap() - split_width).abs() < 0.5,
                "panel did not restore its split width: {case}"
            );
            anyhow::ensure!(
                (widths[0] - start_width).abs() < (start_width - split_width) * 0.25,
                "restore snapped to its target on the first frame: {case}"
            );
        }
        anyhow::ensure!(max_rows < 100, "panel motion rendered unbounded messages");
        cx.capture_screenshot(handle.into())?
            .save(output.join(format!("{case}.png")))?;
        reports.push(json!({"case":case,"p95_draw_ms":after.draw_duration_histogram.value_at_quantile(0.95) as f64 / 1e6,
            "p99_draw_ms":after.draw_duration_histogram.value_at_quantile(0.99) as f64 / 1e6,
            "draws":after.draw_duration_histogram.len(),"max_rendered_rows":max_rows,"panel_widths":widths,"page_widths":page_widths,"region_counts":delta}));
    }
    std::env::set_var("ZORK_GUI_TEST_REDUCE_MOTION", "1");
    cx.update(|cx| cx.set_reduce_motion(true));
    for expected_open in [true, false] {
        cx.update_window(handle.into(), |_, w, cx| {
            driver.dispatch(
                serde_json::from_value(
                    json!({"type":"click","target":{"element_id":"conversation-browser"}}),
                )
                .unwrap(),
                w,
                cx,
            )
        })??;
        cx.run_until_parked();
        cx.update_window(handle.into(), |_, w, cx| w.draw(cx).clear(cx))?;
        let actual = root.read_with(&cx, |v, cx| v.benchmark_panel_width(cx));
        anyhow::ensure!(
            if expected_open {
                actual > 200.
            } else {
                actual == 0.
            },
            "reduced motion did not settle immediately: open={expected_open}, width={actual}"
        );
    }
    cx.capture_screenshot(handle.into())?
        .save(output.join("panel.png"))?;
    let report = json!({"viewport":[width,800],"force_invalidation":std::env::var_os("ZORK_BENCH_PANEL_FORCE_INVALIDATION").is_some(),"reduced_motion":"passed","records":root.read_with(&cx, |v, _| v.benchmark_record_count(false)),
        "measurement":"CPU layout and paint at 120 Hz virtual input; excludes GPU and display FPS", "cases":reports});
    std::fs::write(
        output.join("panel.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

fn geometry(
    cx: &HeadlessAppContext,
    root: &Entity<RootView>,
    driver: &HeadlessAutomation,
) -> serde_json::Value {
    let surface = driver
        .snapshot(false)
        .elements
        .into_iter()
        .find(|e| e.id == "composer-surface")
        .unwrap();
    json!({"panel_width":root.read_with(cx, |v,cx| v.benchmark_panel_width(cx)), "composer":surface.bounds,
        "scroll":root.read_with(cx, |v,_| v.benchmark_frame_state(false))})
}

fn trace(
    cx: &mut HeadlessAppContext,
    window: gpui::AnyWindowHandle,
    root: &Entity<RootView>,
    driver: &HeadlessAutomation,
    output: &Path,
) -> anyhow::Result<()> {
    let mut samples = vec![geometry(cx, root, driver)];
    cx.update_window(window, |_, w, cx| {
        driver.dispatch(
            serde_json::from_value(
                json!({"type":"click","target":{"element_id":"conversation-browser"}}),
            )
            .unwrap(),
            w,
            cx,
        )
    })??;
    cx.run_until_parked();
    samples.push(geometry(cx, root, driver));
    for _ in 0..40 {
        cx.advance_clock(Duration::from_nanos(1_000_000_000 / 120));
        cx.update_window(window, |_, w, cx| {
            w.simulate_next_frame(cx);
            w.draw(cx).clear(cx)
        })?;
        cx.run_until_parked();
        samples.push(geometry(cx, root, driver));
    }
    if std::env::var_os("ZORK_BENCH_PANEL_ASSERT_LAYOUT").is_some() {
        let first_x = samples[0]["composer"]["x"].as_f64().unwrap();
        let last = samples.last().unwrap();
        let last_x = last["composer"]["x"].as_f64().unwrap();
        let last_width = last["panel_width"].as_f64().unwrap();
        for sample in &samples {
            let progress = sample["panel_width"].as_f64().unwrap() / last_width;
            let expected = first_x + (last_x - first_x) * progress;
            anyhow::ensure!(
                (sample["composer"]["x"].as_f64().unwrap() - expected).abs() <= 1.,
                "composer centering jumped ahead of panel motion: {sample}"
            );
        }
    }
    let mut drags = Vec::new();
    for (name, frames, step) in [
        ("slow", 320, -0.5),
        ("fast", 10, 16.),
        ("stationary", 40, 0.),
    ] {
        let grip = driver
            .snapshot(false)
            .elements
            .into_iter()
            .find(|e| e.id == "page-resize")
            .unwrap()
            .center;
        let before = cx.update_window(window, |_, w, _| w.frame_duration_snapshot())?;
        let counters = root.read_with(cx, |v, cx| v.benchmark_region_counts(cx));
        cx.update_window(window, |_, w, cx| {
            w.dispatch_event(
                PlatformInput::MouseDown(MouseDownEvent {
                    button: MouseButton::Left,
                    position: point(px(grip.x), px(grip.y)),
                    click_count: 1,
                    ..Default::default()
                }),
                cx,
            );
        })?;
        cx.run_until_parked();
        let mut frames_out = Vec::new();
        for index in 1..=frames {
            cx.advance_clock(Duration::from_nanos(1_000_000_000 / 120));
            cx.update_window(window, |_, w, cx| {
                w.dispatch_event(
                    PlatformInput::MouseMove(MouseMoveEvent {
                        position: point(px(grip.x + step * index as f32), px(grip.y)),
                        pressed_button: Some(MouseButton::Left),
                        ..Default::default()
                    }),
                    cx,
                );
            })?;
            cx.run_until_parked();
            cx.update_window(window, |_, w, cx| {
                w.simulate_next_frame(cx);
                w.draw(cx).clear(cx)
            })?;
            cx.run_until_parked();
            frames_out.push(geometry(cx, root, driver));
        }
        cx.update_window(window, |_, w, cx| {
            w.dispatch_event(
                PlatformInput::MouseUp(MouseUpEvent {
                    button: MouseButton::Left,
                    position: point(px(grip.x + step * frames as f32), px(grip.y)),
                    click_count: 1,
                    ..Default::default()
                }),
                cx,
            );
        })?;
        let mut after = cx.update_window(window, |_, w, _| w.frame_duration_snapshot())?;
        after
            .draw_duration_histogram
            .subtract(&before.draw_duration_histogram)?;
        let mut delta = root.read_with(cx, |v, cx| v.benchmark_region_counts(cx));
        for (name, values) in &mut delta {
            if let Some(old) = counters.get(name) {
                for (value, old) in values.iter_mut().zip(old) {
                    *value -= old;
                }
            }
        }
        if name == "slow" && std::env::var_os("ZORK_BENCH_PANEL_ASSERT_LAYOUT").is_some() {
            for pair in frames_out.windows(2) {
                let delta = pair[1]["panel_width"].as_f64().unwrap()
                    - pair[0]["panel_width"].as_f64().unwrap();
                anyhow::ensure!(
                    (0.0..=0.501).contains(&delta),
                    "slow drag lost intermediate pointer positions: {pair:?}"
                );
            }
        }
        cx.run_until_parked();
        drags.push(json!({"case":name,"frames":frames_out,"region_counts":delta,"p95_draw_ms":after.draw_duration_histogram.value_at_quantile(0.95) as f64 / 1e6,"p99_draw_ms":after.draw_duration_histogram.value_at_quantile(0.99) as f64 / 1e6}));
    }
    std::fs::write(
        output.join("geometry.json"),
        serde_json::to_vec_pretty(&json!({"open":samples,"drags":drags}))?,
    )?;
    println!("Panel geometry and resize traces saved");
    Ok(())
}
