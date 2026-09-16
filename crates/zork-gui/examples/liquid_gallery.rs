//! Native interactive gallery and offscreen evidence from the same shared view.
use gpui::{prelude::*, *};
use serde_json::{json, Value};
use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use zork_ui::{
    assets::{init_fonts, EmbeddedAssets},
    automation::{
        driver::headless_action,
        element::{AutomationRegistry, AutomationRegistryGlobal},
        protocol::UserAction,
        AutomationRoot,
    },
    liquid_story::{Gallery, Kind},
};

fn install(cx: &mut App) -> AutomationRegistry {
    zork_gui::desktop::stories::install(cx);
    zork_ui::liquid_story::install_composer_fixture(cx, || {
        Box::new(zork_client_core::composer::fixture::Fixture::default())
    });
    init_fonts(cx);
    zork_ui::components::init(cx);
    let registry = AutomationRegistry::new();
    cx.set_global(AutomationRegistryGlobal(registry.clone()));
    registry
}
fn draw(cx: &mut HeadlessAppContext, window: AnyWindowHandle) {
    cx.update_window(window, |_, w, cx| w.simulate_next_frame(cx))
        .unwrap();
    // test-support flush_effects renders dirty windows at the end of the
    // update above. An explicit w.draw here submits the same frame twice.
    cx.run_until_parked();
}
fn action(
    cx: &mut HeadlessAppContext,
    window: AnyWindowHandle,
    registry: &AutomationRegistry,
    id: &str,
) {
    if let Some((prefix, variant)) = id.rsplit_once("-variant-") {
        if variant.parse::<usize>().is_ok() {
            if !registry
                .snapshot(false)
                .elements
                .iter()
                .any(|e| e.id == id && e.enabled)
                && !registry
                    .snapshot(false)
                    .elements
                    .iter()
                    .any(|e| e.id == format!("{prefix}-variant-select-menu"))
            {
                action(cx, window, registry, &format!("{prefix}-variant-select"));
            }
            for _ in 0..4 {
                let snapshot = registry.snapshot(false);
                let target = snapshot.elements.iter().find(|e| e.id == id);
                if target.is_some_and(|target| {
                    target.visible && target.visible_bounds.height >= target.bounds.height - 0.5
                }) {
                    break;
                }
                let menu = snapshot
                    .elements
                    .iter()
                    .find(|e| e.id == format!("{prefix}-variant-select-menu"))
                    .unwrap();
                let command: UserAction = serde_json::from_value(json!({
                    "type":"scroll", "target":{"x":menu.center.x,"y":menu.center.y},
                    "delta_y":if target.is_some_and(|target| target.bounds.y < menu.bounds.y)
                        || (target.is_none() && snapshot.elements.iter().filter_map(|e| e.id.strip_prefix(&format!("{prefix}-variant-"))?.parse::<usize>().ok()).min().is_some_and(|first| variant.parse::<usize>().unwrap() < first)) { 120. } else { -120. }
                }))
                .unwrap();
                cx.update_window(window, |_, w, cx| headless_action(command, w, cx, registry))
                    .unwrap()
                    .unwrap();
                draw(cx, window);
            }
        }
    }
    let command: UserAction =
        serde_json::from_value(json!({"type":"click","target":{"element_id":id}})).unwrap();
    cx.update_window(window, |_, w, cx| headless_action(command, w, cx, registry))
        .unwrap()
        .unwrap();
    draw(cx, window);
}
fn run_headless(output: PathBuf) {
    std::fs::create_dir_all(&output).unwrap();
    let mut results = Vec::new();
    for kind in Kind::ALL {
        let mut cx = HeadlessAppContext::with_platform(
            gpui_platform::current_platform(true).text_system(),
            Arc::new(EmbeddedAssets),
            gpui_platform::current_headless_renderer,
        );
        let registry = cx.update(|cx| {
            let registry = install(cx);
            cx.set_reduce_motion(true);
            registry
        });
        let mut gallery = None;
        let window = cx
            .open_window(size(px(560.), px(900.)), |_, cx| {
                let view = cx.new(|cx| Gallery::with_kind(Some(kind), cx));
                gallery = Some(view.clone());
                cx.new(|_| AutomationRoot::new(view))
            })
            .unwrap();
        let window: AnyWindowHandle = window.into();
        let gallery = gallery.unwrap();
        draw(&mut cx, window);
        draw(&mut cx, window);
        cx.capture_screenshot(window)
            .unwrap()
            .save(output.join(format!("{}-closed.png", kind.key())))
            .unwrap();
        let trigger = format!("liquid-{}-trigger", kind.key());
        if registry
            .snapshot(false)
            .elements
            .iter()
            .any(|e| e.id == trigger && e.visible)
        {
            action(&mut cx, window, &registry, &trigger);
        } else if registry
            .snapshot(false)
            .elements
            .iter()
            .any(|e| e.id == "liquid-demo" && e.visible)
        {
            action(&mut cx, window, &registry, "liquid-demo");
        }
        draw(&mut cx, window);
        draw(&mut cx, window);
        cx.capture_screenshot(window)
            .unwrap()
            .save(output.join(format!("{}-open.png", kind.key())))
            .unwrap();
        let state = gallery.read_with(&cx, |v, cx| v.inspect(cx));
        if matches!(
            kind,
            Kind::Attachments | Kind::Comments | Kind::Disclosure | Kind::Modal | Kind::Popover
        ) {
            assert_eq!(
                state["cards"][0]["open"], true,
                "{kind:?}: trigger did not open its component"
            );
        }
        if kind == Kind::Disclosure {
            let snapshot = registry.snapshot(false);
            let trigger = snapshot
                .elements
                .iter()
                .find(|e| e.id == "liquid-disclosure-trigger")
                .expect("expanded disclosure retains its header action");
            assert!(
                trigger.visible_bounds.height >= trigger.bounds.height - 0.5,
                "disclosure clipped its header action: {trigger:?}"
            );
        }
        assert!(
            state["cards"]
                .as_array()
                .unwrap()
                .iter()
                .all(|c| c["surfaces"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|s| s["error"].is_null() && s["paintError"] == false)),
            "{kind:?} render error: {state}"
        );
        std::fs::write(
            output.join(format!("{}-geometry.json", kind.key())),
            serde_json::to_vec_pretty(&registry.snapshot(false)).unwrap(),
        )
        .unwrap();
        results.push(state);
        if kind == Kind::Composer {
            let mut scenarios = Vec::new();
            for (index, enabled, stop, editable) in [
                (0, false, false, true),
                (1, true, false, true),
                (2, true, true, true),
                (3, false, true, true),
                (4, false, false, false),
                (5, false, false, true),
                (6, true, true, true),
                (7, true, false, true),
            ] {
                action(
                    &mut cx,
                    window,
                    &registry,
                    &format!("liquid-composer-variant-{index}"),
                );
                draw(&mut cx, window);
                let state = gallery.read_with(&cx, |v, cx| v.inspect(cx));
                let card = &state["cards"][0];
                let c = &card["composer"]["capabilities"];
                assert_eq!(c["enabled"], enabled);
                assert_eq!(c["stop"], stop);
                assert_eq!(c["editable"], editable);
                let controls = registry.snapshot(false);
                let send = controls
                    .elements
                    .iter()
                    .find(|e| e.id == "liquid-composer-send")
                    .unwrap();
                assert_eq!((send.bounds.width, send.bounds.height), (24., 24.));
                assert_eq!(send.enabled, enabled);
                cx.capture_screenshot(window)
                    .unwrap()
                    .save(output.join(format!("composer-scenario-{index}.png")))
                    .unwrap();
                scenarios.push(state);
            }
            action(&mut cx, window, &registry, "liquid-composer-variant-0");
            let input:UserAction=serde_json::from_value(json!({"type":"type_text","target":{"element_id":"liquid-composer-editor"},"text":"第一行\n第二行\n第三行\n第四行"})).unwrap();
            cx.update_window(window, |_, w, cx| headless_action(input, w, cx, &registry))
                .unwrap()
                .unwrap();
            draw(&mut cx, window);
            draw(&mut cx, window);
            let state = gallery.read_with(&cx, |v, cx| v.inspect(cx));
            assert_eq!(state["cards"][0]["inputHeight"], 60.);
            assert_eq!(
                state["cards"][0]["composer"]["text"],
                "第一行\n第二行\n第三行\n第四行"
            );
            action(&mut cx, window, &registry, "liquid-composer-send");
            draw(&mut cx, window);
            let state = gallery.read_with(&cx, |v, cx| v.inspect(cx));
            assert_eq!(state["cards"][0]["composer"]["text"], "");
            assert_eq!(
                state["cards"][0]["composer"]["events"][0],
                "send:第一行\n第二行\n第三行\n第四行:0"
            );
            std::fs::write(
                output.join("composer-scenarios.json"),
                serde_json::to_vec_pretty(&scenarios).unwrap(),
            )
            .unwrap();
        }
    }
    std::fs::write(
        output.join("results.json"),
        serde_json::to_vec_pretty(
            &json!({"backend":"GPUI offscreen Metal","controls":results,"errors":[]}),
        )
        .unwrap(),
    )
    .unwrap();
    println!(
        "PASS {} canonical native controls: {}",
        Kind::ALL.len(),
        output.display()
    );
}
fn run_benchmark(output: PathBuf, gestures: bool) {
    std::fs::create_dir_all(&output).unwrap();
    let mut rows = Vec::new();
    for count in [1, 2, 4, 8, 12, 14] {
        let mut cx = HeadlessAppContext::with_platform(
            gpui_platform::current_platform(true).text_system(),
            Arc::new(EmbeddedAssets),
            gpui_platform::current_headless_renderer,
        );
        let registry = cx.update(install);
        let mut gallery = None;
        let window = cx
            .open_window(size(px(1920.), px(2400.)), |_, cx| {
                let view = cx.new(Gallery::new);
                gallery = Some(view.clone());
                cx.new(|_| AutomationRoot::new(view))
            })
            .unwrap();
        let window: AnyWindowHandle = window.into();
        draw(&mut cx, window);
        action(&mut cx, window, &registry, "liquid-tab-4");
        action(&mut cx, window, &registry, &format!("liquid-count-{count}"));
        action(&mut cx, window, &registry, "liquid-cycle");
        let mut times = Vec::new();
        let mut frames = 0;
        let begin = Instant::now();
        let mut gesture = 0_u64;
        let mut next_gesture = Duration::ZERO;
        #[cfg(feature = "frame-profiler")]
        let mut draw_baseline = None;
        while begin.elapsed() < Duration::from_secs(6) {
            let command = if gestures && begin.elapsed() >= next_gesture {
                let phase = gesture % 3;
                let value = match phase {
                    0 => {
                        json!({"type":"click","target":{"element_id":format!("liquid-composer-departure-mode-{}", (gesture / 3) % 2)}})
                    }
                    1 => {
                        json!({"type":"click","target":{"element_id":"liquid-composer-editor"}})
                    }
                    _ => json!({"type":"click","target":{"element_id":"liquid-composer-send"}}),
                };
                gesture += 1;
                next_gesture =
                    begin.elapsed() + Duration::from_millis(if phase == 2 { 650 } else { 40 });
                Some((
                    serde_json::from_value::<UserAction>(value).unwrap(),
                    phase == 1,
                ))
            } else {
                None
            };
            let at = Instant::now();
            cx.update_window(window, |_, w, cx| {
                if let Some((command, commit_text)) = command {
                    headless_action(command, w, cx, &registry).unwrap();
                    if commit_text {
                        // One IME-style text commit, not N keystrokes and their
                        // forced intermediate draws counted as one frame.
                        assert!(w.dispatch_keystroke(
                            Keystroke {
                                modifiers: Modifiers::default(),
                                key: "unidentified".into(),
                                key_char: Some("发送气泡，保留共享输入与液态形变".into()),
                            },
                            cx
                        ));
                    }
                }
                w.simulate_next_frame(cx);
            })
            .unwrap();
            cx.run_until_parked();
            let elapsed = at.elapsed().as_secs_f64() * 1000.;
            if begin.elapsed() > Duration::from_secs(1) {
                times.push(elapsed);
                #[cfg(feature = "frame-profiler")]
                if draw_baseline.is_none() {
                    draw_baseline = Some(
                        cx.update_window(window, |_, w, _| w.frame_duration_snapshot())
                            .unwrap(),
                    );
                }
            }
            frames += 1;
            // This records actual app/render submissions. It is not a display FPS
            // claim; the headless backend has no physical presentation feedback.
            std::thread::sleep(Duration::from_millis(4));
        }
        times.sort_by(f64::total_cmp);
        let percentile =
            |p: f64| times[((times.len() as f64 * p).ceil() as usize).saturating_sub(1)];
        let state = gallery.unwrap().read_with(&cx, |v, cx| v.inspect(cx));
        let active = state["cards"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|card| {
                card["surfaces"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|s| s["visible"] == true)
            })
            .count();
        assert!(
            active >= count.min(12),
            "benchmark did not render its declared controls: count={count}, active={active}"
        );
        assert!(
            state["cards"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|c| c["frames"].as_u64().unwrap_or(0) > 100)
                .count()
                >= count.min(12),
            "benchmark lacked animation frame delivery"
        );
        if gestures {
            let composer = state["cards"]
                .as_array()
                .unwrap()
                .iter()
                .find(|c| c["kind"] == "composer")
                .unwrap();
            assert!(
                composer["departures"]["emitted"].as_u64().unwrap() >= 4,
                "benchmark did not emit send bubbles"
            );
            assert!(
                composer["composer"]["events"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|event| event
                        .as_str()
                        .is_some_and(|text| text.contains("发送气泡，保留共享输入与液态形变"))),
                "benchmark did not commit the declared IME payload: {composer}"
            );
        }
        #[cfg(feature = "frame-profiler")]
        let draw_stats = {
            let mut histogram = cx
                .update_window(window, |_, w, _| w.frame_duration_snapshot())
                .unwrap()
                .draw_duration_histogram;
            histogram
                .subtract(&draw_baseline.unwrap().draw_duration_histogram)
                .unwrap();
            assert!(histogram.len() > 100, "no individual draw samples");
            json!({"frames":histogram.len(),"p95Ms":histogram.value_at_quantile(0.95) as f64 / 1e6,
                "p99Ms":histogram.value_at_quantile(0.99) as f64 / 1e6,
                "scope":"Individual GPUI Window::draw CPU (layout/paint); input handling and platform submission excluded"})
        };
        #[cfg(not(feature = "frame-profiler"))]
        let draw_stats = Value::Null;
        rows.push(json!({"draw":draw_stats,"gestureCommands":gesture,"input":"one complete IME-style commit per send","count":count,"frames":frames,"p95Ms":percentile(0.95),"p99Ms":percentile(0.99),"scope":"Headless GPUI update/layout/paint submission; physical presentation excluded","state":state}));
    }
    std::fs::write(
        output.join("benchmark.json"),
        serde_json::to_vec_pretty(&rows).unwrap(),
    )
    .unwrap();
    println!(
        "{}",
        serde_json::to_string(
            &rows
                .iter()
                .map(|r| json!({"count":r["count"],"p95Ms":r["p95Ms"],"p99Ms":r["p99Ms"],"draw":r["draw"]}))
                .collect::<Vec<Value>>()
        )
        .unwrap()
    );
}
fn run_rebound(output: PathBuf) {
    std::fs::create_dir_all(&output).unwrap();
    let mut results = Vec::new();
    for kind in [Kind::Modal, Kind::Attachments] {
        let mut cx = HeadlessAppContext::with_platform(
            gpui_platform::current_platform(true).text_system(),
            Arc::new(EmbeddedAssets),
            gpui_platform::current_headless_renderer,
        );
        let registry = cx.update(install);
        let mut gallery = None;
        let window = cx
            .open_window(size(px(560.), px(900.)), |_, cx| {
                let view = cx.new(|cx| Gallery::with_kind(Some(kind), cx));
                gallery = Some(view.clone());
                cx.new(|_| AutomationRoot::new(view))
            })
            .unwrap();
        let window: AnyWindowHandle = window.into();
        let gallery = gallery.unwrap();
        draw(&mut cx, window);
        action(
            &mut cx,
            window,
            &registry,
            &format!("liquid-{}-trigger", kind.key()),
        );
        let start = Instant::now();
        while start.elapsed() < Duration::from_millis(800) {
            draw(&mut cx, window);
            std::thread::sleep(Duration::from_millis(4));
        }
        action(
            &mut cx,
            window,
            &registry,
            &format!("liquid-{}-close", kind.key()),
        );
        let start = Instant::now();
        let mut minimum = f64::INFINITY;
        let mut worst = Value::Null;
        let mut frames = 0;
        let mut rebound_image = None;
        let mut captured_height = None;
        while start.elapsed() < Duration::from_millis(850) {
            draw(&mut cx, window);
            let state = gallery.read_with(&cx, |v, cx| v.inspect(cx));
            let card = &state["cards"][0];
            let height = card["surfaces"][0]["pose"]["h"].as_f64().unwrap();
            if height < minimum {
                minimum = height;
                worst = state;
            }
            if height < 31. && rebound_image.is_none() {
                rebound_image = Some(cx.capture_screenshot(window).unwrap());
                captured_height = Some(height);
            }
            frames += 1;
            std::thread::sleep(Duration::from_millis(4));
        }
        if let Some(image) = rebound_image {
            image
                .save(output.join(format!("{}-rebound.png", kind.key())))
                .unwrap();
        }
        assert!(
            minimum < 31. && frames > 40,
            "missing real rebound: {kind:?}, {minimum}, {frames}"
        );
        results
            .push(json!({"kind":kind.key(),"minimumHeight":minimum,"capturedHeight":captured_height,"frames":frames,"state":worst}));
    }
    std::fs::write(
        output.join("results.json"),
        serde_json::to_vec_pretty(&results).unwrap(),
    )
    .unwrap();
    println!("PASS live native rebound and shared clipping");
}
fn main() {
    let args = std::env::args().collect::<Vec<_>>();
    let option = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .map(PathBuf::from)
    };
    if let Some(out) = option("--headless") {
        run_headless(out);
        return;
    }
    if let Some(out) = option("--benchmark") {
        run_benchmark(out, false);
        return;
    }
    if let Some(out) = option("--motion-benchmark") {
        run_benchmark(out, true);
        return;
    }
    if let Some(out) = option("--rebound") {
        run_rebound(out);
        return;
    }
    gpui_platform::application()
        .with_assets(EmbeddedAssets)
        .run(|cx| {
            install(cx);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                        None,
                        size(px(1280.), px(920.)),
                        cx,
                    ))),
                    ..Default::default()
                },
                |w, cx| {
                    w.on_window_should_close(cx, |_, cx| {
                        cx.quit();
                        true
                    });
                    let view = cx.new(Gallery::new);
                    cx.new(|_| AutomationRoot::new(view))
                },
            )
            .unwrap();
            cx.activate(true);
        });
}
