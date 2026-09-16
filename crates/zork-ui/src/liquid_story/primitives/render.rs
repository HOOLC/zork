use super::*;
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    components::liquid::controls::{self, ActionStyle},
    controls::NoticeKind,
    design::CUE_UI,
};

mod content;
mod feedback;
mod inputs;
mod overlays;
mod selection;

const WHITE: u32 = CUE_UI.palette.canvas;
fn stack() -> Div {
    div().flex().flex_col().gap(px(16.)).min_w_0()
}
fn row() -> Div {
    div().flex().flex_wrap().items_center().gap(px(10.))
}
fn note(text: impl Into<SharedString>) -> Div {
    let text: SharedString = text.into();
    let text = text.trim_end_matches('。').to_owned();
    div()
        .text_size(px(12.))
        .line_height(px(20.))
        .text_color(rgb(CUE_UI.palette.muted))
        .child(text)
}
impl Specimen {
    fn button(
        &self,
        key: &str,
        title: &str,
        primary: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> Stateful<Div> {
        controls::action(
            self.sid(key),
            title.to_owned(),
            liquid::overlay::measure_label(title, 12., window) + 32.,
            32.,
            ActionStyle {
                primary,
                disabled: self.disabled,
                ..Default::default()
            },
            WHITE,
            window,
            cx,
        )
    }
    fn variants(&self) -> &'static [&'static str] {
        match self.example {
            Example::Checkbox => &["未选", "选中", "半选"],
            Example::ToggleGroup => &["单选", "多选"],
            Example::Slider => &["单值", "区间", "纵向"],
            Example::Progress => &["确定", "不确定"],
            Example::Toast => &["自动关闭", "手动关闭"],
            Example::Accordion => &["单开", "多开", "始终展开"],
            Example::Tabs => &["自动激活", "手动激活", "链接导航"],
            Example::Avatar => &["图像", "回退", "加载"],
            Example::Layout => &["16:9", "1:1", "4:3"],
            Example::Otp => &["明文", "隐藏"],
            Example::Form => &["普通", "错误"],
            Example::TextArea => &["编辑", "只读"],
            _ => &[],
        }
    }
    fn set_variant(&mut self, index: usize, cx: &mut Context<Self>) {
        self.variant = index;
        match self.example {
            Example::Checkbox => {
                self.checked = [
                    p::selection::Checked::Off,
                    p::selection::Checked::On,
                    p::selection::Checked::Mixed,
                ][index]
            }
            Example::Slider => {
                self.values = if index == 1 {
                    vec![25., 75.]
                } else {
                    vec![35.]
                }
            }
            Example::ToggleGroup if index == 0 => self.selected.truncate(1),
            Example::Accordion => {
                if index != 1 {
                    self.selected.truncate(1);
                }
                if index == 2 && self.selected.is_empty() {
                    self.selected.push(0);
                }
            }
            _ => {}
        }
        cx.notify();
    }
    fn supports_disabled(&self) -> bool {
        matches!(
            self.example,
            Example::Checkbox
                | Example::CheckboxGroup
                | Example::CheckboxCards
                | Example::RadioCards
                | Example::Toggle
                | Example::ToggleGroup
                | Example::Slider
                | Example::Collapsible
                | Example::Otp
                | Example::Password
                | Example::Form
                | Example::TextArea
        )
    }
}
impl Render for Specimen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let width = (self.width - 36.).max(120.);
        let mut contents = stack()
            .id(self.sid("example"))
            .w(px(self.width))
            .p(px(18.))
            .text_color(rgb(CUE_UI.palette.text));
        let labels = self.variants();
        if !labels.is_empty() {
            let measured = labels
                .iter()
                .map(|s| liquid::overlay::measure_label(s, 12., window) + 24.)
                .fold(0f32, f32::max)
                * labels.len() as f32
                + 8.;
            contents = contents.child(controls::segmented(
                self.sid("variants"),
                measured.min(width),
                labels
                    .iter()
                    .enumerate()
                    .map(|(i, s)| (self.sid(&format!("variant-{i}")), (*s).into()))
                    .collect(),
                self.variant,
                true,
                WHITE,
                window,
                cx,
                |v, i, cx| v.set_variant(i, cx),
            ));
        }
        if self.supports_disabled() {
            contents = contents.child(
                row().child(note("禁用")).child(
                    controls::toggle(
                        self.sid("disabled"),
                        self.disabled,
                        true,
                        WHITE,
                        window,
                        cx,
                        |v, on, cx| {
                            v.disabled = on;
                            cx.notify();
                        },
                    )
                    .automation(AutomationRole::Button, "禁用组件"),
                ),
            );
        }
        let body = match self.example {
            Example::Checkbox
            | Example::CheckboxGroup
            | Example::CheckboxCards
            | Example::RadioCards
            | Example::Toggle
            | Example::ToggleGroup
            | Example::Slider => self.selection_example(width, window, cx),
            Example::Progress
            | Example::Toast
            | Example::Badge
            | Example::Skeleton
            | Example::Spinner => self.feedback_example(width, window, cx),
            Example::Accordion
            | Example::Collapsible
            | Example::Dialog
            | Example::AlertDialog
            | Example::ContextMenu
            | Example::Menubar
            | Example::Popover
            | Example::Tooltip
            | Example::HoverCard => self.overlay_example(width, window, cx),
            Example::Tabs
            | Example::Toolbar
            | Example::NavigationMenu
            | Example::Avatar
            | Example::DataList
            | Example::Table
            | Example::ScrollArea
            | Example::Layout => self.content_example(width, window, cx),
            Example::Otp | Example::Password | Example::Form | Example::TextArea => {
                self.input_example(width, window, cx)
            }
        };
        contents = contents.child(body).when(!self.status.is_empty(), |v| {
            v.child(
                note(self.status.clone())
                    .id(self.sid("status"))
                    .automation(AutomationRole::Status, "操作结果"),
            )
        });
        contents.on_key_down(cx.listener(|v, e: &KeyDownEvent, w, cx| {
            if v.example == Example::Toast && e.keystroke.key == "f8" {
                v.toasts.update(cx, |t, cx| t.focus(w, cx));
                w.prevent_default();
                cx.stop_propagation();
            }
        }))
    }
}
