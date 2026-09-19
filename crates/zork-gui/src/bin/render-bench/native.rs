//! Native presentation over the same disconnected desktop fixtures as headless.
use gpui::{prelude::*, px, size, AnyWindowHandle, Bounds, WindowAppearance};
use std::{
    cell::{Cell, RefCell},
    path::Path,
    rc::Rc,
    sync::Arc,
    time::Duration,
};
use zork_gui::{
    assets::EmbeddedAssets,
    automation::{protocol::UserAction, AutomationRoot, HeadlessAutomation},
    views::RootView,
};

pub fn run(name: &str, output: &Path) -> anyhow::Result<()> {
    anyhow::ensure!(
        matches!(name, "history" | "chat" | "markdown" | "mixed" | "files"),
        "unknown workload: {name}"
    );
    std::fs::create_dir_all(output)?;
    let frames = output.join(format!("{name}-frames.json"));
    if frames.exists() {
        std::fs::remove_file(&frames)?;
    }
    std::env::set_var("SEED", "0");
    std::env::set_var("TZ", "UTC");
    std::env::set_var("ZORK_GUI_TEST_REDUCE_MOTION", "1");
    std::env::set_var("ZORK_FRAME_REPORT", &frames);
    if name == "markdown" {
        std::env::set_var("ZORK_SCROLL_MARKDOWN_STRESS", "1");
    } else {
        std::env::remove_var("ZORK_SCROLL_MARKDOWN_STRESS");
    }
    let all_types = matches!(name, "mixed" | "files");
    if all_types {
        std::env::set_var("ZORK_SCROLL_ALL_MESSAGES", "1");
        if std::env::var_os("ZORK_BENCH_MESSAGE_COUNT").is_none() {
            std::env::set_var("ZORK_BENCH_MESSAGE_COUNT", "100000");
        }
    } else {
        std::env::remove_var("ZORK_SCROLL_ALL_MESSAGES");
        std::env::remove_var("ZORK_BENCH_MESSAGE_COUNT");
    }
    let anchor = std::env::var("ZORK_BENCH_ANCHOR")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(100);
    let fixture = tempfile::tempdir()?;
    let preferences = fixture.path().join("preferences.json");
    std::fs::write(&preferences, "{}")?;
    std::env::set_var("ZORK_GUI_PREFERENCES_PATH", preferences);
    let store = Arc::new(zork_gui::desktop::store::ClientStore::open(fixture.path())?);
    let history = name == "history";
    let files = name == "files";
    let speed = if all_types || name == "markdown" {
        6000.
    } else {
        2000.
    };
    let cold = Rc::new(Cell::new(0.));
    let cold_capture = cold.clone();
    let summary = Rc::new(RefCell::new(serde_json::Value::Null));
    let summary_capture = summary.clone();
    let frames_capture = frames.clone();
    let name_capture = name.to_owned();
    gpui_platform::application().with_assets(EmbeddedAssets).run(move |cx| {
        zork_gui::assets::init_fonts(cx);
        zork_gui::components::init(cx);
        cx.set_reduce_motion(true);
        cx.set_window_appearance(Some(WindowAppearance::Light));
        let driver = HeadlessAutomation::install(cx);
        let mut root = None;
        let window: AnyWindowHandle = cx.open_window(gpui::WindowOptions {
            window_bounds: Some(gpui::WindowBounds::Windowed(Bounds::centered(None,size(px(1280.),px(800.)),cx))),
            titlebar: Some(zork_gui::window_chrome::native_titlebar_options()),
            ..Default::default()
        }, |window,cx| {
            window.on_window_should_close(cx,|_,cx|{cx.quit();true});
            let view=cx.new(|cx|RootView::render_benchmark_fixture(history,store,cx));
            root=Some(view.clone());
            cx.new(|_|AutomationRoot::new(view))
        }).expect("native benchmark window").into();
        let root=root.unwrap();
        cx.activate(true);
        cx.spawn(async move |cx| {
            cx.background_executor().timer(Duration::from_secs(1)).await;
            if all_types {
                for index in 0..RootView::benchmark_kind_count()*2 {
                    let _=window.update(cx,|_,_,cx|root.update(cx,|view,cx|view.benchmark_jump_to(index,cx)));
                    cx.background_executor().timer(Duration::from_millis(24)).await;
                }
                for kind in ["png","jpeg","svg","text","markdown","binary","loading","failed"] {
                    let _=window.update(cx,|_,_,cx|root.update(cx,|view,cx|view.benchmark_preview_artifact(kind,cx)));
                    cx.background_executor().timer(Duration::from_millis(80)).await;
                    let _=window.update(cx,|_,_,cx|root.update(cx,|view,cx|view.benchmark_close_artifact(cx)));
                }
                for state in 0..9 {
                    let _=window.update(cx,|_,_,cx|root.update(cx,|view,cx|{view.benchmark_activity(state,cx);}));
                    cx.background_executor().timer(Duration::from_millis(24)).await;
                }
                let _=window.update(cx,|_,_,cx|root.update(cx,|view,cx|{view.benchmark_activity(0,cx);view.benchmark_jump_to(anchor,cx);}));
                cx.background_executor().timer(Duration::from_millis(200)).await;
            }
            if files {
                let mut targets = Vec::new();
                if !driver.snapshot(false).elements.iter().any(|e| e.visible && e.id == "browser-tabs") {
                    targets.push("conversation-browser");
                }
                targets.extend(["conversation-files-button", "conversation-files-all"]);
                for target in targets {
                    // Setup waits for the real overlay's input surface. These
                    // frames are outside the continuous scrolling measurement.
                    for _ in 0..120 {
                        if driver.snapshot(false).elements.iter().any(|e| e.visible && e.id == target) {
                            break;
                        }
                        cx.background_executor().timer(Duration::from_millis(16)).await;
                    }
                    let result = window.update(cx, |_, window, cx| {
                        let action: UserAction = serde_json::from_value(serde_json::json!({"type":"click","target":{"element_id":target}})).unwrap();
                        driver.dispatch(action, window, cx)
                    });
                    if let Err(error) = result.and_then(|result| result) {
                        eprintln!("native file-list setup ({target}): {error:#}");
                        std::process::exit(1);
                    }
                }
                for _ in 0..120 {
                    if driver.snapshot(false).elements.iter().any(|e| e.visible && e.id == "conversation-artifacts") {
                        break;
                    }
                    cx.background_executor().timer(Duration::from_millis(16)).await;
                }
            }
            if history && !driver.snapshot(false).elements.iter().any(|e| e.visible && e.id == "history-ledger") {
                eprintln!("native history fixture did not mount its history page");
                std::process::exit(1);
            }
            let result=window.update(cx,|_,window,cx| {
                cold_capture.set(window.frame_duration_snapshot().draw_duration_histogram.value_at_quantile(1.) as f64/1e6);
                let action:UserAction=serde_json::from_value(serde_json::json!({"type":"scroll_measure","target":if files {serde_json::json!({"element_id":"conversation-artifacts"})} else if history {serde_json::json!({"element_id":"history-ledger"})} else {serde_json::json!({"x":700,"y":550})},"duration_ms":10000,"pixels_per_second":speed,"start_down":all_types&&anchor==0})).unwrap();
                driver.dispatch(action,window,cx)
            });
            if let Err(error)=result.and_then(|result|result){eprintln!("native benchmark: {error:#}");}
            let mut scroll_samples = Vec::new();
            let mut file_scroll_samples = Vec::new();
            for _ in 0..55 {
                cx.background_executor().timer(Duration::from_millis(200)).await;
                if let Ok(state) = window.update(cx, |_, _, cx| root.read(cx).benchmark_frame_state(history)) {
                    scroll_samples.push(state);
                }
                if files {
                    file_scroll_samples.push(driver.snapshot(false).elements.into_iter()
                        .filter(|e| e.visible && (e.id.starts_with("conversation-artifact-") || e.id.starts_with("content-file-")))
                        .map(|e| e.id).collect::<Vec<_>>());
                }
            }
            let _=window.update(cx,|_,window,cx| {
                let view=root.read(cx);
                *summary_capture.borrow_mut()=serde_json::json!({"records":view.benchmark_record_count(history),"coverage":view.benchmark_message_coverage(),"scroll_samples":scroll_samples,"file_scroll_samples":file_scroll_samples,"viewport":[window.viewport_size().width.as_f32(),window.viewport_size().height.as_f32()]});
            });
            if let Err(error) = finish(&frames_capture, &name_capture, anchor, speed, cold_capture.get(), &summary_capture.borrow()) {
                eprintln!("{error:#}");
                std::process::exit(1);
            }
            let _=cx.update(|cx|cx.quit());
        }).detach();
    });
    Ok(())
}

fn finish(
    frames: &Path,
    name: &str,
    anchor: usize,
    speed: f64,
    cold: f64,
    summary: &serde_json::Value,
) -> anyhow::Result<()> {
    let all_types = matches!(name, "mixed" | "files");
    let mut result: serde_json::Value = serde_json::from_slice(&std::fs::read(frames)?)?;
    result["shell"] = "desktop".into();
    result["workload"] = name.into();
    result["records"] = summary["records"].clone();
    result["coverage"] = summary["coverage"].clone();
    result["viewport"] = summary["viewport"].clone();
    result["message_size"] = 13.into();
    result["scroll_samples"] = summary["scroll_samples"].clone();
    result["file_scroll_samples"] = summary["file_scroll_samples"].clone();
    result["cold_draw_ms"] = cold.into();
    result["peak_rss_mib"] = serde_json::json!(super::peak_rss_mib());
    let (entries, bytes, accesses, misses) =
        zork_ui::components::message::text_layout_cache_stats();
    result["layout_cache"] = serde_json::json!({"entries":entries,"estimated_bytes":bytes,"accesses":accesses,"misses":misses});
    anyhow::ensure!(
        entries <= 1024 && bytes <= 32 * 1024 * 1024,
        "text layout cache exceeded its bound"
    );
    result["anchor"] = anchor.into();
    result["pixels_per_second"] = speed.into();
    std::fs::write(&frames, serde_json::to_vec_pretty(&result)?)?;
    anyhow::ensure!(
        result["present_interval_coverage"].as_f64().unwrap_or(0.) > 0.95,
        "native presentation unavailable; keep the desktop unlocked and benchmark window active: {}",
        frames.display()
    );
    anyhow::ensure!(
        result["p95_draw_ms"].as_f64().unwrap_or(f64::INFINITY) <= 1000. / super::FPS as f64,
        "native CPU frame budget exceeded: {}",
        frames.display()
    );
    if name == "files" {
        let samples = result["file_scroll_samples"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("missing file scroll samples"))?;
        anyhow::ensure!(
            samples.windows(2).any(|pair| pair[0] != pair[1]),
            "native file list did not scroll"
        );
        anyhow::ensure!(
            samples.iter().all(|sample| sample
                .as_array()
                .is_some_and(|rows| !rows.is_empty() && rows.len() <= 18)),
            "native file list escaped virtualization"
        );
    } else {
        let samples = result["scroll_samples"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("missing native scroll samples"))?;
        anyhow::ensure!(
            samples
                .windows(2)
                .any(|pair| pair[0][1] != pair[1][1] || pair[0][2] != pair[1][2]),
            "native transcript did not scroll"
        );
    }
    if all_types {
        anyhow::ensure!(
            result["coverage"]["both_roles_measured"] == true,
            "incomplete message coverage: {}",
            frames.display()
        );
    }
    println!(
        "{name}: {} messages, p95 {:.2} ms, p99 {:.2} ms, {:.1} native FPS",
        result["records"],
        result["p95_draw_ms"].as_f64().unwrap(),
        result["p99_draw_ms"].as_f64().unwrap(),
        result["fps"].as_f64().unwrap()
    );
    Ok(())
}
