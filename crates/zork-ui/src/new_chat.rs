//! Complete new-Chat page. Product choices and creation state come from core.
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    components::{
        liquid::{composer, Pose},
        text_input::{ComposerEdited, ComposerInput, ComposerLayoutChanged, ComposerSubmit},
    },
    controls as ui,
    design::ZORK_UI,
    resources::Text,
};
use gpui::{prelude::*, *};
use std::rc::Rc;
#[cfg(not(target_family = "wasm"))]
use std::time::Instant;
#[cfg(target_family = "wasm")]
use web_time::Instant;
use zork_client_types::new_chat::{Action, Snapshot};

pub enum Event {
    Intent(Action),
    ConfigureModels,
}
pub struct Page {
    data: Snapshot,
    text: Text,
    input: Entity<ComposerInput>,
    menus: [bool; 3],
    width: f32,
    scene: composer::Scene,
    previous: Option<Instant>,
    scheduled: bool,
    focus_pending: bool,
}
impl EventEmitter<Event> for Page {}
impl Page {
    #[cfg(feature = "stories")]
    pub fn inspect(&self) -> serde_json::Value {
        serde_json::to_value(&self.data).expect("new chat snapshot")
    }
    pub fn new(text: Text, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| ComposerInput::new(text.text("composer_placeholder"), cx));
        cx.subscribe(&input, |view, _, _: &ComposerEdited, cx| {
            cx.emit(Event::Intent(Action::Edit {
                text: view.input.read(cx).value().into(),
            }))
        })
        .detach();
        cx.subscribe(&input, |view, _, _: &ComposerSubmit, cx| {
            if view.data.can_submit {
                cx.emit(Event::Intent(Action::Submit {
                    text: view.input.read(cx).value().into(),
                }));
            }
        })
        .detach();
        cx.subscribe(&input, |_, _, _: &ComposerLayoutChanged, cx| cx.notify())
            .detach();
        Self {
            data: Default::default(),
            text,
            input,
            menus: [false; 3],
            width: 480.,
            scene: Default::default(),
            previous: None,
            scheduled: false,
            focus_pending: true,
        }
    }
    pub fn configure(&mut self, data: Snapshot, width: f32, text: Text, cx: &mut Context<Self>) {
        self.text = text;
        self.input.update(cx, |input, cx| {
            // Rendering can precede delivery of ComposerEdited. Keep a local
            // edit until core echoes it; a frozen submission remains authoritative.
            if input.value() == self.data.text || !data.editable {
                input.set_value(data.text.clone(), cx);
            }
            input.set_editable(!data.editable, false, cx);
        });
        if self.data != data || self.width != width {
            self.width = width.max(220.);
            self.data = data;
            cx.notify();
        }
    }
    pub fn focus(&mut self, cx: &mut Context<Self>) {
        self.focus_pending = true;
        cx.notify();
    }
    fn selector(&self, field: usize, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let (id, key, choice) = match field {
            0 => ("new-chat-model", "new_chat_choose_model", &self.data.model),
            1 => (
                "new-chat-thinking",
                "new_chat_thinking",
                &self.data.thinking,
            ),
            _ => (
                "new-chat-profile",
                "new_chat_profile_auto",
                &self.data.profile,
            ),
        };
        let label = |value: &str, label: &str| {
            if field == 2 && value == "auto" {
                self.text.text("new_chat_profile_auto")
            } else {
                label.into()
            }
        };
        let current = choice
            .options
            .iter()
            .find(|o| o.value == choice.value)
            .map(|o| label(&o.value, &o.label))
            .unwrap_or_else(|| {
                if choice.value.is_empty() {
                    self.text.text(key)
                } else {
                    choice.value.clone()
                }
            });
        let options = choice
            .options
            .iter()
            .enumerate()
            .map(|(i, o)| {
                (
                    format!("{id}-{i}"),
                    label(&o.value, &o.label),
                    o.value == choice.value,
                )
            })
            .collect();
        let values = choice
            .options
            .iter()
            .map(|o| o.value.clone())
            .collect::<Vec<_>>();
        ui::dropdown_with_icons(
            id,
            current,
            options,
            self.menus[field],
            self.data.editable && !values.is_empty(),
            None,
            vec![None; values.len()],
            window,
            cx,
            move |v, open, cx| {
                v.menus = [false; 3];
                v.menus[field] = open;
                cx.notify();
            },
            move |v, index, cx| {
                if let Some(value) = values.get(index).cloned() {
                    cx.emit(Event::Intent(match field {
                        0 => Action::Model { value },
                        1 => Action::Thinking { value },
                        _ => Action::Profile { value },
                    }));
                }
                v.menus = [false; 3];
                cx.notify();
            },
        )
    }
}
impl Render for Page {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.focus_pending {
            self.focus_pending = false;
            window.focus(&self.input.read(cx).focus_handle(), cx);
        }
        let height = self
            .input
            .read(cx)
            .content_height()
            .unwrap_or(composer::EDITOR_MIN)
            .clamp(composer::EDITOR_MIN, composer::EDITOR_MAX)
            + crate::components::liquid_composer::TOP_EXTENSION
            + crate::components::liquid_composer::COMPOSER_CHROME;
        let now = cx.background_executor().now();
        let elapsed = self.previous.replace(now).map_or(0., |before| {
            now.saturating_duration_since(before).as_secs_f64()
        });
        let body = Pose::rect(
            0.,
            0.,
            self.width as f64,
            height as f64,
            crate::components::liquid_composer::SURFACE_RADIUS as f64,
        );
        let moving = self
            .scene
            .frame(body, &[], None, elapsed, cx.reduce_motion());
        if moving && !self.scheduled {
            self.scheduled = true;
            let owner = cx.entity().downgrade();
            window.on_next_frame(move |_, cx| {
                let _ = owner.update(cx, |v, cx| {
                    v.scheduled = false;
                    cx.notify();
                });
            });
        }
        let owner = cx.entity().downgrade();
        let handler: composer::Handler = Rc::new(move |action, window, cx| {
            let _ = owner.update(cx, |v, cx| match action {
                composer::Action::Primary if v.data.can_submit => {
                    cx.emit(Event::Intent(Action::Submit {
                        text: v.input.read(cx).value().into(),
                    }))
                }
                composer::Action::FocusEditor => window.focus(&v.input.read(cx).focus_handle(), cx),
                _ => {}
            });
        });
        let snapshot = composer::Snapshot {
            text: self.data.text.clone(),
            capabilities: composer::Capabilities {
                editable: self.data.editable,
                stop: false,
                enabled: self.data.can_submit,
            },
            ..Default::default()
        };
        let composer = composer::render(
            composer::Props {
                id: "new-chat-composer",
                surface: self.scene.surface.as_ref().expect("composer surface"),
                width: self.width,
                height,
                editor: &self.input,
                snapshot: &snapshot,
                fan_progress: 0.,
                fan_pinned: false,
                bubbles: &[],
                handler,
                presentation: Some(composer::Presentation {
                    editor_id: "new-chat-input".into(),
                    attach_id: "new-chat-attach".into(),
                    primary_id: "new-chat-send".into(),
                    member_groups: vec![],
                    member_colors: vec![],
                    member_names: vec![],
                    fan: None,
                    busy: self.data.busy,
                    editor_label: self.text.text("composer_placeholder").into(),
                    attach_label: String::new().into(),
                    show_attach: false,
                    primary_label: self
                        .text
                        .text(if self.data.uncertain {
                            "new_chat_retry"
                        } else {
                            "send_message"
                        })
                        .into(),
                }),
            },
            window,
            cx,
        );
        let options = (0..3)
            .map(|i| self.selector(i, window, cx))
            .collect::<Vec<_>>();
        div()
            .id("new-chat-page")
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .px_6()
            .child(div().mb_8().w(px(self.width)).child(ui::heading(
                self.text.text("new_chat"),
                self.text.text("new_chat_intro"),
            )))
            .child(div().w(px(self.width)).h(px(height)).child(composer))
            .child(
                div()
                    .w(px(self.width))
                    .mt_3()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .children(options),
            )
            .child(
                div()
                    .w(px(self.width))
                    .mt_3()
                    .text_size(px(12.))
                    .text_color(rgb(ZORK_UI.palette.muted))
                    .child(self.text.text(if self.data.busy {
                        "new_chat_creating"
                    } else if self.data.uncertain {
                        "new_chat_uncertain"
                    } else {
                        "new_chat_hint"
                    })),
            )
            .when(self.data.loading, |v| {
                v.child(crate::components::loading::status(
                    "new-chat-loading",
                    self.text.text("new_chat_loading"),
                ))
            })
            .when(self.data.needs_model, |v| {
                v.child(
                    div().mt_3().child(
                        ui::button(
                            "new-chat-model-settings",
                            self.text.text("new_chat_add_model"),
                            true,
                            true,
                        )
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(Event::ConfigureModels))),
                    ),
                )
            })
            .when_some(self.data.error.clone(), |v, error| {
                v.child(
                    div()
                        .id("new-chat-error")
                        .w(px(self.width))
                        .mt_3()
                        .text_color(rgb(ZORK_UI.palette.danger))
                        .child(error.clone())
                        .automation(AutomationRole::Status, error),
                )
            })
            .automation(AutomationRole::Status, self.text.text("new_chat"))
    }
}
