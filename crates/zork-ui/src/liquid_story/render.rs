use super::*;
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    components::workbench as wb,
    components::{
        liquid::{
            controls::{self, ActionStyle},
            SurfaceColors,
        },
        message,
        selection::SelectionContext,
    },
    controls as ui,
};

const CANVAS: u32 = wb::CANVAS;
const WHITE: u32 = crate::design::ZORK_UI.palette.canvas;
const LINE: u32 = crate::design::LIQUID_OUTLINE;

fn positioned(x: f64, y: f64, w: f64, h: f64) -> Div {
    div()
        .absolute()
        .left(px(x as f32))
        .top(px(y as f32))
        .w(px(w.max(0.) as f32))
        .h(px(h.max(0.) as f32))
}
fn muted(text: impl Into<SharedString>) -> Div {
    ui::text_role(text, crate::design::TextRole::Metadata)
}
fn caption(text: impl Into<SharedString>) -> Div {
    ui::text_role(text, crate::design::TextRole::Label)
        .text_color(rgb(crate::design::ZORK_UI.palette.text))
}

impl Card {
    fn button(
        &self,
        key: &str,
        label: impl Into<SharedString>,
        width: f32,
        style: ActionStyle,
        parent: u32,
        window: &mut Window,
        cx: &mut App,
    ) -> Stateful<Div> {
        let in_surface = self.kind == Kind::Composer && !key.starts_with("edit-");
        let mut style = style;
        if (in_surface || self.kind == Kind::Modal)
            && matches!(self.kind, Kind::Attachments | Kind::Comments | Kind::Modal)
            && !self.open
        {
            style.disabled = true;
        }
        let mut label = label.into();
        let mut width = width;
        if style.icon.is_none() && matches!(label.as_ref(), "×" | "⌃") {
            style.icon = Some("icons/x.svg");
            style.quiet = true;
            label = "".into();
            width = 24.;
        }
        let height = if style.icon.is_some() && width <= 32. {
            width
        } else {
            32.
        };
        let control = controls::action(
            self.sid(key),
            label,
            width,
            height,
            style,
            parent,
            window,
            cx,
        );
        if in_surface {
            self.surfaces[0].guard(control)
        } else {
            control
        }
    }
    fn variant_labels(&self) -> &'static [&'static str] {
        match self.kind {
            Kind::Actions => &["默认", "忙碌", "禁用"],
            Kind::Fields => &["普通", "错误", "禁用"],
            Kind::Choices => &["分段", "单选", "头像"],
            Kind::Navigation => &["纵向", "横向"],
            Kind::Rows => &["列表行", "卡片"],
            Kind::Popover => &["单选", "成员多选", "操作菜单"],
            Kind::Details => &["详情", "文字提示"],
            Kind::Disclosure => &["执行历史", "导航分组"],
            Kind::Attachments => &["收拢", "展开", "预览"],
            Kind::Composer => &[
                "空白",
                "可发送",
                "运行中",
                "停止中",
                "只读",
                "附件准备中",
                "多人活动",
                "任务评论",
            ],
            Kind::Modal => &["表单", "只读详情"],
            _ => &[],
        }
    }
    fn set_variant(&mut self, variant: usize, cx: &mut Context<Self>) {
        self.variant_menu = false;
        self.variant = variant;
        if self.kind == Kind::Composer {
            self.composer_intent(zork_client_types::composer::Intent::Scenario(variant), cx);
            return;
        }
        if self.kind == Kind::Attachments {
            self.preview = variant == 2;
            self.open = variant > 0;
            if !self.open {
                self.pending = None;
                self.busy = false;
            }
        }
        self.retarget();
        cx.notify();
    }
    fn stage_height(&self) -> f32 {
        let body = match self.kind {
            Kind::Popover => self.popover.pose().unwrap_or_else(|| self.poses().0),
            Kind::Modal => self.dialog.pose().unwrap_or_else(|| self.poses().0),
            _ => self.surfaces[0].simulation.pose(),
        };
        (body.top() + body.h) as f32 + wb::PREVIEW_PADDING
    }
    fn field(
        &self,
        key: &str,
        input: &Entity<ComposerInput>,
        label: &'static str,
        w: f32,
        invalid: bool,
        parent: u32,
        window: &mut Window,
        cx: &mut App,
    ) -> Div {
        if (self.kind == Kind::Fields && self.variant == 2)
            || (self.kind == Kind::Modal && !self.open)
        {
            return div()
                .w(px(w))
                .flex()
                .flex_col()
                .gap_1()
                .child(muted(label))
                .child(
                    liquid::skin(
                        self.sid(key),
                        w,
                        32.,
                        12.,
                        0.6,
                        SurfaceColors::outlined(LINE, parent),
                        div().px_3().h_full().flex().items_center().child(muted(
                            if key == "secret" || key == "credential" {
                                "••••••••".to_owned()
                            } else {
                                input.read(cx).value().to_owned()
                            },
                        )),
                        window,
                        cx,
                    )
                    .automation_enabled(
                        false,
                        AutomationRole::TextInput,
                        label,
                    ),
                );
        }
        div()
            .w(px(w))
            .flex()
            .flex_col()
            .gap_1()
            .child(muted(label))
            .child(
                {
                    let control =
                        controls::input(self.sid(key), input, w, 32., invalid, parent, window, cx);
                    control
                }
                .automation(AutomationRole::TextInput, label),
            )
    }
    fn popover_label(&self) -> &'static str {
        if self.variant == 1 {
            "@ 选择成员"
        } else {
            "选择供应商"
        }
    }
    fn measure_popover(&mut self, window: &mut Window) {
        if self.kind != Kind::Popover || self.variant == 2 {
            return;
        }
        let label = self.popover_label();
        let style = window.text_style();
        let key = (label, style.font());
        if self.popover_measure_key.as_ref() == Some(&key) {
            return;
        }
        self.popover_label_width = window
            .text_system()
            .shape_line(
                label.into(),
                px(13.),
                &[TextRun {
                    len: label.len(),
                    font: key.1.clone(),
                    color: style.color,
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                }],
                None,
            )
            .width
            .as_f32();
        self.popover_measure_key = Some(key);
        self.retarget();
        // The first painted frame already has its measured width. Later label
        // changes retain the current position and velocity of the material.
        if self.frames.is_empty() {
            for surface in &mut self.surfaces {
                surface.simulation.finish();
                surface.prepare();
            }
        }
    }
    pub(super) fn measure_input(&mut self, window: &mut Window, cx: &App) {
        if self.kind != Kind::Composer {
            return;
        }
        let text = self.input.read(cx).value().to_owned();
        let width = (self.width - 36. - 2. * liquid::composer::TEXT_INSET).max(100.);
        let key = (text.clone(), width.to_bits());
        if self.measure_key.as_ref() == Some(&key) {
            return;
        }
        self.measure_key = Some(key);
        self.composer
            .apply(zork_client_types::composer::Intent::Edit(text.clone()));
        let height = self
            .input
            .read(cx)
            .content_height()
            .unwrap_or(liquid::composer::EDITOR_MIN);
        self.input_height =
            height.clamp(liquid::composer::EDITOR_MIN, liquid::composer::EDITOR_MAX);
        let style = window.text_style();
        self.composer.widths = self
            .composer
            .snapshot
            .members
            .iter()
            .map(|m| {
                window
                    .text_system()
                    .shape_line(
                        m.label.clone().into(),
                        px(12.),
                        &[TextRun {
                            len: m.label.len(),
                            font: style.font(),
                            color: style.color,
                            background_color: None,
                            underline: None,
                            strikethrough: None,
                        }],
                        None,
                    )
                    .width
                    .as_f32()
            })
            .collect();
        self.retarget();
    }
    fn render_actions(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let disabled = self.variant == 2;
        let busy = self.variant == 1;
        let mut list = div().p(px(wb::PREVIEW_PADDING)).flex().flex_col().gap_4();
        let mut row = div().flex().flex_wrap().gap_2();
        for (i, label, primary, w) in [
            (0, "保存", true, 76.),
            (1, "取消", false, 76.),
            (2, "文本操作", false, 92.),
            (3, "柔和按钮", false, 92.),
        ] {
            row = row.child(
                self.button(
                    &format!("action-{i}"),
                    label,
                    w,
                    ActionStyle {
                        primary,
                        variant: (i == 3).then_some(controls::ButtonVariant::Soft),
                        quiet: i == 2,
                        disabled,
                        busy: busy && i == 0,
                        ..Default::default()
                    },
                    CANVAS,
                    window,
                    cx,
                )
                .on_click(cx.listener(move |v, _, _, cx| {
                    if v.variant != 2 && !(v.variant == 1 && i == 0) {
                        v.actions += 1;
                        v.status = format!("操作触发 {} 次", v.actions);
                        cx.notify();
                    }
                }))
                .automation_enabled(
                    !disabled && !(busy && i == 0),
                    AutomationRole::Button,
                    label,
                ),
            );
        }
        list = list.child(row);
        let mut icons = div().flex().items_center().gap_3();
        for (n, icon) in [
            (24., "icons/paperclip.svg"),
            (28., "icons/settings-three.svg"),
            (32., "icons/plus.svg"),
        ] {
            icons = icons.child(
                controls::action(
                    self.sid(&format!("icon-{}", n as i32)),
                    "",
                    n,
                    n,
                    ActionStyle {
                        icon: Some(icon),
                        disabled,
                        ..Default::default()
                    },
                    CANVAS,
                    window,
                    cx,
                )
                .on_click(cx.listener(|v, _, _, cx| {
                    if v.variant != 2 {
                        v.actions += 1;
                        v.status = format!("图标操作 {} 次", v.actions);
                        cx.notify();
                    }
                }))
                .automation_enabled(
                    !disabled,
                    AutomationRole::Button,
                    format!("{} px 图标操作", n as i32),
                ),
            );
        }
        list.child(icons)
            .child(
                muted("24 / 28 / 32 px · 默认、悬停、按下、焦点、忙碌、禁用")
                    .id(self.sid("hint"))
                    .automation(AutomationRole::Status, "按钮尺寸与交互状态"),
            )
            .into_any_element()
    }
    fn render_fields(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        self.invalid = self.variant == 1;
        let w = self.width - 36.;
        div()
            .p(px(wb::PREVIEW_PADDING))
            .flex()
            .flex_col()
            .gap_3()
            .child(self.field(
                "name",
                &self.input,
                "名称",
                w,
                self.invalid,
                CANVAS,
                window,
                cx,
            ))
            .when(self.invalid, |v| {
                v.child(muted("字段错误状态由宿主提供").text_color(rgb(0xB8524C)))
            })
            .child(self.field("secret", &self.second, "凭据", w, false, CANVAS, window, cx))
            .child(
                self.button(
                    "reveal",
                    "显示 / 隐藏",
                    110.,
                    Default::default(),
                    CANVAS,
                    window,
                    cx,
                )
                .on_click(cx.listener(|v, _, _, cx| {
                    v.selected = 1 - v.selected;
                    let hidden = v.selected == 0;
                    v.second
                        .update(cx, |input, cx| input.set_secret(hidden, cx));
                    cx.notify();
                }))
                .automation(AutomationRole::Button, "显示或隐藏凭据"),
            )
            .when(self.variant == 2, |v| {
                v.opacity(0.45)
                    .capture_any_mouse_down(|_, _, cx| cx.stop_propagation())
            })
            .into_any_element()
    }
    fn render_choices(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let w = (self.width - 36.).min(280.);
        let content = div()
            .w(px(self.width))
            .p(px(wb::PREVIEW_PADDING))
            .flex()
            .flex_col()
            .gap_6();
        if self.variant == 0 {
            return content
                .child(controls::segmented_with_surface(
                    self.sid("segments"),
                    w,
                    ["随客户端", "后台运行", "手动"]
                        .into_iter()
                        .enumerate()
                        .map(|(i, label)| (self.sid(&format!("choice-{i}-hit")), label.into()))
                        .collect(),
                    self.selected,
                    true,
                    CANVAS,
                    &self.surfaces[0],
                    window,
                    cx,
                    |v, index, cx| {
                        v.selected = index;
                        v.retarget();
                        cx.notify();
                    },
                ))
                .child(
                    muted(format!(
                        "当前选择：{}",
                        ["随客户端", "后台运行", "手动"][self.selected]
                    ))
                    .id(self.sid("hint"))
                    .automation(AutomationRole::Status, "当前选择"),
                )
                .into_any_element();
        }
        if self.variant == 1 {
            return content
                .child(controls::radio_group(
                    self.sid("radios"),
                    w,
                    ["随客户端", "后台运行", "手动"]
                        .into_iter()
                        .enumerate()
                        .map(|(i, label)| (self.sid(&format!("choice-{i}-hit")), label.into()))
                        .collect(),
                    self.selected,
                    true,
                    CANVAS,
                    window,
                    cx,
                    |v, index, cx| {
                        v.selected = index;
                        v.retarget();
                        cx.notify();
                    },
                ))
                .child(
                    muted(format!(
                        "当前选择：{}",
                        ["随客户端", "后台运行", "手动"][self.selected]
                    ))
                    .id(self.sid("hint"))
                    .automation(AutomationRole::Status, "当前选择"),
                )
                .into_any_element();
        }
        use crate::components::liquid::primitives::data::{portrait_choices, PortraitOption};
        let labels = ["熊猫", "狐狸", "章鱼"];
        content
            .child(portrait_choices(
                self.sid("portraits"),
                labels
                    .into_iter()
                    .enumerate()
                    .map(|(index, label)| PortraitOption {
                        id: self.sid(&format!("choice-{index}-hit")),
                        portrait: ["panda", "fox", "octopus"][index],
                        label: label.into(),
                    })
                    .collect(),
                self.selected,
                !self.disabled,
                3,
                (w - 16.) / 3.,
                28.,
                cx.listener(|v, index: &usize, _, cx| {
                    v.selected = *index;
                    v.retarget();
                    cx.notify();
                }),
            ))
            .child(
                muted(format!("当前选择：{}", labels[self.selected]))
                    .id(self.sid("hint"))
                    .automation(AutomationRole::Status, "当前选择"),
            )
            .into_any_element()
    }
    fn render_switch(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let switch = controls::toggle_with_surface(
            self.sid("switch-hit"),
            self.selected == 1,
            !self.disabled,
            CANVAS,
            &mut self.surfaces[0],
            window,
            cx,
            |v, checked, cx| {
                v.selected = usize::from(checked);
                v.retarget();
                cx.notify();
            },
        );
        div()
            .w(px(self.width))
            .p(px(wb::PREVIEW_PADDING))
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child(caption("启用模型"))
                            .child(muted("代码模型")),
                    )
                    .child(switch.automation_enabled(
                        !self.disabled,
                        AutomationRole::Button,
                        "启用模型开关",
                    )),
            )
            .child(
                self.button(
                    "disable",
                    if self.disabled {
                        "启用操作"
                    } else {
                        "切换为禁用"
                    },
                    110.,
                    Default::default(),
                    CANVAS,
                    window,
                    cx,
                )
                .on_click(cx.listener(|v, _, _, cx| {
                    v.disabled = !v.disabled;
                    cx.notify();
                }))
                .automation(AutomationRole::Button, "切换开关禁用"),
            )
            .into_any_element()
    }
    fn navigation_items(&self) -> (Vec<liquid::navigation::Item>, liquid::navigation::Style) {
        use liquid::navigation::{Item, Kind as NavigationKind, Style};
        let horizontal = self.kind == Kind::Navigation && self.variant == 1;
        let cards = self.kind == Kind::Rows && self.variant == 1;
        let count = if horizontal || cards { 3 } else { 5 };
        let labels = if self.kind == Kind::Rows {
            ["常用连接", "开发设备", "最近任务", "工作目录", "资源管理"]
        } else {
            ["我的设备", "开发领队", "组件规范", "模型设置", "设备设置"]
        };
        let items = labels
            .into_iter()
            .take(count)
            .enumerate()
            .map(|(i, label)| {
                let mut item = Item::new(self.sid(&format!("row-{i}")), label);
                if cards {
                    item.detail =
                        Some(["API 接入 · 已配置", "本机运行环境", "保留任务上下文"][i].into());
                }
                item
            })
            .collect();
        (
            items,
            Style {
                kind: if horizontal {
                    NavigationKind::Tabs
                } else if self.kind == Kind::Rows {
                    NavigationKind::Rows
                } else {
                    NavigationKind::Sidebar
                },
                framed: true,
                parent: CANVAS,
                activate_on_arrow: true,
                row_radius: 14.,
            },
        )
    }
    fn render_navigation(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let (items, style) = self.navigation_items();
        self.navigation.render(
            self.id.clone(),
            self.width,
            items,
            self.selected,
            true,
            style,
            self.config.borrow().material,
            window,
            cx,
            |v, index, cx| {
                v.selected = index;
                cx.notify();
            },
        )
    }
    fn render_popover(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        use liquid::overlay::{Choice, Placement, Selection};
        let labels = match self.variant {
            1 => ["开发领队", "界面队员", "代码队员"],
            2 => ["置顶任务", "重命名", "归档任务"],
            _ => ["OpenAI", "Anthropic", "兼容接口"],
        };
        let selection = match self.variant {
            1 => Selection::Multiple,
            2 => Selection::Actions,
            _ => Selection::Single,
        };
        let choices = labels
            .into_iter()
            .enumerate()
            .map(|(i, label)| Choice {
                id: self.sid(&format!("option-{i}")),
                label: label.into(),
                disabled: self.variant == 2 && i == 2,
                checked: match selection {
                    Selection::Multiple => Some(self.members[i]),
                    Selection::Actions => None,
                    _ => Some(self.selected == i),
                },
            })
            .collect();
        let (source, _) = self.poses();
        let menu = self.popover.render(
            self.sid("trigger"),
            self.popover_label(),
            choices,
            selection,
            if selection == Selection::Actions {
                liquid::overlay::Trigger::Icon
            } else {
                liquid::overlay::Trigger::Button
            },
            self.open,
            true,
            Placement::Window {
                width: source.w as f32,
            },
            Material::ordinary(),
            window,
            cx,
            |v, open, w, cx| v.set_open(open, w, cx),
            |v, i, _, cx| {
                if v.variant == 1 {
                    v.members[i] = !v.members[i];
                } else {
                    v.selected = i;
                }
                v.status = format!("已选择第 {} 项", i + 1);
                cx.notify();
            },
        );
        div()
            .relative()
            .w(px(self.width))
            .h(px(self.stage_height()))
            .child(
                div()
                    .absolute()
                    .left(px(source.left() as f32))
                    .top(px(source.top() as f32))
                    .child(menu),
            )
            .into_any_element()
    }
    fn render_details(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let anchor_width = 88_f32.min((self.width - 52.) / 3.);
        let mut header = positioned(18., 14., self.width as f64 - 36., 32.)
            .flex()
            .gap_2();
        for (i, label) in ["开发领队", "组件规范", "我的设备"].into_iter().enumerate() {
            header = header.child(
                self.button(
                    &format!("anchor-{i}"),
                    label,
                    anchor_width,
                    Default::default(),
                    CANVAS,
                    window,
                    cx,
                )
                .on_hover(cx.listener(move |v, on, _, cx| {
                    if *on {
                        v.selected = i;
                        v.retarget();
                        cx.notify();
                    }
                }))
                .on_click(cx.listener(move |v, _, _, cx| {
                    v.selected = i;
                    v.retarget();
                    cx.notify();
                }))
                .automation(AutomationRole::Button, label),
            );
        }
        let text = [
            ("开发领队", "协调实现与审阅", "日常开发"),
            (
                "组件规范",
                "统一表面、轮廓和交互",
                "zork / crates / zork-ui",
            ),
            ("我的设备", "执行本地任务", "开发构建"),
        ][self.selected % 3];
        let compact = self.variant == 1;
        let panel_width = if compact {
            liquid::overlay::measure_label(text.0, 12., window) + 20.
        } else {
            (self.width - 60.).min(320.)
        };
        let panel_x = if compact {
            (18. + self.selected as f32 * (anchor_width + 8.) + anchor_width / 2.
                - panel_width / 2.)
                .clamp(8., (self.width - panel_width - 8.).max(8.))
        } else {
            18. + self.selected as f32 * 12.
        };
        let mut sections = vec![if compact {
            div()
                .text_size(px(12.))
                .line_height(px(18.))
                .child(text.0)
                .into_any_element()
        } else {
            caption(text.0).into_any_element()
        }];
        if self.variant == 0 {
            sections.extend([
                muted(text.1).into_any_element(),
                muted("详细信息").into_any_element(),
                caption(text.2).into_any_element(),
            ]);
        }
        let panel = self.panel.render(
            self.sid("surface"),
            self.width,
            liquid::panel::Placement::new(panel_x, 62., panel_width),
            liquid::panel::Content {
                sections,
                padding: if compact { 8. } else { 16. },
                gap: if compact { 0. } else { 8. },
            },
            None,
            SurfaceColors::plain(WHITE, LINE, CANVAS),
            self.config.borrow().material,
            window,
            cx,
        );
        div()
            .relative()
            .w(px(self.width))
            .child(panel)
            .child(header)
            .into_any_element()
    }
    fn render_disclosure(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        use liquid::navigation::{Item, Kind as NavigationKind, Style};
        let width = (self.width - 36.).max(140.);
        let placement = liquid::panel::Placement::new(18., 18., width);
        let inset = placement.row_inset(6., 32.);
        let label = if self.variant == 0 {
            "检查组件实现"
        } else {
            "工作区分组"
        };
        let mut source = liquid::panel::Source::new(
            self.sid("trigger"),
            label,
            Pose::rect(18., 18., width as f64, 36., 12.),
            self.open,
            false,
            |v: &mut Self, open, w, cx| v.set_open(open, w, cx),
        );
        source.style.icon = Some("icons/chevron-right.svg");
        let header = self.navigation.render(
            self.sid("heading"),
            width - 2. * inset,
            vec![Item::new(self.sid("trigger"), format!("⌄  {label}"))],
            0,
            self.open,
            Style {
                kind: NavigationKind::Actions,
                framed: false,
                parent: WHITE,
                activate_on_arrow: false,
                row_radius: placement.inner_radius(inset, 32.),
            },
            self.config.borrow().material,
            window,
            cx,
            |v, _, cx| {
                v.open = !v.open;
                cx.notify();
            },
        );
        let mut list = div().px_3().pt_3().flex().flex_col().gap_3();
        for label in if self.variant == 0 {
            [
                "✓ 读取共享组件",
                "✓ 核对真实控件尺寸",
                "✓ 记录交互与渲染状态",
            ]
        } else {
            ["我的设备", "开发领队", "组件规范"]
        } {
            list = list.child(caption(label));
        }
        self.panel.render(
            self.sid("surface"),
            self.width,
            placement,
            liquid::panel::Content {
                sections: vec![header, list.into_any_element()],
                padding: inset,
                gap: 0.,
            },
            Some(source),
            SurfaceColors::filled(WHITE, CANVAS),
            self.config.borrow().material,
            window,
            cx,
        )
    }
    fn render_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let weak = cx.entity().downgrade();
        let handler = Rc::new(move |action, w: &mut Window, cx: &mut App| {
            let _ = weak.update(cx, |v, cx| v.composer_action(action, w, cx));
        });
        let bubbles = self.composer.departures.bubbles(
            self.surfaces[0].simulation.pose(),
            &self.surfaces[0].simulation,
        );
        let frame = liquid::composer::render(
            liquid::composer::Props {
                id: &self.id,
                surface: &self.surfaces[0],
                width: self.width,
                height: self.stage_height(),
                editor: &self.input,
                snapshot: &self.composer.snapshot,
                fan_progress: self.composer.fan.position.clamp(0., 1.) as f32,
                fan_pinned: self.composer.pinned,
                bubbles: &bubbles,
                presentation: None,
                handler,
                accessory_band: 0.,
                accessories: vec![],
            },
            window,
            cx,
        );
        let mut scene = div()
            .relative()
            .w_full()
            .h(px(self.stage_height()))
            .child(frame)
            .on_drop(cx.listener(|v, paths: &ExternalPaths, _, cx| {
                v.composer_intent(
                    zork_client_types::composer::Intent::Files(
                        paths
                            .paths()
                            .iter()
                            .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
                            .collect(),
                    ),
                    cx,
                )
            }));
        if let Some(id) = self
            .composer
            .member
            .clone()
            .or(self.composer.hovered_member.clone())
        {
            if let Some(member) = self.composer.snapshot.members.iter().find(|m| m.id == id) {
                let details = crate::components::tooltip::DetailsTooltip {
                    key: id.clone(),
                    title: id.clone(),
                    kind: if self.composer.member.is_some() {
                        "成员活动记录"
                    } else {
                        "成员活动"
                    }
                    .into(),
                    avatar: Some(member.avatar.clone()),
                    description: member.label.clone(),
                    rows: vec![],
                };
                let width = (self.width - 80.).min(320.).max(160.);
                let popup = liquid::skin(
                    self.sid("member-detail"),
                    width,
                    136.,
                    16.,
                    0.6,
                    SurfaceColors::outlined(LINE, CANVAS),
                    div().p_3().child(details.content()),
                    window,
                    cx,
                )
                .on_hover(cx.listener(move |v, hover: &bool, w, cx| {
                    v.composer_action(
                        liquid::composer::Action::MemberHover(hover.then(|| id.clone())),
                        w,
                        cx,
                    )
                }))
                .automation(AutomationRole::Status, "成员活动预览");
                scene = scene.child(positioned(58., 8., width as f64, 136.).child(popup));
            }
        }
        if let Some(id) = self.composer.file {
            if let Some(file) = self.composer.snapshot.files.iter().find(|f| f.id == id) {
                scene = scene.child(
                    crate::components::smooth::surface(self.sid("file-preview"), ui::FIELD_RADIUS)
                        .absolute()
                        .left(px(18.))
                        .top(px(8.))
                        .w(px(self.width - 36.))
                        .h(px(100.))
                        .p_3()
                        .bg(rgb(WHITE))
                        .border(gpui::px(crate::design::BORDER_WIDTH))
                        .border_color(rgb(LINE))
                        .child(caption(file.name.clone()))
                        .child(muted("本地文件样例 · 未读取或上传文件内容")),
                );
            }
        }
        scene.into_any_element()
    }
    fn render_attachments(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        use liquid::navigation::{Item, Kind as NavigationKind, Style};
        let width = (self.width - 36.).clamp(140., 320.);
        let placement = liquid::panel::Placement::new(18., 18., width);
        let inset = placement.row_inset(6., 32.);
        let close = self
            .button("close", "×", 24., Default::default(), WHITE, window, cx)
            .on_click(cx.listener(|v, _, w, cx| {
                cx.stop_propagation();
                v.preview = false;
                v.set_open(false, w, cx);
            }))
            .automation(AutomationRole::Button, "收拢附件");
        let mut heading = Item::new(self.sid("heading"), "草稿附件");
        heading.heading = true;
        heading.trailing = Some(close.into_any_element());
        let mut items = vec![heading];
        if !self.preview {
            for name in self.files.clone() {
                let i = ["组件规范.md", "界面参考.png", "实现笔记.txt"]
                    .iter()
                    .position(|value| *value == name)
                    .unwrap();
                let remove = self
                    .button(
                        &format!("remove-{i}"),
                        "×",
                        24.,
                        Default::default(),
                        WHITE,
                        window,
                        cx,
                    )
                    .on_click(cx.listener(move |v, _, w, cx| {
                        cx.stop_propagation();
                        v.files.retain(|value| *value != name);
                        if v.files.is_empty() {
                            v.set_open(false, w, cx);
                        }
                        cx.notify();
                    }))
                    .automation(AutomationRole::Button, format!("移除 {name}"));
                let mut item = Item::new(self.sid(&format!("file-{i}")), name);
                item.trailing = Some(remove.into_any_element());
                items.push(item);
            }
        }
        let rows = self.navigation.render(
            self.sid("files"),
            width - 2. * inset,
            items,
            0,
            self.open,
            Style {
                kind: NavigationKind::Actions,
                framed: false,
                parent: WHITE,
                activate_on_arrow: false,
                row_radius: placement.inner_radius(inset, 32.),
            },
            self.config.borrow().material,
            window,
            cx,
            |v, index, cx| {
                if index > 0 {
                    v.preview = true;
                    cx.notify();
                }
            },
        );
        let mut sections = vec![rows];
        if self.preview {
            sections.push(div().px_3().pb_3().flex().flex_col().gap_3()
                .child(caption("组件规范.md"))
                .child(muted("共享控件使用同一份 Rust 形变、轮廓与命中逻辑。\n预览正文保留正常文字排版。"))
                .child(self.button("back", "返回附件列表", 128., Default::default(), WHITE, window, cx)
                    .on_click(cx.listener(|v, _, _, cx| { v.preview = false; cx.notify(); }))
                    .automation(AutomationRole::Button, "返回附件列表"))
                .into_any_element());
        }
        let mut source = liquid::panel::Source::new(
            self.sid("trigger"),
            format!("{} 个附件", self.files.len()),
            Pose::rect(18., 18., 124., 32., 16.),
            self.open,
            false,
            |v: &mut Self, open, w, cx| v.set_open(open, w, cx),
        );
        source.style.icon = Some("icons/paperclip.svg");
        source.style.trailing = Some("icons/chevron-down.svg");
        self.panel.render(
            self.sid("surface"),
            self.width,
            placement,
            liquid::panel::Content {
                sections,
                padding: inset,
                gap: 8.,
            },
            Some(source),
            SurfaceColors::filled(WHITE, CANVAS),
            self.config.borrow().material,
            window,
            cx,
        )
    }
    fn render_comments(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        self.selection.borrow_mut().begin_frame();
        let weak = cx.entity().downgrade();
        let selection = SelectionContext::new(
            self.sid("source"),
            crate::comments::CommentSource {
                session_id: "mock-session".into(),
                message_id: Some("mock-message".into()),
                ..Default::default()
            },
            self.document.plain_text(),
            self.selection.clone(),
            self.focus.clone(),
            Rc::new(move |cx| {
                let _ = weak.update(cx, |_, cx| cx.notify());
            }),
        );
        let anchor = self.comment_anchor.clone();
        let trigger = self
            .button(
                "trigger",
                "添加评论",
                90.,
                Default::default(),
                CANVAS,
                window,
                cx,
            )
            .track_focus(&self.trigger_focus)
            .child(
                canvas(move |bounds, _, _| anchor.set(bounds), |_, _, _, _| {})
                    .absolute()
                    .inset_0(),
            )
            .on_click(cx.listener(|v, _, window, cx| v.set_open(true, window, cx)))
            .automation(AutomationRole::Button, "添加评论");
        let queue = crate::components::comments::queue(
            &self.id,
            &self.drafts,
            window,
            cx,
            |v, comment, _, bounds, window, cx| {
                let focus = window.focused(cx);
                v.comment_editor.update(cx, |editor, cx| {
                    editor.open_at(
                        crate::components::comments::EditorRequest {
                            source: comment.source,
                            editing: Some(comment.id),
                            text: comment.comment,
                            toolbar: false,
                        },
                        bounds,
                        focus,
                        window,
                        cx,
                    )
                });
                v.open = true;
                cx.notify();
            },
            |v, key, cx| {
                v.drafts.retain(|draft| draft.id != key);
                cx.notify();
            },
        );
        div()
            .relative()
            .w(px(self.width))
            .p(px(18.))
            .flex()
            .flex_col()
            .gap(px(16.))
            .track_focus(&self.focus)
            .tab_stop(false)
            .child(message::render_selectable_document(
                &self.sid("quote"),
                &self.document,
                &selection,
            ))
            .child(trigger)
            .child(queue)
            .child(self.comment_editor.clone())
            .on_mouse_move(cx.listener(|v, e: &MouseMoveEvent, _, cx| {
                if v.selection.borrow_mut().update(e.position) {
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|v, event: &MouseUpEvent, window, cx| {
                    if !v.selection.borrow().dragging {
                        return;
                    }
                    let bounds = v
                        .selection
                        .borrow()
                        .selected_bounds()
                        .unwrap_or_else(|| Bounds::new(event.position, size(px(1.), px(1.))));
                    let selected = v.selection.borrow_mut().finish();
                    if let Some(source) = selected {
                        v.quote = Some(source.quote.clone());
                        let focus = window.focused(cx);
                        v.comment_editor.update(cx, |editor, cx| {
                            editor.open_at(
                                crate::components::comments::EditorRequest {
                                    source,
                                    editing: None,
                                    text: String::new(),
                                    toolbar: true,
                                },
                                bounds,
                                focus,
                                window,
                                cx,
                            )
                        });
                        v.open = true;
                        cx.notify();
                    }
                }),
            )
            .into_any_element()
    }
    fn render_modal(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        use liquid::overlay::Placement;
        let source = Pose::rect(18., 18., 116., 32., 16.);
        let target = Pose::rect(
            18.,
            64.,
            (self.width - 36.).clamp(140., 540.) as f64,
            (window.viewport_size().height.as_f32() - 64.).max(2.) as f64,
            ui::MODAL_RADIUS as f64,
        );
        let mut content = div()
            .relative()
            .w(px(self.width))
            .h(px(self.stage_height()));
        let trigger = self
            .dialog
            .trigger(
                self.sid("trigger"),
                "添加连接",
                116.,
                ActionStyle {
                    icon: Some("icons/plus.svg"),
                    ..Default::default()
                },
                CANVAS,
                window,
                cx,
                |v, w, cx| v.set_open(true, w, cx),
            )
            .automation(AutomationRole::Button, "添加连接");
        content = content
            .child(positioned(source.left(), source.top(), source.w, source.h).child(trigger));
        let w = (target.w - 48.) as f32;
        let mut dialog = wb::column(12.);
        if self.variant == 1 {
            dialog = dialog
                .child(caption("开发模型 · OpenAI"))
                .child(muted("API 接入\n当前设备可用\n服务地址由宿主提供"));
        } else {
            dialog = dialog
                .child(self.field(
                    "name",
                    &self.input,
                    "连接名称",
                    w,
                    self.invalid,
                    WHITE,
                    window,
                    cx,
                ))
                .when(self.invalid, |v| {
                    v.child(muted("宿主返回的字段错误示例").text_color(rgb(0xB8524C)))
                })
                .child(self.field(
                    "endpoint",
                    &self.endpoint,
                    "服务地址",
                    w,
                    false,
                    WHITE,
                    window,
                    cx,
                ))
                .child(self.field(
                    "credential",
                    &self.second,
                    "访问凭据",
                    w,
                    false,
                    WHITE,
                    window,
                    cx,
                ));
        }
        let footer = div()
            .flex()
            .flex_wrap()
            .justify_end()
            .gap_2()
            .child(
                self.button(
                    "error",
                    "错误状态",
                    86.,
                    ActionStyle {
                        selected: self.invalid,
                        ..Default::default()
                    },
                    WHITE,
                    window,
                    cx,
                )
                .on_click(cx.listener(|v, _, _, cx| {
                    v.invalid = !v.invalid;
                    v.retarget();
                    cx.notify();
                }))
                .automation(AutomationRole::Button, "切换表单错误"),
            )
            .child(
                self.button("cancel", "取消", 68., Default::default(), WHITE, window, cx)
                    .on_click(cx.listener(|v, _, w, cx| v.set_open(false, w, cx)))
                    .automation(AutomationRole::Button, "取消添加连接"),
            )
            .child(
                self.button(
                    "save",
                    "保存",
                    76.,
                    ActionStyle {
                        primary: true,
                        busy: self.busy,
                        ..Default::default()
                    },
                    WHITE,
                    window,
                    cx,
                )
                .on_click(cx.listener(|v, _, _, cx| v.submit(cx)))
                .automation_enabled(
                    !self.busy,
                    AutomationRole::Button,
                    "保存表单意图",
                ),
            );

        let panel = self.dialog.render(
            self.id.clone(),
            if self.variant == 1 {
                "连接详情"
            } else {
                "添加模型连接"
            },
            dialog,
            Some(footer.into_any_element()),
            self.open,
            Placement::Inline {
                width: self.width,
                height: self.stage_height(),
                source,
                target,
            },
            self.config.borrow().material,
            window,
            cx,
            |v, w, cx| v.set_open(false, w, cx),
        );
        content
            .when_some(panel, |v, panel| {
                v.child(div().absolute().inset_0().child(panel))
            })
            .into_any_element()
    }
    fn render_notice(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let labels = ["提示", "加载", "成功", "警告", "错误"];
        let row = positioned(18., 16., self.width as f64 - 36., 32.).child(controls::segmented(
            self.sid("states"),
            (self.width - 36.).min(288.),
            labels
                .iter()
                .enumerate()
                .map(|(i, label)| (self.sid(&format!("state-{i}")), (*label).into()))
                .collect(),
            self.variant,
            true,
            CANVAS,
            window,
            cx,
            |v, i, cx| {
                v.variant = i;
                v.retarget();
                cx.notify();
            },
        ));
        let message = [
            "配置将在下一次操作中生效。",
            "正在等待样例操作结果…",
            "样例操作已完成。",
            "当前选项需要进一步确认。",
            "操作暂时不可用。输入已保留，可以重试。",
        ][self.variant % 5];
        let kind = [
            ui::NoticeKind::Info,
            ui::NoticeKind::Loading,
            ui::NoticeKind::Success,
            ui::NoticeKind::Warning,
            ui::NoticeKind::Error,
        ][self.variant % 5];
        let action = (self.variant == 4).then(|| {
            self.button(
                "retry",
                "重试",
                60.,
                Default::default(),
                0xFFFAFA,
                window,
                cx,
            )
            .on_click(cx.listener(|v, _, _, cx| {
                v.variant = 1;
                v.retarget();
                v.submit(cx);
            }))
            .automation(AutomationRole::Button, "重试状态提示")
            .into_any_element()
        });
        let notice = liquid::primitives::feedback::notice_content(
            self.sid("content"),
            message,
            kind,
            action,
        );
        let panel = self.panel.render(
            self.sid("surface"),
            self.width,
            liquid::panel::Placement {
                x: 18.,
                y: 64.,
                width: (self.width - 36.).max(140.),
                radius: ui::CARD_RADIUS,
            },
            liquid::panel::Content {
                sections: vec![notice.into_any_element()],
                padding: 20.,
                gap: 0.,
            },
            None,
            SurfaceColors::filled(
                if self.variant == 2 {
                    0xECFDF3
                } else if self.variant == 4 {
                    0xFFFAFA
                } else {
                    WHITE
                },
                CANVAS,
            ),
            self.config.borrow().material,
            window,
            cx,
        );
        div()
            .relative()
            .w(px(self.width))
            .child(panel)
            .child(row)
            .into_any_element()
    }
}

impl Render for Card {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.width == 0. {
            self.layout((window.viewport_size().width.as_f32() - 88.).max(240.));
        }
        self.measure_input(window, cx);
        self.measure_popover(window);
        self.advance(window, cx);
        let mut variants = wb::wrap(8.);
        let labels = self.variant_labels();
        if !labels.is_empty() {
            let width = self.selector.trigger_width(
                labels[self.variant.min(labels.len() - 1)],
                liquid::overlay::Trigger::Field,
                window,
            );
            variants = variants
                .child(ui::label(
                    if matches!(self.kind, Kind::Actions | Kind::Fields | Kind::Composer) {
                        "示例状态"
                    } else if self.kind.section() == Section::Scenarios {
                        "场景变体"
                    } else {
                        "组件变体"
                    },
                ))
                .child(if labels.len() <= 3 {
                    controls::segmented(
                        self.sid("variant-options"),
                        (labels
                            .iter()
                            .map(|label| liquid::overlay::measure_label(label, 12., window))
                            .fold(0_f32, f32::max)
                            + 24.)
                            * labels.len() as f32
                            + 8.,
                        labels
                            .iter()
                            .enumerate()
                            .map(|(i, label)| (self.sid(&format!("variant-{i}")), (*label).into()))
                            .collect(),
                        self.variant,
                        true,
                        CANVAS,
                        window,
                        cx,
                        |v, i, cx| v.set_variant(i, cx),
                    )
                    .into_any_element()
                } else {
                    wb::slot(width)
                        .child(
                            self.selector.render(
                                self.sid("variant-select"),
                                labels[self.variant.min(labels.len() - 1)].to_owned(),
                                labels
                                    .iter()
                                    .enumerate()
                                    .map(|(i, label)| liquid::overlay::Choice {
                                        disabled: false,
                                        id: self.sid(&format!("variant-{i}")),
                                        label: (*label).into(),
                                        checked: Some(self.variant == i),
                                    })
                                    .collect(),
                                liquid::overlay::Selection::Single,
                                liquid::overlay::Trigger::Field,
                                self.variant_menu,
                                true,
                                liquid::overlay::Placement::Window { width },
                                self.config.borrow().material,
                                window,
                                cx,
                                |v, open, _, cx| {
                                    v.variant_menu = open;
                                    cx.notify();
                                },
                                |v, i, _, cx| v.set_variant(i, cx),
                            ),
                        )
                        .into_any_element()
                });
        }
        if self.kind == Kind::Composer {
            let origins = controls::segmented(
                self.sid("departure-options"),
                256.,
                ["从发送按钮", "从输入区"]
                    .into_iter()
                    .enumerate()
                    .map(|(i, label)| (self.sid(&format!("departure-mode-{i}")), label.into()))
                    .collect(),
                usize::from(self.composer.departure_origin == liquid::departure::Origin::Composer),
                true,
                CANVAS,
                window,
                cx,
                |v, index, cx| {
                    v.composer.departure_origin = if index == 0 {
                        liquid::departure::Origin::Button
                    } else {
                        liquid::departure::Origin::Composer
                    };
                    cx.notify();
                },
            );
            variants = variants.child(
                wb::wrap(8.)
                    .w_full()
                    .child(ui::label("发送动效"))
                    .child(origins),
            );
        }
        let stage = match self.kind {
            Kind::Primitive(_) => {
                let entity = self.specimen.as_ref().unwrap().clone();
                entity.update(cx, |v, cx| v.set_width(self.width, cx));
                entity.into_any_element()
            }
            Kind::Actions => self.render_actions(window, cx),
            Kind::Fields => self.render_fields(window, cx),
            Kind::Choices => self.render_choices(window, cx),
            Kind::Switch => self.render_switch(window, cx),
            Kind::Navigation | Kind::Rows => self.render_navigation(window, cx),
            Kind::Popover => self.render_popover(window, cx),
            Kind::Details => self.render_details(window, cx),
            Kind::Disclosure => self.render_disclosure(window, cx),
            Kind::Composer => self.render_composer(window, cx),
            Kind::Attachments => self.render_attachments(window, cx),
            Kind::Comments => self.render_comments(window, cx),
            Kind::Modal => self.render_modal(window, cx),
            Kind::Notice => self.render_notice(window, cx),
        };
        let stage = if matches!(
            self.kind,
            Kind::Details | Kind::Disclosure | Kind::Attachments | Kind::Comments | Kind::Notice
        ) {
            div()
                .pb(px(wb::PREVIEW_PADDING))
                .child(stage)
                .into_any_element()
        } else {
            stage
        };
        let mut result = wb::specimen(
            self.id.clone(),
            self.width,
            self.show_title
                .then_some((self.kind.title(), self.kind.description())),
        )
        .when(self.kind == Kind::Composer, |v| v.track_focus(&self.focus))
        .when(!labels.is_empty() || self.kind == Kind::Composer, |v| {
            v.child(variants)
        })
        .child(wb::preview(self.sid("preview-frame"), self.width, stage));
        if self.config.borrow().cycle && !self.hud.is_empty() {
            result = result.child(
                muted(self.hud.clone())
                    .id(self.sid("engine-metrics"))
                    .automation(AutomationRole::Status, self.hud.clone()),
            );
        }
        if !self.status.is_empty() {
            result = result.child(
                muted(self.status.clone())
                    .id(self.sid("status"))
                    .automation(AutomationRole::Status, self.status.clone()),
            );
        }
        self.layout_cache.measure(
            result.on_key_down(cx.listener(|v, e: &KeyDownEvent, w, cx| {
                if e.keystroke.key == "escape" && v.kind == Kind::Composer {
                    v.composer.member = None;
                    v.composer.hovered_member = None;
                    v.composer.member_leave = None;
                    v.composer.file = None;
                    v.composer.pinned = false;
                    v.composer.fan.target = 0.;
                    cx.notify();
                    cx.stop_propagation();
                } else if e.keystroke.key == "escape"
                    && v.open
                    && !matches!(v.kind, Kind::Popover | Kind::Modal)
                {
                    v.set_open(false, w, cx);
                    cx.stop_propagation();
                }
            })),
        )
    }
}
