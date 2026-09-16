use super::*;
use p::selection::{Checked, Item, Mode};
impl Specimen {
    pub(super) fn selection_example(
        &mut self,
        width: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match self.example {
            Example::Checkbox => stack()
                .child(p::selection::checkbox(
                    self.sid("control"),
                    "接收活动通知",
                    self.checked,
                    self.disabled,
                    WHITE,
                    window,
                    cx,
                    |v, checked, cx| {
                        v.checked = checked;
                        v.variant = if checked == Checked::On { 1 } else { 0 };
                        cx.notify();
                    },
                ))
                .child(note(format!(
                    "当前状态：{}",
                    match self.checked {
                        Checked::Off => "未选中",
                        Checked::On => "已选中",
                        Checked::Mixed => "部分选中",
                    }
                )))
                .into_any_element(),
            Example::CheckboxGroup
            | Example::CheckboxCards
            | Example::RadioCards
            | Example::ToggleGroup => {
                let toggles = self.example == Example::ToggleGroup;
                let radio = self.example == Example::RadioCards;
                let items = if toggles {
                    vec![
                        Item::new(self.sid("item-0"), "加粗"),
                        Item::new(self.sid("item-1"), "斜体"),
                        Item::new(self.sid("item-2"), "删除线").disabled(),
                        Item::new(self.sid("item-3"), "下划线"),
                    ]
                } else {
                    vec![
                        Item::new(self.sid("item-0"), "桌面通知").description("在桌面显示活动提醒"),
                        Item::new(self.sid("item-1"), "声音提醒")
                            .description("有新活动时播放提示音"),
                        Item::new(self.sid("item-2"), "邮件摘要")
                            .description("暂不可用")
                            .disabled(),
                    ]
                };
                let mode = if radio {
                    Mode::Radio
                } else if toggles {
                    if self.variant == 0 {
                        Mode::SingleToggle
                    } else {
                        Mode::MultipleToggle
                    }
                } else {
                    Mode::Checkboxes
                };
                let mut body = stack();
                if self.example == Example::CheckboxGroup {
                    let all = Checked::aggregate(&[
                        self.selected.contains(&0),
                        self.selected.contains(&1),
                    ]);
                    body = body.child(p::selection::checkbox(
                        self.sid("all"),
                        "全选可用项目",
                        all,
                        self.disabled,
                        WHITE,
                        window,
                        cx,
                        |v, next, cx| {
                            v.selected = if next == Checked::On {
                                vec![0, 1]
                            } else {
                                vec![]
                            };
                            cx.notify();
                        },
                    ));
                }
                body.child(p::selection::group(
                    self.sid("group"),
                    items,
                    self.selected.clone(),
                    mode,
                    matches!(self.example, Example::CheckboxCards | Example::RadioCards),
                    width,
                    self.disabled,
                    window,
                    cx,
                    |v, values, cx| {
                        v.selected = values;
                        cx.notify();
                    },
                ))
                .child(note(if self.selected.is_empty() {
                    "尚未选择".into()
                } else {
                    format!(
                        "已选：{}",
                        self.selected
                            .iter()
                            .map(|i| (i + 1).to_string())
                            .collect::<Vec<_>>()
                            .join("、")
                    )
                }))
                .into_any_element()
            }
            Example::Toggle => stack()
                .child(p::selection::toggle(
                    self.sid("control"),
                    "固定",
                    self.open,
                    self.disabled,
                    76.,
                    window,
                    cx,
                    |v, on, cx| {
                        v.open = on;
                        cx.notify();
                    },
                ))
                .child(note(if self.open {
                    "已固定，再次点击取消"
                } else {
                    "点击固定"
                }))
                .into_any_element(),
            Example::Slider => {
                let scale = p::range::Scale {
                    step: 5.,
                    minimum_gap: 10.,
                    vertical: self.variant == 2,
                    ..Default::default()
                };
                stack()
                    .child(p::range::slider(
                        self.sid("control"),
                        "数值",
                        self.values.clone(),
                        scale,
                        if self.variant == 2 { 180. } else { width },
                        self.disabled,
                        window,
                        cx,
                        |v, values, commit, cx| {
                            v.values = values;
                            if commit {
                                v.commits += 1;
                            }
                            cx.notify();
                        },
                    ))
                    .child(note(format!(
                        "{}  ·  已提交 {} 次",
                        self.values
                            .iter()
                            .map(|v| format!("{v:.0}"))
                            .collect::<Vec<_>>()
                            .join(" – "),
                        self.commits
                    )))
                    .into_any_element()
            }
            _ => unreachable!(),
        }
    }
}
