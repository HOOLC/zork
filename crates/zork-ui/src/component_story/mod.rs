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
        let directory = gpui::uniform_list(
            "component-gallery-directory",
            80,
            cx.processor(|_, range: std::ops::Range<usize>, _, _| {
                range
                    .map(|index| {
                        let label = format!("控件 {index}");
                        div().h(px(40.)).pb_1().child(
                            ui::quiet_button(
                                format!("component-gallery-directory-row-{index}"),
                                label.clone(),
                                true,
                                ui::IconButtonSize::Small,
                            )
                            .w_full()
                            .justify_start()
                            .automation(AutomationRole::Button, label),
                        )
                    })
                    .collect()
            }),
        )
        .h(px(480.))
        .min_h_0();
        let dialog = self.dialog.render(
            "component-gallery-dialog",
            "组件目录",
            div()
                .id("component-gallery-directory-body")
                .child(directory),
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
                gpui::uniform_list(
                    "component-gallery-scroll",
                    80,
                    cx.processor(|_, range: std::ops::Range<usize>, _, _| {
                        range
                            .map(|index| {
                                let label = format!("组件示例 {index}");
                                div().h(px(40.)).pb_1().child(
                                    ui::quiet_button(
                                        format!("component-gallery-row-{index}"),
                                        label.clone(),
                                        true,
                                        ui::IconButtonSize::Small,
                                    )
                                    .w_full()
                                    .justify_start()
                                    .automation(AutomationRole::Button, label),
                                )
                            })
                            .collect()
                    }),
                )
                .flex_1()
                .min_h_0()
                .p_4(),
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

/// The conversation composer: the shared widget with the automation ids the
/// desktop conversation mounts (`composer-surface`, `composer-input`, …).
pub struct ConversationComposer {
    input: Entity<crate::components::text_input::ComposerInput>,
    scene: crate::components::widgets::composer::Scene,
    width: f32,
}
impl ConversationComposer {
    pub fn new(cx: &mut Context<Self>) -> Self {
        use crate::components::text_input::{ComposerInput, ComposerLayoutChanged};
        let input = cx.new(|cx| ComposerInput::new("输入消息", cx));
        cx.subscribe(&input, |_, _, _: &ComposerLayoutChanged, cx| cx.notify())
            .detach();
        Self {
            input,
            scene: Default::default(),
            width: 480.,
        }
    }
}
impl Render for ConversationComposer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use crate::components::{
            composer_layout as layout,
            widgets::{composer, Pose},
        };
        let height = self
            .input
            .read(cx)
            .content_height()
            .unwrap_or(composer::EDITOR_MIN)
            .clamp(48., composer::EDITOR_MAX)
            + layout::TOP_EXTENSION
            + layout::COMPOSER_CHROME;
        self.scene.frame(Pose::rect(
            0.,
            0.,
            self.width as f64,
            height as f64,
            layout::SURFACE_RADIUS as f64,
        ));
        let owner = cx.entity().downgrade();
        let handler: composer::Handler = Rc::new(move |action, window, cx| {
            if let composer::Action::FocusEditor = action {
                let _ = owner.update(cx, |v, cx| {
                    window.focus(&v.input.read(cx).focus_handle(), cx)
                });
            }
        });
        let snapshot = composer::Snapshot {
            text: self.input.read(cx).value().to_owned(),
            capabilities: composer::Capabilities {
                editable: true,
                stop: false,
                enabled: true,
            },
            ..Default::default()
        };
        let widget = composer::render(
            composer::Props {
                id: "composer",
                scene: &self.scene,
                width: self.width,
                height,
                editor: &self.input,
                snapshot: &snapshot,
                fan_expanded: false,
                fan_pinned: false,
                handler,
                action_size: 24.,
                accessory_band: 0.,
                accessories: vec![],
                presentation: Some(composer::Presentation {
                    editor_id: "composer-input".into(),
                    attach_id: "composer-options".into(),
                    show_attach: true,
                    primary_id: "send-button".into(),
                    fan: None,
                    busy: false,
                    editor_label: "输入消息".into(),
                    attach_label: "添加文件".into(),
                    primary_label: "发送".into(),
                }),
            },
            window,
            cx,
        );
        let owner = cx.entity().downgrade();
        div()
            .relative()
            .size_full()
            .flex()
            .items_end()
            .justify_center()
            .pb(px(crate::design::ZORK_UI.layout.composer_bottom_inset))
            .child(
                canvas(
                    move |bounds, _, cx| {
                        let next = (bounds.size.width.as_f32() - 48.).clamp(220., 760.);
                        let _ = owner.update(cx, |v, cx| {
                            if (v.width - next).abs() > 0.5 {
                                v.width = next;
                                cx.notify();
                            }
                        });
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .child(div().w(px(self.width)).h(px(height)).child(widget))
    }
}
