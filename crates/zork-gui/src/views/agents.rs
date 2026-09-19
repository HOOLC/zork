use super::*;
use serde_json::Value;
impl RootView {
    pub fn new_desktop(
        client: Arc<StationClient>,
        store: Arc<crate::desktop::store::ClientStore>,
        node_id: String,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut view = Self::new(client, Some((store, node_id)), cx);
        let mut updates = view.core_device.subscribe();
        view.apply_device_update(updates.snapshot(), cx);
        view.refresh_queued();
        if let Some(session) =
            view.read_cache::<String>(zork_client_core::preferences::ViewState::LastSession)
        {
            view.select_session(&session, cx);
        }
        view
    }
    pub(crate) fn prepare_device_request(&mut self, cx: &mut Context<Self>) {
        let existing = self.composer_input.read(cx).value().to_owned();
        let request = "请帮我连接一台设备，先与我确认设备信息和接入方式。";
        self.composer_input.update(cx, |input, cx| {
            input.set_value(
                if existing.trim().is_empty() {
                    request.into()
                } else {
                    format!("{existing}\n{request}")
                },
                cx,
            )
        });
        self.save_draft(cx);
    }
    pub(crate) fn attach_navigation(
        &mut self,
        navigation: Entity<crate::desktop::navigation::DeviceNavigation>,
        name: String,
    ) {
        self.device_navigation = Some(navigation);
        self.device_name = Some(name);
    }
    pub(crate) fn start_device_updates(&mut self, cx: &mut Context<Self>) {
        self.start_background(cx);
    }
    pub(crate) fn core_device(&self) -> Arc<zork_client_core::state::Device> {
        self.core_device.clone()
    }
    pub(crate) fn navigation_selection(&self) -> (crate::desktop::navigation::Selection, Locale) {
        (
            crate::desktop::navigation::Selection {
                selected_leader: self.active_leader.clone(),
                selected_session: self.selected_session.clone(),
                route: Some(self.shell.route().clone()),
                reading_tail: self.transcript_list.is_following_tail(),
            },
            self.locale,
        )
    }
    pub(super) fn notify_navigation(&self, cx: &mut Context<Self>) {
        let (selection, locale) = self.navigation_selection();
        cx.emit(super::NavigationChanged { selection, locale });
    }
    pub(crate) fn navigate_device(
        &mut self,
        destination: &crate::desktop::navigation::Destination,
        cx: &mut Context<Self>,
    ) {
        use crate::desktop::navigation::Destination;
        // Navigation can arrive from a sibling view before our scheduled frame.
        // Apply the pending core batch before resolving its selected identity.
        self.deliver_core_updates(cx);
        match destination {
            Destination::Conversation { session, leader } => {
                self.active_leader = leader.clone();
                self.select_session(session, cx);
            }
            Destination::Home => self.navigate_shell(ShellRoute::Home, cx),
            Destination::Leader(id) | Destination::PrepareDevice(id) => {
                // The sidebar can deliver input before this view's next frame
                // applies its catalog batch. Resolve the ID from core's current
                // read-only snapshot instead of the previous painted catalog.
                let state = self.core_device.snapshot();
                if let Some(agent) = state.agents.iter().find(|a| a["id"] == *id).cloned() {
                    self.open_leader(agent, cx);
                    if matches!(destination, Destination::PrepareDevice(_)) {
                        self.prepare_device_request(cx);
                    }
                }
            }
            Destination::Task { leader, session } => {
                self.active_leader = Some(leader.clone());
                self.select_session(session, cx);
            }
            Destination::Page(route) => self.navigate_shell(route.clone(), cx),
            Destination::Manage(_) | Destination::SharedFiles => {}
        }
        self.notify_navigation(cx);
        zork_ui::components::region::invalidate_all(cx);
    }
    pub fn refresh_agents(&mut self, cx: &mut Context<Self>) {
        let core = self.core_device.clone();
        cx.spawn(async move |_, _| {
            core.refresh(
                zork_client_core::state::Domains::AGENTS | zork_client_core::state::Domains::TASKS,
            )
            .await
        })
        .detach();
    }
    fn open_leader(&mut self, agent: Value, cx: &mut Context<Self>) {
        if let Some(id) = agent["session_id"].as_str() {
            self.select_session(id, cx);
            return;
        }
        let Some(id) = agent["id"].as_str().map(str::to_owned) else {
            return;
        };
        self.active_leader = Some(id.clone());
        let selected = self.selected_session.clone();
        let core = self.core_device.clone();
        cx.spawn(async move |view, cx| {
            let result = core.open_agent(&id).await;
            let _ = view.update(cx, |view, cx| {
                if view.active_leader.as_deref() != Some(&id) || view.selected_session != selected {
                    return;
                }
                match result {
                    Ok(value) => {
                        if let Some(session) = value["session_id"].as_str() {
                            view.select_session(session, cx);
                        }
                    }
                    Err(error) => {
                        view.error = Some(error.to_string());
                        zork_ui::components::region::invalidate_all(cx);
                    }
                }
            });
        })
        .detach();
    }
    pub(super) fn selected_leader_id(&self) -> Option<String> {
        let id = self.selected_session.as_deref()?;
        self.node_agents
            .iter()
            .find(|a| a["session_id"] == id)
            .and_then(|a| a["id"].as_str())
            .map(str::to_owned)
    }

    pub(super) fn render_leader_home(&mut self, cx: &mut Context<Self>) -> Div {
        use zork_client_core::state::AgentAvailability;
        let availability = self.core_device.agent_availability();
        if matches!(
            availability,
            AgentAvailability::Loading | AgentAvailability::Unavailable
        ) {
            return div()
                .flex_1()
                .min_w_0()
                .min_h_0()
                .flex()
                .items_center()
                .justify_center()
                .child(if availability == AgentAvailability::Loading {
                    div()
                        .child(zork_ui::components::loading::status(
                            "device-home-loading",
                            self.locale.text("device_home_loading"),
                        ))
                        .into_any_element()
                } else {
                    div()
                        .id("device-home-unavailable")
                        .max_w(px(460.))
                        .px_8()
                        .child(zork_ui::controls::heading(
                            self.locale.text("device_home_unavailable"),
                            self.locale.text("device_home_unavailable_detail"),
                        ))
                        .automation(
                            AutomationRole::Status,
                            self.locale.text("device_home_unavailable"),
                        )
                        .into_any_element()
                });
        }
        let empty = availability == AgentAvailability::Empty;
        let data = zork_ui::welcome::Data {
            title: if empty {
                "创建你的第一位 领队"
            } else {
                "从一段对话开始"
            }
            .into(),
            description: if empty {
                "在设备设置中添加模型连接，再创建 领队。"
            } else {
                self.locale.text("device_choose_leader")
            }
            .into(),
        };
        let brand = if empty {
            self.onboarding_brand.clone()
        } else {
            self.home_brand.clone()
        };
        let welcome = self
            .welcome
            .get_or_insert_with(|| {
                cx.new(|_| zork_ui::welcome::Welcome::new(data.clone(), brand.clone()))
            })
            .clone();
        welcome.update(cx, |view, cx| {
            view.configure(data, brand, self.composer_surface_width, cx)
        });
        div().flex_1().min_w_0().min_h_0().flex().child(welcome)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn selecting_leader_is_navigation_without_starting(cx: &mut gpui::TestAppContext) {
        let view = cx.new(|cx| {
            RootView::new(
                Arc::new(StationClient::new("http://127.0.0.1:9", None)),
                None,
                cx,
            )
        });
        view.update(cx, |view, cx| {
            view.agent_online = true;
            view.selected_session = Some("current".into());
            view.messages_request = 7;
            view.error = Some("existing error".into());
            let agent = serde_json::json!({"id": "leader", "session_id": "current"});
            for _ in 0..3 {
                view.open_leader(agent.clone(), cx);
                assert_eq!(view.messages_request, 7);
                assert_eq!(view.error.as_deref(), Some("existing error"));
                assert_eq!(view.shell.route(), &ShellRoute::Task("current".into()));
            }
            view.open_leader(serde_json::json!({"id": "unallocated"}), cx);
            assert_eq!(view.selected_session.as_deref(), Some("current"));
        });
    }
}
