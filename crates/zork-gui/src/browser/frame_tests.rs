use super::*;
use gpui::{point, size, AppContext, RenderImage, TestAppContext, VisualTestContext};

fn frame(width: u32, height: u32, color: u8) -> Arc<RenderImage> {
    Arc::new(RenderImage::new(vec![image::Frame::new(
        image::RgbaImage::from_pixel(width, height, image::Rgba([color, 100, 200, 255])),
    )]))
}

fn paint(cx: &mut VisualTestContext, image: &Arc<RenderImage>) {
    cx.draw(point(px(0.), px(0.)), size(px(100.), px(100.)), |_, _| {
        img(image.clone()).size_full().into_any_element()
    });
    cx.update(|window, _| assert!(window.has_image_atlas_entry(image)));
}

#[gpui::test]
fn streamed_frames_retire_textures_in_and_outside_window_updates(cx: &mut TestAppContext) {
    let cx = cx.add_empty_window();
    let panel = cx.new(BrowserPanel::new);
    let mut previous: Option<Arc<RenderImage>> = None;
    for index in 0..128 {
        let image = frame(40 + index % 7, 60 + index % 11, index as u8);
        if index % 2 == 0 {
            // Frame delivery runs outside a window update.
            panel.update(cx, |panel, cx| panel.set_frame(Some(image.clone()), cx));
        } else {
            // Tab reconciliation also runs during the current window's render.
            cx.update(|_, cx| {
                panel.update(cx, |panel, cx| panel.set_frame(Some(image.clone()), cx))
            });
        }
        paint(cx, &image);
        if let Some(previous) = previous {
            cx.update(|window, _| assert!(!window.has_image_atlas_entry(&previous)));
        }
        previous = Some(image);
    }
}

#[gpui::test]
fn switching_closing_and_releasing_panels_retire_textures(cx: &mut TestAppContext) {
    let cx = cx.add_empty_window();
    for action in ["host", "tab", "closed", "clear", "release"] {
        let panel = cx.new(BrowserPanel::new);
        let image = frame(64, 64, 50);
        panel.update(cx, |panel, cx| panel.set_frame(Some(image.clone()), cx));
        paint(cx, &image);
        cx.update(|_, cx| {
            panel.update(cx, |panel, cx| match action {
                "host" => panel.set_host("another-chat".into(), cx),
                "tab" => panel.select("another-tab".into(), cx),
                "closed" => {
                    panel
                        .selected
                        .insert(panel.host.clone(), "closed-tab".into());
                    panel.refresh_presentation(cx);
                }
                "clear" => panel.set_frame(None, cx),
                "release" => {}
                _ => unreachable!(),
            });
        });
        if action == "release" {
            cx.update(|_, _| drop(panel));
        }
        cx.update(|window, _| {
            assert!(
                !window.has_image_atlas_entry(&image),
                "{action} retained its texture"
            )
        });
    }
}

// Run separately from compilation and other tests for comparable Metal/footprint
// measurements: cargo test --locked -p zork-gui --lib browser_frame_memory --
// --ignored --nocapture --test-threads=1
#[cfg(target_os = "macos")]
#[test]
#[ignore = "isolated Metal memory and frame-time regression"]
fn browser_frame_memory() -> anyhow::Result<()> {
    struct Page(Entity<BrowserPanel>);
    impl Render for Page {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .when_some(self.0.read(cx).frame.clone(), |v, image| {
                    v.child(img(image).size_full())
                })
        }
    }
    fn footprint() -> u64 {
        let mut usage = std::mem::MaybeUninit::<libc::rusage_info_v4>::zeroed();
        assert_eq!(
            unsafe {
                libc::proc_pid_rusage(
                    std::process::id() as i32,
                    libc::RUSAGE_INFO_V4,
                    usage.as_mut_ptr().cast(),
                )
            },
            0
        );
        unsafe { usage.assume_init().ri_phys_footprint }
    }
    let mut cx = gpui::HeadlessAppContext::with_platform(
        Arc::new(gpui::NoopTextSystem::new()),
        Arc::new(()),
        gpui_platform::current_headless_renderer,
    );
    let panel = cx.new(BrowserPanel::new);
    let window = cx.open_window(size(px(1040.), px(780.)), |_, cx| {
        cx.new(|_| Page(panel.clone()))
    })?;
    let mut durations = vec![];
    let mut warm = 0;
    let mut peak = 0;
    for index in 0..136 {
        let image = frame(1040, 1560, index as u8);
        let began = Instant::now();
        // Match the native event loop: drain temporary Metal/readback objects
        // every frame instead of charging the test's autoreleases to the app.
        objc2::rc::autoreleasepool(|_| -> anyhow::Result<()> {
            window.update(&mut cx, |page, _, cx| {
                page.0
                    .update(cx, |panel, cx| panel.set_frame(Some(image), cx));
                cx.notify();
            })?;
            cx.update_window(window.into(), |_, window, cx| {
                window.simulate_next_frame(cx);
                window.draw(cx).clear(cx);
            })?;
            cx.run_until_parked();
            // Complete each render and check its pixels. Timings include GPU
            // completion/readback, not display FPS.
            let screenshot = cx.capture_screenshot(window.into())?;
            assert_eq!(
                screenshot.get_pixel(500, 400).0,
                [200, 100, index as u8, 255]
            );
            Ok(())
        })?;
        if index == 7 {
            warm = footprint();
        } else if index >= 8 {
            durations.push(began.elapsed().as_secs_f64() * 1000.);
            peak = peak.max(footprint());
        }
    }
    let end = footprint();
    durations.sort_by(f64::total_cmp);
    println!(
        "{}",
        serde_json::json!({
            "frames": durations.len(), "image": [1040, 1560], "window": [1040, 780],
            "warm_bytes": warm, "peak_bytes": peak, "end_bytes": end,
            "growth_bytes": peak.saturating_sub(warm),
            "frame_readback_p95_ms": durations[durations.len() * 95 / 100],
            "frame_readback_p99_ms": durations[durations.len() * 99 / 100],
        })
    );
    assert!(
        peak.saturating_sub(warm) < 64 * 1024 * 1024,
        "old browser frames accumulated in GPU memory"
    );
    Ok(())
}
