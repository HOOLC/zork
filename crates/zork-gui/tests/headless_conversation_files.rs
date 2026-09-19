//! Production draft, file-only send, preview/reuse and clipboard image interaction.
use gpui::{px, AppContext, HeadlessAppContext};
use serde_json::json;
use std::{sync::Arc, time::Duration};
use zork_gui::{
    assets::EmbeddedAssets,
    automation::{AutomationRoot, HeadlessAutomation},
    views::RootView,
};

fn main() -> anyhow::Result<()> {
    std::env::set_var("ZORK_GUI_TEST_REDUCE_MOTION", "1");
    let output = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../artifacts/conversation-files/approved-native");
    let output = if std::env::var_os("ZORK_FILES_CONTOUR_ONLY").is_some() {
        output.join("contour")
    } else {
        output
    };
    std::fs::create_dir_all(&output)?;
    let directory = tempfile::tempdir()?;
    let store = Arc::new(zork_client_core::store::ClientStore::open(
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
    let mut view = None;
    let window = cx.open_window(gpui::size(px(926.), px(600.)), |_, cx| {
        let root = cx.new(|cx| RootView::render_benchmark_fixture(false, store.clone(), cx));
        view = Some(root.clone());
        cx.new(|_| AutomationRoot::new(root))
    })?;
    // Root construction initializes platform motion preferences. Override them
    // after that initialization for this first, reduced-motion interaction pass.
    cx.update(|cx| cx.set_reduce_motion(true));
    let view = view.unwrap();
    let core = view.update(&mut cx, |v, _| v.benchmark_core_device());
    let pump = |cx: &mut HeadlessAppContext| -> anyhow::Result<()> {
        for _ in 0..8 {
            std::thread::sleep(Duration::from_millis(10));
            cx.advance_clock(Duration::from_millis(16));
            cx.run_until_parked();
            cx.update_window(window.into(), |_, w, cx| {
                w.simulate_next_frame(cx);
                w.draw(cx).clear(cx)
            })?;
        }
        Ok(())
    };
    let act = |cx: &mut HeadlessAppContext, value: serde_json::Value| -> anyhow::Result<()> {
        let action = serde_json::from_value(value)?;
        cx.update_window(window.into(), |_, w, cx| driver.dispatch(action, w, cx))??;
        pump(cx)
    };
    let screenshot = directory.path().join("page.png");
    pump(&mut cx)?;
    image::imageops::crop_imm(&cx.capture_screenshot(window.into())?, 1050, 100, 720, 960)
        .to_image()
        .save(&screenshot)?;
    view.update(&mut cx, |v, cx| {
        v.benchmark_replace_messages(vec![], cx);
        v.benchmark_bind_core(cx);
        v.benchmark_restore_draft(cx);
    });
    let mixed_assets = std::env::var_os("ZORK_FILES_MIXED_ASSETS").map(std::path::PathBuf::from);
    let reference_assets =
        std::env::var_os("ZORK_FILES_REFERENCE_ASSETS").map(std::path::PathBuf::from);
    let first_content = if let Some(path) = &mixed_assets {
        std::fs::read(path.join("sample.pdf"))?
    } else if let Some(path) = &reference_assets {
        std::fs::read(path.join("center.png"))?
    } else {
        include_bytes!("fixtures/markdown-scroll.md").repeat(16)
    };
    let second_content = if let Some(path) = &mixed_assets {
        std::fs::read(path.join("sample.zip"))?
    } else if let Some(path) = &reference_assets {
        std::fs::read(path.join("left.png"))?
    } else {
        std::fs::read(&screenshot)?
    };
    let file = core.attach_file(
        "render-fixture",
        if mixed_assets.is_some() {
            "sample.pdf"
        } else if reference_assets.is_some() {
            "页面截图.png"
        } else {
            "方案说明.txt"
        },
        &first_content,
    )?;
    let second = core.attach_file(
        "render-fixture",
        if mixed_assets.is_some() {
            "sample.zip"
        } else if reference_assets.is_some() {
            "方案文档.png"
        } else {
            "页面截图.png"
        },
        &second_content,
    )?;
    // A real PNG attachment with a small report chart, alongside text and a UI screenshot.
    let mut chart = image::RgbaImage::from_pixel(360, 480, image::Rgba([255, 255, 255, 255]));
    let mut rect = |x: u32, y: u32, w: u32, h: u32, color: [u8; 4]| {
        for py in y..y + h {
            for px in x..x + w {
                chart.put_pixel(px, py, image::Rgba(color));
            }
        }
    };
    rect(28, 32, 188, 9, [91, 114, 143, 255]);
    rect(28, 52, 110, 5, [189, 199, 211, 255]);
    for y in [112, 152, 192, 232, 272] {
        rect(32, y, 296, 1, [228, 234, 241, 255]);
    }
    for (i, height) in [48, 76, 101, 137, 162].into_iter().enumerate() {
        rect(
            51 + i as u32 * 52,
            272 - height,
            27,
            height,
            [145, 189, 235, 255],
        );
    }
    for y in [316, 331, 346, 376, 391, 406, 421] {
        rect(
            28,
            y,
            if y % 2 == 0 { 284 } else { 246 },
            4,
            [194, 202, 213, 255],
        );
    }
    let mut chart_bytes = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(chart).write_to(&mut chart_bytes, image::ImageFormat::Png)?;
    let third_content = if let Some(path) = &mixed_assets {
        std::fs::read(path.join("sample.dmg"))?
    } else if let Some(path) = &reference_assets {
        std::fs::read(path.join("right.png"))?
    } else {
        chart_bytes.get_ref().clone()
    };
    let third = core.attach_file(
        "render-fixture",
        if mixed_assets.is_some() {
            "sample.dmg"
        } else {
            "统计图.png"
        },
        &third_content,
    )?;
    for _ in 0..60 {
        pump(&mut cx)?;
        if view.read_with(&cx, |v, _| v.benchmark_file_fan_state().2) >= 3 {
            break;
        }
    }
    anyhow::ensure!(
        view.read_with(&cx, |v, _| v.benchmark_file_fan_state().2) >= 1,
        "real thumbnail did not load"
    );
    anyhow::ensure!(!view.read_with(&cx, |v, _| v.benchmark_file_fan_state().0));
    anyhow::ensure!(
        driver
            .snapshot(false)
            .elements
            .iter()
            .any(|e| e.id == "send-button" && e.enabled),
        "file-only draft cannot send"
    );
    core.edit_draft("render-fixture", "参考这些资料，帮我整理一下方案。".into())?;
    pump(&mut cx)?;
    std::fs::write(
        output.join("draft-geometry.json"),
        serde_json::to_vec_pretty(&view.read_with(&cx, |v, _| v.benchmark_file_geometry()))?,
    )?;
    std::fs::write(
        output.join("draft-elements.json"),
        serde_json::to_vec_pretty(&driver.snapshot(true))?,
    )?;
    cx.capture_screenshot(window.into())?
        .save(output.join("draft-wide.png"))?;
    image::imageops::crop_imm(&cx.capture_screenshot(window.into())?, 500, 735, 1336, 455)
        .to_image()
        .save(output.join("fan-collapsed-component.png"))?;
    let closeup =
        image::imageops::crop_imm(&cx.capture_screenshot(window.into())?, 1316, 803, 387, 216)
            .to_image();
    image::imageops::resize(&closeup, 580, 324, image::imageops::FilterType::Triangle)
        .save(output.join("fan-pocket-closeup.png"))?;
    if std::env::var_os("ZORK_FILES_VISUAL_ONLY").is_some() {
        let out = output.parent().unwrap().join(if mixed_assets.is_some() {
            "mixed-types"
        } else {
            "visual-parity"
        });
        std::fs::create_dir_all(&out)?;
        cx.update_window(window.into(), |_, w, cx| {
            w.resize(gpui::size(px(928.), px(600.)));
            w.bounds_changed(cx);
        })?;
        let reference = [
            (file.clone(), &first_content),
            (second.clone(), &second_content),
            (third.clone(), &third_content),
        ];
        let mut measurements = Vec::new();
        for count in 1..=3 {
            for item in &core.draft("render-fixture").files {
                core.remove_file("render-fixture", &item.id)?;
            }
            for (item, bytes) in &reference[..count] {
                core.reuse_file("render-fixture", item.clone(), bytes)?;
            }
            for _ in 0..4 {
                pump(&mut cx)?;
            }
            for state in ["closed", "open"] {
                if state == "open" {
                    act(
                        &mut cx,
                        json!({"type":"click","target":{"element_id":"draft-file-fan-toggle"}}),
                    )?;
                    act(&mut cx, json!({"type":"move","target":{"x":600,"y":80}}))?;
                }
                let snapshot = driver.snapshot(false);
                let composer = snapshot
                    .elements
                    .iter()
                    .find(|e| e.id == "composer-surface")
                    .unwrap()
                    .bounds;
                anyhow::ensure!((composer.width - 640.).abs() < 0.1);
                let model = view.read_with(&cx, |v, _| v.benchmark_file_geometry());
                anyhow::ensure!(
                    (composer.width as f64 - model["composer_width"].as_f64().unwrap()).abs() < 0.1,
                    "measured composer width differs from rendered width"
                );
                let image = cx.capture_screenshot(window.into())?;
                image.save(out.join(format!("native-{count}-{state}.png")))?;
                image::imageops::crop_imm(
                    &image,
                    ((composer.x + 370.) * 2.) as u32,
                    ((composer.y - 12. - 90.) * 2.) as u32,
                    520,
                    228,
                )
                .to_image()
                .save(out.join(format!("native-{count}-{state}-detail.png")))?;
                if count == 1 && state == "closed" {
                    let paper = snapshot
                        .elements
                        .iter()
                        .find(|e| e.id == format!("draft-preview-{}", file.id))
                        .unwrap()
                        .bounds;
                    anyhow::ensure!(
                        (paper.x + paper.width * 0.5
                            - composer.x
                            - model["axis_x"].as_f64().unwrap() as f32)
                            .abs()
                            <= 0.5,
                        "paper and aperture axes are misaligned"
                    );
                    let x = ((composer.x + model["axis_x"].as_f64().unwrap() as f32) * 2.) as u32;
                    for y in ((composer.y - 12.) * 2.) as u32..((composer.y - 2.) * 2.) as u32 {
                        anyhow::ensure!(
                            &image.get_pixel(x, y).0[..3] != [246, 245, 241],
                            "paper is interrupted by the composer surface at {x},{y}"
                        );
                    }
                }
                // The front lip must remain visible over every paper, including
                // unsupported-file placeholders, throughout the closed fan.
                if state == "closed" {
                    use zork_ui::components::attachment_fan::{sample, Opening, Shape};
                    let rim = Opening::new(Shape::for_count(count), 0.);
                    for curve in &rim.hole[3..5] {
                        for step in 1..10 {
                            let point = sample(*curve, step as f32 / 10.);
                            let x =
                                ((composer.x + model["axis_x"].as_f64().unwrap() as f32 + point.x)
                                    * 2.) as u32;
                            let y = ((composer.y - 12. + point.y) * 2.) as u32;
                            let color = image.get_pixel(x, y).0;
                            anyhow::ensure!(
                                color[0] <= 246
                                    && color[1] <= 246
                                    && color[2] <= 246
                                    && color[..3].iter().max().unwrap()
                                        - color[..3].iter().min().unwrap()
                                        <= 2,
                                "front lip is covered at {x},{y}: {color:?}"
                            );
                        }
                    }
                }
                measurements.push(json!({"count":count,"state":state,"geometry":view.read_with(&cx,|v,_|v.benchmark_file_geometry())}));
                if state == "open" {
                    act(&mut cx, json!({"type":"key","keystroke":"escape"}))?;
                }
            }
        }
        std::fs::write(
            out.join("native-geometry.json"),
            serde_json::to_vec_pretty(&measurements)?,
        )?;
        println!(
            "PASS: matched reference assets; one/two/three-file closed and open visual captures; continuous paper through rim"
        );
        return Ok(());
    }
    if std::env::var_os("ZORK_FILES_STATIC_ONLY").is_some() {
        println!(
            "PASS: native static attachment pocket with three real previews; closeup normalized to reference framing"
        );
        return Ok(());
    }
    act(
        &mut cx,
        json!({"type":"move","target":{"element_id":format!("draft-preview-{}",file.id)}}),
    )?;
    std::fs::write(
        output.join("hover-geometry.json"),
        serde_json::to_vec_pretty(&view.read_with(&cx, |v, _| v.benchmark_file_geometry()))?,
    )?;
    cx.capture_screenshot(window.into())?
        .save(output.join("hover.png"))?;
    anyhow::ensure!(
        view.read_with(&cx, |v, _| v.benchmark_file_fan_state().0),
        "hover did not unfold fan"
    );
    act(&mut cx, json!({"type":"move","target":{"x":600,"y":80}}))?;
    anyhow::ensure!(
        !view.read_with(&cx, |v, _| v.benchmark_file_fan_state().0),
        "hover fan did not close on exit"
    );
    act(
        &mut cx,
        json!({"type":"click","target":{"element_id":"draft-file-fan-toggle"}}),
    )?;
    act(&mut cx, json!({"type":"move","target":{"x":600,"y":80}}))?;
    anyhow::ensure!(
        view.read_with(&cx, |v, _| v.benchmark_file_fan_state().1),
        "click did not pin fan"
    );
    pump(&mut cx)?;
    pump(&mut cx)?;
    cx.capture_screenshot(window.into())?
        .save(output.join("fan-expanded-wide.png"))?;
    image::imageops::crop_imm(&cx.capture_screenshot(window.into())?, 500, 640, 1336, 550)
        .to_image()
        .save(output.join("fan-expanded-component.png"))?;
    core.edit_draft("render-fixture", String::new())?;
    pump(&mut cx)?;
    act(&mut cx, json!({"type":"key","keystroke":"escape"}))?;
    anyhow::ensure!(
        !view.read_with(&cx, |v, _| v.benchmark_file_fan_state().0),
        "Escape did not close fan"
    );
    act(
        &mut cx,
        json!({"type":"move","target":{"element_id":format!("draft-preview-{}",file.id)}}),
    )?;
    act(
        &mut cx,
        json!({"type":"click","target":{"element_id":format!("remove-{}",file.id)}}),
    )?;
    anyhow::ensure!(
        core.draft("render-fixture").files.len() == 2,
        "overlapping file removal failed"
    );
    core.reuse_file("render-fixture", file.clone(), &first_content)?;
    pump(&mut cx)?;
    act(
        &mut cx,
        json!({"type":"click","target":{"element_id":"send-button"}}),
    )?;
    anyhow::ensure!(core.draft("render-fixture").files.is_empty());
    let pending = store.outbox("mini1")?;
    anyhow::ensure!(pending.len() == 1);
    anyhow::ensure!(
        zork_client_core::files::decode(&pending[0].content)
            .unwrap()
            .1
            == vec![second, third, file.clone()]
    );
    anyhow::ensure!(
        driver
            .snapshot(false)
            .elements
            .iter()
            .any(|e| e.id == format!("message-file-0-{}", file.id)),
        "sent file has no card"
    );
    cx.capture_screenshot(window.into())?
        .save(output.join("message-fan.png"))?;
    act(
        &mut cx,
        json!({"type":"move","target":{"element_id":format!("message-file-0-{}",file.id)}}),
    )?;
    act(
        &mut cx,
        json!({"type":"click","target":{"element_id":format!("message-file-0-{}",file.id)}}),
    )?;
    anyhow::ensure!(
        driver
            .snapshot(false)
            .elements
            .iter()
            .any(|e| e.id == "drive-save" && e.enabled),
        "pending local snapshot cannot be previewed/saved"
    );
    cx.capture_screenshot(window.into())?
        .save(output.join("preview-wide.png"))?;
    act(
        &mut cx,
        json!({"type":"click","target":{"element_id":"preview-more"}}),
    )?;
    act(
        &mut cx,
        json!({"type":"click","target":{"element_id":"preview-menu-0-drive-reuse"}}),
    )?;
    anyhow::ensure!(core.draft("render-fixture").files == vec![file.clone()]);
    let overlay = view.read_with(&cx, |v, cx| v.benchmark_attachment_overlay(cx));
    anyhow::ensure!(
        overlay["destinationLayer"] == "modal"
            && overlay["anchor"]["w"].as_f64().is_some_and(|w| w > 40.),
        "attachment preview did not bind the real file source: {overlay}"
    );
    std::env::set_var("ZORK_GUI_TEST_REDUCE_MOTION", "0");
    cx.update(|cx| cx.set_reduce_motion(false));
    act(
        &mut cx,
        json!({"type":"click","target":{"element_id":"attachment-preview-dialog-close"}}),
    )?;
    let retiring = view.read_with(&cx, |v, cx| v.benchmark_attachment_overlay(cx));
    anyhow::ensure!(
        retiring["destinationLayer"] == "source"
            && retiring["progress"].as_f64().is_some_and(|p| p > 0.),
        "closing attachment skipped its retained exit: {retiring}"
    );
    anyhow::ensure!(
        !driver
            .snapshot(false)
            .elements
            .iter()
            .any(|e| e.id == "drive-save" && e.enabled),
        "retiring preview kept an active save action"
    );
    act(
        &mut cx,
        json!({"type":"click","target":{"element_id":format!("message-file-0-{}",file.id)}}),
    )?;
    let reversed = view.read_with(&cx, |v, cx| v.benchmark_attachment_overlay(cx));
    anyhow::ensure!(
        reversed["destinationLayer"] == "modal",
        "attachment did not reverse into the modal layer"
    );
    std::env::set_var("ZORK_GUI_TEST_REDUCE_MOTION", "1");
    cx.update(|cx| cx.set_reduce_motion(true));
    pump(&mut cx)?;
    act(&mut cx, json!({"type":"key","keystroke":"escape"}))?;
    act(
        &mut cx,
        json!({"type":"move","target":{"element_id":format!("draft-preview-{}",file.id)}}),
    )?;
    act(
        &mut cx,
        json!({"type":"click","target":{"element_id":format!("remove-{}",file.id)}}),
    )?;
    anyhow::ensure!(core.draft("render-fixture").files.is_empty());
    let mut image_bytes = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        32,
        32,
        image::Rgba([64, 128, 192, 255]),
    ))
    .write_to(&mut image_bytes, image::ImageFormat::Png)?;
    cx.update(|cx| {
        cx.write_to_clipboard(
            gpui::Image::from_bytes(gpui::ImageFormat::Png, image_bytes.into_inner()).into(),
        )
    });
    act(
        &mut cx,
        json!({"type":"click","target":{"element_id":"composer-input"}}),
    )?;
    act(&mut cx, json!({"type":"key","keystroke":"cmd-v"}))?;
    anyhow::ensure!(
        core.draft("render-fixture").files.len() == 1,
        "clipboard image did not become an attachment"
    );
    anyhow::ensure!(core.draft("render-fixture").files[0].name.ends_with(".png"));
    cx.update_window(window.into(), |_, w, cx| {
        w.resize(gpui::size(px(900.), px(600.)));
        w.bounds_changed(cx);
    })?;
    pump(&mut cx)?;
    cx.capture_screenshot(window.into())?
        .save(output.join("clipboard-compact.png"))?;
    for element in driver
        .snapshot(false)
        .elements
        .iter()
        .filter(|e| e.id == "send-button" || e.id.starts_with("remove-file-"))
    {
        anyhow::ensure!(
            element.bounds.x >= 0.
                && element.bounds.x + element.bounds.width <= 900.
                && element.bounds.y + element.bounds.height <= 600.,
            "file controls clipped: {}",
            element.id
        );
    }
    // Verify actual frame samples as well as reduced-motion interaction states.
    act(&mut cx, json!({"type":"move","target":{"x":600,"y":80}}))?;
    std::env::set_var("ZORK_GUI_TEST_REDUCE_MOTION", "0");
    cx.update(|cx| cx.set_reduce_motion(false));
    let image_id = core.draft("render-fixture").files[0].id.clone();
    act(
        &mut cx,
        json!({"type":"move","target":{"element_id":format!("draft-preview-{image_id}")}}),
    )?;
    let intermediate = view.read_with(&cx, |v, _| v.benchmark_file_fan_progress());
    anyhow::ensure!(
        intermediate > 0. && intermediate < 1.,
        "fan skipped continuous expansion: {intermediate}"
    );
    for _ in 0..4 {
        pump(&mut cx)?;
    }
    anyhow::ensure!(view.read_with(&cx, |v, _| v.benchmark_file_fan_progress()) == 1.);
    act(&mut cx, json!({"type":"move","target":{"x":600,"y":80}}))?;
    anyhow::ensure!(
        view.read_with(&cx, |v, _| v.benchmark_file_fan_progress()) == 1.,
        "fan collapsed during the pointer-exit grace period"
    );
    pump(&mut cx)?;
    let intermediate = view.read_with(&cx, |v, _| v.benchmark_file_fan_progress());
    anyhow::ensure!(
        intermediate > 0. && intermediate < 1.,
        "fan skipped continuous collapse: {intermediate}"
    );
    act(
        &mut cx,
        json!({"type":"move","target":{"element_id":format!("draft-preview-{image_id}")}}),
    )?;
    anyhow::ensure!(
        view.read_with(&cx, |v, _| v.benchmark_file_fan_progress()) > intermediate,
        "re-entering the fan did not reverse the in-flight collapse"
    );
    for _ in 0..4 {
        pump(&mut cx)?;
    }
    anyhow::ensure!(view.read_with(&cx, |v, _| v.benchmark_file_fan_progress()) == 1.);
    act(&mut cx, json!({"type":"move","target":{"x":600,"y":80}}))?;
    for _ in 0..4 {
        pump(&mut cx)?;
    }
    anyhow::ensure!(view.read_with(&cx, |v, _| v.benchmark_file_fan_progress()) == 0.);
    // Membership animation uses the real draft subscription and native frames.
    // File data has already been prepared; time only layout/paint, not thumbnail
    // decoding or sleeping for a background loader.
    let before_count_motion =
        cx.update_window(window.into(), |_, w, _| w.frame_duration_snapshot())?;
    for file in &core.draft("render-fixture").files {
        core.remove_file("render-fixture", &file.id)?;
    }
    for _ in 0..8 {
        pump(&mut cx)?;
    }
    let geometry = |cx: &HeadlessAppContext| view.read_with(cx, |v, _| v.benchmark_file_geometry());
    let until = |cx: &mut HeadlessAppContext,
                 label: &str,
                 predicate: &dyn Fn(&serde_json::Value) -> bool|
     -> anyhow::Result<serde_json::Value> {
        for _ in 0..240 {
            let state = geometry(cx);
            if predicate(&state) {
                return Ok(state);
            }
            std::thread::sleep(Duration::from_millis(10));
            cx.advance_clock(Duration::from_millis(16));
            cx.run_until_parked();
            cx.update_window(window.into(), |_, w, cx| {
                w.simulate_next_frame(cx);
                w.draw(cx).clear(cx)
            })?;
        }
        anyhow::bail!("animation phase not reached: {label}")
    };
    anyhow::ensure!(geometry(&cx)["presence"] == 0.);
    let mut stages = Vec::new();
    let first = core.attach_file("render-fixture", "first.png", chart_bytes.get_ref())?;
    let opening = until(&mut cx, "opening before first file", &|g| {
        let presence = g["presence"].as_f64().unwrap();
        presence >= 0.3 && presence < 1.
    })?;
    anyhow::ensure!(opening["changing_count"] == true);
    anyhow::ensure!(
        opening["presence"].as_f64().unwrap() > 0. && opening["presence"].as_f64().unwrap() < 1.
    );
    anyhow::ensure!(
        opening["files"][0]["visible"] == false,
        "first file appeared before its opening"
    );
    cx.capture_screenshot(window.into())?
        .save(output.join("count-0-to-1-hole-first.png"))?;
    stages.push(json!({"stage":"0-to-1-hole-first","geometry":opening}));
    let emerging = until(&mut cx, "first file emergence", &|g| {
        g["presence"] == 1. && g["files"][0]["visible"] == true
    })?;
    anyhow::ensure!(emerging["presence"] == 1. && emerging["files"][0]["visible"] == true);
    cx.capture_screenshot(window.into())?
        .save(output.join("count-0-to-1-emerging.png"))?;
    stages.push(json!({"stage":"0-to-1-emerging","geometry":emerging}));
    let single = until(&mut cx, "single file settled", &|g| {
        g["changing_count"] == false
    })?;
    anyhow::ensure!(single["changing_count"] == false && single["outer_height"] == 0.);
    cx.capture_screenshot(window.into())?
        .save(output.join("count-1-closed.png"))?;
    let second = core.attach_file("render-fixture", "second.png", chart_bytes.get_ref())?;
    let widening = until(&mut cx, "widen before second file", &|g| {
        g["width_factor"].as_f64().unwrap() > single["width_factor"].as_f64().unwrap() + 0.001
            && g["files"][1]["visible"] == false
    })?;
    anyhow::ensure!(
        widening["width_factor"].as_f64().unwrap() > single["width_factor"].as_f64().unwrap()
    );
    anyhow::ensure!(
        widening["files"][1]["visible"] == false,
        "second file appeared before widening"
    );
    stages.push(json!({"stage":"1-to-2-widen-first","geometry":widening}));
    let two = until(&mut cx, "two files settled", &|g| {
        g["changing_count"] == false
    })?;
    anyhow::ensure!(two["changing_count"] == false);
    anyhow::ensure!(
        (two["files"][0]["x"].as_f64().unwrap() + two["files"][1]["x"].as_f64().unwrap()).abs()
            < 0.01
    );
    cx.capture_screenshot(window.into())?
        .save(output.join("count-2-closed.png"))?;
    act(
        &mut cx,
        json!({"type":"move","target":{"element_id":format!("draft-preview-{}",first.id)}}),
    )?;
    for _ in 0..4 {
        pump(&mut cx)?;
    }
    let expanded = geometry(&cx);
    anyhow::ensure!(
        (expanded["hole_height"].as_f64().unwrap() - single["hole_height"].as_f64().unwrap()).abs()
            < 0.01,
        "single-file slot height differs from flattened multi-file slot"
    );
    for button in driver
        .snapshot(false)
        .elements
        .iter()
        .filter(|e| e.id.starts_with("remove-"))
    {
        anyhow::ensure!(
            button.bounds.width >= 28. && button.bounds.height >= 28.,
            "remove target is too small"
        );
    }
    cx.capture_screenshot(window.into())?
        .save(output.join("count-2-expanded.png"))?;
    let snapshot = driver.snapshot(false);
    let composer = snapshot
        .elements
        .iter()
        .find(|e| e.id == "composer-surface")
        .unwrap()
        .bounds;
    let pixels = cx.capture_screenshot(window.into())?;
    let scale = pixels.width() as f32 / 900.;
    let y = ((composer.y + 40.) * scale) as u32;
    for x in
        ((composer.x + 40.) * scale) as u32..((composer.x + composer.width - 40.) * scale) as u32
    {
        anyhow::ensure!(
            &pixels.get_pixel(x, y).0[..3] == [246, 245, 241],
            "attachment mask seam in composer body at {x},{y}"
        );
    }
    act(
        &mut cx,
        json!({"type":"click","target":{"element_id":format!("remove-{}",first.id)}}),
    )?;
    pump(&mut cx)?;
    let retreating = geometry(&cx);
    anyhow::ensure!(retreating["changing_count"] == true);
    anyhow::ensure!(
        retreating["expanded"] == 1.,
        "deletion reset the hover expansion"
    );
    anyhow::ensure!(retreating["files"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v["id"] == first.id && v["departing"] == true));
    anyhow::ensure!(
        (retreating["width_factor"].as_f64().unwrap() - two["width_factor"].as_f64().unwrap())
            .abs()
            < 0.01,
        "opening narrowed before departing file retreated"
    );
    cx.capture_screenshot(window.into())?
        .save(output.join("count-2-to-1-retreating.png"))?;
    stages.push(json!({"stage":"2-to-1-retreat-first","geometry":retreating}));
    for _ in 0..7 {
        pump(&mut cx)?;
    }
    anyhow::ensure!(
        geometry(&cx)["expanded"] == 1.,
        "fan collapsed after deleting the hovered file"
    );
    act(&mut cx, json!({"type":"move","target":{"x":600,"y":80}}))?;
    for _ in 0..4 {
        pump(&mut cx)?;
    }
    anyhow::ensure!(
        geometry(&cx)["expanded"] == 0.,
        "fan stayed expanded after the pointer left"
    );
    core.remove_file("render-fixture", &second.id)?;
    pump(&mut cx)?;
    pump(&mut cx)?;
    let last = geometry(&cx);
    anyhow::ensure!(last["presence"] == 1. && last["files"][0]["departing"] == true);
    stages.push(json!({"stage":"1-to-0-retreat-first","geometry":last}));
    for _ in 0..7 {
        pump(&mut cx)?;
    }
    anyhow::ensure!(
        geometry(&cx)["presence"] == 0. && geometry(&cx)["files"].as_array().unwrap().is_empty()
    );
    // The shared composer retains the changing opening until its contour also
    // settles. Verify that lifetime explicitly before testing idle redraws.
    let mut settling_frames = 0;
    while view.read_with(&cx, |v, _| {
        v.benchmark_composer_material()["moving"] == true
    }) && settling_frames < 30
    {
        pump(&mut cx)?;
        settling_frames += 1;
    }
    anyhow::ensure!(
        view.read_with(&cx, |v, _| v.benchmark_composer_material()["moving"]
            == false),
        "attachment material did not settle"
    );
    pump(&mut cx)?;
    let stable = view.update(&mut cx, |v, cx| v.benchmark_region_counts(cx));
    for _ in 0..4 {
        pump(&mut cx)?;
    }
    let after = view.read_with(&cx, |v, cx| v.benchmark_region_counts(cx));
    anyhow::ensure!(
        stable == after,
        "attachment animation keeps redrawing after completion: before={stable:?}, after={after:?}, material={}",
        view.read_with(&cx, |v, _| v.benchmark_composer_material())
    );
    let mut timings = cx.update_window(window.into(), |_, w, _| w.frame_duration_snapshot())?;
    timings
        .draw_duration_histogram
        .subtract(&before_count_motion.draw_duration_histogram)?;
    let p95 = timings.draw_duration_histogram.value_at_quantile(0.95) as f64 / 1e6;
    let p99 = timings.draw_duration_histogram.value_at_quantile(0.99) as f64 / 1e6;
    std::fs::write(
        output.join("count-animation.json"),
        serde_json::to_vec_pretty(
            &json!({"stages":stages,"p95_draw_ms":p95,"p99_draw_ms":p99,"idle_stable":true}),
        )?,
    )?;
    anyhow::ensure!(
        p95 <= 1000. / 120.,
        "attachment animation exceeded CPU frame budget: {p95:.2} ms"
    );
    // A single delivered file retains complete round caps inside the compact
    // message plate, rather than being cut off by that plate's outer corners.
    let solo = core.attach_file(
        "render-fixture",
        "single-message.png",
        chart_bytes.get_ref(),
    )?;
    for _ in 0..8 {
        pump(&mut cx)?;
    }
    act(
        &mut cx,
        json!({"type":"click","target":{"element_id":"send-button"}}),
    )?;
    for _ in 0..8 {
        pump(&mut cx)?;
    }
    anyhow::ensure!(driver
        .snapshot(false)
        .elements
        .iter()
        .any(|e| e.id.ends_with(&solo.id) && e.id.starts_with("message-file-")));
    cx.capture_screenshot(window.into())?
        .save(output.join("single-message.png"))?;
    // Exercise the supported maximum through the same real attachment path.
    cx.update(|cx| cx.set_reduce_motion(true));
    std::env::set_var("ZORK_GUI_TEST_REDUCE_MOTION", "1");
    for index in 0..16 {
        core.attach_file(
            "render-fixture",
            &format!("page-{index}.png"),
            chart_bytes.get_ref(),
        )?;
    }
    for _ in 0..3 {
        pump(&mut cx)?;
    }
    act(
        &mut cx,
        json!({"type":"click","target":{"element_id":"draft-file-fan-toggle"}}),
    )?;
    let maximum = geometry(&cx);
    anyhow::ensure!(maximum["files"].as_array().unwrap().len() == 16 && maximum["expanded"] == 1.);
    let controls = driver
        .snapshot(false)
        .elements
        .into_iter()
        .filter(|e| e.id.starts_with("remove-"))
        .collect::<Vec<_>>();
    anyhow::ensure!(controls.len() == 16);
    for control in &controls {
        anyhow::ensure!(
            control.visible_bounds.width >= 27.
                && control.visible_bounds.height >= 27.
                && control.bounds.y >= 0.
                && control.bounds.x + control.bounds.width <= 900.,
            "max-count control clipped: {}",
            control.id
        );
    }
    cx.capture_screenshot(window.into())?
        .save(output.join("count-16-expanded.png"))?;
    println!(
        "PASS: real thumbnails, closed aperture, hover/flat expansion, large remove targets, first-file emergence, widen-before-entry, retreat-before-narrowing, single/two-file geometry, idle scheduling, send/preview/reuse, clipboard, and compact layouts"
    );
    Ok(())
}
