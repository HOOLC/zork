use super::*;
impl RootView {
    pub(super) fn history_statistics_data(&mut self) -> zork_ui::history_page::Statistics {
        let participant = self
            .participants
            .iter()
            .find(|p| Some(&p.session_id) == self.history.session.as_ref());
        let agent = self
            .node_agents
            .iter()
            .find(|a| a["session_id"].as_str() == self.history.session.as_deref());
        let raw = self.history.runtime.as_ref();
        zork_ui::history_page::Statistics {
            usage: self.history.loaded_usage.clone(),
            calls: self.history.model_calls,
            models: self.history.models.clone(),
            runtime: zork_ui::history_page::Runtime {
                name: self.history_name(),
                avatar: participant
                    .and_then(|p| p.avatar.clone())
                    .or_else(|| agent.and_then(|a| a["avatar"].as_str()).map(str::to_owned)),
                role: agent
                    .and_then(|a| a["role"].as_str())
                    .and_then(|role| match role {
                        "leader" => Some(self.locale.text("history_role_leader").into()),
                        "worker" => Some(self.locale.text("history_role_worker").into()),
                        _ => None,
                    }),
                environment: self.device_name.as_ref().map(|name| {
                    zork_ui::device_name::summary(name, &self.core_device.snapshot().status, None)
                }),
                provider: raw
                    .and_then(|r| r.profile.as_ref())
                    .map(|p| p.provider.clone()),
                model: raw.and_then(|r| r.model.clone()),
            },
        }
    }
}
