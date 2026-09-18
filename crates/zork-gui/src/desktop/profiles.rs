use super::{profile_quota::QuotaPresentation, ui};
#[cfg(feature = "headless-bench")]
use crate::api::StationClient;
use crate::api::{compact_tokens, ConnectionInput, ModelInput, MODEL_APIS};
use crate::i18n::Locale;
use crate::{
    api::ProfileInfo,
    automation::{AutomationElementExt, AutomationRole},
    components::text_input::ComposerInput,
    design::CUE_UI,
};
use gpui::{div, prelude::*, px, rgb, Context, Entity, Task, Window};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use zork_ui::controls::{provider_icon, provider_path};

pub struct ProfilesView {
    source: Arc<crate::api::Profiles>,
    source_updates: Option<Task<()>>,
    regions: zork_ui::components::region::Regions<Self>,
    device_name: String,
    modal: ui::ModalState,
    catalog: Vec<Value>,
    profiles: Vec<ProfileInfo>,
    locale: Locale,
    quota: HashMap<String, QuotaPresentation>,
    refreshing_profiles: HashSet<String>,
    quota_failures: HashSet<String>,
    loading_profiles: bool,
    quota_poll: Option<Task<()>>,
    helpers: Vec<Value>,
    provider: usize,
    subscription: bool,
    provider_open: bool,
    detail: Option<Value>,
    discovered: Option<Value>,
    model_switch_focus: HashMap<String, gpui::FocusHandle>,
    #[cfg(feature = "headless-bench")]
    model_rows_built: usize,
    model_form_open: bool,
    model_attempted: bool,
    editing_model: Option<String>,
    model_original: Option<Value>,
    renaming: bool,
    name: Entity<ComposerInput>,
    model_api: usize,
    model_image_input: bool,
    model_image_focus: gpui::FocusHandle,
    api_open: bool,
    copy_model_open: bool,
    copied_model: Option<Value>,
    thinking_levels: Entity<ComposerInput>,
    default_thinking: Entity<ComposerInput>,
    billing: usize,
    id: Entity<ComposerInput>,
    key: Entity<ComposerInput>,
    base_url: Entity<ComposerInput>,
    model: Entity<ComposerInput>,
    context_limit: Entity<ComposerInput>,
    output_limit: Entity<ComposerInput>,
    callback: Entity<ComposerInput>,
    form_open: bool,
    busy: bool,
    discovering: bool,
    message: Option<String>,
    attempt: Option<Value>,
}
impl ProfilesView {
    pub fn set_device_name(&mut self, name: String) {
        self.device_name = name;
    }
    pub fn new_with_source(source: Arc<crate::api::Profiles>, cx: &mut Context<Self>) -> Self {
        let mut view = Self::new_source(source, cx);
        view.refresh(cx);
        view
    }
    fn new_source(source: Arc<crate::api::Profiles>, cx: &mut Context<Self>) -> Self {
        let mut field = |label| {
            let input = cx.new(|cx| ComposerInput::new(label, cx));
            cx.subscribe(
                &input,
                |_, _, _: &crate::components::text_input::ComposerEdited, cx| {
                    zork_ui::components::region::invalidate(cx, &["dialog"])
                },
            )
            .detach();
            input
        };
        let id = field("例如：my-openai");
        let key = field("API Key");
        let base_url = field("兼容接口地址（可选）");
        let model = field("供应商提供的模型标识");
        let callback = field("粘贴浏览器返回的授权码或地址");
        let thinking_levels = field("推理级别，以逗号分隔");
        let default_thinking = field("默认推理级别");
        let context_limit = field("上下文 token 上限");
        let output_limit = field("输出 token 上限");
        let name = cx.new(|cx| ComposerInput::new("名称", cx).single_line());
        cx.subscribe(
            &name,
            |_, _, _: &crate::components::text_input::ComposerEdited, cx| {
                zork_ui::components::region::invalidate(cx, &["dialog"])
            },
        )
        .detach();
        cx.subscribe(
            &name,
            |view, _, _: &crate::components::text_input::ComposerSubmit, cx| {
                if view.renaming {
                    view.save_name(cx);
                }
            },
        )
        .detach();
        context_limit.update(cx, |v, cx| v.set_value("32K", cx));
        output_limit.update(cx, |v, cx| v.set_value("4.096K", cx));
        key.update(cx, |input, cx| input.set_secret(true, cx));
        callback.update(cx, |input, cx| input.set_secret(true, cx));
        Self {
            modal: ui::ModalState::new(cx),
            source,
            source_updates: None,
            regions: Default::default(),
            device_name: String::new(),
            catalog: vec![],
            profiles: vec![],
            locale: Locale::default(),
            quota: HashMap::new(),
            refreshing_profiles: HashSet::new(),
            quota_failures: HashSet::new(),
            loading_profiles: false,
            quota_poll: None,
            helpers: vec![],
            provider: 0,
            subscription: true,
            provider_open: false,
            detail: None,
            discovered: None,
            model_switch_focus: HashMap::new(),
            #[cfg(feature = "headless-bench")]
            model_rows_built: 0,
            model_form_open: false,
            model_attempted: false,
            editing_model: None,
            model_original: None,
            renaming: false,
            name,
            model_api: 0,
            model_image_input: false,
            model_image_focus: cx.focus_handle(),
            api_open: false,
            copy_model_open: false,
            copied_model: None,
            thinking_levels,
            default_thinking,
            billing: 0,
            id,
            key,
            base_url,
            model,
            context_limit,
            output_limit,
            callback,
            form_open: false,
            busy: false,
            discovering: false,
            message: None,
            attempt: None,
        }
    }
    #[cfg(feature = "headless-bench")]
    pub fn headless_fixture(detail: bool, cx: &mut Context<Self>) -> Self {
        #[cfg(not(target_family = "wasm"))]
        let client = Arc::new(StationClient::fixture(zork_ui::stories::page_fixture(),
            serde_json::from_str(include_str!("../../tests/fixtures/provider_catalog.json")).expect("provider fixture")));
        #[cfg(target_family = "wasm")]
        let client = Arc::new(StationClient::new("http://127.0.0.1:9", None));
        let mut view = Self::new_source(crate::api::Profiles::new(client), cx);
        view.catalog = serde_json::from_str::<Value>(include_str!(
            "../../tests/fixtures/provider_catalog.json"
        ))
        .expect("provider catalog fixture")["providers"]
            .as_array()
            .unwrap()
            .clone();
        view.select_access(true);
        view.provider = view
            .catalog
            .iter()
            .position(|p| p["id"] == "openai")
            .unwrap();
        view.billing = view.catalog[view.provider]["billing"]
            .as_array()
            .unwrap()
            .iter()
            .position(|b| b["id"] == "subscription")
            .unwrap();
        let fixture = zork_ui::stories::page_fixture();
        view.device_name = fixture["device"]["name"].as_str().unwrap().into();
        view.profiles = vec![serde_json::from_value(fixture["profile"].clone()).unwrap()];
        view.helpers = fixture["agents"].as_array().unwrap().clone();
        if detail {
            view.detail = Some(fixture["profile"].clone());
        }
        view.accept_profiles(view.profiles.clone());
        view
    }
    #[cfg(feature = "headless-bench")]
    pub fn headless_state(&self, cx: &gpui::App) -> Value {
        json!({"connection_name":self.id.read(cx).value(),"provider":self.catalog.get(self.provider).map(|p|p["id"].clone()),
            "model_id":self.model.read(cx).value(),"context_window":self.context_limit.read(cx).value(),"max_output_tokens":self.output_limit.read(cx).value(),
            "device":self.device_name,"model_form":self.model_form_open,"detail":self.detail.as_ref().map(|d|d["profile_id"].clone())})
    }
    #[cfg(feature = "headless-bench")]
    pub fn headless_connection_name(&self, cx: &gpui::App) -> String {
        self.id.read(cx).value().to_owned()
    }
    pub fn set_locale(&mut self, locale: Locale, cx: &mut Context<Self>) {
        if self.locale != locale {
            self.locale = locale;
            self.rebuild_quota();
            zork_ui::components::region::invalidate_all(cx);
        }
    }
    pub fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if !visible {
            self.quota_poll = None;
        } else if self.quota_poll.is_none() {
            self.watch_source(cx);
            let source = self.source.clone();
            self.quota_poll = Some(cx.spawn(async move |_, _| source.while_visible().await));
        }
    }
    fn watch_source(&mut self, cx: &mut Context<Self>) {
        if self.source_updates.is_some() {
            return;
        }
        let mut updates = self.source.subscribe();
        self.apply_source(updates.snapshot(), cx);
        self.source_updates = Some(cx.spawn(async move |this, cx| {
            while let Some(update) = updates.changed().await {
                if this.update(cx, |v, cx| v.apply_source(update, cx)).is_err() {
                    return;
                }
            }
        }));
    }
    fn apply_source(&mut self, update: crate::api::ProfileUpdate, cx: &mut Context<Self>) {
        let state = update.state;
        let initial = self.catalog.is_empty();
        self.profiles = state.profiles.as_ref().clone();
        if self.attempt.is_some() && state.authorization.is_none() {
            self.busy = false;
            self.callback.update(cx, |v, cx| v.clear(cx));
            if state.authorization_complete {
                self.form_open = false;
            }
        }
        self.attempt = state.authorization.clone();
        if self.attempt.is_some() {
            self.busy = state.authorization_busy;
        }
        if let Some(error) = &state.authorization_error {
            self.message = Some(error.clone());
        }
        self.catalog = state.providers.as_ref().clone();
        self.helpers = state.helpers.as_ref().clone();
        self.loading_profiles = state.loading;
        self.refreshing_profiles = state.refreshing.as_ref().clone();
        self.quota_failures = state.failed.as_ref().clone();
        if initial && !self.catalog.is_empty() {
            self.select_access(self.subscription);
        }
        if let Some(detail) = self.detail.take() {
            self.detail = Some(self.source.detail(detail));
        }
        if state.error.is_some() && self.profiles.is_empty() {
            self.message = Some(self.locale.text("quota_query_failed").into());
        } else if state.error.is_none()
            && self.message.as_deref() == Some(self.locale.text("quota_query_failed"))
        {
            self.message = None;
        }
        self.rebuild_quota();
        let mut names = update
            .records
            .iter()
            .map(|id| format!("profile/{id}"))
            .collect::<Vec<_>>();
        names.push("dialog".into());
        if update.status_changed {
            names.push("feedback".into());
            names.push("empty".into());
        }
        if update.structure_changed || update.catalog_changed {
            names.push("header".into());
            names.push("empty".into());
        }
        zork_ui::components::region::invalidate(
            cx,
            &names.iter().map(String::as_str).collect::<Vec<_>>(),
        );
    }
    fn rebuild_quota(&mut self) {
        self.quota = self
            .profiles
            .iter()
            .map(|profile| {
                (
                    profile.profile_id.clone(),
                    if self.quota_failures.contains(&profile.profile_id) {
                        QuotaPresentation::failure(self.locale)
                    } else {
                        QuotaPresentation::new(profile, self.locale)
                    },
                )
            })
            .collect();
    }
    #[cfg(feature = "headless-bench")]
    fn accept_profiles(&mut self, profiles: Vec<ProfileInfo>) {
        self.source.seed(crate::api::ProfileData {
            profiles: Arc::new(profiles.clone()),
            providers: Arc::new(self.catalog.clone()),
            helpers: Arc::new(self.helpers.clone()),
            ..Default::default()
        });
        if let Some(detail) = self.detail.take() {
            self.detail = Some(self.source.detail(detail));
        }
        self.profiles = profiles;
        self.rebuild_quota();
    }
    #[cfg(feature = "headless-bench")]
    pub fn headless_set_profile(&mut self, value: Value, cx: &mut Context<Self>) {
        if self.detail.is_some() {
            self.detail = Some(value.clone());
        }
        self.quota_failures.clear();
        self.accept_profiles(vec![serde_json::from_value(value).unwrap()]);
        zork_ui::components::region::invalidate_all(cx);
    }
    #[cfg(feature = "headless-bench")]
    pub fn headless_set_client(&mut self, client: Arc<StationClient>) {
        self.source = crate::api::Profiles::new(client);
        self.source_updates = None;
        self.accept_profiles(self.profiles.clone());
    }
    fn refresh_quota(&mut self, id: String, cx: &mut Context<Self>) {
        self.watch_source(cx);
        let source = self.source.clone();
        cx.spawn(async move |_, _| source.refresh_quota(id).await)
            .detach();
    }
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.watch_source(cx);
        let source = self.source.clone();
        cx.spawn(async move |_, _| source.refresh().await).detach();
    }
    fn quota_detail(&self, id: &str) -> gpui::AnyElement {
        let p = CUE_UI.palette;
        let Some(quota) = self.quota.get(id).filter(|q| q.visible()) else {
            return div().into_any_element();
        };
        div()
            .id("profile-quota")
            .mb_4()
            .flex()
            .flex_col()
            .gap_3()
            .when(quota.failed, |v| {
                v.child(
                    div()
                        .text_size(px(11.))
                        .text_color(rgb(p.warning))
                        .child(self.locale.text("quota_query_failed")),
                )
            })
            .children(quota.windows.iter().enumerate().map(|(index, window)| {
                let color = if window.remaining == 0. {
                    p.danger
                } else if window.remaining < 20. {
                    p.warning
                } else {
                    p.success
                };
                div()
                    .id(format!("profile-quota-window-{index}"))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .text_size(px(13.))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .child(window.label.clone()),
                            )
                            .child(div().text_color(rgb(color)).child(window.value.clone())),
                    )
                    .child(
                        div()
                            .w_full()
                            .h(px(4.))
                            .rounded_full()
                            .bg(rgb(p.border))
                            .child(
                                div()
                                    .h_full()
                                    .w(gpui::relative(window.remaining / 100.))
                                    .rounded_full()
                                    .bg(rgb(color)),
                            ),
                    )
                    .when_some(window.reset.clone(), |v, reset| {
                        v.child(
                            div()
                                .text_size(px(11.))
                                .text_color(rgb(p.muted))
                                .child(reset),
                        )
                    })
                    .automation(
                        AutomationRole::Status,
                        format!(
                            "{} {} {}",
                            window.label,
                            window.value,
                            window.reset.as_deref().unwrap_or("")
                        ),
                    )
            }))
            .when_some(quota.balance.clone(), |v, balance| {
                v.child(div().text_size(px(13.)).child(balance))
            })
            .automation(AutomationRole::Status, quota.summary.clone())
            .into_any_element()
    }
    fn selection(&self) -> Option<(&Value, &Value)> {
        let provider = self.catalog.get(self.provider)?;
        let billing = provider["billing"].as_array()?.get(self.billing)?;
        Some((provider, billing))
    }
    fn select_access(&mut self, subscription: bool) {
        self.subscription = subscription;
        if let Some(&(p, b)) = crate::api::connection_options(&self.catalog, subscription).first() {
            self.provider = p;
            self.billing = b;
        }
        self.provider_open = false;
    }
    fn open_detail(&mut self, id: String, cx: &mut Context<Self>) {
        self.watch_source(cx);
        let source = self.source.clone();
        cx.spawn(async move |this, cx| {
            let result = source.open_detail(&id).await;
            let _ = this.update(cx, |v, cx| {
                match result {
                    Ok(()) => {
                        v.detail = Some(source.detail(json!({"profile_id":id})));
                        v.renaming = false;
                        v.form_open = false;
                        v.model_form_open = false;
                        v.discovered = None;
                        v.model.update(cx, |m, cx| m.clear(cx));
                    }
                    Err(e) => v.message = Some(e.to_string()),
                }
                zork_ui::components::region::invalidate_all(cx);
            });
        })
        .detach();
    }
    fn start_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(detail) = &self.detail else {
            return;
        };
        let name = detail["name"]
            .as_str()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| detail["profile_id"].as_str())
            .unwrap_or_default()
            .to_owned();
        self.name.update(cx, |input, cx| {
            input.set_value(name, cx);
        });
        window.focus(&self.name.read(cx).focus_handle(), cx);
        self.renaming = true;
        self.message = None;
        zork_ui::components::region::invalidate(cx, &["dialog"]);
    }
    fn save_name(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let Some(id) = self
            .detail
            .as_ref()
            .and_then(|d| d["profile_id"].as_str())
            .map(str::to_owned)
        else {
            return;
        };
        let name = self.name.read(cx).value().trim().to_owned();
        self.watch_source(cx);
        self.busy = true;
        let source = self.source.clone();
        cx.spawn(async move |this, cx| {
            let result = source.rename(id, name).await;
            let _ = this.update(cx, |view, cx| {
                view.busy = false;
                match result {
                    Ok(()) => {
                        view.renaming = false;
                        view.message = None;
                        if let Some(detail) = view.detail.take() {
                            view.detail = Some(source.detail(detail));
                        }
                    }
                    Err(_) => {
                        view.message = Some(view.locale.text("profile_name_save_failed").into())
                    }
                }
                zork_ui::components::region::invalidate(cx, &["dialog"]);
            });
        })
        .detach();
        zork_ui::components::region::invalidate(cx, &["dialog"]);
    }
    fn rename_editor(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        ui::input_control("profile-name", &self.name, self.message.is_some(), cx)
            .flex_1()
            .min_w_0()
            .on_key_down(cx.listener(|v, event: &gpui::KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" && !v.busy {
                    v.renaming = false;
                    v.message = None;
                    cx.stop_propagation();
                    zork_ui::components::region::invalidate(cx, &["dialog"]);
                }
            }))
            .automation(AutomationRole::TextInput, self.locale.text("profile_name"))
            .into_any_element()
    }
    fn edit_model(&mut self, model: Option<Value>, cx: &mut Context<Self>) {
        let Some(detail) = &self.detail else {
            return;
        };
        self.apply_model_input(crate::api::model_form(detail, &self.catalog, model), cx);
        self.copy_model_open = false;
        self.api_open = false;
        self.model_form_open = true;
        self.model_attempted = false;
        self.message = None;
        zork_ui::components::region::invalidate_all(cx);
    }
    fn apply_model_input(&mut self, input: ModelInput, cx: &mut Context<Self>) {
        self.editing_model = input
            .previous
            .as_ref()
            .and_then(|m| m["id"].as_str())
            .map(str::to_owned);
        self.model_original = input.previous;
        self.copied_model = input.copied;
        self.model_api = MODEL_APIS
            .iter()
            .position(|a| a.0 == input.api)
            .unwrap_or(0);
        self.model_image_input = input.images;
        self.model.update(cx, |v, cx| v.set_value(input.id, cx));
        self.context_limit
            .update(cx, |v, cx| v.set_value(input.context, cx));
        self.output_limit
            .update(cx, |v, cx| v.set_value(input.output, cx));
        self.thinking_levels
            .update(cx, |v, cx| v.set_value(input.thinking, cx));
        self.default_thinking
            .update(cx, |v, cx| v.set_value(input.default_thinking, cx));
    }
    fn copy_model_configuration(&mut self, source: Value, cx: &mut Context<Self>) {
        self.apply_model_input(crate::api::copy_form(self.model_input(cx), source), cx);
        zork_ui::components::region::invalidate_all(cx);
    }

    fn copy_model_action(&self, window: &mut Window, cx: &mut Context<Self>) -> gpui::AnyElement {
        let sources = self
            .detail
            .as_ref()
            .and_then(|detail| detail["models"].as_array())
            .into_iter()
            .flatten()
            .filter(|model| {
                model["id"].as_str() != self.editing_model.as_deref() && crate::api::copyable(model)
            })
            .cloned()
            .collect::<Vec<_>>();
        ui::dropdown_with_icons(
            "model-copy-select",
            "复制配置".into(),
            sources
                .iter()
                .enumerate()
                .map(|(index, model)| {
                    (
                        format!("model-copy-{index}"),
                        model["id"].as_str().unwrap_or_default().to_owned(),
                        false,
                    )
                })
                .collect(),
            self.copy_model_open,
            !self.busy && !sources.is_empty(),
            None,
            vec![None; sources.len()],
            window,
            cx,
            |view, open, cx| {
                view.copy_model_open = open;
                view.api_open = false;
                zork_ui::components::region::invalidate_all(cx);
            },
            move |view, index, cx| {
                view.copy_model_open = false;
                if let Some(source) = sources.get(index) {
                    view.copy_model_configuration(source.clone(), cx);
                }
            },
        )
    }

    fn discover_models(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let Some(id) = self
            .detail
            .as_ref()
            .and_then(|d| d["profile_id"].as_str())
            .map(str::to_owned)
        else {
            return;
        };
        let source = self.source.clone();
        self.busy = true;
        self.discovering = true;
        self.message = None;
        self.discovered = None;
        cx.spawn(async move |this, cx| {
            let result = source.discover_models(&id).await;
            let _ = this.update(cx, |v, cx| {
                v.busy = false;
                v.discovering = false;
                match result {
                    Ok(value) => {
                        let count = value["added"].as_u64().unwrap_or(0);
                        let configured = value["configured"].as_u64().unwrap_or(0);
                        let mut text = if count > 0 {
                            v.locale
                                .text("models_added")
                                .replace("{count}", &count.to_string())
                        } else if configured > 0 {
                            v.locale
                                .text("models_configured")
                                .replace("{count}", &configured.to_string())
                        } else {
                            v.locale.text("models_up_to_date").to_owned()
                        };
                        if value["truncated"] == true {
                            text.push_str(v.locale.text("models_partial"));
                        }
                        v.discovered =
                            Some(json!({"message":text,"truncated":value["truncated"]==true}));
                        v.message = None;
                        v.refresh(cx);
                    }
                    Err(e) => v.message = Some(e.to_string()),
                }
                zork_ui::components::region::invalidate_all(cx);
            });
        })
        .detach();
        zork_ui::components::region::invalidate_all(cx);
    }
    #[cfg(feature = "headless-bench")]
    pub fn headless_model_rows_built(&mut self) -> usize {
        std::mem::take(&mut self.model_rows_built)
    }
    fn render_model_row(&mut self, model: Value, cx: &mut Context<Self>) -> gpui::AnyElement {
        #[cfg(feature = "headless-bench")]
        {
            self.model_rows_built += 1;
        }
        let p = CUE_UI.palette;
        let edit = model.clone();
        let id = model["id"].as_str().unwrap_or_default().to_owned();
        let remove = id.clone();
        self.model_switch_focus
            .entry(id.clone())
            .or_insert_with(|| cx.focus_handle());
        let active = model["enabled"].as_bool().unwrap_or(true);
        let toggle_id = id.clone();
        let limits = model["limits"].as_object();
        let note = match limits
            .and_then(|v| v.get("context_window_tokens"))
            .and_then(Value::as_u64)
            .zip(
                limits
                    .and_then(|v| v.get("max_output_tokens"))
                    .and_then(Value::as_u64),
            ) {
            Some((context, output)) => format!(
                "上下文 {} / 输出 {}",
                compact_tokens(context),
                compact_tokens(output)
            ),
            None => self.locale.text("model_needs_configuration").into(),
        };
        div()
            .id(format!("model-edit-{id}"))
            .group("profile-model-row")
            .cursor_pointer()
            .w_full()
            .h(px(64.))
            .py_3()
            .flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .truncate()
                            .text_size(px(13.))
                            .line_height(px(20.))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(rgb(if active { p.text } else { p.muted }))
                            .child(id.clone()),
                    )
                    .child(
                        div()
                            .id(format!("model-limits-{id}"))
                            .truncate()
                            .text_size(px(11.))
                            .line_height(px(16.))
                            .text_color(rgb(p.muted))
                            .child(note.clone())
                            .automation(AutomationRole::Status, note),
                    ),
            )
            .child(
                ui::button(format!("model-remove-{id}"), "移除", false, !self.busy)
                    .h(px(ui::CONTROL_HEIGHT))
                    .px(px(ui::BUTTON_PADDING_X))
                    .text_size(px(11.))
                    .opacity(0.)
                    .group_hover("profile-model-row", |v| v.opacity(1.))
                    .on_click(cx.listener(move |v, _, _, cx| {
                        cx.stop_propagation();
                        v.remove_model(&remove, cx);
                    })),
            )
            .child(ui::switch(
                format!("model-enabled-{id}"),
                self.locale.text("model_enabled"),
                active,
                !self.busy && (active || limits.is_some()),
                &self.model_switch_focus[&id],
                cx,
                move |v, on, cx| {
                    cx.stop_propagation();
                    v.set_model_enabled(toggle_id.clone(), on, cx);
                },
            ))
            .on_click(cx.listener(move |v, _, _, cx| {
                if !v.busy {
                    v.edit_model(Some(edit.clone()), cx);
                }
            }))
            .map(|row| self.modal.source("model-editor-dialog").bind(row, id.clone(), ui::ActionStyle { disabled: self.busy, ..Default::default() }))
            .automation(AutomationRole::Button, format!("编辑 {id}"))
            .into_any_element()
    }

    fn set_model_enabled(&mut self, model_id: String, enabled: bool, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let Some(id) = self
            .detail
            .as_ref()
            .and_then(|d| d["profile_id"].as_str())
            .map(str::to_owned)
        else {
            return;
        };
        self.busy = true;
        self.message = None;
        let source = self.source.clone();
        cx.spawn(async move |this, cx| {
            let result = source.set_model_enabled(&id, model_id, enabled).await;
            let _ = this.update(cx, |view, cx| {
                view.busy = false;
                view.message = result.err().map(|e| e.to_string());
                zork_ui::components::region::invalidate_all(cx);
            });
        })
        .detach();
        zork_ui::components::region::invalidate_all(cx);
    }

    fn remove_model(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let Some(detail) = &self.detail else {
            return;
        };
        let Some(model) = detail["models"]
            .as_array()
            .and_then(|m| m.iter().find(|m| m["id"] == id))
            .cloned()
        else {
            return;
        };
        let profile = detail["profile_id"].as_str().unwrap_or_default().to_owned();
        let source = self.source.clone();
        self.busy = true;
        cx.spawn(async move |this, cx| {
            let result = source.remove_model(&profile, model).await;
            let _ = this.update(cx, |view, cx| {
                view.busy = false;
                view.message = result.err().map(|e| e.to_string());
                zork_ui::components::region::invalidate_all(cx);
            });
        })
        .detach();
        zork_ui::components::region::invalidate_all(cx);
    }
    fn model_input(&self, cx: &gpui::App) -> ModelInput {
        ModelInput {
            previous: self.model_original.clone(),
            copied: self.copied_model.clone(),
            id: self.model.read(cx).value().into(),
            api: MODEL_APIS[self.model_api].0.into(),
            context: self.context_limit.read(cx).value().into(),
            output: self.output_limit.read(cx).value().into(),
            thinking: self.thinking_levels.read(cx).value().into(),
            default_thinking: self.default_thinking.read(cx).value().into(),
            images: self.model_image_input,
        }
    }
    fn model_errors(&self, cx: &gpui::App) -> Vec<(String, String)> {
        let id = self
            .detail
            .as_ref()
            .and_then(|d| d["profile_id"].as_str())
            .unwrap_or_default();
        self.source
            .model_errors(id, &self.model_input(cx))
            .into_iter()
            .map(|e| (e.field, e.message))
            .collect()
    }
    fn save_model(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.model_attempted = true;
        self.message = None;
        if let Some((field, _)) = self.model_errors(cx).first() {
            let input = match field.as_str() {
                "profile-context-limit" => &self.context_limit,
                "profile-output-limit" => &self.output_limit,
                "profile-default-thinking" => &self.default_thinking,
                _ => &self.model,
            };
            window.focus(&input.read(cx).focus_handle(), cx);
            zork_ui::components::region::invalidate_all(cx);
            return;
        }
        let Some(id) = self
            .detail
            .as_ref()
            .and_then(|d| d["profile_id"].as_str())
            .map(str::to_owned)
        else {
            return;
        };
        let input = self.model_input(cx);
        let source = self.source.clone();
        self.busy = true;
        cx.spawn(async move |this, cx| {
            let result = source.save_model(&id, input).await;
            let _ = this.update(cx, |view, cx| {
                view.busy = false;
                match result {
                    Ok(()) => {
                        view.model_form_open = false;
                        view.editing_model = None;
                        view.model_original = None;
                    }
                    Err(error) => view.message = Some(error.to_string()),
                }
                zork_ui::components::region::invalidate_all(cx);
            });
        })
        .detach();
        zork_ui::components::region::invalidate_all(cx);
    }
    fn cancel(&mut self, cx: &mut Context<Self>) {
        let source = self.source.clone();
        cx.spawn(async move |_, _| {
            let _ = source.cancel_authorization().await;
        })
        .detach();
        self.busy = false;
        self.callback.update(cx, |v, cx| v.clear(cx));
        zork_ui::components::region::invalidate_all(cx);
    }
    fn save_key(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let Some((provider, billing)) = self.selection() else {
            return;
        };
        let input = ConnectionInput {
            id: self.id.read(cx).value().into(),
            key: self.key.read(cx).value().into(),
            base_url: self.base_url.read(cx).value().into(),
            provider: provider["id"].as_str().unwrap_or_default().into(),
            billing: billing["id"].as_str().unwrap_or_default().into(),
        };
        self.busy = true;
        self.message = None;
        let source = self.source.clone();
        cx.spawn(async move |this, cx| {
            let result = source.save_connection(input).await;
            let _ = this.update(cx, |view, cx| {
                view.busy = false;
                match result {
                    Ok(()) => {
                        view.form_open = false;
                        view.key.update(cx, |v, cx| v.clear(cx));
                        view.message = None;
                    }
                    Err(e) => view.message = Some(e.to_string()),
                }
                zork_ui::components::region::invalidate_all(cx);
            });
        })
        .detach();
        zork_ui::components::region::invalidate_all(cx);
    }
    fn start_auth(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let Some((provider, billing)) = self.selection() else {
            return;
        };
        let provider = provider["id"].as_str().unwrap_or_default().to_owned();
        let billing = billing["id"].as_str().unwrap_or_default().to_owned();
        let id = self.id.read(cx).value().to_owned();
        let source = self.source.clone();
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
                        if state.authorization_complete {
                            view.form_open = false;
                        }
                    }
                    Err(e) => {
                        view.busy = false;
                        view.message = Some(e.to_string());
                    }
                }
                zork_ui::components::region::invalidate_all(cx);
            });
        })
        .detach();
        zork_ui::components::region::invalidate_all(cx);
    }
    fn poll_auth(&mut self, cx: &mut Context<Self>) {
        self.source
            .continue_authorization(self.callback.read(cx).value().into());
        self.watch_source(cx);
        zork_ui::components::region::invalidate_all(cx);
    }
    fn close_dialog(&mut self, cx: &mut Context<Self>) {
        if self.busy && self.attempt.is_none() {
            return;
        }
        if self.model_form_open {
            self.model_form_open = false;
            self.api_open = false;
        } else if self.detail.is_some() {
            self.detail = None;
            self.discovered = None;
        } else {
            self.cancel(cx);
            self.form_open = false;
            self.provider_open = false;
            self.key.update(cx, |input, cx| input.clear(cx));
        }
        self.message = None;
        zork_ui::components::region::invalidate_all(cx);
    }
    fn provider_dropdown(&self, window: &mut Window, cx: &mut Context<Self>) -> gpui::AnyElement {
        let choices: Vec<_> = crate::api::connection_options(&self.catalog, self.subscription)
            .into_iter()
            .map(|(index, billing)| {
                let provider = &self.catalog[index];
                (
                    index,
                    billing,
                    provider["label"].as_str().unwrap_or_default().to_owned(),
                    provider_path(provider["id"].as_str().unwrap_or_default()),
                )
            })
            .collect();
        let provider = self.catalog.get(self.provider);
        ui::dropdown_with_icons(
            "profile-provider-select",
            provider
                .and_then(|p| p["label"].as_str())
                .unwrap_or("选择供应商")
                .into(),
            choices
                .iter()
                .map(|(i, _, name, _)| {
                    (
                        format!("provider-option-{i}"),
                        name.clone(),
                        *i == self.provider,
                    )
                })
                .collect(),
            self.provider_open,
            !self.busy && self.attempt.is_none(),
            provider.map(|p| provider_path(p["id"].as_str().unwrap_or_default())),
            choices.iter().map(|(_, _, _, path)| Some(*path)).collect(),
            window,
            cx,
            |v, open, cx| {
                v.provider_open = open;
                zork_ui::components::region::invalidate_all(cx);
            },
            move |v, index, cx| {
                let (provider, billing, _, _) = &choices[index];
                v.provider = *provider;
                v.billing = *billing;
                v.provider_open = false;
                v.key.update(cx, |i, cx| i.clear(cx));
                v.base_url.update(cx, |i, cx| i.clear(cx));
                zork_ui::components::region::invalidate_all(cx);
            },
        )
    }
    fn input(
        &self,
        id: &'static str,
        label: &'static str,
        input: &Entity<ComposerInput>,
        cx: &gpui::App,
    ) -> gpui::Div {
        let error = self
            .model_attempted
            .then(|| self.model_errors(cx))
            .unwrap_or_default()
            .into_iter()
            .find(|(field, _)| *field == id)
            .map(|(_, error)| error);
        ui::field_with_error(id, label, input, error, cx)
    }
}
impl gpui::EventEmitter<ui::OpenAgent> for ProfilesView {}
impl Render for ProfilesView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = CUE_UI.palette;
        let placeholder = format!(
            "例如：my-{}",
            self.catalog
                .get(self.provider)
                .and_then(|p| p["id"].as_str())
                .unwrap_or("openai")
        );
        self.id
            .update(cx, |input, cx| input.set_placeholder(placeholder, cx));
        let show = self.detail.is_none() && (self.form_open || self.attempt.is_some());
        let modal_key = if self.model_form_open {
            Some("model-editor-dialog")
        } else if show {
            Some("profile-create-dialog")
        } else if self.detail.is_some() {
            Some("profile-detail-dialog")
        } else {
            None
        };
        self.modal.sync(modal_key, window, cx);
        let create_visible = self.modal.retain("profile-create-dialog", show.then_some(()), cx).is_some();
        let detail_visible = self.modal.retain("profile-detail-dialog", self.detail.clone().filter(|_| !self.model_form_open), cx);
        let model_visible = self.modal.retain("model-editor-dialog", self.detail.clone().filter(|_| self.model_form_open).map(|detail| (detail, self.editing_model.clone())), cx);
        let displayed_detail = detail_visible.clone().or_else(|| model_visible.as_ref().map(|(detail, _)| detail.clone()));
        let supports = self
            .selection()
            .is_some_and(|(_, b)| b["deviceCode"] == true);
        let custom = self
            .selection()
            .is_some_and(|(p, _)| p["id"] == "openai-compatible");
        let width = window.viewport_size().width.as_f32();
        let ids = self
            .profiles
            .iter()
            .map(|p| p.profile_id.clone())
            .collect::<Vec<_>>();
        let mut keep = ids
            .iter()
            .map(|id| format!("profile/{id}"))
            .collect::<HashSet<_>>();
        keep.extend([
            "header".into(),
            "dialog".into(),
            "empty".into(),
            "feedback".into(),
        ]);
        self.regions.retain(|key| keep.contains(key));
        let rows = ids
            .into_iter()
            .map(|id| {
                self.regions.element(
                    &format!("profile/{id}"),
                    gpui::StyleRefinement::default().w_full().h(px(80.)),
                    cx,
                    move |v, _, cx| {
                        v.profiles
                            .iter()
                            .find(|p| p.profile_id == id)
                            .map(|p| v.render_profile_row(p, cx))
                            .unwrap_or_else(|| gpui::Empty.into_any_element())
                    },
                )
            })
            .collect::<Vec<_>>();
        let header = self
            .regions
            .auto_height("header", width, cx, |v, _, cx| v.render_header(cx));
        div()
            .flex()
            .flex_col()
            .gap_0()
            .child(header)
            .when(!self.profiles.is_empty(), |v| {
                v.child(div().flex().flex_col().mx(px(-12.)).children(rows))
            })
            .when(
                self.profiles.is_empty() && !self.busy && !self.loading_profiles,
                |v| {
                    v.child(
                        div()
                            .py_8()
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap_3()
                            .child(provider_icon("openai", 32.))
                            .child(
                                div()
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .child("连接小伙伴使用的模型"),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(rgb(p.muted))
                                    .child("添加订阅账号或 API 连接，再为小伙伴选择模型。"),
                            ),
                    )
                },
            )
            .when(create_visible, |v| {
                v.child(ui::modal(
                    "profile-create-dialog",
                    "添加模型连接",
                    ui::section()
                        .border_t_0()
                        .py_0()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .text_size(px(11.))
                                .text_color(rgb(p.muted))
                                .child(ui::icon("icons/node.svg", 14.))
                                .child(format!("{} · 连接保存在此设备", self.device_name)),
                        )
                        .child(ui::form_field(
                            "接入方式",
                            zork_ui::components::liquid::controls::deferred_segmented(
                                "profile-access-active",
                                [("profile-access-true", "订阅账号"), ("profile-access-false", "API 接入")]
                                    .into_iter().map(|(id, label)| zork_ui::components::liquid::controls::Segment { id: id.into(), label: label.into(), disabled: false }).collect(),
                                vec![], Some(usize::from(!self.subscription)), zork_ui::components::liquid::controls::SegmentKind::Choice,
                                !self.busy && self.attempt.is_none(), p.canvas,
                                cx.listener(|v, index: &usize, _, cx| {
                                    if !v.busy && v.attempt.is_none() {
                                        v.select_access(*index == 0);
                                        v.key.update(cx, |i, cx| i.clear(cx));
                                        v.base_url.update(cx, |i, cx| i.clear(cx));
                                        zork_ui::components::region::invalidate_all(cx);
                                    }
                                }),
                            ),
                        ))
                        .child(ui::form_field("供应商", self.provider_dropdown(window, cx)))
                        .child(self.input("profile-id", "连接名称", &self.id, cx))
                        .when(custom, |v| {
                            v.child(self.input("profile-base-url", "接口地址", &self.base_url, cx))
                        })
                        .when(!supports, |v| {
                            v.child(self.input("profile-key", "API Key", &self.key, cx))
                        })
                        .when_some(self.attempt.clone(), |v, attempt| {
                            let url = attempt["verification_url"]
                                .as_str()
                                .unwrap_or_default()
                                .to_owned();
                            let code = attempt["user_code"].as_str().unwrap_or_default().to_owned();
                            v.child(
                                div()
                                    .p_4()
                                    .rounded(px(10.))
                                    .bg(rgb(p.selected))
                                    .flex()
                                    .flex_col()
                                    .gap_3()
                                    .child(div().text_size(px(12.)).text_color(rgb(p.muted)).child(
                                        if code.is_empty() {
                                            "在浏览器完成登录，然后粘贴返回的授权码。".into()
                                        } else {
                                            format!("在浏览器输入 {code}，完成后会自动保存。 ")
                                        },
                                    ))
                                    .child(
                                        div().flex().child(
                                            ui::button(
                                                "profile-open-browser",
                                                "打开登录页面",
                                                false,
                                                true,
                                            )
                                            .on_click(move |_, _, cx| cx.open_url(&url))
                                            .automation(AutomationRole::Button, "打开登录页面"),
                                        ),
                                    )
                                    .when(attempt["flow"] == "browser_callback", |v| {
                                        v.child(self.input(
                                            "profile-callback",
                                            "浏览器返回内容",
                                            &self.callback,
                                            cx,
                                        ))
                                    }),
                            )
                        }),
                    div()
                        .flex()
                        .justify_end()
                        .gap_2()
                        .when(self.attempt.is_some(), |v| {
                            v.child(
                                ui::button("profile-cancel", "取消登录", false, true)
                                    .on_click(cx.listener(|v, _, _, cx| v.cancel(cx)))
                                    .automation(AutomationRole::Button, "取消登录"),
                            )
                        })
                        .when(self.attempt.is_none(), |v| {
                            v.child(
                                ui::button("profile-close-form", "取消", false, !self.busy)
                                    .on_click(cx.listener(|v, _, _, cx| {
                                        if !v.busy {
                                            v.form_open = false;
                                            v.key.update(cx, |key, cx| key.clear(cx));
                                            zork_ui::components::region::invalidate_all(cx);
                                        }
                                    }))
                                    .automation_enabled(
                                        !self.busy,
                                        AutomationRole::Button,
                                        "取消添加",
                                    ),
                            )
                        })
                        .when(!supports, |v| {
                            v.child(
                                ui::busy_button(
                                    "profile-save",
                                    if self.busy {
                                        "正在保存…"
                                    } else {
                                        "保存连接"
                                    },
                                    true,
                                    !self.busy,
                                    self.busy,
                                )
                                .on_click(cx.listener(|v, _, _, cx| v.save_key(cx)))
                                .automation_enabled(
                                    !self.busy,
                                    AutomationRole::Button,
                                    "保存 Profile",
                                ),
                            )
                        })
                        .when(supports && self.attempt.is_none(), |v| {
                            v.child(
                                ui::busy_button(
                                    "profile-signin",
                                    if self.busy {
                                        "正在连接…"
                                    } else {
                                        "登录并连接"
                                    },
                                    true,
                                    !self.busy,
                                    self.busy,
                                )
                                .on_click(cx.listener(|v, _, _, cx| v.start_auth(cx)))
                                .automation_enabled(
                                    !self.busy,
                                    AutomationRole::Button,
                                    "登录并创建 Profile",
                                ),
                            )
                        })
                        .when(
                            self.attempt
                                .as_ref()
                                .is_some_and(|a| a["flow"] == "browser_callback"),
                            |v| {
                                v.child(
                                    ui::button("profile-complete", "完成登录", true, true)
                                        .on_click(cx.listener(|v, _, _, cx| v.poll_auth(cx)))
                                        .automation(AutomationRole::Button, "完成登录"),
                                )
                            },
                        ),
                    self.message.clone(),
                    &self.modal,
                    window,
                    cx,
                    !self.busy || self.attempt.is_some(),
                    |v, _, cx| {
                        v.close_dialog(cx);
                    },
                ))
            })
            .when_some(displayed_detail, |v, detail| {
                let profile_id = detail["profile_id"].as_str().unwrap_or_default().to_owned();
                let title = detail["name"]
                    .as_str()
                    .filter(|name| !name.trim().is_empty())
                    .unwrap_or(&profile_id)
                    .to_owned();
                let provider_name = self
                    .catalog
                    .iter()
                    .find(|provider| provider["id"] == detail["provider"])
                    .and_then(|p| p["label"].as_str())
                    .unwrap_or("OpenAI");
                let refresh_id = profile_id.clone();
                let refreshing = self.refreshing_profiles.contains(&profile_id);
                let verified = detail["account"]["verified"]
                    .as_bool()
                    .or_else(|| detail["account"]["ok"].as_bool())
                    .unwrap_or_else(|| detail["verified"].as_bool().unwrap_or(false));
                let model_count = detail["models"].as_array().map_or(0, Vec::len);
                let content = div()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .mb_3()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .text_size(px(11.))
                                    .text_color(rgb(p.muted))
                                    .child(provider_icon(
                                        detail["provider"].as_str().unwrap_or_default(),
                                        18.,
                                    ))
                                    .child(div().truncate().child(format!(
                                        "{} · {}",
                                        provider_name,
                                        self.locale.text(if verified {
                                            "profile_verified"
                                        } else {
                                            "profile_unverified"
                                        })
                                    ))),
                            )
                            .when_some(
                                self.quota.get(&profile_id).and_then(|q| q.checked.clone()),
                                |v, checked| {
                                    v.child(
                                        div()
                                            .id("profile-quota-updated")
                                            .flex_shrink_0()
                                            .text_size(px(11.))
                                            .text_color(rgb(p.muted))
                                            .child(checked.clone())
                                            .automation(AutomationRole::Status, checked),
                                    )
                                },
                            )
                            .child(
                                ui::icon_button("profile-quota-refresh", !refreshing)
                                    .flex_shrink_0()
                                    .when(refreshing, |v| {
                                        v.child(zork_ui::components::loading::indicator(
                                            "quota-loading",
                                            14.,
                                        ))
                                    })
                                    .when(!refreshing, |v| {
                                        v.child(ui::icon("icons/reload.svg", 14.))
                                    })
                                    .on_click(cx.listener(move |v, _, _, cx| {
                                        v.refresh_quota(refresh_id.clone(), cx)
                                    }))
                                    .automation(
                                        AutomationRole::Button,
                                        self.locale.text("quota_refresh"),
                                    ),
                            ),
                    )
                    .child(self.quota_detail(&profile_id))
                    .child(
                        div()
                            .h(px(ui::BUTTON_HEIGHT))
                            .mb_3()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_3()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_size(px(13.))
                                            .font_weight(gpui::FontWeight::MEDIUM)
                                            .child("模型"),
                                    )
                                    .when_some(
                                        self.discovered
                                            .as_ref()
                                            .and_then(|value| value["message"].as_str())
                                            .map(str::to_owned),
                                        |v, message| {
                                            v.child(
                                                div()
                                                    .id("profile-model-update-result")
                                                    .text_size(px(11.))
                                                    .text_color(rgb(p.muted))
                                                    .child(message.clone())
                                                    .automation(AutomationRole::Status, message),
                                            )
                                        },
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .gap_2()
                                    .child(
                                        ui::busy_button(
                                            "profile-model-discover",
                                            self.locale.text("profile_update_models"),
                                            false,
                                            !self.busy,
                                            self.discovering,
                                        )
                                        .text_size(px(11.))
                                        .px(px(ui::BUTTON_PADDING_X))
                                        .gap_1()
                                        .on_click(cx.listener(|v, _, _, cx| v.discover_models(cx)))
                                        .automation(
                                            AutomationRole::Button,
                                            self.locale.text("profile_update_models"),
                                        ),
                                    )
                                    .child(
                                        ui::button("profile-model-add", "", false, !self.busy)
                                            .text_size(px(11.))
                                            .px(px(ui::BUTTON_PADDING_X))
                                            .gap_1()
                                            .child(ui::icon("icons/plus.svg", 13.))
                                            .child("手动添加")
                                            .on_click(cx.listener(|v, _, _, cx| {
                                                if !v.busy {
                                                    v.edit_model(None, cx);
                                                }
                                            }))
                                            .map(|button| self.modal.source("model-editor-dialog").bind(button, "手动添加", ui::ActionStyle { icon: Some("icons/plus.svg"), disabled: self.busy, ..Default::default() }))
                                            .automation(AutomationRole::Button, "手动添加模型"),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .id("profile-models")
                            .h(px(64. * model_count.min(4) as f32))
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
                                            .map(|model| view.render_model_row(model, cx))
                                            .collect::<Vec<_>>()
                                    }),
                                )
                                .size_full(),
                            )
                            .automation(AutomationRole::ScrollArea, "模型列表"),
                    );
                v.when(detail_visible.is_some(), |v| {
                    v.child(ui::detail_modal_with_title_action(
                        "profile-detail-dialog",
                        title,
                        self.renaming.then(|| self.rename_editor(cx)),
                        ui::icon_button(
                            if self.renaming {
                                "profile-name-save"
                            } else {
                                "profile-rename"
                            },
                            !self.busy,
                        )
                        .flex_shrink_0()
                        .when(self.busy, |v| {
                            v.child(zork_ui::components::loading::indicator("name-saving", 14.))
                        })
                        .when(!self.busy, |v| {
                            v.child(ui::icon(
                                if self.renaming {
                                    "icons/check.svg"
                                } else {
                                    "icons/edit.svg"
                                },
                                14.,
                            ))
                        })
                        .on_click(cx.listener(|v, _, window, cx| {
                            if v.renaming {
                                v.save_name(cx)
                            } else {
                                v.start_rename(window, cx)
                            }
                        }))
                        .automation_enabled(
                            !self.busy,
                            AutomationRole::Button,
                            self.locale.text(if self.renaming {
                                "profile_rename_save"
                            } else {
                                "profile_rename"
                            }),
                        ),
                        content,
                        self.message.clone(),
                        &self.modal,
                        window,
                        cx,
                        !self.busy,
                        |v, _, cx| v.close_dialog(cx),
                    ))
                })
                .when(model_visible.is_some(), |v| {
                    v.child(ui::modal(
                        "model-editor-dialog",
                        if model_visible.as_ref().is_some_and(|(_, editing)| editing.is_some()) {
                            "编辑模型"
                        } else {
                            "添加模型"
                        },
                        ui::section()
                            .border_t_0()
                            .py_0()
                            .child(self.input("profile-model", "模型 ID", &self.model, cx))
                            .child(ui::form_field(
                                "接口协议",
                                ui::dropdown_with_icons(
                                    "model-api-select",
                                    MODEL_APIS[self.model_api].1.into(),
                                    MODEL_APIS
                                        .iter()
                                        .enumerate()
                                        .map(|(i, (_, name))| {
                                            (
                                                format!("model-api-{i}"),
                                                (*name).into(),
                                                i == self.model_api,
                                            )
                                        })
                                        .collect(),
                                    self.api_open,
                                    !self.busy,
                                    Some(provider_path(
                                        if MODEL_APIS[self.model_api].0.starts_with("anthropic") {
                                            "anthropic"
                                        } else {
                                            "openai"
                                        },
                                    )),
                                    MODEL_APIS
                                        .iter()
                                        .map(|(api, _)| {
                                            Some(provider_path(if api.starts_with("anthropic") {
                                                "anthropic"
                                            } else {
                                                "openai"
                                            }))
                                        })
                                        .collect(),
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
                                        zork_ui::components::region::invalidate_all(cx);
                                    },
                                ),
                            ))
                            .child(
                                div()
                                    .flex()
                                    .gap_3()
                                    .child(self.input(
                                        "profile-context-limit",
                                        "上下文 token 上限",
                                        &self.context_limit,
                                        cx,
                                    ))
                                    .child(self.input(
                                        "profile-output-limit",
                                        "输出 token 上限",
                                        &self.output_limit,
                                        cx,
                                    )),
                            )
                            .child(
                                div()
                                    .flex()
                                    .gap_3()
                                    .child(self.input(
                                        "profile-thinking-levels",
                                        "推理级别，用逗号分隔",
                                        &self.thinking_levels,
                                        cx,
                                    ))
                                    .child(self.input(
                                        "profile-default-thinking",
                                        "默认推理级别",
                                        &self.default_thinking,
                                        cx,
                                    )),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(rgb(p.muted))
                                    .child("填写该模型实际支持的容量和推理级别。"),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap_3()
                                    .py_2()
                                    .child(
                                        div()
                                            .text_size(px(13.))
                                            .child(self.locale.text("model_image_input")),
                                    )
                                    .child(ui::switch(
                                        "profile-model-image-input",
                                        self.locale.text("model_image_input"),
                                        self.model_image_input,
                                        !self.busy,
                                        &self.model_image_focus,
                                        cx,
                                        |v, on, cx| {
                                            v.model_image_input = on;
                                            zork_ui::components::region::invalidate(
                                                cx,
                                                &["dialog"],
                                            );
                                        },
                                    )),
                            ),
                        div()
                            .w_full()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .child(
                                div()
                                    .w(px(180.))
                                    .flex_shrink_0()
                                    .child(self.copy_model_action(window, cx)),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        ui::button(
                                            "profile-model-cancel",
                                            "取消",
                                            false,
                                            !self.busy,
                                        )
                                        .on_click(cx.listener(|v, _, _, cx| {
                                            if !v.busy {
                                                v.model_form_open = false;
                                                zork_ui::components::region::invalidate_all(cx);
                                            }
                                        }))
                                        .automation_enabled(
                                            !self.busy,
                                            AutomationRole::Button,
                                            "取消编辑模型",
                                        ),
                                    )
                                    .child(
                                        ui::busy_button(
                                            "profile-model-save",
                                            "保存模型",
                                            true,
                                            !self.busy,
                                            self.busy,
                                        )
                                        .on_click(
                                            cx.listener(|v, _, window, cx| {
                                                v.save_model(window, cx)
                                            }),
                                        )
                                        .automation_enabled(
                                            !self.busy,
                                            AutomationRole::Button,
                                            "保存模型",
                                        ),
                                    ),
                            ),
                        self.message.clone(),
                        &self.modal,
                        window,
                        cx,
                        !self.busy,
                        |v, _, cx| {
                            v.close_dialog(cx);
                        },
                    ))
                })
            })
            .when_some(
                self.message.clone().filter(|_| modal_key.is_none()),
                |v, m| v.child(ui::status_notice(m, ui::NoticeKind::Error)),
            )
    }
}

impl ProfilesView {
    fn render_header(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let p = CUE_UI.palette;
        div()
            .w_full()
            .flex()
            .items_start()
            .justify_between()
            .gap_4()
            .items_center()
            .pb(px(18.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(ui::page_title("大模型"))
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(p.muted))
                            .child(format!(
                                "{} · {} 个连接",
                                self.device_name,
                                self.profiles.len()
                            )),
                    ),
            )
            .when(true, |header| {
                header.child(
                    ui::page_action("profile-add", "添加连接")
                        .on_click(cx.listener(|v, _, _, cx| {
                            v.form_open = true;
                            v.id.update(cx, |i, cx| i.clear(cx));
                            v.key.update(cx, |i, cx| i.clear(cx));
                            v.base_url.update(cx, |i, cx| i.clear(cx));
                            v.message = None;
                            zork_ui::components::region::invalidate_all(cx);
                        }))
                        .map(|button| self.modal.source("profile-create-dialog").bind(button, "添加连接", ui::ActionStyle { icon: Some("icons/plus.svg"), ..Default::default() }))
                        .automation(AutomationRole::Button, "添加连接"),
                )
            })
            .into_any_element()
    }
    fn render_profile_row(
        &self,
        profile: &ProfileInfo,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let p = CUE_UI.palette;

        let id = profile.profile_id.clone();
        let provider = self.catalog.iter().find(|p| p["id"] == profile.provider);
        let provider_name = provider
            .and_then(|p| p["label"].as_str())
            .unwrap_or(&profile.provider);
        let billing = provider
            .and_then(|p| p["billing"].as_array())
            .and_then(|items| {
                items
                    .iter()
                    .find(|b| b["id"].as_str() == profile.billing.as_deref())
            })
            .and_then(|b| b["label"].as_str())
            .unwrap_or("");

        ui::quiet_button(format!("profile-detail-{id}"), "", true, ui::IconButtonSize::Standard)
            .radius(ui::FIELD_RADIUS)
            .font_weight(gpui::FontWeight::NORMAL)
            .justify_start()
            .w_full()
            .on_click(cx.listener(move |v, _, _, cx| v.open_detail(id.clone(), cx)))
            .h(px(80.))
            .px(px(12.))
            .rounded(px(ui::FIELD_RADIUS))
            .flex()
            .items_center()
            .gap_3()
            .child(
                div()
                    .id(format!("profile-avatar-{}", profile.profile_id))
                    .size(px(40.))
                    .flex_shrink_0()
                    .rounded(px(ui::FIELD_RADIUS))
                    .bg(rgb(p.sidebar))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(provider_icon(&profile.provider, 24.))
                    .automation(AutomationRole::Status, profile.provider.clone()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_0()
                    .child(
                        div()
                            .truncate()
                            .text_size(px(13.))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .child(profile.display_name().to_owned()),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_size(px(11.))
                            .line_height(px(16.))
                            .text_color(rgb(p.muted))
                            .child(format!(
                                "{}{}{}",
                                provider_name,
                                if billing.is_empty() { "" } else { " · " },
                                billing
                            )),
                    )
                    .when_some(
                        self.quota.get(&profile.profile_id).filter(|q| q.visible()),
                        |v, quota| v.child(self.render_quota_summary(&profile.profile_id, quota)),
                    ),
            )
            .child(div().text_size(px(11.)).text_color(rgb(p.muted)).child(
                if profile.models.is_empty() {
                    "待配置模型".into()
                } else {
                    format!("{} 个模型", profile.models.len())
                },
            ))
            .child(
                div()
                    .px_2()
                    .py_1()
                    .rounded_full()
                    .text_size(px(11.))
                    .bg(gpui::rgba(
                        ((if profile.is_verified() {
                            p.success
                        } else {
                            p.warning
                        }) << 8)
                            | 0x12,
                    ))
                    .text_color(rgb(if profile.is_verified() {
                        p.success
                    } else {
                        p.warning
                    }))
                    .child(self.locale.text(if profile.is_verified() {
                        "profile_verified"
                    } else {
                        "profile_unverified"
                    })),
            )
            .map(|row| self.modal.source("profile-detail-dialog").bind(row, profile.display_name().to_owned(), ui::ActionStyle { image: Some(ui::provider_path(&profile.provider)), ..Default::default() }))
            .automation(AutomationRole::Button, profile.display_name().to_owned())
            .into_any_element()
    }

    fn render_quota_summary(&self, id: &str, quota: &QuotaPresentation) -> gpui::AnyElement {
        let p = CUE_UI.palette;
        div()
            .id(format!("profile-quota-summary-{id}"))
            .flex()
            .items_center()
            .gap(px(16.))
            .min_w_0()
            .overflow_hidden()
            .mt_1()
            .text_size(px(11.))
            .line_height(px(16.))
            .when(quota.failed, |v| {
                v.text_color(rgb(p.warning)).child(quota.summary.clone())
            })
            .when(!quota.failed, |v| {
                v.children(quota.windows.iter().take(2).map(|window| {
                    let color = if window.remaining == 0. {
                        p.danger
                    } else if window.remaining < 20. {
                        p.warning
                    } else {
                        p.success
                    };
                    div()
                        .flex()
                        .flex_shrink_0()
                        .items_center()
                        .gap_2()
                        .child(div().text_color(rgb(p.muted)).child(window.label.clone()))
                        .child(
                            div()
                                .w(px(32.))
                                .h(px(4.))
                                .rounded_full()
                                .bg(gpui::rgba((p.muted << 8) | 0x30))
                                .child(
                                    div()
                                        .h_full()
                                        .w(gpui::relative(window.remaining / 100.))
                                        .rounded_full()
                                        .bg(rgb(color)),
                                ),
                        )
                        .child(
                            div()
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .text_color(rgb(color))
                                .child(window.value.clone()),
                        )
                }))
                .when_some(quota.balance.clone(), |v, balance| {
                    v.child(
                        div()
                            .flex_shrink_0()
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(rgb(p.text))
                            .child(balance),
                    )
                })
            })
            .automation(AutomationRole::Status, quota.summary.clone())
            .into_any_element()
    }
}
