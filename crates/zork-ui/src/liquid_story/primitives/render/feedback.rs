use super::*;
impl Specimen {
    pub(super) fn feedback_example(
        &mut self,
        width: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match self.example {
            Example::Progress => {
                let progress = p::range::progress(
                    self.sid("control"),
                    if self.variant == 0 {
                        Some(self.values[0])
                    } else {
                        None
                    },
                    100.,
                    width,
                );
                let decrease = self
                    .button("decrease", "减小", false, window, cx)
                    .on_click(cx.listener(|v, _, _, cx| {
                        v.values[0] = (v.values[0] - 10.).max(0.);
                        cx.notify();
                    }))
                    .automation(AutomationRole::Button, "减小进度");
                let increase = self
                    .button("increase", "增加", false, window, cx)
                    .on_click(cx.listener(|v, _, _, cx| {
                        v.values[0] = (v.values[0] + 10.).min(100.);
                        cx.notify();
                    }))
                    .automation(AutomationRole::Button, "增加进度");
                stack()
                    .child(progress)
                    .child(note(if self.variant == 0 {
                        format!(
                            "{:.0}%{}",
                            self.values[0],
                            if self.values[0] == 100. {
                                " · 已完成"
                            } else {
                                ""
                            }
                        )
                    } else {
                        "正在处理…".into()
                    }))
                    .when(self.variant == 0, |v| {
                        v.child(row().child(decrease).child(increase))
                    })
                    .into_any_element()
            }
            Example::Toast => {
                let add = self
                    .button("add", "显示通知", true, window, cx)
                    .on_click(cx.listener(|v, _, _, cx| v.add_toasts(1, cx)))
                    .automation(AutomationRole::Button, "显示通知");
                let queue = self
                    .button("queue", "连续添加 6 条", false, window, cx)
                    .on_click(cx.listener(|v, _, _, cx| v.add_toasts(6, cx)))
                    .automation(AutomationRole::Button, "连续添加六条通知");
                stack()
                    .child(row().child(add).child(queue))
                    .child(note("最多保留 4 条；悬停或聚焦时暂停计时，F8 进入通知。"))
                    .child(self.toasts.clone())
                    .into_any_element()
            }
            Example::Badge => row()
                .child(p::feedback::badge(
                    self.sid("neutral"),
                    "草稿",
                    NoticeKind::Info,
                ))
                .child(p::feedback::badge(
                    self.sid("success"),
                    "已完成",
                    NoticeKind::Success,
                ))
                .child(p::feedback::badge(
                    self.sid("warning"),
                    "需关注",
                    NoticeKind::Warning,
                ))
                .child(p::feedback::badge(
                    self.sid("error"),
                    "失败",
                    NoticeKind::Error,
                ))
                .into_any_element(),
            Example::Skeleton => {
                let switch = self
                    .button(
                        "loaded",
                        if self.open {
                            "重新加载"
                        } else {
                            "显示内容"
                        },
                        false,
                        window,
                        cx,
                    )
                    .on_click(cx.listener(|v, _, _, cx| {
                        v.open = !v.open;
                        cx.notify();
                    }))
                    .automation(AutomationRole::Button, "切换加载状态");
                let content = if self.open {
                    row()
                        .flex_nowrap()
                        .child(p::data::avatar(
                            self.sid("portrait"),
                            "小狐",
                            p::data::AvatarSource::Portrait("fox"),
                            40.,
                        ))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap(px(6.))
                                .child(
                                    div()
                                        .text_size(px(13.))
                                        .line_height(px(18.))
                                        .child("组件示例"),
                                )
                                .child(note("内容准备完成。")),
                        )
                        .into_any_element()
                } else {
                    row()
                        .flex_nowrap()
                        .child(p::feedback::skeleton(self.sid("avatar"), 40., 40., true))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(10.))
                                .child(p::feedback::skeleton(
                                    self.sid("title"),
                                    (width - 50.).min(150.),
                                    18.,
                                    false,
                                ))
                                .child(p::feedback::skeleton(
                                    self.sid("line"),
                                    (width - 50.).max(30.),
                                    12.,
                                    false,
                                )),
                        )
                        .into_any_element()
                };
                stack().child(content).child(switch).into_any_element()
            }
            Example::Spinner => {
                let mut body = row();
                for size in [12., 16., 24.] {
                    body = body.child(
                        row()
                            .child(
                                crate::components::loading::indicator(
                                    self.sid(&format!("size-{size:.0}")),
                                    size,
                                )
                                .without_delay(),
                            )
                            .child(note(format!("{size:.0}px"))),
                    );
                }
                stack()
                    .child(body)
                    .child(p::feedback::notice_content(
                        self.sid("loading"),
                        "正在读取内容…",
                        NoticeKind::Loading,
                        None,
                    ))
                    .into_any_element()
            }
            _ => unreachable!(),
        }
    }
    fn add_toasts(&mut self, count: usize, cx: &mut Context<Self>) {
        for _ in 0..count {
            self.commits += 1;
            let serial = self.commits;
            let timeout = (self.variant == 0).then(|| std::time::Duration::from_secs(4));
            self.toasts.update(cx, |toasts, cx| {
                toasts.push(
                    format!("已更新样例 {serial}"),
                    Some("可以撤销这次示例操作。".into()),
                    NoticeKind::Success,
                    Some(p::feedback::ToastAction {
                        key: format!("undo-{serial}").into(),
                        label: "撤销".into(),
                    }),
                    timeout,
                    cx,
                );
            });
        }
        cx.notify();
    }
}
