//! A connection opened from its list row: one page in the centered settings
//! column. Quota and models are sections; rare actions sit behind 更多.
use super::*;
use std::rc::Rc;

/// Beyond this many models the list stays virtualized and edits use a dialog.
pub(super) const INLINE_EDIT_LIMIT: usize = 100;
const MODEL_ROW: f32 = 44.;

pub(super) struct MenuItem {
    pub id: String,
    pub label: String,
    pub danger: bool,
    pub enabled: bool,
    pub run: Rc<dyn Fn(&mut ProfilesView, &mut Window, &mut Context<ProfilesView>)>,
}

impl ProfilesView {
    pub fn has_page(&self) -> bool {
        self.detail.is_some()
    }
    /// Inline editing is available while the list is short enough to lay out.
    pub(super) fn inline_editing(&self) -> bool {
        self.editor
            .as_ref()
            .is_some_and(|e| e.editing().is_some())
            && self
                .detail
                .as_ref()
                .and_then(|d| d["models"].as_array())
                .is_some_and(|m| m.len() <= INLINE_EDIT_LIMIT)
    }
    /// A popover menu behind a quiet trigger; one menu is open at a time.
    pub(super) fn menu(
        &self,
        id: String,
        trigger: zork_ui::automation::element::AutomationElement<ui::Action>,
        items: Vec<MenuItem>,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let owner = cx.entity().downgrade();
        let change = owner.clone();
        let key = id.clone();
        let items = Rc::new(items);
        ui::Popover::new(gpui::SharedString::from(format!("{id}-menu")))
            .rounded(px(zork_ui::design::RADIUS.container))
            .trigger(trigger)
            .open(self.menu_open.as_deref() == Some(id.as_str()))
            .on_open_change(move |open, _, app| {
                let key = key.clone();
                let _ = change.update(app, |view, cx| {
                    view.menu_open = open.then_some(key);
                    cx.notify();
                });
            })
            .content(move |_, _, cx| {
                let items = items.clone();
                owner
                    .update(cx, |view, cx| {
                        let p = ZORK_UI.palette;
                        div()
                            .min_w(px(160.))
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .children(items.iter().map(|item| {
                                let run = item.run.clone();
                                ui::quiet_button(
                                    item.id.clone(),
                                    item.label.clone(),
                                    item.enabled && !view.busy,
                                    ui::IconButtonSize::Standard,
                                )
                                .w_full()
                                .justify_start()
                                .when(item.danger, |v| v.text_color(rgb(p.danger)))
                                .on_click(cx.listener(move |view, _, window, cx| {
                                    view.menu_open = None;
                                    run(view, window, cx);
                                    cx.notify();
                                }))
                                .automation_enabled(
                                    item.enabled && !view.busy,
                                    AutomationRole::Button,
                                    item.label.clone(),
                                )
                            }))
                            .into_any_element()
                    })
                    .unwrap_or_else(|_| div().into_any_element())
            })
            .into_any_element()
    }
    /// Re-authorizes a subscription connection under its existing id.
    fn relogin(&mut self, cx: &mut Context<Self>) {
        let Some(detail) = &self.detail else {
            return;
        };
        if self.busy {
            return;
        }
        let id = detail["profile_id"].as_str().unwrap_or_default().to_owned();
        let provider = detail["provider"].as_str().unwrap_or_default().to_owned();
        let billing = detail["billing"].as_str().unwrap_or_default().to_owned();
        let source = self.source.clone();
        self.watch_source(cx);
        self.busy = true;
        self.message = None;
        cx.spawn(async move |this, cx| {
            let result = source.start_authorization(id, provider, billing).await;
            let _ = this.update(cx, |view, cx| {
                match result {
                    Ok(()) => {
                        let state = source.snapshot();
                        view.busy = state.authorization_busy;
                        view.attempt = state.authorization.clone();
                    }
                    Err(e) => {
                        view.busy = false;
                        view.message = Some(e.to_string());
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    /// Whether this connection signs in through a browser rather than a key.
    fn signs_in(&self, detail: &Value) -> bool {
        self.catalog
            .iter()
            .find(|p| p["id"] == detail["provider"])
            .and_then(|p| p["billing"].as_array())
            .and_then(|b| b.iter().find(|b| b["id"] == detail["billing"]))
            .is_some_and(|b| b["deviceCode"] == true)
    }
    pub(super) fn attempt_block(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let p = ZORK_UI.palette;
        let attempt = self.attempt.clone()?;
        let url = attempt["verification_url"].as_str().unwrap_or_default().to_owned();
        let code = attempt["user_code"].as_str().unwrap_or_default().to_owned();
        Some(
            div()
                .p(px(16.))
                .rounded(px(zork_ui::design::RADIUS.block))
                .bg(rgb(p.prompt))
                .flex()
                .flex_col()
                .gap(px(10.))
                .child(div().text_size(px(12.5)).text_color(rgb(p.muted)).child(
                    if code.is_empty() {
                        "在浏览器完成登录，然后粘贴返回的授权码。".to_owned()
                    } else {
                        format!("在浏览器输入 {code}，完成后会自动保存。")
                    },
                ))
                .child(
                    div().flex().gap_2().child(
                        ui::button("profile-open-browser", "打开登录页面", false, true)
                            .on_click(move |_, _, cx| cx.open_url(&url))
                            .automation(AutomationRole::Button, "打开登录页面"),
                    ),
                )
                .when(attempt["flow"] == "browser_callback", |v| {
                    v.child(
                        ui::input_control("profile-callback", &self.callback, false, cx)
                            .automation(AutomationRole::TextInput, "浏览器返回内容"),
                    )
                })
                .into_any_element(),
        )
    }
    /// `id · 128K · 可开关 · 按预设 | 待配置 · switch`. The row opens its
    /// inline editor; the switch is a sibling, not nested in the row action.
    fn model_row(&mut self, model: Value, cx: &mut Context<Self>) -> gpui::AnyElement {
        #[cfg(feature = "headless-bench")]
        {
            self.model_rows_built += 1;
        }
        let p = ZORK_UI.palette;
        let provider = self
            .detail
            .as_ref()
            .and_then(|d| d["provider"].as_str())
            .unwrap_or_default()
            .to_owned();
        let row = crate::api::model_catalog::model_row(&model, &provider);
        let id = row.id.clone();
        self.model_switch_focus
            .entry(id.clone())
            .or_insert_with(|| cx.focus_handle());
        let busy = self.busy || self.editor.as_ref().is_some_and(|e| e.busy);
        let editing = self.inline_editing()
            && self.editor.as_ref().and_then(|e| e.editing()) == Some(id.as_str());
        let toggle_id = id.clone();
        let edit_id = id.clone();
        let badge = if row.unconfigured {
            Some(
                div()
                    .flex_shrink_0()
                    .h(px(20.))
                    .px(px(8.))
                    .flex()
                    .items_center()
                    .rounded_full()
                    .bg(gpui::rgba((p.warning << 8) | 0x24))
                    .text_color(rgb(p.warning))
                    .text_size(px(11.5))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .child("待配置")
                    .into_any_element(),
            )
        } else if row.preset {
            Some(
                div()
                    .flex_shrink_0()
                    .text_size(px(12.))
                    .text_color(rgb(p.subtle))
                    .child("按预设")
                    .into_any_element(),
            )
        } else {
            None
        };
        let label = format!(
            "编辑 {id}{}",
            if row.unconfigured {
                " · 待配置".to_owned()
            } else {
                format!(" · {}", row.meta)
            }
        );
        div()
            .id(gpui::SharedString::from(format!("model-row-{id}")))
            .h(px(MODEL_ROW))
            .flex_shrink_0()
            .pr(px(10.))
            .flex()
            .items_center()
            .gap(px(10.))
            .child(
                ui::quiet_button(
                    gpui::SharedString::from(format!("model-edit-{id}")),
                    "",
                    !busy,
                    ui::IconButtonSize::Standard,
                )
                .selected(editing)
                .flex_1()
                .min_w_0()
                .h(px(MODEL_ROW))
                .pl(px(14.))
                .pr(px(4.))
                .justify_start()
                .gap(px(10.))
                .rounded_full()
                .child(
                    div()
                        .min_w_0()
                        .flex_shrink(1.)
                        .truncate()
                        .font_family(zork_ui::assets::CODE_FONT_FAMILY)
                        .text_size(px(12.5))
                        .font_weight(gpui::FontWeight::NORMAL)
                        .text_color(rgb(if row.enabled { p.text } else { p.muted }))
                        .child(id.clone()),
                )
                .when(!row.meta.is_empty(), |v| {
                    v.child(
                        div()
                            .flex_shrink_0()
                            .text_size(px(12.))
                            .font_weight(gpui::FontWeight::NORMAL)
                            .text_color(rgb(p.subtle))
                            .child(row.meta.clone()),
                    )
                })
                .children(badge)
                .child(div().flex_1())
                .on_click(cx.listener(move |v, _, _, cx| {
                    if !v.busy {
                        v.toggle_edit_model(&edit_id, cx);
                    }
                }))
                .automation_enabled(!busy, AutomationRole::Button, label),
            )
            .child(ui::switch(
                format!("model-enabled-{id}"),
                self.locale.text("model_enabled"),
                row.enabled,
                !busy && (row.enabled || !row.unconfigured),
                &self.model_switch_focus[&id],
                cx,
                move |v, on, cx| {
                    cx.stop_propagation();
                    v.set_model_enabled(toggle_id.clone(), on, cx);
                },
            ))
            .into_any_element()
    }
    fn section_title(&self, title: &'static str) -> gpui::Div {
        div()
            .min_h(px(32.))
            .flex()
            .items_center()
            .gap(px(10.))
            .child(
                div()
                    .text_size(px(14.))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(title),
            )
    }
    pub(super) fn render_page(
        &mut self,
        detail: Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let p = ZORK_UI.palette;
        // Escape needs a focus inside the page; take it when nothing holds focus.
        if std::mem::take(&mut self.page_focus_pending) || window.focused(cx).is_none() {
            window.focus(&self.page_focus, cx);
        }
        let profile_id = detail["profile_id"].as_str().unwrap_or_default().to_owned();
        let title = detail["name"]
            .as_str()
            .filter(|name| !name.trim().is_empty())
            .unwrap_or(&profile_id)
            .to_owned();
        let provider = self.catalog.iter().find(|x| x["id"] == detail["provider"]);
        let provider_name = provider
            .and_then(|x| x["label"].as_str())
            .unwrap_or_else(|| detail["provider"].as_str().unwrap_or_default())
            .to_owned();
        let billing = provider
            .and_then(|x| x["billing"].as_array())
            .and_then(|b| b.iter().find(|b| b["id"] == detail["billing"]))
            .and_then(|b| b["label"].as_str())
            .unwrap_or_default()
            .to_owned();
        let verified = detail["account"]["verified"]
            .as_bool()
            .or_else(|| detail["account"]["ok"].as_bool())
            .unwrap_or_else(|| detail["verified"].as_bool().unwrap_or(false));
        let fetch_label = format!("从 {provider_name} 获取");
        let mut meta = provider_name;
        if !billing.is_empty() {
            meta = format!("{meta} · {billing}");
        }
        if !self.device_name.is_empty() {
            meta = format!("{meta} · 保存在 {}", self.device_name);
        }
        let models = detail["models"].as_array().cloned().unwrap_or_default();
        let signs_in = self.signs_in(&detail);
        let refresh_id = profile_id.clone();
        let more = self.menu(
            "profile-more".into(),
            ui::icon_button("profile-more", !self.busy)
                .child(ui::icon("icons/more-horizontal.svg", 14.))
                .automation(AutomationRole::Button, "更多"),
            vec![
                MenuItem {
                    id: "profile-menu-rename".into(),
                    label: "重命名".into(),
                    danger: false,
                    enabled: true,
                    run: Rc::new(|v, window, cx| v.start_rename(window, cx)),
                },
                MenuItem {
                    id: "profile-quota-refresh".into(),
                    label: "刷新额度".into(),
                    danger: false,
                    enabled: !self.refreshing_profiles.contains(&profile_id),
                    run: Rc::new(move |v, _, cx| v.refresh_quota(refresh_id.clone(), cx)),
                },
                // Station's node API has no connection delete yet.
                MenuItem {
                    id: "profile-delete".into(),
                    label: "删除（暂不支持）".into(),
                    danger: true,
                    enabled: false,
                    run: Rc::new(|_, _, _| {}),
                },
            ],
            cx,
        );
        let name = if self.renaming {
            self.rename_editor(cx)
        } else {
            div()
                .flex()
                .items_center()
                .gap(px(4.))
                .min_w_0()
                .child(
                    div()
                        .truncate()
                        .text_size(px(17.))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(title.clone()),
                )
                .child(
                    ui::icon_button("profile-rename", !self.busy)
                        .child(ui::icon("icons/edit.svg", 14.))
                        .on_click(cx.listener(|v, _, window, cx| v.start_rename(window, cx)))
                        .automation_enabled(
                            !self.busy,
                            AutomationRole::Button,
                            self.locale.text("profile_rename"),
                        ),
                )
                .into_any_element()
        };
        let header = div()
            .flex()
            .items_center()
            .gap(px(12.))
            .child(
                ui::icon_button("profile-detail-dialog-close", true)
                    .child(ui::icon("icons/arrow-left.svg", 16.))
                    .on_click(cx.listener(|v, _, _, cx| {
                        // Back leaves the page, taking an open editor with it.
                        v.close_editor(cx);
                        v.close_dialog(cx);
                    }))
                    .automation(AutomationRole::Button, "返回模型连接"),
            )
            .child(provider_icon(detail["provider"].as_str().unwrap_or_default(), 28.))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(name)
                    .child(
                        div()
                            .id("profile-meta")
                            .truncate()
                            .text_size(px(12.))
                            .text_color(rgb(p.muted))
                            .child(meta.clone())
                            .automation(AutomationRole::Status, meta),
                    ),
            )
            // Status only matters when it needs attention.
            .when(!verified, |v| {
                v.child(
                    div()
                        .id("profile-verification")
                        .flex_shrink_0()
                        .h(px(22.))
                        .px(px(9.))
                        .flex()
                        .items_center()
                        .rounded_full()
                        .text_size(px(12.))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .bg(gpui::rgba((p.warning << 8) | 0x1f))
                        .text_color(rgb(p.warning))
                        .child(self.locale.text("profile_unverified")),
                )
            })
            .when(signs_in, |v| {
                v.child(
                    ui::button("profile-relogin", "重新登录", false, !self.busy)
                        .on_click(cx.listener(|v, _, _, cx| v.relogin(cx)))
                        .automation_enabled(!self.busy, AutomationRole::Button, "重新登录"),
                )
            })
            .child(more);
        let quota = self
            .quota
            .get(&profile_id)
            .filter(|q| q.visible())
            .map(|q| q.checked.clone());
        let editing_id = self
            .inline_editing()
            .then(|| self.editor.as_ref().and_then(|e| e.editing()).map(str::to_owned))
            .flatten();
        let model_count = models.len();
        let list = if model_count > INLINE_EDIT_LIMIT {
            let visible = model_count.min(10) as f32;
            div()
                .id("profile-models")
                .h(px(MODEL_ROW * visible))
                .w_full()
                .child(
                    gpui::uniform_list(
                        "profile-model-list",
                        model_count,
                        cx.processor(|view, range: std::ops::Range<usize>, _, cx| {
                            let models = view
                                .detail
                                .as_ref()
                                .and_then(|d| d["models"].as_array())
                                .map(|models| {
                                    models
                                        .iter()
                                        .skip(range.start)
                                        .take(range.len())
                                        .cloned()
                                        .collect::<Vec<_>>()
                                })
                                .unwrap_or_default();
                            models
                                .into_iter()
                                .map(|model| view.model_row(model, cx))
                                .collect::<Vec<_>>()
                        }),
                    )
                    .size_full(),
                )
                .automation(AutomationRole::ScrollArea, "模型列表")
                .into_any_element()
        } else {
            let mut rows: Vec<gpui::AnyElement> = Vec::new();
            for model in models {
                let id = model["id"].as_str().unwrap_or_default().to_owned();
                let is_editing = editing_id.as_deref() == Some(id.as_str());
                rows.push(self.model_row(model, cx));
                // Every row keeps its disclosure state so reopening animates in.
                let frame = zork_ui::motion::fold(
                    format!("model-inline-{id}"),
                    is_editing,
                    false,
                    window,
                    cx,
                );
                if let Some(frame) = frame.filter(|_| is_editing) {
                    let body = self.editor_body(false, window, cx);
                    let footer = self.editor_footer(false, cx);
                    let error = self.editor.as_ref().and_then(|e| e.error.clone());
                    rows.push(
                        frame.wrap(
                            div()
                                .id("model-inline-editor")
                                .mt(px(4.))
                                .mb(px(8.))
                                .pt(px(16.))
                                .px(px(18.))
                                .pb(px(14.))
                                .rounded(px(zork_ui::design::RADIUS.container))
                                .bg(rgb(p.prompt))
                                .flex()
                                .flex_col()
                                .gap(px(14.))
                                .child(body)
                                .when_some(error, |v, error| {
                                    v.child(ui::status_notice(error, ui::NoticeKind::Error))
                                })
                                .child(footer)
                                .automation(AutomationRole::Status, "编辑模型"),
                        ),
                    );
                }
            }
            div()
                .id("profile-models")
                .w_full()
                .flex()
                .flex_col()
                .gap(px(2.))
                .children(rows)
                .automation(AutomationRole::ScrollArea, "模型列表")
                .into_any_element()
        };
        let discovered = self
            .discovered
            .as_ref()
            .and_then(|value| value["message"].as_str())
            .map(str::to_owned);
        div()
            .id("profile-detail-dialog")
            .track_focus(&self.page_focus)
            .w_full()
            .flex()
            .flex_col()
            .gap(px(28.))
            .pb(px(24.))
            .on_key_down(cx.listener(|v, event: &gpui::KeyDownEvent, _, cx| {
                if event.keystroke.key != "escape" {
                    return;
                }
                if v.renaming || v.menu_open.is_some() {
                    return;
                }
                if v.editor.is_some() {
                    v.close_editor(cx);
                } else {
                    v.close_dialog(cx);
                }
                cx.stop_propagation();
                cx.notify();
            }))
            .child(header)
            .children(self.attempt_block(cx))
            .when_some(quota, |v, checked| {
                v.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(8.))
                        .child(self.section_title("额度").when_some(checked, |v, checked| {
                            v.child(
                                div()
                                    .id("profile-quota-updated")
                                    .text_size(px(12.))
                                    .text_color(rgb(p.subtle))
                                    .child(checked),
                            )
                        }))
                        .child(self.quota_detail(&profile_id)),
                )
            })
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(
                        self.section_title("模型")
                            .when_some(discovered, |v, message| {
                                v.child(
                                    div()
                                        .id("profile-model-update-result")
                                        .text_size(px(12.))
                                        .text_color(rgb(p.subtle))
                                        .child(message.clone())
                                        .automation(AutomationRole::Status, message),
                                )
                            })
                            .child(div().flex_1())
                            .child(
                                ui::busy_button(
                                    "profile-model-discover",
                                    fetch_label,
                                    false,
                                    !self.busy,
                                    self.discovering,
                                )
                                .text_size(px(12.))
                                .on_click(cx.listener(|v, _, _, cx| v.discover_models(cx)))
                                .automation(
                                    AutomationRole::Button,
                                    self.locale.text("profile_update_models"),
                                ),
                            )
                            .child(
                                ui::button("profile-model-add", "", false, !self.busy)
                                    .text_size(px(12.))
                                    .gap_1()
                                    .child(ui::icon("interface/plus.svg", 13.))
                                    .child("添加模型")
                                    .on_click(cx.listener(|v, _, _, cx| {
                                        if !v.busy {
                                            v.open_add_model(cx);
                                        }
                                    }))
                                    .automation(AutomationRole::Button, "添加模型"),
                            ),
                    )
                    .when(model_count == 0, |v| {
                        v.child(
                            div()
                                .text_size(px(12.5))
                                .text_color(rgb(p.muted))
                                .child("还没有模型：从供应商获取，或手动添加。"),
                        )
                    })
                    .when(model_count > 0, |v| v.child(list)),
            )
            .when_some(self.message.clone(), |v, m| {
                v.child(ui::status_notice(m, ui::NoticeKind::Error))
            })
            .automation(AutomationRole::Status, title)
            .into_any_element()
    }
}
