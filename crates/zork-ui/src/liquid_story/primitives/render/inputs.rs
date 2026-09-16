use super::*;
impl Specimen {
    pub(super) fn input_example(
        &mut self,
        width: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let readonly = self.example == Example::TextArea && self.variant == 1;
        self.input.update(cx, |input, cx| {
            input.set_editable(self.disabled, readonly, cx)
        });
        self.second
            .update(cx, |input, cx| input.set_editable(self.disabled, false, cx));
        match self.example {
            Example::Otp => {
                self.otp.update(cx, |otp, cx| {
                    otp.configure(width, self.disabled, self.variant == 1, cx)
                });
                let clear = self
                    .button("clear", "清空", false, window, cx)
                    .on_click(cx.listener(|v, _, _, cx| {
                        if !v.disabled {
                            v.otp.update(cx, |otp, cx| otp.clear(cx));
                        }
                    }))
                    .automation_enabled(!self.disabled, AutomationRole::Button, "清空验证码");
                stack()
                    .child(self.otp.clone())
                    .child(note("输入或粘贴 6 位数字；点击某一格可替换该位。"))
                    .child(clear)
                    .into_any_element()
            }
            Example::Password => p::input::password(
                self.sid("control"),
                &self.input,
                self.open,
                self.disabled,
                width,
                window,
                cx,
                |v, on, cx| {
                    v.open = on;
                    cx.notify();
                },
            ),
            Example::TextArea => stack()
                .child(p::input::field(
                    self.sid("control"),
                    "备注",
                    &self.input,
                    width,
                    112.,
                    None,
                    self.disabled,
                    WHITE,
                    window,
                    cx,
                ))
                .child(note(if readonly {
                    "只读状态仍可选择和复制文字。"
                } else {
                    "Enter 换行，支持中文输入与撤销。"
                }))
                .into_any_element(),
            Example::Form => {
                let error =
                    (self.variant == 1).then(|| "这个名称已被使用，请选择其他名称。".into());
                let first = p::input::field(
                    self.sid("name"),
                    "名称",
                    &self.input,
                    width,
                    32.,
                    error,
                    self.disabled,
                    WHITE,
                    window,
                    cx,
                );
                let second = p::input::field(
                    self.sid("description"),
                    "简介",
                    &self.second,
                    width,
                    32.,
                    None,
                    self.disabled,
                    WHITE,
                    window,
                    cx,
                );
                let submit = self
                    .button("submit", "保存", true, window, cx)
                    .on_click(cx.listener(|v, _, _, cx| {
                        if !v.disabled {
                            v.actions += 1;
                            v.status = "已收到示例表单内容".into();
                            cx.notify();
                        }
                    }))
                    .automation_enabled(!self.disabled, AutomationRole::Button, "保存表单");
                p::input::form(
                    self.sid("form"),
                    stack()
                        .child(first)
                        .child(second)
                        .child(row().child(submit)),
                )
                .into_any_element()
            }
            _ => unreachable!(),
        }
    }
}
