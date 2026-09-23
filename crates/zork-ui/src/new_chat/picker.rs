//! Model and reasoning controls share one compact, anchored panel.
use super::*;
use crate::components::liquid::{
    controls::{adaptive_action, ActionStyle},
    primitives::range::{self, Scale},
};

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
    fn picker_button(
        &self,
        id: &'static str,
        effort: String,
        model: String,
        max_width: f32,
        cx: &Context<Self>,
    ) -> AnyElement {
        let accessible_label = format!(
            "{}: {}; {}: {}",
            self.text.text("new_chat_choose_model"),
            model,
            self.text.text("new_chat_thinking"),
            effort
        );
        adaptive_action(
            id,
            "",
            ActionStyle {
                quiet: true,
                bare: true,
                icon_only: Some(false),
                radius: Some(8.),
                disabled: !(self.picker_open && self.data.editable),
                ..Default::default()
            },
            ZORK_UI.palette.canvas,
        )
        .max_w(px(max_width))
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
                        .child(effort.clone())
                        .automation(AutomationRole::Status, effort),
                ),
        )
        .text_size(px(13.))
        .font_weight(FontWeight::MEDIUM)
        .aria_label(accessible_label.clone())
        .on_click(cx.listener(move |view, _, window, cx| {
            view.picker_mode = PickerMode::Models;
            view.picker.focus(window, cx);
            cx.notify();
        }))
        .automation_enabled(
            self.picker_open && self.data.editable,
            AutomationRole::Button,
            accessible_label,
        )
        .into_any_element()
    }
    fn picker_option(
        &self,
        id: String,
        label: String,
        value: String,
        selected: bool,
        model: bool,
        enabled: bool,
        cx: &Context<Self>,
    ) -> AnyElement {
        adaptive_action(
            id,
            label.clone(),
            ActionStyle {
                quiet: true,
                selected,
                leading: true,
                trailing: selected.then_some("icons/check.svg"),
                radius: Some(8.),
                disabled: !enabled,
                ..Default::default()
            },
            ZORK_UI.palette.canvas,
        )
        .w_full()
        .h(px(36.))
        .px_0()
        .font_weight(if selected {
            FontWeight::MEDIUM
        } else {
            FontWeight::NORMAL
        })
        .aria_toggled(if selected {
            Toggled::True
        } else {
            Toggled::False
        })
        .on_click(cx.listener(move |view, _, window, cx| {
            view.picker.focus(window, cx);
            cx.emit(Event::Intent(if model {
                Action::Model {
                    value: value.clone(),
                }
            } else {
                Action::Profile {
                    value: value.clone(),
                }
            }));
            view.thinking_preview = None;
            cx.notify();
        }))
        .automation_enabled(enabled, AutomationRole::Button, label)
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
        if self.picker_mode == PickerMode::Models {
            let back_label = self.text.text("back");
            let header = div()
                .h(px(30.))
                .mb_2()
                .flex()
                .items_center()
                .child(
                    adaptive_action(
                        "new-chat-picker-back",
                        "",
                        ActionStyle {
                            quiet: true,
                            icon_only: Some(true),
                            icon: Some("icons/arrow-left.svg"),
                            disabled: !enabled,
                            ..Default::default()
                        },
                        ZORK_UI.palette.canvas,
                    )
                    .size(px(28.))
                    .px_0()
                    .aria_label(back_label.clone())
                    .on_click(cx.listener(move |view, _, window, cx| {
                        view.picker_mode = PickerMode::Strength;
                        view.picker.focus(window, cx);
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
            let mut model_rows = div()
                .id("new-chat-model-list")
                .h(px(
                    (self.data.model.options.len() as f32 * 40. - 4.).clamp(0., 280.)
                ))
                .min_h_0()
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .gap_1();
            for (index, option) in self.data.model.options.iter().enumerate() {
                model_rows = model_rows.child(self.picker_option(
                    format!("new-chat-model-{index}"),
                    option.label.clone(),
                    option.value.clone(),
                    option.value == self.data.model.value,
                    true,
                    enabled,
                    cx,
                ));
            }
            let mut profile_rows = div()
                .id("new-chat-profile-list")
                .h(px(
                    (self.data.profile.options.len() as f32 * 40. - 4.).clamp(0., 280.)
                ))
                .min_h_0()
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .gap_1();
            for (index, option) in self.data.profile.options.iter().enumerate() {
                profile_rows = profile_rows.child(self.picker_option(
                    format!("new-chat-profile-{index}"),
                    if option.value == "auto" {
                        self.text.text("new_chat_profile_automatic")
                    } else {
                        option.label.clone()
                    },
                    option.value.clone(),
                    option.value == self.data.profile.value,
                    false,
                    enabled,
                    cx,
                ));
            }
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
            .map(|o| self.thinking_label(&o.value))
            .unwrap_or_else(|| self.text.text("new_chat_thinking"));
        let header = div()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(8.))
            .child(self.picker_button(
                "new-chat-model",
                label.clone(),
                model,
                (width - 56.).max(24.),
                cx,
            ))
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
