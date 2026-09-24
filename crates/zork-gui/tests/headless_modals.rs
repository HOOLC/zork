use anyhow::Context as _;
use gpui::{
    div, prelude::*, px, rgb, AppContext, Context, Entity, HeadlessAppContext, Render, Window,
    WindowHandle,
};
use serde_json::json;
use std::{path::PathBuf, sync::Arc};
use zork_gui::{
    assets::EmbeddedAssets,
    automation::{
        protocol::{ElementInfo, UserAction},
        AutomationRoot, HeadlessAutomation,
    },
    desktop::HeadlessProfilesView,
};

struct SettingsFrame<V: Render + 'static> {
    inner: Entity<V>,
}
impl<V: Render + 'static> Render for SettingsFrame<V> {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .font_family("Inter Variable")
            .text_size(px(13.))
            .bg(rgb(0xF7F6F2))
            .child(
                div()
                    .id("settings-scroll")
                    .ml(px(240.))
                    .h_full()
                    .overflow_y_scroll()
                    .bg(rgb(0xFFFFFF))
                    .child(zork_gui::desktop::headless_settings_content(
                        self.inner.clone(),
                    )),
            )
    }
}
struct Fixture<V: Render + 'static> {
    view: Entity<V>,
    cx: HeadlessAppContext,
    window: WindowHandle<AutomationRoot<SettingsFrame<V>>>,
    driver: HeadlessAutomation,
}
impl<V: Render + 'static> Fixture<V> {
    fn new(
        width: f32,
        height: f32,
        make: impl FnOnce(&mut Context<V>) -> V,
    ) -> anyhow::Result<Self> {
        let mut cx = HeadlessAppContext::with_platform(
            gpui_platform::current_platform(true).text_system(),
            Arc::new(EmbeddedAssets),
            gpui_platform::current_headless_renderer,
        );
        let driver = cx.update(|cx| {
            zork_gui::assets::init_fonts(cx);
            zork_gui::components::init(cx);
            // Layout/focus contracts inspect endpoints; test_motion.py samples animated frames.
            cx.set_reduce_motion(true);
            HeadlessAutomation::install(cx)
        });
        let mut view = None;
        let window = cx.open_window(gpui::size(px(width), px(height)), |_, cx| {
            let inner = cx.new(make);
            view = Some(inner.clone());
            let frame = cx.new(|_| SettingsFrame { inner });
            cx.new(|_| AutomationRoot::new(frame))
        })?;
        cx.update_window(window.into(), |_, w, cx| w.draw(cx).clear(cx))?;
        cx.run_until_parked();
        Ok(Self {
            cx,
            window,
            view: view.unwrap(),
            driver,
        })
    }
    fn element(&self, id: &str) -> Option<ElementInfo> {
        self.driver
            .snapshot(false)
            .elements
            .into_iter()
            .find(|e| e.id == id)
    }
    fn action(&mut self, value: serde_json::Value) -> anyhow::Result<()> {
        let action: UserAction = serde_json::from_value(value)?;
        self.cx.update_window(self.window.into(), |_, w, cx| {
            self.driver.dispatch(action, w, cx)
        })??;
        // Measured shared controls publish their layout on the following frame.
        // Drive the same frame boundary before inspecting reduced-motion endpoints.
        for _ in 0..4 {
            self.cx.advance_clock(std::time::Duration::from_millis(16));
            self.cx.run_until_parked();
            self.cx.update_window(self.window.into(), |_, w, cx| {
                w.simulate_next_frame(cx);
                w.draw(cx).clear(cx);
            })?;
        }
        Ok(())
    }
    fn click(&mut self, id: &str) -> anyhow::Result<()> {
        self.action(json!({"type":"click","target":{"element_id":id}}))
    }
    fn key(&mut self, key: &str) -> anyhow::Result<()> {
        self.action(json!({"type":"key","keystroke":key}))
    }
    fn modal(&mut self, id: &str, width: f32, height: f32) -> anyhow::Result<()> {
        let card = self
            .element(id)
            .ok_or_else(|| anyhow::anyhow!("missing modal {id}"))?;
        anyhow::ensure!(
            card.bounds.x >= 19.
                && card.bounds.y >= 19.
                && card.bounds.x + card.bounds.width <= width - 19.
                && card.bounds.y + card.bounds.height <= height - 19.,
            "modal outside viewport: {:?}",
            card.bounds
        );
        anyhow::ensure!(
            card.bounds == card.visible_bounds,
            "modal was clipped by settings scroll container"
        );
        anyhow::ensure!(
            (card.bounds.x + card.bounds.width / 2. - width / 2.).abs() < 1.
                && (card.bounds.y + card.bounds.height / 2. - height / 2.).abs() < 1.,
            "dialog centered in its host instead of the window: {:?}",
            card.bounds
        );
        let pixels = self.cx.capture_screenshot(self.window.into())?;
        for (x, y) in [
            (2, 2),
            (pixels.width() - 3, 2),
            (2, pixels.height() - 3),
            (pixels.width() - 3, pixels.height() - 3),
        ] {
            anyhow::ensure!(
                pixels.get_pixel(x, y).0[..3]
                    .iter()
                    .all(|channel| *channel < 180),
                "modal backdrop did not cover window corner ({x}, {y})"
            );
        }
        anyhow::ensure!(
            [
                zork_ui::controls::DIALOG_CONFIRM_WIDTH,
                zork_ui::controls::DIALOG_FORM_WIDTH,
                zork_ui::controls::DIALOG_STEP_WIDTH,
                zork_ui::controls::DIALOG_RICH_WIDTH,
            ]
            .iter()
            .any(|tier| (card.bounds.width - tier).abs() < 1.),
            "modal width {} is not one of the shared width tiers",
            card.bounds.width
        );
        let footer = self
            .element(&format!("{id}-footer"))
            .expect("modal must keep a fixed action area");
        if footer.bounds != footer.visible_bounds
            || footer.bounds.y + footer.bounds.height > card.bounds.y + card.bounds.height
        {
            self.screenshot(&format!("{id}-{width}-clipped.png"))?;
        }
        anyhow::ensure!(
            footer.bounds == footer.visible_bounds
                && footer.bounds.y + footer.bounds.height <= card.bounds.y + card.bounds.height,
            "modal actions are clipped: card={:?}, footer={:?}, visible={:?}",
            card.bounds,
            footer.bounds,
            footer.visible_bounds
        );
        Ok(())
    }
    fn screenshot(&mut self, name: &str) -> anyhow::Result<()> {
        let out = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../artifacts/headless-interactions/modals");
        std::fs::create_dir_all(&out)?;
        // Wait for stable pixels without advancing the virtual clock, which
        // would animate the text caret.
        let started = std::time::Instant::now();
        let mut previous = None;
        let mut stable = 0;
        loop {
            self.cx.run_until_parked();
            self.cx
                .update_window(self.window.into(), |_, w, cx| w.draw(cx).clear(cx))?;
            let pixels = self.cx.capture_screenshot(self.window.into())?;
            let snapshot = self.driver.snapshot(false);
            if previous.as_ref() == Some(pixels.as_raw()) {
                stable += 1;
            } else {
                stable = 0;
            }
            previous = Some(pixels.as_raw().clone());
            if stable >= 2 && started.elapsed().as_millis() >= 100 {
                pixels.save(out.join(name))?;
                std::fs::write(
                    out.join(name.replace(".png", ".json")),
                    serde_json::to_vec_pretty(&snapshot)?,
                )?;
                break;
            }
            if started.elapsed().as_secs() >= 5 {
                pixels.save(out.join(format!("failed-{name}")))?;
                std::fs::write(
                    out.join(format!("failed-{name}.json")),
                    serde_json::to_vec_pretty(&snapshot)?,
                )?;
                anyhow::bail!("screenshot did not settle: {name} (stable={stable})");
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        Ok(())
    }
}
fn main() -> anyhow::Result<()> {
    if std::env::args().any(|arg| arg == "--paint-nodes") {
        return paint_node_checks();
    }
    if std::env::args().any(|arg| arg == "--enrollment") {
        return enrollment_checks();
    }
    if std::env::args().any(|arg| arg == "--data-reset") {
        return data_reset_checks();
    }
    std::env::set_var("SEED", "0");
    let state = tempfile::tempdir()?;
    std::env::set_var(
        "ZORK_GUI_PREFERENCES_PATH",
        state.path().join("preferences.json"),
    );
    for (width, height) in [(900., 600.), (1280., 800.)] {
        let mut f = Fixture::new(width, height, |cx| {
            HeadlessProfilesView::headless_fixture(false, cx)
        })?;
        anyhow::ensure!(
            f.element("profile-create-dialog").is_none(),
            "create form opened without user action"
        );
        let opener = f.element("profile-add").unwrap().center;
        f.click("profile-add")?;
        f.modal("profile-create-dialog", width, height)?;
        f.screenshot(&format!("connection-{width}.png"))?;
        anyhow::ensure!(
            f.element("profile-provider-card-0").is_some(),
            "step 1 did not list providers"
        );
        f.click("profile-next")?;
        anyhow::ensure!(
            f.element("profile-signin").is_some() || f.element("profile-key").is_some(),
            "step 2 did not ask for a sign-in or key"
        );
        f.screenshot(&format!("connection-step-2-{width}.png"))?;
        f.key("escape")?;
        anyhow::ensure!(
            f.element("profile-create-dialog").is_none(),
            "Escape did not dismiss modal"
        );
        f.click("profile-add")?;
        f.action(json!({"type":"click","target":{"x":opener.x,"y":opener.y}}))?;
        anyhow::ensure!(
            f.element("profile-create-dialog").is_none(),
            "backdrop click leaked to underlying add button"
        );
        let mut f = Fixture::new(width, height, |cx| {
            HeadlessProfilesView::headless_fixture(true, cx)
        })?;
        f.click("profile-model-add")?;
        f.modal("model-editor-dialog", width, height)?;
        f.click("profile-model")?;
        f.action(json!({"type":"type_text","text":"copied-model"}))?;
        f.click("model-params-toggle")?;
        f.click("model-copy-select")?;
        f.click("model-copy-0")?;
        anyhow::ensure!(
            f.element("model-copy-select-menu").is_none(),
            "copy menu remained open after choosing a model"
        );
        let state = f.view.read_with(&f.cx, |view, cx| view.headless_state(cx));
        anyhow::ensure!(
            state["model_id"] == "copied-model",
            "copy replaced the new model identity"
        );
        anyhow::ensure!(
            state["context_window"] == "32K" && state["max_output_tokens"] == "4.096K",
            "copy did not populate token limits: {state}"
        );
        let card = f.element("model-editor-dialog").unwrap().bounds;
        for id in [
            "model-copy-select",
            "profile-model-cancel",
            "profile-model-save",
        ] {
            let action = f.element(id).unwrap();
            anyhow::ensure!(
                action.bounds == action.visible_bounds
                    && action.bounds.x >= card.x + 24.
                    && action.bounds.x + action.bounds.width <= card.x + card.width - 24.,
                "{id} overflowed the model dialog: {:?}",
                action.bounds
            );
        }
        f.screenshot(&format!("model-{width}.png"))?;
        f.click("model-editor-dialog-close")?;
        anyhow::ensure!(
            f.element("model-editor-dialog").is_none(),
            "close button did not dismiss model editor"
        );
        f.click("model-edit-fixture-model")?;
        anyhow::ensure!(
            f.element("model-inline-editor").is_some(),
            "editing a model did not expand in place: {:?}",
            f.driver
                .snapshot(false)
                .elements
                .iter()
                .filter(|e| e.id.starts_with("model") || e.id.starts_with("profile-model"))
                .map(|e| (&e.id, e.visible))
                .collect::<Vec<_>>()
        );
        f.screenshot(&format!("model-inline-{width}.png"))?;
        f.key("escape")?;
    }
    paint_node_checks()?;
    enrollment_checks()?;
    println!(
        "Headless modal opening, clipping, input, Escape, close button and backdrop checks passed at both sizes."
    );
    Ok(())
}

struct PaintNodeFrame {
    node: gpui::PaintNode,
    copies: usize,
    gpu: bool,
    nested: bool,
    alpha: f32,
    snapshot: std::rc::Rc<std::cell::RefCell<Option<gpui::PaintSnapshot>>>,
    region: std::rc::Rc<std::cell::RefCell<Option<gpui::PaintRegion>>>,
}

impl Render for PaintNodeFrame {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        use gpui::*;
        window.enable_gpu_layers(self.gpu);
        let (node, copies, nested, alpha) = (self.node, self.copies, self.nested, self.alpha);
        let snapshot = self.snapshot.clone();
        let region = self.region.clone();
        canvas(
            |_, _, _| {},
            move |bounds, _, window, cx| {
                for row in 0..16 {
                    for col in 0..24 {
                        let cell = Bounds::new(
                            bounds.origin + point(px(col as f32 * 16.), px(row as f32 * 16.)),
                            size(px(16.), px(16.)),
                        );
                        window.paint_quad(fill(
                            cell,
                            rgb(if (row + col) % 2 == 0 {
                                0xffffff
                            } else {
                                0x7e9ca8
                            }),
                        ));
                    }
                }
                let source = Bounds::new(
                    bounds.origin + point(px(32.5), px(40.5)),
                    size(px(280.), px(70.)),
                );
                let (_, picture) = window.capture_paint_snapshot(0x501, source, None, |window| {
                    window.paint_quad(quad(
                        source,
                        px(24.),
                        rgba(0xf7672a80),
                        px(1.),
                        rgba(0x24313b80),
                        gpui::BorderStyle::Solid,
                    ));
                    let label = "半透明 text + icon";
                    let line = window.text_system().shape_line(
                        label.into(),
                        px(16.),
                        &[TextRun {
                            len: label.len(),
                            font: font("Inter Variable"),
                            color: rgba(0x152b4580).into(),
                            background_color: None,
                            underline: None,
                            strikethrough: None,
                        }],
                        None,
                    );
                    line.paint(
                        source.origin + point(px(16.), px(20.)),
                        px(24.),
                        TextAlign::Left,
                        None,
                        window,
                        cx,
                    )
                    .unwrap();
                });
                *snapshot.borrow_mut() = Some(picture.clone());
                let paint = |window: &mut Window| {
                    window.with_scaled_alpha_paint_clip(
                        point(px(0.), px(0.)),
                        1.,
                        alpha,
                        &[bounds],
                        |window| {
                            if copies == 0 {
                                window.paint_snapshot(&picture);
                            } else {
                                window.paint_node(node, &picture);
                            }
                        },
                    );
                };
                if nested {
                    window.with_retained_paint(0x503, bounds, paint);
                } else {
                    paint(window);
                }
                let (_, recorded) = window.record_paint_region(|window| {
                    for _ in 1..copies {
                        if nested {
                            window.with_retained_paint(0x502, bounds, paint);
                        } else {
                            paint(window);
                        }
                    }
                });
                *region.borrow_mut() = recorded;
            },
        )
        .w(px(384.))
        .h(px(256.))
    }
}

fn paint_node_checks() -> anyhow::Result<()> {
    for gpu in [false, true] {
        let mut f = Fixture::new(900., 600., |_| PaintNodeFrame {
            node: Default::default(),
            copies: 0,
            gpu,
            nested: false,
            alpha: 1.,
            snapshot: Default::default(),
            region: Default::default(),
        })?;
        for (alpha, nested) in [0., 0.35, 1.]
            .into_iter()
            .flat_map(|alpha| [false, true].map(|nested| (alpha, nested)))
        {
            f.view.update(&mut f.cx, |view, cx| {
                view.copies = 0;
                view.alpha = alpha;
                view.nested = nested;
                cx.notify();
            });
            f.cx.run_until_parked();
            let reference = f.cx.capture_screenshot(f.window.into())?;
            for copies in [1, 2, 3] {
                f.view.update(&mut f.cx, |view, cx| {
                    view.copies = copies;
                    cx.notify();
                });
                f.cx.run_until_parked();
                let pixels = f.cx.capture_screenshot(f.window.into())?;
                let difference = pixels
                    .pixels()
                    .zip(reference.pixels())
                    .filter(|(a, b)| a != b)
                    .count();
                anyhow::ensure!(difference == 0, "node changed native alpha/edges: gpu={gpu}, alpha={alpha}, copies={copies}, nested={nested}, different pixels={difference}");
            }
            let region = f
                .view
                .read_with(&f.cx, |view, _| view.region.borrow().clone())
                .unwrap();
            let replaced = f.cx.update_window(f.window.into(), |_, window, _| {
                let replaced = window.repaint_region(&region, |_| {});
                window.present_if_needed();
                replaced
            })?;
            anyhow::ensure!(replaced, "paint-only cancellation was not exercised");
            let pixels = f.cx.capture_screenshot(f.window.into())?;
            anyhow::ensure!(
                pixels == reference,
                "cancelling playback lost/doubled source pixels: gpu={gpu}, alpha={alpha}"
            );
        }
    }
    println!("paint nodes: native transparency, text edges, parent alpha, nested replay and paint-only cancellation passed on GPU and fallback");
    Ok(())
}

struct EnrollmentOwner {
    created: bool,
}

struct EnrollmentFrame {
    content: Entity<EnrollmentOwner>,
    navigation: Entity<zork_ui::chat_navigation::Navigation>,
    modal: zork_ui::modal::ModalState,
    open: bool,
}

impl Render for EnrollmentFrame {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let visible = self
            .modal
            .retain("add-device-dialog", self.open.then_some(()), cx)
            .is_some();
        let content = self.content.clone();
        div()
            .size_full()
            .relative()
            .font_family("Inter Variable")
            .text_size(px(13.))
            .bg(rgb(0xffffff))
            .child(
                div()
                    .w(px(240.))
                    .h(window.viewport_size().height - px(60.))
                    .child(self.navigation.clone()),
            )
            .when(visible, |frame| {
                let data = zork_ui::network::EnrollmentData {
                    available: true,
                    busy: false,
                    command: String::new(),
                    status: String::new(),
                    status_label: String::new(),
                    notice: None,
                };
                frame.child(zork_ui::network::enrollment_dialog(
                    data,
                    &self.modal,
                    window,
                    cx,
                    move |_, action, cx| {
                        content.update(cx, |owner, cx| {
                            if matches!(action, zork_ui::network::EnrollmentAction::Create) {
                                owner.created = true;
                            }
                            cx.notify();
                        });
                    },
                    |view, cx| {
                        view.open = false;
                        cx.notify();
                    },
                ))
            })
    }
}

fn settle_enrollment(
    f: &mut Fixture<EnrollmentFrame>,
    open: bool,
) -> anyhow::Result<serde_json::Value> {
    use std::time::{Duration, Instant};
    let started = Instant::now();
    let capture = std::env::var_os("ZORK_ENROLLMENT_SOURCE_FRAMES").map(|path| {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(path)
    });
    let mut samples = Vec::new();
    let mut minimum_ink = usize::MAX;
    // Quiet icon sources draw only their glyph, so compare with the first frame.
    let mut baseline: Option<usize> = None;
    loop {
        f.cx.advance_clock(Duration::from_millis(16));
        f.cx.update_window(f.window.into(), |_, window, cx| {
            window.simulate_next_frame(cx);
        })?;
        f.cx.run_until_parked();
        f.cx.update_window(f.window.into(), |_, window, _| {
            window.present_if_needed();
        })?;
        let state = f
            .view
            .read_with(&f.cx, |view, _| view.modal.inspect("add-device-dialog"));
        {
            let source = f
                .element("device-add")
                .context("source disappeared from input tree")?;
            let snapshot = f.driver.snapshot(false);
            let scale = snapshot.scale_factor;
            let pixels = f.cx.capture_screenshot(f.window.into())?;
            let x = (source.bounds.x * scale).round() as u32;
            let y = (source.bounds.y * scale).round() as u32;
            let width = (source.bounds.width * scale).round() as u32;
            let height = (source.bounds.height * scale).round() as u32;
            let crop = image::imageops::crop_imm(&pixels, x, y, width, height).to_image();
            let ink = crop
                .pixels()
                .filter(|pixel| pixel.0[..3].iter().all(|c| *c < 150))
                .count();
            let name = format!("{}-{}", pixels.width(), if open { "open" } else { "close" });
            if let Some(output) = &capture {
                std::fs::create_dir_all(output)?;
                crop.save(output.join(format!("{name}-{:03}.png", samples.len())))?;
            }
            if ink < minimum_ink {
                minimum_ink = ink;
                if let Some(output) = &capture {
                    pixels.save(output.join(format!("{name}-minimum.png")))?;
                }
            }
            let counters = f
                .view
                .read_with(&f.cx, |view, cx| view.navigation.read(cx).counters(cx));
            samples.push(json!({"ms":started.elapsed().as_secs_f64()*1000.,"ink":ink,"motion":state,"counters":counters}));
            if let Some(output) = &capture {
                std::fs::write(
                    output.join(format!("{name}.json")),
                    serde_json::to_vec_pretty(&samples)?,
                )?;
            }
            let first = *baseline.get_or_insert(ink);
            anyhow::ensure!(
                ink > 20 && ink * 2 >= first,
                "source button stopped drawing: {name}, ink={ink} of {first}, state={state}"
            );
        }
        let alpha = if open { 1. } else { 0. };
        anyhow::ensure!(
            state["engine"] == "plain",
            "unexpected dialog renderer: {state}"
        );
        if state["open"] == open
            && state["contentAlpha"] == alpha
            && state["backdropAlpha"] == alpha
        {
            return Ok(state);
        }
        anyhow::ensure!(
            started.elapsed() < Duration::from_secs(5),
            "enrollment did not settle: {state}"
        );
        std::thread::sleep(Duration::from_millis(16));
    }
}

fn enrollment_checks() -> anyhow::Result<()> {
    std::env::set_var("SEED", "0");
    std::env::set_var("ZORK_GUI_TEST_REDUCE_MOTION", "0");
    let output = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../artifacts/headless-interactions/modals");
    std::fs::create_dir_all(&output)?;
    // At DPR 2 these exercise both retained GPU content and the size-limited fallback.
    for (width, height) in [(1280., 800.), (1470., 956.)] {
        let mut f = Fixture::new(width, height, |cx| {
            let mut modal = zork_ui::modal::ModalState::new(cx);
            modal.retain("add-device-dialog", None::<()>, cx);
            let navigation = cx.new(|cx| {
                zork_ui::chat_navigation::Navigation::new(
                    zork_ui::resources::Text(std::rc::Rc::new(|key| {
                        zork_gui::i18n::Locale::ZhCn.text(key).into()
                    })),
                    cx,
                )
            });
            cx.subscribe(&navigation, |view: &mut EnrollmentFrame, _, action, cx| {
                if matches!(
                    action,
                    zork_ui::chat_navigation::Action::Navigate {
                        node: None,
                        destination: zork_ui::chat_navigation::Destination::Manage(3)
                    }
                ) {
                    view.open = true;
                    cx.notify();
                }
            })
            .detach();
            EnrollmentFrame {
                content: cx.new(|_| EnrollmentOwner { created: false }),
                navigation,
                modal,
                open: false,
            }
        })?;
        f.cx.update(|cx| cx.set_reduce_motion(false));
        f.element("device-add").context("sidebar source missing")?;
        f.click("device-add")?;
        let state = settle_enrollment(&mut f, true)?;
        let pixels = f.cx.capture_screenshot(f.window.into())?;
        pixels.save(output.join(format!("enrollment-{width}.png")))?;
        std::fs::write(
            output.join(format!("enrollment-{width}.json")),
            serde_json::to_vec_pretty(
                &json!({"motion":state,"elements":f.driver.snapshot(false)}),
            )?,
        )?;
        let card = f
            .element("add-device-dialog")
            .context("enrollment dialog missing")?;
        // Plain dialogs are centered on the window; they no longer expose a
        // source anchor or a simulated panel position.
        anyhow::ensure!(
            (card.bounds.x + card.bounds.width / 2. - width / 2.).abs() < 1.
                && (card.bounds.y + card.bounds.height / 2. - height / 2.).abs() < 1.
                && card.bounds == card.visible_bounds,
            "enrollment dialog is not centered and fully visible: {:?}",
            card.bounds
        );
        for id in ["add-device-dialog-close", "mesh-invite-create"] {
            let control = f.element(id).with_context(|| format!("missing {id}"))?;
            anyhow::ensure!(
                control.visible && control.bounds == control.visible_bounds,
                "{id} was clipped"
            );
            anyhow::ensure!(
                control.bounds.x >= card.bounds.x
                    && control.bounds.y >= card.bounds.y
                    && control.bounds.x + control.bounds.width <= card.bounds.x + card.bounds.width
                    && control.bounds.y + control.bounds.height
                        <= card.bounds.y + card.bounds.height,
                "{id} escaped the dialog"
            );
        }
        let scale = f.driver.snapshot(false).scale_factor;
        let title_ink = pixels
            .enumerate_pixels()
            .filter(|(x, y, pixel)| {
                let (x, y) = (*x as f32 / scale, *y as f32 / scale);
                x >= card.bounds.x + 24.
                    && x < card.bounds.x + 190.
                    && y >= card.bounds.y + 20.
                    && y < card.bounds.y + 56.
                    && pixel.0[..3].iter().all(|channel| *channel < 150)
            })
            .count();
        anyhow::ensure!(title_ink > 50, "dialog title missing from native pixels");
        f.click("mesh-invite-create")?;
        anyhow::ensure!(
            f.view
                .read_with(&f.cx, |view, cx| view.content.read(cx).created),
            "create input missed its painted control"
        );
        f.click("add-device-dialog-close")?;
        settle_enrollment(&mut f, false)?;
        f.key("enter")?;
        anyhow::ensure!(
            f.view.read_with(&f.cx, |view, _| view.open),
            "closing did not restore the sidebar trigger's keyboard focus"
        );
        f.key("escape")?;
        settle_enrollment(&mut f, false)?;
        println!("enrollment {width}: source, layout, native pixels, tabs, close and keyboard reversal passed");
    }
    Ok(())
}

struct ResetFrame {
    content: Entity<zork_ui::settings::data::DataSettings>,
    confirmations: usize,
}
impl Render for ResetFrame {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.content.clone()
    }
}
fn data_reset_checks() -> anyhow::Result<()> {
    use zork_ui::settings::data::{Confirmed, Data, DataSettings};
    for (width, height) in [(900., 600.), (1280., 800.)] {
        for locale in zork_gui::i18n::Locale::ALL {
            let text =
                zork_ui::resources::Text(std::rc::Rc::new(move |key| locale.text(key).into()));
            let view_text = text.clone();
            let mut f = Fixture::<ResetFrame>::new(width, height, move |cx| {
                let content = cx.new(|cx| DataSettings::new(Data::default(), view_text, cx));
                cx.subscribe(&content, |v, _, _: &Confirmed, cx| {
                    v.confirmations += 1;
                    cx.notify();
                })
                .detach();
                ResetFrame {
                    content,
                    confirmations: 0,
                }
            })?;
            f.click("clear-client-data")?;
            anyhow::ensure!(
                f.view.read_with(&f.cx, |v, _| v.confirmations) == 0,
                "opening cleared data"
            );
            let confirm = f
                .element("clear-client-data-dialog-confirm")
                .context("missing reset confirmation")?;
            anyhow::ensure!(
                confirm.bounds == confirm.visible_bounds,
                "reset confirmation clipped"
            );
            f.key("escape")?;
            anyhow::ensure!(
                f.view.read_with(&f.cx, |v, _| v.confirmations) == 0,
                "cancelling cleared data"
            );
            f.click("clear-client-data")?;
            f.click("clear-client-data-dialog-cancel")?;
            anyhow::ensure!(
                f.view.read_with(&f.cx, |v, _| v.confirmations) == 0,
                "cancel button cleared data"
            );
            f.click("clear-client-data")?;
            f.screenshot(&format!("clear-data-{}-{width}.png", locale.code()))?;
            f.click("clear-client-data-dialog-confirm")?;
            anyhow::ensure!(
                f.view.read_with(&f.cx, |v, _| v.confirmations) == 1,
                "confirmed reset was not emitted exactly once"
            );
            f.view.update(&mut f.cx, |v, cx| {
                v.content.update(cx, |v, cx| {
                    v.configure(
                        Data {
                            busy: true,
                            error: None,
                        },
                        text.clone(),
                        cx,
                    )
                });
            });
            f.key("escape")?;
            anyhow::ensure!(
                f.view
                    .read_with(&f.cx, |v, cx| v.content.read(cx).inspect())["open"]
                    == true,
                "busy reset dismissed"
            );
            anyhow::ensure!(
                !f.element("clear-client-data-dialog-confirm")
                    .unwrap()
                    .enabled,
                "busy reset stayed clickable"
            );
            f.view.update(&mut f.cx, |v, cx| {
                v.content.update(cx, |v, cx| {
                    v.configure(
                        Data {
                            busy: false,
                            error: Some("Node could not stop".into()),
                        },
                        text.clone(),
                        cx,
                    )
                });
            });
            f.key("escape")?;
        }
    }
    println!(
        "PASS reset confirmation, cancellation, busy state and viewport: Chinese/English, 900/1280"
    );
    Ok(())
}
