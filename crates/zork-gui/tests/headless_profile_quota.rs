//! Profile quota rendering, compact modal geometry, and CPU draw regression evidence.
use gpui::{
    div, prelude::*, px, rgb, AppContext, Context, Entity, HeadlessAppContext, Render, Window,
};
use serde_json::json;
use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use zork_gui::desktop::HeadlessProfilesView;
use zork_gui::{
    assets::EmbeddedAssets,
    automation::{AutomationRoot, HeadlessAutomation},
};
struct Frame<V: Render + 'static> {
    view: Entity<V>,
}
impl<V: Render + 'static> Render for Frame<V> {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .font_family("Inter Variable")
            .text_size(px(13.))
            .bg(rgb(0xf7f6f2))
            .child(div().ml(px(240.)).h_full().bg(rgb(0xffffff)).child(
                zork_gui::desktop::headless_settings_content(self.view.clone()),
            ))
    }
}
fn main() -> anyhow::Result<()> {
    if std::env::args().any(|v| v == "--refresh-only") {
        return verify_refresh();
    }
    let output = PathBuf::from(
        std::env::var("ZORK_QUOTA_RENDER_OUTPUT").unwrap_or_else(|_| {
            format!(
                "{}/../../artifacts/profile-quota/render",
                env!("CARGO_MANIFEST_DIR")
            )
        }),
    );
    std::fs::create_dir_all(&output)?;
    let mut reports = Vec::new();
    for (width, height) in [(900., 600.), (1280., 800.)] {
        for detail in [false, true] {
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
            let cold = Instant::now();
            let window = cx.open_window(gpui::size(px(width), px(height)), |_, cx| {
                let inner = cx.new(|cx| HeadlessProfilesView::headless_fixture(detail, cx));
                view = Some(inner.clone());
                let frame = cx.new(|_| Frame { view: inner });
                cx.new(|_| AutomationRoot::new(frame))
            })?;
            let view = view.unwrap();
            cx.update_window(window.into(), |_, w, cx| w.draw(cx).clear(cx))?;
            let cold_ms = cold.elapsed().as_secs_f64() * 1000.;
            let mut samples = Vec::new();
            for index in 0..140 {
                cx.advance_clock(Duration::from_millis(16));
                cx.run_until_parked();
                view.update(&mut cx, |_, cx| cx.notify());
                let started = Instant::now();
                cx.update_window(window.into(), |_, w, cx| w.draw(cx).clear(cx))?;
                if index >= 20 {
                    samples.push(started.elapsed().as_secs_f64() * 1000.);
                }
            }
            samples.sort_by(f64::total_cmp);
            anyhow::ensure!(
                samples[114] < 8.33,
                "profile draw exceeds the 8.33 ms p95 budget"
            );
            for idle_frame in 0..8 {
                cx.advance_clock(Duration::from_millis(250));
                cx.run_until_parked();
                let requests = cx.update_window(window.into(), |_, w, cx| {
                    let requests = w.simulate_next_frame(cx);
                    if requests > 0 {
                        w.draw(cx).clear(cx);
                    }
                    requests
                })?;
                if idle_frame > 5 {
                    anyhow::ensure!(requests == 0, "idle quota view kept repainting");
                }
            }
            let snapshot = driver.snapshot(false);
            if detail {
                let modal = snapshot
                    .elements
                    .iter()
                    .find(|e| e.id == "profile-detail-dialog")
                    .expect("detail modal");
                anyhow::ensure!(
                    modal.visible
                        && modal.bounds == modal.visible_bounds
                        && modal.bounds.y + modal.bounds.height <= height,
                    "quota modal clipped"
                );
                anyhow::ensure!(
                    (modal.bounds.x + modal.bounds.width / 2. - width / 2.).abs() < 1.,
                    "quota dialog is not centered over the window"
                );
                let close = snapshot
                    .elements
                    .iter()
                    .find(|e| e.id == "profile-detail-dialog-close")
                    .expect("detail close button");
                if !(close.visible && close.enabled && close.bounds == close.visible_bounds) {
                    cx.capture_screenshot(window.into())?
                        .save(output.join(format!("detail-close-clipped-{width}.png")))?;
                    std::fs::write(
                        output.join(format!("detail-close-clipped-{width}.json")),
                        serde_json::to_vec_pretty(&snapshot)?,
                    )?;
                }
                anyhow::ensure!(
                    close.visible && close.enabled && close.bounds == close.visible_bounds,
                    "detail close button clipped"
                );
                let rename = snapshot
                    .elements
                    .iter()
                    .find(|e| e.id == "profile-rename")
                    .expect("rename action");
                anyhow::ensure!(
                    (rename.bounds.y + rename.bounds.height / 2.
                        - close.bounds.y
                        - close.bounds.height / 2.)
                        .abs()
                        < 1.,
                    "rename action is not beside the title"
                );
                let discover = snapshot
                    .elements
                    .iter()
                    .find(|e| e.id == "profile-model-discover")
                    .unwrap();
                anyhow::ensure!(
                    discover.label == "更新模型",
                    "model refresh label is outdated"
                );
            }
            let name = format!("{}-{width}", if detail { "detail" } else { "list" });
            cx.capture_screenshot(window.into())?
                .save(output.join(format!("{name}.png")))?;
            std::fs::write(
                output.join(format!("{name}.json")),
                serde_json::to_vec_pretty(&snapshot)?,
            )?;
            if !detail {
                let row = snapshot
                    .elements
                    .iter()
                    .find(|e| e.id == "profile-detail-fixture")
                    .unwrap();
                let avatar = snapshot
                    .elements
                    .iter()
                    .find(|e| e.id == "profile-provider-mark-fixture")
                    .unwrap();
                anyhow::ensure!(
                    avatar.bounds.x - row.bounds.x >= 12.,
                    "hover background touches avatar"
                );
                let action = serde_json::from_value(
                    json!({"type":"move","target":{"element_id":"profile-detail-fixture"}}),
                )?;
                cx.update_window(window.into(), |_, w, cx| driver.dispatch(action, w, cx))??;
                cx.run_until_parked();
                cx.update_window(window.into(), |_, w, cx| w.draw(cx).clear(cx))?;
                cx.capture_screenshot(window.into())?
                    .save(output.join(format!("hover-{width}.png")))?;
            }
            reports.push(json!({"scene":name,"cold_first_draw_ms":cold_ms,"frames":samples.len(),"p95_cpu_draw_ms":samples[114],"p99_cpu_draw_ms":samples[118]}));
        }
    }
    std::fs::write(
        output.join("performance.json"),
        serde_json::to_vec_pretty(&reports)?,
    )?;
    println!("{}", serde_json::to_string_pretty(&reports)?);
    if std::env::var_os("ZORK_QUOTA_RENDER_ONLY").is_none() {
        verify_cases(&output)?;
        verify_refresh()?;
    }
    Ok(())
}
fn verify_cases(output: &std::path::Path) -> anyhow::Result<()> {
    use zork_gui::{desktop::HeadlessProfilesView as CurrentProfilesView, i18n::Locale};
    for (width, height) in [(900., 600.), (1280., 800.)] {
        for state in [
            "windows",
            "empty",
            "failed",
            "balance",
            "exhausted",
            "additional",
            "english",
            "model-catalog",
        ] {
            let mut profile = zork_ui::stories::page_fixture()["profile"].clone();
            match state {
                "empty" => profile["rateLimits"] = json!({"ok":true,"reported":false}),
                "failed" => {
                    profile["rateLimits"] =
                        json!({"ok":false,"error":"private provider diagnostic must not appear"})
                }
                "balance" => {
                    profile["rateLimits"] =
                        json!({"ok":true,"rateLimits":{"credits":{"balance":"0.00","unit":"USD"}}})
                }
                "exhausted" => {
                    profile["rateLimits"]["rateLimits"]["primary"]["usedPercent"] = json!(100)
                }
                "model-catalog" => {
                    let model = profile["models"][0].clone();
                    profile["models"] = json!((0..500)
                        .map(|index| {
                            let mut m = model.clone();
                            m["id"] = json!(format!("catalog-{index:03}"));
                            m["enabled"] = json!(index % 2 == 0);
                            m["default"] = json!(index == 0);
                            m
                        })
                        .collect::<Vec<_>>());
                }
                "additional" => {
                    profile["rateLimits"]["rateLimitsByLimitId"] = json!({"sonnet":{"limitName":"Sonnet","secondary":{"usedPercent":15,"windowDurationMins":10080,"resetsAt":1789430400}}})
                }
                _ => (),
            }
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
            let mut rendered_view = None;
            let window = cx.open_window(gpui::size(px(width), px(height)), |_, cx| {
                let inner = cx.new(|cx| {
                    let mut view = CurrentProfilesView::headless_fixture(true, cx);
                    if state == "english" {
                        view.set_locale(Locale::En, cx);
                    }
                    view.headless_set_profile(profile, cx);
                    view
                });
                rendered_view = Some(inner.clone());
                let frame = cx.new(|_| Frame { view: inner });
                cx.new(|_| AutomationRoot::new(frame))
            })?;
            for _ in 0..30 {
                cx.advance_clock(Duration::from_millis(16));
                cx.run_until_parked();
                cx.update_window(window.into(), |_, w, cx| w.draw(cx).clear(cx))?;
            }
            let snapshot = driver.snapshot(false);
            let quota = snapshot.elements.iter().find(|e| e.id == "profile-quota");
            match state {
                "empty" => anyhow::ensure!(quota.is_none(), "missing quota rendered a placeholder"),
                "failed" => anyhow::ensure!(
                    quota.is_some_and(|e| e.label == "查询失败"),
                    "failure copy is wrong"
                ),
                "balance" => anyhow::ensure!(
                    quota.is_some_and(|e| e.label == "余额 0.00 USD"),
                    "zero balance was hidden"
                ),
                "exhausted" => anyhow::ensure!(
                    quota.is_some_and(|e| e.label.contains("剩余 0%")),
                    "exhausted limit was hidden"
                ),
                "english" => anyhow::ensure!(
                    quota.is_some_and(|e| e.label.contains("Remaining 72%")),
                    "quota locale did not update"
                ),
                _ => anyhow::ensure!(
                    quota.is_some_and(|e| e.label.contains("剩余 72%")),
                    "missing quota values"
                ),
            }
            anyhow::ensure!(
                !serde_json::to_string(&snapshot)?.contains("private provider diagnostic"),
                "provider diagnostics leaked into UI"
            );
            for id in [
                "profile-detail-dialog",
                "profile-detail-dialog-close",
                "profile-quota-refresh",
            ] {
                let element = snapshot
                    .elements
                    .iter()
                    .find(|e| e.id == id)
                    .expect("quota control");
                anyhow::ensure!(
                    element.visible && element.bounds == element.visible_bounds,
                    "{state} at {width}x{height}: clipped {id}: bounds={:?}, visible={:?}",
                    element.bounds,
                    element.visible_bounds
                );
            }
            cx.capture_screenshot(window.into())?
                .save(output.join(format!("{state}-{width}.png")))?;
            std::fs::write(
                output.join(format!("{state}-{width}.json")),
                serde_json::to_vec_pretty(&snapshot)?,
            )?;
            if state == "model-catalog" {
                let view = rendered_view.unwrap();
                let mut samples = Vec::new();
                let mut built_max = 0;
                let mut seen = std::collections::HashSet::new();
                for index in 0..120 {
                    view.update(&mut cx, |view, _| {
                        view.headless_model_rows_built();
                    });
                    let action = serde_json::from_value(
                        json!({"type":"scroll","target":{"element_id":"profile-models"},"delta_y":if index<60 {-640.}else{640.}}),
                    )?;
                    let start = Instant::now();
                    cx.update_window(window.into(), |_, window, cx| {
                        driver.dispatch(action, window, cx)
                    })??;
                    cx.advance_clock(Duration::from_millis(16));
                    // HeadlessAppContext paints dirty windows while draining.
                    // A second explicit draw doubled both row work and frames.
                    cx.run_until_parked();
                    if index >= 20 {
                        samples.push(start.elapsed().as_secs_f64() * 1000.);
                    }
                    built_max = built_max
                        .max(view.update(&mut cx, |view, _| view.headless_model_rows_built()));
                    seen.extend(
                        driver
                            .snapshot(false)
                            .elements
                            .into_iter()
                            .filter(|e| e.id.starts_with("model-edit-catalog-"))
                            .map(|e| e.id),
                    );
                }
                samples.sort_by(f64::total_cmp);
                anyhow::ensure!(
                    built_max <= 16,
                    "model catalog built offscreen rows: {built_max}"
                );
                anyhow::ensure!(seen.len() > 100, "model catalog did not actually scroll");
                anyhow::ensure!(samples[94] < 8.33, "model catalog exceeds CPU draw budget");
                std::fs::write(
                    output.join(format!("model-catalog-performance-{width}.json")),
                    serde_json::to_vec_pretty(
                        &json!({"models":500,"built_max":built_max,"seen":seen.len(),"p95_cpu_draw_ms":samples[94],"p99_cpu_draw_ms":samples[98]}),
                    )?,
                )?;
            }
            let action = serde_json::from_value(json!({"type":"key","keystroke":"escape"}))?;
            cx.update_window(window.into(), |_, w, cx| driver.dispatch(action, w, cx))??;
            cx.run_until_parked();
            cx.update_window(window.into(), |_, w, cx| w.draw(cx).clear(cx))?;
            anyhow::ensure!(
                !driver
                    .snapshot(false)
                    .elements
                    .iter()
                    .any(|e| e.id == "profile-detail-dialog"),
                "Escape did not close quota details"
            );
        }
    }
    Ok(())
}

fn verify_refresh() -> anyhow::Result<()> {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::atomic::{AtomicUsize, Ordering},
    };
    use zork_gui::{api::StationClient, desktop::HeadlessProfilesView as CurrentProfilesView};
    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let address = listener.local_addr()?;
    let count = Arc::new(AtomicUsize::new(0));
    let requests = count.clone();
    let release = Arc::new(AtomicUsize::new(0));
    let responses = release.clone();
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
    let window = cx.open_window(gpui::size(px(900.), px(600.)), |_, cx| {
        let view = cx.new(|cx| {
            let mut view = CurrentProfilesView::headless_fixture(true, cx);
            view.headless_set_client(Arc::new(StationClient::new(
                &format!("http://{address}"),
                Some("quota-fixture".into()),
            )));
            view
        });
        let frame = cx.new(|_| Frame { view });
        cx.new(|_| AutomationRoot::new(frame))
    })?;
    cx.update_window(window.into(), |_, w, cx| w.draw(cx).clear(cx))?;
    let server = std::thread::spawn(move || -> anyhow::Result<()> {
        let deadline = Instant::now() + Duration::from_secs(10);
        while requests.load(Ordering::SeqCst) < 2 {
            anyhow::ensure!(Instant::now() < deadline, "quota fixture request timeout");
            let (mut stream, _) = match listener.accept() {
                Ok(value) => value,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(5));
                    continue;
                }
                Err(e) => return Err(e.into()),
            };
            stream.set_read_timeout(Some(Duration::from_secs(2)))?;
            let mut request = Vec::new();
            loop {
                let mut buffer = [0; 1024];
                let n = stream.read(&mut buffer)?;
                request.extend_from_slice(&buffer[..n]);
                if n == 0 || request.windows(4).any(|v| v == b"\r\n\r\n") {
                    break;
                }
            }
            let request = String::from_utf8(request)?;
            eprintln!(
                "quota fixture request: {}",
                request.lines().next().unwrap_or("empty")
            );
            anyhow::ensure!(
                request.starts_with("POST /v1/node/profiles/fixture/refresh "),
                "wrong quota route"
            );
            anyhow::ensure!(
                request
                    .to_ascii_lowercase()
                    .contains("authorization: bearer quota-fixture"),
                "missing node authorization"
            );
            let index = requests.fetch_add(1, Ordering::SeqCst);
            let (status, body) = if index == 0 {
                (
                    "503 Service Unavailable",
                    json!({"error":"private provider diagnostic must not appear"}),
                )
            } else {
                let mut profile = zork_ui::stories::page_fixture()["profile"].clone();
                profile["rateLimits"]["rateLimits"]["primary"]["usedPercent"] = json!(90);
                ("200 OK", profile)
            };
            // Release only after both clicks, independent of rendering or host speed.
            while responses.load(Ordering::SeqCst) <= index {
                anyhow::ensure!(Instant::now() < deadline, "quota response release timeout");
                std::thread::sleep(Duration::from_millis(5));
            }
            let body = serde_json::to_vec(&body)?;
            write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )?;
            stream.write_all(&body)?;
        }
        Ok(())
    });
    for (index, expected) in [(0, "查询失败"), (1, "剩余 10%")] {
        for _ in 0..2 {
            let action = serde_json::from_value(
                json!({"type":"click","target":{"element_id":"profile-quota-refresh"}}),
            )?;
            cx.update_window(window.into(), |_, w, cx| driver.dispatch(action, w, cx))??;
            cx.run_until_parked();
            cx.update_window(window.into(), |_, w, cx| w.draw(cx).clear(cx))?;
        }
        release.store(index + 1, Ordering::SeqCst);
        let started = Instant::now();
        loop {
            cx.advance_clock(Duration::from_millis(16));
            cx.run_until_parked();
            cx.update_window(window.into(), |_, w, cx| w.draw(cx).clear(cx))?;
            let snapshot = driver.snapshot(false);
            if snapshot
                .elements
                .iter()
                .any(|e| e.id == "profile-quota" && e.label.contains(expected))
            {
                break;
            }
            anyhow::ensure!(
                started.elapsed() < Duration::from_secs(5),
                "quota refresh did not render {expected}"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        anyhow::ensure!(
            count.load(Ordering::SeqCst) == index + 1,
            "quota request count after stage {index}: got {}, expected {}",
            count.load(Ordering::SeqCst),
            index + 1
        );
    }
    server
        .join()
        .map_err(|_| anyhow::anyhow!("quota fixture panicked"))??;
    Ok(())
}
