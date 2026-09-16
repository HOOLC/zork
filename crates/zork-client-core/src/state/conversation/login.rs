//! Private login material stays in the shared core's transient projection.
//! Provider progression lives on the node; UI closure does not stop it.
use super::*;
use crate::interactions::{Command, LoginView, Outcome};

impl Conversation {
    pub fn interaction_open_url(&self, message: &str, action: &str) -> Option<String> {
        let owned = self.owned.lock().unwrap();
        let index = owned.index_of(message)?;
        let crate::transcript::TranscriptLine::Message { metadata, .. } = &owned.lines[index];
        metadata
            .interaction_view
            .as_ref()?
            .actions
            .iter()
            .find(|a| a.id == action)?
            .open_url
            .clone()
    }

    pub(super) fn start_login_view(self: &Arc<Self>, id: String) {
        if !self.login_tasks.lock().unwrap().insert(id.clone()) {
            return;
        }
        let conversation = self.clone();
        self.client.spawn(async move {
            let result = conversation.follow_login(&id).await;
            conversation.login_tasks.lock().unwrap().remove(&id);
            match result {
                Ok(()) => conversation.commit(|s| s.set_login_view(&id, None)),
                Err(error) => conversation.commit(|s| {
                    s.set_login_view(
                        &id,
                        Some(LoginView {
                            error: Some(error.to_string()),
                            ..Default::default()
                        }),
                    )
                }),
            }
        });
    }

    pub(crate) fn login_action(self: &Arc<Self>, command: Command) -> anyhow::Result<()> {
        let id = command.message_id().to_owned();
        let device = self
            .device
            .upgrade()
            .ok_or_else(|| anyhow::anyhow!("Device unavailable"))?;
        let (store, node) = device
            .cache
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Login card unavailable"))?;
        let message = store
            .cached_message_at(node, &self.id, &id, self.cache_generation)?
            .ok_or_else(|| anyhow::anyhow!("Login card unavailable"))?;
        let TranscriptMessage::Message { metadata, .. } = message;
        let initial = crate::interactions::initial_result(&metadata);
        if metadata
            .interaction_result
            .as_deref()
            .or(initial.as_ref())
            .is_some_and(|result| result.outcome.terminal())
        {
            return Ok(());
        }
        if matches!(command, Command::ContinueLogin { .. }) {
            self.start_login_view(id);
            return Ok(());
        }
        let (method, body) = match command {
            Command::CancelLogin { .. } => (http::Method::DELETE, None),
            Command::LoginCallback { callback, .. } => {
                anyhow::ensure!(
                    !callback.trim().is_empty() && callback.len() <= 8192,
                    "Enter the browser callback to continue"
                );
                (
                    http::Method::POST,
                    Some(serde_json::json!({"callback":callback})),
                )
            }
            _ => unreachable!(),
        };
        self.commit(|s| {
            let mut view = s.login_views.get(&id).cloned().unwrap_or_default();
            view.busy = true;
            view.error = None;
            s.set_login_view(&id, Some(view));
        });
        let conversation = self.clone();
        self.client.spawn(async move {
            let result = conversation
                .client
                .node_request(
                    method,
                    format!(
                        "/v1/node/chats/{}/messages/{id}/provider-login",
                        conversation.id
                    ),
                    body,
                )
                .await;
            conversation.commit(|s| {
                let mut view = s.login_views.get(&id).cloned().unwrap_or_default();
                view.busy = false;
                view.error = result.err().map(|e| e.to_string());
                s.set_login_view(&id, Some(view));
            });
            conversation.start_login_view(id);
        });
        Ok(())
    }

    async fn follow_login(&self, id: &str) -> anyhow::Result<()> {
        let mut changes = self.subscribe();
        let mut fetched = false;
        loop {
            let snapshot = changes.snapshot().state;
            if snapshot.revoked {
                return Ok(());
            }
            let (outcome, rejected) = {
                let owned = self.owned.lock().unwrap();
                let Some(index) = owned.index_of(id) else {
                    return Ok(());
                };
                let crate::transcript::TranscriptLine::Message { metadata, .. } =
                    &owned.lines[index];
                let outcome = metadata
                    .interaction_result
                    .as_deref()
                    .or(crate::interactions::initial_result(metadata).as_ref())
                    .map(|r| r.outcome);
                (
                    outcome,
                    owned
                        .interaction_submissions
                        .get(id)
                        .is_some_and(|s| s.rejected),
                )
            };
            if outcome.is_some_and(Outcome::terminal) || rejected {
                return Ok(());
            }
            if outcome == Some(Outcome::Pending) && !fetched {
                let value = self
                    .client
                    .node_request(
                        http::Method::GET,
                        format!("/v1/node/chats/{}/messages/{id}/provider-login", self.id),
                        None,
                    )
                    .await?;
                if let Some(auth) = value.get("authorization") {
                    let url =
                        reqwest::Url::parse(auth["verification_url"].as_str().unwrap_or_default())
                            .map_err(|_| anyhow::anyhow!("Sign-in link unavailable"))?;
                    anyhow::ensure!(
                        matches!(url.scheme(), "https" | "http"),
                        "Unsupported sign-in link"
                    );
                    let view = LoginView {
                        open_url: Some(url.into()),
                        user_code: auth["user_code"]
                            .as_str()
                            .filter(|s| !s.is_empty())
                            .map(str::to_owned),
                        callback: auth["flow"] == "browser_callback",
                        busy: false,
                        error: None,
                    };
                    self.commit(|s| s.set_login_view(id, Some(view)));
                    fetched = true;
                }
            }
            if changes.changed().await.is_none() {
                return Ok(());
            }
        }
    }
}
