use gpui::{div, prelude::*, px, AppContext, Context, HeadlessAppContext, Window};
use serde_json::json;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use zork_gui::{
    assets::EmbeddedAssets,
    automation::{AutomationElementExt, AutomationRole, AutomationRoot, HeadlessAutomation},
};
use zork_ui::{
    components::{
        collapse::Collapse,
        region::{self, Regions},
    },
    navigation::TabGroup,
};

struct Fixture {
    tabs: TabGroup,
    regions: Regions<Self>,
    expanded: bool,
    rows: usize,
    width: f32,
    child_focus: gpui::FocusHandle,
    header_focus: Option<gpui::FocusHandle>,
    built: usize,
}
impl Render for Fixture {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = div()
            .font_family("Inter Variable")
            .size_full()
            .flex()
            .flex_col()
            .child(
                self.regions
                    .auto_height("fold", self.width, cx, |v, window, cx| {
                        let fold = Collapse::new("fixture-fold", v.expanded, v.width, window, cx);
                        let focus = fold.header_focus(cx);
                        v.header_focus = Some(focus.clone());
                        let interactive = fold.interactive(cx);
                        let body = fold.mounted(cx).then(|| {
                            v.built += v.rows;
                            v.tabs
                                .column()
                                .pt(px(2.))
                                .children((0..v.rows).map(|i| {
                                    v.tabs
                                        .tab(format!("fold-row-{i}"), false)
                                        .tab_stop(interactive)
                                        .when(i == 0, |row| {
                                            row.track_focus(
                                                &v.child_focus.clone().tab_stop(interactive),
                                            )
                                        })
                                        .child(format!("任务 {i}"))
                                        .automation(AutomationRole::Button, format!("Task {i}"))
                                }))
                                .into_any_element()
                        });
                        let owner = cx.entity().downgrade();
                        div()
                            .flex()
                            .flex_col()
                            .child(
                                v.tabs
                                    .tab("fold-header".into(), false)
                                    .track_focus(&focus)
                                    .child("设备")
                                    .on_click(cx.listener(|v, _, _, cx| {
                                        v.expanded = !v.expanded;
                                        region::invalidate(cx, &["fold"]);
                                    }))
                                    .automation(AutomationRole::Button, "Device"),
                            )
                            .child(fold.element(
                                body,
                                move |_, cx| {
                                    let _ =
                                        owner.update(cx, |_, cx| region::invalidate(cx, &["fold"]));
                                },
                                cx,
                            ))
                            .into_any_element()
                    }),
            )
            .child(
                self.tabs
                    .tab("following-tab".into(), false)
                    .child("下一台设备")
                    .automation(AutomationRole::Button, "Next device"),
            );
        div()
            .size_full()
            .bg(gpui::rgb(zork_ui::design::ZORK_UI.palette.sidebar))
            .child(self.tabs.surface(content))
    }
}

pub fn run(output: &std::path::Path) -> anyhow::Result<()> {
    let mut reports = Vec::new();
    for (width, rows) in [(220., 2), (400., 12)] {
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
        let mut fixture = None;
        let window = cx.open_window(gpui::size(px(width), px(700.)), |_, cx| {
            let view = cx.new(|cx| Fixture {
                tabs: TabGroup::new(cx),
                regions: Regions::default(),
                expanded: true,
                rows,
                width,
                child_focus: cx.focus_handle().tab_stop(true),
                header_focus: None,
                built: 0,
            });
            fixture = Some(view.clone());
            cx.new(|_| AutomationRoot::new(view))
        })?;
        let fixture = fixture.unwrap();
        let pump = |cx: &mut HeadlessAppContext, n: usize| -> anyhow::Result<Vec<f64>> {
            let mut samples = Vec::new();
            for _ in 0..n {
                std::thread::sleep(Duration::from_millis(16));
                let start = Instant::now();
                cx.advance_clock(Duration::from_millis(16));
                cx.run_until_parked();
                cx.update_window(window.into(), |_, w, cx| {
                    w.simulate_next_frame(cx);
                })?;
                samples.push(start.elapsed().as_secs_f64() * 1000.);
            }
            Ok(samples)
        };
        let bottom = || {
            driver
                .snapshot(false)
                .elements
                .iter()
                .find(|e| e.id == "following-tab")
                .unwrap()
                .bounds
                .y
        };
        let click = |cx: &mut HeadlessAppContext| -> anyhow::Result<()> {
            cx.update_window(window.into(), |_, w, cx| {
                driver.dispatch(
                    serde_json::from_value(
                        json!({"type":"click","target":{"element_id":"fold-header"}}),
                    )
                    .unwrap(),
                    w,
                    cx,
                )
            })??;
            Ok(())
        };
        pump(&mut cx, 4)?;
        let full = bottom();
        anyhow::ensure!(
            (full - (32. + rows as f32 * 34.)).abs() < 0.1,
            "natural height wrong: {full}"
        );
        cx.capture_screenshot(window.into())?
            .save(output.join(format!("fold-{width}-open.png")))?;
        // Keyboard/programmatic closure must return focus from a disappearing child.
        cx.update_window(window.into(), |_, w, cx| {
            fixture.read(cx).child_focus.clone().focus(w, cx)
        })?;
        fixture.update(&mut cx, |v, cx| {
            v.expanded = false;
            region::invalidate(cx, &["fold"]);
        });
        let mut samples = pump(&mut cx, 2)?;
        let closing = bottom();
        anyhow::ensure!(
            closing > 32. && closing < full,
            "collapse jumped: {full} -> {closing}"
        );
        if rows == 2 {
            anyhow::ensure!(
                closing > 32. + (full - 32.) * 0.5,
                "short disclosure disappeared before its start ramp was visible: {closing}"
            );
        }
        cx.update_window(window.into(), |_, w, cx| {
            assert!(fixture
                .read(cx)
                .header_focus
                .as_ref()
                .unwrap()
                .is_focused(w));
        })?;
        cx.capture_screenshot(window.into())?
            .save(output.join(format!("fold-{width}-closing.png")))?;
        click(&mut cx)?;
        samples.extend(pump(&mut cx, 2)?);
        let reverse = bottom();
        anyhow::ensure!(
            reverse > closing && reverse <= full,
            "reverse lost current height: {closing} -> {reverse}"
        );
        samples.extend(pump(&mut cx, 40)?);
        anyhow::ensure!((bottom() - full).abs() < 0.1, "expand failed to settle");
        click(&mut cx)?;
        samples.extend(pump(&mut cx, 45)?);
        anyhow::ensure!(bottom() == 32., "collapsed gap remains: {}", bottom());
        anyhow::ensure!(
            !driver
                .snapshot(true)
                .elements
                .iter()
                .any(|e| e.id.starts_with("fold-row-")),
            "closed children remained mounted"
        );
        let built = fixture.read_with(&cx, |v, _| v.built);
        pump(&mut cx, 5)?;
        anyhow::ensure!(
            fixture.read_with(&cx, |v, _| v.built) == built,
            "closed body rebuilt at idle"
        );
        let pending = cx.update_window(window.into(), |_, w, cx| w.simulate_next_frame(cx))?;
        anyhow::ensure!(pending == 0, "idle fold still schedules frames: {pending}");
        cx.capture_screenshot(window.into())?
            .save(output.join(format!("fold-{width}-closed.png")))?;
        click(&mut cx)?;
        pump(&mut cx, 2)?;
        let before_update = bottom();
        fixture.update(&mut cx, |v, cx| {
            v.rows += 4;
            region::invalidate(cx, &["fold"]);
        });
        pump(&mut cx, 2)?;
        anyhow::ensure!(
            bottom() > before_update && bottom() < full + 136.,
            "content update jumped"
        );
        pump(&mut cx, 55)?;
        anyhow::ensure!(
            (bottom() - full - 136.).abs() < 0.1,
            "updated height wrong: {}",
            bottom()
        );
        cx.update(|cx| cx.set_reduce_motion(true));
        click(&mut cx)?;
        pump(&mut cx, 2)?;
        anyhow::ensure!(bottom() == 32., "reduced collapse animated");
        click(&mut cx)?;
        pump(&mut cx, 2)?;
        anyhow::ensure!(
            (bottom() - full - 136.).abs() < 0.1,
            "reduced expansion animated"
        );
        cx.update_window(window.into(), |_, w, cx| {
            w.focus_next(cx);
            assert!(
                fixture.read(cx).child_focus.is_focused(w),
                "reduced expansion did not restore child tab stops"
            );
        })?;
        samples.sort_by(f64::total_cmp);
        let p95 = samples[samples.len() * 95 / 100];
        let p99 = samples[samples.len() * 99 / 100];
        anyhow::ensure!(p95 < 8.33, "fold CPU frames exceeded budget: {p95}");
        reports.push(json!({"width":width,"rows":rows,"full":full,"closing":closing,"reverse":reverse,"idle_callbacks":pending,"p95_cpu_ms":p95,"p99_cpu_ms":p99,"frames":samples.len()}));
    }
    std::fs::write(
        output.join("collapse.json"),
        serde_json::to_vec_pretty(&reports)?,
    )?;
    production_sidebar(output)?;
    println!("PASS fold: intrinsic geometry, reversal, dynamic content, clipping, focus return, unmount, reduced motion, idle and CPU frames");
    Ok(())
}

fn production_sidebar(output: &std::path::Path) -> anyhow::Result<()> {
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
        cx.set_reduce_motion(false);
        HeadlessAutomation::install(cx)
    });
    let window = cx.open_window(gpui::size(px(900.), px(600.)), |_, cx| {
        let root =
            cx.new(|cx| zork_gui::views::RootView::render_benchmark_fixture(false, store, cx));
        cx.new(|_| AutomationRoot::new(root))
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
    let leader = || {
        driver
            .snapshot(false)
            .elements
            .into_iter()
            .find(|e| e.id == "leader-mini1-leader")
    };
    let toggle = |cx: &mut HeadlessAppContext| -> anyhow::Result<()> {
        cx.update_window(window.into(), |_, w, cx| {
            driver.dispatch(
                serde_json::from_value(
                    json!({"type":"click", "target":{"element_id":"device-mini1"}}),
                )
                .unwrap(),
                w,
                cx,
            )
        })??;
        Ok(())
    };
    pump(&mut cx, 4)?;
    let full = leader().expect("production leader missing").bounds;
    toggle(&mut cx)?;
    pump(&mut cx, 1)?;
    anyhow::ensure!(
        leader().is_some(),
        "production sidebar dropped its child on the first closing frame"
    );
    cx.capture_screenshot(window.into())?
        .save(output.join("production-closing.png"))?;
    pump(&mut cx, 12)?;
    anyhow::ensure!(
        leader().is_none(),
        "production sidebar failed to finish closing"
    );
    toggle(&mut cx)?;
    pump(&mut cx, 12)?;
    let restored = leader().expect("production leader did not return").bounds;
    anyhow::ensure!(
        restored == full,
        "production sidebar did not restore its geometry"
    );
    cx.capture_screenshot(window.into())?
        .save(output.join("production-open.png"))?;
    std::fs::write(
        output.join("production.json"),
        serde_json::to_vec_pretty(
            &json!({"first_closing_frame_retains_child":true,"restored_bounds":restored}),
        )?,
    )?;
    Ok(())
}
