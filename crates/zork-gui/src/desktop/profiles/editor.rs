//! Model editor. A recognized id fills every parameter; parameters stay
//! collapsed until asked for, and each one can borrow a popular model's value.
use super::*;
use crate::api::model_catalog::Field;
use crate::api::thinking::{BudgetChoice, ThinkingScheme as T};
use std::rc::Rc;

const LABEL_WIDTH: f32 = 64.;
/// Native effort names in the order providers list them.
const ORDER: [&str; 6] = ["minimal", "low", "medium", "high", "xhigh", "max"];

impl ProfilesView {
    pub(super) fn provider_id(&self) -> Option<String> {
        self.detail
            .as_ref()
            .and_then(|d| d["provider"].as_str())
            .map(str::to_owned)
    }
    pub(super) fn recognition_line(&self, id: &str) -> Option<(String, String)> {
        if id.trim().is_empty() {
            return None;
        }
        let provider = self.provider_id();
        let found = crate::api::model_catalog::recognition(id, provider.as_deref(), None);
        Some((
            found["entry"]["name"].as_str()?.to_owned(),
            found["summary"].as_str().unwrap_or_default().to_owned(),
        ))
    }
    pub(super) fn recognize(&mut self, cx: &mut Context<Self>) {
        let id = self.model.read(cx).value().to_owned();
        self.recognized = self.recognition_line(&id);
        if self.editing_model.is_none() {
            let provider = self.provider_id();
            let entry = crate::api::model_catalog::recognize(&id, provider.as_deref());
            if let Some(entry) = entry.filter(|_| !self.api_touched) {
                if let Some(index) = MODEL_APIS.iter().position(|a| a.0 == entry.api) {
                    self.model_api = index;
                }
            }
            if !self.params_touched {
                // A new model takes every parameter from the recognized entry
                // until the user changes one of them.
                let blank = ModelInput {
                    id: id.clone(),
                    context: String::new(),
                    output: String::new(),
                    thinking: String::new(),
                    default_thinking: String::new(),
                    images: false,
                    thinking_scheme: None,
                    ..self.model_input(cx)
                };
                let (filled, entry) =
                    crate::api::model_catalog::fill_input(blank, provider.as_deref());
                if entry.is_some() {
                    self.context_limit
                        .update(cx, |v, cx| v.set_value(filled.context.clone(), cx));
                    self.output_limit
                        .update(cx, |v, cx| v.set_value(filled.output.clone(), cx));
                    self.thinking_scheme = filled.scheme();
                    self.model_image_input = filled.images;
                    self.params_touched = false;
                }
            }
        }
        zork_ui::components::region::invalidate(cx, &["dialog", "page"]);
    }
    /// The request field the thinking choice is sent as.
    fn request_field(&self, cx: &gpui::App) -> Option<String> {
        let id = self.model.read(cx).value().to_owned();
        let provider = self.provider_id();
        crate::api::model_catalog::recognize(&id, provider.as_deref())
            .and_then(|entry| entry.thinking_field.clone())
            .or_else(|| {
                match &self.thinking_scheme {
                    T::Levels { .. } => Some("reasoning.effort"),
                    T::Budget { .. } => Some("thinking.budget_tokens"),
                    T::Toggle { .. } => Some("thinking.type"),
                    _ => None,
                }
                .map(str::to_owned)
            })
    }
    fn apply_reference(&mut self, field: Field, value: Value, cx: &mut Context<Self>) {
        let tokens = |v: &Value| v.as_u64().map(compact_tokens);
        match field {
            Field::Context => {
                if let Some(text) = tokens(&value) {
                    self.context_limit.update(cx, |v, cx| v.set_value(text, cx));
                }
            }
            Field::Output => {
                if let Some(text) = tokens(&value) {
                    self.output_limit.update(cx, |v, cx| v.set_value(text, cx));
                }
            }
            Field::Thinking => {
                if let Ok(scheme) = serde_json::from_value(value["scheme"].clone()) {
                    self.thinking_scheme = scheme;
                }
            }
            Field::Capabilities => self.model_image_input = value["image"] == true,
        }
        self.params_touched = true;
        self.reference_open = None;
        zork_ui::components::region::invalidate(cx, &["dialog", "page"]);
    }
    fn set_thinking(&mut self, scheme: T, cx: &mut Context<Self>) {
        self.thinking_scheme = scheme;
        self.params_touched = true;
        zork_ui::components::region::invalidate(cx, &["dialog", "page"]);
    }
    /// A small pill that is either on (ink) or off (quiet).
    pub(super) fn chip(
        &self,
        id: impl Into<gpui::ElementId>,
        label: String,
        on: bool,
        cx: &mut Context<Self>,
        click: impl Fn(&mut Self, &mut Context<Self>) + 'static,
    ) -> gpui::AnyElement {
        let p = ZORK_UI.palette;
        let enabled = !self.busy;
        div()
            .id(id)
            .h(px(26.))
            .px(px(10.))
            .flex()
            .flex_shrink_0()
            .items_center()
            .rounded_full()
            .text_size(px(12.5))
            .font_weight(gpui::FontWeight::MEDIUM)
            .bg(rgb(if on { p.text } else { p.prompt }))
            .text_color(rgb(if on { p.canvas } else { p.muted }))
            .when(enabled && !on, |v| {
                v.hover(|s| s.bg(rgb(zork_ui::design::INTERACTION.neutral_hover)))
            })
            .when(enabled, |v| {
                v.cursor_pointer()
                    .on_click(cx.listener(move |view, _, _, cx| click(view, cx)))
            })
            .child(label.clone())
            .automation_enabled(enabled, AutomationRole::Button, label)
            .into_any_element()
    }
    /// A compact 28 px segmented control for short option sets.
    fn segmented(
        &self,
        id: &'static str,
        options: Vec<String>,
        selected: Option<usize>,
        cx: &mut Context<Self>,
        pick: impl Fn(&mut Self, usize, &mut Context<Self>) + 'static,
    ) -> gpui::AnyElement {
        let p = ZORK_UI.palette;
        let enabled = !self.busy;
        let pick = Rc::new(pick);
        let items: Vec<_> = options
            .into_iter()
            .enumerate()
            .map(|(i, label)| {
                let on = selected == Some(i);
                let pick = pick.clone();
                div()
                    .id(gpui::SharedString::from(format!("{id}-{i}")))
                    .h(px(24.))
                    .px(px(10.))
                    .flex()
                    .items_center()
                    .rounded_full()
                    .text_size(px(12.5))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(rgb(if on { p.text } else { p.muted }))
                    .when(on, |v| v.bg(rgb(p.canvas)))
                    .when(enabled && !on, |v| v.hover(|s| s.text_color(rgb(p.text))))
                    .when(enabled, |v| {
                        v.cursor_pointer()
                            .on_click(cx.listener(move |view, _, _, cx| pick(view, i, cx)))
                    })
                    .child(label.clone())
                    .automation_enabled(enabled, AutomationRole::Option, label)
            })
            .collect();
        div()
            .id(id)
            .h(px(28.))
            .p(px(2.))
            .flex()
            .flex_shrink_0()
            .items_center()
            .rounded_full()
            .bg(rgb(p.prompt))
            .children(items)
            .into_any_element()
    }
    /// label | control | trailing, on one baseline.
    fn row(
        &self,
        label: &'static str,
        control: gpui::AnyElement,
        trailing: Option<gpui::AnyElement>,
    ) -> gpui::Div {
        div()
            .min_h(px(36.))
            .flex()
            .items_center()
            .gap(px(8.))
            .child(
                div()
                    .w(px(LABEL_WIDTH))
                    .flex_shrink_0()
                    .text_size(px(12.5))
                    .text_color(rgb(ZORK_UI.palette.muted))
                    .child(label),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .flex_wrap()
                    .gap(px(6.))
                    .child(control),
            )
            .children(trailing)
    }
    fn field_error(&self, field: &str, cx: &gpui::App) -> Option<gpui::AnyElement> {
        let error = self
            .model_attempted
            .then(|| self.model_errors(cx))
            .unwrap_or_default()
            .into_iter()
            .find(|(f, _)| f == field)
            .map(|(_, e)| e)?;
        Some(
            div()
                .id(gpui::SharedString::from(format!("{field}-error")))
                .pl(px(LABEL_WIDTH + 8.))
                .text_size(px(12.))
                .text_color(rgb(ZORK_UI.palette.danger))
                .child(error.clone())
                .automation(AutomationRole::Status, error)
                .into_any_element(),
        )
    }
    fn tokens_field(
        &self,
        id: &'static str,
        label: &'static str,
        input: &Entity<ComposerInput>,
        cx: &gpui::App,
    ) -> gpui::AnyElement {
        let invalid = self
            .model_attempted
            .then(|| self.model_errors(cx))
            .unwrap_or_default()
            .iter()
            .any(|(f, _)| f == id);
        ui::input_control(id, input, invalid, cx)
            .w(px(120.))
            .automation_enabled(!self.busy, AutomationRole::TextInput, label)
            .into_any_element()
    }
    fn reference_toggle(&self, field: Field, cx: &mut Context<Self>) -> gpui::AnyElement {
        let open = self.reference_open == Some(field);
        ui::quiet_button(
            format!("model-reference-{field:?}").to_lowercase(),
            "参照",
            !self.busy,
            ui::IconButtonSize::Compact,
        )
        .text_size(px(12.))
        .child(ui::icon("icons/chevron-down.svg", 12.).when(open, |icon| {
            icon.with_transformation(gpui::Transformation::rotate(gpui::radians(
                std::f32::consts::PI,
            )))
        }))
        .on_click(cx.listener(move |v, _, _, cx| {
            v.reference_open = (v.reference_open != Some(field)).then_some(field);
            zork_ui::components::region::invalidate(cx, &["dialog", "page"]);
        }))
        .automation(AutomationRole::Button, "参照常见模型")
        .into_any_element()
    }
    fn reference_list(&self, field: Field, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        if self.reference_open != Some(field) {
            return None;
        }
        let id = self.model.read(cx).value().to_owned();
        let chips: Vec<_> = crate::api::model_catalog::references(field, Some(&id))
            .into_iter()
            .take(10)
            .enumerate()
            .map(|(i, r)| {
                let value = r.value.clone();
                self.chip(
                    gpui::SharedString::from(format!("model-reference-{i}")),
                    format!("{} · {}", r.name, r.label),
                    r.recognized,
                    cx,
                    move |v, cx| v.apply_reference(field, value.clone(), cx),
                )
            })
            .collect();
        Some(
            div()
                .id(gpui::SharedString::from(
                    format!("model-references-{field:?}").to_lowercase(),
                ))
                .pl(px(LABEL_WIDTH + 8.))
                .pb(px(4.))
                .flex()
                .flex_wrap()
                .gap(px(6.))
                .children(chips)
                .into_any_element(),
        )
    }
    /// A small ⓘ whose hint names the request field.
    fn request_hint(&self, cx: &gpui::App) -> Option<gpui::AnyElement> {
        let field = self.request_field(cx)?;
        let p = ZORK_UI.palette;
        let text = format!("请求字段：{field}");
        let mark = div()
            .id("model-thinking-field")
            .size(px(16.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .border(px(1.))
            .border_color(rgb(p.subtle))
            .text_size(px(12.))
            .line_height(px(14.))
            .text_color(rgb(p.muted))
            .child("i")
            .automation(AutomationRole::Status, text.clone());
        Some(zork_ui::components::tooltip::hint(mark, "model-thinking-field", text).into_any_element())
    }
    fn thinking_kind(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let kind = match &self.thinking_scheme {
            T::Unsupported => 0,
            T::Always { .. } => 1,
            T::Toggle { .. } => 2,
            T::Levels { .. } => 3,
            T::Budget { .. } => 4,
        };
        self.segmented(
            "model-thinking-kind",
            ["不支持", "固定开启", "开关", "档位", "预算"]
                .iter()
                .map(|s| (*s).to_owned())
                .collect(),
            Some(kind),
            cx,
            move |v, i, cx| {
                if i == kind {
                    return;
                }
                let next = match i {
                    0 => T::Unsupported,
                    1 => T::Always {
                        value: "high".into(),
                    },
                    2 => T::Toggle {
                        on: "enabled".into(),
                        default_on: true,
                    },
                    3 => T::Levels {
                        values: vec!["low".into(), "medium".into(), "high".into()],
                        default: "medium".into(),
                    },
                    _ => T::Budget {
                        presets: vec![4096, 16384, 32768],
                        default: BudgetChoice::Tokens(16384),
                        dynamic: false,
                        allow_off: true,
                    },
                };
                v.set_thinking(next, cx)
            },
        )
    }
    /// Controls for the chosen scheme, one or two rows under 思考方式.
    fn thinking_rows(&mut self, cx: &mut Context<Self>) -> Vec<gpui::AnyElement> {
        let p = ZORK_UI.palette;
        let hint = self.request_hint(cx);
        match self.thinking_scheme.clone() {
            T::Unsupported => vec![],
            T::Always { .. } => vec![self
                .row(
                    "",
                    div()
                        .text_size(px(12.5))
                        .text_color(rgb(p.muted))
                        .child("这个模型总是会思考")
                        .into_any_element(),
                    hint,
                )
                .into_any_element()],
            T::Toggle { on, default_on } => {
                let switch = ui::switch(
                    "model-thinking-default-on",
                    "默认开启",
                    default_on,
                    !self.busy,
                    &self.thinking_focus,
                    cx,
                    move |v, value, cx| {
                        v.set_thinking(
                            T::Toggle {
                                on: on.clone(),
                                default_on: value,
                            },
                            cx,
                        )
                    },
                );
                vec![self
                    .row(
                        "",
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(switch)
                            .child(div().text_size(px(12.5)).child("默认开启"))
                            .into_any_element(),
                        hint,
                    )
                    .into_any_element()]
            }
            T::Levels { values, default } => {
                let mut names: Vec<String> = ORDER.iter().map(|v| (*v).to_owned()).collect();
                for value in &values {
                    if !names.contains(value) {
                        names.push(value.clone());
                    }
                }
                let chips: Vec<_> = names
                    .into_iter()
                    .enumerate()
                    .map(|(i, name)| {
                        let on = values.contains(&name);
                        let (values, default) = (values.clone(), default.clone());
                        self.chip(
                            gpui::SharedString::from(format!("model-thinking-level-{i}")),
                            name.clone(),
                            on,
                            cx,
                            move |v, cx| {
                                let mut next = values.clone();
                                if on {
                                    next.retain(|x| x != &name);
                                } else {
                                    next.push(name.clone());
                                }
                                next.sort_by_key(|x| {
                                    ORDER.iter().position(|o| o == x).unwrap_or(ORDER.len())
                                });
                                let default = if next.contains(&default) {
                                    default.clone()
                                } else {
                                    next.first().cloned().unwrap_or_default()
                                };
                                v.set_thinking(
                                    T::Levels {
                                        values: next,
                                        default,
                                    },
                                    cx,
                                )
                            },
                        )
                    })
                    .collect();
                let selected = values.iter().position(|v| v == &default);
                let options = values.clone();
                let default_control = self.segmented(
                    "model-thinking-default",
                    values.clone(),
                    selected,
                    cx,
                    move |v, i, cx| {
                        if let Some(value) = options.get(i) {
                            v.set_thinking(
                                T::Levels {
                                    values: options.clone(),
                                    default: value.clone(),
                                },
                                cx,
                            )
                        }
                    },
                );
                vec![
                    self.row(
                        "",
                        div()
                            .flex()
                            .flex_wrap()
                            .gap(px(6.))
                            .children(chips)
                            .into_any_element(),
                        None,
                    )
                    .into_any_element(),
                    self.row("默认", default_control, hint).into_any_element(),
                ]
            }
            T::Budget {
                presets,
                default,
                dynamic,
                allow_off,
            } => {
                let mut options: Vec<(BudgetChoice, String)> = Vec::new();
                if allow_off {
                    options.push((BudgetChoice::Off, "关".into()));
                }
                if dynamic {
                    options.push((BudgetChoice::Dynamic, "动态".into()));
                }
                options.extend(
                    presets
                        .iter()
                        .map(|t| (BudgetChoice::Tokens(*t), compact_tokens(u64::from(*t)))),
                );
                let chips: Vec<_> = options
                    .into_iter()
                    .enumerate()
                    .map(|(i, (choice, label))| {
                        let presets = presets.clone();
                        self.chip(
                            gpui::SharedString::from(format!("model-thinking-budget-{i}")),
                            label,
                            choice == default,
                            cx,
                            move |v, cx| {
                                v.set_thinking(
                                    T::Budget {
                                        presets: presets.clone(),
                                        default: choice,
                                        dynamic,
                                        allow_off,
                                    },
                                    cx,
                                )
                            },
                        )
                    })
                    .collect();
                let custom = ui::input_control("model-thinking-budget", &self.budget_input, false, cx)
                    .w(px(88.))
                    .automation_enabled(!self.busy, AutomationRole::TextInput, "自定义思考预算")
                    .into_any_element();
                vec![self
                    .row(
                        "默认",
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(px(6.))
                            .children(chips)
                            .child(custom)
                            .into_any_element(),
                        hint,
                    )
                    .into_any_element()]
            }
        }
    }
    /// A typed budget such as `24` or `24K` becomes the default and a preset.
    pub(super) fn apply_budget_input(&mut self, cx: &mut Context<Self>) {
        let T::Budget {
            mut presets,
            dynamic,
            allow_off,
            ..
        } = self.thinking_scheme.clone()
        else {
            return;
        };
        let text = self.budget_input.read(cx).value().trim().to_uppercase();
        let Some(k) = text.trim_end_matches('K').trim().parse::<f32>().ok() else {
            return;
        };
        let tokens = (k * 1000.).round() as u32;
        if tokens == 0 {
            return;
        }
        if !presets.contains(&tokens) {
            presets.push(tokens);
            presets.sort_unstable();
        }
        self.set_thinking(
            T::Budget {
                presets,
                default: BudgetChoice::Tokens(tokens),
                dynamic,
                allow_off,
            },
            cx,
        );
    }
    /// Other models of this connection whose settings can be taken over.
    fn copy_sources(&self) -> Vec<Value> {
        self.detail
            .as_ref()
            .and_then(|detail| detail["models"].as_array())
            .into_iter()
            .flatten()
            .filter(|model| {
                model["id"].as_str() != self.editing_model.as_deref() && crate::api::copyable(model)
            })
            .cloned()
            .collect()
    }
    fn copy_link(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let sources = self.copy_sources();
        if sources.is_empty() {
            return None;
        }
        let open = self.copy_model_open;
        let chips: Vec<_> = open
            .then(|| {
                sources
                    .iter()
                    .enumerate()
                    .map(|(i, source)| {
                        let source = source.clone();
                        self.chip(
                            gpui::SharedString::from(format!("model-copy-{i}")),
                            source["id"].as_str().unwrap_or_default().to_owned(),
                            false,
                            cx,
                            move |v, cx| {
                                v.copy_model_open = false;
                                v.params_touched = true;
                                v.copy_model_configuration(source.clone(), cx);
                            },
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        Some(
            div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(
                    ui::quiet_button(
                        "model-copy-select",
                        "参考已有模型",
                        !self.busy,
                        ui::IconButtonSize::Compact,
                    )
                    .self_start()
                    .ml(px(-6.))
                    .text_size(px(12.5))
                    .child(ui::icon("icons/chevron-down.svg", 12.))
                    .on_click(cx.listener(|v, _, _, cx| {
                        v.copy_model_open = !v.copy_model_open;
                        zork_ui::components::region::invalidate(cx, &["dialog", "page"]);
                    }))
                    .automation(AutomationRole::Button, "参考已有模型"),
                )
                .when(open, |v| {
                    v.child(div().flex().flex_wrap().gap(px(6.)).children(chips))
                })
                .into_any_element(),
        )
    }
    pub(super) fn model_editor_body(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let p = ZORK_UI.palette;
        let open = self.model_params_open;
        let toggle_label = if open { "收起参数" } else { "调整参数" };
        let recognition = match self.recognized.clone() {
            Some((name, summary)) => div()
                .id("model-recognition-summary")
                .flex()
                .items_center()
                .gap(px(8.))
                .child(
                    div()
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(rgb(p.text))
                        .child(name.clone()),
                )
                .child(div().text_color(rgb(p.muted)).child(summary.clone()))
                .automation(AutomationRole::Status, format!("{name} · {summary}"))
                .into_any_element(),
            None => div()
                .text_color(rgb(p.subtle))
                .child("未识别的模型：展开参数手动填写")
                .into_any_element(),
        };
        let id_invalid = self
            .model_attempted
            .then(|| self.model_errors(cx))
            .unwrap_or_default()
            .iter()
            .any(|(f, _)| f == "profile-model");
        let mut body = div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(
                ui::input_control("profile-model", &self.model, id_invalid, cx)
                    .w_full()
                    .automation_enabled(!self.busy, AutomationRole::TextInput, "模型 ID"),
            )
            .children(self.field_error("profile-model", cx))
            .child(
                div()
                    .id("model-recognition")
                    .min_h(px(24.))
                    .px(px(4.))
                    .flex()
                    .items_center()
                    .text_size(px(12.5))
                    .child(recognition),
            )
            .child(
                ui::quiet_button(
                    "model-params-toggle",
                    toggle_label,
                    !self.busy,
                    ui::IconButtonSize::Compact,
                )
                .ml(px(-6.))
                .self_start()
                .text_size(px(12.5))
                .child(ui::icon("icons/chevron-down.svg", 12.).when(open, |icon| {
                    icon.with_transformation(gpui::Transformation::rotate(gpui::radians(
                        std::f32::consts::PI,
                    )))
                }))
                .on_click(cx.listener(|v, _, _, cx| {
                    v.model_params_open = !v.model_params_open;
                    v.reference_open = None;
                    zork_ui::components::region::invalidate(cx, &["dialog", "page"]);
                }))
                .automation(AutomationRole::Button, toggle_label),
            );
        if !open {
            return body;
        }
        let family = |api: &str| {
            provider_path(if api.starts_with("anthropic") {
                "anthropic"
            } else {
                "openai"
            })
        };
        let api = ui::dropdown_with_icons(
            "model-api-select",
            MODEL_APIS[self.model_api].1.into(),
            MODEL_APIS
                .iter()
                .enumerate()
                .map(|(i, (_, name))| (format!("model-api-{i}"), (*name).into(), i == self.model_api))
                .collect(),
            self.api_open,
            !self.busy,
            Some(family(MODEL_APIS[self.model_api].0)),
            MODEL_APIS.iter().map(|(api, _)| Some(family(api))).collect(),
            window,
            cx,
            |v, open, cx| {
                v.api_open = open;
                v.copy_model_open = false;
                zork_ui::components::region::invalidate_all(cx);
            },
            |v, index, cx| {
                v.model_api = index;
                v.api_open = false;
                v.api_touched = true;
                v.params_touched = true;
                zork_ui::components::region::invalidate_all(cx);
            },
        );
        let context = self.tokens_field("profile-context-limit", "上下文", &self.context_limit, cx);
        let output = self.tokens_field("profile-output-limit", "最长输出", &self.output_limit, cx);
        let context_ref = self.reference_toggle(Field::Context, cx);
        let output_ref = self.reference_toggle(Field::Output, cx);
        let thinking_ref = self.reference_toggle(Field::Thinking, cx);
        let caps_ref = self.reference_toggle(Field::Capabilities, cx);
        let kind = self.thinking_kind(cx);
        let thinking = self.thinking_rows(cx);
        let image = self.chip(
            "profile-model-image-input",
            self.locale.text("model_image_input").to_string(),
            self.model_image_input,
            cx,
            |v, cx| {
                v.model_image_input = !v.model_image_input;
                v.params_touched = true;
                zork_ui::components::region::invalidate(cx, &["dialog", "page"]);
            },
        );
        body = body
            .child(self.row("协议", div().w(px(220.)).child(api).into_any_element(), None))
            .child(self.row("上下文", context, Some(context_ref)))
            .children(self.field_error("profile-context-limit", cx))
            .children(self.reference_list(Field::Context, cx))
            .child(self.row("最长输出", output, Some(output_ref)))
            .children(self.field_error("profile-output-limit", cx))
            .children(self.reference_list(Field::Output, cx))
            .child(self.row("思考方式", kind, Some(thinking_ref)))
            .children(thinking)
            .children(self.field_error("profile-default-thinking", cx))
            .children(self.reference_list(Field::Thinking, cx))
            .child(self.row("能力", image, Some(caps_ref)))
            .children(self.reference_list(Field::Capabilities, cx))
            .children(self.copy_link(cx));
        body
    }
    /// 取消 / 保存 for either the dialog or the inline editor.
    pub(super) fn model_editor_actions(&self, cx: &mut Context<Self>) -> gpui::Div {
        div()
            .w_full()
            .flex()
            .items_center()
            .justify_end()
            .gap_2()
            .child(
                ui::button("profile-model-cancel", "取消", false, !self.busy)
                    .on_click(cx.listener(|v, _, _, cx| {
                        if !v.busy {
                            v.model_form_open = false;
                            v.editing_model = None;
                            zork_ui::components::region::invalidate_all(cx);
                        }
                    }))
                    .automation_enabled(!self.busy, AutomationRole::Button, "取消编辑模型"),
            )
            .child(
                ui::busy_button("profile-model-save", "保存模型", true, !self.busy, self.busy)
                    .on_click(cx.listener(|v, _, window, cx| v.save_model(window, cx)))
                    .automation_enabled(!self.busy, AutomationRole::Button, "保存模型"),
            )
    }
}
