use super::*;
impl Specimen {
    pub(super) fn overlay_example(
        &mut self,
        width: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match self.example {
            Example::Accordion => {
                let input = self.input.clone();
                let field_id = self.sid("body-field");
                let items = vec![
                    p::disclosure::Item::new(self.sid("item-0"), "组件说明", |_, _, _, _| {
                        note("每一项保留自己的内容；展开后可以继续操作。").into_any_element()
                    }),
                    p::disclosure::Item::new(
                        self.sid("item-1"),
                        "编辑备注",
                        move |interactive, content_width, w, cx| {
                            input.update(cx, |input, cx| {
                                input.set_editable(!interactive, false, cx)
                            });
                            p::input::field(
                                field_id,
                                "备注",
                                &input,
                                content_width,
                                32.,
                                None,
                                !interactive,
                                ZORK_UI.palette.prompt,
                                w,
                                cx,
                            )
                        },
                    ),
                    p::disclosure::Item::new(self.sid("item-2"), "暂不可用", |_, _, _, _| {
                        div().into_any_element()
                    })
                    .disabled(),
                ];
                p::disclosure::accordion(
                    self.sid("control"),
                    items,
                    self.selected.clone(),
                    p::disclosure::AccordionMode {
                        multiple: self.variant == 1,
                        collapsible: self.variant != 2,
                    },
                    width,
                    window,
                    cx,
                    |v, expanded, cx| {
                        v.selected = expanded;
                        cx.notify();
                    },
                )
            }
            Example::Collapsible => {
                let input = self.input.clone();
                let field_id = self.sid("body-field");
                let disabled = self.disabled;
                p::disclosure::collapsible(
                    self.sid("control"),
                    "补充内容",
                    move |interactive, content_width, w, cx| {
                        let disabled = disabled || !interactive;
                        input.update(cx, |input, cx| input.set_editable(disabled, false, cx));
                        stack()
                            .child(note("这里的高度由实际内容决定。"))
                            .child(p::input::field(
                                field_id,
                                "备注",
                                &input,
                                content_width,
                                32.,
                                None,
                                disabled,
                                ZORK_UI.palette.prompt,
                                w,
                                cx,
                            ))
                            .into_any_element()
                    },
                    self.open,
                    self.disabled,
                    width,
                    window,
                    cx,
                    |v, open, cx| {
                        v.open = open;
                        cx.notify();
                    },
                )
            }
            Example::Dialog => {
                let trigger = self
                    .dialog
                    .trigger(
                        self.sid("trigger"),
                        "打开对话框",
                        128.,
                        ActionStyle::default(),
                        WHITE,
                        window,
                        cx,
                        |v, _, cx| {
                            v.open = true;
                            cx.notify();
                        },
                    )
                    .automation(AutomationRole::Button, "打开对话框");
                let field = p::input::field(
                    self.sid("dialog-field"),
                    "名称",
                    &self.input,
                    (window.viewport_size().width.as_f32() - 96.).clamp(80., 356.),
                    32.,
                    None,
                    !self.open,
                    WHITE,
                    window,
                    cx,
                );
                let add = self
                    .button("add-content", "添加说明", false, window, cx)
                    .on_click(cx.listener(|v, _, _, cx| {
                        v.actions += 1;
                        cx.notify();
                    }))
                    .automation(AutomationRole::Button, "添加说明");
                let mut body = stack().child(field).child(row().child(add));
                for _ in 0..self.actions.min(8) {
                    body = body.child(note("增加一段说明，面板会随内容调整高度。"));
                }
                let done = self
                    .button("done", "完成", true, window, cx)
                    .on_click(cx.listener(|v, _, _, cx| {
                        v.open = false;
                        v.status = "已关闭对话框".into();
                        cx.notify();
                    }))
                    .automation(AutomationRole::Button, "完成对话框");
                let panel = self.dialog.render(
                    self.sid("panel"),
                    "编辑示例",
                    body,
                    Some(row().justify_end().child(done).into_any_element()),
                    self.open,
                    liquid::overlay::Placement::Window { width: 420. },
                    Material::ordinary(),
                    window,
                    cx,
                    |v, _, cx| {
                        v.open = false;
                        cx.notify();
                    },
                );
                stack()
                    .child(trigger)
                    .when_some(panel, |v, panel| v.child(panel))
                    .into_any_element()
            }
            Example::AlertDialog => {
                let trigger = self.alert.trigger(
                    self.sid("trigger"),
                    "清空示例",
                    112.,
                    window,
                    cx,
                    |v, _, cx| {
                        v.open = true;
                        cx.notify();
                    },
                );
                let panel = self.alert.render(
                    self.sid("panel"),
                    "清空这段内容？",
                    "确认后将清空下方的样例文字。",
                    "确认清空",
                    self.open,
                    false,
                    Material::ordinary(),
                    window,
                    cx,
                    |v, _, cx| {
                        v.open = false;
                        cx.notify();
                    },
                    |v, _, cx| {
                        v.input.update(cx, |input, cx| input.clear(cx));
                        v.open = false;
                        v.actions += 1;
                        v.status = "已清空示例内容".into();
                        cx.notify();
                    },
                );
                stack()
                    .child(note(if self.input.read(cx).value().is_empty() {
                        "（内容为空）".into()
                    } else {
                        self.input.read(cx).value().to_owned()
                    }))
                    .child(row().child(trigger))
                    .when_some(panel, |v, panel| v.child(panel))
                    .into_any_element()
            }
            Example::ContextMenu => {
                let trigger = self.menu.context_trigger(
                    self.sid("trigger"),
                    note("在这里右键、长按，或按 Shift F10"),
                    width,
                    window,
                    cx,
                );
                let items = self.menu_items();
                let menu =
                    self.menu
                        .render(self.sid("menu"), items, window, cx, |v, key, _, cx| {
                            v.choose_menu(key, cx)
                        });
                stack()
                    .child(trigger)
                    .when_some(menu, |v, menu| v.child(menu))
                    .into_any_element()
            }
            Example::Menubar => {
                let groups = vec![
                    p::menu::Group::new(self.sid("file"), "文件", self.menu_items()),
                    p::menu::Group::new(
                        self.sid("edit"),
                        "编辑",
                        vec![
                            p::menu::Item::new("undo", "撤销"),
                            p::menu::Item::new("redo", "重做").disabled(),
                        ],
                    ),
                    p::menu::Group::new(
                        self.sid("view"),
                        "视图",
                        vec![
                            p::menu::Item::new("check", "显示辅助线")
                                .check(self.checked == p::selection::Checked::On),
                            p::menu::Item::new("compact", "紧凑").radio(self.variant == 1),
                            p::menu::Item::new("comfortable", "宽松").radio(self.variant != 1),
                        ],
                    ),
                ];
                self.menubar
                    .render(self.sid("control"), groups, window, cx, |v, key, _, cx| {
                        v.choose_menu(key, cx)
                    })
            }
            Example::Popover => {
                let trigger =
                    self.flyout
                        .trigger(self.sid("trigger"), "显示选项", 112., window, cx);
                let panel_width = 280f32.min(window.viewport_size().width.as_f32() - 24.);
                let input = self.input.clone();
                let field_id = self.sid("popover-field");
                let check_id = self.sid("popover-check");
                let close_id = self.sid("close");
                let checked = self.checked;
                let panel = self.flyout.render(
                    self.sid("panel"),
                    "显示选项",
                    move |interactive, content_width, w, cx| {
                        input.update(cx, |input, cx| input.set_editable(!interactive, false, cx));
                        let field = p::input::field(
                            field_id,
                            "标题",
                            &input,
                            content_width,
                            32.,
                            None,
                            !interactive,
                            WHITE,
                            w,
                            cx,
                        );
                        let check = p::selection::checkbox(
                            check_id,
                            "显示说明",
                            checked,
                            !interactive,
                            WHITE,
                            w,
                            cx,
                            |v, checked, cx| {
                                v.checked = checked;
                                cx.notify();
                            },
                        );
                        let close = controls::action(
                            close_id,
                            "完成",
                            56.,
                            32.,
                            ActionStyle {
                                primary: true,
                                disabled: !interactive,
                                ..Default::default()
                            },
                            WHITE,
                            w,
                            cx,
                        )
                        .on_click(cx.listener(move |v, _, w, cx| {
                            if interactive {
                                v.flyout.close(w, cx);
                                v.actions += 1;
                                cx.notify();
                            }
                        }))
                        .automation_enabled(
                            interactive,
                            AutomationRole::Button,
                            "完成浮层",
                        );
                        stack()
                            .child(field)
                            .child(check)
                            .child(row().child(close))
                            .into_any_element()
                    },
                    panel_width,
                    window,
                    cx,
                );
                stack()
                    .child(trigger)
                    .when_some(panel, |v, panel| v.child(panel))
                    .into_any_element()
            }
            Example::Tooltip => {
                let id = self.sid("trigger");
                let focus = controls::action_focus(id.clone(), window, cx);
                let trigger = self
                    .button("trigger", "复制", false, window, cx)
                    .on_click(cx.listener(|v, _, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string("文字提示示例".into()));
                        v.actions += 1;
                        v.status = "已复制".into();
                        cx.notify();
                    }))
                    .automation(AutomationRole::Button, "复制提示示例");
                stack()
                    .child(
                        row().child(
                            crate::components::tooltip::hint(trigger, id, "复制到剪贴板")
                                .focus_handle(&focus),
                        ),
                    )
                    .child(note("悬停或用 Tab 聚焦按钮。"))
                    .into_any_element()
            }
            Example::HoverCard => {
                let trigger = self
                    .button("trigger", "小狐", false, window, cx)
                    .automation(AutomationRole::Button, "查看小狐详情");
                let details = crate::components::tooltip::DetailsTooltip {
                    key: self.sid("details"),
                    title: "小狐".into(),
                    kind: "示例成员".into(),
                    avatar: Some("fox".into()),
                    description: "这是一张只读详情卡，可从触发器移动到卡片中查看。".into(),
                    rows: vec![
                        ("状态".into(), "可用".into()),
                        ("职责".into(), "设计与协作".into()),
                    ],
                };
                stack()
                    .child(row().child(crate::components::tooltip::trigger(
                        trigger,
                        details,
                        self.details.clone(),
                    )))
                    .child(self.details.clone())
                    .into_any_element()
            }
            _ => unreachable!(),
        }
    }
    fn menu_items(&self) -> Vec<p::menu::Item> {
        use p::menu::Item;
        vec![
            Item::heading("heading", "示例操作"),
            Item::new("copy", "复制").text_value("copy"),
            Item::new("duplicate", "创建副本"),
            Item::new("delete", "删除").disabled(),
            Item::separator("separator"),
            Item::new("check", "显示辅助线").check(self.checked == p::selection::Checked::On),
            Item::new("share", "分享").text_value("share").submenu(vec![
                Item::new("link", "复制链接"),
                Item::new("export", "导出示例"),
                Item::new("publish", "发布").disabled(),
            ]),
        ]
    }
    fn choose_menu(&mut self, key: String, cx: &mut Context<Self>) {
        match key.as_str() {
            "check" => self.checked = self.checked.next(),
            "compact" => self.variant = 1,
            "comfortable" => self.variant = 0,
            "copy" | "link" => {
                cx.write_to_clipboard(ClipboardItem::new_string("菜单示例内容".into()))
            }
            _ => {}
        }
        self.actions += 1;
        self.status = format!(
            "已选择：{}",
            match key.as_str() {
                "copy" => "复制",
                "duplicate" => "创建副本",
                "check" => "辅助线",
                "link" => "复制链接",
                "export" => "导出示例",
                "undo" => "撤销",
                "compact" => "紧凑",
                "comfortable" => "宽松",
                _ => "操作",
            }
        );
        cx.notify();
    }
}
