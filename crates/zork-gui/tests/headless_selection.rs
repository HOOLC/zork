//! Real GPUI mouse input over the production desktop Markdown transcript.
//! This target has no test harness so the headless text/renderer run on the main
//! thread, with fixed data and virtual time. It never opens a system window.
use anyhow::{anyhow, ensure, Context as _, Result};
use gpui::{
    point, px, size, AnyWindowHandle, AppContext, Entity, HeadlessAppContext, Modifiers,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PlatformInput,
};
use std::{path::PathBuf, sync::Arc, time::Duration};
use zork_gui::{
    api::{MessageMetadata, Role},
    assets::EmbeddedAssets,
    automation::{
        protocol::{ElementInfo, UserAction},
        AutomationRoot, HeadlessAutomation,
    },
    components::message::MessageDocument,
    transcript::TranscriptLine,
    views::RootView,
};

const MARKDOWN: &str = "前言段落甲乙。\n\n| 左列标题 | 右列标题 |\n| --- | --- |\n| 左甲乙丙 | 右中文🐈末 |\n\n后续段落：中文🐈甲乙终。\n\n末尾段落丙丁。";
const STEP: Duration = Duration::from_millis(16);

struct Fixture {
    // Drop external entity handles before HeadlessAppContext's leak assertion.
    root: Entity<RootView>,
    cx: HeadlessAppContext,
    window: AnyWindowHandle,
    automation: HeadlessAutomation,
    _directory: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Result<Self> {
        Self::new_sized(1280., 800.)
    }

    fn new_sized(width: f32, height: f32) -> Result<Self> {
        let directory = tempfile::tempdir()?;
        let store = Arc::new(zork_gui::desktop::store::ClientStore::open(
            directory.path(),
        )?);
        let mut cx = HeadlessAppContext::with_platform(
            gpui_platform::current_platform(true).text_system(),
            Arc::new(EmbeddedAssets),
            gpui_platform::current_headless_renderer,
        );
        let automation = cx.update(|cx| {
            zork_gui::assets::init_fonts(cx);
            zork_gui::components::init(cx);
            HeadlessAutomation::install(cx)
        });
        let mut root = None;
        let window = cx.open_window(size(px(width), px(height)), |_, cx| {
            let view = cx.new(|cx| RootView::render_benchmark_fixture(false, store.clone(), cx));
            root = Some(view.clone());
            cx.new(|_| AutomationRoot::new(view))
        })?;
        let mut fixture = Self {
            cx,
            window: window.into(),
            root: root.unwrap(),
            automation,
            _directory: directory,
        };
        fixture.replace_message(MARKDOWN)?;
        Ok(fixture)
    }

    fn replace_message(&mut self, content: &str) -> Result<()> {
        self.replace_message_as(content, Role::Assistant)
    }

    fn replace_message_as(&mut self, content: &str, role: Role) -> Result<()> {
        self.root.update(&mut self.cx, |view, cx| {
            view.benchmark_replace_messages(
                vec![TranscriptLine::Message {
                    role,
                    content: content.to_owned(),
                    metadata: MessageMetadata {
                        id: Some("headless-selection-message".into()),
                        created_at: Some("2027-01-15T08:00:00Z".into()),
                        author_agent_id: Some("leader".into()),
                        author_name: Some("产品 Leader".into()),
                        author_avatar: Some("fox".into()),
                        ..Default::default()
                    },
                }],
                cx,
            );
        });
        for _ in 0..3 {
            self.frame()?;
        }
        Ok(())
    }

    fn frame(&mut self) -> Result<()> {
        self.cx.advance_clock(STEP);
        self.cx
            .update_window(self.window, |_, window, cx| { window.simulate_next_frame(cx); })?;
        self.cx.run_until_parked();
        Ok(())
    }

    fn element(&self, id: &str) -> Result<ElementInfo> {
        self.automation
            .snapshot(false)
            .elements
            .into_iter()
            .find(|element| element.id == id)
            .with_context(|| format!("missing rendered element {id}"))
    }

    fn text(&self, label: &str) -> Result<ElementInfo> {
        self.automation
            .snapshot(false)
            .elements
            .into_iter()
            .find(|element| element.id.ends_with("-selection") && element.label == label)
            .with_context(|| format!("missing selectable text {label}"))
    }

    fn action(&mut self, value: serde_json::Value) -> Result<()> {
        let action: UserAction = serde_json::from_value(value)?;
        self.cx.update_window(self.window, |_, window, cx| {
            self.automation.dispatch(action, window, cx)
        })??;
        self.frame()
    }

    fn mouse(&mut self, input: PlatformInput) -> Result<()> {
        self.cx.update_window(self.window, |_, window, cx| {
            window.dispatch_event(input, cx);
        })?;
        self.frame()
    }

    fn drag(
        &mut self,
        from: gpui::Point<gpui::Pixels>,
        to: gpui::Point<gpui::Pixels>,
    ) -> Result<()> {
        self.mouse(PlatformInput::MouseMove(MouseMoveEvent {
            position: from,
            pressed_button: None,
            modifiers: Modifiers::default(),
        }))?;
        self.mouse(PlatformInput::MouseDown(MouseDownEvent {
            button: MouseButton::Left,
            position: from,
            modifiers: Modifiers::default(),
            click_count: 1,
            first_mouse: false,
        }))?;
        // Draw between events, rather than updating a synthetic selection range.
        for step in 1..=8 {
            self.mouse(PlatformInput::MouseMove(MouseMoveEvent {
                position: from + (to - from) * (step as f32 / 8.),
                pressed_button: Some(MouseButton::Left),
                modifiers: Modifiers::default(),
            }))?;
        }
        self.mouse(PlatformInput::MouseUp(MouseUpEvent {
            button: MouseButton::Left,
            position: to,
            modifiers: Modifiers::default(),
            click_count: 1,
        }))
    }

    fn quote(&mut self) -> Result<String> {
        if self.element("selection-toolbar").is_ok() {
            ensure!(
                self.element("comment-input").is_err(),
                "selection opened the comment editor directly"
            );
            self.element("selection-copy")?;
            self.action(
                serde_json::json!({"type":"click","target":{"element_id":"selection-comment"}}),
            )?;
        }
        Ok(self.element("comment-selected-quote")?.label)
    }

    fn close_quote(&mut self) -> Result<()> {
        self.action(
            serde_json::json!({"type":"click","target":{"element_id":"comment-popover-close"}}),
        )
    }
}

fn ends(element: &ElementInfo) -> (gpui::Point<gpui::Pixels>, gpui::Point<gpui::Pixels>) {
    let b = element.visible_bounds;
    let y = b.y + b.height.min(20.) * 0.5;
    (
        point(px(b.x + 0.1), px(y)),
        point(px(b.x + b.width - 0.1), px(y)),
    )
}

fn selection_toolbar(fixture: &mut Fixture) -> Result<()> {
    let label = "后续段落：中文🐈甲乙终。";
    let (from, to) = ends(&fixture.text(label)?);
    fixture.drag(from, to)?;
    fixture.element("selection-copy")?;
    let toolbar = fixture.element("selection-toolbar")?.visible_bounds;
    ensure!(
        (toolbar.height - 32.).abs() <= 1.,
        "toolbar height is {}",
        toolbar.height
    );
    ensure!(
        toolbar.y + toolbar.height <= from.y.as_f32(),
        "toolbar overlaps the selected line"
    );
    fixture
        .cx
        .capture_screenshot(fixture.window)?
        .save(std::env::temp_dir().join("zork-selection-toolbar-approved.png"))?;
    fixture.action(serde_json::json!({"type":"key","keystroke":"cmd-c"}))?;
    let shortcut = fixture
        .cx
        .update(|cx| cx.read_from_clipboard().and_then(|item| item.text()));
    ensure!(
        shortcut.as_deref() == Some(label),
        "keyboard copy lost selection"
    );
    ensure!(
        fixture.element("comment-input").is_err(),
        "selection opened comment input"
    );
    fixture.action(serde_json::json!({"type":"click","target":{"element_id":"selection-copy"}}))?;
    let copied = fixture
        .cx
        .update(|cx| cx.read_from_clipboard().and_then(|item| item.text()));
    ensure!(
        copied.as_deref() == Some(label),
        "copy did not preserve selected text: {copied:?}"
    );
    ensure!(
        fixture.element("selection-toolbar").is_err(),
        "copy left toolbar open"
    );
    fixture.drag(from, to)?;
    fixture.action(serde_json::json!({"type":"key","keystroke":"escape"}))?;
    ensure!(
        fixture.element("selection-toolbar").is_err(),
        "escape left toolbar open"
    );
    fixture.drag(from, to)?;
    fixture.action(serde_json::json!({"type":"click","target":{"x":5.,"y":5.}}))?;
    ensure!(
        fixture.element("selection-toolbar").is_err(),
        "outside click left toolbar open"
    );
    Ok(())
}

fn single_clicks(fixture: &mut Fixture) -> Result<()> {
    for label in [
        "前言段落甲乙。",
        "左列标题",
        "右列标题",
        "左甲乙丙",
        "右中文🐈末",
        "后续段落：中文🐈甲乙终。",
        "末尾段落丙丁。",
    ] {
        let element = fixture.text(label)?;
        fixture.action(serde_json::json!({"type":"click","target":{
            "x":element.visible_bounds.x+2.,"y":element.center.y
        }}))?;
        ensure!(
            fixture.element("comment-selected-quote").is_err(),
            "a single click selected text in {label}"
        );
    }
    Ok(())
}

fn same_row_cells(fixture: &mut Fixture) -> Result<()> {
    for label in ["左甲乙丙", "右中文🐈末"] {
        let element = fixture.text(label)?;
        let (from, to) = ends(&element);
        fixture.drag(from, to)?;
        ensure!(
            fixture.quote()? == label,
            "wrong column: expected {label:?}, got {:?}; bounds {:?}",
            fixture.quote()?,
            element.bounds
        );
        fixture.close_quote()?;
    }
    let left = fixture.text("左甲乙丙")?;
    let right = fixture.text("右中文🐈末")?;
    ensure!(
        (left.bounds.y - right.bounds.y).abs() < 1.,
        "fixture cells are not on one row"
    );
    fixture.drag(ends(&left).0, ends(&right).1)?;
    let plain = MessageDocument::parse(MARKDOWN).plain_text();
    let start = plain.find("左甲乙丙").unwrap();
    let end = plain.find("右中文🐈末").unwrap() + "右中文🐈末".len();
    ensure!(
        fixture.quote()? == plain[start..end],
        "cross-column quote is {:?}",
        fixture.quote()?
    );
    fixture.close_quote()
}

fn reverse_utf8_and_paragraphs(fixture: &mut Fixture) -> Result<()> {
    let text = "后续段落：中文🐈甲乙终。";
    let element = fixture.text(text)?;
    let (start, end) = ends(&element);
    fixture.drag(end, start)?;
    ensure!(
        fixture.quote()? == text,
        "reverse UTF-8 quote is {:?}",
        fixture.quote()?
    );
    fixture.close_quote()?;
    let first = fixture.text("前言段落甲乙。")?;
    let last = fixture.text("末尾段落丙丁。")?;
    fixture.drag(ends(&last).1, ends(&first).0)?;
    ensure!(
        fixture.quote()? == MessageDocument::parse(MARKDOWN).plain_text(),
        "reverse multi-block quote is {:?}",
        fixture.quote()?
    );
    fixture.close_quote()
}

fn markdown_polish(fixture: &mut Fixture) -> Result<()> {
    const CODE: &str = "let name = \"你好🐈\";";
    fixture.replace_message(&format!("正文起点\n\n- 列表正文\n- 第二项\n\n1. 数字使用等宽字体\n2. 后续文本保持对齐\n\n行内 `REPOS_ROOT` 与[文档](https://example.com/docs)。\n\n```rust\n{CODE}\n```"))?;
    let paragraph = fixture.text("正文起点")?;
    let item = fixture.text("列表正文")?;
    let indent = item.bounds.x - paragraph.bounds.x;
    ensure!((14.0..=18.0).contains(&indent), "list indent is {indent}px");
    let code = fixture.text(CODE)?;
    let (from, to) = ends(&code);
    fixture.drag(from, to)?;
    ensure!(
        fixture.quote()? == CODE,
        "code selection changed source: {:?}",
        fixture.quote()?
    );
    fixture.close_quote()?;
    Ok(())
}

fn markdown_polish_compact(fixture: &mut Fixture) -> Result<()> {
    *fixture = Fixture::new_sized(900., 600.)?;
    markdown_polish(fixture)
}

fn markdown_code_overflow(fixture: &mut Fixture) -> Result<()> {
    let code = format!("let value = \"{}\";", "long_code_".repeat(24));
    fixture.replace_message(&format!("```rust\n{code}\n```"))?;
    let before = fixture.text(&code)?;
    ensure!(
        before.bounds.height <= 22.,
        "code unexpectedly wrapped: {:?}",
        before.bounds
    );
    fixture.action(serde_json::json!({"type":"scroll","target":{
        "x":before.visible_bounds.x + 20.,"y":before.center.y
    },"delta_x":-180.,"delta_y":0.}))?;
    let after = fixture.text(&code)?;
    ensure!(
        after.bounds.x < before.bounds.x - 100.,
        "long code did not scroll horizontally: before {:?}, after {:?}",
        before.bounds,
        after.bounds
    );
    Ok(())
}

fn shared_service_link_opens_panel(fixture: &mut Fixture) -> Result<()> {
    let origin = zork_mesh::bridge::key_origin(&"00".repeat(32))?;
    fixture.replace_message(&format!(
        "[打开共享服务](zork://service/{}/{}/)",
        origin.trim_start_matches("key:"),
        "a".repeat(32)
    ))?;
    let link = fixture.text("打开共享服务")?;
    let position = point(px(link.bounds.x + 8.), px(link.center.y));
    fixture.mouse(PlatformInput::MouseDown(MouseDownEvent {
        button: MouseButton::Left,
        position,
        modifiers: Modifiers::default(),
        click_count: 1,
        first_mouse: false,
    }))?;
    fixture.mouse(PlatformInput::MouseUp(MouseUpEvent {
        button: MouseButton::Left,
        position,
        modifiers: Modifiers::default(),
        click_count: 1,
    }))?;
    for _ in 0..30 {
        fixture.frame()?;
    }
    fixture.element("browser-address")?;
    // This fixture deliberately has no Mesh node. The link must still open the
    // in-app panel (where connection errors are shown), never dispatch to the OS.
    fixture.action(
        serde_json::json!({"type":"click","target":{"element_id":"conversation-browser"}}),
    )?;
    for _ in 0..30 {
        fixture.frame()?;
    }
    Ok(())
}

fn markdown_link_hover(fixture: &mut Fixture) -> Result<()> {
    fixture.replace_message("[查看文档](https://example.com/docs)")?;
    let link = fixture.text("查看文档")?;
    fixture.mouse(PlatformInput::MouseMove(MouseMoveEvent {
        position: point(px(link.bounds.x + 8.), px(link.center.y)),
        pressed_button: None,
        modifiers: Modifiers::default(),
    }))?;
    for _ in 0..80 {
        fixture.frame()?;
    }
    ensure!(
        fixture.element("markdown-link-destination")?.label == "https://example.com/docs",
        "missing link destination"
    );
    let (from, to) = ends(&link);
    fixture.drag(from, to)?;
    ensure!(
        fixture.quote()? == "查看文档",
        "link drag must select the text"
    );
    fixture.close_quote()?;
    Ok(())
}

fn literal_user_message(fixture: &mut Fixture) -> Result<()> {
    const SOURCE: &str =
        "# 标题\n**中文🐈** `代码`\n[链接](https://example.test) &amp;\n```rust\nlet x = 1;\n```";
    fixture.replace_message_as(SOURCE, Role::User)?;
    let text = fixture.text(SOURCE)?;
    let bounds = text.visible_bounds;
    fixture.drag(
        point(px(bounds.x + 0.1), px(bounds.y + 10.)),
        point(
            px(bounds.x + bounds.width - 0.1),
            px(bounds.y + bounds.height - 10.),
        ),
    )?;
    ensure!(
        fixture.quote()? == SOURCE,
        "user selection must preserve literal Markdown and line breaks"
    );
    fixture.close_quote()?;
    Ok(())
}

fn cached_text_reflows_and_edits(fixture: &mut Fixture) -> Result<()> {
    *fixture = Fixture::new_sized(1280., 800.)?;
    let source = "中文 English 混排保持选择与换行一致。".repeat(8);
    fixture.replace_message(&source)?;
    let wide = fixture.text(&source)?.bounds.height;
    fixture.cx.update_window(fixture.window, |_, window, cx| {
        window.resize(size(px(800.), px(800.)));
        window.bounds_changed(cx);
    })?;
    for _ in 0..4 {
        fixture.frame()?;
    }
    let narrowed = fixture.text(&source)?;
    ensure!(
        narrowed.bounds.height > wide,
        "cached text did not reflow after resize"
    );
    let from = point(px(narrowed.bounds.x + 1.), px(narrowed.bounds.y + 8.));
    let to = point(
        px(narrowed.bounds.x + narrowed.bounds.width - 1.),
        px(narrowed.bounds.y + narrowed.bounds.height - 2.),
    );
    fixture.drag(from, to)?;
    ensure!(
        fixture.quote()? == source,
        "resized cached layout lost selection offsets"
    );
    fixture.close_quote()?;
    let changed = "更新后的正文 🐈。";
    fixture.replace_message(changed)?;
    ensure!(
        fixture.text(&source).is_err(),
        "edited text retained the old cached document"
    );
    let (from, to) = ends(&fixture.text(changed)?);
    fixture.drag(from, to)?;
    ensure!(
        fixture.quote()? == changed,
        "edited cached text has stale selection geometry"
    );
    fixture.close_quote()
}

fn main() -> Result<()> {
    std::env::set_var("SEED", "0");
    std::env::set_var("TZ", "UTC");
    std::env::set_var("ZORK_GUI_TEST_REDUCE_MOTION", "1");
    let output = std::env::var_os("ZORK_HEADLESS_SELECTION_OUTPUT")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("zork-headless-selection"));
    std::fs::create_dir_all(&output)?;
    let preferences = output.join("preferences.json");
    std::fs::write(&preferences, "{}")?;
    std::env::set_var("ZORK_GUI_PREFERENCES_PATH", preferences);
    let mut fixture = Fixture::new()?;
    std::fs::write(
        output.join("initial-elements.json"),
        serde_json::to_vec_pretty(&fixture.automation.snapshot(true))?,
    )?;
    let cases: [(&str, fn(&mut Fixture) -> Result<()>); 11] = [
        ("cached-text-resize-edit", cached_text_reflows_and_edits),
        ("markdown-polish", markdown_polish),
        ("markdown-link-hover", markdown_link_hover),
        ("shared-service-link", shared_service_link_opens_panel),
        ("markdown-polish-compact", markdown_polish_compact),
        ("markdown-code-overflow", markdown_code_overflow),
        ("selection-toolbar", selection_toolbar),
        ("single-click", single_clicks),
        ("literal-user-message", literal_user_message),
        ("same-row-columns", same_row_cells),
        ("reverse-utf8-multiple-blocks", reverse_utf8_and_paragraphs),
    ];
    let filter = std::env::args().skip(1).find(|arg| arg != "--nocapture");
    ensure!(
        filter
            .as_deref()
            .is_none_or(|filter| cases.iter().any(|(name, _)| *name == filter)),
        "unknown selection case: {filter:?}"
    );
    for (name, run) in cases {
        if filter.as_deref().is_some_and(|filter| filter != name) {
            continue;
        }
        fixture.replace_message(MARKDOWN)?;
        if let Err(error) = run(&mut fixture) {
            fixture
                .cx
                .capture_screenshot(fixture.window)?
                .save(output.join(format!("{name}-failed.png")))?;
            return Err(anyhow!("{name}: {error:#}"));
        }
        fixture
            .cx
            .capture_screenshot(fixture.window)?
            .save(output.join(format!("{name}.png")))?;
        println!("PASS headless selection: {name}");
    }
    fixture
        .cx
        .capture_screenshot(fixture.window)?
        .save(output.join("selection.png"))?;
    println!(
        "No system windows or HTTP services; fixed virtual step: 16 ms; evidence: {}",
        output.display()
    );
    Ok(())
}
