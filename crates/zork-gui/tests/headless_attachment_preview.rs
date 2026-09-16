//! Message thumbnails and the read-only viewer use production input handlers.
use gpui::{px, AppContext, HeadlessAppContext};
use serde_json::json;
use std::{sync::Arc, time::Duration};
use zork_gui::{
    api::Role,
    assets::EmbeddedAssets,
    automation::{AutomationRoot, HeadlessAutomation},
    views::{RootView, TranscriptLine},
};

fn main() -> anyhow::Result<()> {
    std::env::set_var("ZORK_GUI_TEST_REDUCE_MOTION", "1");
    let output = std::env::var_os("ZORK_ATTACHMENT_PREVIEW_OUTPUT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../artifacts/message-attachments-redesign/native")
        });
    std::fs::create_dir_all(&output)?;
    for (width, height) in [(1280., 800.), (900., 600.)] {
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
        let window = cx.open_window(gpui::size(px(width), px(height)), |_, cx| {
            let root = cx.new(|cx| RootView::render_benchmark_fixture(false, store, cx));
            view = Some(root.clone());
            cx.new(|_| AutomationRoot::new(root))
        })?;
        let view = view.unwrap();
        let core = view.update(&mut cx, |v, _| v.benchmark_core_device());
        let fox = core.attach_file(
            "render-fixture",
            "狐狸.svg",
            include_bytes!("../../zork-ui/assets/avatars/portraits/fox.svg"),
        )?;
        let cat = core.attach_file(
            "render-fixture",
            "猫.svg",
            include_bytes!("../../zork-ui/assets/avatars/portraits/cat.svg"),
        )?;
        let markdown=core.attach_file("render-fixture","使用说明.md","# 头像使用说明\n\n狐狸与猫 · SVG 原始文件\n\n## 保持原始比例\n\n缩放时保持宽高一致，避免拉伸角色轮廓。\n\n## 选择背景\n\n在浅色、深色和透明背景下检查边缘。".as_bytes())?;
        let files = vec![fox.clone(), cat.clone(), markdown.clone()];
        for file in &files {
            core.remove_file("render-fixture", &file.id)?;
        }
        core.edit_draft("render-fixture", "保留这段草稿".into())?;
        view.update(&mut cx,|v,cx|{v.benchmark_replace_messages(vec![
            TranscriptLine::Message{role:Role::Assistant,content:zork_client_core::files::compose("两个头像和使用说明都在这里。SVG 保留透明背景，可以直接用于界面。",&files),metadata:serde_json::from_value(json!({"id":"attachment-message","author_name":"产品领队","author_agent_id":"leader-local"})).unwrap()},
        ],cx);v.benchmark_restore_draft(cx);});
        let pump = |cx: &mut HeadlessAppContext| -> anyhow::Result<()> {
            for _ in 0..12 {
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
        let click = |cx: &mut HeadlessAppContext, id: &str| {
            act(cx, json!({"type":"click","target":{"element_id":id}}))
        };
        let menu = |cx: &mut HeadlessAppContext| -> anyhow::Result<()> {
            if driver
                .snapshot(false)
                .elements
                .iter()
                .any(|e| e.id == "preview-more")
            {
                click(cx, "preview-more")
            } else {
                let bounds = driver
                    .snapshot(false)
                    .elements
                    .into_iter()
                    .find(|e| e.id == "drive-preview")
                    .unwrap()
                    .bounds;
                act(
                    cx,
                    json!({"type":"click","target":{"x":bounds.x+bounds.width/2.,"y":bounds.y+bounds.height/2.},"button":"right"}),
                )
            }
        };
        let save = |cx: &mut HeadlessAppContext, name: &str| -> anyhow::Result<()> {
            cx.capture_screenshot(window.into())?
                .save(output.join(format!("{width:.0}-{name}.png")))?;
            Ok(())
        };
        pump(&mut cx)?;
        let id = format!("message-file-0-{}", fox.id);
        let before = driver
            .snapshot(false)
            .elements
            .into_iter()
            .find(|e| e.id == id)
            .expect("image thumbnail missing")
            .bounds;
        save(&mut cx, "message")?;
        click(&mut cx, &id)?;
        anyhow::ensure!(
            driver
                .snapshot(false)
                .elements
                .iter()
                .any(|e| e.id == "preview-zoom-in" && e.enabled),
            "local bytes did not load"
        );
        anyhow::ensure!(driver
            .snapshot(false)
            .elements
            .iter()
            .any(|e| e.id == "preview-next" && e.enabled));
        save(&mut cx, "svg")?;
        anyhow::ensure!(
            !driver
                .snapshot(false)
                .elements
                .iter()
                .any(|e| e.id == "preview-background"),
            "image preview exposed background settings"
        );
        click(&mut cx, "preview-actual")?;
        for _ in 0..6 {
            click(&mut cx, "preview-zoom-in")?;
        }
        save(&mut cx, "zoom")?;
        let bounds = driver
            .snapshot(false)
            .elements
            .into_iter()
            .find(|e| e.id == "drive-preview")
            .unwrap()
            .bounds;
        let before_pan = cx.capture_screenshot(window.into())?;
        act(
            &mut cx,
            json!({"type":"drag","from":{"x":bounds.x+bounds.width/2.,"y":bounds.y+bounds.height/2.},"to":{"x":bounds.x+bounds.width/2.-80.,"y":bounds.y+bounds.height/2.-60.},"steps":12}),
        )?;
        let after_pan = cx.capture_screenshot(window.into())?;
        let scale = driver.snapshot(false).scale_factor;
        let x = ((bounds.x + bounds.width / 2. - 100.) * scale) as u32;
        let y = ((bounds.y + bounds.height / 2. - 80.) * scale) as u32;
        anyhow::ensure!(
            image::imageops::crop_imm(
                &before_pan,
                x,
                y,
                (200. * scale) as u32,
                (160. * scale) as u32
            )
            .to_image()
                != image::imageops::crop_imm(
                    &after_pan,
                    x,
                    y,
                    (200. * scale) as u32,
                    (160. * scale) as u32
                )
                .to_image(),
            "drag did not pan the image content"
        );
        save(&mut cx, "pan")?;
        click(&mut cx, "preview-fit")?;
        menu(&mut cx)?;
        click(&mut cx, "preview-menu-0-preview-source")?;
        anyhow::ensure!(
            !driver
                .snapshot(false)
                .elements
                .iter()
                .any(|e| e.id == "preview-zoom-in"),
            "source mode kept image tools"
        );
        save(&mut cx, "source")?;
        let bounds = driver
            .snapshot(false)
            .elements
            .into_iter()
            .find(|e| e.id == "drive-preview")
            .unwrap()
            .bounds;
        act(
            &mut cx,
            json!({"type":"drag","from":{"x":bounds.x+26.,"y":bounds.y+32.},"to":{"x":bounds.x+180.,"y":bounds.y+32.},"steps":10}),
        )?;
        act(&mut cx, json!({"type":"key","keystroke":"cmd-c"}))?;
        let copied = cx.update(|cx| cx.read_from_clipboard().and_then(|item| item.text()));
        anyhow::ensure!(
            copied.as_deref().is_some_and(|text| text.contains("svg")),
            "source selection did not copy actual text"
        );
        act(
            &mut cx,
            json!({"type":"click","target":{"x":bounds.x+30.,"y":bounds.y+bounds.height-20.}}),
        )?;
        act(&mut cx, json!({"type":"key","keystroke":"right"}))?;
        anyhow::ensure!(driver
            .snapshot(false)
            .elements
            .iter()
            .any(|e| e.id == "preview-previous" && e.enabled));
        click(&mut cx, "preview-next")?;
        anyhow::ensure!(
            driver
                .snapshot(false)
                .elements
                .iter()
                .any(|e| e.id == "preview-next" && !e.enabled),
            "navigation escaped the opening message"
        );
        save(&mut cx, "markdown")?;
        menu(&mut cx)?;
        click(&mut cx, "preview-menu-0-drive-reuse")?;
        anyhow::ensure!(
            core.draft("render-fixture").files == vec![markdown.clone()],
            "reuse did not preserve selected file identity"
        );
        anyhow::ensure!(
            core.draft("render-fixture").text == "保留这段草稿",
            "reuse cleared text"
        );
        act(&mut cx, json!({"type":"key","keystroke":"escape"}))?;
        anyhow::ensure!(!driver
            .snapshot(false)
            .elements
            .iter()
            .any(|e| e.id == "drive-close-preview"));
        let after = driver
            .snapshot(false)
            .elements
            .into_iter()
            .find(|e| e.id == id)
            .expect("closing lost message")
            .bounds;
        anyhow::ensure!(
            (before.y - after.y).abs() < 1.,
            "closing moved the message reading anchor"
        );
        click(&mut cx, &id)?;
        act(&mut cx, json!({"type":"click","target":{"x":4.,"y":100.}}))?;
        anyhow::ensure!(
            !driver
                .snapshot(false)
                .elements
                .iter()
                .any(|e| e.id == "drive-close-preview"),
            "outside click did not close preview"
        );
        std::fs::write(
            output.join(format!("{width:.0}-elements.json")),
            serde_json::to_vec_pretty(&driver.snapshot(false))?,
        )?;
        for (name, w, h, expected_w, expected_h) in [
            ("landscape", 300, 200, 300., 200.),
            ("portrait", 200, 300, 200., 300.),
            ("panorama", 1000, 100, 300., 100.),
            ("long-capture", 100, 1000, 100., 300.),
        ] {
            let mut bytes = std::io::Cursor::new(Vec::new());
            image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
                w,
                h,
                image::Rgba([120, 165, 190, 255]),
            ))
            .write_to(&mut bytes, image::ImageFormat::Png)?;
            let file = core.attach_file(
                "render-fixture",
                &format!("{name}.png"),
                &bytes.into_inner(),
            )?;
            core.remove_file("render-fixture", &file.id)?;
            view.update(&mut cx, |v, cx| {
                v.benchmark_replace_messages(
                    vec![TranscriptLine::Message {
                        role: Role::Assistant,
                        content: zork_client_core::files::compose("", &[file.clone()]),
                        metadata: serde_json::from_value(json!({"id":name})).unwrap(),
                    }],
                    cx,
                )
            });
            pump(&mut cx)?;
            let bounds = driver
                .snapshot(false)
                .elements
                .into_iter()
                .find(|e| e.id == format!("message-file-0-{}", file.id))
                .unwrap()
                .bounds;
            anyhow::ensure!(
                (bounds.width - expected_w).abs() < 1. && (bounds.height - expected_h).abs() < 1.,
                "{name}: incorrect natural/cropped frame {bounds:?}"
            );
        }
    }
    println!("PASS: message thumbnails, SVG zoom/source, same-message navigation, Markdown, reuse, preserved draft and reading anchor, Esc/outside dismissal at 1280 and 900 px");
    Ok(())
}
