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
        if view.read_with(&cx, |v, _| v.benchmark_file_thumbnails()) >= 2 {
            break;
        }
    }
    anyhow::ensure!(
        view.read_with(&cx, |v, _| v.benchmark_file_thumbnails()) >= 1,
        "real thumbnail did not load"
    );
    anyhow::ensure!(
        view.read_with(&cx, |v, _| v.benchmark_file_geometry())["band"]
            .as_f64()
            .unwrap_or_default()
            > 0.,
        "draft files did not reserve their row inside the composer"
    );
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
        .save(output.join("draft-files-component.png"))?;
    let closeup =
        image::imageops::crop_imm(&cx.capture_screenshot(window.into())?, 1316, 803, 387, 216)
            .to_image();
    image::imageops::resize(&closeup, 580, 324, image::imageops::FilterType::Triangle)
        .save(output.join("draft-chip-closeup.png"))?;
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
            for state in ["row"] {
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
                    (composer.y * 2.) as u32,
                    520,
                    228,
                )
                .to_image()
                .save(out.join(format!("native-{count}-{state}-detail.png")))?;
                measurements.push(json!({"count":count,"state":state,"geometry":view.read_with(&cx,|v,_|v.benchmark_file_geometry())}));
            }
        }
        std::fs::write(
            out.join("native-geometry.json"),
            serde_json::to_vec_pretty(&measurements)?,
        )?;
        println!("PASS: one/two/three-file attachment row captures and core-backed geometry");
        return Ok(());
    }
    if std::env::var_os("ZORK_FILES_STATIC_ONLY").is_some() {
        println!("PASS: native static attachment row with three real previews");
        return Ok(());
    }
    // Draft files sit inside the composer surface and keep their remove
    // action visible without hover.
    {
        let snapshot = driver.snapshot(false);
        let bounds = |id: &str| {
            snapshot
                .elements
                .iter()
                .find(|e| e.id == id)
                .map(|e| e.bounds)
                .ok_or_else(|| anyhow::anyhow!("missing {id}"))
        };
        let composer = bounds("composer-surface")?;
        let chip = bounds(&format!("draft-preview-{}", file.id))?;
        let input = bounds("composer-input")?;
        anyhow::ensure!(
            chip.y >= composer.y && chip.y + chip.height <= input.y + 0.5,
            "draft file row is not inside the composer above the editor"
        );
        anyhow::ensure!(
            snapshot.elements.iter().any(|e| e.id == format!("remove-{}", file.id)),
            "remove action is hidden until hover"
        );
    }
    std::fs::write(
        output.join("row-geometry.json"),
        serde_json::to_vec_pretty(&view.read_with(&cx, |v, _| v.benchmark_file_geometry()))?,
    )?;
    cx.capture_screenshot(window.into())?
        .save(output.join("draft-row-wide.png"))?;
    core.edit_draft("render-fixture", String::new())?;
    pump(&mut cx)?;
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
        overlay["engine"] == "plain"
            && overlay["open"] == true
            && overlay["contentAlpha"] == 1.
            && overlay["backdropAlpha"] == 1.,
        "attachment preview did not open its plain dialog: {overlay}"
    );
    std::env::set_var("ZORK_GUI_TEST_REDUCE_MOTION", "0");
    cx.update(|cx| cx.set_reduce_motion(false));
    act(
        &mut cx,
        json!({"type":"click","target":{"element_id":"attachment-preview-dialog-close"}}),
    )?;
    let retiring = view.read_with(&cx, |v, cx| v.benchmark_attachment_overlay(cx));
    anyhow::ensure!(
        retiring["engine"] == "plain"
            && retiring["open"] == false
            && retiring["contentAlpha"]
                .as_f64()
                .is_some_and(|alpha| alpha > 0.),
        "closing attachment skipped its dialog fade: {retiring}"
    );
    anyhow::ensure!(
        !driver
            .snapshot(false)
            .elements
            .iter()
            .any(|e| e.id == "drive-save" && e.enabled),
        "retiring preview kept an active save action"
    );
    pump(&mut cx)?;
    let closed = view.read_with(&cx, |v, cx| v.benchmark_attachment_overlay(cx));
    anyhow::ensure!(
        closed["open"] == false && closed["contentAlpha"] == 0.,
        "attachment dialog did not finish closing: {closed}"
    );
    act(
        &mut cx,
        json!({"type":"click","target":{"element_id":format!("message-file-0-{}",file.id)}}),
    )?;
    let reversed = view.read_with(&cx, |v, cx| v.benchmark_attachment_overlay(cx));
    anyhow::ensure!(
        reversed["engine"] == "plain" && reversed["open"] == true,
        "attachment did not reopen its dialog: {reversed}"
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
    // The draft row follows the current core attachment list directly.
    for file in &core.draft("render-fixture").files {
        core.remove_file("render-fixture", &file.id)?;
    }
    pump(&mut cx)?;
    let geometry = |cx: &HeadlessAppContext| view.read_with(cx, |v, _| v.benchmark_file_geometry());
    anyhow::ensure!(geometry(&cx)["files"].as_array().unwrap().is_empty());
    let before_change = cx.update_window(window.into(), |_, w, _| w.frame_duration_snapshot())?;
    let first = core.attach_file("render-fixture", "first.png", chart_bytes.get_ref())?;
    pump(&mut cx)?;
    let single = geometry(&cx);
    anyhow::ensure!(single["files"].as_array().unwrap().len() == 1);
    cx.capture_screenshot(window.into())?
        .save(output.join("count-1.png"))?;
    let second = core.attach_file("render-fixture", "second.png", chart_bytes.get_ref())?;
    pump(&mut cx)?;
    let two = geometry(&cx);
    anyhow::ensure!(two["files"].as_array().unwrap().len() == 2);
    act(
        &mut cx,
        json!({"type":"click","target":{"element_id":format!("remove-{}",first.id)}}),
    )?;
    pump(&mut cx)?;
    anyhow::ensure!(core.draft("render-fixture").files.len() == 1);
    anyhow::ensure!(geometry(&cx)["files"].as_array().unwrap().len() == 1);
    core.remove_file("render-fixture", &second.id)?;
    pump(&mut cx)?;
    anyhow::ensure!(geometry(&cx)["files"].as_array().unwrap().is_empty());
    let stable = view.update(&mut cx, |v, cx| v.benchmark_region_counts(cx));
    for _ in 0..4 {
        pump(&mut cx)?;
    }
    let after = view.read_with(&cx, |v, cx| v.benchmark_region_counts(cx));
    anyhow::ensure!(stable == after, "static attachment row kept redrawing");
    let mut timings = cx.update_window(window.into(), |_, w, _| w.frame_duration_snapshot())?;
    timings
        .draw_duration_histogram
        .subtract(&before_change.draw_duration_histogram)?;
    let p95 = timings.draw_duration_histogram.value_at_quantile(0.95) as f64 / 1e6;
    std::fs::write(
        output.join("attachment-states.json"),
        serde_json::to_vec_pretty(
            &json!({"single":single,"two":two,"idle_stable":true,"p95_draw_ms":p95}),
        )?,
    )?;
    // A delivered file remains available through its message preview control.
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
    let maximum = geometry(&cx);
    anyhow::ensure!(maximum["files"].as_array().unwrap().len() == 16);
    let controls = driver
        .snapshot(true)
        .elements
        .into_iter()
        .filter(|e| e.id.starts_with("remove-"))
        .collect::<Vec<_>>();
    anyhow::ensure!(controls.len() == 16);
    for control in controls
        .iter()
        .filter(|control| control.visible_bounds.width > 0.)
    {
        anyhow::ensure!(
            control.bounds.width >= 24.
                && control.bounds.height >= 24.
                && control.bounds.y >= 0.
                && control.bounds.x + control.bounds.width <= 900.,
            "max-count control clipped: {}",
            control.id
        );
    }
    let draft = core.draft("render-fixture").files.clone();
    let last = draft.last().unwrap().id.clone();
    let row_center = driver
        .snapshot(false)
        .elements
        .iter()
        .find(|element| element.id == format!("draft-preview-{}", draft[0].id))
        .unwrap()
        .center;
    act(
        &mut cx,
        json!({"type":"scroll","target":{"x":row_center.x,"y":row_center.y},"delta_x":-2000.,"delta_y":0.}),
    )?;
    let after = driver.snapshot(false).elements;
    let found = after.iter().find(|element| element.id == format!("remove-{last}"));
    anyhow::ensure!(
        found.is_some_and(|element| element.visible && element.bounds == element.visible_bounds),
        "last attachment remove button is unreachable by horizontal scrolling: {:?} row {:?}",
        found.map(|e| (e.visible, e.bounds, e.visible_bounds)),
        after
            .iter()
            .find(|e| e.id == format!("draft-preview-{}", draft[0].id))
            .map(|e| (e.bounds, e.visible_bounds))
    );
    cx.capture_screenshot(window.into())?
        .save(output.join("count-16-row.png"))?;
    println!("PASS: attachment row, core membership, preview controls, clipboard, and compact layouts");
    Ok(())
}
