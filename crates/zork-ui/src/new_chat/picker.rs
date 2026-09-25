//! One anchored panel picks a model and, below it, the selected model's own
//! thinking options. The connection is optional and automatic by default; it
//! can be pinned to one of the connections serving the model. Values, labels
//! and which connections serve a model come from core.
use super::*;
use crate::design::INTERACTION;
use zork_client_types::new_chat::OptionItem;

/// Beyond this many models the panel offers a search field.
const SEARCH_THRESHOLD: usize = 8;
const ROW: f32 = 36.;
const ROW_GAP: f32 = 2.;

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
    /// Core labels thinking values with each provider's own names.
    pub(super) fn thinking_label(&self, value: &str) -> String {
        self.data
            .thinking
            .options
            .iter()
            .find(|option| option.value == value)
            .map(|option| option.label.clone())
            .unwrap_or_else(|| value.to_owned())
    }
    pub(super) fn selected_thinking_index(&self) -> Option<usize> {
        let levels = &self.data.thinking.options;
        (!levels.is_empty()).then(|| {
            levels
                .iter()
                .position(|option| option.value == self.data.thinking.value)
                .unwrap_or(0)
        })
    }
    /// The pinned connection; `None` while the connection is automatic.
    pub(super) fn pinned_connection(&self) -> Option<&OptionItem> {
        let value = self.data.profile.value.as_str();
        if value.is_empty() || value == "auto" {
            return None;
        }
        self.data.profile.options.iter().find(|o| o.value == value)
    }
    /// Listed models, once each in core's order; a long list filters by name.
    fn visible_models(&self, cx: &App) -> Vec<&OptionItem> {
        let options = &self.data.model.options;
        let query = if options.len() > SEARCH_THRESHOLD {
            self.picker_search.read(cx).value().trim().to_lowercase()
        } else {
            String::new()
        };
        options
            .iter()
            .filter(|o| {
                query.is_empty()
                    || o.value.to_lowercase().contains(&query)
                    || o.label.to_lowercase().contains(&query)
            })
            .collect()
    }
    fn model_label(option: &OptionItem) -> String {
        if option.label.trim().is_empty() {
            option.value.clone()
        } else {
            option.label.clone()
        }
    }
    /// Maker of a listed model; `None` for an unknown maker.
    fn maker_of(&self, model: &str) -> Option<&str> {
        self.data
            .model
            .options
            .iter()
            .find(|o| o.value == model)
            .and_then(|o| o.maker.as_deref())
    }
    /// The current model's maker mark, once a listed model is chosen. The
    /// trigger names a model, so it carries the maker, not the connection.
    pub(super) fn selected_maker_path(&self) -> Option<&'static str> {
        let model = self.data.model.value.as_str();
        (!model.is_empty() && self.data.model.options.iter().any(|o| o.value == model))
            .then(|| ui::maker_path(self.maker_of(model)))
    }
    fn selected_row(&self, cx: &App) -> Option<usize> {
        self.visible_models(cx)
            .iter()
            .position(|o| o.value == self.data.model.value)
    }
    /// Scrolls the model list so the current choice is in view. Row geometry
    /// is fixed, so the offset comes from it rather than from last frame's bounds.
    fn reveal_selected_model(&self, list_height: f32, cx: &App) {
        let Some(selected) = self.selected_row(cx) else {
            return;
        };
        if self.picker_revealed.get() == Some(selected) {
            return;
        }
        self.picker_revealed.set(Some(selected));
        let y = selected as f32 * (ROW + ROW_GAP);
        let top = -self.picker_scroll.offset().y.as_f32();
        let target = if y < top {
            y
        } else if y + ROW > top + list_height {
            y + ROW - list_height
        } else {
            return;
        };
        self.picker_scroll.set_offset(point(px(0.), px(-target)));
    }
    fn select_model(&mut self, model: String, cx: &mut Context<Self>) {
        cx.emit(Event::Intent(Action::Model { value: model }));
        cx.notify();
    }
    fn pin_connection(&mut self, profile: String, cx: &mut Context<Self>) {
        self.picker_connections_open = false;
        cx.emit(Event::Intent(Action::Profile { value: profile }));
        cx.notify();
    }
    /// The optional connection: a quiet row naming the current choice, which
    /// unfolds into automatic plus the connections serving the model.
    fn connection_section(
        &self,
        enabled: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let p = ZORK_UI.palette;
        let options = &self.data.profile.options;
        // Nothing to pin until a model with a serving connection is chosen.
        if self.data.model.value.is_empty() || !options.iter().any(|o| o.value != "auto") {
            return None;
        }
        let automatic = self.text.text("new_chat_connection_auto");
        let title = self.text.text("new_chat_connection");
        let pinned = self.pinned_connection();
        let current = pinned.map_or_else(|| automatic.clone(), |o| o.label.clone());
        let open = self.picker_connections_open && enabled;
        let toggle = div()
            .id("new-chat-connection")
            .h(px(ROW))
            .flex_shrink_0()
            .px(px(14.))
            .flex()
            .items_center()
            .gap(px(8.))
            .rounded_full()
            .text_size(px(13.))
            .when(enabled, |v| {
                v.hover(|s| s.bg(rgb(INTERACTION.neutral_hover)))
            })
            .child(div().text_color(rgb(p.muted)).child(title.clone()))
            .child(div().flex_1())
            .when_some(pinned.and_then(|o| o.provider.clone()), |v, provider| {
                v.child(
                    div()
                        .id("new-chat-connection-mark")
                        .flex_shrink_0()
                        .child(ui::icon(ui::provider_path(&provider), 14.).text_color(rgb(p.text)))
                        .automation(AutomationRole::Status, ui::provider_path(&provider)),
                )
            })
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_color(rgb(if pinned.is_some() { p.text } else { p.muted }))
                    .child(current.clone()),
            )
            .child(crate::components::disclosure::chevron(
                "new-chat-connection",
                open,
                p.muted,
            ))
            .aria_expanded(open)
            .when(enabled, |v| {
                v.on_click(cx.listener(|view, _, _, cx| {
                    view.picker_connections_open = !view.picker_connections_open;
                    cx.notify();
                }))
            })
            .automation_enabled(
                enabled,
                AutomationRole::Button,
                format!("{title} · {current}"),
            );
        let selected = self.data.profile.value.as_str();
        let selected = if selected.is_empty() {
            "auto"
        } else {
            selected
        };
        let rows =
            div()
                .flex()
                .flex_col()
                .gap(px(ROW_GAP))
                .children(options.iter().enumerate().map(|(i, option)| {
                    let checked = option.value == selected;
                    let auto = option.value == "auto";
                    let label = if auto {
                        automatic.clone()
                    } else {
                        option.label.clone()
                    };
                    // Automatic names the pool the node picks from for this model.
                    let pool = auto.then(|| {
                        option
                            .connections
                            .iter()
                            .map(|c| c.name.as_str())
                            .collect::<Vec<_>>()
                            .join("、")
                    });
                    let mark = if auto {
                        "icons/sparkles.svg"
                    } else {
                        ui::provider_path(option.provider.as_deref().unwrap_or_default())
                    };
                    let value = option.value.clone();
                    div()
                        .id(SharedString::from(format!("new-chat-connection-{i}")))
                        .h(px(ROW))
                        .flex_shrink_0()
                        .px(px(14.))
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .rounded_full()
                        .text_size(px(13.))
                        .text_color(rgb(p.text))
                        .when(checked, |v| {
                            v.bg(rgb(p.selected)).font_weight(FontWeight::MEDIUM)
                        })
                        .when(enabled && !checked, |v| {
                            v.hover(|s| s.bg(rgb(INTERACTION.neutral_hover)))
                        })
                        .child(
                            div()
                                .id(SharedString::from(format!("new-chat-connection-mark-{i}")))
                                .flex_shrink_0()
                                .child(ui::icon(mark, 16.).text_color(rgb(p.text)))
                                .automation(AutomationRole::Status, mark),
                        )
                        .child(div().flex_shrink_0().child(label.clone()))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(px(12.))
                                .font_weight(FontWeight::NORMAL)
                                .text_color(rgb(p.muted))
                                .children(pool.filter(|names| !names.is_empty())),
                        )
                        .child(
                            div()
                                .w(px(14.))
                                .flex_shrink_0()
                                .when(checked, |v| v.child(ui::icon("icons/check.svg", 14.))),
                        )
                        .when(enabled, |v| {
                            v.on_click(cx.listener(move |view, _, _, cx| {
                                view.pin_connection(value.clone(), cx)
                            }))
                        })
                        .automation_enabled(
                            enabled,
                            AutomationRole::Button,
                            format!("{title} · {label}"),
                        )
                }));
        let body = crate::motion::fold("new-chat-connection-list", open, false, window, cx)
            .map(|frame| frame.wrap(div().w_full().pt(px(ROW_GAP)).child(rows)));
        Some(
            div()
                .mt(px(10.))
                .flex()
                .flex_col()
                .child(toggle)
                .children(body)
                .into_any_element(),
        )
    }
    pub(super) fn picker_content(
        &self,
        width: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = ZORK_UI.palette;
        let enabled = self.picker_open && self.data.editable;
        let selected = self.selected_row(cx);
        // The panel opens above or below the composer; keep the list inside the window.
        let list_height = (window.viewport_size().height.as_f32() - 480.).clamp(120., 260.);
        // Keeps the current model in view when the panel opens or the choice moves.
        self.reveal_selected_model(list_height, cx);
        let visible = self.visible_models(cx);
        let list = div()
            .id("new-chat-model-list")
            .max_h(px(list_height))
            .overflow_y_scroll()
            .track_scroll(&self.picker_scroll)
            .flex()
            .flex_col()
            .gap(px(ROW_GAP))
            .children(visible.iter().enumerate().map(|(index, option)| {
                let checked = selected == Some(index);
                let label = Self::model_label(option);
                let id = option.value.clone();
                let maker = ui::maker_path(option.maker.as_deref());
                div()
                    .id(SharedString::from(format!("new-chat-model-{index}")))
                    .h(px(ROW))
                    .flex_shrink_0()
                    .px(px(14.))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .rounded_full()
                    .text_size(px(13.))
                    .text_color(rgb(p.text))
                    .when(checked, |v| v.bg(rgb(p.selected)).font_weight(FontWeight::MEDIUM))
                    .when(enabled && !checked, |v| {
                        v.hover(|s| s.bg(rgb(INTERACTION.neutral_hover)))
                    })
                    // The row names a model: its maker's mark.
                    .child(
                        div()
                            .id(SharedString::from(format!("new-chat-model-mark-{index}")))
                            .flex_shrink_0()
                            .child(ui::icon(maker, 16.).text_color(rgb(p.text)))
                            .automation(AutomationRole::Status, maker),
                    )
                    .child(div().flex_1().min_w_0().truncate().child(label.clone()))
                    .child(
                        div().w(px(14.)).flex_shrink_0().when(checked, |v| {
                            v.child(ui::icon("icons/check.svg", 14.))
                        }),
                    )
                    .when(enabled, |v| {
                        v.on_click(cx.listener(move |view, _, _, cx| {
                            view.select_model(id.clone(), cx)
                        }))
                    })
                    .automation_enabled(enabled, AutomationRole::Button, label)
            }));
        let search = (self.data.model.options.len() > SEARCH_THRESHOLD).then(|| {
            div().pb(px(6.)).child(
                ui::input_control("new-chat-model-search", &self.picker_search, false, cx)
                    .automation(AutomationRole::TextInput, "搜索模型"),
            )
        });
        let levels = &self.data.thinking.options;
        let thinking = (levels.len() > 1).then(|| {
            let budget = levels
                .iter()
                .any(|o| o.label.ends_with('K') || o.label.ends_with('M'));
            let title = if budget {
                self.text.text("new_chat_thinking_budget")
            } else {
                self.text.text("new_chat_thinking")
            };
            let values: Vec<_> = levels.iter().map(|o| o.value.clone()).collect();
            let control = crate::components::widgets::controls::segmented(
                "new-chat-thinking",
                (width - 28.).max(2.),
                levels
                    .iter()
                    .enumerate()
                    .map(|(i, o)| (format!("new-chat-thinking-{i}"), o.label.clone().into()))
                    .collect(),
                self.selected_thinking_index().unwrap_or(0),
                enabled,
                p.elevated,
                window,
                cx,
                move |_, i, cx| {
                    if let Some(value) = values.get(i) {
                        cx.emit(Event::Intent(Action::Thinking {
                            value: value.clone(),
                        }));
                    }
                },
            );
            div()
                .mt(px(14.))
                .px(px(6.))
                .pb(px(4.))
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(
                    div()
                        .px(px(6.))
                        .text_size(px(12.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(p.muted))
                        .child(title),
                )
                .child(control)
        });
        let connection = self.connection_section(enabled, window, cx);
        div()
            .id("new-chat-picker-content")
            .track_focus(&self.picker_focus)
            .w(px(width))
            .flex()
            .flex_col()
            .on_key_down(cx.listener(|view, event: &KeyDownEvent, _, cx| {
                if !view.data.editable {
                    return;
                }
                let key = event.keystroke.key.as_str();
                match key {
                    "up" | "down" => {
                        let models: Vec<String> = view
                            .visible_models(cx)
                            .iter()
                            .map(|o| o.value.clone())
                            .collect();
                        if models.is_empty() {
                            return;
                        }
                        let count = models.len();
                        let next = match (view.selected_row(cx), key) {
                            (Some(i), "up") => (i + count - 1) % count,
                            (Some(i), _) => (i + 1) % count,
                            (None, _) => 0,
                        };
                        view.select_model(models[next].clone(), cx);
                    }
                    "left" | "right" => {
                        let levels = &view.data.thinking.options;
                        if levels.len() < 2 {
                            return;
                        }
                        let current = view.selected_thinking_index().unwrap_or(0);
                        let next = if key == "left" {
                            current.saturating_sub(1)
                        } else {
                            (current + 1).min(levels.len() - 1)
                        };
                        if let Some(option) = levels.get(next) {
                            cx.emit(Event::Intent(Action::Thinking {
                                value: option.value.clone(),
                            }));
                        }
                    }
                    "enter" => {
                        view.picker_open = false;
                        cx.notify();
                    }
                    _ => return,
                }
                cx.stop_propagation();
            }))
            .children(search)
            .child(list)
            .children(thinking)
            .children(connection)
            .into_any_element()
    }
}
