//! The actual resource views inside the existing settings/Agent layout contracts.
use gpui::{div, prelude::*, px, AppContext, HeadlessAppContext};
use serde_json::json;
use std::{sync::Arc, time::Duration};
use zork_client_core::resources::*;
use zork_gui::{
    assets::EmbeddedAssets,
    automation::{AutomationRoot, HeadlessAutomation},
    desktop::HeadlessResourcesView,
    i18n::Locale,
};

fn fixture() -> ResourcesData {
    let mut mcp = Resource::new(
        ResourceKind::Mcp,
        "mcp-1".into(),
        "团队知识库".into(),
        "ready".into(),
        "selected".into(),
    );
    mcp.description = "查询团队文档与已归档研究材料。".into();
    let mut service = Resource::new(
        ResourceKind::Service,
        "service-1".into(),
        "研究报告预览".into(),
        "stopped".into(),
        "shared".into(),
    );
    service.owner_session = Some("research-report-task".into());
    let mut items = vec![mcp, service];
    for n in 0..10_000 {
        items.push(Resource::new(
            ResourceKind::Mcp,
            format!("extra-{n}"),
            format!("归档知识库 {n}"),
            "disabled".into(),
            "selected".into(),
        ));
    }
    let mut data = ResourcesData {
        devices: vec![
            ResourceDevice {
                id: "studio".into(),
                name: "工作室".into(),
                catalog: Some(ResourceCatalog {
                    origin: "key:studio".into(),
                    items,
                    issues: vec![],
                }),
                ..Default::default()
            },
            ResourceDevice {
                id: "laptop".into(),
                name: "笔记本".into(),
                error: Some("设备未连接".into()),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let mut insert = |query, content| {
        data.inspections.insert(
            ("studio".into(), query),
            InspectionState {
                content: Some(Arc::new(content)),
                ..Default::default()
            },
        );
    };
    insert(
        Inspection::Mcp("mcp-1".into()),
        InspectionContent::Details(ResourceDetails {
            title: "团队知识库".into(),
            description: "查询团队文档与已归档研究材料。".into(),
            tools: vec![ResourceTool {
                name: "search".into(),
                description: "按关键词查找文档".into(),
                input_schema: json!({"type":"object","properties":{"query":{"type":"string"}},"required":["query"]}),
            }],
            facts: vec![
                ("status".into(), "ready".into()),
                ("protocol".into(), "stdio".into()),
            ],
            ..Default::default()
        }),
    );
    insert(
        Inspection::AgentSkills("research".into()),
        InspectionContent::Skills(AgentSkills {
            skills: vec![SkillEntry {
                id: "skill-1".into(),
                name: "写作与研究手册".into(),
                description: "整理资料、撰写报告，附带参考模板。".into(),
                path: "/skills/research/SKILL.md".into(),
                source: "/skills".into(),
                content_hash: "revision".into(),
            }],
            diagnostics: vec![],
        }),
    );
    insert(
        Inspection::Skill {
            agent: "research".into(),
            skill: "skill-1".into(),
            file: None,
        },
        InspectionContent::Details(ResourceDetails {
            title: "写作与研究手册".into(),
            description: "研究资料的使用方式".into(),
            document: Some(ResourceDocument {
                path: "SKILL.md".into(),
                text:
                    "# 写作与研究\n\n先核实来源，再撰写结论。\n\n- 保留原始链接\n- 区分事实与推测"
                        .into(),
                truncated: false,
            }),
            files: vec![ResourceFile {
                path: "references/template.md".into(),
                byte_len: 64,
            }],
            ..Default::default()
        }),
    );
    insert(
        Inspection::Skill {
            agent: "research".into(),
            skill: "skill-1".into(),
            file: Some("references/template.md".into()),
        },
        InspectionContent::Details(ResourceDetails {
            title: "写作与研究手册".into(),
            document: Some(ResourceDocument {
                path: "references/template.md".into(),
                text: "# 报告模板\n\n## 事实依据\n\n列出来源与证据。".into(),
                truncated: false,
            }),
            ..Default::default()
        }),
    );
    insert(
        Inspection::Service {
            id: "service-1".into(),
            log: None,
        },
        InspectionContent::Details(ResourceDetails {
            title: "研究报告预览".into(),
            files: vec![ResourceFile {
                path: "stderr.log".into(),
                byte_len: 48,
            }],
            facts: vec![
                ("status".into(), "failed".into()),
                ("last_error".into(), "启动失败：端口已被占用".into()),
            ],
            ..Default::default()
        }),
    );
    insert(
        Inspection::Service {
            id: "service-1".into(),
            log: Some("stderr.log".into()),
        },
        InspectionContent::Details(ResourceDetails {
            title: "研究报告预览".into(),
            document: Some(ResourceDocument {
                path: "stderr.log".into(),
                text: "Error: address already in use\nCannot bind port 3000".into(),
                truncated: false,
            }),
            ..Default::default()
        }),
    );
    data
}
fn main() -> anyhow::Result<()> {
    let output = std::env::var_os("ZORK_RESOURCE_UI_OUTPUT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../artifacts/resource-native-implementation/resources-ui")
        });
    std::fs::create_dir_all(&output)?;
    let mut evidence = vec![];
    for (width, height, locale, label) in [
        (1280., 820., Locale::ZhCn, "wide"),
        (660., 700., Locale::ZhCn, "compact"),
        (1280., 820., Locale::En, "english"),
    ] {
        for mode in ["connections", "skills", "services"] {
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
                let core = Resources::fixture(fixture());
                let view = cx.new(|cx| match mode {
                    "skills" => HeadlessResourcesView::skills(
                        core,
                        "studio".into(),
                        "research".into(),
                        locale,
                        cx,
                    ),
                    "services" => {
                        HeadlessResourcesView::services(core, "studio".into(), locale, cx)
                    }
                    _ => HeadlessResourcesView::new(core, locale, cx),
                });
                // Uses the same setting width, padding and scrolling container.
                let content = cx.new(|_| Frame(view));
                cx.new(|_| AutomationRoot::new(content))
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
            let click = |cx: &mut HeadlessAppContext, id: &str| {
                act(cx, json!({"type":"click","target":{"element_id":id}}))
            };
            let has = |id: &str| driver.snapshot(false).elements.iter().any(|e| e.id == id);
            pump(&mut cx)?;
            if mode == "connections" {
                let count = driver
                    .snapshot(true)
                    .elements
                    .iter()
                    .filter(|e| e.id.starts_with("resource-row-"))
                    .count();
                anyhow::ensure!(
                    count > 0 && count < 40,
                    "connection list is not virtualized: {count}"
                );
                let mut samples = vec![];
                for _ in 0..20 {
                    let start = std::time::Instant::now();
                    cx.update_window(window.into(), |_, w, cx| {
                        w.simulate_next_frame(cx);
                        w.draw(cx).clear(cx)
                    })?;
                    samples.push(start.elapsed().as_secs_f64() * 1000.);
                }
                samples.sort_by(f64::total_cmp);
                anyhow::ensure!(
                    samples[18] < 50.,
                    "resource redraw exceeded budget: {}",
                    samples[18]
                );
                evidence.push(json!({"viewport":label,"connections":10001,"visible_rows":count,"p95_ms":samples[18]}));
                click(&mut cx, "resource-row-0")?;
                anyhow::ensure!(has("resource-tool-search"), "MCP tools missing");
                click(&mut cx, "resource-tool-search")?;
                cx.capture_screenshot(window.into())?
                    .save(output.join(format!("{label}-mcp-parameters.png")))?;
                click(&mut cx, "resource-back")?;
                click(&mut cx, "resource-more")?;
                cx.capture_screenshot(window.into())?
                    .save(output.join(format!("{label}-mcp-details.png")))?;
                click(&mut cx, "resource-detail-modal-close")?;
                anyhow::ensure!(!has("resource-detail-modal-close"), "detail did not close");
                let row = driver
                    .snapshot(false)
                    .elements
                    .into_iter()
                    .find(|e| e.id == "resource-row-0")
                    .unwrap();
                let center = row.bounds.center();
                act(
                    &mut cx,
                    json!({"type":"scroll","target":{"x":center.x,"y":center.y},"delta_y":-5000.}),
                )?;
                anyhow::ensure!(!has("resource-row-0"), "virtual list did not scroll");
            } else if mode == "skills" {
                click(&mut cx, "agent-skill-skill-1")?;
                anyhow::ensure!(
                    !has("resource-detail-modal-close"),
                    "Skill inspection created a nested modal"
                );
                cx.capture_screenshot(window.into())?
                    .save(output.join(format!("{label}-skill-body.png")))?;
                click(&mut cx, "resource-file-references/template.md")?;
                cx.capture_screenshot(window.into())?
                    .save(output.join(format!("{label}-skill-file.png")))?;
                click(&mut cx, "resource-back")?;
                click(&mut cx, "resource-back")?;
                anyhow::ensure!(
                    has("agent-skill-skill-1"),
                    "Skill back did not restore the catalog"
                );
            } else {
                click(&mut cx, "resource-row-0")?;
                click(&mut cx, "resource-file-stderr.log")?;
                cx.capture_screenshot(window.into())?
                    .save(output.join(format!("{label}-service-log.png")))?;
                act(&mut cx, json!({"type":"key","keystroke":"escape"}))?;
                anyhow::ensure!(
                    !has("resource-detail-modal-close"),
                    "Escape did not close service details"
                );
            }
        }
    }
    std::fs::write(
        output.join("performance.json"),
        serde_json::to_vec_pretty(&evidence)?,
    )?;
    pages_in_conversation(&output)?;
    println!("PASS: existing resource views, tool parameters, Skill body/files, service logs, close/Escape, locales and 10k connection virtualization");
    Ok(())
}
fn pages_in_conversation(output: &std::path::Path) -> anyhow::Result<()> {
    use zork_client_core::pages::{
        Application, ApplicationEntry, ConversationPage, PageCatalog, PageLink,
    };
    use zork_gui::views::RootView;
    std::env::set_var("ZORK_SCROLL_ALL_MESSAGES", "1");
    std::env::set_var("ZORK_BENCH_MESSAGE_COUNT", "256");
    for (width, height, label) in [
        (1280., 820., "wide"),
        (900., 700., "compact"),
        (1280., 820., "english"),
    ] {
        std::env::set_var(
            "ZORK_GUI_LOCALE",
            if label == "english" { "en" } else { "zh-CN" },
        );
        let temp = tempfile::tempdir()?;
        let store = Arc::new(zork_client_core::store::ClientStore::open(temp.path())?);
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
        let mut root = None;
        let page = PageLink {
            id: "report".into(),
            title: "项目研究报告".into(),
            url: "https://example.test/report".into(),
            description: "已交付的报告页面".into(),
        };
        let app = PageLink {
            id: "dashboard".into(),
            title: "团队看板".into(),
            url: "https://example.test/dashboard".into(),
            description: "跨任务持续使用".into(),
        };
        let window = cx.open_window(gpui::size(px(width), px(height)), |_, cx| {
            let view = cx.new(|cx| {
                let mut view = RootView::render_benchmark_fixture(false, store, cx);
                view.benchmark_page_catalog(
                    PageCatalog {
                        references: vec![
                            ConversationPage {
                                id: "delivered".into(),
                                session_id: "render-fixture".into(),
                                message_id: "report-message".into(),
                                page: page.clone(),
                                source_session_id: Some("worker".into()),
                                created_at: "2026-09-11T00:00:00Z".into(),
                            },
                            ConversationPage {
                                id: "unrelated".into(),
                                session_id: "another-conversation".into(),
                                message_id: "other-message".into(),
                                page: app.clone(),
                                source_session_id: None,
                                created_at: "2026-09-11T00:00:00Z".into(),
                            },
                        ],
                        applications: vec![Application {
                            page: app.clone(),
                            owner_session_id: "other-task".into(),
                            created_at: "2026-09-11T00:00:00Z".into(),
                        }],
                        ..Default::default()
                    },
                    cx,
                );
                view.set_applications(
                    Arc::new(vec![ApplicationEntry {
                        page: app.clone(),
                        device_id: "mini1".into(),
                        device_name: "mini1".into(),
                        offline: false,
                    }]),
                    cx,
                );
                view
            });
            root = Some(view.clone());
            cx.new(|_| AutomationRoot::new(view))
        })?;
        // Compare settled layout; opening and resizing can schedule panel motion.
        cx.update(|cx| cx.set_reduce_motion(true));
        let pump = |cx: &mut HeadlessAppContext| -> anyhow::Result<()> {
            for _ in 0..24 {
                cx.advance_clock(Duration::from_millis(16));
                cx.run_until_parked();
                cx.update_window(window.into(), |_, w, cx| {
                    w.simulate_next_frame(cx);
                    w.draw(cx).clear(cx)
                })?;
            }
            Ok(())
        };
        let act = |cx: &mut HeadlessAppContext, action: serde_json::Value| -> anyhow::Result<()> {
            cx.update_window(window.into(), |_, w, cx| {
                driver.dispatch(serde_json::from_value(action).unwrap(), w, cx)
            })??;
            pump(cx)
        };
        let click = |cx: &mut HeadlessAppContext, id: &str| {
            act(cx, json!({"type":"click","target":{"element_id":id}}))
        };
        let has = |id: &str| {
            driver
                .snapshot(false)
                .elements
                .iter()
                .any(|e| e.id == id && e.visible)
        };
        pump(&mut cx)?;
        click(&mut cx, "conversation-files-button")?;
        anyhow::ensure!(
            has("conversation-content-pages-preview") && has("conversation-content-files-preview"),
            "content groups are missing"
        );
        anyhow::ensure!(
            has("conversation-pages-all") && has("conversation-files-all"),
            "View all entry is missing"
        );
        anyhow::ensure!(
            driver
                .snapshot(false)
                .elements
                .iter()
                .filter(|e| e.visible && e.id.starts_with("conversation-artifact-"))
                .count()
                <= 3,
            "file preview contains the full catalog"
        );
        anyhow::ensure!(
            has("conversation-page-delivered"),
            "handed page missing from conversation"
        );
        anyhow::ensure!(
            !has("conversation-page-unrelated"),
            "another conversation's page leaked into the menu"
        );
        anyhow::ensure!(
            !has("application-dashboard"),
            "published applications leaked into conversation content"
        );
        let elements = driver.snapshot(false).elements;
        let panel = elements
            .iter()
            .find(|e| e.id == "conversation-files-flyout-material-content")
            .unwrap();
        let close = elements
            .iter()
            .find(|e| e.id == "conversation-files-close")
            .unwrap();
        std::fs::write(
            output.join(format!("{label}-content-elements.json")),
            serde_json::to_vec_pretty(&elements)?,
        )?;
        anyhow::ensure!(
            (close.bounds.width - 28.).abs() < 0.5,
            "close button geometry changed"
        );
        anyhow::ensure!(
            (panel.bounds.x + panel.bounds.width - close.bounds.x - close.bounds.width - 15.).abs()
                < 1.1,
            "close button right inset changed"
        );
        cx.capture_screenshot(window.into())?
            .save(output.join(format!("{label}-files-and-pages.png")))?;
        click(&mut cx, "conversation-files-close")?;
        if !has("browser-new-tab") {
            click(&mut cx, "conversation-browser")?;
        }
        click(&mut cx, "browser-new-tab")?;
        anyhow::ensure!(
            has("application-dashboard"),
            "global application missing on browser start page"
        );
        anyhow::ensure!(
            !has("application-report"),
            "delivery implicitly published an application"
        );
        cx.capture_screenshot(window.into())?
            .save(output.join(format!("{label}-applications.png")))?;
        let root = root.unwrap();
        let geometry =
            |cx: &HeadlessAppContext| root.read_with(cx, |v, _| v.benchmark_file_geometry());
        let rect = |id: &str| -> anyhow::Result<zork_gui::automation::protocol::Rect> {
            driver
                .snapshot(false)
                .elements
                .iter()
                .find(|e| e.id == id)
                .map(|e| e.bounds)
                .ok_or_else(|| anyhow::anyhow!("missing {id}"))
        };
        for state in ["split", "resized", "closed"] {
            if state == "resized" {
                let grip = rect("page-resize")?.center();
                act(
                    &mut cx,
                    json!({"type":"drag","from":{"x":grip.x,"y":grip.y},
                    "to":{"x":grip.x - 100.,"y":grip.y},"steps":24}),
                )?;
            } else if state == "closed" {
                click(&mut cx, "conversation-browser")?;
            }
            let before = geometry(&cx);
            let transcript_before = rect("conversation-transcript")?;
            let composer_before = rect("composer-surface")?;
            click(&mut cx, "conversation-files-button")?;
            let after = geometry(&cx);
            let panel = rect("conversation-files-flyout-material-content")?;
            let button = rect("conversation-files-button")?;
            let transcript = rect("conversation-transcript")?;
            let composer = rect("composer-surface")?;
            std::fs::write(
                output.join(format!("{label}-{state}-content-geometry.json")),
                serde_json::to_vec_pretty(&json!({"before":before,"after":after,"panel":panel,
                    "button":button,"transcript_before":transcript_before,"transcript":transcript,
                    "composer_before":composer_before,"composer":composer}))?,
            )?;
            cx.capture_screenshot(window.into())?
                .save(output.join(format!("{label}-{state}-files-and-pages.png")))?;
            anyhow::ensure!(
                before["composer_width"] == after["composer_width"],
                "opening content menu changed chat width ({label}/{state}): {before} -> {after}"
            );
            anyhow::ensure!(
                transcript == transcript_before && composer == composer_before,
                "opening content menu moved chat layout ({label}/{state})"
            );
            anyhow::ensure!(
                panel.x >= transcript.x + 8.
                    && panel.x + panel.width <= transcript.x + transcript.width - 8.,
                "content menu escaped the chat column: {panel:?} outside {transcript:?}"
            );
            anyhow::ensure!(
                (panel.x + panel.width - button.x - button.width).abs() < 1.,
                "content menu lost its button anchor"
            );
            match state {
                "split" => act(&mut cx, json!({"type":"key","keystroke":"escape"}))?,
                "resized" => click(&mut cx, "conversation-files-button")?,
                _ => click(&mut cx, "conversation-files-close")?,
            }
            anyhow::ensure!(
                !has("conversation-files-panel"),
                "menu did not close: {state}"
            );
            anyhow::ensure!(
                geometry(&cx)["composer_width"] == before["composer_width"],
                "closing content menu changed chat width"
            );
        }
        click(&mut cx, "conversation-browser")?;
        click(&mut cx, "conversation-files-button")?;
        click(&mut cx, "browser-address")?;
        anyhow::ensure!(
            !has("conversation-files-panel"),
            "click outside did not close menu"
        );
        let before_expand = geometry(&cx)["composer_width"].clone();
        click(&mut cx, "browser-expand")?;
        anyhow::ensure!(
            !has("conversation-transcript"),
            "expanded browser exposed chat"
        );
        click(&mut cx, "browser-expand")?;
        anyhow::ensure!(
            geometry(&cx)["composer_width"] == before_expand,
            "restoring the split lost the chat width"
        );
        let file_rows = || {
            driver
                .snapshot(false)
                .elements
                .into_iter()
                .filter(|e| e.visible && e.id.starts_with("content-file-"))
                .collect::<Vec<_>>()
        };
        let clear_search = |cx: &mut HeadlessAppContext, id: &str| -> anyhow::Result<()> {
            click(cx, id)?;
            act(cx, json!({"type":"key","keystroke":"cmd-a"}))?;
            act(cx, json!({"type":"key","keystroke":"backspace"}))
        };
        click(&mut cx, "conversation-files-button")?;
        click(&mut cx, "conversation-pages-all")?;
        std::fs::write(
            output.join(format!("{label}-page-tab-elements.json")),
            serde_json::to_vec_pretty(&driver.snapshot(true))?,
        )?;
        cx.capture_screenshot(window.into())?
            .save(output.join(format!("{label}-page-tab-open.png")))?;
        anyhow::ensure!(
            !has("conversation-files-panel") && has("content-page-delivered"),
            "View all pages did not open the native tab"
        );
        anyhow::ensure!(
            file_rows().is_empty() && !has("content-page-unrelated"),
            "page tab mixed content groups or conversations"
        );
        act(
            &mut cx,
            json!({"type":"type_text","target":{"element_id":"content-search-pages"},"text":"not-in-this-chat"}),
        )?;
        anyhow::ensure!(!has("content-page-delivered"), "page search did not filter");
        clear_search(&mut cx, "content-search-pages")?;
        anyhow::ensure!(
            has("content-page-delivered"),
            "clearing page search did not restore content"
        );
        click(&mut cx, "conversation-files-button")?;
        click(&mut cx, "conversation-files-all")?;
        let first = file_rows();
        anyhow::ensure!(
            first.len() > 3 && first.len() <= 18 && !has("content-page-delivered"),
            "file tab did not show its full virtual list"
        );
        let selected = first[0].clone();
        act(
            &mut cx,
            json!({"type":"type_text","target":{"element_id":"content-search-files"},"text":selected.label}),
        )?;
        anyhow::ensure!(
            file_rows().len() == 1 && has(&selected.id),
            "file name search failed"
        );
        click(&mut cx, "page-tab-conversation-content-pages")?;
        click(&mut cx, "page-tab-conversation-content-files")?;
        anyhow::ensure!(
            file_rows().len() == 1 && has(&selected.id),
            "switching tabs lost the file search"
        );
        click(&mut cx, "conversation-files-button")?;
        click(&mut cx, "conversation-files-all")?;
        anyhow::ensure!(
            file_rows().len() == 1 && has(&selected.id),
            "View all replaced the existing tab state"
        );
        anyhow::ensure!(
            driver
                .snapshot(false)
                .elements
                .iter()
                .filter(|e| e.id == "page-tab-conversation-content-files")
                .count()
                == 1,
            "View all duplicated the file tab"
        );
        clear_search(&mut cx, "content-search-files")?;
        act(
            &mut cx,
            json!({"type":"scroll","target":{"element_id":"conversation-artifacts"},"delta_y":-500.}),
        )?;
        let scrolled = file_rows().iter().map(|e| e.id.clone()).collect::<Vec<_>>();
        anyhow::ensure!(
            scrolled != first.iter().map(|e| e.id.clone()).collect::<Vec<_>>(),
            "full file list did not scroll"
        );
        click(&mut cx, "page-tab-conversation-content-pages")?;
        click(&mut cx, "page-tab-conversation-content-files")?;
        anyhow::ensure!(
            file_rows().iter().map(|e| e.id.clone()).collect::<Vec<_>>() == scrolled,
            "switching tabs lost file scroll position"
        );
        cx.capture_screenshot(window.into())?
            .save(output.join(format!("{label}-file-management.png")))?;
        click(&mut cx, &scrolled[0])?;
        anyhow::ensure!(
            has("drive-close-preview"),
            "file tab row did not open preview"
        );
        click(&mut cx, "drive-close-preview")?;
        anyhow::ensure!(
            file_rows().iter().map(|e| e.id.clone()).collect::<Vec<_>>() == scrolled,
            "closing preview lost file scroll position"
        );
        click(&mut cx, "conversation-browser")?;
        click(&mut cx, "conversation-browser")?;
        anyhow::ensure!(
            file_rows().iter().map(|e| e.id.clone()).collect::<Vec<_>>() == scrolled,
            "hiding the panel lost file scroll position"
        );
        click(&mut cx, "page-close-conversation-content-files")?;
        anyhow::ensure!(
            has("content-page-delivered") && file_rows().is_empty(),
            "closing file tab did not return to pages"
        );
        cx.capture_screenshot(window.into())?
            .save(output.join(format!("{label}-page-management.png")))?;
        click(&mut cx, "page-close-conversation-content-pages")?;
        anyhow::ensure!(!has("content-page-delivered"), "page tab did not close");
        std::fs::write(
            output.join(format!("{label}-content-tabs.json")),
            serde_json::to_vec_pretty(&json!({
                "groups":["pages","files"],"preview_limit":3,"full_list_visible_rows":first.len(),
                "search":"passed","tab_reuse":"passed","switch_scroll":"passed","preview_scroll":"passed","hide_restore":"passed"
            }))?,
        )?;
        drop(root);
    }
    Ok(())
}
struct Frame(gpui::Entity<HeadlessResourcesView>);
impl gpui::Render for Frame {
    fn render(
        &mut self,
        _: &mut gpui::Window,
        _: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        div()
            .id("settings-test-scroll")
            .size_full()
            .overflow_y_scroll()
            .bg(gpui::rgb(zork_ui::design::CUE_UI.palette.canvas))
            .child(zork_ui::controls::settings_content(
                div().child(self.0.clone()),
            ))
    }
}
