//! Business specimens mount the production component supplied by the host.
//! This module owns catalog browsing and mock inputs, never an alternate view.
use crate::{
    automation::{driver::headless_action, element::AutomationRegistryGlobal},
    components::workbench as wb,
    stories::Story,
};
use gpui::{prelude::*, *};
use std::{collections::VecDeque, rc::Rc};

type Factory = Rc<dyn Fn(Story, &mut App) -> AnyView>;
type Inspector = Rc<dyn Fn(&AnyView, &App) -> serde_json::Value>;

#[derive(Clone)]
pub struct Catalog {
    pub entries: Vec<Story>,
    create: Factory,
    inspector: Inspector,
}
impl Global for Catalog {}

/// The application adapter constructs the same production views with mock
/// sources. Native and Web install their shared production story factory here.
pub fn install(
    entries: Vec<Story>,
    create: impl Fn(Story, &mut App) -> AnyView + 'static,
    inspector: impl Fn(&AnyView, &App) -> serde_json::Value + 'static,
    cx: &mut App,
) {
    let entries = entries
        .into_iter()
        .filter(|story| is_business(&story.family))
        .filter(|story| !story.state.ends_with("-wide"))
        .collect();
    cx.set_global(Catalog {
        entries,
        create: Rc::new(create),
        inspector: Rc::new(inspector),
    });
}

pub fn is_business(family: &str) -> bool {
    matches!(
        family,
        "device-name"
            | "welcome"
            | "node-directory"
            | "connection"
            | "model"
            | "agent"
            | "client"
            | "device"
            | "mesh"
            | "enrollment"
            | "message-interaction"
            | "conversation"
            | "history-details"
            | "history"
            | "comments"
            | "attachment"
            | "tooltip"
            | "markdown"
            | "activity"
            | "composer"
            | "resources"
            | "shared-files"
            | "notifications"
            | "appearance"
            | "data-settings"
            | "browser"
            | "attachment-viewer"
            | "chat-navigation"
            | "message-reader"
            | "conversation-files"
            | "member-activity"
    )
}

pub(super) fn catalog(cx: &App) -> Catalog {
    cx.try_global::<Catalog>()
        .cloned()
        .unwrap_or_else(|| Catalog {
            entries: crate::stories::catalog()
                .into_iter()
                .filter(|story| is_business(&story.family))
                .collect(),
            inspector: Rc::new(|view, cx| {
                view.clone()
                    .downcast::<crate::stories::PrimitiveStory>()
                    .map(|view| view.read(cx).inspect(cx))
                    .unwrap_or_default()
            }),
            create: Rc::new(|story, cx| {
                cx.new(|cx| crate::stories::PrimitiveStory::new(story, cx))
                    .into()
            }),
        })
}

pub(super) fn families(cx: &App) -> Vec<(String, String)> {
    let mut result = Vec::new();
    for story in catalog(cx).entries {
        if !result.iter().any(|(key, _)| *key == story.family) {
            result.push((story.family, story.title));
        }
    }
    result
}

pub(super) struct Example {
    cases: Vec<Story>,
    selected: usize,
    view: AnyView,
    create: Factory,
    inspector: Inspector,
    pending: VecDeque<serde_json::Value>,
    scheduled: bool,
    generation: u64,
    error: Option<String>,
    choices_open: bool,
    active: bool,
    attempts: u16,
}
impl Example {
    pub(super) fn new(family: &str, cx: &mut Context<Self>) -> Self {
        let catalog = catalog(cx);
        let cases: Vec<_> = catalog
            .entries
            .into_iter()
            .filter(|story| story.family == family)
            .collect();
        let first = cases
            .first()
            .expect("a business family has at least one registered case")
            .clone();
        let view = (catalog.create)(first.clone(), cx);
        Self {
            cases,
            selected: 0,
            view,
            create: catalog.create,
            inspector: catalog.inspector,
            pending: first.actions.into(),
            scheduled: false,
            generation: 0,
            error: None,
            choices_open: false,
            active: true,
            attempts: 0,
        }
    }
    fn choose(&mut self, index: usize, cx: &mut Context<Self>) {
        if index == self.selected {
            return;
        }
        self.selected = index;
        let story = self.cases[index].clone();
        self.view = (self.create)(story.clone(), cx);
        self.pending = story.actions.into();
        self.generation += 1;
        self.scheduled = false;
        self.error = None;
        self.attempts = 0;
        cx.notify();
    }
    pub(super) fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        if self.active != active {
            self.active = active;
            if active {
                cx.notify();
            }
        }
    }
    pub(super) fn inspect(&self, cx: &App) -> serde_json::Value {
        serde_json::json!({"story":self.cases[self.selected].id,"pending":self.pending.len(),"error":self.error,"value":(self.inspector)(&self.view, cx)})
    }
}
impl Render for Example {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.active && !self.scheduled && !self.pending.is_empty() {
            self.scheduled = true;
            let owner = cx.entity().downgrade();
            let generation = self.generation;
            window.on_next_frame(move |window, cx| {
                let action = owner
                    .update(cx, |v, _| {
                        v.scheduled = false;
                        (v.active && v.generation == generation)
                            .then(|| v.pending.front().cloned())
                            .flatten()
                    })
                    .ok()
                    .flatten();
                let Some(action) = action else {
                    return;
                };
                let registry = cx
                    .try_global::<AutomationRegistryGlobal>()
                    .map(|global| global.0.clone());
                let ready =
                    registry.as_ref().is_some_and(|registry| {
                        let snapshot = registry.snapshot(false);
                        if snapshot.elements.iter().any(|e| {
                            e.id == "liquid-library-dialog" || e.id == "business-state-menu"
                        }) {
                            return false;
                        }
                        action["target"]["element_id"].as_str().is_none_or(|id| {
                            snapshot.elements.iter().any(|e| {
                                e.id == id
                                    && e.enabled
                                    && e.visible
                                    && e.visible_bounds.height >= e.bounds.height - 0.5
                            })
                        })
                    });
                let result = if ready {
                    Some(
                        serde_json::from_value(action)
                            .map_err(|error| error.to_string())
                            .and_then(|action| {
                                headless_action(action, window, cx, registry.as_ref().unwrap())
                                    .map(|_| ())
                                    .map_err(|error| error.message)
                            }),
                    )
                } else {
                    None
                };
                let _ = owner.update(cx, |v, cx| {
                    if !v.active || v.generation != generation {
                        return;
                    }
                    match result {
                        Some(Ok(())) => {
                            v.pending.pop_front();
                            v.attempts = 0;
                        }
                        Some(Err(error)) => {
                            v.error = Some(error);
                            v.pending.clear();
                        }
                        None => {
                            v.attempts += 1;
                            if v.attempts >= 240 {
                                v.error = Some("示例操作等待控件就绪超时".into());
                                v.pending.clear();
                            }
                        }
                    }
                    cx.notify();
                });
            });
        }
        let variants = crate::controls::dropdown(
            "business-state",
            state_label(&self.cases[self.selected].state),
            self.cases
                .iter()
                .enumerate()
                .map(|(index, story)| {
                    (
                        format!("business-state-{}", story.id),
                        state_label(&story.state),
                        index == self.selected,
                    )
                })
                .collect(),
            self.choices_open,
            true,
            window,
            cx,
            |v, open, cx| {
                v.choices_open = open;
                cx.notify();
            },
            |v, index, cx| {
                v.choices_open = false;
                v.choose(index, cx);
            },
        );
        wb::column(16.)
            .w_full()
            .child(variants)
            .when_some(self.error.clone(), |body, error| {
                body.child(crate::controls::status_notice(
                    error,
                    crate::controls::NoticeKind::Error,
                ))
            })
            .child(
                wb::slot(
                    self.cases[self.selected]
                        .width
                        .min((window.viewport_size().width.as_f32() - 48.).max(2.)),
                )
                .max_w_full()
                .h(px(self.cases[self.selected].height))
                .child(self.view.clone()),
            )
    }
}

pub fn state_label(state: &str) -> String {
    let state = state.trim_end_matches("-compact");
    match state {
        "primary" => "主要操作",
        "secondary" => "次要操作",
        "with-icon" => "带图标",
        "focus" => "键盘焦点",
        "value" => "已输入",
        "secret" => "密码",
        "off" => "关闭",
        "on" => "开启",
        "disabled-off" => "禁用 · 关闭",
        "disabled-on" => "禁用 · 开启",
        "selected" => "选中",
        "selected-focus" => "选中 · 键盘焦点",
        "selected-hover" => "选中 · 悬停",
        "gap" => "跨分组滑动",
        "fold-open" => "分组展开",
        "fold-closed" => "分组收起",
        "closed" => "收起",
        "open" => "展开",
        "default" => "默认",
        "standard" => "标准",
        "scroll" => "内容滚动",
        "success" => "成功",
        "warning" => "警告",
        "notice" => "提示",
        "overview" => "交互总览",
        "form" => "表单",
        "inline" => "行内",
        "button" => "按钮",
        "gallery" => "交互展台",
        "all" => "全部",
        "wordmark" => "字标",
        "icon" => "图标",
        "morph" => "形变",
        "header" => "页头",
        "linked" => "联动",
        "first-agent" => "首次使用",
        "choose-agent" => "选择领队",
        "background" => "后台运行",
        "pairing" => "配对表单",
        "toolbar" => "成员与文件菜单",
        "tabs" => "多个页面",
        "agent" => "成员详情",
        "entry" => "执行记录",
        "idle" => "空闲",
        "active" => "工作中",
        "automatic" => "自动",
        "minimum" => "最小高度",
        "maximum" => "最大高度",
        "custom" => "自定义高度",
        "applications" => "应用列表",
        "interactive" => "交互示例",
        "document" => "文档",
        "literal" => "原文",
        "markdown" => "格式化正文",
        "files" => "文件列表",
        "pages" => "页面列表",
        "menu" => "菜单",
        "grid" => "网格",
        "preview" => "预览",
        "unread" => "未读",
        "offline" => "离线",
        "creator" => "创建者",
        "enabled" => "已启用",
        "disabled" => "已关闭",
        "muted" => "已静音",
        "denied" => "未获授权",
        "busy" => "处理中",
        "services" => "服务",
        "skills" => "技能",
        "list" => "列表",
        "create" => "新建",
        "edit" | "editing" => "编辑",
        "detail" => "详情",
        "provider" => "供应商",
        "protocol" => "接口类型",
        "dropdown" => "选择模型",
        "signed-out" => "未登录",
        "signed-in" => "已登录",
        "not-started" => "未启动",
        "preparing" => "准备中",
        "direct" => "直连",
        "relay" => "中继",
        "connecting" => "连接中",
        "stopping" => "停止中",
        "revoked" => "访问已撤销",
        "loading" => "加载中",
        "error" => "错误",
        "running" => "运行中",
        "stopped" => "已停止",
        "connected" => "已连接",
        "empty" => "空状态",
        "manual" => "手动添加",
        "start" => "开始",
        "command" => "连接命令",
        "expired" => "已过期",
        "collapsed" => "收拢",
        "expanded" => "展开",
        "compose" => "添加评论",
        "queued" => "待发队列",
        "file" => "文件",
        "image" => "图片",
        "unavailable" => "不可用",
        "row" => "列表行",
        "message-image" => "消息图片",
        "message-document" => "消息文档",
        "leader" => "领队",
        "task" => "任务",
        "hover" => "悬停",
        "paragraph" => "段落",
        "heading" => "标题",
        "quote" => "引用",
        "code" => "代码",
        "table" => "表格",
        "failed" => "失败",
        "done" => "完成",
        "approval" => "等待批准",
        "approved" => "已批准",
        "declined" => "已拒绝",
        "cancelled" => "已取消",
        "input" => "填写",
        "prefilled" => "已填写",
        "update" => "修改",
        "login" => "登录",
        "login-device" => "设备登录",
        "login-callback" => "登录回调",
        "login-completed" => "登录完成",
        "completed" => "已完成",
        "long" => "长内容",
        "messages" => "消息",
        "composer" => "消息输入",
        "history" => "执行历史",
        _ => state,
    }
    .to_owned()
}
