use super::super::controls::{self, ActionStyle};
use crate::modal::PlainDialog;
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    design::ZORK_UI,
};
use gpui::{prelude::*, *};
use std::rc::Rc;

pub struct AlertDialog {
    dialog: PlainDialog,
    cancel_label: Option<SharedString>,
}
impl AlertDialog {
    pub fn new(cx: &mut App) -> Self {
        Self {
            dialog: PlainDialog::new(cx).alert(),
            cancel_label: None,
        }
    }
    pub fn cancel_label(&mut self, label: impl Into<SharedString>) {
        self.cancel_label = Some(label.into());
    }
    pub fn inspect(&self) -> serde_json::Value {
        self.dialog.inspect()
    }
    pub fn trigger<V: 'static>(
        &self,
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        width: f32,
        window: &mut Window,
        cx: &mut Context<V>,
        open: impl Fn(&mut V, &mut Window, &mut Context<V>) + 'static,
    ) -> AnyElement {
        let label = label.into();
        self.dialog
            .trigger(
                id,
                label.clone(),
                width,
                ActionStyle::default(),
                ZORK_UI.palette.canvas,
                window,
                cx,
                open,
            )
            .automation(AutomationRole::Button, label)
            .into_any_element()
    }
    pub fn render<V: 'static>(
        &mut self,
        id: impl Into<SharedString>,
        title: impl Into<SharedString>,
        description: impl Into<SharedString>,
        confirm_label: impl Into<SharedString>,
        open: bool,
        busy: bool,
        window: &mut Window,
        cx: &mut Context<V>,
        cancel: impl Fn(&mut V, &mut Window, &mut Context<V>) + 'static,
        confirm: impl Fn(&mut V, &mut Window, &mut Context<V>) + 'static,
    ) -> Option<AnyElement> {
        let id = id.into();
        let title = title.into();
        let confirm_label = confirm_label.into();
        let cancel_label = self.cancel_label.clone().unwrap_or_else(|| "取消".into());
        let cancel_accessible = self
            .cancel_label
            .clone()
            .unwrap_or_else(|| "取消确认".into());
        let cancel = Rc::new(cancel);
        let callback = cancel.clone();
        let cancel_id = format!("{id}-cancel");
        self.dialog
            .initial_focus(controls::action_focus(cancel_id.clone(), window, cx));
        let footer = div()
            .flex()
            .justify_end()
            .gap(px(8.))
            .child(
                controls::action(
                    cancel_id,
                    cancel_label,
                    76.,
                    32.,
                    ActionStyle {
                        disabled: busy,
                        ..Default::default()
                    },
                    ZORK_UI.palette.canvas,
                    window,
                    cx,
                )
                .on_click(cx.listener(move |v, _, w, cx| {
                    if !busy {
                        callback(v, w, cx);
                    }
                }))
                .automation_enabled(
                    !busy,
                    AutomationRole::Button,
                    cancel_accessible,
                ),
            )
            .child(
                controls::action(
                    format!("{id}-confirm"),
                    confirm_label.clone(),
                    104.,
                    32.,
                    ActionStyle {
                        primary: true,
                        busy,
                        ..Default::default()
                    },
                    ZORK_UI.palette.canvas,
                    window,
                    cx,
                )
                .on_click(cx.listener(move |v, _, w, cx| {
                    if !busy {
                        confirm(v, w, cx);
                    }
                }))
                .automation_enabled(!busy, AutomationRole::Button, confirm_label),
            );
        self.dialog.render(
            id,
            title,
            div()
                .text_size(px(13.))
                .line_height(px(22.))
                .child(description.into()),
            Some(footer.into_any_element()),
            open,
            window,
            cx,
            move |v, w, cx| {
                if !busy {
                    cancel(v, w, cx);
                }
            },
        )
    }
}
