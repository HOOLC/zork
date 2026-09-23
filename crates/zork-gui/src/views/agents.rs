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
        view.watch_new_chat(cx);
        if let Some(session) =
            view.read_cache::<String>(zork_client_core::preferences::ViewState::LastSession)
        {
            view.select_session(&session, cx);
        }
        view
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
            self.preview_original.clone().unwrap_or_else(|| {
                crate::desktop::navigation::Selection {
                    selected_leader: self.active_leader.clone(),
                    selected_session: self.selected_session.clone(),
                    route: Some(self.shell.route().clone()),
                    reading_tail: self.transcript_list.is_following_tail(),
                }
            }),
            self.locale,
        )
    }
    pub(crate) fn preview_session(
        &mut self,
        id: &str,
        overlay: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.preview_original.is_none() {
            if !overlay
                && self.selected_session.as_deref() == Some(id)
                && matches!(self.shell.route(), ShellRoute::Task(_))
            {
                return false;
            }
            self.save_reading_position();
            self.save_chat_history();
            self.preview_original = Some(self.navigation_selection().0);
        }
        if self.selected_session.as_deref() != Some(id) {
            self.save_draft(cx);
            self.select_session(id, cx);
            self.restore_draft(cx);
        }
        self.preview_overlay = overlay;
        true
    }
    pub(crate) fn restore_preview(&mut self, cx: &mut Context<Self>) {
        let Some(original) = self.preview_original.clone() else {
            return;
        };
        self.save_draft(cx);
        self.active_leader = original.selected_leader;
        self.navigate_shell(original.route.unwrap_or(ShellRoute::Home), cx);
        self.preview_original = None;
        self.preview_overlay = false;
        self.restore_draft(cx);
        self.notify_navigation(cx);
        zork_ui::components::region::invalidate_all(cx);
    }
    pub(crate) fn commit_preview(&mut self, cx: &mut Context<Self>) {
        if self.preview_original.is_some() {
            self.save_draft(cx);
            self.preview_original = None;
            self.preview_overlay = false;
            if let Some(id) = &self.selected_session {
                self.persist_cache(zork_client_core::preferences::ViewState::LastSession, id);
            }
            self.restore_draft(cx);
            self.refresh_agents(cx);
            self.notify_navigation(cx);
            zork_ui::components::region::invalidate_all(cx);
        }
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
            Destination::Home => {
                let _ = self.core_device.new_chat().begin();
                self.navigate_shell(ShellRoute::Home, cx);
                if let Some(page) = &self.new_chat_page {
                    page.update(cx, |v, cx| v.focus(cx));
                }
            }
            Destination::Leader(id) => {
                // The sidebar can deliver input before this view's next frame
                // applies its catalog batch. Resolve the ID from core's current
                // read-only snapshot instead of the previous painted catalog.
                let state = self.core_device.snapshot();
                if let Some(agent) = state.agents.iter().find(|a| a["id"] == *id).cloned() {
                    self.open_leader(agent, cx);
                }
            }
            Destination::Manage(_) | Destination::SharedFiles => {}
        }
        self.notify_navigation(cx);
        zork_ui::components::region::invalidate_all(cx);
    }
    pub fn refresh_agents(&mut self, cx: &mut Context<Self>) {
        #[cfg(test)]
        if self.benchmark_offline {
            return;
        }
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
        // A compatibility notification may refer to a definition without a home.
        // Opening it must not allocate an empty Chat.
        self.navigate_shell(ShellRoute::Home, cx);
    }
    pub(super) fn selected_leader_id(&self) -> Option<String> {
        let id = self.selected_session.as_deref()?;
        self.node_agents
            .iter()
            .find(|a| a["session_id"] == id)
            .and_then(|a| a["id"].as_str())
            .map(str::to_owned)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn hover_preview_restores_the_original_chat_until_clicked(cx: &mut gpui::TestAppContext) {
        let view = cx.new(|cx| {
            let mut view = RootView::new(
                Arc::new(StationClient::new("http://127.0.0.1:9", None)),
                None,
                cx,
            );
            view.benchmark_offline = true;
            view
        });
        view.update(cx, |view, cx| {
            view.selected_session = Some("original".into());
            view.shell.navigate(ShellRoute::Task("original".into()));
            view.composer_input
                .update(cx, |input, cx| input.reset_value("original draft", cx));
            view.core_device
                .edit_draft("hovered", "hovered draft".into())
                .unwrap();
            assert!(view.preview_session("hovered", false, cx));
            assert_eq!(view.selected_session.as_deref(), Some("hovered"));
            assert_eq!(view.composer_input.read(cx).value(), "hovered draft");
            view.composer_input
                .update(cx, |input, cx| input.reset_value("hovered edit", cx));
            view.save_draft(cx);
            assert_eq!(view.core_device.draft("hovered").text, "hovered edit");
            assert_eq!(view.core_device.draft("original").text, "original draft");
            assert_eq!(
                view.navigation_selection().0.selected_session.as_deref(),
                Some("original")
            );
            view.restore_preview(cx);
            assert_eq!(view.selected_session.as_deref(), Some("original"));
            assert_eq!(view.composer_input.read(cx).value(), "original draft");
            assert_eq!(view.core_device.draft("hovered").text, "hovered edit");
            assert_eq!(view.shell.route(), &ShellRoute::Task("original".into()));

            assert!(view.preview_session("original", true, cx));
            assert!(view.preview_overlay);
            view.restore_preview(cx);
            assert!(!view.preview_overlay);

            assert!(view.preview_session("hovered", false, cx));
            view.commit_preview(cx);
            assert_eq!(view.selected_session.as_deref(), Some("hovered"));
            assert_eq!(view.composer_input.read(cx).value(), "hovered edit");
            assert_eq!(
                view.navigation_selection().0.selected_session.as_deref(),
                Some("hovered")
            );
        });
    }

    #[gpui::test]
    fn legacy_home_navigation_never_allocates_an_empty_chat(cx: &mut gpui::TestAppContext) {
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
            assert!(view.selected_session.is_none());
            assert_eq!(view.shell.route(), &ShellRoute::Home);
            assert!(view.core_device.snapshot().sessions.is_empty());
        });
    }
}
