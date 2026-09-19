use super::{
    super::{
        controls::{self, ActionStyle},
        skin, SurfaceColors,
    },
    label,
};
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    components::text_input::{ComposerEdited, ComposerInput},
    design::{LIQUID_OUTLINE, ZORK_UI},
};
use gpui::{prelude::*, *};
use std::{cell::Cell, rc::Rc};

pub struct CodeEdited {
    pub value: String,
    pub complete: bool,
}
pub struct OneTimeCode {
    input: Entity<ComposerInput>,
    length: usize,
    disabled: bool,
    masked: bool,
    width: f32,
    focus_subscriptions: Vec<Subscription>,
    subscribed_focus: Option<FocusHandle>,
    bounds: Rc<Cell<Bounds<Pixels>>>,
}
impl OneTimeCode {
    pub fn new(length: usize, cx: &mut Context<Self>) -> Self {
        let length = length.clamp(1, 12);
        let input = cx.new(|cx| ComposerInput::new("验证码", cx).numeric_code(length));
        cx.observe(&input, |_, _, cx| cx.notify()).detach();
        cx.subscribe(&input, |v, input, _: &ComposerEdited, cx| {
            let value = input.read(cx).value().to_owned();
            cx.emit(CodeEdited {
                complete: value.len() == v.length,
                value,
            });
        })
        .detach();
        Self {
            input,
            length,
            disabled: false,
            masked: false,
            width: 280.,
            focus_subscriptions: vec![],
            subscribed_focus: None,
            bounds: Default::default(),
        }
    }
    pub fn configure(&mut self, width: f32, disabled: bool, masked: bool, cx: &mut Context<Self>) {
        if (self.width, self.disabled, self.masked) != (width, disabled, masked) {
            self.width = width;
            self.disabled = disabled;
            self.masked = masked;
            self.input
                .update(cx, |input, cx| input.set_editable(disabled, false, cx));
            cx.notify();
        }
    }
    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.input.update(cx, |input, cx| input.clear(cx));
    }
    pub fn inspect(&self, cx: &App) -> serde_json::Value {
        serde_json::json!({"value":self.input.read(cx).value(),"selection":self.input.read(cx).selection(cx),"length":self.length,"disabled":self.disabled})
    }
}
impl EventEmitter<CodeEdited> for OneTimeCode {}
impl Render for OneTimeCode {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focus = self.input.read(cx).focus_handle();
        if self.subscribed_focus.as_ref() != Some(&focus) {
            self.focus_subscriptions.clear();
            self.subscribed_focus = Some(focus.clone());
            self.focus_subscriptions
                .push(cx.on_focus(&focus, window, |_, _, cx| cx.notify()));
            self.focus_subscriptions
                .push(cx.on_blur(&focus, window, |_, _, cx| cx.notify()));
        }
        let value = self.input.read(cx).value().as_bytes().to_vec();
        let selection = self.input.read(cx).selection(cx);
        let cell = ((self.width - 6. * self.length.saturating_sub(1) as f32) / self.length as f32)
            .clamp(20., 40.);
        let mut row = div()
            .id("otp-cells")
            .relative()
            .flex()
            .gap(px(6.))
            .w(px(
                cell * self.length as f32 + 6. * self.length.saturating_sub(1) as f32
            ))
            .h(px(44.))
            .role(Role::Group)
            .aria_label("一次性验证码")
            .when(self.disabled, |v| v.opacity(0.4));
        for index in 0..self.length {
            let selected = if selection.is_empty() {
                index == selection.start.min(self.length - 1)
            } else {
                selection.contains(&index)
            };
            let text = value.get(index).map_or_else(
                || "".to_owned(),
                |c| {
                    if self.masked {
                        "•".to_owned()
                    } else {
                        (*c as char).to_string()
                    }
                },
            );
            let input = self.input.clone();
            let disabled = self.disabled;
            row = row.child(
                skin(
                    format!("otp-cell-{index}"),
                    cell,
                    44.,
                    crate::controls::FIELD_RADIUS,
                    0.6,
                    SurfaceColors {
                        fill: if selected && !selection.is_empty() {
                            ZORK_UI.palette.selected
                        } else {
                            ZORK_UI.palette.canvas
                        },
                        border: Some(LIQUID_OUTLINE),
                        parent: ZORK_UI.palette.canvas,
                        focused: selected && focus.is_focused(window) && !disabled,
                    },
                    div()
                        .size_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(20.))
                        .child(text),
                    window,
                    cx,
                )
                .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                    if !disabled {
                        input.update(cx, |input, cx| {
                            let start = index.min(input.value().len());
                            input.select(start..(start + 1).min(input.value().len()), window, cx);
                            window.focus(&input.focus_handle(), cx);
                        });
                    }
                    cx.stop_propagation();
                })
                .automation_enabled(
                    !disabled,
                    AutomationRole::TextInput,
                    format!("验证码第 {} 位", index + 1),
                ),
            );
        }
        // One real editor owns selection, IME, clipboard and undo. Capture a
        // cell click before the hidden text layout can choose a different caret.
        let measured = self.bounds.clone();
        row.capture_any_mouse_down(cx.listener(move |v, e: &MouseDownEvent, window, cx| {
            if e.button != MouseButton::Left {
                return;
            }
            if !v.disabled {
                let index = (((e.position.x - v.bounds.get().left()).as_f32() / (cell + 6.))
                    .floor()
                    .max(0.) as usize)
                    .min(v.length - 1);
                v.input.update(cx, |input, cx| {
                    let start = index.min(input.value().len());
                    let end = (start + 1).min(input.value().len());
                    input.select(start..end, window, cx);
                    window.focus(&input.focus_handle(), cx);
                });
                cx.notify();
            }
            cx.stop_propagation();
        }))
        .child(
            div()
                .absolute()
                .inset_0()
                .opacity(0.)
                .child(self.input.clone()),
        )
        .child(
            canvas(move |bounds, _, _| measured.set(bounds), |_, _, _, _| {})
                .absolute()
                .inset_0(),
        )
        .into_any_element()
    }
}

pub fn password<V: 'static>(
    id: impl Into<SharedString>,
    input: &Entity<ComposerInput>,
    revealed: bool,
    disabled: bool,
    width: f32,
    window: &mut Window,
    cx: &mut Context<V>,
    change: impl Fn(&mut V, bool, &mut Context<V>) + 'static,
) -> AnyElement {
    let id = id.into();
    let target = input.clone();
    let toggle = controls::action(
        format!("{id}-toggle"),
        if revealed { "隐藏" } else { "显示" },
        52.,
        32.,
        ActionStyle {
            quiet: true,
            disabled,
            ..Default::default()
        },
        ZORK_UI.palette.canvas,
        window,
        cx,
    )
    .aria_toggled(if revealed {
        Toggled::True
    } else {
        Toggled::False
    })
    .aria_label(if revealed {
        "隐藏密码"
    } else {
        "显示密码"
    })
    .on_click(cx.listener(move |v, _, window, cx| {
        if !disabled {
            target.update(cx, |input, cx| input.set_secret(revealed, cx));
            window.focus(&target.read(cx).focus_handle(), cx);
            change(v, !revealed, cx);
        }
    }))
    .automation_enabled(
        !disabled,
        AutomationRole::Button,
        if revealed {
            "隐藏密码"
        } else {
            "显示密码"
        },
    );
    div()
        .id(id.clone())
        .w(px(width))
        .flex()
        .flex_col()
        .gap(px(5.))
        .child(label(
            format!("{id}-label"),
            "密码",
            input.clone(),
            disabled,
        ))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .child(
                    controls::input(
                        format!("{id}-input"),
                        input,
                        (width - 58.).max(40.),
                        32.,
                        false,
                        ZORK_UI.palette.canvas,
                        window,
                        cx,
                    )
                    .automation_enabled(
                        !disabled,
                        AutomationRole::TextInput,
                        "密码",
                    ),
                )
                .child(toggle),
        )
        .into_any_element()
}

/// A field associates its visible label, editor and caller-supplied error. This
/// component never invents validation rules or issues a network submission.
pub fn field(
    id: impl Into<SharedString>,
    title: impl Into<SharedString>,
    input: &Entity<ComposerInput>,
    width: f32,
    height: f32,
    error: Option<SharedString>,
    disabled: bool,
    parent: u32,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let id = id.into();
    let title = title.into();
    let control = controls::input(
        format!("{id}-input"),
        input,
        width,
        height,
        error.is_some(),
        parent,
        window,
        cx,
    )
    .aria_label(title.clone())
    .when_some(error.clone(), |v, error| v.aria_description(error))
    .automation_enabled(!disabled, AutomationRole::TextInput, title.clone());
    div()
        .id(id.clone())
        .w(px(width))
        .flex()
        .flex_col()
        .gap(px(5.))
        .child(label(format!("{id}-label"), title, input.clone(), disabled))
        .child(control)
        .when_some(error, |v, error| {
            v.child(
                div()
                    .id(format!("{id}-error"))
                    .role(Role::Alert)
                    .aria_label(error.clone())
                    .text_size(px(12.))
                    .line_height(px(18.))
                    .text_color(rgb(ZORK_UI.palette.danger))
                    .child(error),
            )
        })
        .into_any_element()
}

pub fn form(id: impl Into<SharedString>, content: impl IntoElement) -> Stateful<Div> {
    div()
        .id(id.into())
        .role(Role::Form)
        .flex()
        .flex_col()
        .gap(px(16.))
        .child(content)
}
