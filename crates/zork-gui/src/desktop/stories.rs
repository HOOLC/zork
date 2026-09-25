//! Development-only component stories. Every sample calls production renderers.
use super::{profiles::ProfilesView, ui};
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    design::ZORK_UI,
};
use gpui::{div, prelude::*, px, rgb, AnyView, Context, Window};
use serde_json::{json, Value};

pub use zork_ui::stories::{PrimitiveStory, Story};

pub(super) fn new_chat_story(
    state: &str,
    width: f32,
    cx: &mut gpui::App,
) -> gpui::Entity<zork_ui::new_chat::Page> {
    let fixture = std::rc::Rc::new(std::cell::RefCell::new(
        zork_client_core::new_chat::Fixture::new(state),
    ));
    let text = zork_ui::resources::Text(std::rc::Rc::new(|key| {
        crate::i18n::Locale::ZhCn.text(key).into()
    }));
    let form_width = (width - 48.).clamp(220., 744.);
    let view = cx.new(|cx| {
        let mut view = zork_ui::new_chat::Page::new(text.clone(), cx);
        view.configure(fixture.borrow().snapshot(), form_width, text.clone(), cx);
        view
    });
    cx.subscribe(&view, move |view, event: &zork_ui::new_chat::Event, cx| {
        match event {
            zork_ui::new_chat::Event::Intent(action) => fixture.borrow_mut().apply(action.clone()),
            zork_ui::new_chat::Event::SelectDevice(id) => fixture.borrow_mut().select_device(id),
            zork_ui::new_chat::Event::ConfigureModels => return,
        }
        view.update(cx, |v, cx| {
            v.configure(fixture.borrow().snapshot(), form_width, text.clone(), cx)
        });
    })
    .detach();
    view
}
fn click(id: &str) -> Value {
    json!({"type":"click","target":{"element_id":id}})
}
pub fn install(cx: &mut gpui::App) {
    cx.set_global(zork_ui::history_page::stories::StoryText(
        zork_ui::resources::Text(std::rc::Rc::new(|key| {
            crate::i18n::Locale::ZhCn.text(key).into()
        })),
    ));
}

/// Product areas, in sidebar order.
pub const GROUPS: [&str; 7] = [
    "基础控件",
    "对话",
    "导航与新建",
    "执行历史",
    "设置与设备",
    "首次使用",
    "文件",
];

/// Sidebar placement for each render family: group, entry id and entry title.
fn placement(family: &str) -> Option<(&'static str, &'static str, &'static str)> {
    Some(match family {
        "button" => ("基础控件", "button", "按钮"),
        "field" => ("基础控件", "field", "输入框"),
        "choice" => ("基础控件", "choice", "选项"),
        "switch" => ("基础控件", "switch", "开关"),
        "dropdown" => ("基础控件", "dropdown", "下拉菜单"),
        "modal" => ("基础控件", "modal", "弹窗"),
        "feedback" => ("基础控件", "feedback", "状态提示"),
        "loading" => ("基础控件", "loading", "加载状态"),
        "navigation" => ("基础控件", "navigation", "导航行"),
        "device-name" => ("基础控件", "device-name", "设备标识"),
        "brand" | "icons" | "providers" => ("基础控件", "icons-brand", "图标与品牌"),
        "conversation" => ("对话", "conversation", "会话"),
        "markdown" => ("对话", "markdown", "消息正文"),
        "multi-agent" => ("对话", "multi-agent", "多 Agent 消息"),
        "comments" => ("对话", "comments", "文字评论"),
        "activity" => ("对话", "activity", "会话动态"),
        "message-interaction" => ("对话", "message-interaction", "交互卡片"),
        "browser" => ("对话", "browser", "页面浏览器"),
        "chat-navigation" => ("导航与新建", "chat-navigation", "侧栏与设备坞"),
        "new-chat" => ("导航与新建", "new-chat", "新建 Chat"),
        "history" => ("执行历史", "history", "执行历史"),
        "history-details" => ("执行历史", "history-details", "执行记录详情"),
        "connection" => ("设置与设备", "connection", "模型连接"),
        "model" => ("设置与设备", "model", "模型配置"),
        "profile-card" => ("设置与设备", "profile-card", "模型连接卡片"),
        "client" => ("设置与设备", "client", "客户端设置"),
        "notifications" => ("设置与设备", "notifications", "通知设置"),
        "data-settings" => ("设置与设备", "data-settings", "清空本机数据"),
        "device" => ("设置与设备", "device", "设备设置"),
        "mesh" => ("设置与设备", "mesh", "设备连接"),
        "enrollment" => ("设置与设备", "enrollment", "连接设备"),
        "node-directory" => ("设置与设备", "node-directory", "本机与已存设备"),
        "resources" => ("设置与设备", "resources", "资源目录"),
        "onboarding" => ("首次使用", "onboarding", "首次使用"),
        "attachment" => ("文件", "attachment", "附件"),
        "attachment-viewer" => ("文件", "attachment-viewer", "附件预览"),
        "conversation-files" => ("文件", "conversation-files", "会话文件与页面"),
        _ => return None,
    })
}

/// Repository-relative source file for "open in editor".
fn source_path(source: &str) -> String {
    let first = source
        .split(" / ")
        .next()
        .unwrap_or(source)
        .split(" + ")
        .next()
        .unwrap_or(source)
        .split("::")
        .next()
        .unwrap_or(source)
        .trim();
    if first.starts_with("crates/") {
        first.to_owned()
    } else if let Some(rest) = first.strip_prefix("desktop/") {
        format!("crates/zork-gui/src/desktop/{rest}")
    } else if first.starts_with("zork-") {
        format!("crates/{first}")
    } else {
        first.to_owned()
    }
}

fn label(family: &str, state: &str) -> String {
    use zork_ui::component_story::business::state_label;
    match (family, state) {
        ("history", "collapsed") => "正常记录".into(),
        ("history", "expanded") => "全部展开".into(),
        ("icons", _) => "功能图标".into(),
        ("providers", _) => "供应商图标".into(),
        ("brand", "linked") => "品牌 · 组合标志".into(),
        ("brand", state) => format!("品牌 · {}", state_label(state)),
        ("conversation", "history") => "执行历史".into(),
        ("multi-agent", "conversation") => "多 Agent 对话".into(),
        ("multi-agent", "comment-batch") => "发出的引用回复".into(),
        ("multi-agent", "run-hover") => "同一 Agent 连续消息 · 悬停时间".into(),
        ("multi-agent", "time-hover") => "相对时间 · 悬停完整时间".into(),
        ("multi-agent", "reply-jump") => "点击引用 · 跳转并标记原文".into(),
        ("multi-agent", "not-loaded") => "引用 · 原消息尚未加载".into(),
        ("multi-agent", "loaded") => "引用 · 第一次点击只加载".into(),
        ("multi-agent", "drafts") => "引用回复 · 草稿".into(),
        ("multi-agent", "seven-agents") => "7 个 Agent · 色盘重复".into(),
        ("multi-agent", "phone") => "手机宽度".into(),
        ("activity", "collapsed") => "收起".into(),
        ("activity", "expanded") => "展开".into(),
        ("activity", "live") => "实时（动效演示）".into(),
        ("activity", "waiting") => "等待记录".into(),
        ("activity", "finished") => "已结束".into(),
        ("activity", "disconnected") => "连接中断".into(),
        ("button" | "field" | "dropdown" | "choice", "disabled") => "禁用".into(),
        _ => state_label(state),
    }
}

/// Folds `-compact`/`-wide` pairs into one state with an alternative size,
/// assigns every story a product area, and orders the catalog by area.
fn organize(items: Vec<Story>) -> Vec<Story> {
    let mut out: Vec<Story> = Vec::new();
    for mut story in items {
        if let Some(base) = story.state.strip_suffix("-wide") {
            if let Some(compact) = out
                .iter_mut()
                .find(|s| s.family == story.family && s.state == base)
            {
                compact.wide = Some([story.width, story.height]);
                continue;
            }
        }
        if let Some(base) = story.state.strip_suffix("-compact").map(str::to_owned) {
            story.id = format!("{}-{base}", story.family);
            story.state = base;
        }
        let Some((group, entry, title)) = placement(&story.family) else {
            continue;
        };
        story.group = group.into();
        story.entry = entry.into();
        story.entry_title = title.into();
        story.label = label(&story.family, &story.state);
        story.source = source_path(&story.source);
        out.push(story);
    }
    // The narrow history fixture is a width, not a state.
    out.retain(|s| !(s.family == "history" && s.state == "narrow"));
    // Group by area, then keep each entry's states together in first-seen order.
    let mut entries: Vec<String> = Vec::new();
    for story in &out {
        if !entries.contains(&story.entry) {
            entries.push(story.entry.clone());
        }
    }
    out.sort_by_key(|s| {
        (
            GROUPS
                .iter()
                .position(|g| *g == s.group)
                .unwrap_or(GROUPS.len()),
            entries.iter().position(|e| *e == s.entry).unwrap_or(0),
        )
    });
    out
}

/// Resolves a story id, including the retired `-compact`/`-wide` and
/// `history-narrow` ids, to a catalog story at that size.
pub fn resolve(catalog: &[Story], id: &str) -> Option<Story> {
    if let Some(story) = catalog.iter().find(|s| s.id == id) {
        return Some(story.clone());
    }
    if id == "history-narrow" {
        let mut story = catalog
            .iter()
            .find(|s| s.id == "history-collapsed")?
            .clone();
        story.width = 320.;
        return Some(story);
    }
    if let Some(base) = id.strip_suffix("-wide") {
        let mut story = catalog.iter().find(|s| s.id == base)?.clone();
        if let Some([w, h]) = story.wide {
            story.width = w;
            story.height = h;
        }
        return Some(story);
    }
    id.strip_suffix("-compact")
        .and_then(|base| catalog.iter().find(|s| s.id == base))
        .cloned()
}

/// Every story a test may open by id: the catalog plus hidden walkthroughs.
pub fn fixture(id: &str) -> Option<Story> {
    resolve(&catalog(), id).or_else(|| {
        zork_ui::stories::fixtures()
            .into_iter()
            .find(|s| s.id == id)
    })
}

pub fn catalog() -> Vec<Story> {
    organize(raw_catalog())
}

fn raw_catalog() -> Vec<Story> {
    let mut items = zork_ui::stories::catalog();
    for state in [
        "login",
        "waiting",
        "preparing",
        "failure",
        "models",
        "model-form",
        "ready",
    ] {
        for (width, suffix) in [(360., "compact"), (960., "wide")] {
            let mut story = Story::new(
                "onboarding",
                "首次使用",
                &format!("{state}-{suffix}"),
                "crates/zork-ui/src/onboarding.rs / crates/zork-gui/src/desktop/startup.rs",
                "onboarding",
            );
            story.width = width;
            story.height = 680.;
            story.target = match state {
                "login" => "desktop-welcome-login",
                "waiting" => "onboarding-cancel-login",
                "preparing" => "onboarding-preparing",
                "failure" => "desktop-startup-retry",
                "models" => "onboarding-add-model",
                "model-form" => "profile-create-dialog",
                _ => "new-chat-welcome",
            }
            .into();
            items.push(story);
        }
    }
    for story in &mut items {
        if story.family == "history" {
            story.source = "crates/zork-ui/src/history_page/mod.rs".into();
        }
    }
    for (family, title, states, source) in [
        (
            "new-chat",
            "新建 Chat",
            &["draft", "no-models", "creating", "retry", "error"][..],
            "crates/zork-ui/src/new_chat.rs",
        ),
        (
            "node-directory",
            "本机与已存设备",
            &[
                "empty",
                "running",
                "stopped",
                "background",
                "pairing",
                "loading",
            ][..],
            "crates/zork-ui/src/node_directory.rs",
        ),
    ] {
        for state in states {
            let mut story = Story::new(family, title, state, source, family);
            story.width = 900.;
            story.height = 700.;
            if family == "new-chat" {
                // Stills and thumbnails show the composer, not the empty page.
                story.target = "new-chat-form".into();
            }
            items.push(story);
        }
    }
    for state in ["entry", "long"] {
        let mut story = Story::new(
            "history-details",
            "执行记录与成员详情",
            state,
            "crates/zork-ui/src/history_details.rs",
            "history-details",
        );
        story.width = 900.;
        story.height = 700.;
        story.actions = vec![click("history-details-open")];
        items.push(story);
    }
    for state in ["idle", "error"] {
        let mut story = Story::new(
            "data-settings",
            "清空本机数据",
            state,
            "crates/zork-ui/src/settings/data.rs",
            "data-settings",
        );
        story.width = 640.;
        story.height = 640.;
        story.actions = vec![click("clear-client-data")];
        items.push(story);
    }
    for state in ["empty", "tabs", "applications", "loading", "error"] {
        let mut story = Story::new(
            "browser",
            "页面浏览器",
            state,
            "crates/zork-ui/src/browser_chrome.rs",
            "browser",
        );
        story.width = 640.;
        story.height = 600.;
        items.push(story);
    }
    for state in ["image", "document", "long", "loading", "error"] {
        let mut story = Story::new(
            "attachment-viewer",
            "附件预览",
            state,
            "crates/zork-ui/src/attachment_viewer.rs",
            "attachment-viewer",
        );
        story.width = 900.;
        story.height = 760.;
        story.actions = vec![click("attachment-viewer-open")];
        items.push(story);
    }
    for state in ["menu", "toolbar", "files", "pages", "empty"] {
        let mut story = Story::new(
            "conversation-files",
            "会话文件与页面",
            state,
            "crates/zork-ui/src/conversation_contents.rs",
            "conversation-files",
        );
        story.width = 640.;
        story.height = 600.;
        items.push(story);
    }
    for state in [
        "connected",
        "creator",
        "unread",
        "collapsed",
        "offline",
        "empty",
    ] {
        let mut story = Story::new(
            "chat-navigation",
            "设备与 Chat 导航",
            state,
            "crates/zork-ui/src/chat_navigation.rs",
            "chat-navigation",
        );
        story.width = 320.;
        story.height = 760.;
        items.push(story);
    }
    for state in ["list", "detail", "services", "empty", "loading", "error"] {
        let mut story = Story::new(
            "resources",
            "资源目录与详情",
            state,
            "crates/zork-ui/src/resources.rs",
            "resources",
        );
        story.width = 900.;
        story.height = 600.;
        items.push(story);
    }
    for (family, title, state, target, actions, reference) in [
        (
            "connection",
            "大模型",
            "list",
            "desktop-settings-column",
            vec![],
            "connection-list",
        ),
        (
            "connection",
            "大模型",
            "create",
            "profile-create-dialog",
            vec![click("profile-add")],
            "connection-create",
        ),
        (
            "connection",
            "大模型",
            "provider",
            "profile-create-dialog",
            vec![click("profile-add"), click("profile-next")],
            "connection-provider",
        ),
        (
            "conversation",
            "会话",
            "messages",
            "story-component",
            vec![],
            "conversation",
        ),
        (
            "conversation",
            "会话",
            "composer",
            "composer-surface",
            vec![],
            "composer",
        ),
        (
            "conversation",
            "会话",
            "history",
            "story-component",
            vec![],
            "conversation-history",
        ),
    ] {
        for (width, height, suffix) in [(900., 600., "compact"), (1280., 800., "wide")] {
            let mut story = Story::new(
                family,
                title,
                &format!("{state}-{suffix}"),
                if family == "conversation" {
                    "crates/zork-ui/src/components/message_row.rs / history_page/mod.rs"
                } else {
                    "desktop/profiles.rs"
                },
                reference,
            );
            story.width = width;
            story.height = height;
            story.target = target.into();
            story.actions = actions.clone();
            items.push(story);
        }
    }
    for (family, title, states) in [
        (
            "client",
            "客户端设置",
            &["signed-out", "signed-in", "loading", "error", "archived", "archived-empty"][..],
        ),
        (
            "device",
            "设备设置",
            &["running", "stopped", "loading", "error", "renaming", "rename-error"][..],
        ),
        ("mesh", "设备连接", &["connected", "empty", "manual"][..]),
        (
            "enrollment",
            "连接设备",
            &["loading", "command", "error", "expired"][..],
        ),
    ] {
        for state in states {
            for (width, height, suffix) in [(900., 600., "compact"), (1280., 800., "wide")] {
                let mut story = Story::new(
                    family,
                    title,
                    &format!("{state}-{suffix}"),
                    if matches!(family, "mesh" | "enrollment") {
                        "zork-ui/src/network.rs"
                    } else {
                        "zork-ui/src/settings.rs"
                    },
                    family,
                );
                story.width = width;
                story.height = height;
                story.target = if family == "enrollment" {
                    "add-device-dialog"
                } else if family == "mesh" && *state == "manual" {
                    "mesh-peer-dialog"
                } else {
                    "desktop-settings-column"
                }
                .into();
                items.push(story);
            }
        }
    }
    for state in [
        "approval",
        "approved",
        "declined",
        "cancelled",
        "input",
        "prefilled",
        "create",
        "update",
        "login",
        "login-device",
        "login-callback",
        "login-completed",
        "completed",
        "long",
    ] {
        for (width, height, suffix) in [(420., 760., "compact"), (900., 700., "wide")] {
            let mut story = Story::new(
                "message-interaction",
                "交互消息",
                &format!("{state}-{suffix}"),
                "zork-ui/src/components/interaction.rs",
                "interaction",
            );
            story.width = width;
            story.height = height;
            story.target = "interaction-card-preview".into();
            items.push(story);
        }
    }
    for state in ["enabled", "disabled", "muted", "denied", "busy", "error"] {
        let mut story = Story::new(
            "notifications",
            "通知设置",
            state,
            "zork-ui/src/settings/notifications.rs",
            "notifications",
        );
        story.width = 900.;
        story.height = 680.;
        items.push(story);
    }
    // Session activity in the real Chat view: the last item of the message list.
    for state in [
        "collapsed",
        "expanded",
        "live",
        "waiting",
        "finished",
        "disconnected",
    ] {
        for (width, height, suffix) in [(900., 640., "compact"), (1440., 800., "wide")] {
            let mut story = Story::new(
                "activity",
                "会话动态",
                &format!("{state}-{suffix}"),
                "crates/zork-ui/src/components/activity.rs + zork-gui/src/views/session_activity.rs",
                "activity",
            );
            story.width = width;
            story.height = height;
            items.push(story);
        }
    }
    // Chats shared by several agents, drawn from core's message presentation.
    for (state, width, height) in zork_ui::components::message_row::multi_agent::STATES {
        let mut story = Story::new(
            "multi-agent",
            "多 Agent 消息",
            state,
            "crates/zork-ui/src/components/message_row/multi_agent.rs + message_row/identity.rs",
            "multi-agent",
        );
        story.width = width;
        story.height = height;
        story
            .actions
            .extend(zork_ui::components::message_row::multi_agent::actions(state));
        items.push(story);
    }
    for state in ["long", "loading", "empty", "offline", "error"] {
        let mut story = Story::new(
            "conversation",
            "会话",
            state,
            "crates/zork-ui/src/components/message_row.rs",
            "conversation",
        );
        story.width = 900.;
        story.height = 700.;
        items.push(story);
    }
    // Model editor: one story per state, driven through the real editor.
    let typed = |id: &str| {
        vec![
            click("profile-model-add"),
            click("profile-model"),
            json!({"type":"type_text","text":id}),
            json!({"type":"key","keystroke":"enter"}),
        ]
    };
    let then = |mut first: Vec<Value>, rest: Vec<Value>| {
        first.extend(rest);
        first
    };
    let dialog = "model-editor-dialog";
    let page = "profile-detail-dialog";
    for (state, target, actions) in [
        ("list", page, vec![]),
        ("add", dialog, vec![click("profile-model-add")]),
        (
            "suggestions",
            dialog,
            vec![
                click("profile-model-add"),
                click("profile-model"),
                json!({"type":"type_text","text":"gpt-5"}),
                json!({"type":"key","keystroke":"down"}),
            ],
        ),
        ("recognized", dialog, typed("gpt-5-nano")),
        ("variant", dialog, typed("gpt-5-nano-2026-08-07")),
        (
            "levels",
            dialog,
            then(typed("gpt-5-nano"), vec![click("model-section-thinking")]),
        ),
        (
            "modified",
            dialog,
            then(
                typed("gpt-5-nano"),
                vec![click("model-section-thinking"), click("model-level-0")],
            ),
        ),
        (
            "level-add",
            dialog,
            then(
                typed("gpt-5-nano"),
                vec![click("model-section-thinking"), click("model-level-add")],
            ),
        ),
        (
            "budget",
            dialog,
            then(
                typed("claude-sonnet-4-5"),
                vec![
                    click("model-section-thinking"),
                    click("model-budget-add"),
                    json!({"type":"type_text","text":"64K"}),
                    json!({"type":"key","keystroke":"enter"}),
                ],
            ),
        ),
        (
            "length",
            dialog,
            then(
                typed("gpt-5-nano"),
                vec![
                    click("model-section-length"),
                    click("profile-output-limit"),
                ],
            ),
        ),
        ("unknown", dialog, typed("my-model")),
        (
            "errors",
            dialog,
            then(typed("my-model"), vec![click("profile-model-save")]),
        ),
        (
            "sources",
            dialog,
            then(typed("my-model"), vec![click("model-source-open")]),
        ),
        ("duplicate", dialog, typed("gpt-5")),
        ("inline", page, vec![click("model-edit-gpt-5-mini")]),
        (
            "inline-restore",
            page,
            vec![
                click("model-edit-gpt-5-mini"),
                click("model-section-thinking"),
            ],
        ),
        ("inline-unconfigured", page, vec![click("model-edit-internal-preview")]),
        ("fetched", page, vec![click("profile-model-discover")]),
        (
            "custom",
            dialog,
            then(typed("qwen3-coder-plus"), vec![click("model-section-api")]),
        ),
    ] {
        let mut story = Story::new(
            "model",
            "模型配置",
            state,
            "desktop/profiles/editor.rs",
            "model-editor",
        );
        story.width = 1100.;
        story.height = 820.;
        story.target = target.into();
        story.actions = actions;
        items.push(story);
    }
    // The model panel: automatic connection, the connection list unfolded,
    // then pinned to API (the trigger names it).
    for (state, actions) in [
        ("picker", vec![click("new-chat-options")]),
        (
            "picker-connections",
            vec![click("new-chat-options"), click("new-chat-connection")],
        ),
        (
            "picker-pinned",
            vec![
                click("new-chat-options"),
                click("new-chat-connection"),
                click("new-chat-connection-2"),
            ],
        ),
    ] {
        let mut picker = Story::new(
            "new-chat",
            "新建 Chat",
            state,
            "crates/zork-ui/src/new_chat/picker.rs",
            "new-chat",
        );
        picker.width = 900.;
        // Tall enough for the unfolded panel below the composer, clear of the trigger.
        picker.height = 1000.;
        picker.actions = actions;
        items.push(picker);
    }
    items
}

pub struct StoryHost {
    inner: AnyView,
    settings: bool,
    _live: Option<gpui::Task<()>>,
    _directory: tempfile::TempDir,
}
impl StoryHost {
    pub fn specimen_focus(&self, cx: &gpui::App) -> Option<gpui::FocusHandle> {
        self.inner
            .clone()
            .downcast::<PrimitiveStory>()
            .ok()
            .map(|view| view.read(cx).specimen_focus())
    }

    /// Presentation-only bulk expansion for the native specimen host.
    pub fn history_expanded(&self, cx: &gpui::App) -> bool {
        use zork_ui::history_page::Host;
        self.inner
            .clone()
            .downcast::<zork_ui::history_page::stories::Story>()
            .is_ok_and(|view| {
                let state = view.read(cx).history();
                !state.expanded.is_empty() || !state.output_expanded.is_empty()
            })
    }

    pub fn set_history_expanded(&mut self, expanded: bool, cx: &mut Context<Self>) {
        use zork_ui::history_page::Host;
        if let Ok(view) = self
            .inner
            .clone()
            .downcast::<zork_ui::history_page::stories::Story>()
        {
            view.update(cx, |v, cx| {
                let state = v.history_mut();
                state.hold_disclosure(0);
                state.expanded.clear();
                state.output_expanded.clear();
                state.records_expanded.clear();
                if expanded {
                    for block in &state.projection.blocks {
                        if block.is_group() {
                            let entry = state.projection.activities[block.start].entry;
                            state.expanded.insert(state.entries[entry].id.clone());
                        }
                    }
                    for activity in &state.projection.activities {
                        if activity.kind == zork_ui::history::activity::Kind::Output {
                            state
                                .output_expanded
                                .insert(state.entries[activity.entry].id.clone());
                        }
                    }
                }
                state.rebuild_rows();
                zork_ui::components::region::invalidate_all(cx);
                cx.emit(zork_ui::history_page::HistoryChanged::clock());
                cx.notify();
            });
            cx.notify();
        }
    }
    pub fn inspect(&self, cx: &gpui::App) -> Value {
        if let Ok(view) = self.inner.clone().downcast::<zork_ui::new_chat::Page>() {
            return view.read(cx).inspect();
        }
        if let Ok(view) = self
            .inner
            .clone()
            .downcast::<zork_ui::settings::data::DataSettings>()
        {
            return view.read(cx).inspect();
        }
        if let Ok(view) = self.inner.clone().downcast::<PrimitiveStory>() {
            return view.read(cx).inspect(cx);
        }
        if let Ok(view) = self.inner.clone().downcast::<ProfilesView>() {
            return view.read(cx).headless_state(cx);
        }
        if let Ok(view) = self
            .inner
            .clone()
            .downcast::<super::onboarding_story::OnboardingStory>()
        {
            return view.read(cx).inspect();
        }
        if let Ok(view) = self
            .inner
            .clone()
            .downcast::<super::interaction_story::InteractionStory>()
        {
            return view.read(cx).inspect();
        }
        json!({})
    }

    pub fn new(story: Story, cx: &mut Context<Self>) -> Self {
        install(cx);
        let directory = tempfile::tempdir().expect("isolated story directory");
        let settings = matches!(
            story.family.as_str(),
            "connection" | "model" | "client" | "device" | "mesh" | "enrollment"
        );
        let mut live = None;
        let inner = match story.family.as_str() {
            "activity" => {
                let store = std::sync::Arc::new(
                    crate::desktop::store::ClientStore::open(directory.path())
                        .expect("story client store"),
                );
                let state = story
                    .state
                    .trim_end_matches("-compact")
                    .trim_end_matches("-wide")
                    .to_owned();
                let view = cx.new(|cx| {
                    let mut view = crate::views::RootView::render_benchmark_fixture(false, store, cx);
                    view.benchmark_activity_story(&state, cx);
                    view
                });
                if state == "live" {
                    // Plays the band's life: steps change, the round ends and
                    // leaves, then a new round appears.
                    let weak = view.downgrade();
                    live = Some(cx.spawn(async move |_, cx| {
                        let mut step = 0;
                        loop {
                            cx.background_executor()
                                .timer(std::time::Duration::from_millis(1800))
                                .await;
                            step += 1;
                            let ok = weak.update(cx, |view, cx| match step % 5 {
                                3 => view.benchmark_activity_finish(cx),
                                4 => view.benchmark_activity_story("live", cx),
                                n => view.benchmark_activity_step(n, cx),
                            });
                            if ok.is_err() {
                                return;
                            }
                        }
                    }));
                }
                view.into()
            }
            "onboarding" => cx
                .new(|cx| {
                    // The onboarding fixture lays out for the compact or wide window.
                    let size = if story.width < 600. { "compact" } else { "wide" };
                    super::onboarding_story::OnboardingStory::new(
                        &format!("{}-{size}", story.state),
                        cx,
                    )
                })
                .into(),
            "new-chat" => new_chat_story(&story.state, story.width, cx).into(),
            "node-directory" => cx
                .new(|cx| zork_ui::node_directory::Story::new(&story.state, cx))
                .into(),
            "history" => cx
                .new(|cx| {
                    zork_ui::history_page::stories::Story::with_text(
                        &story.state,
                        zork_ui::resources::Text(std::rc::Rc::new(|key| {
                            crate::i18n::Locale::ZhCn.text(key).into()
                        })),
                        cx,
                    )
                })
                .into(),
            "conversation" if story.state.starts_with("history") => cx
                .new(|cx| {
                    zork_ui::history_page::stories::Story::with_text(
                        "compact",
                        zork_ui::resources::Text(std::rc::Rc::new(|key| {
                            crate::i18n::Locale::ZhCn.text(key).into()
                        })),
                        cx,
                    )
                })
                .into(),
            "history-details" => cx
                .new(|cx| {
                    zork_ui::history_details::stories::Story::new(
                        &story.state,
                        zork_ui::resources::Text(std::rc::Rc::new(|key| {
                            crate::i18n::Locale::ZhCn.text(key).into()
                        })),
                        cx,
                    )
                })
                .into(),
            "data-settings" => cx
                .new(|cx| {
                    zork_ui::settings::data::DataSettings::new(
                        zork_ui::settings::data::Data {
                            busy: false,
                            error: (story.state == "error")
                                .then(|| "本机运行尚未结束，请稍后重试".into()),
                        },
                        zork_ui::resources::Text(std::rc::Rc::new(|key| {
                            crate::i18n::Locale::ZhCn.text(key).into()
                        })),
                        cx,
                    )
                })
                .into(),
            "browser" => cx
                .new(|cx| {
                    zork_ui::browser_chrome::stories::Story::new(
                        &story.state,
                        zork_ui::resources::Text(std::rc::Rc::new(|key| {
                            crate::i18n::Locale::ZhCn.text(key).into()
                        })),
                        cx,
                    )
                })
                .into(),
            "attachment-viewer" => cx
                .new(|cx| {
                    zork_ui::attachment_viewer::stories::Story::new(
                        &story.state,
                        zork_ui::resources::Text(std::rc::Rc::new(|key| {
                            crate::i18n::Locale::ZhCn.text(key).into()
                        })),
                        cx,
                    )
                })
                .into(),
            "conversation-files" => cx
                .new(|cx| {
                    zork_ui::conversation_contents::stories::Story::new(
                        &story.state,
                        zork_ui::resources::Text(std::rc::Rc::new(|key| {
                            crate::i18n::Locale::ZhCn.text(key).into()
                        })),
                        cx,
                    )
                })
                .into(),
            "chat-navigation" => {
                let view = zork_ui::chat_navigation::stories::create(
                    &story.state,
                    zork_ui::resources::Text(std::rc::Rc::new(|key| {
                        crate::i18n::Locale::ZhCn.text(key).into()
                    })),
                    cx,
                );
                // The meta line's relative time comes from core `message_time`.
                view.update(cx, |view, cx| {
                    view.set_time_format(
                        super::navigation::chat_time_format(crate::i18n::Locale::ZhCn),
                        cx,
                    )
                });
                view.into()
            }
            "resources" => zork_ui::resources::stories::create(
                &story.state,
                zork_ui::resources::Text(std::rc::Rc::new(|key| {
                    crate::i18n::Locale::ZhCn.text(key).into()
                })),
                cx,
            )
            .into(),
            "notifications" => cx
                .new(|cx| super::notification_story::NotificationStory::new(&story.state, cx))
                .into(),
            "message-interaction" => cx
                .new(|cx| super::interaction_story::InteractionStory::new(&story.state, cx))
                .into(),
            "mesh" | "enrollment" => cx
                .new(|cx| {
                    zork_ui::network::NetworkStory::new(
                        story.family.clone(),
                        story
                            .state
                            .trim_end_matches("-compact")
                            .trim_end_matches("-wide")
                            .into(),
                        zork_client_core::device_edit::validate_peer,
                        cx,
                    )
                })
                .into(),
            "client" | "device" => cx
                .new(|cx| {
                    zork_ui::settings::SettingsStory::new(
                        story.family.clone(),
                        story
                            .state
                            .trim_end_matches("-compact")
                            .trim_end_matches("-wide")
                            .into(),
                        zork_client_core::device_edit::validate_story_display_name,
                        cx,
                    )
                })
                .into(),
            "connection" => cx
                .new(|cx| ProfilesView::headless_fixture(false, cx))
                .into(),
            "model" => cx
                .new(|cx| ProfilesView::headless_model_fixture(story.state.starts_with("custom"), cx))
                .into(),
            "multi-agent" => zork_ui::components::message_row::multi_agent::create(
                &story.state,
                std::rc::Rc::new(|request: serde_json::Value| {
                    // The same JSON bridge Android uses; every rule is core's.
                    serde_json::from_value(request)
                        .map_err(anyhow::Error::from)
                        .and_then(zork_client_core::message_presentation::handle)
                        .unwrap_or_default()
                }),
                cx,
            )
            .into(),
            "conversation" => cx
                .new(|cx| {
                    zork_ui::components::message_row::stories::Story::new(
                        &story.state,
                        zork_ui::resources::Text(std::rc::Rc::new(|key| {
                            crate::i18n::Locale::ZhCn.text(key).into()
                        })),
                        cx,
                    )
                })
                .into(),
            _ => cx.new(|cx| PrimitiveStory::new(story, cx)).into(),
        };
        Self {
            inner,
            settings,
            _live: live,
            _directory: directory,
        }
    }
}
impl Render for StoryHost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let inner = if self.settings {
            div()
                .size_full()
                .bg(rgb(ZORK_UI.palette.canvas))
                .child(ui::settings_content(self.inner.clone()))
        } else {
            div().size_full().child(self.inner.clone())
        };
        div()
            .id("story-component")
            .size_full()
            .font_family("Inter Variable")
            .text_size(px(13.))
            .text_color(rgb(ZORK_UI.palette.text))
            .bg(rgb(ZORK_UI.palette.canvas))
            .child(inner)
            .automation(AutomationRole::Status, "组件画布")
    }
}
