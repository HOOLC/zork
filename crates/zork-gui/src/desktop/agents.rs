use super::ui;
#[cfg(feature = "headless-bench")]
use crate::api::StationClient;
use crate::{
    api::ProfileInfo,
    automation::{AutomationElementExt, AutomationRole},
    components::text_input::ComposerInput,
    design::CUE_UI,
};
use gpui::{div, prelude::*, px, rgb, Context, Entity, FontWeight, Window};
#[cfg(test)]
use serde_json::json;
use serde_json::Value;
use std::{collections::HashSet, sync::Arc};
// View-only choice. The actual profile catalog remains unchanged; "auto" is
// reserved by the server and resolves at invocation time.
use crate::api::profile_options;

fn profile_label(id: &str) -> &str {
    if id.is_empty() || id == "auto" {
        "自动分配"
    } else {
        id
    }
}

pub struct AgentsView {
    #[cfg(not(target_family = "wasm"))]
    resources: Option<(
        Arc<zork_client_core::resources::Resources>,
        String,
        crate::i18n::Locale,
    )>,
    #[cfg(not(target_family = "wasm"))]
    skills: Option<(String, Entity<super::resources::ResourcesView>)>,
    source: Arc<crate::api::Agents>,
    source_updates: Option<gpui::Task<()>>,
    regions: zork_ui::components::region::Regions<Self>,
    device_name: String,
    modal: ui::ModalState,
    agents: Vec<Value>,
    profiles: Vec<ProfileInfo>,
    profile: usize,
    model: usize,
    thinking: String,
    thinking_open: bool,
    profile_open: bool,
    model_open: bool,
    name: Entity<ComposerInput>,
    instructions: Entity<ComposerInput>,
    remote_grants: Entity<ComposerInput>,
    worker: bool,
    grants: HashSet<String>,
    editing: Option<Value>,
    node_origin: Option<String>,
    creation_id: String,
    avatar: String,
    editing_avatar: Option<Value>,
    form_open: bool,
    busy: bool,
    message: Option<String>,
}
impl AgentsView {
    #[cfg(not(target_family = "wasm"))]
    pub fn set_resources(
        &mut self,
        resources: Arc<zork_client_core::resources::Resources>,
        node: String,
        locale: crate::i18n::Locale,
    ) {
        self.resources = Some((resources, node, locale));
    }
    fn skill_section(&self) -> gpui::Div {
        let body = div();
        #[cfg(not(target_family = "wasm"))]
        let body = body.when_some(
            self.skills.as_ref().map(|(_, view)| view.clone()),
            |body, view| body.child(view),
        );
        body
    }
    fn sync_skills(&mut self, _cx: &mut Context<Self>) {
        #[cfg(not(target_family = "wasm"))]
        {
            let selected = self
                .editing_avatar
                .as_ref()
                .and_then(|agent| agent["id"].as_str());
            if self.skills.as_ref().map(|(id, _)| id.as_str()) != selected {
                self.skills = selected.and_then(|agent| {
                    self.resources.as_ref().map(|(core, node, locale)| {
                        (
                            agent.to_owned(),
                            _cx.new(|cx| {
                                super::resources::ResourcesView::skills(
                                    core.clone(),
                                    node.clone(),
                                    agent.to_owned(),
                                    *locale,
                                    cx,
                                )
                            }),
                        )
                    })
                });
            }
        }
    }

    pub fn set_device_name(&mut self, name: String) {
        self.device_name = name;
    }
    pub fn new_with_source(source: Arc<crate::api::Agents>, cx: &mut Context<Self>) -> Self {
        let mut view = Self::new_inner(source, cx);
        view.watch_source(cx);
        view.refresh(cx);
        view
    }
    fn watch_source(&mut self, cx: &mut Context<Self>) {
        let mut updates = self.source.subscribe();
        self.apply_source(updates.snapshot(), cx);
        self.source_updates = Some(cx.spawn(async move |this, cx| {
            while let Some(update) = updates.changed().await {
                if this
                    .update(cx, |view, cx| view.apply_source(update, cx))
                    .is_err()
                {
                    return;
                }
            }
        }));
    }
    fn apply_source(&mut self, update: crate::api::AgentUpdate, cx: &mut Context<Self>) {
        if update.agents_changed {
            self.agents = update.state.agents.as_ref().clone();
        }
        if update.profiles_changed {
            let selected = self.selected_model().map(|m| m.id.clone());
            let profile_id = self
                .profiles
                .get(self.profile)
                .map(|p| p.profile_id.clone());
            self.profiles = profile_options(update.state.profiles.as_ref());
            if let Some(id) = profile_id {
                self.profile = self
                    .profiles
                    .iter()
                    .position(|p| p.profile_id == id)
                    .unwrap_or(usize::MAX);
            }
            if let Some(id) = selected {
                self.model = self
                    .models()
                    .iter()
                    .position(|m| m.id == id)
                    .unwrap_or(usize::MAX);
            }
            if self.thinking.is_empty() {
                self.thinking = self
                    .selected_model()
                    .map(|m| m.default_thinking.clone())
                    .unwrap_or_default();
            }
        }

        if update.origin_changed {
            self.node_origin = update.state.node_origin.clone();
        }
        if update.error_changed {
            self.message = update.state.error.clone();
        }
        let mut regions = vec!["form"];
        if update.agents_changed {
            regions.extend(["list", "header"]);
        }
        zork_ui::components::region::invalidate(cx, &regions);
    }
    fn new_inner(source: Arc<crate::api::Agents>, cx: &mut Context<Self>) -> Self {
        let mut field = |label| {
            let e = cx.new(|cx| ComposerInput::new(label, cx));
            cx.observe(&e, |_, _, cx| {
                zork_ui::components::region::invalidate(cx, &["form"])
            })
            .detach();
            e
        };
        let name = field("小伙伴名称");
        let instructions = field("职责和偏好（可选）");
        let remote_grants = field("粘贴远端领队引用，多个以空格分隔");
        Self {
            #[cfg(not(target_family = "wasm"))]
            resources: None,
            #[cfg(not(target_family = "wasm"))]
            skills: None,
            modal: ui::ModalState::new(cx),
            source,
            source_updates: None,
            regions: Default::default(),
            device_name: String::new(),
            agents: vec![],
            profiles: vec![],
            profile: 0,
            model: 0,
            thinking: String::new(),
            thinking_open: false,
            profile_open: false,
            model_open: false,
            name,
            instructions,
            remote_grants,
            worker: false,
            grants: HashSet::new(),
            editing: None,
            node_origin: None,
            creation_id: ulid::Ulid::new().to_string(),
            avatar: "cat".into(),
            editing_avatar: None,
            form_open: false,
            busy: false,
            message: None,
        }
    }
    #[cfg(feature = "headless-bench")]
    pub fn headless_fixture(cx: &mut Context<Self>) -> Self {
        #[cfg(not(target_family = "wasm"))]
        let client = Arc::new(StationClient::fixture(zork_ui::stories::page_fixture(),
            serde_json::from_str(include_str!("../../tests/fixtures/provider_catalog.json")).expect("provider fixture")));
        #[cfg(target_family = "wasm")]
        let client = Arc::new(StationClient::new("http://127.0.0.1:9", None));
        let source = crate::api::Agents::new(client.clone(), crate::api::Profiles::new(client));
        let mut view = Self::new_inner(source, cx);
        let fixture = zork_ui::stories::page_fixture();
        view.device_name = fixture["device"]["name"].as_str().unwrap().into();
        view.agents = fixture["agents"].as_array().unwrap().clone();
        view.profiles = vec![serde_json::from_value(fixture["profile"].clone()).unwrap()];
        view.source.seed(crate::api::AgentData {
            agents: Arc::new(view.agents.clone()),
            loaded: true,
            profiles: Arc::new(view.profiles.clone()),
            ..Default::default()
        });
        view.watch_source(cx);
        view.creation_id = "fixture-agent".into();
        view
    }
    fn close_dialog(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.form_open = false;
        self.profile_open = false;
        self.model_open = false;
        self.thinking_open = false;
        self.editing = None;
        self.editing_avatar = None;
        self.message = None;
        zork_ui::components::region::invalidate(cx, &["form"]);
    }
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let source = self.source.clone();
        cx.spawn(async move |_, _| {
            source.refresh().await;
        })
        .detach();
    }
    fn current_grants(&self, cx: &Context<Self>) -> Vec<String> {
        zork_client_core::agent_edit::grant_references(
            self.grants.iter().cloned().collect(),
            self.remote_grants.read(cx).value(),
        )
    }
    fn models(&self) -> &[crate::api::ProfileModel] {
        self.profiles
            .first()
            .map(|p| p.models.as_slice())
            .unwrap_or_default()
    }
    fn selected_model(&self) -> Option<&crate::api::ProfileModel> {
        self.models().get(self.model)
    }
    fn compatible_profiles(&self) -> Vec<usize> {
        crate::api::compatible_profiles(
            &self.profiles,
            self.selected_model().map(|m| m.id.as_str()),
            &self.thinking,
        )
    }
    fn repair_profile(&mut self) {
        self.profile = crate::api::repair_profile(
            &self.profiles,
            self.profile,
            self.selected_model().map(|m| m.id.as_str()),
            &self.thinking,
        );
    }
    fn valid_selection(&self) -> bool {
        crate::api::validate_selection(
            &self.source.snapshot().profiles,
            self.profiles
                .get(self.profile)
                .map(|p| p.profile_id.as_str())
                .unwrap_or(""),
            self.selected_model().map(|m| m.id.as_str()).unwrap_or(""),
            &self.thinking,
        )
        .is_ok()
    }
    fn create(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let Some(profile) = self.profiles.get(self.profile) else {
            self.message = Some("请先创建一个 Profile".into());
            zork_ui::components::region::invalidate(cx, &["form"]);
            return;
        };
        let Some(model) = self.selected_model().filter(|_| self.valid_selection()) else {
            return;
        };
        let input = crate::api::AgentInput {
            id: self.creation_id.clone(),
            creating: true,
            name: self.name.read(cx).value().into(),
            role: if self.worker { "worker" } else { "leader" }.into(),
            avatar: self.avatar.clone(),
            profile: profile.profile_id.clone(),
            model: model.id.clone(),
            thinking: self.thinking.clone(),
            instructions: self.instructions.read(cx).value().into(),
            allowed: self.current_grants(cx),
        };
        let core = self.source.clone();
        self.busy = true;
        self.message = None;
        cx.spawn(async move |this, cx| {
            let result = core.save(input).await;
            let _ = this.update(cx, |v, cx| {
                v.busy = false;
                match result {
                    Ok(_) => {
                        v.form_open = false;
                        v.name.update(cx, |i, cx| i.clear(cx));
                        v.instructions.update(cx, |i, cx| i.clear(cx));
                        v.creation_id = ulid::Ulid::new().to_string();
                        v.avatar = "cat".into();
                        v.message = None;
                    }
                    Err(e) => v.message = Some(e.to_string()),
                }
                zork_ui::components::region::invalidate(cx, &["form"]);
            });
        })
        .detach();
        zork_ui::components::region::invalidate(cx, &["form"]);
    }
    fn profile_dropdown(
        &self,
        editing: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let id = if editing {
            "agent-edit-profile-select"
        } else {
            "agent-profile-select"
        };
        let prefix = if editing {
            "agent-edit-profile"
        } else {
            "agent-profile"
        };
        let eligible = self.compatible_profiles();
        ui::dropdown_with_icons(
            id,
            self.profiles
                .get(self.profile)
                .map(|p| p.display_name().to_owned())
                .unwrap_or("选择模型连接".into()),
            eligible
                .iter()
                .map(|&i| {
                    let p = &self.profiles[i];
                    (
                        format!("{prefix}-{i}"),
                        p.display_name().to_owned(),
                        self.profile == i,
                    )
                })
                .collect(),
            self.profile_open,
            !self.busy && !self.profiles.is_empty(),
            self.profiles
                .get(self.profile)
                .map(|p| ui::provider_path(&p.provider)),
            eligible
                .iter()
                .map(|&i| Some(ui::provider_path(&self.profiles[i].provider)))
                .collect(),
            window,
            cx,
            |v, open, cx| {
                v.profile_open = open;
                v.model_open = false;
                v.thinking_open = false;
                zork_ui::components::region::invalidate(cx, &["form"]);
            },
            |v, index, cx| {
                v.profile = v.compatible_profiles().get(index).copied().unwrap_or(0);
                v.profile_open = false;
                zork_ui::components::region::invalidate(cx, &["form"]);
            },
        )
    }
    fn model_dropdown(
        &self,
        editing: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let id = if editing {
            "agent-edit-model-select"
        } else {
            "agent-model-select"
        };
        let prefix = if editing {
            "agent-edit-model"
        } else {
            "agent-model"
        };
        let models = self.models();
        let placeholder = if models.is_empty() {
            "没有已启用的模型"
        } else {
            "选择模型"
        };
        ui::dropdown_with_icons(
            id,
            models
                .get(self.model)
                .map(|m| m.id.clone())
                .unwrap_or(placeholder.into()),
            models
                .iter()
                .enumerate()
                .map(|(i, m)| (format!("{prefix}-{i}"), m.id.clone(), self.model == i))
                .collect(),
            self.model_open,
            !self.busy && !models.is_empty(),
            None,
            models.iter().map(|_| None).collect(),
            window,
            cx,
            |v, open, cx| {
                v.model_open = open;
                v.profile_open = false;
                v.thinking_open = false;
                zork_ui::components::region::invalidate(cx, &["form"]);
            },
            |v, index, cx| {
                v.model = index;
                if let Some(model) = v.selected_model() {
                    v.thinking = crate::api::thinking_after_choice(Some(model), &v.thinking);
                }
                v.repair_profile();
                v.model_open = false;
                zork_ui::components::region::invalidate(cx, &["form"]);
            },
        )
    }
    fn thinking_dropdown(
        &self,
        editing: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let id = if editing {
            "agent-edit-thinking-select"
        } else {
            "agent-thinking-select"
        };
        let prefix = if editing {
            "agent-edit-thinking"
        } else {
            "agent-thinking"
        };
        let levels = self
            .selected_model()
            .map(|m| m.thinking.as_slice())
            .unwrap_or_default();
        ui::dropdown_with_icons(
            id,
            if self.thinking.is_empty() {
                "选择思考深度".into()
            } else {
                self.thinking.clone()
            },
            levels
                .iter()
                .enumerate()
                .map(|(i, level)| {
                    (
                        format!("{prefix}-{i}"),
                        level.clone(),
                        *level == self.thinking,
                    )
                })
                .collect(),
            self.thinking_open,
            !self.busy && !levels.is_empty(),
            None,
            levels.iter().map(|_| None).collect(),
            window,
            cx,
            |v, open, cx| {
                v.thinking_open = open;
                v.profile_open = false;
                v.model_open = false;
                zork_ui::components::region::invalidate(cx, &["form"]);
            },
            |v, index, cx| {
                if let Some(level) = v.selected_model().and_then(|m| m.thinking.get(index)) {
                    v.thinking = level.clone();
                }
                v.repair_profile();
                v.thinking_open = false;
                zork_ui::components::region::invalidate(cx, &["form"]);
            },
        )
    }
    fn avatar_picker(&self, cx: &mut Context<Self>) -> gpui::Div {
        ui::avatar_picker(
            "agent-avatar",
            &self.avatar,
            !self.busy,
            cx,
            |v, avatar, cx| {
                v.avatar = avatar.into();
                zork_ui::components::region::invalidate(cx, &["form"]);
            },
        )
    }
    fn save_settings(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let Some(agent) = self.editing_avatar.clone() else {
            return;
        };
        if !self.valid_selection() {
            self.message = Some("请选择可用的模型和思考深度".into());
            zork_ui::components::region::invalidate(cx, &["form"]);
            return;
        }
        let selection = Some((
            self.profiles[self.profile].profile_id.clone(),
            self.selected_model().unwrap().id.clone(),
            self.thinking.clone(),
        ));
        let avatar = self.avatar.clone();
        let core = self.source.clone();
        let id = agent["id"].as_str().unwrap_or_default().to_owned();
        self.busy = true;
        self.message = None;
        cx.spawn(async move |this, cx| {
            let result = core.update_agent_settings(&id, selection, avatar).await;
            let _ = this.update(cx, |v, cx| {
                v.busy = false;
                match result {
                    Ok(()) => {
                        v.editing_avatar = None;
                    }
                    Err(error) => {
                        v.message = Some(format!("未能保存全部修改，请重试：{error}"));
                    }
                }
                zork_ui::components::region::invalidate(cx, &["form"]);
            });
        })
        .detach();
        zork_ui::components::region::invalidate(cx, &["form"]);
    }
    fn edit_grants(&mut self, agent: Value, cx: &mut Context<Self>) {
        self.grants = agent["allowed_leaders"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str().filter(|s| !s.contains('/')).map(str::to_owned))
            .collect();
        let remote = agent["allowed_leaders"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str().filter(|s| s.contains('/')))
            .collect::<Vec<_>>()
            .join(" ");
        self.remote_grants
            .update(cx, |v, cx| v.set_value(remote, cx));
        self.worker = true;
        self.editing = Some(agent);
        self.message = None;
        zork_ui::components::region::invalidate(cx, &["form"]);
    }
    fn save_grants(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let Some(agent) = self.editing.clone() else {
            return;
        };
        let id = agent["id"].as_str().unwrap_or_default().to_owned();
        let allowed = self.current_grants(cx);
        let expected = agent["allowed_leaders"].clone();
        let core = self.source.clone();
        self.busy = true;
        cx.spawn(async move |this, cx| {
            let result = core.update_agent_grants(&id, allowed, expected).await;
            let _ = this.update(cx, |v, cx| {
                v.busy = false;
                match result {
                    Ok(_) => {
                        v.editing = None;
                        v.message = Some("队员授权已更新".into());
                    }
                    Err(e) => v.message = Some(e.to_string()),
                }
                zork_ui::components::region::invalidate(cx, &["form"]);
            });
        })
        .detach();
        zork_ui::components::region::invalidate(cx, &["form"]);
    }
    fn field(
        &self,
        id: &'static str,
        label: &'static str,
        input: &Entity<ComposerInput>,
        cx: &gpui::App,
    ) -> gpui::Div {
        ui::field(id, label, input, cx)
    }
}
impl AgentsView {
    fn render_header(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = CUE_UI.palette;
        div()
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
                    .child(ui::page_title("队员"))
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(p.muted))
                            .child(format!(
                                "{} · {} 位小伙伴",
                                self.device_name,
                                self.agents.len()
                            )),
                    ),
            )
            .child(
                ui::page_action("agent-add", "添加小伙伴")
                    .on_click(cx.listener(|v, _, _, cx| {
                        v.form_open = true;
                        v.profile = 0;
                        v.message = None;
                        zork_ui::components::region::invalidate(cx, &["form"]);
                    }))
                    .map(|button| self.modal.source("agent-create-dialog").bind(button, "添加小伙伴", ui::ActionStyle { icon: Some("icons/plus.svg"), ..Default::default() }))
                    .automation(AutomationRole::Button, "创建小伙伴"),
            )
    }
    fn render_list(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = CUE_UI.palette;
        div().when(!self.agents.is_empty(), |v| {
            v.child(
                div().flex().flex_col().gap_0().children(
                    ["leader", "worker"]
                        .into_iter()
                        .filter(|role| self.agents.iter().any(|a| a["role"] == *role))
                        .map(|role| {
                            div()
                                .flex()
                                .flex_col()
                                .gap_0()
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .px_2()
                                        .pt_4()
                                        .pb_2()
                                        .text_size(px(12.))
                                        .font_weight(FontWeight::MEDIUM)
                                        .child(if role == "leader" { "领队" } else { "队员" })
                                        .child(
                                            div()
                                                .text_size(px(11.))
                                                .text_color(rgb(p.muted))
                                                .child(format!(
                                                    "{}",
                                                    self.agents
                                                        .iter()
                                                        .filter(|a| a["role"] == role)
                                                        .count()
                                                )),
                                        ),
                                )
                                .child(
                                    div()
                                        .px_2()
                                        .pb_2()
                                        .text_size(px(11.))
                                        .text_color(rgb(p.muted))
                                        .child(if role == "leader" {
                                            "与你沟通，安排任务与队员"
                                        } else {
                                            "接受领队安排，专注完成任务"
                                        }),
                                )
                                .children(
                                    self.agents
                                        .iter()
                                        .filter(|a| a["role"] == role)
                                        .cloned()
                                        .enumerate()
                                        .map(|(index, agent)| {
                                            let id =
                                                agent["id"].as_str().unwrap_or_default().to_owned();
                                            let edit = agent.clone();
                                            ui::quiet_button(format!("agent-settings-{id}"), "", true, ui::IconButtonSize::Standard)
                                                .radius(ui::FIELD_RADIUS)
                                                .justify_start()
                                                .font_weight(FontWeight::NORMAL)
                                                .w_full()
                                                .h(px(64.))
                                                .when(index > 0, |v| v.mt(px(2.)))
                                                .px_2()
                                                .text_size(px(12.))
                                                .flex()
                                                .items_center()
                                                .gap(px(9.))

                                                .child(ui::agent_avatar(
                                                    agent["avatar"].as_str(),
                                                    36.,
                                                ))
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
                                                                .font_weight(FontWeight::MEDIUM)
                                                                .child(
                                                                    agent["name"]
                                                                        .as_str()
                                                                        .unwrap_or_default()
                                                                        .to_owned(),
                                                                ),
                                                        )
                                                        .child(
                                                            div()
                                                                .truncate()
                                                                .text_size(px(11.))
                                                                .text_color(rgb(p.muted))
                                                                .child(format!(
                                                                    "{} · {}",
                                                                    agent["model"]
                                                                        .as_str()
                                                                        .unwrap_or("未配置模型"),
                                                                    profile_label(
                                                                        agent["profile_id"]
                                                                            .as_str()
                                                                            .unwrap_or(
                                                                                "未配置连接"
                                                                            )
                                                                    )
                                                                )),
                                                        ),
                                                )
                                                .on_click(cx.listener(move |v, _, _, cx| {
                                                    if !v.busy {
                                                        v.avatar = edit["avatar"]
                                                            .as_str()
                                                            .unwrap_or("cat")
                                                            .into();
                                                        v.profile = v
                                                            .profiles
                                                            .iter()
                                                            .position(|p| {
                                                                Some(p.profile_id.as_str())
                                                                    == edit["profile_id"].as_str()
                                                            })
                                                            .unwrap_or(usize::MAX);
                                                        v.model = v
                                                            .profiles
                                                            .first()
                                                            .and_then(|p| {
                                                                p.models.iter().position(|m| {
                                                                    Some(m.id.as_str())
                                                                        == edit["model"].as_str()
                                                                })
                                                            })
                                                            .unwrap_or(usize::MAX);
                                                        v.thinking = edit["thinking"]
                                                            .as_str()
                                                            .unwrap_or_default()
                                                            .into();
                                                        v.editing_avatar = Some(edit.clone());
                                                        v.message = None;
                                                        zork_ui::components::region::invalidate(
                                                            cx,
                                                            &["form"],
                                                        );
                                                    }
                                                }))
                                                .map(|row| self.modal.source("agent-editor-dialog").bind(row, agent["name"].as_str().unwrap_or("小伙伴").to_owned(), ui::ActionStyle { disabled: self.busy, ..Default::default() }))
                                                .automation(
                                                    AutomationRole::Button,
                                                    format!(
                                                        "设置 {}",
                                                        agent["name"].as_str().unwrap_or("小伙伴")
                                                    ),
                                                )
                                        }),
                                )
                        }),
                ),
            )
        })
    }
}
impl gpui::EventEmitter<ui::OpenAgent> for AgentsView {}
impl Render for AgentsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_skills(cx);
        let p = CUE_UI.palette;
        let show = self.form_open || self.editing.is_some();
        let modal_key = if self.editing_avatar.is_some() {
            Some("agent-editor-dialog")
        } else if show {
            Some("agent-create-dialog")
        } else {
            None
        };
        self.modal.sync(modal_key, window, cx);
        let avatar_visible = self.modal.retain("agent-editor-dialog", self.editing_avatar.clone(), cx);
        let create_visible = self.modal.retain("agent-create-dialog", show.then(|| self.editing.clone()), cx);
        let displayed_editing = create_visible.clone().flatten();
        div()
            .flex()
            .flex_col()
            .gap_0()
            .child(self.regions.auto_height(
                "header",
                window.viewport_size().width.as_f32(),
                cx,
                |v, _, cx| v.render_header(cx).into_any_element(),
            ))
            .child(self.regions.auto_height(
                "list",
                window.viewport_size().width.as_f32(),
                cx,
                |v, _, cx| v.render_list(cx).into_any_element(),
            ))
            .when_some(avatar_visible, |v, agent| {
                v.child(ui::modal(
                    "agent-editor-dialog",
                    "编辑小伙伴",
                    ui::section()
                        .border_t_0()
                        .py_0()
                        .child(ui::label(format!(
                            "{}",
                            agent["name"].as_str().unwrap_or_default()
                        )))
                        .child(ui::form_field(
                            "模型",
                            self.model_dropdown(true, window, cx),
                        ))
                        .child(ui::form_field(
                            "思考深度",
                            self.thinking_dropdown(true, window, cx),
                        ))
                        .child(ui::form_field(
                            "模型连接（可选）",
                            self.profile_dropdown(true, window, cx),
                        ))
                        .child(self.avatar_picker(cx))
                        .child(self.skill_section())
                        .when(agent["role"] == "worker", |v| {
                            let grant_agent = agent.clone();
                            v.child(
                                ui::button("agent-detail-grants", "管理授权", false, !self.busy)
                                    .on_click(cx.listener(move |v, _, _, cx| {
                                        v.editing_avatar = None;
                                        v.edit_grants(grant_agent.clone(), cx);
                                    }))
                                    .map(|button| self.modal.source("agent-create-dialog").bind(button, "管理授权", ui::ActionStyle { disabled: self.busy, ..Default::default() })),
                            )
                        }),
                    div()
                        .flex()
                        .justify_end()
                        .gap_2()
                        .child(
                            ui::button("agent-avatar-cancel", "取消", false, !self.busy)
                                .on_click(cx.listener(|v, _, _, cx| {
                                    if !v.busy {
                                        v.editing_avatar = None;
                                        v.avatar = "cat".into();
                                        zork_ui::components::region::invalidate(cx, &["form"]);
                                    }
                                }))
                                .automation_enabled(
                                    !self.busy,
                                    AutomationRole::Button,
                                    "取消编辑小伙伴",
                                ),
                        )
                        .child(
                            ui::busy_button(
                                "agent-avatar-save",
                                "保存修改",
                                true,
                                !self.busy,
                                self.busy,
                            )
                            .on_click(cx.listener(|v, _, _, cx| v.save_settings(cx)))
                            .automation_enabled(
                                !self.busy,
                                AutomationRole::Button,
                                "保存修改",
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
            .when(create_visible.is_some(), |v| {
                v.child(ui::modal(
                    "agent-create-dialog",
                    if displayed_editing.is_some() {
                        "管理领队授权"
                    } else {
                        "添加小伙伴"
                    },
                    ui::section()
                        .border_t_0()
                        .py_0()
                        .when_some(displayed_editing.clone(), |v, agent| {
                            v.child(ui::label(format!(
                                "{} ·领队授权",
                                agent["name"].as_str().unwrap_or_default()
                            )))
                        })
                        .when(displayed_editing.is_none(), |v| {
                            v.child(div().flex().gap_3().children([false, true].into_iter().map(
                                |worker| {
                                    let label = if worker { "队员" } else { "领队" };
                                    ui::choice(
                                        if worker { "agent-role-worker" } else { "agent-role-leader" },
                                        "", self.worker == worker, !self.busy,
                                    )
                                        .radius(10.)
                                        .h_auto()
                                        .flex_1()
                                        .min_w_0()
                                        .flex_col()
                                        .items_stretch()
                                        .gap_0()
                                        .whitespace_normal()
                                        .px_4()
                                        .py_3()
                                        .child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .gap_2()
                                                .child(ui::icon(
                                                    if worker {
                                                        "icons/checklist.svg"
                                                    } else {
                                                        "icons/sparkles.svg"
                                                    },
                                                    16.,
                                                ))
                                                .child(label)
                                                .when(self.worker == worker, |v| {
                                                    v.child(div().flex_1())
                                                        .child(ui::icon("icons/check.svg", 14.))
                                                }),
                                        )
                                        .child(
                                            div()
                                                .pt_2()
                                                .text_size(px(12.))
                                                .text_color(rgb(p.muted))
                                                .child(if worker {
                                                    "接收任务，独立执行"
                                                } else {
                                                    "长期对话，协调任务"
                                                }),
                                        )
                                        .on_click(cx.listener(move |v, _, _, cx| {
                                            if !v.busy {
                                                v.worker = worker;
                                                zork_ui::components::region::invalidate(
                                                    cx,
                                                    &["form"],
                                                );
                                            }
                                        }))
                                        .automation_enabled(
                                            !self.busy,
                                            AutomationRole::Button,
                                            label,
                                        )
                                },
                            )))
                            .child(self.field("agent-name", "名称", &self.name, cx))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .child(ui::label("模型设置"))
                                    .child(
                                        ui::button("agents-refresh", "刷新", false, !self.busy)
                                            .on_click(cx.listener(|v, _, _, cx| v.refresh(cx)))
                                            .automation_enabled(
                                                !self.busy,
                                                AutomationRole::Button,
                                                "刷新 Profiles",
                                            ),
                                    ),
                            )
                            .when(self.models().is_empty(), |v| {
                                v.child(
                                    div()
                                        .text_size(px(12.))
                                        .text_color(rgb(p.muted))
                                        .child("先到「大模型」添加一个账号。"),
                                )
                            })
                            .child(ui::form_field(
                                "模型",
                                self.model_dropdown(false, window, cx),
                            ))
                            .child(ui::form_field(
                                "思考深度",
                                self.thinking_dropdown(false, window, cx),
                            ))
                            .child(ui::form_field(
                                "模型连接（可选）",
                                self.profile_dropdown(false, window, cx),
                            ))
                            .child(self.avatar_picker(cx))
                            .child(self.field(
                                "agent-instructions",
                                "职责与偏好 · 可选",
                                &self.instructions,
                                cx,
                            ))
                        })
                        .when(self.worker, |v| {
                            v.child(ui::label("单独授权领队"))
                                .child(div().text_size(px(12.)).text_color(rgb(p.muted)).child(
                                    "同一 mesh 内的领队默认可以指派任务。此列表用于其余单独授权。",
                                ))
                                .child(
                                    div().flex().flex_wrap().gap_2().children(
                                        self.agents.iter().filter(|a| a["role"] == "leader").map(
                                            |a| {
                                                let id =
                                                    a["id"].as_str().unwrap_or_default().to_owned();
                                                let name = a["name"]
                                                    .as_str()
                                                    .unwrap_or_default()
                                                    .to_owned();
                                                let granted = self.grants.contains(&id);
                                                ui::choice(
                                                    format!("agent-grant-{id}"),
                                                    name.clone(),
                                                    granted,
                                                    !self.busy,
                                                )
                                                .child(ui::icon(
                                                    if granted {
                                                        "icons/check.svg"
                                                    } else {
                                                        "icons/plus.svg"
                                                    },
                                                    14.,
                                                ))
                                                .on_click(cx.listener(move |v, _, _, cx| {
                                                    if !v.busy {
                                                        if !v.grants.remove(&id) {
                                                            v.grants.insert(id.clone());
                                                        }
                                                        zork_ui::components::region::invalidate(
                                                            cx,
                                                            &["form"],
                                                        );
                                                    }
                                                }))
                                                .automation_enabled(
                                                    !self.busy,
                                                    AutomationRole::Button,
                                                    name,
                                                )
                                            },
                                        ),
                                    ),
                                )
                                .child(self.field(
                                    "agent-remote-grants",
                                    "其他设备的领队引用 · 可选",
                                    &self.remote_grants,
                                    cx,
                                ))
                        }),
                    div()
                        .pt_2()
                        .flex()
                        .justify_end()
                        .gap_2()
                        .when(displayed_editing.is_some(), |v| {
                            v.child(
                                ui::button("cancel-edit-grants", "取消", false, !self.busy)
                                    .on_click(cx.listener(|v, _, _, cx| {
                                        if !v.busy {
                                            v.editing = None;
                                            zork_ui::components::region::invalidate(cx, &["form"]);
                                        }
                                    }))
                                    .automation_enabled(
                                        !self.busy,
                                        AutomationRole::Button,
                                        "取消编辑",
                                    ),
                            )
                        })
                        .when(displayed_editing.is_none(), |v| {
                            v.child(
                                ui::button("agent-close-form", "取消", false, !self.busy)
                                    .on_click(cx.listener(|v, _, _, cx| {
                                        if !v.busy {
                                            v.form_open = false;
                                            zork_ui::components::region::invalidate(cx, &["form"]);
                                        }
                                    }))
                                    .automation_enabled(
                                        !self.busy,
                                        AutomationRole::Button,
                                        "取消创建",
                                    ),
                            )
                        })
                        .child(
                            ui::busy_button(
                                if displayed_editing.is_some() {
                                    "agent-save-grants"
                                } else {
                                    "agent-create"
                                },
                                if self.busy {
                                    "正在保存…"
                                } else if displayed_editing.is_some() {
                                    "保存授权"
                                } else {
                                    "创建小伙伴"
                                },
                                true,
                                !self.busy && (displayed_editing.is_some() || self.valid_selection()),
                                self.busy,
                            )
                            .on_click(cx.listener(|v, _, _, cx| {
                                if v.editing.is_some() {
                                    v.save_grants(cx)
                                } else {
                                    v.create(cx)
                                }
                            }))
                            .automation_enabled(
                                !self.busy && (displayed_editing.is_some() || self.valid_selection()),
                                AutomationRole::Button,
                                if displayed_editing.is_some() {
                                    "保存授权"
                                } else {
                                    "创建小伙伴"
                                },
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
            .when_some(
                self.message.clone().filter(|_| modal_key.is_none()),
                |v, m| v.child(ui::feedback(m)),
            )
    }
}

#[cfg(test)]
mod pool_choice_tests {
    use super::*;
    #[test]
    fn automatic_option_aggregates_enabled_models_without_mutating_catalog() {
        let profile: ProfileInfo =
            serde_json::from_value(json!({"profile_id":"a","provider":"openai","models":[
            {"id":"enabled","thinking":["high"],"default_thinking":"high"},
            {"id":"disabled","enabled":false}]}))
            .unwrap();
        let mut second = profile.clone();
        second.profile_id = "b".into();
        second.models[0].thinking = vec!["xhigh".into()];
        let catalog = vec![profile, second];
        let options = profile_options(&catalog);
        assert_eq!(options[0].profile_id, "auto");
        assert_eq!(options[0].models.len(), 1);
        assert_eq!(options[0].models[0].thinking, vec!["high", "xhigh"]);
        assert_eq!(options[1].profile_id, "a");
        assert_eq!(options[2].profile_id, "b");
        assert_eq!(catalog[0].models.len(), 2);
    }
}
