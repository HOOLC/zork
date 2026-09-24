//! Adding a connection in three steps: provider, sign-in or key, models.
use super::*;
use std::rc::Rc;

/// Another device that a new connection could be saved on.
#[derive(Clone)]
pub struct SaveTarget {
    pub id: String,
    pub name: String,
    pub status: zork_ui::device_name::DeviceStatus,
}
/// Moves the add flow to another device, keeping the chosen provider.
pub type Retarget = Rc<dyn Fn(String, usize, &mut gpui::App)>;

impl ProfilesView {
    pub fn set_targets(&mut self, targets: Vec<SaveTarget>, retarget: Retarget) {
        self.targets = targets;
        self.retarget = Some(retarget);
    }
    /// Starts the flow with a provider already chosen, e.g. after a retarget.
    pub fn add_connection_with(&mut self, provider: usize, cx: &mut Context<Self>) {
        self.add_connection(cx);
        if provider < self.catalog.len() {
            self.choose_provider(provider);
        }
    }
    fn billings(&self, provider: usize) -> Vec<Value> {
        self.catalog
            .get(provider)
            .and_then(|p| p["billing"].as_array())
            .cloned()
            .unwrap_or_default()
    }
    /// Picks a provider; sign-in is preferred when the provider offers it.
    pub(super) fn choose_provider(&mut self, provider: usize) {
        let billings = self.billings(provider);
        let signin = billings.iter().any(|b| b["deviceCode"] == true);
        self.provider = provider;
        self.choose_access(signin);
    }
    fn choose_access(&mut self, subscription: bool) {
        let billings = self.billings(self.provider);
        let index = billings
            .iter()
            .position(|b| (b["deviceCode"] == true) == subscription)
            .unwrap_or(0);
        self.subscription = subscription;
        self.billing = index;
    }
    /// A connection id that does not exist yet, derived from the provider.
    fn new_connection_id(&self) -> String {
        let base = self
            .catalog
            .get(self.provider)
            .and_then(|p| p["id"].as_str())
            .unwrap_or("connection")
            .to_owned();
        (1..)
            .map(|n| if n == 1 { base.clone() } else { format!("{base}-{n}") })
            .find(|id| !self.profiles.iter().any(|p| &p.profile_id == id))
            .unwrap_or(base)
    }
    pub(super) fn submit_credentials(&mut self, signin: bool, cx: &mut Context<Self>) {
        let id = self.new_connection_id();
        self.id.update(cx, |v, cx| v.set_value(id.clone(), cx));
        self.pending_id = Some(id);
        if signin {
            self.start_auth(cx);
        } else {
            self.save_key(cx);
        }
    }
    /// Step 3: fetch the new connection's models so they can be chosen.
    pub(super) fn enter_models_step(&mut self, id: String, cx: &mut Context<Self>) {
        self.create_step = 3;
        self.created = Some(id.clone());
        self.created_detail = None;
        self.discovering = true;
        let name = self
            .selection()
            .and_then(|(p, b)| b["label"].as_str().or_else(|| p["label"].as_str()))
            .unwrap_or(&id)
            .to_owned();
        self.name.update(cx, |v, cx| v.set_value(name, cx));
        let source = self.source.clone();
        cx.spawn(async move |this, cx| {
            let _ = source.open_detail(&id).await;
            let _ = source.discover_models(&id).await;
            let _ = source.open_detail(&id).await;
            let _ = this.update(cx, |view, cx| {
                view.discovering = false;
                if view.created.as_deref() == Some(id.as_str()) {
                    view.created_detail = Some(source.detail(json!({"profile_id": id})));
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    fn toggle_created_model(&mut self, model: String, on: bool, cx: &mut Context<Self>) {
        let Some(id) = self.created.clone() else {
            return;
        };
        let source = self.source.clone();
        cx.spawn(async move |this, cx| {
            let result = source.set_model_enabled(&id, model, on).await;
            let _ = this.update(cx, |view, cx| {
                view.message = result.err().map(|e| e.to_string());
                view.created_detail = Some(source.detail(json!({"profile_id": id})));
                cx.notify();
            });
        })
        .detach();
    }
    fn finish_wizard(&mut self, cx: &mut Context<Self>) {
        if let Some(id) = self.created.clone() {
            let name = self.name.read(cx).value().trim().to_owned();
            if !name.is_empty() && name != id {
                let source = self.source.clone();
                cx.spawn(async move |_, _| {
                    let _ = source.rename(id, name).await;
                })
                .detach();
            }
        }
        self.form_open = false;
        self.create_step = 1;
        self.created = None;
        self.created_detail = None;
        self.pending_id = None;
        self.message = None;
        zork_ui::components::region::invalidate_all(cx);
    }
    fn steps(&self) -> gpui::AnyElement {
        let p = ZORK_UI.palette;
        div()
            .flex()
            .items_center()
            .gap(px(16.))
            .children(["选择供应商", "登录", "选择模型"].iter().enumerate().map(|(i, label)| {
                let step = i as u8 + 1;
                let (on, done) = (step == self.create_step, step < self.create_step);
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .text_size(px(12.5))
                    .text_color(rgb(if on { p.text } else { p.subtle }))
                    .when(on, |v| v.font_weight(gpui::FontWeight::SEMIBOLD))
                    .child(
                        div()
                            .size(px(20.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .text_size(px(12.))
                            .bg(rgb(if on { p.text } else { p.prompt }))
                            .text_color(rgb(if on {
                                p.canvas
                            } else if done {
                                p.success
                            } else {
                                p.muted
                            }))
                            .child(if done { "✓".to_owned() } else { step.to_string() }),
                    )
                    .child(*label)
            }))
            .into_any_element()
    }
    fn provider_cards(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let p = ZORK_UI.palette;
        let cards: Vec<_> = self
            .catalog
            .iter()
            .enumerate()
            .map(|(i, provider)| {
                let billings = provider["billing"].as_array().cloned().unwrap_or_default();
                let signin = billings.iter().any(|b| b["deviceCode"] == true);
                let key = billings.iter().any(|b| b["deviceCode"] != true);
                let badge = match (signin, key) {
                    (true, true) => "订阅 · API",
                    (true, false) => "订阅",
                    _ => "API",
                };
                let note = billings
                    .iter()
                    .filter_map(|b| b["label"].as_str())
                    .collect::<Vec<_>>()
                    .join(" / ");
                let label = provider["label"].as_str().unwrap_or_default().to_owned();
                let on = i == self.provider;
                div()
                    .id(gpui::SharedString::from(format!("profile-provider-card-{i}")))
                    .w(px(228.))
                    .p(px(12.))
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .rounded(px(zork_ui::design::RADIUS.block))
                    .border(px(if on { 2. } else { 1. }))
                    .border_color(rgb(if on { p.text } else { p.border }))
                    .cursor_pointer()
                    .when(!on, |v| {
                        v.hover(|s| s.bg(rgb(zork_ui::design::INTERACTION.neutral_hover)))
                    })
                    .on_click(cx.listener(move |v, _, _, cx| {
                        if !v.busy {
                            v.choose_provider(i);
                            cx.notify();
                        }
                    }))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(provider_icon(provider["id"].as_str().unwrap_or_default(), 20.))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(px(13.5))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child(label.clone()),
                            )
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .h(px(20.))
                                    .px(px(8.))
                                    .flex()
                                    .items_center()
                                    .rounded_full()
                                    .bg(rgb(p.prompt))
                                    .text_size(px(12.))
                                    .text_color(rgb(p.muted))
                                    .child(badge),
                            ),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_size(px(12.))
                            .text_color(rgb(p.subtle))
                            .child(note),
                    )
                    .automation(AutomationRole::Option, label)
            })
            .collect();
        div()
            .flex()
            .flex_wrap()
            .gap(px(8.))
            .children(cards)
            .into_any_element()
    }
    fn target_picker(&self, window: &mut Window, cx: &mut Context<Self>) -> gpui::AnyElement {
        let p = ZORK_UI.palette;
        let mut row = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .text_size(px(12.5))
            .text_color(rgb(p.muted))
            .child("保存到");
        if self.targets.len() <= 1 {
            return row
                .child(zork_ui::device_name::label(
                    "profile-create-device",
                    self.device_name.clone(),
                    &self.device_status,
                    None,
                ))
                .into_any_element();
        }
        let targets = self.targets.clone();
        let current = self
            .row_scope
            .clone()
            .and_then(|id| targets.iter().position(|t| t.id == id))
            .unwrap_or(0);
        row = row.child(ui::dropdown_with_icons(
            "profile-target-select",
            self.device_name.clone().into(),
            targets
                .iter()
                .enumerate()
                .map(|(i, t)| (format!("profile-target-{i}"), t.name.clone(), i == current))
                .collect(),
            self.target_open,
            !self.busy,
            None,
            vec![None; targets.len()],
            window,
            cx,
            |v, open, cx| {
                v.target_open = open;
                zork_ui::components::region::invalidate_all(cx);
            },
            move |v, index, cx| {
                v.target_open = false;
                let Some(target) = targets.get(index) else {
                    return;
                };
                if Some(target.id.as_str()) == v.row_scope.as_deref() {
                    return;
                }
                // This editor closes; the other device's editor takes over.
                v.form_open = false;
                if let Some(retarget) = v.retarget.clone() {
                    let (id, provider) = (target.id.clone(), v.provider);
                    cx.defer(move |cx| retarget(id, provider, cx));
                }
                zork_ui::components::region::invalidate_all(cx);
            },
        ));
        row.into_any_element()
    }
    fn access_choice(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let billings = self.billings(self.provider);
        let signin = billings.iter().any(|b| b["deviceCode"] == true);
        let key = billings.iter().any(|b| b["deviceCode"] != true);
        (signin && key).then(|| {
            zork_ui::components::widgets::controls::deferred_segmented(
                "profile-access-active",
                [("profile-access-true", "订阅账号"), ("profile-access-false", "API 接入")]
                    .into_iter()
                    .map(|(id, label)| zork_ui::components::widgets::controls::Segment {
                        id: id.into(),
                        label: label.into(),
                        disabled: false,
                    })
                    .collect(),
                vec![],
                Some(usize::from(!self.subscription)),
                zork_ui::components::widgets::controls::SegmentKind::Choice,
                !self.busy && self.attempt.is_none(),
                ZORK_UI.palette.canvas,
                cx.listener(|v, index: &usize, _, cx| {
                    if !v.busy && v.attempt.is_none() {
                        v.choose_access(*index == 0);
                        v.key.update(cx, |i, cx| i.clear(cx));
                        zork_ui::components::region::invalidate_all(cx);
                    }
                }),
            )
            .into_any_element()
        })
    }
    fn created_models(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let p = ZORK_UI.palette;
        if self.discovering {
            return div()
                .text_size(px(12.5))
                .text_color(rgb(p.muted))
                .child("正在获取模型…")
                .into_any_element();
        }
        let models = self
            .created_detail
            .as_ref()
            .and_then(|d| d["models"].as_array())
            .cloned()
            .unwrap_or_default();
        if models.is_empty() {
            return div()
                .text_size(px(12.5))
                .text_color(rgb(p.muted))
                .child("没有获取到模型，之后可以在连接里手动添加。")
                .into_any_element();
        }
        let rows: Vec<_> = models
            .into_iter()
            .map(|model| {
                let id = model["id"].as_str().unwrap_or_default().to_owned();
                self.model_switch_focus
                    .entry(id.clone())
                    .or_insert_with(|| cx.focus_handle());
                let on = model["enabled"].as_bool().unwrap_or(true);
                let summary = crate::api::model_catalog::model_summary(&model);
                let toggle = id.clone();
                div()
                    .h(px(40.))
                    .px(px(12.))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(px(13.))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .child(id.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(12.))
                            .text_color(rgb(p.subtle))
                            .child(summary),
                    )
                    .child(ui::switch(
                        format!("profile-new-model-{id}"),
                        id.clone(),
                        on,
                        true,
                        &self.model_switch_focus[&id],
                        cx,
                        move |v, value, cx| v.toggle_created_model(toggle.clone(), value, cx),
                    ))
            })
            .collect();
        div()
            .id("profile-new-models")
            .max_h(px(260.))
            .overflow_y_scroll()
            .mx(px(-12.))
            .flex()
            .flex_col()
            .children(rows)
            .into_any_element()
    }
    /// Body and actions of the add dialog for the current step.
    pub(super) fn wizard(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (gpui::AnyElement, gpui::AnyElement) {
        let p = ZORK_UI.palette;
        let enabled = !self.busy;
        let mut body = div().flex().flex_col().gap(px(14.)).child(self.steps());
        let mut actions = div().w_full().flex().items_center().justify_end().gap_2();
        let cancel = ui::button("profile-close-form", "取消", false, enabled)
            .on_click(cx.listener(|v, _, _, cx| {
                if !v.busy {
                    v.form_open = false;
                    v.create_step = 1;
                    v.key.update(cx, |key, cx| key.clear(cx));
                    zork_ui::components::region::invalidate_all(cx);
                }
            }))
            .automation_enabled(enabled, AutomationRole::Button, "取消添加");
        match self.create_step {
            3 => {
                body = body
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                div()
                                    .w(px(64.))
                                    .text_size(px(12.5))
                                    .text_color(rgb(p.muted))
                                    .child("名称"),
                            )
                            .child(
                                ui::input_control("profile-new-name", &self.name, false, cx)
                                    .flex_1()
                                    .automation(AutomationRole::TextInput, "连接名称"),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(12.5))
                            .text_color(rgb(p.muted))
                            .child("开启的模型会出现在新建 Chat 的模型面板里。"),
                    )
                    .child(self.created_models(cx));
                actions = actions.child(
                    ui::button("profile-finish", "完成", true, !self.discovering)
                        .on_click(cx.listener(|v, _, _, cx| v.finish_wizard(cx)))
                        .automation_enabled(!self.discovering, AutomationRole::Button, "完成"),
                );
            }
            2 => {
                let supports = self
                    .selection()
                    .is_some_and(|(_, b)| b["deviceCode"] == true);
                let custom = self
                    .selection()
                    .is_some_and(|(p, _)| p["id"] == "openai-compatible");
                let provider = self.catalog.get(self.provider).cloned().unwrap_or_default();
                body = body
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(provider_icon(provider["id"].as_str().unwrap_or_default(), 20.))
                            .child(
                                div()
                                    .text_size(px(13.5))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child(provider["label"].as_str().unwrap_or_default().to_owned()),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(rgb(p.subtle))
                                    .child(format!("保存到 {}", self.device_name)),
                            ),
                    )
                    .children(self.access_choice(cx))
                    .when(custom, |v| {
                        v.child(self.input("profile-base-url", "接口地址", &self.base_url, cx))
                    })
                    .when(!supports && self.attempt.is_none(), |v| {
                        v.child(self.input("profile-key", "API Key", &self.key, cx))
                    })
                    .when(supports && self.attempt.is_none(), |v| {
                        v.child(
                            div()
                                .text_size(px(12.5))
                                .text_color(rgb(p.muted))
                                .child("会在浏览器里登录这个订阅账号。"),
                        )
                    })
                    .children(self.attempt_block(cx));
                if self.attempt.is_some() {
                    actions = actions.child(
                        ui::button("profile-cancel", "取消登录", false, true)
                            .on_click(cx.listener(|v, _, _, cx| v.cancel(cx)))
                            .automation(AutomationRole::Button, "取消登录"),
                    );
                    if self
                        .attempt
                        .as_ref()
                        .is_some_and(|a| a["flow"] == "browser_callback")
                    {
                        actions = actions.child(
                            ui::button("profile-complete", "完成登录", true, true)
                                .on_click(cx.listener(|v, _, _, cx| v.poll_auth(cx)))
                                .automation(AutomationRole::Button, "完成登录"),
                        );
                    }
                } else {
                    actions = actions
                        .child(
                            ui::button("profile-back", "上一步", false, enabled)
                                .on_click(cx.listener(|v, _, window, cx| {
                                    if !v.busy {
                                        v.create_step = 1;
                                        v.message = None;
                                        // The pressed button leaves; keep keys in the dialog.
                                        window.focus(&v.modal.focus, cx);
                                        cx.notify();
                                    }
                                }))
                                .automation_enabled(enabled, AutomationRole::Button, "上一步"),
                        )
                        .child(
                            ui::busy_button(
                                if supports { "profile-signin" } else { "profile-save" },
                                if supports { "登录并连接" } else { "保存连接" },
                                true,
                                enabled,
                                self.busy,
                            )
                            .on_click(cx.listener(move |v, _, _, cx| {
                                v.submit_credentials(supports, cx)
                            }))
                            .automation_enabled(
                                enabled,
                                AutomationRole::Button,
                                if supports { "登录并连接" } else { "保存连接" },
                            ),
                        );
                }
            }
            _ => {
                body = body.child(self.provider_cards(cx));
                let target = self.target_picker(window, cx);
                actions = div()
                    .w_full()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().flex_1().min_w_0().child(target))
                    .child(cancel)
                    .child(
                        ui::button("profile-next", "下一步", true, enabled && !self.catalog.is_empty())
                            .on_click(cx.listener(|v, _, window, cx| {
                                if !v.busy {
                                    v.create_step = 2;
                                    v.message = None;
                                    window.focus(&v.modal.focus, cx);
                                    cx.notify();
                                }
                            }))
                            .automation_enabled(enabled, AutomationRole::Button, "下一步"),
                    );
            }
        }
        (body.into_any_element(), actions.into_any_element())
    }
}
