//! Client operation lifecycle for Agent configuration review cards.
use super::*;
impl Conversation {
    pub(crate) fn submit_configuration_review(
        self: &Arc<Self>,
        command: crate::interactions::Command,
    ) -> anyhow::Result<()> {
        use crate::interactions::{self, Command, Response};
        let id = command.message_id().to_owned();
        let device = self
            .device
            .upgrade()
            .ok_or_else(|| anyhow::anyhow!("Device unavailable"))?;
        let (store, node) = device
            .cache
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Persistent interaction delivery unavailable"))?;
        let message = store
            .cached_message_at(node, &self.id, &id, self.cache_generation)?
            .ok_or_else(|| anyhow::anyhow!("Interaction request unavailable"))?;
        let TranscriptMessage::Message { metadata, .. } = &message;
        let request = interactions::request(metadata)
            .ok_or_else(|| anyhow::anyhow!("Unsupported interaction request"))?;
        anyhow::ensure!(
            interactions::MessageContent::parse(metadata.interaction.as_deref().unwrap())
                .is_some_and(|content| content.handler == interactions::AGENT_CONFIGURATION),
            "Unsupported Agent configuration card"
        );
        if metadata.interaction_result.is_some() || interactions::initial_result(metadata).is_some()
        {
            self.commit(|s| s.update_cached_interactions(std::slice::from_ref(&message)));
            return Ok(());
        }
        if matches!(&command, Command::Retry { .. }) {
            store.retry_configuration_submission(node, &self.id, &id, self.cache_generation)?;
        } else {
            let (accept, values) = match command {
                Command::Submit { values, .. } => {
                    match interactions::agent_configuration::validate(&request, &values) {
                        Ok(values) => (true, values),
                        Err(errors) => {
                            self.commit(|s| s.set_interaction_errors(&id, errors));
                            return Ok(());
                        }
                    }
                }
                Command::Decline { .. } => (false, Default::default()),
                Command::Retry { .. } => unreachable!(),
                Command::Activate { .. } => unreachable!(),
                Command::ContinueLogin { .. }
                | Command::CancelLogin { .. }
                | Command::LoginCallback { .. } => unreachable!(),
            };
            store.prepare_configuration_submission(
                node,
                &self.id,
                &id,
                Response {
                    response_id: ulid::Ulid::new().to_string(),
                    accept,
                    values,
                },
                self.cache_generation,
            )?;
        }
        self.commit(|s| s.set_interaction_errors(&id, Default::default()));
        self.sync_interactions();
        device.start_delivery();
        Ok(())
    }
}
