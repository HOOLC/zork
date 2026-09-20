//! Native component gallery and offscreen story exporter; never packaged in Zork.app.
use gpui::{prelude::*, px, size, AppContext, Bounds, Context, Entity, HeadlessAppContext, Window};
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};
use zork_gui::{
    assets::EmbeddedAssets,
    automation::{protocol::UserAction, AutomationRoot, HeadlessAutomation},
    desktop::stories::{self, Story, StoryHost},
};

fn settle(
    cx: &mut HeadlessAppContext,
    window: gpui::AnyWindowHandle,
) -> anyhow::Result<image::RgbaImage> {
    let start = Instant::now();
    let mut previous = None;
    let mut stable = 0;
    loop {
        cx.run_until_parked();
        cx.update_window(window, |_, w, cx| w.draw(cx).clear(cx))?;
        let pixels = cx.capture_screenshot(window)?;
        if previous.as_ref() == Some(pixels.as_raw()) {
            stable += 1;
        } else {
            stable = 0;
        }
        previous = Some(pixels.as_raw().clone());
        if stable >= 3 && start.elapsed() >= Duration::from_millis(180) {
            return Ok(pixels);
        }
        anyhow::ensure!(
            start.elapsed() < Duration::from_secs(5),
            "story pixels did not settle"
        );
        std::thread::sleep(Duration::from_millis(12));
    }
}
fn export_story(story: &Story, output: &Path) -> anyhow::Result<Value> {
    let mut cx = HeadlessAppContext::with_platform(
        gpui_platform::current_platform(true).text_system(),
        Arc::new(EmbeddedAssets),
        gpui_platform::current_headless_renderer,
    );
    let driver = cx.update(|cx| {
        zork_gui::assets::init_fonts(cx);
        zork_gui::components::init(cx);
        // Reference stills show the final state; browser motion checks sample transitions.
        cx.set_reduce_motion(true);
        HeadlessAutomation::install(cx)
    });
    let mut root = None;
    let window = cx.open_window(size(px(story.width), px(story.height)), |_, cx| {
        let host = cx.new(|cx| StoryHost::new(story.clone(), cx));
        root = Some(host.clone());
        cx.new(|_| AutomationRoot::new(host))
    })?;
    let root = root.unwrap();
    cx.update_window(window.into(), |_, w, cx| w.draw(cx).clear(cx))?;
    cx.run_until_parked();
    for action in &story.actions {
        let action: UserAction = serde_json::from_value(action.clone())?;
        cx.update_window(window.into(), |_, w, cx| driver.dispatch(action, w, cx))??;
        cx.run_until_parked();
        cx.update_window(window.into(), |_, w, cx| w.draw(cx).clear(cx))?;
    }
    if story.actions.iter().any(|a| a["type"] == "move") {
        for _ in 0..40 {
            cx.advance_clock(Duration::from_millis(12));
            cx.run_until_parked();
            cx.update_window(window.into(), |_, w, cx| w.draw(cx).clear(cx))?;
        }
    }
    let pixels = settle(&mut cx, window.into())?;
    let snapshot = driver.snapshot(false);
    let target = if story.target == "story-component"
        && snapshot.elements.iter().any(|e| e.id == "story-sample")
    {
        "story-sample"
    } else {
        story.target.as_str()
    };
    let target = snapshot
        .elements
        .iter()
        .find(|e| e.id == target)
        .ok_or_else(|| anyhow::anyhow!("{}: missing story target {target}", story.id))?;
    anyhow::ensure!(target.visible, "story target is hidden: {}", story.id);
    let scale = snapshot.scale_factor;
    let mut b = target.visible_bounds;
    {
        for menu in snapshot.elements.iter().filter(|e| {
            e.visible && (e.id.ends_with("-menu") || e.id.starts_with("detail-tooltip-"))
        }) {
            let m = menu.visible_bounds;
            let right = (b.x + b.width).max(m.x + m.width);
            let bottom = (b.y + b.height).max(m.y + m.height);
            b.x = b.x.min(m.x);
            b.y = b.y.min(m.y);
            b.width = right - b.x;
            b.height = bottom - b.y;
        }
    }
    let x = (b.x * scale).round().max(0.) as u32;
    let y = (b.y * scale).round().max(0.) as u32;
    let w = (b.width * scale).round().max(1.) as u32;
    let h = (b.height * scale).round().max(1.) as u32;
    let folder = output.join("native");
    std::fs::create_dir_all(&folder)?;
    pixels.save(folder.join(format!("{}-window.png", story.id)))?;
    image::imageops::crop_imm(
        &pixels,
        x,
        y,
        w.min(pixels.width() - x),
        h.min(pixels.height() - y),
    )
    .to_image()
    .save(folder.join(format!("{}.png", story.id)))?;
    std::fs::write(
        folder.join(format!("{}.json", story.id)),
        serde_json::to_vec_pretty(&snapshot)?,
    )?;
    let mut data = serde_json::to_value(story)?;
    data["native"] = json!({"image":format!("native/{}.png",story.id),"window":format!("native/{}-window.png",story.id),"geometry":format!("native/{}.json",story.id),"bounds":b,"scale":scale,"renderer":"production GPUI + offscreen Metal"});
    drop(root);
    Ok(data)
}

#[path = "storybook/workbench.rs"]
mod workbench;
use workbench::Gallery;

fn main() -> anyhow::Result<()> {
    std::env::set_var("SEED", "0");
    std::env::set_var("TZ", "UTC");
    let temporary = tempfile::tempdir()?;
    std::env::set_var(
        "ZORK_GUI_PREFERENCES_PATH",
        temporary.path().join("preferences.json"),
    );
    let args: Vec<_> = std::env::args().skip(1).collect();
    let value = |flag: &str| {
        args.iter()
            .position(|s| s == flag)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let mut catalog = stories::catalog();
    if let Some(family) = value("--family") {
        catalog.retain(|s| s.family == family);
        anyhow::ensure!(!catalog.is_empty(), "unknown story family {family}");
    }
    if let Some(filter) = value("--story") {
        catalog.retain(|s| s.id == filter);
        anyhow::ensure!(!catalog.is_empty(), "unknown story {filter}");
    }
    for (flag, width) in [("--width", true), ("--height", false)] {
        if let Some(value) = value(flag) {
            let value: f32 = value.parse()?;
            anyhow::ensure!(value.is_finite() && value >= 100., "invalid story size");
            for story in &mut catalog {
                if width {
                    story.width = value;
                } else {
                    story.height = value;
                }
            }
        }
    }
    if let Some(path) = value("--actions") {
        let actions: Vec<Value> = serde_json::from_slice(&std::fs::read(path)?)?;
        for story in &mut catalog {
            story.actions.extend(actions.clone());
        }
    }
    if let Some(target) = value("--hover") {
        for story in &mut catalog {
            story
                .actions
                .push(json!({"type":"move","target":{"element_id":target.clone()}}));
        }
    }
    if args.iter().any(|a| a == "--list") {
        println!("{}", serde_json::to_string_pretty(&catalog)?);
        return Ok(());
    }
    if let Some(output) = value("--export") {
        let output = PathBuf::from(output);
        std::fs::create_dir_all(&output)?;
        // A failed story must not leave a gallery containing images from two
        // builds. Render the complete catalog before publishing its evidence.
        let staging = tempfile::tempdir_in(&output)?;
        let mut rendered = vec![];
        let mut failures = vec![];
        for story in &catalog {
            match export_story(story, staging.path()) {
                Ok(story) => {
                    println!("{}", story["id"]);
                    rendered.push(story);
                }
                Err(error) => {
                    eprintln!("{}: {error:#}", story.id);
                    failures.push(format!("{}: {error:#}", story.id));
                }
            }
        }
        anyhow::ensure!(
            failures.is_empty(),
            "gallery was not replaced; {} stories failed:\n{}",
            failures.len(),
            failures.join("\n")
        );
        let generation = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_millis();
        for story in &mut rendered {
            story["native"]["generation"] = json!(generation);
        }
        std::fs::write(
            staging.path().join("manifest.json"),
            serde_json::to_vec_pretty(
                &json!({"version":1,"generation":generation,"stories":rendered,"isolated":true,"network":"none","reference_status":"provided by the design capture step"}),
            )?,
        )?;
        let native = output.join("native");
        let previous = staging.path().join("previous-native");
        if native.exists() {
            std::fs::rename(&native, &previous)?;
        }
        if let Err(error) = std::fs::rename(staging.path().join("native"), &native) {
            if previous.exists() {
                std::fs::rename(previous, native)?;
            }
            return Err(error.into());
        }
        std::fs::rename(
            staging.path().join("manifest.json"),
            output.join("manifest.json"),
        )?;
        return Ok(());
    }
    let initial = value("--start-story")
        .map(|id| {
            catalog
                .iter()
                .position(|s| s.id == id)
                .ok_or_else(|| anyhow::anyhow!("unknown story {id}"))
        })
        .transpose()?
        .unwrap_or_else(|| {
            catalog
                .iter()
                .position(|s| s.family == "button")
                .unwrap_or(0)
        });
    let automation = args
        .iter()
        .any(|a| a == "--dev")
        .then(|| {
            zork_gui::automation::DevAutomation::bind(
                value("--dev-port")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0),
                value("--dev-token"),
            )
        })
        .transpose()?;
    if let Some(automation) = &automation {
        println!(
            "{}",
            json!({"storybook_automation":automation.address().to_string(),"token":automation.token()})
        );
    }
    let fixed_size = value("--width").is_some() || value("--height").is_some();
    gpui_platform::application()
        .with_assets(EmbeddedAssets)
        .run(move |cx| {
            zork_gui::assets::init_fonts(cx);
            zork_gui::components::init(cx);
            let driver = if let Some(automation) = &automation {
                automation.install(cx);
                automation.in_process_driver()
            } else {
                HeadlessAutomation::install(cx)
            };
            let window = cx
                .open_window(
                    gpui::WindowOptions {
                        window_bounds: Some(gpui::WindowBounds::Windowed(Bounds::centered(
                            None,
                            size(px(1320.), px(860.)),
                            cx,
                        ))),
                        titlebar: Some(zork_gui::window_chrome::native_titlebar_options()),
                        ..Default::default()
                    },
                    |w, cx| {
                        w.set_window_title("Zork Storybook");
                        w.on_window_should_close(cx, |_, cx| {
                            cx.quit();
                            true
                        });
                        let gallery =
                            cx.new(|cx| Gallery::new(catalog, initial, driver, fixed_size, cx));
                        cx.new(|_| AutomationRoot::new(gallery))
                    },
                )
                .expect("open component gallery");
            if let Some(automation) = automation {
                automation.attach(window.into(), cx);
            }
            cx.activate(true);
        });
    Ok(())
}
