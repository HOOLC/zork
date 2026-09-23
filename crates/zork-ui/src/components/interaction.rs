//! Presentation-only interaction card. Business actions, validation and outcome
//! state arrive from core; the entity owns only unsubmitted field buffers.
use super::text_input::{ComposerInput, ComposerLayoutChanged};
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    controls,
    design::ZORK_UI,
};
use gpui::{
    div, prelude::*, px, rgb, Context, Entity, EventEmitter, FontWeight, SharedString, Window,
};
use std::collections::{BTreeMap, HashMap, HashSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldKind {
    Text,
    Multiline,
    Choice,
    MultiChoice,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Field {
    pub id: String,
    pub label: SharedString,
    pub kind: FieldKind,
    pub advanced: bool,
    pub value: SharedString,
    pub options: Vec<(String, SharedString)>,
    pub error: Option<SharedString>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Action {
    pub id: String,
    pub label: SharedString,
    pub primary: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct View {
    pub id: String,
    pub title: SharedString,
    pub status: SharedString,
    pub description: Option<SharedString>,
    pub fields: Vec<Field>,
    pub details: Vec<(SharedString, SharedString)>,
    pub actions: Vec<Action>,
    pub editable: bool,
    pub placeholder: SharedString,
    pub expand_label: SharedString,
    pub collapse_label: SharedString,
    pub more_label: SharedString,
    pub less_label: SharedString,
    pub empty_label: SharedString,
    pub error: Option<SharedString>,
}

pub struct Activated {
    pub action: String,
    pub values: BTreeMap<String, String>,
}

pub struct InteractionCard {
    view: View,
    inputs: HashMap<String, Entity<ComposerInput>>,
    choices: HashMap<String, String>,
    open_choice: Option<String>,
    expanded: HashSet<String>,
    advanced_open: bool,
    on_action: Option<std::rc::Rc<dyn Fn(&Activated, &mut gpui::App)>>,
}
impl EventEmitter<Activated> for InteractionCard {}

impl InteractionCard {
    pub fn new(view: View, cx: &mut Context<Self>) -> Self {
        let mut card = Self {
            view: view.clone(),
            inputs: HashMap::new(),
            choices: HashMap::new(),
            open_choice: None,
            expanded: HashSet::new(),
            advanced_open: false,
            on_action: None,
        };
        card.synchronize_fields(&view, true, cx);
        card
    }

    pub fn with_action_handler(
        mut self,
        handler: impl Fn(&Activated, &mut gpui::App) + 'static,
    ) -> Self {
        self.on_action = Some(std::rc::Rc::new(handler));
        self
    }

    pub fn set_view(&mut self, view: View, cx: &mut Context<Self>) {
        if self.view == view {
            return;
        }
        let reset = self.view.id != view.id || !self.view.editable || !view.editable;
        if self.view.id != view.id {
            self.expanded.clear();
            self.advanced_open = false;
        }
        self.synchronize_fields(&view, reset, cx);
        if !view.editable {
            self.open_choice = None;
            if self.view.editable {
                self.advanced_open = false;
            }
        }
        self.view = view;
        cx.notify();
    }

    fn synchronize_fields(&mut self, view: &View, reset: bool, cx: &mut Context<Self>) {
        self.inputs
            .retain(|id, _| view.fields.iter().any(|f| &f.id == id));
        self.choices
            .retain(|id, _| view.fields.iter().any(|f| &f.id == id));
        for field in &view.fields {
            if matches!(field.kind, FieldKind::Choice | FieldKind::MultiChoice) {
                if reset || !self.choices.contains_key(&field.id) {
                    self.choices
                        .insert(field.id.clone(), field.value.to_string());
                }
            } else {
                let fresh = !self.inputs.contains_key(&field.id);
                let input = self.inputs.entry(field.id.clone()).or_insert_with(|| {
                    cx.new(|cx| {
                        let input = ComposerInput::new(field.label.clone(), cx);
                        if field.kind == FieldKind::Text {
                            input.single_line()
                        } else {
                            input
                        }
                    })
                });
                if fresh {
                    cx.subscribe(input, |_, _, _: &ComposerLayoutChanged, cx| cx.notify())
                        .detach();
                }
                if fresh || reset {
                    input.update(cx, |input, cx| input.set_value(field.value.to_string(), cx));
                }
            }
        }
    }

    fn activate(&self, action: &str, cx: &mut Context<Self>) {
        let mut values = BTreeMap::new();
        for field in &self.view.fields {
            let value = if matches!(field.kind, FieldKind::Choice | FieldKind::MultiChoice) {
                self.choices.get(&field.id).cloned().unwrap_or_default()
            } else {
                self.inputs
                    .get(&field.id)
                    .map(|input| input.read(cx).value().to_owned())
                    .unwrap_or_default()
            };
            values.insert(field.id.clone(), value);
        }
        let event = Activated {
            action: action.into(),
            values,
        };
        if let Some(handler) = &self.on_action {
            handler(&event, cx);
        }
        cx.emit(event);
    }

    fn read_value(
        &self,
        id: String,
        text: SharedString,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let mut lines = 0;
        let end = text
            .char_indices()
            .enumerate()
            .find_map(|(count, (offset, ch))| {
                if count >= 384 || lines >= 6 {
                    return Some(offset);
                }
                if ch == '\n' {
                    lines += 1;
                }
                None
            })
            .unwrap_or(text.len());
        let long = end < text.len();
        let expanded = self.expanded.contains(&id);
        let shown: SharedString = if long && !expanded {
            format!("{}…", &text[..end]).into()
        } else {
            text
        };
        let content = div()
            .text_size(px(13.))
            .line_height(px(20.))
            .text_color(rgb(ZORK_UI.palette.text))
            .child(shown);
        let mut row = div().flex().flex_col().gap_1().min_w(px(0.));
        row = if expanded {
            row.child(
                div()
                    .id(format!("{id}-full"))
                    .max_h(px(220.))
                    .overflow_y_scroll()
                    .child(content),
            )
        } else {
            row.child(content)
        };
        if long {
            let label = if expanded {
                self.view.collapse_label.clone()
            } else {
                self.view.expand_label.clone()
            };
            row = row.child(
                controls::button(format!("{id}-expand"), label.clone(), false, true)
                    .on_click(cx.listener(move |card, _, _, cx| {
                        if !card.expanded.remove(&id) {
                            card.expanded.insert(id.clone());
                        }
                        cx.notify();
                    }))
                    .automation(AutomationRole::Button, label),
            );
        }
        row.into_any_element()
    }
}

impl Render for InteractionCard {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = ZORK_UI.palette;
        let mut body = super::liquid::panel::inline(format!("interaction-card-{}", self.view.id))
            .w_full()
            .min_w(px(0.))
            .max_w(px(620.))
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .radius(12.)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_size(px(14.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(p.text))
                            .child(self.view.title.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(p.muted))
                            .child(self.view.status.clone()),
                    )
                    .when_some(self.view.description.clone(), |header, text| {
                        header.child(
                            div()
                                .text_size(px(12.))
                                .text_color(rgb(p.muted))
                                .child(text),
                        )
                    }),
            );
        for field in self
            .view
            .fields
            .clone()
            .into_iter()
            .filter(|field| !field.advanced || self.advanced_open || field.error.is_some())
        {
            let id = format!("interaction-{}-{}", self.view.id, field.id);
            let control = if !self.view.editable {
                let text = if field.kind == FieldKind::MultiChoice {
                    let selected: Vec<String> =
                        serde_json::from_str(&field.value).unwrap_or_default();
                    let labels = field
                        .options
                        .iter()
                        .filter(|(value, _)| selected.contains(value))
                        .map(|(_, label)| label.as_ref())
                        .collect::<Vec<_>>();
                    if labels.is_empty() {
                        self.view.empty_label.clone()
                    } else {
                        labels.join("、").into()
                    }
                } else if field.kind == FieldKind::Choice {
                    field
                        .options
                        .iter()
                        .find(|(value, _)| value.as_str() == field.value.as_ref())
                        .map(|(_, label)| label.clone())
                        .unwrap_or_else(|| field.value.clone())
                } else {
                    field.value.clone()
                };
                self.read_value(id, text, cx)
            } else if matches!(field.kind, FieldKind::Choice | FieldKind::MultiChoice) {
                let selected = self.choices.get(&field.id).cloned().unwrap_or_default();
                let multiple = field.kind == FieldKind::MultiChoice;
                let selected_values: Vec<String> = if multiple {
                    serde_json::from_str(&selected).unwrap_or_default()
                } else {
                    vec![selected.clone()]
                };
                let label = if multiple {
                    let labels = field
                        .options
                        .iter()
                        .filter(|(value, _)| selected_values.contains(value))
                        .map(|(_, label)| label.as_ref())
                        .collect::<Vec<_>>();
                    if labels.is_empty() {
                        self.view.empty_label.to_string()
                    } else {
                        labels.join("、")
                    }
                } else {
                    field
                        .options
                        .iter()
                        .find(|(value, _)| *value == selected)
                        .map(|(_, label)| label.to_string())
                        .unwrap_or_else(|| self.view.placeholder.to_string())
                };
                let open_id = field.id.clone();
                let choose_id = field.id.clone();
                let options = field.options.clone();
                controls::dropdown_with_selection(
                    id.clone(),
                    label,
                    field
                        .options
                        .iter()
                        .enumerate()
                        .map(|(i, (value, label))| {
                            (
                                format!("{id}-{i}"),
                                label.to_string(),
                                selected_values.contains(value),
                            )
                        })
                        .collect(),
                    self.open_choice.as_ref() == Some(&field.id),
                    !field.options.is_empty(),
                    if multiple {
                        crate::components::liquid::overlay::Selection::Multiple
                    } else {
                        crate::components::liquid::overlay::Selection::Single
                    },
                    window,
                    cx,
                    move |view, open, cx| {
                        view.open_choice = open.then(|| open_id.clone());
                        cx.notify();
                    },
                    move |view, index, cx| {
                        if let Some((value, _)) = options.get(index) {
                            if multiple {
                                let mut selected: Vec<String> = view
                                    .choices
                                    .get(&choose_id)
                                    .and_then(|value| serde_json::from_str(value).ok())
                                    .unwrap_or_default();
                                if selected.contains(value) {
                                    selected.retain(|item| item != value);
                                } else {
                                    selected.push(value.clone());
                                }
                                view.choices.insert(
                                    choose_id.clone(),
                                    serde_json::to_string(&selected).unwrap(),
                                );
                            } else {
                                view.choices.insert(choose_id.clone(), value.clone());
                            }
                        }
                        if !multiple {
                            view.open_choice = None;
                        }
                        cx.notify();
                    },
                )
            } else {
                let input = self.inputs[&field.id].clone();
                let content_height = input.read(cx).content_height().unwrap_or(20.);
                controls::input_control(id, &input, field.error.is_some(), cx)
                    .when(field.kind == FieldKind::Multiline, |field| {
                        field.h(px(content_height.min(200.) + 10.))
                    })
                    .automation(AutomationRole::TextInput, field.label.clone())
                    .into_any_element()
            };
            let row = div()
                .flex()
                .flex_col()
                .gap_1()
                .min_w(px(0.))
                .child(controls::label(field.label))
                .child(control)
                .when_some(field.error, |row, error| {
                    row.child(controls::feedback(error.to_string()))
                });
            body = body.child(row);
        }
        if self.view.fields.iter().any(|field| field.advanced) {
            let label = if self.advanced_open {
                self.view.less_label.clone()
            } else {
                self.view.more_label.clone()
            };
            body = body.child(
                controls::button(
                    format!("interaction-{}-settings", self.view.id),
                    label.clone(),
                    false,
                    true,
                )
                .on_click(cx.listener(|view, _, _, cx| {
                    view.advanced_open = !view.advanced_open;
                    cx.notify();
                }))
                .automation(AutomationRole::Button, label),
            );
        }
        if !self.view.details.is_empty() {
            body = body.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .pt_2()
                    .border_t(gpui::px(crate::design::BORDER_WIDTH))
                    .border_color(rgb(p.border_strong))
                    .children(self.view.details.iter().enumerate().map(
                        |(index, (label, value))| {
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(
                                    div()
                                        .text_size(px(11.))
                                        .text_color(rgb(p.muted))
                                        .child(label.clone()),
                                )
                                .child(self.read_value(
                                    format!("interaction-{}-detail-{index}", self.view.id),
                                    value.clone(),
                                    cx,
                                ))
                        },
                    )),
            );
        }
        if let Some(error) = &self.view.error {
            body = body.child(controls::feedback(error.to_string()));
        }
        if !self.view.actions.is_empty() {
            let mut actions = div().flex().flex_wrap().items_center().gap_2().pt_1();
            for action in self.view.actions.clone() {
                let id = format!("interaction-{}-{}", self.view.id, action.id);
                actions = actions.child(
                    controls::button(id, action.label.clone(), action.primary, true)
                        .on_click(cx.listener(move |view, _, _, cx| view.activate(&action.id, cx)))
                        .automation(AutomationRole::Button, action.label),
                );
            }
            body = body.child(actions);
        }
        body.automation(
            AutomationRole::Status,
            format!("{} · {}", self.view.title, self.view.status),
        )
    }
}
