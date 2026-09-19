//! Hallmark · genre: modern-minimal · macrostructure: Workbench
//! Hallmark · pre-emit critique: P4 H4 E4 S5 R4 V4
//! design-system: design.md · designed-as-app · live component previews
//! The workbench and specimens consume the same complete liquid components.
use super::*;
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    components::{
        liquid::controls::{self as component, ActionStyle},
        workbench as wb,
    },
    controls as ui,
};

const WHITE: u32 = crate::design::CUE_UI.palette.canvas;

pub(super) const GROUPS: [(&str, &str); 10] = [
    ("基础控件", "按钮、输入、选择与开关的交互状态。"),
    ("导航与反馈", "比较导航、菜单、提示与卡片的定位和反馈。"),
    (
        "消息与附件",
        "组合输入、成员、附件与评论，检查完整消息场景。",
    ),
    ("设置与执行", "检查连接表单与执行历史中的组合交互。"),
    ("并发测试", "同时运行多个组件和场景，观察连续交互的表现。"),
    ("选择与数值", "复选、切换与数值输入的交互合同。"),
    ("反馈与状态", "进度、通知和加载状态的独立展示。"),
    ("浮层与展开", "菜单、对话框、提示和内容展开。"),
    ("内容与布局", "内容组织、导航、数据呈现与滚动。"),
    ("输入与表单", "文本编辑、标签和输入反馈。"),
];

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Panel {
    Library,
    Parameters,
    Rules,
}
impl Panel {
    pub(super) fn key(self) -> &'static str {
        match self {
            Self::Library => "library",
            Self::Parameters => "parameters",
            Self::Rules => "rules",
        }
    }
}

impl Gallery {
    fn library(
        &mut self,
        width: f32,
        parent: u32,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let props = super::library::Props {
            width,
            parent,
            section: self.section,
            group: self.group,
            selected_kind: self.selected_kind,
            business_family: self.business_family.clone(),
            families: business::families(cx),
            enabled: self.recording.is_none(),
            material: self.config.borrow().material,
        };
        if let Some(library) = &self.library {
            if library.read(cx).props != props {
                library.update(cx, |v, cx| {
                    v.props = props;
                    cx.notify();
                });
                crate::components::region::invalidate(cx, &["playground-library"]);
            }
        } else {
            let parent = cx.entity().downgrade();
            self.library = Some(cx.new(|cx| {
                crate::components::region::forget_on_release(cx);
                super::library::Library {
                    props,
                    parent,
                    navigation: liquid::navigation::Navigation::new(),
                }
            }));
        }
        self.regions
            .auto_height("playground-library", width, cx, |v, _, _| {
                crate::components::region::tracked_view(
                    v.library.as_ref().expect("directory entity").clone(),
                )
                .into_any_element()
            })
    }

    fn parameter_panel(&self, width: f32, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let cfg = self.config.borrow();
        let material = &cfg.material;
        let enabled = self.recording.is_none();
        let mut rows = wb::column(20.);
        for (key, label, help, value, less, more) in [
            (
                "budget",
                "粒子数量",
                "形变精度与计算量",
                material.budget.to_string(),
                material.budget > 6,
                material.budget < 64,
            ),
            (
                "flow",
                "流动",
                "表面流动的幅度",
                format!("{:.2}", material.flow),
                material.flow > 0.001,
                material.flow < 0.399,
            ),
            (
                "damping",
                "阻尼",
                "振动衰减的速度",
                format!("{:.1}", material.damping),
                material.damping > 0.401,
                material.damping < 1.999,
            ),
            (
                "adhesion",
                "牵引",
                "相邻表面的牵引力度",
                format!("{:.2}", material.adhesion),
                material.adhesion > 0.001,
                material.adhesion < 1.199,
            ),
            (
                "smoothing",
                "圆角平滑",
                "轮廓圆角的平滑程度",
                format!("{:.1}", material.smoothing),
                material.smoothing > 0.001,
                material.smoothing < 0.999,
            ),
        ] {
            rows = rows.child(wb::setting(
                label,
                help,
                wb::stepper(
                    format!("liquid-{key}"),
                    label,
                    value,
                    enabled && less,
                    enabled && more,
                    window,
                    cx,
                    move |v, direction, cx| v.set_parameter(key, direction, cx),
                ),
            ));
        }
        rows.child(wb::separated(
            wb::switch_setting(
                "慢放",
                "以 0.3 倍速度观察过渡",
                component::toggle(
                    "liquid-slow",
                    cfg.slow,
                    enabled,
                    WHITE,
                    window,
                    cx,
                    |v, slow, cx| {
                        if v.recording.is_none() {
                            v.config.borrow_mut().slow = slow;
                            v.refresh_cards(cx);
                        }
                    },
                )
                .automation_enabled(enabled, AutomationRole::Option, "慢放"),
            ),
            16.,
        ))
        .child(
            component::action(
                "liquid-reset",
                "恢复默认值",
                width,
                32.,
                ActionStyle {
                    disabled: !enabled,
                    ..Default::default()
                },
                WHITE,
                window,
                cx,
            )
            .on_click(cx.listener(|v, _, _, cx| {
                if v.recording.is_some() {
                    return;
                }
                let mut config = v.config.borrow_mut();
                config.material = Material::default();
                config.slow = false;
                config.epoch += 1;
                drop(config);
                v.refresh_cards(cx);
            }))
            .automation_enabled(enabled, AutomationRole::Button, "恢复默认值"),
        )
        .child(wb::description("参数只影响当前页面的演示。"))
        .child(
            liquid::controls::action(
                "liquid-rules-open",
                "查看界面规范",
                116.,
                32.,
                ActionStyle {
                    quiet: true,
                    ..Default::default()
                },
                WHITE,
                window,
                cx,
            )
            .on_click(cx.listener(|v, _, _, cx| {
                v.panel = Some(Panel::Rules);
                cx.notify();
            }))
            .automation(AutomationRole::Button, "查看界面规范"),
        )
    }

    fn benchmark_controls(&self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let cfg = self.config.borrow();
        let enabled = self.recording.is_none();
        let mut counts = wb::wrap(8.);
        for count in [1, 2, 4, 8, 12, 14] {
            counts = counts.child(
                component::action(
                    format!("liquid-count-{count}"),
                    count.to_string(),
                    36.,
                    32.,
                    ActionStyle {
                        selected: self.count == count,
                        disabled: !enabled,
                        ..Default::default()
                    },
                    wb::CANVAS,
                    window,
                    cx,
                )
                .on_click(cx.listener(move |v, _, _, cx| {
                    if v.recording.is_none() {
                        v.count = count;
                        v.refresh_cards(cx);
                    }
                }))
                .automation_enabled(
                    enabled,
                    AutomationRole::Button,
                    format!("{count} 个控件"),
                ),
            );
        }
        wb::toolbar()
            .child(wb::wrap(12.).child(ui::label("同时展示")).child(counts))
            .child(
                wb::wrap(8.)
                    .child(
                        component::action(
                            "liquid-cycle",
                            "连续交互",
                            88.,
                            32.,
                            ActionStyle {
                                selected: cfg.cycle,
                                disabled: !enabled,
                                ..Default::default()
                            },
                            wb::CANVAS,
                            window,
                            cx,
                        )
                        .on_click(cx.listener(|v, _, _, cx| {
                            if v.recording.is_some() {
                                return;
                            }
                            let mut config = v.config.borrow_mut();
                            config.cycle = !config.cycle;
                            config.reset += 1;
                            drop(config);
                            v.refresh_cards(cx);
                        }))
                        .automation_enabled(
                            enabled,
                            AutomationRole::Button,
                            "连续交互",
                        ),
                    )
                    .child(
                        component::action(
                            "liquid-record",
                            "记录 5 秒",
                            96.,
                            32.,
                            ActionStyle {
                                primary: true,
                                busy: !enabled,
                                ..Default::default()
                            },
                            wb::CANVAS,
                            window,
                            cx,
                        )
                        .on_click(cx.listener(|v, _, _, cx| v.start_recording(cx)))
                        .automation_enabled(
                            enabled,
                            AutomationRole::Button,
                            "记录五秒",
                        ),
                    )
                    .child(
                        component::action(
                            "liquid-copy-record",
                            "复制记录",
                            88.,
                            32.,
                            ActionStyle {
                                disabled: self.last_record.is_none(),
                                ..Default::default()
                            },
                            wb::CANVAS,
                            window,
                            cx,
                        )
                        .on_click(cx.listener(|v, _, _, cx| {
                            if let Some(record) = &v.last_record {
                                cx.write_to_clipboard(ClipboardItem::new_string(
                                    serde_json::to_string_pretty(record).unwrap(),
                                ));
                            }
                        }))
                        .automation_enabled(
                            self.last_record.is_some(),
                            AutomationRole::Button,
                            "复制记录 JSON",
                        ),
                    ),
            )
            .child(wb::description(
                "记录形变与轮廓计算，包含发送气泡；不包含完整绘制或屏幕帧率。",
            ))
    }
}

fn rules() -> Div {
    let mut content = wb::column(20.);
    for (title, text) in [
        (
            "明确层级",
            "目录区分共享组件与业务组合，工作区展示示例，参数面板调整演示。并发测试单独进入。",
        ),
        (
            "统一排版",
            "标题、正文、说明使用项目的文字角色；同类内容保持字号、字重和行高一致。",
        ),
        (
            "统一尺寸",
            "普通操作与导航使用 32px 热区。图标、输入和按钮沿用各自的共享形状与内边距。",
        ),
        (
            "完整反馈",
            "悬停、按下和键盘焦点可区分；选中状态保持可见。禁用和忙碌时不响应重复操作。",
        ),
        (
            "稳定布局",
            "切换状态不改变控件尺寸。窄窗口通过目录与参数弹层保持示例宽度。",
        ),
        (
            "可观察的动效",
            "快速反向操作从当前画面接续；尊重减少动态效果设置，静止后停止绘制。",
        ),
    ] {
        content = content.child(wb::section_title(title, text));
    }
    content
}

#[derive(Clone, PartialEq)]
pub(super) struct Background {
    width: f32,
    layout: wb::Layout,
    selected_kind: Option<Kind>,
    benchmark: bool,
    business_example: Option<Entity<business::Example>>,
    title: String,
    help: String,
    cards: Vec<Entity<Card>>,
    enabled: bool,
}

pub(super) struct BackgroundView {
    props: Background,
    parent: WeakEntity<Gallery>,
    scroll: ScrollHandle,
    scroll_offset: Point<Pixels>,
}
impl Render for BackgroundView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Background {
            width,
            layout,
            selected_kind,
            benchmark,
            business_example,
            title,
            help,
            cards,
            enabled,
        } = self.props.clone();
        let offset = self.scroll.offset();
        let scrolled = offset != self.scroll_offset;
        self.scroll_offset = offset;
        let library_inline = layout.navigation.is_some();
        let inspector_inline = layout.inspector.is_some();
        let inner = layout.content_width;
        let card_width = layout.item_width;
        let gutter = layout.gutter;
        let mut body = wb::body();
        if library_inline {
            body = body.child(wb::rail(
                "liquid-library",
                layout.navigation.unwrap(),
                wb::Rail::Navigation,
                self.parent
                    .update(cx, |v, cx| {
                        v.library(
                            layout
                                .navigation
                                .map_or((width - 40.).min(ui::DIALOG_WIDTH) - 48., |w| w - 24.),
                            if library_inline {
                                crate::design::CUE_UI.palette.sidebar
                            } else {
                                WHITE
                            },
                            window,
                            cx,
                        )
                    })
                    .unwrap_or_else(|_| gpui::Empty.into_any_element()),
            ));
        }
        let show_demo = cards
            .iter()
            .any(|card| !matches!(card.read(cx).kind, Kind::Primitive(_)));
        let mut content = wb::content(inner).child(wb::page_heading(
            title,
            help,
            wb::wrap(0.).when(show_demo, |row| {
                row.child(
                    component::action(
                        "liquid-demo",
                        "演示",
                        60.,
                        32.,
                        ActionStyle {
                            primary: !benchmark,
                            disabled: !enabled,
                            ..Default::default()
                        },
                        wb::CANVAS,
                        window,
                        cx,
                    )
                    .on_click(cx.listener(|v, _, _, cx| {
                        let _ = v.parent.update(cx, |v, cx| {
                            if v.recording.is_some() {
                                return;
                            }
                            for card in v.active_cards(cx) {
                                card.update(cx, |v, cx| v.demo(cx));
                            }
                            cx.notify();
                        });
                    }))
                    .automation_enabled(
                        enabled,
                        AutomationRole::Button,
                        "演示当前组",
                    ),
                )
            }),
        ));
        if benchmark {
            content = content.child(
                self.parent
                    .update(cx, |v, cx| v.benchmark_controls(window, cx))
                    .unwrap_or_else(|_| div()),
            );
        }
        let mut grid = wb::grid();
        for card in cards {
            card.update(cx, |v, cx| {
                // A scroll changes every view origin. Measure those children
                // in this layout pass instead of discovering cache misses in
                // prepaint and running a separate root layout for each card.
                if scrolled {
                    v.layout_cache.invalidate();
                }
                let mut changed = false;
                if (v.width - card_width).abs() > 0.01 {
                    v.layout(card_width);
                    changed = true;
                }
                if v.show_title != selected_kind.is_none() {
                    v.show_title = selected_kind.is_none();
                    v.layout_cache.invalidate();
                    changed = true;
                }
                if changed {
                    cx.notify();
                }
            });
            let cache = card.read(cx).layout_cache.clone();
            grid = grid.child(wb::slot(card_width).child(cache.element(card, px(card_width))));
        }
        content = content
            .child(if let Some(example) = business_example {
                example.into_any_element()
            } else {
                grid.into_any_element()
            })
            .child(wb::description("本页操作使用独立演示数据。"));
        body = body.child(
            wb::viewport("liquid-playground-content", gutter, content).track_scroll(&self.scroll),
        );
        if inspector_inline {
            body = body.child(wb::rail(
                "liquid-inspector",
                layout.inspector.unwrap(),
                wb::Rail::Inspector,
                wb::column(20.)
                    .child(wb::section_title("演示参数", "调整材料的形变与过渡。"))
                    .child(
                        self.parent
                            .update(cx, |v, cx| {
                                v.parameter_panel(
                                    layout
                                        .inspector
                                        .map_or((width - 40.).min(ui::DIALOG_WIDTH) - 48., |w| {
                                            w - 32.
                                        }),
                                    window,
                                    cx,
                                )
                            })
                            .unwrap_or_else(|_| div()),
                    ),
            ));
        }
        div().size_full().flex().child(body).into_any_element()
    }
}

impl Render for Gallery {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let width = window.viewport_size().width.as_f32();
        let selected_kind = self.focused.or(self.selected_kind);
        let benchmark = self.group == 4;
        let business_active =
            self.section == Section::Scenarios && self.focused.is_none() && !benchmark;
        let business_families = business::families(cx);
        if business_active && self.business_family.is_none() {
            self.business_family = business_families.first().map(|(key, _)| key.clone());
        }
        for (family, example) in &self.business_examples {
            let active = business_active && self.business_family.as_ref() == Some(family);
            example.update(cx, |v, cx| v.set_active(active, cx));
        }
        let business_example = if business_active {
            self.business_family.clone().map(|family| {
                self.business_examples
                    .entry(family.clone())
                    .or_insert_with(|| cx.new(|cx| business::Example::new(&family, cx)))
                    .clone()
            })
        } else {
            None
        };
        let mut layout = wb::Layout::new(
            width,
            self.focused.is_none(),
            if selected_kind.is_some() || business_active {
                wb::GridMode::Focused
            } else if benchmark {
                wb::GridMode::Dense
            } else {
                wb::GridMode::Standard
            },
        );
        if business_active {
            if let Some(inspector) = layout.inspector.take() {
                layout.content_width = (layout.content_width + inspector).min(1120.);
                layout.item_width = layout.content_width;
            }
        }
        let library_inline = layout.navigation.is_some();
        let inspector_inline = layout.inspector.is_some();
        if (library_inline && self.panel == Some(Panel::Library))
            || (inspector_inline && self.panel == Some(Panel::Parameters))
        {
            self.panel = None;
        }
        if self.panel.is_some() {
            self.presented_panel = self.panel;
        }
        let cfg = self.config.borrow().clone();
        liquid::press::set_playback_rate(if cfg.slow { 0.3 } else { 1. }, cx);
        self.record_frame(window, cx);
        let (title, help) = if business_active {
            (
                business_families
                    .iter()
                    .find(|(family, _)| Some(family) == self.business_family.as_ref())
                    .map_or_else(|| "业务组件".to_owned(), |(_, title)| title.clone()),
                "与项目使用同一个业务组件，当前传入 mock 参数。".to_owned(),
            )
        } else {
            let (title, help) = selected_kind.map_or(GROUPS[self.group], |kind| {
                (kind.title(), kind.description())
            });
            (title.to_owned(), help.to_owned())
        };
        let gutter = layout.gutter;

        let mut actions = wb::row(8.);
        if !library_inline && self.focused.is_none() {
            actions = actions.child(
                self.modal
                    .trigger(
                        "liquid-library-toggle",
                        "目录",
                        64.,
                        ActionStyle {
                            icon: Some("icons/chevron-down.svg"),
                            ..Default::default()
                        },
                        WHITE,
                        window,
                        cx,
                        |v, _, cx| {
                            v.panel = Some(Panel::Library);
                            cx.notify();
                        },
                    )
                    .automation(AutomationRole::Button, "打开演示目录"),
            );
        }
        if !inspector_inline && !business_active {
            actions = actions.child(
                self.modal
                    .trigger(
                        "liquid-parameters-toggle",
                        "参数",
                        64.,
                        ActionStyle {
                            icon: Some("icons/settings.svg"),
                            ..Default::default()
                        },
                        WHITE,
                        window,
                        cx,
                        |v, _, cx| {
                            v.panel = Some(Panel::Parameters);
                            cx.notify();
                        },
                    )
                    .automation(AutomationRole::Button, "打开演示参数"),
            );
        }
        let header = wb::header(
            "Playground",
            (width > 600.).then_some(if benchmark {
                "测试工具"
            } else {
                self.section.title()
            }),
            gutter,
            actions,
        );

        let props = Background {
            width,
            layout,
            selected_kind,
            benchmark,
            business_example,
            title,
            help,
            cards: self.active_cards(cx),
            enabled: self.recording.is_none(),
        };
        if let Some(background) = &self.background {
            if background.read(cx).props != props {
                background.update(cx, |v, cx| {
                    v.props = props;
                    cx.notify();
                });
                crate::components::region::invalidate(cx, &["playground-body"]);
            }
        } else {
            let parent = cx.entity().downgrade();
            self.background = Some(cx.new(|cx| {
                crate::components::region::forget_on_release(cx);
                BackgroundView {
                    props,
                    parent,
                    scroll: ScrollHandle::new(),
                    scroll_offset: point(px(0.), px(0.)),
                }
            }));
        }
        // Keep each example's input/layout cache independent while the GPU
        // reuses unchanged page pixels during material-only animation.
        let render_background = |v: &mut Gallery, _: &mut Window, _: &mut Context<Gallery>| {
            crate::components::region::tracked_view(
                v.background.as_ref().expect("page background").clone(),
            )
        };
        let scrolling = self.background.as_ref().is_some_and(|background| {
            let view = background.read(cx);
            view.scroll.offset() != view.scroll_offset
        });
        let background = if scrolling {
            // Moving page pixels cannot reuse a color texture. Drawing them
            // directly avoids an extra offscreen pass on every wheel update.
            self.regions
                .uncached("playground-body", cx, render_background)
        } else {
            self.regions
                .gpu_uncached("playground-body", cx, render_background)
        };
        let body = div().w_full().flex_1().min_h_0().child(background);
        let overlay = self.presented_panel.and_then(|panel| {
            let (id, title, contents) = match panel {
                Panel::Library => (
                    "liquid-library-dialog",
                    "演示目录",
                    self.library(
                        layout
                            .navigation
                            .map_or((width - 40.).min(ui::DIALOG_WIDTH) - 48., |w| w - 24.),
                        if library_inline {
                            crate::design::CUE_UI.palette.sidebar
                        } else {
                            WHITE
                        },
                        window,
                        cx,
                    ),
                ),
                Panel::Parameters => (
                    "liquid-parameters",
                    "演示参数",
                    self.parameter_panel(
                        layout
                            .inspector
                            .map_or((width - 40.).min(ui::DIALOG_WIDTH) - 48., |w| w - 32.),
                        window,
                        cx,
                    )
                    .into_any_element(),
                ),
                Panel::Rules => (
                    "liquid-rules",
                    "Playground 界面规范",
                    rules().into_any_element(),
                ),
            };
            let overlay = self.modal.render(
                id,
                title,
                contents,
                None,
                self.panel.is_some(),
                liquid::overlay::Placement::Window {
                    width: ui::DIALOG_WIDTH,
                },
                cfg.material,
                window,
                cx,
                |v, _, cx| {
                    v.panel = None;
                    cx.notify();
                },
            );
            if overlay.is_none() {
                self.presented_panel = None;
            }
            overlay
        });
        wb::shell("liquid-gallery", wb::font(), header, body, overlay)
    }
}
