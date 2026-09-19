//! The production composer keeps compact controls and semantic send states.
use gpui::{px, AppContext, HeadlessAppContext};
use serde_json::json;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use zork_gui::{
    assets::EmbeddedAssets,
    automation::{AutomationRoot, HeadlessAutomation},
    views::RootView,
};

fn main() -> anyhow::Result<()> {
    std::env::set_var("ZORK_GUI_TEST_REDUCE_MOTION", "1");
    let baseline = std::env::var_os("ZORK_COMPOSER_BASELINE").is_some();
    let output = std::env::var_os("ZORK_COMPOSER_OUTPUT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../artifacts/composer-controls/after")
        });
    std::fs::create_dir_all(&output)?;
    let mut reports = Vec::new();
    for (width, height) in [(900., 600.), (1280., 800.)] {
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
            cx.set_reduce_motion(true);
            HeadlessAutomation::install(cx)
        });
        let mut view = None;
        let window = cx.open_window(gpui::size(px(width), px(height)), |_, cx| {
            let root = cx.new(|cx| RootView::render_benchmark_fixture(false, store.clone(), cx));
            view = Some(root.clone());
            cx.new(|_| AutomationRoot::new(root))
        })?;
        cx.update_window(window.into(), |_, window, _| window.activate_window())?;
        let view = view.unwrap();
        view.update(&mut cx, |v, cx| {
            v.benchmark_bind_core(cx);
            v.benchmark_restore_draft(cx);
        });
        let core = view.update(&mut cx, |v, _| v.benchmark_core_device());
        let pump = |cx: &mut HeadlessAppContext| -> anyhow::Result<()> {
            for _ in 0..4 {
                cx.advance_clock(Duration::from_millis(16));
                cx.run_until_parked();
                cx.update_window(window.into(), |_, w, cx| {
                    w.simulate_next_frame(cx);
                })?;
            }
            Ok(())
        };
        let act = |cx: &mut HeadlessAppContext, value| -> anyhow::Result<()> {
            cx.update_window(window.into(), |_, w, cx| {
                driver.dispatch(serde_json::from_value(value).unwrap(), w, cx)
            })??;
            pump(cx)
        };
        pump(&mut cx)?;
        anyhow::ensure!(
            cx.update_window(window.into(), |_, window, _| window.is_window_active())?,
            "selection rendering requires an active headless window"
        );
        let messages = view.update(&mut cx, |view, _| view.benchmark_record_count(false));
        if let Ok(requested) = std::env::var("ZORK_BENCH_MESSAGE_COUNT") {
            anyhow::ensure!(
                messages == requested.parse::<usize>()?,
                "fixture did not load the requested message count"
            );
        }
        let snapshot = driver.snapshot(false);
        let find = |id: &str| snapshot.elements.iter().find(|e| e.id == id).unwrap();
        let send = find("send-button").bounds;
        let options = find("composer-options").bounds;
        let input = find("composer-input").bounds;
        let surface = find("composer-surface").bounds;
        let extent = if baseline { 32. } else { 24. };
        anyhow::ensure!(
            send.width == extent
                && send.height == extent
                && options.width == extent
                && options.height == extent,
            "composer controls are not the compact size: {send:?} {options:?}"
        );
        anyhow::ensure!(
            send.y >= input.y + input.height && options.y >= input.y + input.height,
            "controls overlap editable text"
        );
        anyhow::ensure!(
            send.x + send.width <= surface.x + surface.width
                && send.y + send.height <= surface.y + surface.height,
            "send control escaped the composer"
        );
        anyhow::ensure!(!find("send-button").enabled, "empty composer allows send");
        let scale = snapshot.scale_factor;
        let sample = |cx: &mut HeadlessAppContext| -> anyhow::Result<[u8; 4]> {
            Ok(cx
                .capture_screenshot(window.into())?
                .get_pixel(
                    ((send.x + 5.) * scale) as u32,
                    ((send.y + send.height / 2.) * scale) as u32,
                )
                .0)
        };
        anyhow::ensure!(
            sample(&mut cx)?[..3]
                == if baseline {
                    [170, 170, 164]
                } else {
                    zork_ui::design::INTERACTION.neutral_pressed.to_be_bytes()[1..]
                        .try_into()
                        .unwrap()
                },
            "disabled send should use the shared primary action state"
        );
        cx.capture_screenshot(window.into())?
            .save(output.join(format!("composer-{width}-empty.png")))?;
        act(
            &mut cx,
            json!({"type":"click","target":{"element_id":"composer-input"}}),
        )?;
        act(
            &mut cx,
            json!({"type":"type_text","text":"检查紧凑发送按钮"}),
        )?;
        anyhow::ensure!(
            core.draft("render-fixture").text == "检查紧凑发送按钮",
            "input did not update the shared draft"
        );
        let command = if cfg!(target_os = "macos") {
            "cmd"
        } else {
            "ctrl"
        };
        let select_line_start = if cfg!(target_os = "macos") {
            "cmd-shift-left"
        } else {
            "shift-home"
        };
        let select_document_start = if cfg!(target_os = "macos") {
            "cmd-shift-up"
        } else {
            "ctrl-shift-home"
        };
        for (step, action) in [
            json!({"type":"key","keystroke":"shift-enter"}),
            json!({"type":"type_text","text":"second line"}),
            json!({"type":"key","keystroke":select_line_start}),
            json!({"type":"type_text","text":"替换"}),
        ]
        .into_iter()
        .enumerate()
        {
            act(&mut cx, action)?;
            if step == 2 {
                let screenshot = cx.capture_screenshot(window.into())?;
                let snapshot = driver.snapshot(false);
                let bounds = snapshot
                    .elements
                    .iter()
                    .find(|e| e.id == "composer-input")
                    .unwrap()
                    .bounds;
                let scale = snapshot.scale_factor;
                let mut selection_pixels = 0;
                for y in (bounds.y * scale) as u32..((bounds.y + bounds.height) * scale) as u32 {
                    for x in (bounds.x * scale) as u32..((bounds.x + bounds.width) * scale) as u32 {
                        let color = screenshot.get_pixel(x, y).0;
                        if color[2] as u16 > color[0] as u16 + 12
                            && color[2] as u16 > color[1] as u16 + 3
                        {
                            selection_pixels += 1;
                        }
                    }
                }
                anyhow::ensure!(
                    selection_pixels > 50,
                    "keyboard selection is not visibly highlighted"
                );
                screenshot.save(output.join(format!("composer-{width}-line-selection.png")))?;
            }
        }
        anyhow::ensure!(
            core.draft("render-fixture").text == "检查紧凑发送按钮\n替换",
            "line selection crossed into the previous line"
        );
        act(
            &mut cx,
            json!({"type":"key","keystroke":format!("{command}-z")}),
        )?;
        anyhow::ensure!(
            core.draft("render-fixture").text == "检查紧凑发送按钮\nsecond line",
            "replacement typing did not undo through the draft subscription as one edit: {:?}",
            core.draft("render-fixture").text
        );
        act(
            &mut cx,
            json!({"type":"key","keystroke":format!("{command}-shift-z")}),
        )?;
        anyhow::ensure!(
            core.draft("render-fixture").text == "检查紧凑发送按钮\n替换",
            "redo did not publish the restored draft"
        );
        act(
            &mut cx,
            json!({"type":"key","keystroke":select_document_start}),
        )?;
        act(
            &mut cx,
            json!({"type":"type_text","text":"检查紧凑发送按钮"}),
        )?;
        anyhow::ensure!(
            core.draft("render-fixture").text == "检查紧凑发送按钮",
            "document selection did not replace the whole draft"
        );
        anyhow::ensure!(
            driver
                .snapshot(false)
                .elements
                .iter()
                .any(|e| e.id == "send-button" && e.enabled),
            "text did not enable send"
        );
        let expected = if baseline {
            0x24282Bu32
        } else {
            zork_ui::design::BRAND_ACCENT
        };
        anyhow::ensure!(
            sample(&mut cx)?[..3] == expected.to_be_bytes()[1..],
            "ready send should use the shared primary action color"
        );
        cx.capture_screenshot(window.into())?
            .save(output.join(format!("composer-{width}-ready.png")))?;
        act(
            &mut cx,
            json!({"type":"move","target":{"element_id":"send-button"}}),
        )?;
        if !baseline {
            let expected = zork_ui::design::INTERACTION.accent_hover.to_be_bytes();
            // Drain pending frame deliveries and the retained hover spring
            // before checking the final color.
            for _ in 0..32 {
                if sample(&mut cx)?[..3] == expected[1..] {
                    break;
                }
                pump(&mut cx)?;
            }
            let actual = sample(&mut cx)?;
            cx.capture_screenshot(window.into())?
                .save(output.join(format!("composer-{width}-hover.png")))?;
            anyhow::ensure!(
                actual[..3] == expected[1..],
                "send hover did not use shared primary feedback: {actual:?}"
            );
        }
        cx.capture_screenshot(window.into())?
            .save(output.join(format!("composer-{width}-hover.png")))?;
        let mut samples = Vec::new();
        for frame in 0..140 {
            let start = Instant::now();
            cx.update_window(window.into(), |_, w, cx| {
                w.refresh();
                w.draw(cx).clear(cx)
            })?;
            if frame >= 20 {
                samples.push(start.elapsed().as_secs_f64() * 1000.);
            }
        }
        samples.sort_by(f64::total_cmp);
        let p95 = samples[114];
        let p99 = samples[118];
        anyhow::ensure!(p95 < 8.33, "composer draw exceeds CPU frame budget: {p95}");
        let previous = store
            .outbox("mini1")?
            .into_iter()
            .map(|message| message.request_id)
            .collect::<std::collections::HashSet<_>>();
        act(
            &mut cx,
            json!({"type":"click","target":{"element_id":"send-button"}}),
        )?;
        anyhow::ensure!(
            core.draft("render-fixture").text.is_empty(),
            "compact send did not submit the draft"
        );
        let pending = store.outbox("mini1")?;
        anyhow::ensure!(
            pending.len() == previous.len() + 1
                && previous
                    .iter()
                    .all(|id| pending.iter().any(|m| &m.request_id == id)),
            "send should preserve existing pending deliveries and add exactly one offline message"
        );
        let added = pending
            .iter()
            .find(|m| !previous.contains(&m.request_id))
            .unwrap();
        anyhow::ensure!(
            added.content == "检查紧凑发送按钮",
            "queued message lost the submitted input"
        );
        reports.push(json!({"width":width,"height":height,"messages":messages,"all_message_types":std::env::var_os("ZORK_SCROLL_ALL_MESSAGES").is_some(),"send":send,"options":options,"input":input,"surface":surface,"frames":samples.len(),"p95_ms":p95,"p99_ms":p99}));
    }
    std::fs::write(
        output.join("report.json"),
        serde_json::to_vec_pretty(&reports)?,
    )?;
    println!("PASS composer controls: geometry, keyboard selection/undo/redo through shared drafts, one offline send, CPU budget");
    Ok(())
}
