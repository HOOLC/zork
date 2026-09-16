//! Pixel evidence for the production hover and active surfaces across section gaps.
use gpui::{div, prelude::*, px, rgb, AppContext, Context, HeadlessAppContext, Window};
use serde_json::json;
use std::{
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};
use zork_ui::{
    assets::EmbeddedAssets,
    automation::{
        driver::headless_action,
        element::{AutomationRegistry, AutomationRegistryGlobal},
        AutomationRoot,
    },
};
use zork_ui::{
    automation::{AutomationElementExt, AutomationRole},
    navigation::TabGroup,
};

const ROWS: [f32; 4] = [20., 54., 180., 654.];

struct Fixture {
    tabs: TabGroup,
    selected: usize,
}
impl Render for Fixture {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().bg(rgb(0xffffff)).child(
            self.tabs.surface(
                div().size_full().child(
                    div()
                        .relative()
                        .left(px(20.))
                        .w(px(240.))
                        .h_full()
                        .children(ROWS.iter().enumerate().map(|(i, y)| {
                            self.tabs
                                .tab(format!("slide-tab-{i}"), self.selected == i)
                                .absolute()
                                .top(px(*y))
                                .w_full()
                                .child(format!("标签 {}", i + 1))
                                .on_click(cx.listener(move |v, _, _, cx| {
                                    v.selected = i;
                                    cx.notify();
                                }))
                                .automation(AutomationRole::Button, format!("标签 {}", i + 1))
                        })),
                ),
            ),
        )
    }
}

pub fn run(output: &Path, baseline: bool) -> anyhow::Result<()> {
    let mut reports = Vec::new();
    for width in [400., 800.] {
        let mut cx = HeadlessAppContext::with_platform(
            gpui_platform::current_platform(true).text_system(),
            Arc::new(EmbeddedAssets),
            gpui_platform::current_headless_renderer,
        );
        let driver = cx.update(|cx| {
            zork_ui::assets::init_fonts(cx);
            zork_ui::components::init(cx);
            cx.set_reduce_motion(true);
            {
                let registry = AutomationRegistry::new();
                cx.set_global(AutomationRegistryGlobal(registry.clone()));
                registry
            }
        });
        let window = cx.open_window(gpui::size(px(width), px(720.)), |_, cx| {
            let fixture = cx.new(|cx| Fixture {
                tabs: TabGroup::new(cx),
                selected: 0,
            });
            cx.new(|_| AutomationRoot::new(fixture))
        })?;
        let tick = |cx: &mut HeadlessAppContext| -> anyhow::Result<(usize, f64)> {
            let delta = Duration::from_nanos(8_333_333);
            std::thread::sleep(delta);
            cx.advance_clock(delta);
            cx.run_until_parked();
            let began = Instant::now();
            let callbacks = cx.update_window(window.into(), |_, w, cx| {
                let pending = w.simulate_next_frame(cx);
                pending
            })?;
            Ok((callbacks, began.elapsed().as_secs_f64() * 1000.))
        };
        let action =
            |cx: &mut HeadlessAppContext, kind: &str, index: usize| -> anyhow::Result<()> {
                cx.update_window(window.into(), |_, w, cx| {
                    headless_action(
                    serde_json::from_value(
                        json!({"type":kind,"target":{"element_id":format!("slide-tab-{index}")}}),
                    )?,
                    w,
                    cx,
                    &driver,
                ).map_err(|error| anyhow::anyhow!("{}: {}", error.code, error.message))
                })??;
                Ok(())
            };
        for _ in 0..3 {
            tick(&mut cx)?;
        }
        let scale = driver.snapshot(false).scale_factor;
        let first = driver
            .snapshot(false)
            .elements
            .into_iter()
            .find(|e| e.id == "slide-tab-0")
            .unwrap()
            .bounds;
        let sample = |image: &image::RgbaImage, active: bool| -> anyhow::Result<f32> {
            let (x, color) = if active {
                (first.x + 3., zork_ui::design::BRAND_ACCENT)
            } else {
                (
                    first.x + first.width - 12.,
                    zork_ui::design::INTERACTION.neutral_hover,
                )
            };
            let color = color.to_be_bytes();
            (0..image.height())
                .find(|y| image.get_pixel((x * scale) as u32, *y).0[..3] == color[1..])
                .map(|y| y as f32 / scale)
                .ok_or_else(|| {
                    anyhow::anyhow!("missing {} pixels", if active { "active" } else { "hover" })
                })
        };
        for active in [false, true] {
            let kind = if active { "active" } else { "hover" };
            for index in 1..ROWS.len() {
                cx.update(|cx| cx.set_reduce_motion(true));
                action(&mut cx, "click", 0)?;
                action(&mut cx, "move", 0)?;
                tick(&mut cx)?;
                let initial = cx.capture_screenshot(window.into())?;
                let from = sample(&initial, active)?;
                let distance = ROWS[index] - ROWS[0];
                let target = from + distance;
                let other_before = sample(&initial, !active)?;
                cx.update(|cx| cx.set_reduce_motion(false));
                let began = Instant::now();
                action(&mut cx, if active { "click" } else { "move" }, index)?;
                let mut frames = Vec::new();
                let mut costs = Vec::new();
                let mut screenshots = Vec::new();
                for frame in 0..240 {
                    let (callbacks, cost) = tick(&mut cx)?;
                    costs.push(cost);
                    let image = cx.capture_screenshot(window.into())?;
                    let top = sample(&image, active)?;
                    let elapsed = began.elapsed().as_secs_f64() * 1000.;
                    frames.push(json!({"ms":elapsed,"top":top}));
                    anyhow::ensure!(
                        top >= from - 0.5 && top <= target + 0.5,
                        "{kind} overshot a resting target"
                    );
                    if !active {
                        anyhow::ensure!(
                            sample(&image, true)? == other_before,
                            "hover moved the active indicator"
                        );
                    }
                    if frame == 1 || (top - target).abs() < 0.5 && callbacks == 0 {
                        screenshots.push((format!("{width}-{kind}-{distance}-{frame}.png"), image));
                    }
                    if (top - target).abs() < 0.5 && callbacks == 0 {
                        break;
                    }
                }
                let settled_ms = frames.last().unwrap()["ms"].as_f64().unwrap();
                anyhow::ensure!(
                    (frames.last().unwrap()["top"].as_f64().unwrap() - target as f64).abs() < 0.5,
                    "{kind} never settled"
                );
                if !baseline {
                    anyhow::ensure!(
                        settled_ms < 360.,
                        "{distance}px {kind} took {settled_ms:.0}ms"
                    );
                    anyhow::ensure!(
                        frames
                            .iter()
                            .any(|f| f["top"].as_f64().unwrap() > from as f64 + 1.
                                && f["top"].as_f64().unwrap() < target as f64 - 1.),
                        "{kind} teleported"
                    );
                }
                costs.sort_by(f64::total_cmp);
                let p95 = costs[((costs.len() - 1) as f64 * 0.95).round() as usize];
                let p99 = costs[((costs.len() - 1) as f64 * 0.99).round() as usize];
                anyhow::ensure!(
                    p95 <= 1000. / 120.,
                    "slide exceeded CPU frame budget: {p95}"
                );
                for (name, image) in screenshots {
                    image.save(output.join(name))?;
                }
                reports.push(json!({"width":width,"surface":kind,"distance":distance,"settled_ms":settled_ms,"p95_draw_ms":p95,"p99_draw_ms":p99,"frames":frames}));
            }
        }
        cx.update(|cx| cx.set_reduce_motion(true));
        action(&mut cx, "click", 0)?;
        tick(&mut cx)?;
        let marker = sample(&cx.capture_screenshot(window.into())?, true)?;
        action(&mut cx, "click", 3)?;
        tick(&mut cx)?;
        anyhow::ensure!(
            (sample(&cx.capture_screenshot(window.into())?, true)? - marker - (ROWS[3] - ROWS[0]))
                .abs()
                < 0.5,
            "reduced motion did not snap"
        );
        for _ in 0..3 {
            tick(&mut cx)?;
        }
        anyhow::ensure!(
            cx.update_window(window.into(), |_, w, cx| w.simulate_next_frame(cx))? == 0,
            "settled tabs keep scheduling frames"
        );
    }
    std::fs::write(
        output.join("distance-motion.json"),
        serde_json::to_vec_pretty(&json!({"baseline":baseline,"checks":reports}))?,
    )?;
    println!("PASS near/medium/far active and hover, reduced motion, idle and CPU budget");
    Ok(())
}
