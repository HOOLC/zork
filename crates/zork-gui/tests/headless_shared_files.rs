//! Native rendering and bounded row construction. Connected business behavior
//! is covered separately by client-core and the real Mesh process fixture.
use gpui::{div, prelude::*, px, AppContext, HeadlessAppContext};
use serde_json::json;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use zork_client_core::{shared_files::*, store::ClientStore};
use zork_gui::{
    assets::EmbeddedAssets,
    automation::{AutomationRoot, HeadlessAutomation},
    desktop::HeadlessSharedFilesView,
    i18n::Locale,
};

fn fixture(preview: bool, grid: bool) -> SharedFilesData {
    let sources = vec![
        Source {
            id: "studio".into(),
            name: "工作室".into(),
            online: Some(true),
            cached: false,
        },
        Source {
            id: "laptop".into(),
            name: "笔记本".into(),
            online: Some(true),
            cached: false,
        },
    ];
    let text = "# 项目资料\n\n在一个入口浏览所有 Station 自动发布的文件。\n\n共享文件保持只读，每个副本保留自己的来源和内容版本。\n\n## 下一步\n\n- 检查资料\n- 保留原始版本\n";
    let root = zork_client_core::api::content_root(text.as_bytes());
    let versions = vec![Version {
        root: root.clone(),
        size: text.len() as u64,
        modified_ns: 1_780_000_000_000_000_000,
        sources: sources.clone(),
        can_read: true,
    }];
    let entries = (0..100_000)
        .map(|i| Entry {
            target: None,
            id: format!("file:file-{i:06}.txt"),
            path: format!("file-{i:06}.txt"),
            name: format!("项目资料 {i:06}.txt"),
            kind: EntryKind::File,
            sources: sources.clone(),
            versions: versions.clone(),
        })
        .collect();
    SharedFilesData {
        active: true,
        devices: sources
            .iter()
            .map(|s| Device {
                id: s.id.clone(),
                name: s.name.clone(),
                online: Some(true),
                ..Default::default()
            })
            .collect(),
        spaces: vec![Space {
            id: "01ARZ3NDEKTSV4RRFFQ69G5FAV".into(),
            name: "项目资料".into(),
            sources,
        }],
        location: Some(Location {
            space: "01ARZ3NDEKTSV4RRFFQ69G5FAV".into(),
            path: String::new(),
        }),
        location_name: "项目资料".into(),
        entries: Arc::new(entries),
        layout: if grid { Layout::Grid } else { Layout::List },
        preview: preview.then(|| Preview {
            path: "file-000000.txt".into(),
            name: "项目说明.md".into(),
            versions,
            selected: root,
            loading: false,
            error: None,
            text: Some(text.into()),
            truncated: false,
            mime: "text/plain".into(),
            cached: false,
            can_save: true,
            bytes: Some(Arc::new(text.as_bytes().to_vec())),
        }),
        ..Default::default()
    }
}
fn main() -> anyhow::Result<()> {
    let output = std::env::var_os("ZORK_SHARED_FILES_UI_OUTPUT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../artifacts/unified-file-tree/native-ui")
        });
    std::fs::create_dir_all(&output)?;
    let mut evidence = vec![];
    for (width, height, locale, label) in [
        (1040., 780., Locale::ZhCn, "wide"),
        (420., 740., Locale::ZhCn, "compact"),
        (1040., 780., Locale::En, "english"),
    ] {
        for mode in ["list", "grid", "preview"] {
            let store_root = tempfile::tempdir()?;
            let store = Arc::new(ClientStore::open(store_root.path())?);
            let source =
                SharedFiles::rendering_fixture(store, fixture(mode == "preview", mode == "grid"));
            let observed_source = source.clone();
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
            let window = cx.open_window(gpui::size(px(width), px(height)), |_, cx| {
                let view =
                    cx.new(|cx| HeadlessSharedFilesView::rendering_fixture(source, locale, cx));
                let frame = cx.new(|_| Frame(view));
                cx.new(|_| AutomationRoot::new(frame))
            })?;
            let pump = |cx: &mut HeadlessAppContext| -> anyhow::Result<()> {
                for _ in 0..4 {
                    cx.advance_clock(Duration::from_millis(16));
                    cx.run_until_parked();
                    cx.update_window(window.into(), |_, w, cx| {
                        w.simulate_next_frame(cx);
                        w.draw(cx).clear(cx)
                    })?;
                }
                Ok(())
            };
            let act =
                |cx: &mut HeadlessAppContext, action: serde_json::Value| -> anyhow::Result<()> {
                    cx.update_window(window.into(), |_, w, cx| {
                        driver.dispatch(serde_json::from_value(action).unwrap(), w, cx)
                    })??;
                    pump(cx)
                };
            pump(&mut cx)?;
            cx.capture_screenshot(window.into())?
                .save(output.join(format!("{label}-{mode}.png")))?;
            let count = driver
                .snapshot(true)
                .elements
                .iter()
                .filter(|e| e.id.starts_with("shared-row-"))
                .count();
            if mode != "preview" || label != "compact" {
                anyhow::ensure!(count > 0 && count < 48, "100k files built {count} rows");
            }
            if mode == "list" {
                let mut samples = vec![];
                for _ in 0..40 {
                    cx.update_window(window.into(), |_, w, cx| {
                        driver.dispatch(
                            serde_json::from_value(
                                json!({"type":"scroll","target":{"x":100,"y":300},"delta_y":-68.}),
                            )
                            .unwrap(),
                            w,
                            cx,
                        )
                    })??;
                    cx.advance_clock(Duration::from_millis(16));
                    cx.run_until_parked();
                    let started = Instant::now();
                    cx.update_window(window.into(), |_, w, cx| {
                        w.simulate_next_frame(cx);
                        w.draw(cx).clear(cx)
                    })?;
                    samples.push(started.elapsed().as_secs_f64() * 1000.);
                }
                samples.sort_by(f64::total_cmp);
                anyhow::ensure!(
                    samples[37] < 50.,
                    "file scrolling exceeded native redraw budget: {} ms",
                    samples[37]
                );
                act(
                    &mut cx,
                    json!({"type":"scroll","target":{"x":100,"y":300},"delta_y":-3_000_000.}),
                )?;
                let middle = driver
                    .snapshot(false)
                    .elements
                    .iter()
                    .filter(|e| e.id.starts_with("shared-row-"))
                    .count();
                anyhow::ensure!(middle > 0 && middle < 48);
                act(
                    &mut cx,
                    json!({"type":"scroll","target":{"x":100,"y":300},"delta_y":-10_000_000.}),
                )?;
                anyhow::ensure!(
                    driver
                        .snapshot(false)
                        .elements
                        .iter()
                        .any(|e| e.id == "shared-row-file:file-099999.txt"),
                    "could not reach the last row"
                );
                evidence.push(json!({"viewport":label,"items":100000,"visible_rows":count,"middle_rows":middle,"cpu_frame_p95_ms":samples[37],"cpu_frame_p99_ms":samples[39]}));
                act(
                    &mut cx,
                    json!({"type":"click","target":{"element_id":"shared-more"}}),
                )?;
                act(
                    &mut cx,
                    json!({"type":"click","target":{"element_id":"shared-menu-0-shared-sources"}}),
                )?;
                cx.capture_screenshot(window.into())?
                    .save(output.join(format!("{label}-sources.png")))?;
                anyhow::ensure!(
                    driver
                        .snapshot(false)
                        .elements
                        .iter()
                        .any(|entry| entry.label == locale.text("shared_all_sources")),
                    "source menu did not open"
                );
                anyhow::ensure!(
                    observed_source.snapshot().entries.len() == 100000
                        && observed_source.snapshot().error.is_none(),
                    "menu click reached the underlying file view"
                );
                act(&mut cx, json!({"type":"key","keystroke":"escape"}))?;
            }
        }
    }
    std::fs::write(
        output.join("report.json"),
        serde_json::to_vec_pretty(&evidence)?,
    )?;
    println!(
        "PASS: native shared-file layouts and 100,000-row scrolling: {}",
        output.display()
    );
    Ok(())
}
struct Frame(gpui::Entity<HeadlessSharedFilesView>);
impl gpui::Render for Frame {
    fn render(&mut self, _: &mut gpui::Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .font_family("Inter Variable")
            .text_size(px(13.))
            .child(self.0.clone())
    }
}
