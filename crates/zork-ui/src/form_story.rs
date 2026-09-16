//! Interactive form recipe using production controls and local fixture state.
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    components::text_input::ComposerInput,
    controls as ui,
    design::{TextRole, CUE_UI},
};
use gpui::{div, prelude::*, px, rgb, Context, Entity, FocusHandle, Render, Task, Window};

pub struct FormStory {
    input: Entity<ComposerInput>,
    selected: usize,
    open: bool,
    checked: bool,
    focus: FocusHandle,
    error: Option<String>,
    saved: bool,
    pending: Option<Task<()>>,
}
impl FormStory {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| ComposerInput::new("例如：主力模型", cx));
        Self {
            input,
            selected: 0,
            open: false,
            checked: true,
            focus: cx.focus_handle(),
            error: None,
            saved: false,
            pending: None,
        }
    }
}
impl Render for FormStory {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let busy = self.pending.is_some();
        let connection = ["订阅连接", "API 连接"];
        let choose = ui::dropdown(
            "form-connection",
            connection[self.selected].into(),
            connection
                .iter()
                .enumerate()
                .map(|(i, label)| {
                    (
                        format!("form-option-{i}"),
                        (*label).into(),
                        i == self.selected,
                    )
                })
                .collect(),
            self.open,
            !busy,
            window,
            cx,
            |v, open, cx| {
                v.open = open;
                cx.notify();
            },
            |v, index, cx| {
                v.selected = index;
                v.open = false;
                v.saved = false;
                cx.notify();
            },
        );
        div()
            .w_full()
            .max_w(px(620.))
            .mx_auto()
            .flex()
            .flex_col()
            .gap_5()
            .child(ui::page_title("表单与状态反馈"))
            .child(ui::text_role(
                "使用正式组件，数据仅保存在这份示例中。可试空值保存、切换连接、开关和键盘操作。",
                TextRole::Description,
            ))
            .child(
                ui::section()
                    .border_t_0()
                    .py_0()
                    .child(ui::text_role("模型设置", TextRole::SectionTitle))
                    .child(ui::field_with_error(
                        "form-name",
                        "名称",
                        &self.input,
                        self.error.clone(),
                        cx,
                    ))
                    .child(ui::form_field("连接方式", choose))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(ui::text_role("启用模型", TextRole::Body))
                            .child(ui::switch(
                                "form-enabled",
                                "启用模型",
                                self.checked,
                                !busy,
                                &self.focus,
                                cx,
                                |v, checked, cx| {
                                    v.checked = checked;
                                    cx.notify();
                                },
                            )),
                    )
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .items_center()
                            .child(
                                ui::action_link("form-reset", "恢复初始值", !busy)
                                    .on_click(cx.listener(|v, _, _, cx| {
                                        if v.pending.is_none() {
                                            v.input.update(cx, |input, cx| input.clear(cx));
                                            v.error = None;
                                            v.saved = false;
                                            v.selected = 0;
                                            v.checked = true;
                                            cx.notify();
                                        }
                                    }))
                                    .automation_enabled(
                                        !busy,
                                        AutomationRole::Button,
                                        "恢复初始值",
                                    ),
                            )
                            .child(
                                ui::busy_button("form-save", "保存示例", true, !busy, busy)
                                    .on_click(cx.listener(|v, _, window, cx| {
                                        if v.pending.is_some() {
                                            return;
                                        }
                                        v.saved = false;
                                        if v.input.read(cx).value().trim().is_empty() {
                                            v.error = Some("请填写名称".into());
                                            window.focus(&v.input.read(cx).focus_handle(), cx);
                                        } else {
                                            v.error = None;
                                            v.open = false;
                                            v.pending = Some(cx.spawn(async move |weak, cx| {
                                                cx.background_executor()
                                                    .timer(std::time::Duration::from_millis(700))
                                                    .await;
                                                let _ = weak.update(cx, |v, cx| {
                                                    v.pending = None;
                                                    v.saved = true;
                                                    cx.notify();
                                                });
                                            }));
                                        }
                                        cx.notify();
                                    }))
                                    .automation_enabled(!busy, AutomationRole::Button, "保存示例"),
                            ),
                    )
                    .when(self.saved, |v| {
                        v.child(
                            div()
                                .id("form-saved")
                                .child(ui::status_notice(
                                    "示例已保存".into(),
                                    ui::NoticeKind::Success,
                                ))
                                .automation(AutomationRole::Status, "示例已保存"),
                        )
                    }),
            )
            .child(
                div()
                    .border_t(gpui::px(crate::design::BORDER_WIDTH))
                    .border_color(rgb(CUE_UI.palette.border))
                    .pt_4()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(ui::text_role("状态样本", TextRole::Label))
                    .child(ui::status_notice(
                        "辅助说明保持中性，不与错误混用。".into(),
                        ui::NoticeKind::Info,
                    ))
                    .child(ui::status_notice(
                        "部分内容尚待确认。".into(),
                        ui::NoticeKind::Warning,
                    ))
                    .child(ui::status_notice(
                        "保存失败时保留输入，允许修正后重试。".into(),
                        ui::NoticeKind::Error,
                    )),
            )
    }
}
