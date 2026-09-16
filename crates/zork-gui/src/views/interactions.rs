use super::*;

impl RootView {
    pub(crate) fn activate_interaction(
        &mut self,
        session: &str,
        message: &str,
        action: &str,
        values: std::collections::BTreeMap<String, String>,
        cx: &mut Context<Self>,
    ) {
        if let Some(url) = self
            .core_device
            .conversation(session)
            .interaction_open_url(message, action)
        {
            cx.open_url(&url);
            return;
        }
        let result = zork_client_core::interactions::Command::from_action(message, action, values)
            .and_then(|command| {
                self.core_device
                    .conversation(session)
                    .respond_to_interaction(command)
            });
        if let Err(error) = result {
            self.error = Some(error.to_string());
        }
        self.deliver_core_updates(cx);
        zork_ui::components::region::invalidate(cx, &["transcript"]);
    }
}
