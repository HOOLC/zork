//! Complete new-Chat page. Product choices and creation state come from core.
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    components::{
        liquid::{composer, Pose},
        text_input::{ComposerEdited, ComposerInput, ComposerLayoutChanged, ComposerSubmit},
    },
    controls as ui,
    design::{TextRole, ZORK_UI},
    resources::Text,
};
use gpui::{prelude::*, *};
use std::rc::Rc;
use std::time::Instant;
use zork_client_types::new_chat::{Action, Snapshot};
mod picker;
use crate::components::liquid::panel::PopoverPanel;
use picker::PickerMode;

pub enum Event {
    Intent(Action),
    ConfigureModels,
    SelectDevice(String),
}
pub struct Page {
    data: Snapshot,
    text: Text,
    input: Entity<ComposerInput>,
    device_menu: bool,
    picker_open: bool,
    picker_mode: PickerMode,
    picker: PopoverPanel,
    thinking_preview: Option<usize>,
    width: f32,
    scene: composer::Scene,
    previous: Option<Instant>,
    scheduled: bool,
    focus_pending: bool,
    welcome: bool,
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
            device_menu: false,
            picker_open: false,
            picker_mode: PickerMode::Strength,
            picker: PopoverPanel::new(cx).with_radius(24.),
            thinking_preview: None,
            width: 480.,
            scene: Default::default(),
            previous: None,
            scheduled: false,
            focus_pending: true,
            welcome: false,
        }
    }
    pub fn set_welcome(&mut self, welcome: bool, cx: &mut Context<Self>) {
        if self.welcome != welcome {
            self.welcome = welcome;
            cx.notify();
        }
    }
    pub fn configure(&mut self, data: Snapshot, width: f32, text: Text, cx: &mut Context<Self>) {
        self.text = text;
        if self.data.model != data.model || self.data.thinking != data.thinking || !data.editable {
            self.thinking_preview = None;
        }
        if !data.editable {
            self.picker_open = false;
            self.device_menu = false;
        }
        if data.device.options.len() <= 1 {
            self.device_menu = false;
        }
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
    fn device_selector(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let choice = &self.data.device;
        let current = choice
            .options
            .iter()
            .find(|o| o.value == choice.value)
            .map(|o| {
                o.status.as_ref().map_or_else(
                    || o.label.clone(),
                    |status| crate::device_name::summary(&o.label, status, Some(&self.text)),
                )
            })
            .unwrap_or_else(|| self.text.text("new_chat_device"));
        let options = choice
            .options
            .iter()
            .enumerate()
            .map(|(i, o)| {
                (
                    format!("new-chat-device-{i}"),
                    o.status.as_ref().map_or_else(
                        || o.label.clone(),
                        |status| crate::device_name::summary(&o.label, status, Some(&self.text)),
                    ),
                    o.value == choice.value,
                )
            })
            .collect();
        let values: Vec<_> = choice.options.iter().map(|o| o.value.clone()).collect();
        ui::quiet_dropdown(
            "new-chat-device",
            current,
            options,
            self.device_menu,
            self.data.editable && !values.is_empty(),
            (self.width - 70.).max(48.),
            window,
            cx,
            |view, open, cx| {
                view.device_menu = open;
                if open {
                    view.picker_open = false;
                }
                cx.notify();
            },
            move |view, index, cx| {
                if let Some(id) = values.get(index) {
                    cx.emit(Event::SelectDevice(id.clone()));
                }
                view.device_menu = false;
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
            .clamp(48., composer::EDITOR_MAX)
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
        let model_label = self
            .data
            .model
            .options
            .iter()
            .find(|option| option.value == self.data.model.value)
            .map(|option| option.label.clone())
            .unwrap_or_else(|| self.text.text("new_chat_choose_model"));
        let trigger_label = self
            .selected_thinking_index()
            .and_then(|index| self.data.thinking.options.get(index))
            .filter(|_| !self.data.model.value.is_empty())
            .map(|option| format!("{} · {}", model_label, self.thinking_label(&option.value)))
            .unwrap_or(model_label);
        let trigger = self.picker.trigger(
            "new-chat-options",
            trigger_label,
            self.picker_open,
            self.data.editable,
            cx,
            |view, open, cx| {
                view.picker_open = open;
                view.device_menu = false;
                view.picker_mode = PickerMode::Strength;
                cx.notify();
            },
        );
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
                transparent_exterior: false,
                action_size: ZORK_UI.composer.action_size,
                accessory_band: 0.,
                accessories: vec![div()
                    .w_full()
                    .flex()
                    .justify_end()
                    .child(trigger)
                    .into_any_element()],
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
        let has_device_selector = self.data.device.options.len() > 1;
        let device = has_device_selector.then(|| self.device_selector(window, cx));
        let popup_width = (if self.picker_mode == PickerMode::Models {
            480_f32
        } else {
            284_f32
        })
        .min((window.viewport_size().width.as_f32() - 24.).max(2.));
        let content = if self.picker_open || self.picker.alive() {
            self.picker_content(popup_width, window, cx)
        } else {
            div().into_any_element()
        };
        let popup = self.picker.render(
            "new-chat-options-panel",
            self.picker_open,
            popup_width,
            ZORK_UI.composer.action_size,
            content,
            window,
            cx,
            |view, cx| {
                view.picker_open = false;
                view.thinking_preview = None;
                cx.notify();
            },
        );
        let note = if self.data.busy {
            Some(self.text.text("new_chat_creating"))
        } else if self.data.uncertain {
            Some(self.text.text("new_chat_uncertain"))
        } else {
            None
        };
        div()
            .id("new-chat-page")
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .items_center()
            .justify_end()
            .px_6()
            .pb(px(ZORK_UI.layout.composer_bottom_inset))
            .when(self.welcome, |v| {
                v.child(
                    div()
                        .id("new-chat-welcome")
                        .w_full()
                        .flex_1()
                        .min_h_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .pb_6()
                        .child(
                            div()
                                .max_w(px(600.))
                                .px_4()
                                .flex()
                                .flex_col()
                                .items_center()
                                .text_center()
                                .child(ui::page_title(self.text.text("onboarding_ready")))
                                .child(div().mt_3().child(ui::text_role(
                                    self.text.text("onboarding_ready_description"),
                                    TextRole::Description,
                                ))),
                        )
                        .automation(AutomationRole::Status, self.text.text("onboarding_ready")),
                )
            })
            .when(self.data.loading, |v| {
                v.child(
                    div()
                        .w(px(self.width))
                        .mb_2()
                        .child(crate::components::loading::status(
                            "new-chat-loading",
                            self.text.text("new_chat_loading"),
                        )),
                )
            })
            .when(self.data.needs_model, |v| {
                v.child(
                    div().w(px(self.width)).mb_2().child(
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
            .when_some(note, |v, note| {
                v.child(
                    div()
                        .w(px(self.width))
                        .mb_2()
                        .text_size(px(12.))
                        .text_color(rgb(ZORK_UI.palette.muted))
                        .child(note),
                )
            })
            .when_some(self.data.error.clone(), |v, error| {
                v.child(
                    div()
                        .id("new-chat-error")
                        .w(px(self.width))
                        .mb_2()
                        .text_size(px(12.))
                        .text_color(rgb(ZORK_UI.palette.danger))
                        .child(error.clone())
                        .automation(AutomationRole::Status, error),
                )
            })
            .when_some(device, |v, device| {
                v.child(
                    crate::components::liquid::primitives::surface(
                        "new-chat-context",
                        16.,
                        ZORK_UI.palette.sidebar_hover,
                        false,
                    )
                    .w(px((self.width - 24.).max(196.)))
                    .px_3()
                    .font_weight(FontWeight::NORMAL)
                    .pt_2()
                    .pb(px(16.))
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(ui::icon("icons/node.svg", 14.))
                    .child(device),
                )
            })
            .child(
                div()
                    .mt(px(if has_device_selector { -8. } else { 0. }))
                    .w(px(self.width))
                    .h(px(height))
                    .child(composer),
            )
            .children(popup)
            .automation(AutomationRole::Status, self.text.text("new_chat"))
    }
}
