//! One anchored panel picks a model from a connection and, below it, the
//! selected model's own thinking options. Values and labels come from core.
use super::*;
use crate::design::INTERACTION;

/// Beyond this many models the panel offers a search field.
const SEARCH_THRESHOLD: usize = 8;

/// Models of one connection, built from the connections core names on each
/// model option; first appearance keeps core's order.
pub(super) struct Group {
    pub profile: String,
    pub name: String,
    pub provider: String,
    pub models: Vec<String>,
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
    pub(super) fn groups(&self, cx: &App) -> Vec<Group> {
        let mut groups: Vec<Group> = Vec::new();
        for option in &self.data.model.options {
            for connection in &option.connections {
                match groups.iter_mut().find(|g| g.profile == connection.profile) {
                    Some(group) => group.models.push(option.value.clone()),
                    None => groups.push(Group {
                        profile: connection.profile.clone(),
                        name: connection.name.clone(),
                        provider: connection.provider.clone(),
                        models: vec![option.value.clone()],
                    }),
                }
            }
        }
        if self.data.model.options.len() > SEARCH_THRESHOLD {
            let query = self.picker_search.read(cx).value().trim().to_lowercase();
            if !query.is_empty() {
                for group in &mut groups {
                    let name = group.name.to_lowercase();
                    if !name.contains(&query) {
                        group.models.retain(|m| m.to_lowercase().contains(&query));
                    }
                }
                groups.retain(|g| !g.models.is_empty());
                return groups;
            }
        }
        if groups.is_empty() && !self.data.model.options.is_empty() {
            // Older payloads carry no connection names; keep one plain group.
            groups.push(Group {
                profile: self.data.profile.value.clone(),
                name: self.text.text("new_chat_choose_model"),
                provider: String::new(),
                models: self.data.model.options.iter().map(|o| o.value.clone()).collect(),
            });
        }
        groups
    }
    /// Provider of the connection the current model resolves to.
    pub(super) fn selected_provider(&self, cx: &App) -> Option<String> {
        let profile = self.data.profile.value.as_str();
        self.groups(cx)
            .into_iter()
            .find(|g| {
                (profile == "auto" || g.profile == profile)
                    && g.models.iter().any(|m| m == &self.data.model.value)
            })
            .map(|g| g.provider)
            .filter(|p| !p.is_empty())
    }
    fn selectable_pairs(&self, cx: &App) -> Vec<(String, String)> {
        self.groups(cx)
            .into_iter()
            .flat_map(|g| {
                let profile = g.profile;
                g.models.into_iter().map(move |m| (profile.clone(), m))
            })
            .collect()
    }
    fn selected_pair(&self, cx: &App) -> Option<usize> {
        let pairs = self.selectable_pairs(cx);
        let profile = self.data.profile.value.as_str();
        pairs
            .iter()
            .position(|(p, m)| m == &self.data.model.value && p == profile)
            .or_else(|| pairs.iter().position(|(_, m)| m == &self.data.model.value))
    }
    /// Scrolls the model list so the current choice is in view. Row geometry
    /// is fixed, so the offset comes from it rather than from last frame's bounds.
    fn reveal_selected_model(&self, list_height: f32, cx: &App) {
        let Some(selected) = self.selected_pair(cx) else {
            return;
        };
        if self.picker_revealed.get() == Some(selected) {
            return;
        }
        self.picker_revealed.set(Some(selected));
        let (title, row, gap, group_gap) = (30., 36., 2., 6.);
        let mut y = 0.;
        let mut index = 0;
        for (g, group) in self.groups(cx).into_iter().enumerate() {
            if g > 0 {
                y += group_gap;
            }
            y += title + gap;
            for _ in &group.models {
                if index == selected {
                    let top = -self.picker_scroll.offset().y.as_f32();
                    let target = if y < top {
                        y
                    } else if y + row > top + list_height {
                        y + row - list_height
                    } else {
                        return;
                    };
                    self.picker_scroll.set_offset(point(px(0.), px(-target)));
                    return;
                }
                index += 1;
                y += row + gap;
            }
        }
    }
    fn select(&mut self, profile: String, model: String, cx: &mut Context<Self>) {
        cx.emit(Event::Intent(Action::Select { profile, model }));
        cx.notify();
    }
    pub(super) fn picker_content(
        &self,
        width: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = ZORK_UI.palette;
        let enabled = self.picker_open && self.data.editable;
        let selected = self.selected_pair(cx);
        // The panel opens above or below the composer; keep the list inside the window.
        let list_height = (window.viewport_size().height.as_f32() - 480.).clamp(120., 260.);
        // Keeps the current model in view when the panel opens or the choice moves.
        self.reveal_selected_model(list_height, cx);
        let mut list = div()
            .id("new-chat-model-list")
            .max_h(px(list_height))
            .overflow_y_scroll()
            .track_scroll(&self.picker_scroll)
            .flex()
            .flex_col()
            .gap(px(2.));
        let device = self
            .data
            .model
            .options
            .iter()
            .find_map(|o| o.device.clone());
        let mut index = 0usize;
        for (g, group) in self.groups(cx).into_iter().enumerate() {
            list = list.child(
                div()
                    .h(px(30.))
                    .flex_shrink_0()
                    .px(px(12.))
                    .mt(px(if g == 0 { 0. } else { 6. }))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .text_size(px(12.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(p.muted))
                    .when(!group.provider.is_empty(), |v| {
                        v.child(ui::provider_icon(&group.provider, 16.))
                    })
                    .child(div().min_w_0().truncate().child(group.name.clone()))
                    .when_some(device.clone(), |v, device| {
                        v.child(
                            div()
                                .ml_auto()
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .font_weight(FontWeight::NORMAL)
                                .child(crate::device_name::mark(&device, 16.))
                                .child(device),
                        )
                    }),
            );
            for model in group.models {
                let checked = selected == Some(index);
                let row_id = format!("new-chat-model-{index}");
                let (profile, id) = (group.profile.clone(), model.clone());
                let label = format!("{} · {}", group.name, model);
                list = list.child(
                    div()
                        .id(SharedString::from(row_id))
                        .h(px(36.))
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
                        .child(div().flex_1().min_w_0().truncate().child(model))
                        .child(
                            div().w(px(14.)).flex_shrink_0().when(checked, |v| {
                                v.child(ui::icon("icons/check.svg", 14.))
                            }),
                        )
                        .when(enabled, |v| {
                            v.on_click(cx.listener(move |view, _, _, cx| {
                                view.select(profile.clone(), id.clone(), cx)
                            }))
                        })
                        .automation_enabled(enabled, AutomationRole::Button, label),
                );
                index += 1;
            }
        }
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
                        let pairs = view.selectable_pairs(cx);
                        if pairs.is_empty() {
                            return;
                        }
                        let count = pairs.len();
                        let next = match (view.selected_pair(cx), key) {
                            (Some(i), "up") => (i + count - 1) % count,
                            (Some(i), _) => (i + 1) % count,
                            (None, _) => 0,
                        };
                        let (profile, model) = pairs[next].clone();
                        view.select(profile, model, cx);
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
            .into_any_element()
    }
}
