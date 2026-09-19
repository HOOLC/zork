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
    desktop::{HeadlessAgentsView, HeadlessProfilesView},
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
    fn reveal(&mut self, id: &str, dialog: &str) -> anyhow::Result<()> {
        for _ in 0..5 {
            if self
                .element(id)
                .is_some_and(|element| element.bounds == element.visible_bounds)
            {
                return Ok(());
            }
            let card = self.element(dialog).expect("dialog is open");
            self.action(json!({"type":"scroll", "target":{"x":card.center.x,"y":card.bounds.y+140.},"delta_y":-100.}))?;
        }
        self.screenshot(&format!("{dialog}-missing-{id}.png"))?;
        anyhow::bail!("{id} could not be reached by scrolling {dialog}")
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
            (card.bounds.width - 540.).abs() < 1.,
            "modal width drifted from the shared desktop design"
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
        // SVG decoding is asynchronous. Require painted avatars and stable pixels,
        // without advancing the virtual clock (which would animate the text caret).
        let started = std::time::Instant::now();
        let mut previous = None;
        let mut stable = 0;
        loop {
            self.cx.run_until_parked();
            self.cx
                .update_window(self.window.into(), |_, w, cx| w.draw(cx).clear(cx))?;
            let pixels = self.cx.capture_screenshot(self.window.into())?;
            let snapshot = self.driver.snapshot(false);
            let scale = snapshot.scale_factor;
            let avatars = [
                "cat", "bunny", "bear", "fox", "panda", "chick", "dog", "owl", "koala", "penguin",
                "deer", "octopus",
            ];
            let ready = snapshot
                .elements
                .iter()
                .filter(|e| {
                    avatars.iter().any(|a| e.id == format!("agent-avatar-{a}"))
                        && e.visible_bounds.width >= 16.
                        && e.visible_bounds.height >= 16.
                })
                .all(|e| {
                    let colors: std::collections::HashSet<_> = (-8..8)
                        .flat_map(|dy| (-8..8).map(move |dx| (dx, dy)))
                        .map(|(dx, dy)| {
                            let x = ((e.center.x + dx as f32) * scale)
                                .clamp(0., (pixels.width() - 1) as f32)
                                as u32;
                            let y = ((e.center.y + dy as f32) * scale)
                                .clamp(0., (pixels.height() - 1) as f32)
                                as u32;
                            pixels.get_pixel(x, y).0
                        })
                        .collect();
                    colors.len() > 4
                });
            if ready && previous.as_ref() == Some(pixels.as_raw()) {
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
                anyhow::bail!("screenshot did not settle or has unloaded SVGs: {name} (avatars={ready}, stable={stable})");
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
            (f.element("profile-id").unwrap().bounds.height - 32.).abs() < 0.1,
            "field height drift"
        );
        f.click("profile-id")?;
        f.action(json!({"type":"type_text","text":"固定连接"}))?;
        anyhow::ensure!(
            f.view
                .read_with(&f.cx, |v, cx| v.headless_connection_name(cx))
                == "固定连接",
            "modal field did not receive input"
        );
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
        f.modal("model-editor-dialog", width, height)?;
        f.key("escape")?;
        let mut f = Fixture::new(width, height, HeadlessAgentsView::headless_fixture)?;
        f.click("agent-add")?;
        f.modal("agent-create-dialog", width, height)?;
        f.reveal("agent-profile-select", "agent-create-dialog")?;
        anyhow::ensure!(
            f.element("agent-profile-select").unwrap().label == "自动分配",
            "new agents must default to the pool"
        );
        f.reveal("agent-avatar-cat", "agent-create-dialog")?;
        f.screenshot(&format!("agent-create-{width}.png"))?;
        anyhow::ensure!(
            f.element("agent-create")
                .is_some_and(|e| e.visible_bounds == e.bounds),
            "create action hidden below scroll area"
        );
        f.key("escape")?;
        anyhow::ensure!(
            f.element("agent-create-dialog").is_none(),
            "Escape did not dismiss Agent creator"
        );
        f.click("agent-settings-leader")?;
        f.modal("agent-editor-dialog", width, height)?;
        let model_before = f.element("agent-edit-model-select").unwrap().bounds;
        let effort = f.element("agent-edit-thinking-select").unwrap();
        let profile = f.element("agent-edit-profile-select").unwrap();
        anyhow::ensure!(
            model_before.y < effort.bounds.y && effort.bounds.y < profile.bounds.y,
            "model and effort must precede optional connection"
        );
        let model_label = f.element("agent-edit-model-select").unwrap().label.clone();
        let effort_label = effort.label.clone();
        f.click("agent-edit-profile-select")?;
        let option = f
            .element("agent-edit-profile-0")
            .expect("dropdown options must be visible");
        anyhow::ensure!(
            option.label == "自动分配",
            "automatic account option missing"
        );
        anyhow::ensure!(option.visible_bounds == option.bounds, "dropdown clipped");
        anyhow::ensure!(
            f.element("agent-edit-model-select").unwrap().bounds == model_before,
            "dropdown reflowed form"
        );
        let select = f.element("agent-edit-profile-select").unwrap().bounds;
        let menu = f.element("agent-edit-profile-select-menu").unwrap().bounds;
        anyhow::ensure!(
            (menu.y >= select.y + select.height + 6.
                || menu.y + menu.height <= select.y - 6.)
                && menu.width >= select.width
                && menu.x >= 0. && menu.x + menu.width <= width as f32
                && menu.y >= 0. && menu.y + menu.height <= height as f32,
            "shared dropdown overlaps its source or leaves the viewport: select={select:?}, menu={menu:?}"
        );
        anyhow::ensure!(
            option.bounds.y >= menu.y + 4.
                && option.bounds.x >= menu.x + 4.
                && option.bounds.x + option.bounds.width <= menu.x + menu.width - 4.,
            "dropdown option enters the rounded panel edge"
        );
        f.screenshot(&format!("agent-dropdown-{width}.png"))?;
        f.click("agent-edit-profile-0")?;
        anyhow::ensure!(
            f.element("agent-edit-model-select").unwrap().label == model_label
                && f.element("agent-edit-thinking-select").unwrap().label == effort_label,
            "connection change reset model or effort"
        );
        f.click("agent-edit-thinking-select")?;
        f.click("agent-edit-thinking-0")?;
        f.click("agent-edit-model-select")?;
        f.click("agent-edit-model-0")?;
        f.screenshot(&format!("agent-edit-{width}.png"))?;
        f.click("agent-editor-dialog-close")?;
        anyhow::ensure!(
            f.element("agent-editor-dialog").is_none(),
            "Agent editor did not close"
        );
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
    phone: bool,
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
                    client: content.read(cx).phone,
                    available: true,
                    busy: false,
                    ticket: String::new(),
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
                            if let zork_ui::network::EnrollmentAction::Select(phone) = action {
                                owner.phone = phone;
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
            anyhow::ensure!(
                ink > 500,
                "source button stopped drawing: {name}, ink={ink}, state={state}"
            );
        }
        let alpha = if open { 1. } else { 0. };
        if state["moving"] == false
            && state["open"] == open
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
                let mut navigation = zork_ui::chat_navigation::Navigation::new(
                    Default::default(),
                    zork_ui::resources::Text(std::rc::Rc::new(|key| {
                        zork_gui::i18n::Locale::ZhCn.text(key).into()
                    })),
                    cx,
                );
                navigation.bind_add_device_source(modal.source("add-device-dialog"), cx);
                navigation
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
                content: cx.new(|_| EnrollmentOwner { phone: true }),
                navigation,
                modal,
                open: false,
            }
        })?;
        f.cx.update(|cx| cx.set_reduce_motion(false));
        let source = f
            .element("device-add")
            .context("sidebar source missing")?
            .bounds;
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
        anyhow::ensure!(
            (state["anchor"]["cx"].as_f64().unwrap() - (source.x + source.width / 2.) as f64).abs()
                < 1.
                && (state["anchor"]["cy"].as_f64().unwrap()
                    - (source.y + source.height / 2.) as f64)
                    .abs()
                    < 1.,
            "dialog did not bind its real sidebar source: {}",
            state["anchor"]
        );
        anyhow::ensure!(
            (state["pose"]["w"].as_f64().unwrap() - card.bounds.width as f64).abs() < 1.,
            "enrollment material and content width differ: {state}"
        );
        anyhow::ensure!(
            (state["pose"]["h"].as_f64().unwrap() - card.bounds.height as f64).abs() < 1.,
            "enrollment material and content height differ: {state}"
        );
        for id in [
            "add-device-dialog-close",
            "mesh-connect-phone-tab",
            "mesh-connect-device-tab",
            "mesh-client-invite-create",
        ] {
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
        f.click("mesh-connect-device-tab")?;
        anyhow::ensure!(
            !f.view
                .read_with(&f.cx, |view, cx| view.content.read(cx).phone),
            "tab input missed its painted control"
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
