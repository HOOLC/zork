//! Model and reasoning controls share one compact, anchored panel.
use super::*;
use gpui_component::slider::{Slider, SliderEvent, SliderState};

#[derive(Clone, Copy, PartialEq)]
pub(super) enum PickerMode {
    Strength,
    Models,
}

impl Page {
    pub(super) fn selected_model_label(&self) -> Option<String> {
        let value = self.data.model.value.trim();
        if value.is_empty() {
            return None;
        }
        Some(
            self.data
                .model
                .options
                .iter()
                .find(|option| option.value == value)
                .map(|option| option.label.as_str())
                .filter(|label| !label.trim().is_empty())
                .unwrap_or(value)
                .to_owned(),
        )
    }
    pub(super) fn thinking_label(&self, value: &str) -> String {
        match value {
            "off" | "low" | "medium" | "high" | "minimal" | "xhigh" | "max" => {
                self.text.text(&format!("thinking_{value}"))
            }
            _ => value.to_owned(),
        }
    }
    pub(super) fn selected_thinking_index(&self) -> Option<usize> {
        let levels = &self.data.thinking.options;
        (!levels.is_empty()).then(|| {
            self.thinking_preview
                .unwrap_or_else(|| {
                    levels
                        .iter()
                        .position(|option| option.value == self.data.thinking.value)
                        .unwrap_or(0)
                })
                .min(levels.len() - 1)
        })
    }
    pub(super) fn picker_content(
        &self,
        width: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let enabled = self.picker_open && self.data.editable;
        let model = self
            .selected_model_label()
            .unwrap_or_else(|| self.text.text("new_chat_choose_model"));
        if self.picker_mode == PickerMode::Models {
            let back_label = self.text.text("back");
            let header = div()
                .h(px(30.))
                .mb_2()
                .flex()
                .items_center()
                .child(
                    ui::quiet_button(
                        "new-chat-picker-back",
                        "",
                        enabled,
                        ui::IconButtonSize::Small,
                    )
                    .size(px(28.))
                    .px_0()
                    .child(ui::icon("icons/arrow-left.svg", 14.))
                    .aria_label(back_label.clone())
                    .on_click(cx.listener(|view, _, window, cx| {
                        view.picker_mode = PickerMode::Strength;
                        window.focus(&view.picker_focus, cx);
                        cx.notify();
                    }))
                    .automation_enabled(
                        enabled,
                        AutomationRole::Button,
                        back_label,
                    ),
                )
                .child(
                    div()
                        .flex_1()
                        .text_center()
                        .text_size(px(13.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgb(ZORK_UI.palette.text))
                        .child(self.text.text("new_chat_model_profile")),
                )
                .child(div().w(px(28.)));
            let available_width = (width - 24. - 12.).max(4.);
            let model_width = (available_width * 0.6).max(2.);
            let profile_width = (available_width - model_width).max(2.);
            let model_values: Vec<_> = self
                .data
                .model
                .options
                .iter()
                .map(|o| o.value.clone())
                .collect();
            let model_rows = crate::components::choice_menu::inline_list(
                "new-chat-model-list",
                self.data
                    .model
                    .options
                    .iter()
                    .enumerate()
                    .map(|(i, o)| {
                        (
                            format!("new-chat-model-{i}"),
                            o.label.clone(),
                            o.value == self.data.model.value,
                        )
                    })
                    .collect(),
                enabled,
                window,
                cx,
                move |view, index, cx| {
                    if let Some(value) = model_values.get(index) {
                        cx.emit(Event::Intent(Action::Model {
                            value: value.clone(),
                        }));
                        view.thinking_preview = None;
                        cx.notify();
                    }
                },
            );
            let profile_values: Vec<_> = self
                .data
                .profile
                .options
                .iter()
                .map(|o| o.value.clone())
                .collect();
            let profile_rows = crate::components::choice_menu::inline_list(
                "new-chat-profile-list",
                self.data
                    .profile
                    .options
                    .iter()
                    .enumerate()
                    .map(|(i, o)| {
                        (
                            format!("new-chat-profile-{i}"),
                            if o.value == "auto" {
                                self.text.text("new_chat_profile_automatic")
                            } else {
                                o.label.clone()
                            },
                            o.value == self.data.profile.value,
                        )
                    })
                    .collect(),
                enabled,
                window,
                cx,
                move |view, index, cx| {
                    if let Some(value) = profile_values.get(index) {
                        cx.emit(Event::Intent(Action::Profile {
                            value: value.clone(),
                        }));
                        view.thinking_preview = None;
                        cx.notify();
                    }
                },
            );
            let columns = div()
                .flex()
                .gap(px(12.))
                .child(
                    div()
                        .w(px(model_width))
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .px_3()
                                .pb_2()
                                .text_size(px(12.))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(rgb(ZORK_UI.palette.muted))
                                .child(self.text.text("new_chat_choose_model")),
                        )
                        .child(model_rows),
                )
                .child(
                    div()
                        .w(px(profile_width))
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .px_3()
                                .pb_2()
                                .text_size(px(12.))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(rgb(ZORK_UI.palette.muted))
                                .child(self.text.text("new_chat_choose_profile")),
                        )
                        .child(profile_rows),
                );
            return div()
                .id("new-chat-picker-content")
                .track_focus(&self.picker_focus)
                .flex()
                .flex_col()
                .child(header)
                .child(columns)
                .into_any_element();
        }
        let levels = &self.data.thinking.options;
        let selected = self.selected_thinking_index().unwrap_or(0);
        let label = levels
            .get(selected)
            .map(|option| self.thinking_label(&option.value))
            .unwrap_or_else(|| self.text.text("new_chat_thinking"));
        let accessible_label = format!(
            "{}: {}; {}: {}",
            self.text.text("new_chat_choose_model"),
            model,
            self.text.text("new_chat_thinking"),
            label,
        );
        let header = div()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(8.))
            .child(
                ui::quiet_button("new-chat-model", "", enabled, ui::IconButtonSize::Small)
                    .max_w(px((width - 56.).max(24.)))
                    .h(px(28.))
                    .px_0()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(5.))
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_color(rgb(ZORK_UI.palette.text))
                                    .child(model),
                            )
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .text_color(rgb(ZORK_UI.palette.muted))
                                    .child("·"),
                            )
                            .child(
                                div()
                                    .id("new-chat-thinking-label")
                                    .flex_shrink_0()
                                    .text_size(px(13.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(rgb(crate::design::BRAND_ACCENT))
                                    .child(label.clone())
                                    .automation(AutomationRole::Status, label.clone()),
                            ),
                    )
                    .aria_label(accessible_label.clone())
                    .on_click(cx.listener(|view, _, window, cx| {
                        view.picker_mode = PickerMode::Models;
                        window.focus(&view.picker_focus, cx);
                        cx.notify();
                    }))
                    .automation_enabled(enabled, AutomationRole::Button, accessible_label),
            )
            .child(
                ui::quiet_button(
                    "new-chat-thinking-reset",
                    "",
                    enabled && !levels.is_empty(),
                    ui::IconButtonSize::Small,
                )
                .w(px(24.))
                .p_0()
                .child(ui::icon("icons/reload.svg", 16.))
                .on_click(cx.listener(|view, _, _, cx| {
                    cx.stop_propagation();
                    view.thinking_preview = None;
                    cx.emit(Event::Intent(Action::Thinking {
                        value: String::new(),
                    }));
                }))
                .automation_enabled(
                    enabled && !levels.is_empty(),
                    AutomationRole::Button,
                    self.text.text("new_chat_reset_thinking"),
                ),
            );
        let mut content = div().flex().flex_col().child(header);
        if levels.len() > 1 {
            let owner = cx.entity().downgrade();
            let count = levels.len();
            let keyed =
                window.use_keyed_state(format!("new-chat-thinking-{count}"), cx, |window, app| {
                    let slider = app.new(|_| {
                        SliderState::new()
                            .min(0.)
                            .max((count - 1) as f32)
                            .step(1.)
                            .default_value(selected as f32)
                    });
                    let subscription =
                        window.subscribe(&slider, app, move |_, event: &SliderEvent, _, app| {
                            let (value, commit) = match event {
                                SliderEvent::Change(value) => (value.end(), false),
                                SliderEvent::Release(value) => (value.end(), true),
                            };
                            let _ = owner.update(app, |view, cx| {
                                let index = (value.round() as usize)
                                    .min(view.data.thinking.options.len().saturating_sub(1));
                                if let Some(option) = view.data.thinking.options.get(index) {
                                    view.thinking_preview = Some(index);
                                    if commit {
                                        cx.emit(Event::Intent(Action::Thinking {
                                            value: option.value.clone(),
                                        }));
                                    }
                                    cx.notify();
                                }
                            });
                        });
                    (slider, subscription)
                });
            let slider = keyed.read(cx).0.clone();
            if slider.read(cx).value().end() != selected as f32 {
                slider.update(cx, |state, cx| state.set_value(selected as f32, window, cx));
            }
            let keyboard_slider = slider.clone();
            content = content.child(
                div()
                    .id("new-chat-thinking-keyboard")
                    .mt_1()
                    .w(px(width - 24.))
                    .focusable()
                    .tab_stop(enabled)
                    .aria_label(label.clone())
                    .on_key_down(cx.listener(move |view, event: &KeyDownEvent, window, cx| {
                        if !enabled {
                            return;
                        }
                        let current = view.thinking_preview.unwrap_or_else(|| {
                            view.data
                                .thinking
                                .options
                                .iter()
                                .position(|option| option.value == view.data.thinking.value)
                                .unwrap_or(0)
                        });
                        let last = view.data.thinking.options.len().saturating_sub(1);
                        let next = match event.keystroke.key.as_str() {
                            "left" | "down" => current.saturating_sub(1),
                            "right" | "up" => (current + 1).min(last),
                            "home" => 0,
                            "end" => last,
                            _ => return,
                        };
                        if let Some(option) = view.data.thinking.options.get(next) {
                            keyboard_slider
                                .update(cx, |state, cx| state.set_value(next as f32, window, cx));
                            view.thinking_preview = Some(next);
                            cx.emit(Event::Intent(Action::Thinking {
                                value: option.value.clone(),
                            }));
                            cx.notify();
                            window.prevent_default();
                            cx.stop_propagation();
                        }
                    }))
                    .child(Slider::new(&slider).disabled(!enabled).w_full())
                    .automation_enabled(enabled, AutomationRole::Option, label),
            );
        } else {
            content = content.child(
                div()
                    .h(px(32.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(12.))
                    .text_color(rgb(ZORK_UI.palette.muted))
                    .child(self.text.text("new_chat_fixed_thinking")),
            );
        }
        content
            .id("new-chat-picker-content")
            .track_focus(&self.picker_focus)
            .into_any_element()
    }
}
