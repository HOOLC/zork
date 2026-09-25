//! Complete new-Chat page. Product choices and creation state come from core.
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    components::{
        text_input::{ComposerEdited, ComposerInput, ComposerLayoutChanged, ComposerSubmit},
        widgets::{composer, Pose},
    },
    controls as ui,
    design::{TextRole, ZORK_UI},
    resources::Text,
};
use gpui::{prelude::*, *};
use gpui_component::popover::Popover;
use std::rc::Rc;
use zork_client_types::new_chat::{Action, Snapshot};
mod picker;

const RAIL_INSET: f32 = 13.;
const RAIL_CONTENT_INSET: f32 = 12.;
const RAIL_VISIBLE_HEIGHT: f32 = 40.;
const RAIL_OVERLAP: f32 = 16.;
const RAIL_RADIUS: f32 = 16.;

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
    picker_focus: FocusHandle,
    picker_trigger_focus: FocusHandle,
    picker_scroll: ScrollHandle,
    picker_search: Entity<ComposerInput>,
    /// Model row last scrolled into view; cleared when the panel closes.
    picker_revealed: std::cell::Cell<Option<usize>>,
    width: f32,
    scene: composer::Scene,
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
        let picker_search = cx.new(|cx| ComposerInput::new("搜索模型", cx).single_line());
        cx.subscribe(&picker_search, |view: &mut Self, _, _: &ComposerEdited, cx| {
            view.picker_revealed.set(None);
            cx.notify();
        })
        .detach();
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
            picker_focus: cx.focus_handle(),
            picker_trigger_focus: cx.focus_handle(),
            picker_scroll: ScrollHandle::new(),
            picker_search,
            picker_revealed: Default::default(),
            width: 480.,
            scene: Default::default(),
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
    /// Up to four targets show as tabs: every device's mark, name and
    /// reachability stay visible, and one click chooses it.
    fn device_tabs(&self, cx: &mut Context<Self>) -> AnyElement {
        let choice = &self.data.device;
        let editable = self.data.editable;
        div()
            .id("new-chat-device-tabs")
            .flex()
            .items_center()
            .gap(px(4.))
            .min_w_0()
            .overflow_hidden()
            .children(choice.options.iter().enumerate().map(|(i, option)| {
                let selected = option.value == choice.value;
                let value = option.value.clone();
                let content = match &option.status {
                    Some(status) => crate::device_name::label(
                        format!("new-chat-device-name-{i}"),
                        option.label.clone(),
                        status,
                        Some(&self.text),
                    )
                    .into_any_element(),
                    None => div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .min_w_0()
                        .child(crate::device_name::mark(&option.label, 18.))
                        .child(div().min_w_0().text_ellipsis().child(option.label.clone()))
                        .into_any_element(),
                };
                crate::components::widgets::controls::adaptive_action(
                    format!("new-chat-device-{i}"),
                    "",
                    crate::components::widgets::controls::ActionStyle {
                        quiet: true,
                        icon_only: Some(false),
                        selected,
                        disabled: !editable,
                        ..Default::default()
                    },
                    ZORK_UI.palette.sidebar_hover,
                )
                .h(px(28.))
                .min_h(px(28.))
                .pl(px(6.))
                .pr(px(12.))
                // Narrow windows shrink every tab and ellipsize its name
                // instead of clipping the last device.
                .min_w_0()
                .flex_shrink_1()
                .font_weight(FontWeight::NORMAL)
                .child(content)
                .on_click(cx.listener(move |view, _, _, cx| {
                    if view.data.editable {
                        cx.emit(Event::SelectDevice(value.clone()));
                        view.device_menu = false;
                        cx.notify();
                    }
                }))
                .automation_enabled(editable, AutomationRole::Button, option.label.clone())
            }))
            .into_any_element()
    }
    fn device_selector(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        if self.data.device.options.len() <= 4 {
            return self.device_tabs(cx);
        }
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
            (self.width - 2. * (RAIL_INSET + RAIL_CONTENT_INSET) - 18.).max(48.),
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
        let has_device_selector = self.data.device.options.len() > 1;
        let body_height = self
            .input
            .read(cx)
            .content_height()
            .unwrap_or(composer::EDITOR_MIN)
            .clamp(48., composer::EDITOR_MAX)
            + crate::components::composer_layout::TOP_EXTENSION
            + crate::components::composer_layout::COMPOSER_CHROME;
        let rail_offset = if has_device_selector {
            RAIL_VISIBLE_HEIGHT
        } else {
            0.
        };
        let height = body_height + rail_offset;
        let body = Pose::rect(
            0.,
            0.,
            self.width as f64,
            body_height as f64,
            crate::components::composer_layout::SURFACE_RADIUS as f64,
        );
        self.scene.frame(body);
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
            .selected_model_label()
            .unwrap_or_else(|| self.text.text("new_chat_choose_model"));
        let trigger_label = self
            .selected_thinking_index()
            .and_then(|index| self.data.thinking.options.get(index))
            .filter(|_| !self.data.model.value.is_empty())
            .map(|option| self.thinking_label(&option.value));
        let model_part = model_label.clone();
        let thinking_part = trigger_label.clone();
        let label = match &trigger_label {
            Some(thinking) => format!("{model_label} · {thinking}"),
            None => model_label,
        };
        let picker_owner = cx.entity().downgrade();
        let change_owner = picker_owner.clone();
        let popup_width = 380_f32.min((window.viewport_size().width.as_f32() - 24.).max(2.));
        let trigger = Popover::new("new-chat-options-panel")
            .rounded(px(crate::design::RADIUS.container))
            .trigger(
                ui::quiet_button("new-chat-options", "", self.data.editable, ui::IconButtonSize::Small)
                .h(px(28.))
                .font_weight(FontWeight::MEDIUM)
                .radius(14.)
                .track_focus(&self.picker_trigger_focus)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .when_some(self.selected_maker_path(), |v, maker| {
                            v.child(
                                div()
                                    .id("new-chat-options-mark")
                                    .flex_shrink_0()
                                    .child(ui::icon(maker, 16.).text_color(rgb(ZORK_UI.palette.text)))
                                    .automation(AutomationRole::Status, maker),
                            )
                        })
                        .child(div().text_color(rgb(ZORK_UI.palette.text)).child(model_part))
                        .when_some(thinking_part, |v, thinking| {
                            v.child(
                                div()
                                    .text_color(rgb(ZORK_UI.palette.muted))
                                    .child(format!("· {thinking}")),
                            )
                        })
                        .child(ui::icon("icons/chevron-down.svg", 12.)),
                )
                .on_click(cx.listener(|view, event: &ClickEvent, window, cx| {
                    if matches!(event, ClickEvent::Keyboard(_)) && view.data.editable {
                        view.picker_open = true;
                        view.device_menu = false;
                        window.focus(&view.picker_focus, cx);
                        cx.notify();
                    }
                }))
                .automation_enabled(
                    self.data.editable,
                    AutomationRole::Button,
                    label,
                ),
            )
            .open(self.picker_open && self.data.editable)
            .track_focus(&self.picker_focus)
            .on_open_change(move |open, window, app| {
                let _ = change_owner.update(app, |view, cx| {
                    view.picker_open = *open;
                    view.picker_revealed.set(None);
                    if *open {
                        window.focus(&view.picker_focus, cx);
                    }
                    view.device_menu = false;
                    if !open {
                        window.focus(&view.picker_trigger_focus, cx);
                    }
                    cx.notify();
                });
            })
            .content(move |_, window, cx| {
                picker_owner
                    .update(cx, |view, cx| view.picker_content(popup_width, window, cx))
                    .unwrap_or_else(|_| div().into_any_element())
            });
        let rail = has_device_selector.then(|| {
            let device = self.device_selector(window, cx);
            div()
                .id("new-chat-device-rail")
                .absolute()
                .left(px(RAIL_INSET))
                .top(px(0.))
                .w(px(self.width - 2. * RAIL_INSET))
                .h(px(RAIL_VISIBLE_HEIGHT + RAIL_OVERLAP))
                .rounded_tl(px(RAIL_RADIUS))
                .rounded_tr(px(RAIL_RADIUS))
                .bg(rgb(ZORK_UI.palette.sidebar_hover))
                .child(
                    div()
                        .h(px(RAIL_VISIBLE_HEIGHT))
                        .px(px(RAIL_CONTENT_INSET))
                        .flex()
                        .items_center()
                        .gap_1()
                        .when(self.data.device.options.len() > 4, |v| {
                            v.child(ui::icon("icons/node.svg", 14.))
                        })
                        .child(device),
                )
                .automation(AutomationRole::Status, "设备选择栏")
        });
        let composer = composer::render(
            composer::Props {
                id: "new-chat-composer",
                scene: &self.scene,
                width: self.width,
                height: body_height,
                editor: &self.input,
                snapshot: &snapshot,
                handler,
                action_size: 28.,
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
                    files: None,
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
            // Without a greeting the composer sits in the middle of the page
            // instead of under an empty field.
            .when(self.welcome, |v| v.justify_end())
            .when(!self.welcome, |v| v.justify_center())
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
            // Notices and the composer form one block; stories crop to it.
            .child(
                div()
                    .id("new-chat-form")
                    .flex()
                    .flex_col()
                    .items_center()
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
                    .child(
                        div()
                            .relative()
                            .w(px(self.width))
                            .h(px(height))
                            .when_some(rail, |wrapper, rail| wrapper.child(rail))
                            .child(
                                div()
                                    .absolute()
                                    .left(px(0.))
                                    .top(px(rail_offset))
                                    .w(px(self.width))
                                    .h(px(body_height))
                                    .child(composer),
                            ),
                    )
                    .automation(AutomationRole::Status, "new-chat-form"),
            )
            .automation(AutomationRole::Status, self.text.text("new_chat"))
    }
}
