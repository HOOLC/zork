use super::*;
impl Specimen {
    pub(super) fn content_example(
        &mut self,
        width: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match self.example {
            Example::Tabs => {
                let selected = self.selected.first().copied().unwrap_or(0);
                if self.variant == 2 {
                    return stack()
                        .child(p::disclosure::tab_nav(
                            self.sid("nav"),
                            vec![
                                p::selection::Item::new(self.sid("link-0"), "概览"),
                                p::selection::Item::new(self.sid("link-1"), "详情"),
                                p::selection::Item::new(self.sid("link-2"), "归档").disabled(),
                            ],
                            selected,
                            width,
                            window,
                            cx,
                            |v, index, cx| {
                                v.selected = vec![index];
                                cx.notify();
                            },
                        ))
                        .child(note(if selected == 0 {
                            "当前页面：概览"
                        } else {
                            "当前页面：详情"
                        }))
                        .into_any_element();
                }
                let body = stack()
                    .child(div().child(if selected == 0 {
                        "这里是概览面板。"
                    } else {
                        "这里是详情面板，可以编辑备注。"
                    }))
                    .when(selected == 1, |v| {
                        v.child(p::input::field(
                            self.sid("details"),
                            "备注",
                            &self.input,
                            width,
                            32.,
                            None,
                            false,
                            WHITE,
                            window,
                            cx,
                        ))
                    });
                p::disclosure::tabs(
                    self.sid("control"),
                    vec![
                        p::selection::Item::new(self.sid("item-0"), "概览"),
                        p::selection::Item::new(self.sid("item-1"), "详情"),
                        p::selection::Item::new(self.sid("item-2"), "归档").disabled(),
                    ],
                    selected,
                    self.variant == 1,
                    width,
                    body,
                    window,
                    cx,
                    |v, index, cx| {
                        v.selected = vec![index];
                        cx.notify();
                    },
                )
            }
            Example::Toolbar => {
                let toolbar = p::menu::toolbar(
                    self.sid("control"),
                    vec![
                        p::menu::Item::new(self.sid("bold"), "加粗")
                            .check(self.selected.contains(&0)),
                        p::menu::Item::new(self.sid("italic"), "斜体")
                            .check(self.selected.contains(&1)),
                        p::menu::Item::new(self.sid("disabled"), "链接").disabled(),
                        p::menu::Item::separator(self.sid("separator")),
                        p::menu::Item::new(self.sid("copy"), "复制"),
                    ],
                    window,
                    cx,
                    |v, key, cx| {
                        if key.ends_with("-bold") || key.ends_with("-italic") {
                            let index = if key.ends_with("-bold") { 0 } else { 1 };
                            v.selected =
                                p::selection::Mode::MultipleToggle.change(&v.selected, index);
                        } else {
                            cx.write_to_clipboard(ClipboardItem::new_string("工具栏示例".into()));
                            v.actions += 1;
                            v.status = "已复制示例文字".into();
                        }
                        cx.notify();
                    },
                );
                stack()
                    .child(toolbar)
                    .child(
                        div()
                            .text_size(px(16.))
                            .line_height(px(25.))
                            .when(self.selected.contains(&0), |v| {
                                v.font_weight(FontWeight::BOLD)
                            })
                            .when(self.selected.contains(&1), |v| v.italic())
                            .child("这是一段可设置格式的示例文字。"),
                    )
                    .into_any_element()
            }
            Example::NavigationMenu => {
                use p::dialog::{NavGroup, NavLink};
                let groups = vec![
                    NavGroup {
                        key: self.sid("products").into(),
                        label: "产品".into(),
                        links: vec![
                            NavLink {
                                key: "overview".into(),
                                label: "产品概览".into(),
                                description: "了解功能与使用方式。".into(),
                            },
                            NavLink {
                                key: "updates".into(),
                                label: "最新更新".into(),
                                description: "查看近期变化。".into(),
                            },
                        ],
                    },
                    NavGroup {
                        key: self.sid("resources").into(),
                        label: "资源".into(),
                        links: vec![
                            NavLink {
                                key: "guide".into(),
                                label: "使用指南".into(),
                                description: "从第一步开始。".into(),
                            },
                            NavLink {
                                key: "examples".into(),
                                label: "组件示例".into(),
                                description: "探索可交互的组件。".into(),
                            },
                        ],
                    },
                ];
                let nav = self.navigation_menu.render(
                    self.sid("control"),
                    groups,
                    width,
                    window,
                    cx,
                    |v, key, cx| {
                        v.status = format!(
                            "当前页面：{}",
                            match key.as_str() {
                                "overview" => "产品概览",
                                "updates" => "最新更新",
                                "guide" => "使用指南",
                                _ => "组件示例",
                            }
                        );
                        v.actions += 1;
                        cx.notify();
                    },
                );
                stack()
                    .child(nav)
                    .child(note("悬停或聚焦后按方向键展开，选择一个页面。"))
                    .into_any_element()
            }
            Example::Avatar => {
                let mut avatars = row();
                for (index, size) in [24., 40., 64.].into_iter().enumerate() {
                    let source = match self.variant {
                        1 => p::data::AvatarSource::Fallback,
                        2 => p::data::AvatarSource::Loading,
                        _ => p::data::AvatarSource::Portrait("fox"),
                    };
                    avatars = avatars.child(
                        stack()
                            .gap(px(8.))
                            .items_center()
                            .child(p::data::avatar(
                                self.sid(&format!("avatar-{index}")),
                                "Alex",
                                source,
                                size,
                            ))
                            .child(note(format!("{size:.0}px"))),
                    );
                }
                avatars.into_any_element()
            }
            Example::DataList => p::data::data_list(
                self.sid("control"),
                vec![
                    ("名称".into(), "组件工作台".into()),
                    ("状态".into(), "已准备".into()),
                    (
                        "说明".into(),
                        "较长的内容会在可用宽度内换行，标签保持在左侧对齐。".into(),
                    ),
                    ("更新时间".into(), "今天 09:30".into()),
                ],
                width,
            ),
            Example::Table => p::data::table(
                self.sid("control"),
                vec![
                    p::data::Column {
                        label: "名称".into(),
                        width: 150.,
                        numeric: false,
                    },
                    p::data::Column {
                        label: "状态".into(),
                        width: 108.,
                        numeric: false,
                    },
                    p::data::Column {
                        label: "数量".into(),
                        width: 90.,
                        numeric: true,
                    },
                ],
                [
                    ("设计稿", "已完成", "12"),
                    ("交互原型", "进行中", "4"),
                    ("图标资源", "已完成", "128"),
                ]
                .into_iter()
                .enumerate()
                .map(|(i, (name, status, count))| p::data::Row {
                    key: i.to_string(),
                    cells: vec![name.into(), status.into(), count.into()],
                })
                .collect(),
                width,
            ),
            Example::ScrollArea => {
                let mut content = div().w(px(width.max(400.) + 100.)).flex().flex_col();
                for index in 0..20 {
                    content = content.child(
                        div()
                            .h(px(36.))
                            .flex_shrink_0()
                            .px(px(12.))
                            .flex()
                            .items_center()
                            .text_size(px(12.))
                            .border_b(px(crate::design::BORDER_WIDTH))
                            .border_color(rgb(CUE_UI.palette.border))
                            .child(format!(
                                "第 {:02} 行　可横向与纵向滚动的示例内容　　　　　　　行尾",
                                index + 1
                            )),
                    );
                }
                self.scroll
                    .render(self.sid("control"), width, 180., content)
            }
            Example::Layout => {
                let ratio = [16. / 9., 1., 4. / 3.][self.variant];
                let art = p::data::aspect_ratio(
                    self.sid("ratio"),
                    ratio,
                    width,
                    div()
                        .size_full()
                        .bg(rgb(CUE_UI.palette.prompt))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(crate::controls::agent_portrait(Some("fox"), 64.)),
                );
                let inset = p::surface(self.sid("inset"), 0., WHITE, true)
                    .w(px(width))
                    .p(px(16.))
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .child(p::data::inset(
                        div()
                            .h(px(36.))
                            .bg(rgb(CUE_UI.palette.prompt))
                            .flex()
                            .items_center()
                            .px(px(16.))
                            .child(note("内容延展到容器边缘")),
                        16.,
                        p::data::Inset::Top,
                    ))
                    .child(note("正文保持内边距。"));
                stack()
                    .child(art)
                    .child(p::data::separator(
                        self.sid("horizontal"),
                        false,
                        false,
                        width,
                    ))
                    .child(
                        row()
                            .child(note("左侧"))
                            .child(p::data::separator(self.sid("vertical"), true, false, 20.))
                            .child(note("右侧")),
                    )
                    .child(inset)
                    .into_any_element()
            }
            _ => unreachable!(),
        }
    }
}
