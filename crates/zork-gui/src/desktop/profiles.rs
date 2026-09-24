use super::{profile_quota::QuotaPresentation, ui};
#[cfg(feature = "headless-bench")]
use crate::api::StationClient;
use crate::api::{compact_tokens, ConnectionInput, ModelInput, MODEL_APIS};
use crate::i18n::Locale;
use crate::{
    api::ProfileInfo,
    automation::{AutomationElementExt, AutomationRole},
    components::text_input::ComposerInput,
    design::ZORK_UI,
};
use gpui::{div, prelude::*, px, rgb, Context, Entity, Task, Window};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use zork_ui::controls::{provider_icon, provider_path};

pub struct ProfilesView {
    source: Arc<crate::api::Profiles>,
    dialog_only: bool,
    row_scope: Option<String>,
    detail_request: u64,
    source_updates: Option<Task<()>>,
    regions: zork_ui::components::region::Regions<Self>,
    device_name: String,
    device_status: zork_ui::device_name::DeviceStatus,
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
    model_params_open: bool,
    params_touched: bool,
    thinking_scheme: crate::api::thinking::ThinkingScheme,
    recognized: Option<(String, String)>,
    reference_open: Option<crate::api::model_catalog::Field>,
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
    pub fn set_device_status(
        &mut self,
        status: zork_ui::device_name::DeviceStatus,
        cx: &mut Context<Self>,
    ) {
        if self.device_status != status {
            self.device_status = status;
            zork_ui::components::region::invalidate_all(cx);
        }
    }
    pub fn set_device_name(&mut self, name: String) {
        self.device_name = name;
    }
    pub fn editor(
        source: Arc<crate::api::Profiles>,
        device_id: String,
        name: String,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut view = Self::new_source(source, cx);
        view.dialog_only = true;
        view.row_scope = Some(device_id);
        view.device_name = name;
        view.set_visible(true, cx);
        view
    }
    pub fn add_connection(&mut self, cx: &mut Context<Self>) {
        self.detail_request += 1;
        self.model_form_open = false;
        self.detail = None;
        self.form_open = true;
        self.id.update(cx, |v, cx| v.clear(cx));
        self.key.update(cx, |v, cx| v.clear(cx));
        self.base_url.update(cx, |v, cx| v.clear(cx));
        self.message = None;
        zork_ui::components::region::invalidate_all(cx);
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
        cx.subscribe(
            &model,
            |view, _, _: &crate::components::text_input::ComposerEdited, cx| view.recognize(cx),
        )
        .detach();
        for limit in [&context_limit, &output_limit] {
            cx.subscribe(
                limit,
                |view, _, _: &crate::components::text_input::ComposerEdited, _| {
                    view.params_touched = true
                },
            )
            .detach();
        }
        context_limit.update(cx, |v, cx| v.set_value("32K", cx));
        output_limit.update(cx, |v, cx| v.set_value("4.096K", cx));
        key.update(cx, |input, cx| input.set_secret(true, cx));
        callback.update(cx, |input, cx| input.set_secret(true, cx));
        Self {
            modal: ui::ModalState::new(cx),
            source,
            dialog_only: false,
            row_scope: None,
            detail_request: 0,
            source_updates: None,
            regions: Default::default(),
            device_name: String::new(),
            device_status: Default::default(),
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
            model_params_open: false,
            params_touched: false,
            thinking_scheme: crate::api::thinking::ThinkingScheme::Unsupported,
            recognized: None,
            reference_open: None,
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
        Self::fixture(detail, cx)
    }

    #[cfg(feature = "headless-bench")]
    fn fixture(detail: bool, cx: &mut Context<Self>) -> Self {
        let fixture = zork_ui::stories::page_fixture();
        let client = Arc::new(StationClient::fixture(
            fixture.clone(),
            serde_json::from_str(include_str!("../../tests/fixtures/provider_catalog.json"))
                .expect("provider fixture"),
        ));
        let mut view = Self::new_source(crate::api::Profiles::new(client), cx);
        view.seed_fixture_catalog();
        view.device_name = fixture["device"]["name"].as_str().unwrap().into();
        view.device_status = zork_ui::device_name::DeviceStatus::Direct;
        view.profiles = vec![serde_json::from_value(fixture["profile"].clone()).unwrap()];
        view.helpers = fixture["agents"].as_array().unwrap().clone();
        if detail {
            view.detail = Some(fixture["profile"].clone());
        }
        view.accept_profiles(view.profiles.clone());
        view
    }
    #[cfg(feature = "headless-bench")]
    pub(super) fn seed_fixture_catalog(&mut self) {
        self.catalog = serde_json::from_str::<Value>(include_str!(
            "../../tests/fixtures/provider_catalog.json"
        ))
        .expect("provider catalog fixture")["providers"]
            .as_array()
            .unwrap()
            .clone();
        self.select_access(true);
        self.provider = self
            .catalog
            .iter()
            .position(|p| p["id"] == "openai")
            .unwrap();
        self.billing = self.catalog[self.provider]["billing"]
            .as_array()
            .unwrap()
            .iter()
            .position(|b| b["id"] == "subscription")
            .unwrap();
    }
    #[cfg(feature = "headless-bench")]
    pub(super) fn freeze_onboarding_catalog(&mut self) {
        // The design story has no remote catalog refresh; a queued empty
        // offline snapshot must not replace its fixed provider choices.
        self.source_updates = None;
        self.seed_fixture_catalog();
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
        let p = ZORK_UI.palette;
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
                        .text_size(px(12.))
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
                                .text_size(px(12.))
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
        self.open_model(id, None, cx);
    }
    pub fn open_model(&mut self, id: String, model: Option<String>, cx: &mut Context<Self>) {
        self.detail_request += 1;
        let request = self.detail_request;
        self.watch_source(cx);
        let source = self.source.clone();
        cx.spawn(async move |this, cx| {
            let result = source.open_detail(&id).await;
            let _ = this.update(cx, |v, cx| {
                if v.detail_request != request {
                    return;
                }
                match result {
                    Ok(()) => {
                        v.detail = Some(source.detail(json!({"profile_id":id})));
                        v.renaming = false;
                        v.form_open = false;
                        v.model_form_open = false;
                        v.discovered = None;
                        v.model.update(cx, |m, cx| m.clear(cx));
                        if let Some(model) = &model {
                            if let Some(value) = v
                                .detail
                                .as_ref()
                                .and_then(|detail| detail["models"].as_array())
                                .and_then(|models| {
                                    models.iter().find(|value| value["id"] == *model)
                                })
                                .cloned()
                            {
                                v.edit_model(Some(value), cx);
                            }
                        }
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
        self.apply_model_input(
            crate::api::model_form_with_catalog(detail, &self.catalog, model),
            cx,
        );
        self.copy_model_open = false;
        self.api_open = false;
        self.model_params_open = false;
        self.reference_open = None;
        self.params_touched = self.editing_model.is_some();
        self.model_form_open = true;
        self.model_attempted = false;
        self.message = None;
        zork_ui::components::region::invalidate_all(cx);
    }
    fn apply_model_input(&mut self, input: ModelInput, cx: &mut Context<Self>) {
        self.thinking_scheme = input.scheme();
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
        self.recognized = self.recognition_line(&input.id);
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
        let p = ZORK_UI.palette;
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
                            .text_size(px(12.))
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
                    .text_size(px(12.))
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
            thinking: self.thinking_scheme.encode().0.join(", "),
            default_thinking: self.thinking_scheme.encode().1,
            images: self.model_image_input,
            thinking_scheme: Some(self.thinking_scheme.clone()),
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
            self.model_params_open = field != "profile-model";
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
impl Render for ProfilesView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = ZORK_UI.palette;
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
        let create_visible = self
            .modal
            .retain("profile-create-dialog", show.then_some(()), cx)
            .is_some();
        let detail_visible = self.modal.retain(
            "profile-detail-dialog",
            self.detail.clone().filter(|_| !self.model_form_open),
            cx,
        );
        let model_visible = self.modal.retain(
            "model-editor-dialog",
            self.detail
                .clone()
                .filter(|_| self.model_form_open)
                .map(|detail| (detail, self.editing_model.clone())),
            cx,
        );
        let displayed_detail = detail_visible
            .clone()
            .or_else(|| model_visible.as_ref().map(|(detail, _)| detail.clone()));
        let editor_body = model_visible
            .is_some()
            .then(|| self.model_editor_body(window, cx));
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
        let rows = if self.dialog_only {
            vec![]
        } else {
            // Rows keep their intrinsic height: quota rings may wrap in narrow windows.
            let row_width = window.viewport_size().width.as_f32();
            ids.into_iter()
                .map(|id| {
                    self.regions.auto_height(
                        &format!("profile/{id}"),
                        row_width,
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
                .collect::<Vec<_>>()
        };
        let header = self
            .regions
            .auto_height("header", width, cx, |v, _, cx| v.render_header(cx));
        div()
            .flex()
            .flex_col()
            .gap_0()
            .when(!self.dialog_only, |v| v.child(header))
            .when(!self.dialog_only && !self.profiles.is_empty(), |v| {
                v.child(div().flex().flex_col().mx(px(-12.)).children(rows))
            })
            .when(
                !self.dialog_only
                    && self.profiles.is_empty()
                    && !self.busy
                    && !self.loading_profiles,
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
                                    .child("连接对话使用的模型"),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(rgb(p.muted))
                                    .child("添加订阅账号或 API 连接，供新对话选择模型。"),
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
                                .text_size(px(12.))
                                .text_color(rgb(p.muted))
                                .child(zork_ui::device_name::label(
                                    "profile-create-device",
                                    self.device_name.clone(),
                                    &self.device_status,
                                    None,
                                ))
                                .child("· 连接保存在此设备"),
                        )
                        .child(ui::form_field(
                            "接入方式",
                            zork_ui::components::widgets::controls::deferred_segmented(
                                "profile-access-active",
                                [
                                    ("profile-access-true", "订阅账号"),
                                    ("profile-access-false", "API 接入"),
                                ]
                                .into_iter()
                                .map(|(id, label)| {
                                    zork_ui::components::widgets::controls::Segment {
                                        id: id.into(),
                                        label: label.into(),
                                        disabled: false,
                                    }
                                })
                                .collect(),
                                vec![],
                                Some(usize::from(!self.subscription)),
                                zork_ui::components::widgets::controls::SegmentKind::Choice,
                                !self.busy && self.attempt.is_none(),
                                p.canvas,
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
                                    .rounded(px(zork_ui::design::RADIUS.control))
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
                                    .text_size(px(12.))
                                    .text_color(rgb(p.muted))
                                    .child(provider_icon(
                                        detail["provider"].as_str().unwrap_or_default(),
                                        18.,
                                    ))
                                    .child(div().truncate().child(if self.device_name.is_empty() {
                                        provider_name.to_owned()
                                    } else {
                                        format!("{provider_name} · 保存在 {}", self.device_name)
                                    }))
                                    // Status only matters when it needs attention.
                                    .when(!verified, |v| {
                                        v.child(
                                            div()
                                                .flex_shrink_0()
                                                .h(px(22.))
                                                .px(px(9.))
                                                .flex()
                                                .items_center()
                                                .rounded_full()
                                                .font_weight(gpui::FontWeight::MEDIUM)
                                                .bg(gpui::rgba((p.warning << 8) | 0x1f))
                                                .text_color(rgb(p.warning))
                                                .child(self.locale.text("profile_unverified")),
                                        )
                                    }),
                            )
                            .when_some(
                                self.quota.get(&profile_id).and_then(|q| q.checked.clone()),
                                |v, checked| {
                                    v.child(
                                        div()
                                            .id("profile-quota-updated")
                                            .flex_shrink_0()
                                            .text_size(px(12.))
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
                                                    .text_size(px(12.))
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
                                        .text_size(px(12.))
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
                                            .text_size(px(12.))
                                            .px(px(ui::BUTTON_PADDING_X))
                                            .gap_1()
                                            .child(ui::icon("icons/plus.svg", 13.))
                                            .child("手动添加")
                                            .on_click(cx.listener(|v, _, _, cx| {
                                                if !v.busy {
                                                    v.edit_model(None, cx);
                                                }
                                            }))
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
                        if model_visible
                            .as_ref()
                            .is_some_and(|(_, editing)| editing.is_some())
                        {
                            "编辑模型"
                        } else {
                            "添加模型"
                        },
                        ui::section()
                            .border_t_0()
                            .py_0()
                            .when(self.dialog_only, |v| {
                                v.child(ui::label(format!(
                                    "{} · {}",
                                    zork_ui::device_name::summary(
                                        &self.device_name,
                                        &self.device_status,
                                        None,
                                    ),
                                    detail["name"]
                                        .as_str()
                                        .or_else(|| detail["profile_id"].as_str())
                                        .unwrap_or("")
                                )))
                            })
                            .children(editor_body),
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

/// Model editor: recognition fills everything; parameters stay collapsed until
/// the user asks, and every parameter can borrow a popular model's value.
impl ProfilesView {
    fn provider_id(&self) -> Option<String> {
        self.detail
            .as_ref()
            .and_then(|d| d["provider"].as_str())
            .map(str::to_owned)
    }
    fn recognition_line(&self, id: &str) -> Option<(String, String)> {
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
    fn recognize(&mut self, cx: &mut Context<Self>) {
        let id = self.model.read(cx).value().to_owned();
        self.recognized = self.recognition_line(&id);
        if !self.params_touched && self.editing_model.is_none() {
            // A new model takes every parameter from the recognized entry
            // until the user changes one of them.
            let provider = self.provider_id();
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
        zork_ui::components::region::invalidate(cx, &["dialog"]);
    }
    fn apply_reference(
        &mut self,
        field: crate::api::model_catalog::Field,
        value: Value,
        cx: &mut Context<Self>,
    ) {
        use crate::api::model_catalog::Field;
        let tokens = |v: &Value| v.as_u64().map(zork_client_core::model_edit::compact_tokens);
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
        zork_ui::components::region::invalidate(cx, &["dialog"]);
    }
    fn set_thinking(
        &mut self,
        scheme: crate::api::thinking::ThinkingScheme,
        cx: &mut Context<Self>,
    ) {
        self.thinking_scheme = scheme;
        self.params_touched = true;
        zork_ui::components::region::invalidate(cx, &["dialog"]);
    }
    fn chip(
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
            .h(px(28.))
            .px(px(12.))
            .flex()
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
    fn param_row(&self, label: &'static str, control: gpui::AnyElement) -> gpui::Div {
        div()
            .min_h(px(36.))
            .flex()
            .items_center()
            .gap(px(8.))
            .child(
                div()
                    .w(px(76.))
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
    }
    fn reference_toggle(
        &self,
        field: crate::api::model_catalog::Field,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
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
            zork_ui::components::region::invalidate(cx, &["dialog"]);
        }))
        .automation(AutomationRole::Button, "参照常见模型")
        .into_any_element()
    }
    fn reference_list(
        &self,
        field: crate::api::model_catalog::Field,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        if self.reference_open != Some(field) {
            return None;
        }
        let id = self.model.read(cx).value().to_owned();
        let refs = crate::api::model_catalog::references(field, Some(&id));
        let chips: Vec<_> = refs
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
                .pl(px(84.))
                .pb(px(6.))
                .flex()
                .flex_wrap()
                .gap(px(6.))
                .children(chips)
                .into_any_element(),
        )
    }
    fn thinking_controls(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        use crate::api::thinking::{BudgetChoice, ThinkingScheme as T};
        let p = ZORK_UI.palette;
        let kind = match &self.thinking_scheme {
            T::Unsupported => 0,
            T::Always { .. } => 1,
            T::Toggle { .. } => 2,
            T::Levels { .. } => 3,
            T::Budget { .. } => 4,
        };
        let kinds = ["不支持", "固定开启", "开关", "档位", "预算"];
        let kind_chips: Vec<_> = kinds
            .iter()
            .enumerate()
            .map(|(i, name)| {
                self.chip(
                    gpui::SharedString::from(format!("model-thinking-kind-{i}")),
                    (*name).to_owned(),
                    i == kind,
                    cx,
                    move |v, cx| {
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
            })
            .collect();
        let mut col = div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(div().flex().flex_wrap().gap(px(6.)).children(kind_chips));
        const ORDER: [&str; 6] = ["minimal", "low", "medium", "high", "xhigh", "max"];
        match self.thinking_scheme.clone() {
            T::Levels { values, default } => {
                let mut known: Vec<String> = ORDER.iter().map(|v| (*v).to_owned()).collect();
                for value in &values {
                    if !known.contains(value) {
                        known.push(value.clone());
                    }
                }
                let level_chips: Vec<_> = known
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
                let options = values.iter().map(|v| (v.clone(), v.clone())).collect();
                col = col
                    .child(div().flex().flex_wrap().gap(px(6.)).children(level_chips))
                    .child(self.default_choice(options, default, cx, move |value| T::Levels {
                        values: values.clone(),
                        default: value,
                    }));
            }
            T::Budget {
                presets,
                default,
                dynamic,
                allow_off,
            } => {
                let mut options: Vec<(String, String)> = Vec::new();
                if allow_off {
                    options.push(("off".into(), "关".into()));
                }
                if dynamic {
                    options.push(("dynamic".into(), "动态".into()));
                }
                options.extend(presets.iter().map(|t| {
                    (
                        crate::api::thinking::budget_value(*t),
                        zork_client_core::model_edit::compact_tokens(u64::from(*t)),
                    )
                }));
                let current = match default {
                    BudgetChoice::Off => "off".into(),
                    BudgetChoice::Dynamic => "dynamic".into(),
                    BudgetChoice::Tokens(t) => crate::api::thinking::budget_value(t),
                };
                col = col.child(self.default_choice(options, current, cx, move |value| {
                    let default = match value.as_str() {
                        "off" => BudgetChoice::Off,
                        "dynamic" => BudgetChoice::Dynamic,
                        other => BudgetChoice::Tokens(
                            crate::api::thinking::budget_tokens(other).unwrap_or(16384),
                        ),
                    };
                    T::Budget {
                        presets: presets.clone(),
                        default,
                        dynamic,
                        allow_off,
                    }
                }));
            }
            T::Toggle { on, default_on } => {
                col = col.child(self.chip(
                    "model-thinking-default-on",
                    if default_on { "默认开启" } else { "默认关闭" }.to_owned(),
                    default_on,
                    cx,
                    move |v, cx| {
                        v.set_thinking(
                            T::Toggle {
                                on: on.clone(),
                                default_on: !default_on,
                            },
                            cx,
                        )
                    },
                ));
            }
            T::Always { .. } => {
                col = col.child(
                    div()
                        .text_size(px(12.))
                        .text_color(rgb(p.subtle))
                        .child("这个模型总是会思考"),
                );
            }
            T::Unsupported => {}
        }
        col.into_any_element()
    }
    fn default_choice(
        &self,
        options: Vec<(String, String)>,
        current: String,
        cx: &mut Context<Self>,
        make: impl Fn(String) -> crate::api::thinking::ThinkingScheme + 'static,
    ) -> gpui::AnyElement {
        let make = std::rc::Rc::new(make);
        let chips: Vec<_> = options
            .into_iter()
            .enumerate()
            .map(|(i, (value, label))| {
                let make = make.clone();
                let on = value == current;
                self.chip(
                    gpui::SharedString::from(format!("model-thinking-default-{i}")),
                    label,
                    on,
                    cx,
                    move |v, cx| v.set_thinking(make(value.clone()), cx),
                )
            })
            .collect();
        div()
            .flex()
            .items_center()
            .flex_wrap()
            .gap(px(6.))
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(ZORK_UI.palette.subtle))
                    .mr(px(2.))
                    .child("默认"),
            )
            .children(chips)
            .into_any_element()
    }
    fn model_editor_body(&mut self, window: &mut Window, cx: &mut Context<Self>) -> gpui::Div {
        use crate::api::model_catalog::Field;
        let p = ZORK_UI.palette;
        let recognized = self.recognized.clone();
        let open = self.model_params_open;
        let toggle_label = if open { "收起参数" } else { "调整参数" };
        let recognition = match recognized {
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
        let mut body = div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(self.input("profile-model", "模型 ID", &self.model, cx))
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
                .text_size(px(12.5))
                .child(ui::icon("icons/chevron-down.svg", 12.).when(open, |icon| {
                    icon.with_transformation(gpui::Transformation::rotate(gpui::radians(
                        std::f32::consts::PI,
                    )))
                }))
                .on_click(cx.listener(|v, _, _, cx| {
                    v.model_params_open = !v.model_params_open;
                    v.reference_open = None;
                    zork_ui::components::region::invalidate(cx, &["dialog"]);
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
                v.params_touched = true;
                zork_ui::components::region::invalidate_all(cx);
            },
        );
        let context = self
            .input("profile-context-limit", "", &self.context_limit, cx)
            .w(px(140.));
        let output = self
            .input("profile-output-limit", "", &self.output_limit, cx)
            .w(px(140.));
        let context_ref = self.reference_toggle(Field::Context, cx);
        let output_ref = self.reference_toggle(Field::Output, cx);
        let thinking_ref = self.reference_toggle(Field::Thinking, cx);
        let caps_ref = self.reference_toggle(Field::Capabilities, cx);
        let thinking = self.thinking_controls(cx);
        let image = self.chip(
            "profile-model-image-input",
            self.locale.text("model_image_input").to_string(),
            self.model_image_input,
            cx,
            |v, cx| {
                v.model_image_input = !v.model_image_input;
                v.params_touched = true;
                zork_ui::components::region::invalidate(cx, &["dialog"]);
            },
        );
        let row = |a: gpui::AnyElement, b: gpui::AnyElement| {
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .child(a)
                .child(b)
                .into_any_element()
        };
        body = body
            .child(self.param_row("协议", div().w(px(220.)).child(api).into_any_element()))
            .child(self.param_row("上下文", row(context.into_any_element(), context_ref)))
            .children(self.reference_list(Field::Context, cx))
            .child(self.param_row("最长输出", row(output.into_any_element(), output_ref)))
            .children(self.reference_list(Field::Output, cx))
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(8.))
                    .child(
                        div()
                            .w(px(76.))
                            .pt(px(6.))
                            .flex_shrink_0()
                            .text_size(px(12.5))
                            .text_color(rgb(p.muted))
                            .child("思考方式"),
                    )
                    .child(div().flex_1().min_w_0().child(thinking))
                    .child(thinking_ref),
            )
            .children(self.reference_list(Field::Thinking, cx))
            .child(self.param_row("能力", row(image, caps_ref)))
            .children(self.reference_list(Field::Capabilities, cx));
        body
    }
}

impl ProfilesView {
    fn render_header(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let p = ZORK_UI.palette;
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
                            .flex()
                            .items_center()
                            .gap_2()
                            .text_size(px(12.))
                            .text_color(rgb(p.muted))
                            .child(zork_ui::device_name::label(
                                "profile-page-device",
                                self.device_name.clone(),
                                &self.device_status,
                                None,
                            ))
                            .child(format!("· {} 个连接", self.profiles.len())),
                    ),
            )
            .when(true, |header| {
                header.child(
                    ui::page_action("profile-add", "添加连接")
                        .on_click(cx.listener(|v, _, _, cx| {
                            v.add_connection(cx);
                        }))
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
        let id = profile.profile_id.clone();
        self.render_profile_row_with_click(
            profile,
            &self.catalog,
            self.quota_failures.contains(&profile.profile_id),
            None,
            None,
            cx.listener(move |v, _, _, cx| v.open_detail(id.clone(), cx)),
        )
    }

    pub(super) fn render_profile_row_with_click(
        &self,
        profile: &ProfileInfo,
        catalog: &[Value],
        quota_failed: bool,
        device_label: Option<&str>,
        device_status: Option<&zork_ui::device_name::DeviceStatus>,
        on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static,
    ) -> gpui::AnyElement {
        use zork_ui::components::profile_card::{
            self, DeviceIdentity, ProfileCard, Quota, QuotaWindow,
        };

        let id = profile.profile_id.clone();
        let row_key = self
            .row_scope
            .as_ref()
            .map(|scope| format!("{scope}-{id}"))
            .unwrap_or_else(|| id.clone());
        let provider = catalog.iter().find(|p| p["id"] == profile.provider);
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
        let billing_summary = format!(
            "{}{}{}",
            provider_name,
            if billing.is_empty() { "" } else { " · " },
            billing
        );
        let verified = profile.is_verified();
        let quota = if quota_failed {
            QuotaPresentation::failure(self.locale)
        } else {
            QuotaPresentation::new(profile, self.locale)
        };
        let quota = quota.visible().then(|| Quota {
            summary: quota.summary,
            failed: quota.failed,
            windows: quota
                .windows
                .into_iter()
                .map(|window| QuotaWindow {
                    label: window.label,
                    short_label: window.short_label,
                    remaining: window.remaining,
                    center_value: window.center_value,
                    value: window.value,
                    reset: window.reset,
                })
                .collect(),
            balance: quota.balance,
        });
        let accessible = device_label
            .zip(device_status)
            .map(|(device, status)| {
                format!(
                    "{} · {}",
                    profile.display_name(),
                    zork_ui::device_name::accessible_summary(device, status, None)
                )
            })
            .unwrap_or_else(|| profile.display_name().to_owned());
        profile_card::render(
            ProfileCard {
                key: row_key,
                provider: profile.provider.clone(),
                name: profile.display_name().to_owned(),
                device: device_label
                    .zip(device_status)
                    .map(|(name, status)| DeviceIdentity {
                        name: name.to_owned(),
                        status: status.clone(),
                    }),
                billing: billing_summary,
                model_count: if profile.models.is_empty() {
                    "待配置模型".to_owned()
                } else {
                    format!("{} 个模型", profile.models.len())
                },
                verified,
                verification: self
                    .locale
                    .text(if verified {
                        "profile_verified"
                    } else {
                        "profile_unverified"
                    })
                    .to_owned(),
                quota,
            },
            on_click,
        )
        .automation(AutomationRole::Button, accessible)
        .into_any_element()
    }
}
