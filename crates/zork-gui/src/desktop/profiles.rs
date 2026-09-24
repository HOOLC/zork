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

mod editor;
mod page;
mod wizard;
pub use wizard::{Retarget, SaveTarget};

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
    api_touched: bool,
    thinking_focus: gpui::FocusHandle,
    budget_input: Entity<ComposerInput>,
    page_focus: gpui::FocusHandle,
    page_focus_pending: bool,
    menu_open: Option<String>,
    create_step: u8,
    pending_id: Option<String>,
    created: Option<String>,
    created_detail: Option<Value>,
    targets: Vec<SaveTarget>,
    retarget: Option<Retarget>,
    target_open: bool,
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
        self.create_step = 1;
        self.pending_id = None;
        self.created = None;
        self.created_detail = None;
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
        let budget_input = cx.new(|cx| ComposerInput::new("自定义 K", cx).single_line());
        cx.subscribe(
            &budget_input,
            |view, _, _: &crate::components::text_input::ComposerEdited, cx| {
                view.apply_budget_input(cx)
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
            api_touched: false,
            thinking_focus: cx.focus_handle(),
            budget_input,
            page_focus: cx.focus_handle(),
            page_focus_pending: false,
            menu_open: None,
            create_step: 1,
            pending_id: None,
            created: None,
            created_detail: None,
            targets: vec![],
            retarget: None,
            target_open: false,
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
                match self.pending_id.take().filter(|_| self.form_open) {
                    Some(id) => self.enter_models_step(id, cx),
                    None => self.form_open = false,
                }
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
        if let Some(id) = self.created.clone().filter(|_| !self.discovering) {
            self.created_detail = Some(self.source.detail(json!({"profile_id": id})));
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
                // Ink by default; colour only when the window runs low.
                let color = if window.remaining < 10. {
                    p.danger
                } else if window.remaining < 30. {
                    p.warning
                } else {
                    p.text
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
                        v.page_focus_pending = true;
                        v.menu_open = None;
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
        self.api_touched = self.editing_model.is_some();
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
                        view.key.update(cx, |v, cx| v.clear(cx));
                        view.message = None;
                        match view.pending_id.take() {
                            Some(id) => view.enter_models_step(id, cx),
                            None => view.form_open = false,
                        }
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
                            match view.pending_id.take() {
                                Some(id) => view.enter_models_step(id, cx),
                                None => view.form_open = false,
                            }
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
            self.editing_model = None;
            self.api_open = false;
        } else if self.detail.is_some() {
            self.detail = None;
            self.discovered = None;
            self.renaming = false;
            self.menu_open = None;
        } else {
            self.cancel(cx);
            self.form_open = false;
            self.create_step = 1;
            self.created = None;
            self.created_detail = None;
            self.pending_id = None;
            self.provider_open = false;
            self.key.update(cx, |input, cx| input.clear(cx));
        }
        self.message = None;
        zork_ui::components::region::invalidate_all(cx);
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
        let paged = self.detail.clone();
        let show = paged.is_none() && (self.form_open || self.attempt.is_some());
        let model_dialog = self.model_form_open && paged.is_some() && !self.inline_editing();
        let modal_key = if model_dialog {
            Some("model-editor-dialog")
        } else if show {
            Some("profile-create-dialog")
        } else {
            None
        };
        self.modal.sync(modal_key, window, cx);
        let create_visible = self
            .modal
            .retain("profile-create-dialog", show.then_some(()), cx)
            .is_some();
        let model_visible = self.modal.retain(
            "model-editor-dialog",
            model_dialog.then(|| self.editing_model.clone()),
            cx,
        );
        let model_dialog_element = model_visible.map(|editing| {
            let body = self.model_editor_body(window, cx);
            let actions = self.model_editor_actions(cx);
            ui::modal(
                "model-editor-dialog",
                if editing.is_some() { "编辑模型" } else { "添加模型" },
                body,
                actions,
                self.message.clone(),
                &self.modal,
                window,
                cx,
                !self.busy,
                |v, _, cx| v.close_dialog(cx),
            )
        });
        if let Some(detail) = paged {
            let page = self.render_page(detail, window, cx);
            return div()
                .w_full()
                .child(page)
                .children(model_dialog_element)
                .into_any_element();
        }
        let wizard = create_visible.then(|| self.wizard(window, cx));
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
            .when_some(wizard, |v, (body, actions)| {
                v.child(ui::modal_sized(
                    "profile-create-dialog",
                    "添加模型连接",
                    body,
                    actions,
                    self.message.clone(),
                    &self.modal,
                    520.,
                    window,
                    cx,
                    !self.busy || self.attempt.is_some(),
                    |v, _, cx| v.close_dialog(cx),
                ))
            })
            .when_some(
                self.message.clone().filter(|_| modal_key.is_none()),
                |v, m| v.child(ui::status_notice(m, ui::NoticeKind::Error)),
            )
            .into_any_element()
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
