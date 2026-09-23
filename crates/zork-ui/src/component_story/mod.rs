//! Native design fixtures built from production controls.
pub mod business;

use crate::{
    automation::{AutomationElementExt, AutomationRole},
    controls as ui,
    modal::PlainDialog,
    new_chat,
    resources::Text,
};
use gpui::{prelude::*, *};
use serde_json::{json, Value};
use std::rc::Rc;

pub struct Gallery {
    dialog: PlainDialog,
    open: bool,
}

impl Gallery {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            dialog: PlainDialog::new(cx),
            open: false,
        }
    }

    pub fn inspect(&self, _: &App) -> Value {
        json!({"dialog": self.dialog.inspect(), "panel": self.open.then_some("library")})
    }
}

impl Render for Gallery {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = (0..80).map(|index| {
            ui::quiet_button(
                format!("component-gallery-row-{index}"),
                format!("组件示例 {index}"),
                true,
                ui::IconButtonSize::Small,
            )
            .w_full()
            .justify_start()
            .into_any_element()
        });
        let directory = (0..80).map(|index| {
            ui::quiet_button(
                format!("component-gallery-directory-row-{index}"),
                format!("控件 {index}"),
                true,
                ui::IconButtonSize::Small,
            )
            .w_full()
            .justify_start()
            .into_any_element()
        });
        let dialog = self.dialog.render(
            "component-gallery-dialog",
            "组件目录",
            div()
                .id("component-gallery-directory")
                .flex()
                .flex_col()
                .children(directory),
            None,
            self.open,
            window,
            cx,
            |view, _, cx| {
                view.open = false;
                cx.notify();
            },
        );
        div()
            .id("component-gallery")
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(crate::design::ZORK_UI.palette.canvas))
            .child(
                div()
                    .h(px(52.))
                    .flex_shrink_0()
                    .px_4()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(ui::page_title("组件展示"))
                    .child(
                        ui::button("component-gallery-toggle", "打开组件目录", false, true)
                            .on_click(cx.listener(|view, _, _, cx| {
                                view.open = true;
                                cx.notify();
                            }))
                            .automation(AutomationRole::Button, "打开组件目录"),
                    ),
            )
            .child(
                div()
                    .id("component-gallery-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_4()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .children(rows),
            )
            .children(dialog)
    }
}

/// The composer story mounts the same new-chat page used by the desktop client.
pub struct ComposerExample {
    page: Entity<new_chat::Page>,
}
impl ComposerExample {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let text = Text(Rc::new(|key| key.into()));
        let page = cx.new(|cx| new_chat::Page::new(text.clone(), cx));
        page.update(cx, |page, cx| {
            page.configure(
                zork_client_types::new_chat::Snapshot {
                    editable: true,
                    can_submit: true,
                    ..Default::default()
                },
                480.,
                text,
                cx,
            );
        });
        Self { page }
    }
    pub fn inspect(&self, cx: &App) -> Value {
        self.page.read(cx).inspect()
    }
}
impl Render for ComposerExample {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.page.clone())
    }
}
