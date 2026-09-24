//! Interactive stories built from the public component package, without app services.
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    components::{
        activity,
        brand::{Brand, BrandMotion},
        message,
        profile_card::{self, DeviceIdentity, ProfileCard, Quota, QuotaWindow},
        text_input::ComposerInput,
    },
    controls as ui,
    design::ZORK_UI,
    navigation,
};
use gpui::{div, prelude::*, px, rgb, Context, Entity, Window};
use serde::Serialize;
use serde_json::{json, Value};
pub fn page_fixture() -> Value {
    serde_json::from_str(include_str!("../assets/stories/page-fixture.json")).expect("page fixture")
}
#[derive(Clone, Serialize)]
pub struct Story {
    pub id: String,
    pub family: String,
    pub title: String,
    pub state: String,
    pub source: String,
    pub reference: String,
    pub width: f32,
    pub height: f32,
    pub target: String,
    pub actions: Vec<Value>,
    /// Product area in the design browser sidebar (基础控件, 对话, …).
    pub group: String,
    /// Sidebar entry; several render families can share one entry.
    pub entry: String,
    pub entry_title: String,
    /// Human label of this state.
    pub label: String,
    /// Alternative wide size for pages that used to ship as `-compact`/`-wide` pairs.
    pub wide: Option<[f32; 2]>,
}
impl Story {
    pub fn new(family: &str, title: &str, state: &str, source: &str, reference: &str) -> Self {
        Self {
            id: format!("{family}-{state}"),
            family: family.into(),
            title: title.into(),
            state: state.into(),
            source: source.into(),
            reference: reference.into(),
            width: 560.,
            height: 360.,
            target: "story-component".into(),
            actions: vec![],
            group: String::new(),
            entry: family.into(),
            entry_title: title.into(),
            label: state.into(),
            wide: None,
        }
    }
}
fn click(id: &str) -> Value {
    json!({"type":"click","target":{"element_id":id}})
}
pub fn catalog() -> Vec<Story> {
    let mut items = vec![];
    for (family, title, states, source, reference) in [
        (
            "device-name",
            "设备名称与连接状态",
            &[
                "not-started",
                "preparing",
                "direct",
                "relay",
                "connected",
                "connecting",
                "offline",
                "failed",
                "stopping",
                "stopped",
                "revoked",
                "long",
            ][..],
            "crates/zork-ui/src/device_name.rs",
            "status",
        ),
        (
            "button",
            "按钮",
            &[
                "primary",
                "secondary",
                "disabled",
                "hover",
                "focus",
                "with-icon",
                "long",
            ][..],
            "crates/zork-ui/src/controls.rs::button",
            "button",
        ),
        (
            "loading",
            "加载状态",
            &["inline", "button"][..],
            "crates/zork-ui/src/components/loading.rs",
            "loading",
        ),
        (
            "field",
            "输入框",
            &[
                "empty", "value", "focus", "secret", "error", "disabled", "readonly",
            ][..],
            "crates/zork-ui/src/controls.rs::field + components/text_input.rs",
            "field",
        ),
        (
            "choice",
            "选项",
            &["selected", "disabled"][..],
            "crates/zork-ui/src/controls.rs::choice",
            "choice",
        ),
        (
            "switch",
            "开关 Switch",
            &["off", "on", "disabled-off", "disabled-on", "focus"][..],
            "crates/zork-ui/src/controls.rs::switch",
            "switch",
        ),
        (
            "dropdown",
            "下拉菜单",
            &["closed", "open", "long-list", "empty", "disabled"][..],
            "crates/zork-ui/src/controls.rs::dropdown",
            "dropdown",
        ),
        (
            "modal",
            "弹窗",
            &["standard", "detail", "scroll", "error"][..],
            "crates/zork-ui/src/modal.rs",
            "modal",
        ),
        (
            "history",
            "执行历史",
            &["collapsed", "expanded", "narrow", "empty", "error"][..],
            "crates/zork-ui/src/components/history.rs",
            "history",
        ),
        (
            "comments",
            "文字评论",
            &["empty", "compose", "queued", "editing"][..],
            "crates/zork-ui/src/components/comments.rs",
            "comments",
        ),
        (
            "attachment",
            "附件",
            &[
                "file",
                "image",
                "loading",
                "error",
                "unavailable",
                "row",
                "message-image",
                "message-document",
            ][..],
            "crates/zork-ui/src/components/attachments.rs",
            "attachment",
        ),
        (
            "providers",
            "供应商图标",
            &["all"][..],
            "crates/zork-ui/src/controls.rs::provider_icon",
            "providers",
        ),
        (
            "profile-card",
            "模型连接卡片",
            &[
                "subscription",
                "manual",
                "offline",
                "quota-error",
                "narrow",
                "full",
                "hover-5h",
            ][..],
            "crates/zork-ui/src/components/profile_card.rs",
            "profile-card",
        ),
        (
            "icons",
            "功能图标",
            &["all"][..],
            "crates/zork-ui/src/controls.rs::icon",
            "icons",
        ),
        (
            "brand",
            "品牌组件",
            &["linked", "wordmark", "icon", "morph", "header"][..],
            "crates/zork-ui/src/components/brand.rs",
            "brand",
        ),
        (
            "navigation",
            "导航行",
            &[
                "default",
                "selected",
                "hover",
                "selected-hover",
                "gap",
                "focus",
                "selected-focus",
                "long",
            ][..],
            "crates/zork-ui/src/navigation.rs::TabGroup",
            "navigation",
        ),
        (
            "markdown",
            "消息正文",
            &["paragraph", "heading", "list", "quote", "code", "table"][..],
            "crates/zork-ui/src/components/message.rs::render_markdown",
            "markdown",
        ),
        (
            "activity",
            "会话动态",
            &[
                "session-compact",
                "session-compact-expanded",
                "session-live",
            ][..],
            "crates/zork-ui/src/components/activity.rs",
            "missing",
        ),
        (
            "feedback",
            "状态提示",
            &["notice", "success", "warning", "error", "loading"][..],
            "crates/zork-ui/src/controls.rs::feedback",
            "feedback",
        ),
    ] {
        for state in states {
            let mut story = Story::new(family, title, state, source, reference);
            if matches!(family, "button" | "field" | "navigation" | "switch") {
                story.target = match family {
                    "button" => "story-button",
                    "field" => "story-field",
                    "switch" => "story-switch",
                    _ => "story-nav",
                }
                .into();
            }
            if family == "field" && *state == "error" {
                story.target = "story-field-surface".into();
            }
            if family == "navigation" && *state == "gap" {
                story.target = "story-component".into();
            }
            if family == "brand" {
                story.target = format!("brand-{state}");
            }
            if family == "icons" {
                story.height = 560.;
            }
            if family == "history" {
                story.width = if *state == "narrow" { 320. } else { 560. };
                story.height = 600.;
            }
            if family == "profile-card" {
                story.width = if *state == "narrow" { 460. } else { 760. };
                story.height = if *state == "hover-5h" { 220. } else { 160. };
                story.target = "profile-detail-story-card".into();
            }
            if family == "activity" {
                story.height = 200.;
            }
            match (family, *state) {
                ("button", "hover") => story
                    .actions
                    .push(json!({"type":"move","target":{"element_id":"story-button"}})),
                ("button", "focus") => story.actions.push(json!({"type":"key","keystroke":"tab"})),
                ("field", "focus") => story.actions.push(click("story-field")),
                ("switch", "focus") => story.actions.push(json!({"type":"key","keystroke":"tab"})),
                ("navigation", "focus" | "selected-focus") => {
                    story.actions.push(json!({"type":"key","keystroke":"tab"}))
                }
                ("navigation", "hover" | "selected-hover") => story
                    .actions
                    .push(json!({"type":"move","target":{"element_id":"story-nav"}})),
                ("profile-card", "hover-5h") => story.actions.push(json!({
                    "type":"move","target":{"element_id":"profile-quota-window-story-card-0"}
                })),
                ("activity", "session-compact-expanded") => {
                    story.actions.push(click("session-activity-expand"))
                }
                _ => {}
            }
            items.push(story);
        }
    }
    items
}

/// Test-only fixtures: complete interaction and form walkthroughs used by
/// headless state tests. They are not listed in the design browser, where
/// each control shows its own interaction states instead.
pub fn fixtures() -> Vec<Story> {
    ["overview", "form"]
        .into_iter()
        .map(|state| {
            let mut story = Story::new(
                "interaction",
                "交互反馈",
                state,
                "crates/zork-ui/src/interaction_story.rs",
                "button",
            );
            story.width = 800.;
            story.height = if state == "form" { 760. } else { 620. };
            story
        })
        .collect()
}

pub struct PrimitiveStory {
    story: Story,
    input: Entity<ComposerInput>,
    brand: Entity<Brand>,
    open: bool,
    selected: usize,
    clicks: usize,
    selection: std::rc::Rc<std::cell::RefCell<crate::components::selection::TranscriptSelection>>,
    focus: gpui::FocusHandle,
    quote: Option<String>,
    grouped: bool,
    extra: Option<gpui::AnyView>,
    focus_pending: bool,
}
impl PrimitiveStory {
    fn id(&self, id: &str) -> String {
        if self.grouped {
            format!("{}-{id}", self.story.id)
        } else {
            id.into()
        }
    }
    pub fn grouped(story: Story, cx: &mut Context<Self>) -> Self {
        let mut view = Self::new(story, cx);
        view.grouped = true;
        view
    }

    pub fn inspect(&self, cx: &gpui::App) -> Value {
        json!({"id":self.story.id,"state":self.story.state,"brand_progress":self.brand.read(cx).morph_progress(),"selected":self.selected,"open":self.open,"clicks":self.clicks,"checked":self.selected==1,"quote":self.quote,"text":if self.story.state=="secret" { "[redacted]" } else {self.input.read(cx).value()}})
    }
    /// Hosts with their own keyboard controls focus the specimen after replaying
    /// the keyboard gesture that enables focus-visible styling.
    pub fn specimen_focus(&self) -> gpui::FocusHandle {
        self.focus.clone()
    }

    pub fn new(story: Story, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| ComposerInput::new("连接名称", cx));
        cx.observe(&input, |_, _, cx| cx.notify()).detach();
        input.update(cx, |v, cx| {
            if matches!(story.state.as_str(), "value" | "focus") {
                v.set_value("产品模型连接", cx);
            }
            if story.family == "field" && matches!(story.state.as_str(), "disabled" | "readonly") {
                v.set_value("产品模型连接", cx);
                v.set_editable(story.state == "disabled", story.state == "readonly", cx);
            }
            if story.state == "secret" {
                v.set_value("fixture-secret", cx);
                v.set_secret(true, cx);
            }
        });
        let motion = match story.state.as_str() {
            "wordmark" => BrandMotion::Wordmark,
            "icon" => BrandMotion::Icon,
            "morph" => BrandMotion::Morph,
            "header" => BrandMotion::Header,
            _ => BrandMotion::Linked,
        };
        let brand = cx.new(|_| Brand::new(motion, ZORK_UI.palette.canvas));
        let selected = usize::from(matches!(story.state.as_str(), "on" | "disabled-on"));
        let extra = if story.family == "interaction" {
            if story.state == "form" {
                Some(cx.new(crate::form_story::FormStory::new).into())
            } else {
                Some(
                    cx.new(|_| crate::interaction_story::InteractionStory::new(story.id.clone()))
                        .into(),
                )
            }
        } else if story.family == "history" {
            Some(
                cx.new(|cx| {
                    crate::components::history::HistoryStory::new(
                        story.id.clone(),
                        &story.state,
                        cx,
                    )
                })
                .into(),
            )
        } else if story.family == "attachment" {
            let text = cx
                .try_global::<crate::history_page::stories::StoryText>()
                .map(|text| text.0.clone())
                .unwrap_or_else(|| crate::resources::Text(std::rc::Rc::new(str::to_owned)));
            Some(
                cx.new(|cx| {
                    crate::attachment_viewer::stories::Story::thumbnail(&story.state, text, cx)
                })
                .into(),
            )
        } else if story.family == "comments" {
            Some(
                cx.new(|cx| {
                    crate::components::comments::CommentsStory::new(
                        story.id.clone(),
                        &story.state,
                        cx,
                    )
                })
                .into(),
            )
        } else {
            None
        };
        let focus_pending = matches!(story.family.as_str(), "modal" | "navigation");
        Self {
            open: story.state == "open" || story.family == "modal",
            story,
            input,
            brand,
            selected,
            clicks: 0,
            selection: Default::default(),
            focus: cx.focus_handle(),
            quote: None,
            grouped: false,
            extra,
            focus_pending,
        }
    }
}
impl Render for PrimitiveStory {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.focus_pending && !self.grouped {
            self.focus_pending = false;
            window.focus(&self.focus, cx);
        }
        let state = self.story.state.as_str();
        let p = ZORK_UI.palette;
        let component: gpui::AnyElement = match self.story.family.as_str() {
            "profile-card" => {
                let quota = match state {
                    "subscription" | "narrow" | "full" | "hover-5h" => Some(Quota {
                        summary: "5 小时剩余 72%，7 天剩余 38%".into(),
                        windows: vec![
                            QuotaWindow {
                                label: "5 小时".into(),
                                short_label: "5H".into(),
                                remaining: if state == "full" { 100. } else { 72. },
                                center_value: if state == "full" { "" } else { "72" }.into(),
                                value: if state == "full" {
                                    "剩余 100%"
                                } else {
                                    "剩余 72%"
                                }
                                .into(),
                                reset: Some("1 小时后重置".into()),
                            },
                            QuotaWindow {
                                label: "7 天".into(),
                                short_label: "7D".into(),
                                remaining: 38.,
                                center_value: "38".into(),
                                value: "剩余 38%".into(),
                                reset: Some("3 天后重置".into()),
                            },
                        ],
                        ..Default::default()
                    }),
                    "quota-error" => Some(Quota {
                        summary: "暂时无法获取额度".into(),
                        failed: true,
                        ..Default::default()
                    }),
                    _ => None,
                };
                let card = ProfileCard {
                    key: "story-card".into(),
                    provider: if state == "manual" { "anthropic" } else { "openai" }.into(),
                    name: if state == "manual" { "Claude API" } else { "Codex" }.into(),
                    device: Some(DeviceIdentity {
                        name: if state == "narrow" {
                            "设计工作室的 MacBook Air"
                        } else {
                            "MacBook Air"
                        }
                        .into(),
                        status: if state == "offline" {
                            crate::device_name::DeviceStatus::Offline
                        } else {
                            crate::device_name::DeviceStatus::Connected
                        },
                    }),
                    billing: if state == "manual" {
                        "Anthropic · API Key"
                    } else {
                        "OpenAI · ChatGPT 订阅"
                    }.into(),
                    model_count: if state == "manual" { "2 个模型" } else { "1 个模型" }.into(),
                    verified: state != "quota-error",
                    verification: if state == "quota-error" { "未验证" } else { "已验证" }.into(),
                    quota,
                };
                profile_card::render(card, cx.listener(|v, _, _, cx| {
                    v.clicks += 1;
                    cx.notify();
                }))
                .automation(AutomationRole::Button, "Codex 模型连接")
                .into_any_element()
            }
            "device-name" => {
                use crate::device_name::{label, DeviceStatus};
                let status = match state {
                    "not-started" => DeviceStatus::MeshNotStarted,
                    "preparing" => DeviceStatus::MeshPreparing,
                    "stopping" => DeviceStatus::MeshStopping,
                    "connected" => DeviceStatus::Connected,
                    "revoked" => DeviceStatus::Revoked,
                    "direct" | "long" => DeviceStatus::Direct,
                    "relay" => DeviceStatus::Relay,
                    "connecting" => DeviceStatus::Connecting,
                    "offline" => DeviceStatus::Offline,
                    "failed" => DeviceStatus::MeshFailed("无法恢复 Mesh 连接，请重试".into()),
                    _ => DeviceStatus::MeshStopped,
                };
                div().w(px(260.)).child(label(
                        "device-name-example",
                    if state == "long" { "设计工作室的 MacBook Air 与远程构建设备" } else { "MacBook Air" },
                    &status, None,
                )).into_any_element()
            }
            "interaction" => div().child(self.extra.clone().unwrap()).into_any_element(),
            "loading" => {
                if state == "button" {
                    ui::busy_button(self.id("loading-button"), "保存中…", true, false, true)
                        .into_any_element()
                } else {
                    crate::components::loading::status(self.id("loading-status"), "正在加载…")
                        .into_any_element()
                }
            }
            "button" => div()
                .flex()
                .child(
                    ui::button(
                        self.id("story-button"),
                        match state {
                            "secondary" => "取消",
                            "with-icon" => "",
                            "long" => "保存模型与推理设置",
                            _ => "保存",
                        },
                        state != "secondary",
                        state != "disabled",
                    )
                    .when(state == "with-icon", |v| {
                        v.pl(px(12.))
                            .child(ui::icon("icons/plus.svg", 14.).text_color(rgb(p.canvas)))
                            .child("添加连接")
                    })
                    .when(self.grouped && state == "hover", |v| {
                        v.bg(rgb(ui::BUTTON_FOCUS_BACKGROUND()))
                            .border_color(rgb(ui::BUTTON_FOCUS_BACKGROUND()))
                    })
                    .when(state == "focus", |v| {
                        v.track_focus(&self.focus)
                    })
                    .on_click(cx.listener(|v, _, _, cx| {
                        if v.story.state != "disabled" {
                            v.clicks += 1;
                            cx.notify();
                        }
                    }))
                    .automation_enabled(
                        state != "disabled",
                        AutomationRole::Button,
                        match state {
                            "secondary" => "取消",
                            "with-icon" => "添加连接",
                            "long" => "保存模型与推理设置",
                            _ => "保存",
                        },
                    ),
                )
                .into_any_element(),
            "switch" => div()
                .flex()
                .items_center()
                .gap_2()
                .child(ui::switch(
                    self.id("story-switch"),
                    "接收通知",
                    self.selected == 1,
                    !state.starts_with("disabled"),
                    &self.focus,
                    cx,
                    |v, checked, cx| {
                        v.selected = usize::from(checked);
                        cx.notify();
                    },
                ))
                .child(div().text_size(px(12.)).text_color(rgb(p.muted)).child(
                    if self.selected == 1 {
                        "已开启"
                    } else {
                        "已关闭"
                    },
                ))
                .into_any_element(),
            "field" => ui::field_with_error(
                self.id("story-field"),
                "连接名称",
                &self.input,
                (state == "error" && self.input.read(cx).value().trim().is_empty())
                    .then(|| "填写连接名称。".into()),
                cx,
            )
            .id(self.id("story-field-surface"))
            .automation(AutomationRole::Status, "输入框示例")
            .into_any_element(),
            "choice" => crate::components::widgets::controls::deferred_segmented(
                self.id("choice-active"),
                ["订阅账号", "API 接入"]
                    .into_iter()
                    .enumerate()
                    .map(
                        |(index, label)| crate::components::widgets::controls::Segment {
                            id: self.id(&format!("story-choice-{index}")),
                            label: label.into(),
                            disabled: false,
                        },
                    )
                    .collect(),
                vec![],
                Some(self.selected),
                crate::components::widgets::controls::SegmentKind::Choice,
                state != "disabled",
                ZORK_UI.palette.canvas,
                cx.listener(|v, index: &usize, _, cx| {
                    v.selected = *index;
                    cx.notify();
                }),
            )
            .into_any_element(),
            "dropdown" => ui::dropdown_with_icons(
                self.id("story-select"),
                if state == "empty" {
                    "此连接尚未添加模型".into()
                } else if state == "long-list" {
                    format!("模型连接 {}", self.selected + 1)
                } else {
                    ["OpenAI", "Anthropic", "OpenAI Compatible"][self.selected].into()
                },
                if state == "empty" {
                    vec![]
                } else if state == "long-list" {
                    (0..40).map(|i| (
                        self.id(&format!("story-option-{i}")),
                        format!("模型连接 {}", i + 1),
                        i == self.selected,
                    )).collect()
                } else {
                    ["OpenAI", "Anthropic", "OpenAI Compatible"]
                        .into_iter()
                        .enumerate()
                        .map(|(i, label)| {
                            (
                                self.id(&format!("story-option-{i}")),
                                label.into(),
                                i == self.selected,
                            )
                        })
                        .collect()
                },
                self.open,
                !matches!(state, "disabled" | "empty"),
                if matches!(state, "empty" | "long-list") {
                    None
                } else {
                    Some(ui::provider_path(
                        ["openai", "anthropic", "openai-compatible"][self.selected],
                    ))
                },
                ["openai", "anthropic", "openai-compatible"]
                    .into_iter()
                    .map(|id| Some(ui::provider_path(id)))
                    .collect(),
                window,
                cx,
                |v, open, cx| {
                    v.open = open;
                    cx.notify();
                },
                |v, i, cx| {
                    v.selected = i;
                    v.open = false;
                    cx.notify();
                },
            ),
            "history" | "comments" => div()
                .size_full()
                .when_some(self.extra.clone(), |v, e| v.child(e))
                .into_any_element(),
            "modal" => {
                if !self.open {
                    ui::button(self.id("story-modal-open"), "打开弹窗", false, true)
                        .on_click(cx.listener(|v, _, window, cx| {
                            v.open = true;
                            window.focus(&v.focus, cx);
                            cx.notify();
                        }))
                        .automation(AutomationRole::Button, "打开弹窗")
                        .into_any_element()
                } else {
                    let body = div()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .child(if state == "detail" {
                            "查看当前连接和模型配置。"
                        } else {
                            "填写信息后保存，或取消返回。"
                        })
                        .when(state != "detail", |v| {
                            v.child(ui::field(
                                self.id("story-modal-field"),
                                "名称",
                                &self.input,
                                cx,
                            ))
                        })
                        .when(state == "scroll", |v| {
                            v.children((1..=12).map(|i| {
                                div()
                                    .py_2()
                                    .border_b(gpui::px(crate::design::BORDER_WIDTH))
                                    .border_color(rgb(p.border))
                                    .child(format!("设置项目 {i}"))
                            }))
                        });
                    let footer = (state != "detail").then(|| {
                        div()
                            .flex()
                            .justify_end()
                            .gap_2()
                            .child(
                                ui::button(self.id("story-modal-cancel"), "取消", false, true)
                                    .on_click(cx.listener(|v, _, _, cx| {
                                        v.open = false;
                                        cx.notify();
                                    }))
                                    .automation(AutomationRole::Button, "取消"),
                            )
                            .child(
                                ui::button(self.id("story-modal-save"), "保存", true, true)
                                    .on_click(cx.listener(|v, _, _, cx| {
                                        v.open = false;
                                        cx.notify();
                                    }))
                                    .automation(AutomationRole::Button, "保存"),
                            )
                            .into_any_element()
                    });
                    crate::modal::modal_preview(
                        self.id("story-modal"),
                        if state == "detail" {
                            "连接详情"
                        } else {
                            "编辑设置"
                        },
                        body,
                        footer,
                        (state == "error").then(|| "暂时无法保存，请检查后重试。".into()),
                        &self.focus,
                        window,
                        cx,
                        true,
                        Some(if self.grouped {
                            360.
                        } else {
                            f32::from(window.viewport_size().height)
                        }),
                        |v, _, cx| {
                            v.open = false;
                            cx.notify();
                        },
                    )
                }
            }
            "providers" => div()
                .flex()
                .flex_wrap()
                .gap_4()
                .children(
                    [
                        "openai",
                        "anthropic",
                        "github-copilot",
                        "kimi-coding",
                        "xai",
                        "openrouter",
                        "opencode-go",
                        "openai-compatible",
                    ]
                    .into_iter()
                    .map(|id| ui::provider_icon(id, 20.)),
                )
                .into_any_element(),
            "icons" => {
                let usage: Value =
                    serde_json::from_str(include_str!("../assets/usage-v2.json")).unwrap();
                div()
                    .flex()
                    .flex_wrap()
                    .gap_3()
                    .children(
                        usage["native_icon_references"]
                            .as_object()
                            .unwrap()
                            .keys()
                            .map(|name| {
                                div()
                                    .w(px(96.))
                                    .h(px(52.))
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .items_center()
                                    .child(
                                        gpui::svg()
                                            .path(format!("icons/{name}"))
                                            .size(px(20.))
                                            .text_color(rgb(p.muted)),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(12.))
                                            .text_color(rgb(p.muted))
                                            .max_w_full()
                                            .overflow_hidden()
                                            .text_ellipsis()
                                            .whitespace_nowrap()
                                            .child(name.trim_end_matches(".svg").to_owned()),
                                    )
                            }),
                    )
                    .into_any_element()
            }
            "brand" => div().flex().child(self.brand.clone()).into_any_element(),
            "navigation" if state == "gap" => {
                let tabs = navigation::TabGroup::keyed(self.id("navigation-tabs"), window, cx);
                tabs.surface(
                    tabs.column()
                        .w(px(240.))
                        .child(
                            tabs.tab(self.id("gap-tab-first"), self.selected == 0)
                                .child("外观")
                                .on_click(cx.listener(|v, _, _, cx| {
                                    v.selected = 0;
                                    cx.notify();
                                }))
                                .automation(AutomationRole::Button, "外观"),
                        )
                        .child(div().h(px(48.)))
                        .child(
                            tabs.tab(self.id("gap-tab-second"), self.selected == 1)
                                .child("账号")
                                .on_click(cx.listener(|v, _, _, cx| {
                                    v.selected = 1;
                                    cx.notify();
                                }))
                                .automation(AutomationRole::Button, "账号"),
                        ),
                )
                .into_any_element()
            }
            "navigation" => {
                let tabs = navigation::TabGroup::keyed(self.id("navigation-tabs"), window, cx);
                tabs.surface(
                    tabs.column().w(px(240.)).child(
                        tabs.tab(self.id("story-nav"), state.starts_with("selected"))
                            .when(state.ends_with("focus"), |v| v.track_focus(&self.focus))
                            .when(
                                self.grouped && matches!(state, "hover" | "selected-hover"),
                                |v| v.bg(rgb(crate::design::INTERACTION.neutral_hover)),
                            )
                            .when(self.grouped && state.ends_with("focus"), |v| {
                                v.border_color(rgb(crate::design::INTERACTION.focus_border))
                            })
                            .child(ui::icon("icons/node.svg", 20.))
                            .child(div().flex_1().min_w_0().text_ellipsis().child(
                                if state == "long" {
                                    "产品设计与研发协作 · 本地工作设备"
                                } else {
                                    "mini1"
                                },
                            ))
                            .on_click(cx.listener(|v, _, _, cx| {
                                v.story.state = if v.story.state.starts_with("selected") {
                                    "default"
                                } else {
                                    "selected"
                                }
                                .into();
                                cx.notify();
                            }))
                            .automation(AutomationRole::Button, "mini1"),
                    ),
                )
                .into_any_element()
            }
            "markdown" if state == "table" => {
                use crate::components::selection::SelectionContext;
                let document = message::MessageDocument::parse(
                    "| 组件 | 状态 |\n| --- | --- |\n| 导航 | 已检查 |\n| 弹窗 | 待复查 |",
                );
                self.selection.borrow_mut().begin_frame();
                let weak = cx.entity().downgrade();
                let context = SelectionContext::new(
                    self.id("design-pc-table"),
                    crate::comments::CommentSource {
                        message_id: Some("design-pc-message".into()),
                        ..Default::default()
                    },
                    document.plain_text(),
                    self.selection.clone(),
                    self.focus.clone(),
                    std::rc::Rc::new(move |cx| {
                        let _ = weak.update(cx, |_, cx| cx.notify());
                    }),
                );
                message::render_selectable_document(&self.id("story-table"), &document, &context)
            }
            "markdown" => message::render_markdown(
                &self.id("story-markdown"),
                match state {
                    "heading" => "## 本周交付\n\n逐项核对组件规范。",
                    "list" => "- 对齐布局与间距\n- 检查窄窗口\n- 保留真实交互",
                    "quote" => "> 使用相同尺寸对比设计稿与原生组件。",
                    "code" => "```rust\nlet ready = check_component();\nassert!(ready);\n```",
                    "table" => {
                        "| 组件 | 状态 |\n| --- | --- |\n| 导航 | 已检查 |\n| 弹窗 | 待复查 |"
                    }
                    _ => {
                        "这是一段 **强调文字**，包含[链接](https://example.com)和 `inline code`。中文、英文与数字 123 保持清晰可读。"
                    }
                },
            ),
            "activity" => {
                let live = state == "session-live";
                let stopped = live && self.open;
                let root = cx.entity().downgrade();
                let open = root.clone();
                let preview = activity::render_session(
                    "Studio",
                    stopped,
                    stopped,
                    live && !cx.reduce_motion(),
                    self.selected == 1,
                    &[
                        activity::SessionRow {
                            id: "read-1".into(),
                            icon: "history/file-read.svg",
                            label: "读取文件".into(),
                            summary: "src/chat.rs".into(),
                            failed: false,
                            running: false,
                        },
                        activity::SessionRow {
                            id: "write-1".into(),
                            icon: "history/file-write.svg",
                            label: "写入文件".into(),
                            summary: "src/activity.rs".into(),
                            failed: false,
                            running: false,
                        },
                        if live {
                            activity::SessionRow {
                                id: "command-1".into(),
                                icon: "history/terminal.svg",
                                label: "运行命令".into(),
                                summary: "cargo test --locked".into(),
                                failed: false,
                                running: true,
                            }
                        } else {
                            activity::SessionRow {
                                id: "thinking-1".into(),
                                icon: "interface/sparkles.svg",
                                label: "正在思考".into(),
                                summary: String::new(),
                                failed: false,
                                running: true,
                            }
                        },
                    ],
                    "更多",
                    "收起",
                    if stopped { "已完成" } else { "执行中" },
                    std::rc::Rc::new(move |cx| {
                        let _ = root.update(cx, |view, cx| {
                            view.selected = 1 - view.selected;
                            cx.notify();
                        });
                    }),
                    std::rc::Rc::new(move |_, cx| {
                        let _ = open.update(cx, |view, cx| {
                            view.clicks += 1;
                            cx.notify();
                        });
                    }),
                );
                if live {
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(preview)
                        .child(div().flex().justify_start().child(
                            ui::quiet_button(
                                self.id("session-activity-simulate-end"),
                                if stopped { "重新开始" } else { "模拟完成" },
                                true,
                                ui::IconButtonSize::Compact,
                            )
                            .w(px(88.))
                            .on_click(cx.listener(|view, _, _, cx| {
                                view.open = !view.open;
                                cx.notify();
                            }))
                            .automation(
                                AutomationRole::Button,
                                if stopped { "重新开始" } else { "模拟完成" },
                            ),
                        ))
                        .into_any_element()
                } else {
                    preview.into_any_element()
                }
            }
            "attachment" => self.extra.clone().unwrap().into_any_element(),
            "feedback" => ui::status_notice(
                match state {
                    "success" => "连接已保存，可以继续配置模型。",
                    "error" => "暂时无法连接，请检查后重试。",
                    "loading" => "正在获取模型…",
                    _ => "选择连接后继续配置模型。",
                }
                .into(),
                match state {
                    "success" => ui::NoticeKind::Success,
                    "error" => ui::NoticeKind::Error,
                    "loading" => ui::NoticeKind::Loading,
                    "warning" => ui::NoticeKind::Warning,
                    _ => ui::NoticeKind::Info,
                },
            )
            .into_any_element(),
            _ => div().child("未注册的组件").into_any_element(),
        };
        div()
            .when(self.story.family == "navigation", |v| {
                v.track_focus(&self.focus)
                    .tab_stop(false)
                    .on_key_down(navigation::keyboard_navigation)
            })
            .size_full()
            .p_6()
            .on_mouse_move(cx.listener(|v, event: &gpui::MouseMoveEvent, _, cx| {
                if v.selection.borrow_mut().update(event.position) {
                    cx.notify();
                }
            }))
            .on_mouse_up(
                gpui::MouseButton::Left,
                cx.listener(|v, _, _, cx| {
                    v.quote = v.selection.borrow_mut().finish().map(|s| s.quote);
                    cx.notify();
                }),
            )
            .child(
                div()
                    .id(self.id("story-sample"))
                    .w_full()
                    .when(self.story.family == "history", |v| {
                        v.h_full().min_h_0().overflow_hidden()
                    })
                    .child(component)
                    .automation(AutomationRole::Status, self.story.title.clone()),
            )
            .into_any_element()
    }
}
