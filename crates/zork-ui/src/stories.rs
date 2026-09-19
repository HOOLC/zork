//! Interactive stories built from the public component package, without app services.
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    components::{
        activity,
        brand::{Brand, BrandMotion},
        message,
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
        }
    }
}
fn click(id: &str) -> Value {
    json!({"type":"click","target":{"element_id":id}})
}
pub fn catalog() -> Vec<Story> {
    let mut items = vec![];
    let mut liquid = Story::new(
        "liquid",
        "液态控件",
        "gallery",
        "crates/zork-ui/src/liquid_story",
        "liquid",
    );
    liquid.width = 1180.;
    liquid.height = 900.;
    items.push(liquid);
    for (family, title, states, source, reference) in [
        (
            "interaction",
            "交互反馈",
            &["overview", "form"][..],
            "crates/zork-ui/src/interaction_story.rs",
            "button",
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
            &["empty", "value", "focus", "secret", "error"][..],
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
            &["closed", "open", "empty", "disabled"][..],
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
            "avatar-picker",
            "头像选择",
            &["selected", "disabled"][..],
            "crates/zork-ui/src/controls.rs::avatar_picker",
            "avatar-picker",
        ),
        (
            "history",
            "执行历史",
            &["collapsed", "expanded", "empty", "error"][..],
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
            "avatar",
            "Agent 头像",
            &["24", "32", "40"][..],
            "crates/zork-ui/src/controls.rs::agent_avatar",
            "avatar",
        ),
        (
            "providers",
            "供应商图标",
            &["all"][..],
            "crates/zork-ui/src/controls.rs::provider_icon",
            "providers",
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
                "fold-open",
                "fold-closed",
            ][..],
            "crates/zork-ui/src/navigation.rs::TabGroup",
            "navigation",
        ),
        (
            "tooltip",
            "悬停详情",
            &["leader", "task", "hover"][..],
            "crates/zork-ui/src/components/tooltip.rs",
            "tooltip",
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
            "执行状态",
            &["running", "failed", "done"][..],
            "crates/zork-ui/src/components/activity.rs::render",
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
            if family == "brand" {
                story.target = format!("brand-{state}");
            }
            if family == "interaction" {
                story.width = 800.;
                story.height = if *state == "form" { 760. } else { 620. };
            }
            if family == "icons" {
                story.height = 560.;
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
                _ => {}
            }
            items.push(story);
        }
    }
    items
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
        if let Some(extra) = &self.extra {
            if let Ok(view) = extra.clone().downcast::<crate::liquid_story::Gallery>() {
                return view.read(cx).inspect(cx);
            }
        }
        json!({"id":self.story.id,"state":self.story.state,"brand_progress":self.brand.read(cx).morph_progress(),"selected":self.selected,"open":self.open,"clicks":self.clicks,"checked":self.selected==1,"quote":self.quote,"text":if self.story.state=="secret" { "[redacted]" } else {self.input.read(cx).value()}})
    }

    pub fn new(story: Story, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| ComposerInput::new("连接名称", cx));
        cx.observe(&input, |_, _, cx| cx.notify()).detach();
        input.update(cx, |v, cx| {
            if matches!(story.state.as_str(), "value" | "focus") {
                v.set_value("产品模型连接", cx);
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
        let selected = if story.family == "avatar-picker" {
            3
        } else {
            usize::from(matches!(story.state.as_str(), "on" | "disabled-on"))
        };
        let extra = if story.family == "liquid" {
            Some(cx.new(crate::liquid_story::Gallery::new).into())
        } else if story.family == "interaction" {
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
        if self.story.family == "liquid" {
            return div()
                .id(self.id("story-sample"))
                .size_full()
                .child(self.extra.clone().unwrap())
                .into_any_element();
        }
        if self.focus_pending && !self.grouped {
            self.focus_pending = false;
            window.focus(&self.focus, cx);
        }
        let state = self.story.state.as_str();
        let p = ZORK_UI.palette;
        let component: gpui::AnyElement = match self.story.family.as_str() {
            "interaction" => div().child(self.extra.clone().unwrap()).into_any_element(),
            "loading" => if state == "button" {
                ui::busy_button(self.id("loading-button"), "保存中…", true, false, true).into_any_element()
            } else {
                crate::components::loading::status(self.id("loading-status"), "正在加载…").into_any_element()
            },
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
                        v.pl(px(12.)).child(ui::icon("icons/plus.svg", 14.).text_color(rgb(p.canvas)))
                            .child("添加连接")
                    })
                    .when(self.grouped && state == "hover", |v| {
                        v.bg(rgb(ui::BUTTON_FOCUS_BACKGROUND))
                            .border_color(rgb(ui::BUTTON_FOCUS_BACKGROUND))
                    })
                    .when(self.grouped && state == "focus", |v| {
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
            "choice" => crate::components::liquid::controls::deferred_segmented(
                self.id("choice-active"),
                ["订阅账号", "API 接入"].into_iter().enumerate().map(|(index, label)| crate::components::liquid::controls::Segment {
                    id: self.id(&format!("story-choice-{index}")), label: label.into(), disabled: false,
                }).collect(),
                vec![], Some(self.selected), crate::components::liquid::controls::SegmentKind::Choice,
                state != "disabled", ZORK_UI.palette.canvas,
                cx.listener(|v, index: &usize, _, cx| { v.selected = *index; cx.notify(); }),
            ).into_any_element(),
            "dropdown" => ui::dropdown_with_icons(
                self.id("story-select"),
                if state == "empty" {
                    "此连接尚未添加模型".into()
                } else {
                    ["OpenAI", "Anthropic", "OpenAI Compatible"][self.selected].into()
                },
                if state == "empty" {
                    vec![]
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
                if state == "empty" {
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
            "avatar-picker" => ui::avatar_picker(
                self.id("story-avatar"),
                ui::AGENT_AVATARS[self.selected].0,
                state != "disabled",
                cx,
                |v, key, cx| {
                    v.selected = ui::AGENT_AVATARS
                        .iter()
                        .position(|a| a.0 == key)
                        .unwrap_or(0);
                    cx.notify();
                },
            )
            .into_any_element(),
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
            "tooltip" => {
                use crate::components::tooltip::DetailsTooltip;
                let leader = DetailsTooltip {
                    key: self.id("leader-detail"),
                    title: "产品 Leader".into(),
                    kind: "Leader".into(),
                    avatar: Some("fox".into()),
                    description: "梳理产品需求，协调任务并检查交付结果。".into(),
                    rows: vec![
                        ("设备".into(), "mini1".into()),
                        ("模型".into(), "fixture-model".into()),
                    ],
                };
                let task = DetailsTooltip {
                    key: self.id("task-detail"),
                    title: "完善导航交互".into(),
                    kind: "Task".into(),
                    avatar: None,
                    description: "统一导航层级与交互反馈，并验证窄窗口下的显示。".into(),
                    rows: vec![
                        ("Leader".into(), "产品 Leader".into()),
                        ("状态".into(), "进行中".into()),
                        ("执行设备".into(), "mini1".into()),
                    ],
                };
                if state == "hover" {
                    let tabs = navigation::TabGroup::keyed(self.id("navigation-tabs"), window, cx);
                    let overlay = window.use_keyed_state(self.id("details-overlay"), cx, |_, _| crate::components::tooltip::DetailsOverlay::default());
                    tabs.surface(tabs.column()
                        .w(px(240.))
                        .child(
                            tabs.tab(self.id("tooltip-leader-trigger"), false)
                                .child(ui::agent_avatar(Some("fox"), 20.))
                                .child("产品 Leader")
                                .automation(AutomationRole::Button, "产品 Leader")
                                .map(|row| crate::components::tooltip::trigger(row, leader, overlay.clone())),
                        )
                        .child(
                            tabs.tab(self.id("tooltip-task-trigger"), false)
                                .child(ui::icon("icons/checklist.svg", 16.))
                                .child("完善导航交互")
                                .automation(AutomationRole::Button, "完善导航交互")
                                .map(|row| crate::components::tooltip::trigger(row, task, overlay.clone())),
                        )
                        .child(overlay)
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(rgb(p.muted))
                                .child("悬停查看详情，移开后关闭"),
                        )
)
                        .into_any_element()
                } else {
                    if state == "leader" {
                        leader.card()
                    } else {
                        task.card()
                    }
                    .into_any_element()
                }
            }
            "avatar" => {
                div()
                    .max_w(px(state.parse::<f32>().unwrap_or(32.) * 6. + 80.))
                    .flex()
                    .flex_wrap()
                    .gap_4()
                    .children(ui::AGENT_AVATARS.into_iter().map(|(key, _, _)| {
                        ui::agent_avatar(Some(key), state.parse().unwrap_or(26.))
                    }))
                    .into_any_element()
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
                                    .w(px(68.))
                                    .h(px(44.))
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
                                            .text_size(px(9.))
                                            .text_color(rgb(p.muted))
                                            .child(name.trim_end_matches(".svg").to_owned()),
                                    )
                            }),
                    )
                    .into_any_element()
            }
            "brand" => div().flex().child(self.brand.clone()).into_any_element(),
            "navigation" if state.starts_with("fold-") => {
                let tabs = navigation::TabGroup::keyed(self.id("navigation-tabs"), window, cx);
                let fold = crate::components::collapse::Collapse::new(
                    self.id("navigation-fold"), state == "fold-open", 240., window, cx,
                );
                let focus = fold.header_focus(cx);
                let interactive = fold.interactive(cx);
                let body = fold.mounted(cx).then(|| tabs.column().pt(px(2.))
                    .children((0..4).map(|i| tabs.tab(self.id(&format!("fold-task-{i}")), i == 1)
                        .tab_stop(interactive).pl(px(30.)).child(format!("Task {}", i + 1))
                        .automation(AutomationRole::Button, format!("Task {}", i + 1))))
                    .into_any_element());
                let owner = cx.entity().downgrade();
                tabs.surface(div().w(px(240.)).flex().flex_col()
                    .child(tabs.tab(self.id("fold-header"), false).track_focus(&focus)
                        .child(ui::icon("icons/node.svg", 20.)).child("mini1")
                        .on_click(cx.listener(|v, _, _, cx| {
                            v.story.state = if v.story.state == "fold-open" { "fold-closed" } else { "fold-open" }.into();
                            cx.notify();
                        })).automation(AutomationRole::Button, "mini1"))
                    .child(fold.element(body, move |_, cx| {
                        let _ = owner.update(cx, |_, cx| cx.notify());
                    }, cx))
                    .child(tabs.tab(self.id("fold-following"), false)
                        .child(ui::icon("icons/node.svg", 20.)).child("mini2")
                        .automation(AutomationRole::Button, "mini2"))
                ).into_any_element()
            }
            "navigation" if state == "gap" => {
                let tabs = navigation::TabGroup::keyed(self.id("navigation-tabs"), window, cx);
                tabs.surface(tabs.column().w(px(240.))
                    .child(tabs.tab(self.id("gap-tab-first"), self.selected == 0)
                        .child("外观")
                        .on_click(cx.listener(|v, _, _, cx| { v.selected = 0; cx.notify(); }))
                        .automation(AutomationRole::Button, "外观"))
                    .child(div().h(px(48.)))
                    .child(tabs.tab(self.id("gap-tab-second"), self.selected == 1)
                        .child("账号")
                        .on_click(cx.listener(|v, _, _, cx| { v.selected = 1; cx.notify(); }))
                        .automation(AutomationRole::Button, "账号")))
                    .into_any_element()
            }
            "navigation" => {
                let tabs = navigation::TabGroup::keyed(self.id("navigation-tabs"), window, cx);
                tabs.surface(tabs.column()
                .w(px(240.))
                .child(
                    tabs.tab(self.id("story-nav"), state.starts_with("selected"))
                        .when(self.grouped && matches!(state, "hover" | "selected-hover"), |v| {
                            v.bg(rgb(crate::design::INTERACTION.neutral_hover))
                        })
                        .when(self.grouped && state.ends_with("focus"), |v| v.border_color(rgb(crate::design::INTERACTION.focus_border)))
                        .child(ui::icon("icons/node.svg", 20.))
                        .child(div().flex_1().min_w_0().text_ellipsis().child(if state == "long" { "产品设计与研发协作 · 本地工作设备" } else { "mini1" }))
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
                )
)
                .into_any_element()
            },
            "markdown" if state == "table" => {
                use crate::components::selection::SelectionContext;
                let document = message::MessageDocument::parse(
                    "| 组件 | 状态 |\n| --- | --- |\n| 导航 | 已检查 |\n| 弹窗 | 待复查 |",
                );
                self.selection.borrow_mut().begin_frame();
                let weak = cx.entity().downgrade();
                let context = SelectionContext::new(
                    self.id("storybook-table"),
                    crate::comments::CommentSource {
                        message_id: Some("storybook-message".into()),
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
            "activity" => activity::render_with_id(
                self.id("participant-activity"),
                &[activity::Presentation {
                    id: self.id("story"),
                    name: "产品 Leader".into(),
                    avatar: Some("fox".into()),
                    label: match state {
                        "failed" => "执行失败",
                        "done" => "已完成",
                        _ => "正在检查组件",
                    }
                    .into(),
                    failed: state == "failed",
                    running: state == "running",
                }],
                false,
            )
            .into_any_element(),
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

/// One component tab, with all of its states rendered by independent entities.
pub struct FamilyStories {
    family: String,
    items: Vec<(Story, Entity<PrimitiveStory>)>,
    pending_focus: Option<gpui::FocusHandle>,
}
impl FamilyStories {
    pub fn new(family: String, cx: &mut Context<Self>) -> Self {
        let mut pending_focus = None;
        let items = catalog()
            .into_iter()
            .filter(|s| s.family == family)
            .map(|story| {
                let child = cx.new(|cx| PrimitiveStory::grouped(story.clone(), cx));
                if story.state == "focus" {
                    pending_focus = Some(if story.family == "field" {
                        child.read(cx).input.read(cx).focus_handle()
                    } else {
                        child.read(cx).focus.clone()
                    });
                }
                (story, child)
            })
            .collect();
        Self {
            family,
            items,
            pending_focus,
        }
    }
    pub fn inspect(&self, cx: &gpui::App) -> Value {
        json!({"id":format!("family-{}", self.family),"family":self.family,
            "states":self.items.iter().map(|(_,view)|view.read(cx).inspect(cx)).collect::<Vec<_>>()})
    }
}
impl Render for FamilyStories {
    fn render(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        if self.family == "liquid" {
            return div()
                .size_full()
                .children(self.items.first().map(|(_, child)| child.clone()))
                .into_any_element();
        }
        if let Some(focus) = self.pending_focus.take() {
            window.on_next_frame(move |window, cx| {
                window.blur();
                let key = gpui::Keystroke::parse("tab").expect("tab key");
                window.dispatch_keystroke(key.clone(), cx);
                window.dispatch_event(
                    gpui::PlatformInput::KeyUp(gpui::KeyUpEvent { keystroke: key }),
                    cx,
                );
                window.focus(&focus, cx);
            });
        }
        let p = ZORK_UI.palette;
        let width = f32::from(window.viewport_size().width);
        let columns = if width >= 1000. && self.family != "icons" {
            2.
        } else {
            1.
        };
        let card_width = ((width - 48. - (columns - 1.) * 16.) / columns).max(200.);
        let height = match self.family.as_str() {
            "button" | "navigation" | "switch" => 96.,
            "field" => 132.,
            "modal" => 400.,
            "history" => 380.,
            "tooltip" => 280.,
            "comments" => 380.,
            "attachment" => 220.,
            "avatar-picker" => 160.,
            "dropdown" => 246.,
            "icons" => 580.,
            "markdown" => 232.,
            "avatar" | "brand" => 140.,
            _ => 112.,
        };
        div()
            .id(format!("family-{}", self.family))
            .size_full()
            .overflow_y_scroll()
            .font_family("Inter Variable")
            .text_size(px(13.))
            .text_color(rgb(p.text))
            .bg(rgb(p.canvas))
            .p_6()
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_start()
                    .gap_4()
                    .children(self.items.iter().map(|(story, child)| {
                        let height = if story.state.starts_with("fold-") {
                            300.
                        } else {
                            height
                        };
                        let label = match story.state.as_str() {
                            "compose" => "添加评论",
                            "queued" => "待发送",
                            "editing" => "编辑",
                            "file" => "文件",
                            "image" => "图片",
                            "loading" => "加载中",
                            "success" => "成功",
                            "unavailable" => "离线",
                            "standard" => "标准弹窗",
                            "detail" => "详情",
                            "scroll" => "内容滚动",
                            "error" => "错误",
                            "collapsed" => "收起",
                            "expanded" => "展开",
                            "primary" => "主按钮",
                            "secondary" => "次按钮",
                            "with-icon" => "带图标",
                            "leader" => "Leader 详情",
                            "task" => "Task 详情",
                            "long" => "长文字",
                            "disabled" => "禁用",
                            "hover" => "悬停",
                            "focus" => "键盘焦点",
                            "empty" => "空状态",
                            "off" => "关闭",
                            "on" => "开启",
                            "disabled-off" => "禁用 · 关闭",
                            "disabled-on" => "禁用 · 开启",
                            "value" => "已输入",
                            "secret" => "密码",
                            "selected" => "选中",
                            "selected-hover" => "选中 · 悬停",
                            "gap" => "跨分组滑动",
                            "selected-focus" => "选中 · 键盘焦点",
                            "closed" => "收起",
                            "open" => "展开",
                            "default" => "默认",
                            "paragraph" => "段落",
                            "heading" => "标题",
                            "list" => "列表",
                            "quote" => "引用",
                            "code" => "代码",
                            "table" => "表格",
                            "running" => "进行中",
                            "failed" => "失败",
                            "done" => "完成",
                            "all" => "全部",
                            "notice" => "提示",
                            "linked" => "组合标志",
                            "wordmark" => "字标",
                            "icon" => "图标",
                            "morph" => "标志动效",
                            other => other,
                        };
                        div()
                            .id(format!("state-{}", story.id))
                            .w(px(card_width))
                            .flex_shrink_0()
                            .rounded(px(ui::CARD_RADIUS))
                            .border(gpui::px(crate::design::BORDER_WIDTH))
                            .border_color(rgb(p.border))
                            .overflow_hidden()
                            .child(
                                div()
                                    .px_6()
                                    .pt_4()
                                    .text_size(px(12.))
                                    .text_color(rgb(p.muted))
                                    .child(label.to_owned()),
                            )
                            .child(div().h(px(height)).w_full().child(child.clone()))
                            .automation(
                                AutomationRole::Status,
                                format!("{} · {label}", story.title),
                            )
                    })),
            )
            .automation(AutomationRole::Status, "组件的全部状态")
            .into_any_element()
    }
}
