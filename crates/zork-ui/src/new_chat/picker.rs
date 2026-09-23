//! Model and reasoning controls share one compact, anchored panel.
use super::*;
use crate::components::liquid::primitives::range::{self, Scale};

#[derive(Clone, Copy, PartialEq)]
pub(super) enum PickerMode {
    Strength,
    Models,
    Profiles,
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
    fn picker_button(
        &self,
        id: &'static str,
        label: String,
        mode: PickerMode,
        cx: &Context<Self>,
    ) -> AnyElement {
        ui::quiet_button(
            id,
            label.clone(),
            self.picker_open && self.data.editable,
            ui::IconButtonSize::Small,
        )
        .font_weight(FontWeight::NORMAL)
        .gap_1()
        .child(ui::icon("icons/chevron-down.svg", 10.))
        .on_click(cx.listener(move |view, _, window, cx| {
            view.picker_mode = mode;
            view.picker.focus(window, cx);
            cx.notify();
        }))
        .automation_enabled(
            self.picker_open && self.data.editable,
            AutomationRole::Button,
            label,
        )
        .into_any_element()
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
        if self.picker_mode != PickerMode::Strength {
            let models = self.picker_mode == PickerMode::Models;
            let choice = if models {
                &self.data.model
            } else {
                &self.data.profile
            };
            let title = self.text.text(if models {
                "new_chat_choose_model"
            } else {
                "new_chat_profile_auto"
            });
            let mut rows = div().flex().flex_col().gap_1().child(
                ui::quiet_button(
                    "new-chat-picker-back",
                    title.clone(),
                    enabled,
                    ui::IconButtonSize::Small,
                )
                .child(ui::icon("icons/arrow-left.svg", 14.))
                .on_click(cx.listener(|view, _, window, cx| {
                    view.picker_mode = PickerMode::Strength;
                    view.picker.focus(window, cx);
                    cx.notify();
                }))
                .automation_enabled(enabled, AutomationRole::Button, title),
            );
            for (index, option) in choice.options.iter().enumerate() {
                let value = option.value.clone();
                let label = if !models && value == "auto" {
                    self.text.text("new_chat_profile_auto")
                } else {
                    option.label.clone()
                };
                rows = rows.child(
                    ui::choice(
                        format!(
                            "new-chat-{}-{index}",
                            if models { "model" } else { "profile" }
                        ),
                        label.clone(),
                        value == choice.value,
                        enabled,
                    )
                    .w_full()
                    .justify_start()
                    .on_click(cx.listener(move |view, _, window, cx| {
                        view.picker.focus(window, cx);
                        cx.emit(Event::Intent(if models {
                            Action::Model {
                                value: value.clone(),
                            }
                        } else {
                            Action::Profile {
                                value: value.clone(),
                            }
                        }));
                        view.thinking_preview = None;
                        view.picker_mode = PickerMode::Strength;
                        cx.notify();
                    }))
                    .automation_enabled(enabled, AutomationRole::Button, label),
                );
            }
            if models {
                let label = self
                    .data
                    .profile
                    .options
                    .iter()
                    .find(|o| o.value == self.data.profile.value)
                    .map(|o| {
                        if o.value == "auto" {
                            self.text.text("new_chat_profile_auto")
                        } else {
                            o.label.clone()
                        }
                    })
                    .unwrap_or_else(|| self.text.text("new_chat_profile_auto"));
                rows = rows.child(div().mt_2().child(self.picker_button(
                    "new-chat-profile",
                    label,
                    PickerMode::Profiles,
                    cx,
                )));
            }
            return rows.into_any_element();
        }
        let levels = &self.data.thinking.options;
        let selected = self
            .thinking_preview
            .unwrap_or_else(|| {
                levels
                    .iter()
                    .position(|o| o.value == self.data.thinking.value)
                    .unwrap_or(0)
            })
            .min(levels.len().saturating_sub(1));
        let label = levels
            .get(selected)
            .map(|o| self.thinking_label(&o.value))
            .unwrap_or_else(|| self.text.text("new_chat_thinking"));
        let header = div()
            .flex()
            .items_center()
            .justify_between()
            .child(
                div()
                    .w(px(24.))
                    .text_color(rgb(ZORK_UI.palette.muted))
                    .child(ui::icon("icons/phosphor-brain.svg", 16.)),
            )
            .child(
                div()
                    .id("new-chat-thinking-label")
                    .text_size(px(15.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(crate::design::BRAND_ACCENT))
                    .child(label.clone())
                    .automation(AutomationRole::Status, label.clone()),
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
        let mut content = div().flex().flex_col().child(header).child(
            div().flex().justify_center().child(self.picker_button(
                "new-chat-model",
                model,
                PickerMode::Models,
                cx,
            )),
        );
        if levels.len() > 1 {
            let values: Vec<String> = levels.iter().map(|o| o.value.clone()).collect();
            let slider = range::step_slider(
                "new-chat-thinking",
                label,
                vec![selected as f64],
                Scale {
                    min: 0.,
                    max: (values.len() - 1) as f64,
                    step: 1.,
                    ..Default::default()
                },
                width - 24.,
                !enabled,
                window,
                cx,
                move |view, next, commit, cx| {
                    let index = next[0].round() as usize;
                    if let Some(value) = values.get(index) {
                        view.thinking_preview = Some(index);
                        if commit {
                            cx.emit(Event::Intent(Action::Thinking {
                                value: value.clone(),
                            }));
                        }
                        cx.notify();
                    }
                },
            );
            content = content.child(div().mt_1().child(slider));
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
        content.into_any_element()
    }
}
