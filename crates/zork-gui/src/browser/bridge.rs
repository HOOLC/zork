//! The UI only supplies the core browser host and observes the grant.
use super::worker::Worker;
use crate::api::StationClient;
use std::sync::Arc;
pub struct Grant(zork_client_core::desktop::browser::Grant);
impl std::ops::Deref for Grant {
    type Target = zork_client_core::desktop::browser::Grant;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl Grant {
    pub fn start(
        worker: Worker,
        client: Arc<StationClient>,
        session: String,
        host: String,
        _cx: &mut gpui::App,
    ) -> Self {
        Self(zork_client_core::desktop::browser::Grant::start(
            worker.0, client, session, host,
        ))
    }
}
