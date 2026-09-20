//! Transport-independent Agent operations and snapshot-based event observation.
use std::sync::Arc;
use zork_agent_api::{
    AgentProfile, ApiErrorCode, CreateSessionRequest, DurableEvent, EventQuery, ItemList,
    MailboxRequest, MessageKind, MessagePage, MessageQuery, PublicMessage, PublicRole,
    SessionSelection, SessionStatus, SessionSummary, SessionView,
};

use crate::session::events::{SessionEvent, ToolOutcome, TurnOutcome};
use crate::session::ports::ProfileResolver;
use crate::session::query::QueryError;
use crate::session::service::{LiveSessionEvent, SessionService};
use crate::session::state::SessionState;
use crate::session::store::EventEnvelope;
use crate::session::supervisor::{PublicSlotStatus, SupervisorError};

const DEFAULT_MESSAGE_LIMIT: usize = 50;
const MAX_MESSAGE_LIMIT: usize = 200;
const HISTORY_PAGE_SIZE: usize = 200;

#[derive(Clone)]
pub struct Agent {
    pub service: Arc<SessionService>,
    pub profiles: Arc<crate::ProfileStore>,
}

#[derive(Debug)]
pub struct AgentError {
    pub code: ApiErrorCode,
    pub message: String,
}

impl AgentError {
    pub fn invalid(message: impl Into<String>) -> Self {
        Self {
            code: ApiErrorCode::InvalidRequest,
            message: message.into(),
        }
    }

    fn internal() -> Self {
        Self {
            code: ApiErrorCode::InternalError,
            message: "internal error".into(),
        }
    }
}

impl From<SupervisorError> for AgentError {
    fn from(error: SupervisorError) -> Self {
        match error {
            SupervisorError::NotFound => Self {
                code: ApiErrorCode::SessionNotFound,
                message: error.to_string(),
            },
            SupervisorError::SessionOverloaded => Self {
                code: ApiErrorCode::SessionOverloaded,
                message: error.to_string(),
            },
            SupervisorError::GlobalOverloaded => Self {
                code: ApiErrorCode::GlobalOverloaded,
                message: error.to_string(),
            },
            SupervisorError::CircuitOpen(_) => Self {
                code: ApiErrorCode::RunnerCircuitOpen,
                message: error.to_string(),
            },
            SupervisorError::Deleting => Self {
                code: ApiErrorCode::SessionDeleting,
                message: error.to_string(),
            },
            SupervisorError::Unavailable(_)
            | SupervisorError::Runner(_)
            | SupervisorError::Store(_)
            | SupervisorError::Query(_) => Self {
                code: ApiErrorCode::SessionUnavailable,
                message: error.to_string(),
            },
        }
    }
}

impl std::fmt::Display for AgentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for AgentError {}

impl Agent {
    pub async fn create_session(
        &self,
        request: CreateSessionRequest,
    ) -> Result<SessionView, AgentError> {
        let selection = selection(request.profile_id, request.model, request.thinking)?;
        validate_selection(&self.profiles, &selection)?;
        let workspace = request
            .workspace
            .unwrap_or_else(|| std::env::temp_dir().to_string_lossy().into_owned());
        let session_id = self
            .service
            .create_session(selection, request.system_prompt, workspace, request.context)
            .await
            .map_err(AgentError::from)?;
        let session = self
            .service
            .state(&session_id)
            .await
            .map_err(AgentError::from)?;
        Ok(session_view(&session))
    }

    pub async fn ensure_session(
        &self,
        session_id: String,
        request: CreateSessionRequest,
    ) -> Result<SessionView, AgentError> {
        if session_id.len() != 26 || ulid::Ulid::from_string(&session_id).is_err() {
            return Err(AgentError::invalid("session_id must be a ULID"));
        }
        let selection = selection(request.profile_id, request.model, request.thinking)?;
        validate_selection(&self.profiles, &selection)?;
        let workspace = request
            .workspace
            .ok_or_else(|| AgentError::invalid("workspace required"))?;
        self.service
            .ensure_session(
                session_id.clone(),
                selection,
                request.system_prompt,
                workspace.clone(),
                request.context,
            )
            .await
            .map_err(AgentError::from)?;
        let session = self
            .service
            .state(&session_id)
            .await
            .map_err(AgentError::from)?;
        let view = session_view(&session);
        if view.workspace != workspace {
            return Err(AgentError::invalid(
                "session already belongs to another workspace",
            ));
        }
        Ok(view)
    }

    pub async fn append_mailbox_id(
        &self,
        session_id: String,
        request_id: String,
        request: MailboxRequest,
    ) -> Result<(), AgentError> {
        if request_id.is_empty()
            || request_id.len() > 160
            || !request_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err(AgentError::invalid("invalid request_id"));
        }
        if request.content.trim().is_empty() {
            return Err(AgentError::invalid("content required"));
        }
        self.service
            .submit_input_id(&session_id, request_id, request.content)
            .await
            .map_err(AgentError::from)?;
        Ok(())
    }

    /// Ordered sources retain a bounded per-source watermark in the durable
    /// snapshot. Replayed input cannot run twice after intervening input or a
    /// restart. Quiet input is consumed at the next ordinary model boundary.
    pub async fn append_ordered_mailbox(
        &self,
        session_id: String,
        source: String,
        sequence: u64,
        content: String,
        wake: bool,
    ) -> Result<(), AgentError> {
        if source.is_empty() || source.len() > 160 || sequence == 0 || content.trim().is_empty() {
            return Err(AgentError::invalid("invalid ordered mailbox input"));
        }
        self.service
            .submit_ordered_input(
                &session_id,
                crate::session::events::InputPosition { source, sequence },
                wake,
                content,
            )
            .await
            .map_err(AgentError::from)
    }

    pub async fn list_sessions(&self) -> ItemList<SessionSummary> {
        let items = self
            .service
            .sessions()
            .into_iter()
            .map(|slot| SessionSummary {
                session_id: slot.session_id,
                status: slot_status(slot.status, slot.finished, slot.last_turn_outcome),
            })
            .collect::<Vec<_>>();
        ItemList { items }
    }

    pub async fn get_session(&self, session_id: String) -> Result<SessionView, AgentError> {
        let session = self
            .service
            .state(&session_id)
            .await
            .map_err(AgentError::from)?;
        Ok(session_view(&session))
    }

    pub async fn session_snapshot(
        &self,
        session_id: &str,
    ) -> Result<zork_agent_api::SessionSnapshot, AgentError> {
        let current = self
            .service
            .overview(session_id)
            .await
            .map_err(AgentError::from)?;
        self.snapshot_from(current).await
    }

    pub async fn snapshot_from(
        &self,
        current: crate::session::state::OverviewProjection,
    ) -> Result<zork_agent_api::SessionSnapshot, AgentError> {
        let profiles = self.profiles.clone();
        tokio::task::spawn_blocking(move || {
            let selection = current.selection.as_ref();
            zork_agent_api::SessionSnapshot {
                session_id: current.session_id,
                cursor: current.cursor,
                server_time_ms: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as i64,
                runtime: zork_agent_api::HistoryRuntime {
                    model: selection.map(|s| s.model.clone()),
                    thinking: selection.map(|s| s.thinking.clone()),
                    context_tokens: current.context_tokens,
                    context_limit: selection
                        .and_then(|s| profiles.model_limits(s).ok())
                        .map(|limits| limits.context_window_tokens),
                    profile: current
                        .resolved_profile
                        .as_deref()
                        .and_then(|id| profiles.get(id).ok().flatten()),
                },
                aggregates: current.aggregates,
                execution: current.execution,
            }
        })
        .await
        .map_err(|_| AgentError::internal())
    }

    pub async fn delete_session(&self, session_id: String) -> Result<(), AgentError> {
        self.service
            .delete(&session_id)
            .await
            .map_err(AgentError::from)?;
        Ok(())
    }

    pub async fn append_mailbox(
        &self,
        session_id: String,
        request: MailboxRequest,
    ) -> Result<(), AgentError> {
        if request.content.is_empty() {
            return Err(AgentError::invalid("content is required"));
        }
        self.service
            .submit_input(&session_id, request.content)
            .await
            .map_err(AgentError::from)?;
        Ok(())
    }

    pub async fn cancel_session(&self, session_id: String) -> Result<(), AgentError> {
        self.service
            .cancel(&session_id)
            .await
            .map_err(AgentError::from)?;
        Ok(())
    }

    pub async fn set_selection(
        &self,
        session_id: String,
        body: SessionSelection,
    ) -> Result<SessionView, AgentError> {
        let selection = selection(body.profile_id, body.model, body.thinking)?;
        validate_selection(&self.profiles, &selection)?;
        self.service
            .set_selection(&session_id, selection.clone())
            .await
            .map_err(AgentError::from)?;
        let session = self
            .service
            .state(&session_id)
            .await
            .map_err(AgentError::from)?;
        Ok(session_view(&session))
    }

    pub async fn set_context(
        &self,
        session_id: String,
        config: zork_agent_api::ContextConfig,
    ) -> Result<SessionView, AgentError> {
        self.service
            .set_context(&session_id, config)
            .await
            .map_err(AgentError::from)?;
        let session = self
            .service
            .state(&session_id)
            .await
            .map_err(AgentError::from)?;
        Ok(session_view(&session))
    }

    pub async fn list_messages(
        &self,
        session_id: String,
        query: MessageQuery,
    ) -> Result<MessagePage, AgentError> {
        if !self.service.contains(&session_id) {
            return Err(SupervisorError::NotFound.into());
        }
        let limit = query
            .limit
            .unwrap_or(DEFAULT_MESSAGE_LIMIT)
            .min(MAX_MESSAGE_LIMIT);
        if limit == 0 {
            return Ok(MessagePage {
                items: Vec::new(),
                older_cursor: None,
            });
        }

        let mut cursor = query.before;
        let mut messages = Vec::with_capacity(limit);
        while messages.len() < limit {
            let page = self
                .service
                .history_before(&session_id, cursor.as_deref(), HISTORY_PAGE_SIZE)
                .map_err(query_error)?;
            let Some(first) = page.first() else {
                break;
            };
            let next_cursor = first.event_id.clone();
            for envelope in page.iter().rev() {
                if let Some(message) = public_message(&envelope.event) {
                    messages.push((envelope.event_id.clone(), message));
                    if messages.len() == limit {
                        break;
                    }
                }
            }
            if messages.len() == limit || page.len() < HISTORY_PAGE_SIZE {
                break;
            }
            cursor = Some(next_cursor);
        }
        let older_cursor = (messages.len() == limit)
            .then(|| messages.last().map(|(event_id, _)| event_id.clone()))
            .flatten();
        messages.reverse();
        Ok(MessagePage {
            items: messages.into_iter().map(|(_, message)| message).collect(),
            older_cursor,
        })
    }

    pub async fn list_history(
        &self,
        session_id: String,
        query: zork_agent_api::HistoryQuery,
    ) -> Result<zork_agent_api::HistoryPage<SessionEvent>, AgentError> {
        if query.before.is_some() && query.after.is_some() {
            return Err(AgentError::invalid(
                "before and after are mutually exclusive",
            ));
        }
        if !self.service.contains(&session_id) {
            return Err(SupervisorError::NotFound.into());
        }
        let limit = query.limit.unwrap_or(100).clamp(1, 200);
        let service = self.service.clone();
        let forward = query.after.is_some();
        let mut events = tokio::task::spawn_blocking(move || {
            if forward {
                service.history_after(&session_id, query.after.as_deref(), limit + 1)
            } else {
                service.history_before(&session_id, query.before.as_deref(), limit + 1)
            }
        })
        .await
        .map_err(|_| AgentError::invalid("history query interrupted"))?
        .map_err(query_error)?;
        let has_more = events.len() > limit;
        if has_more {
            if forward {
                events.truncate(limit);
            } else {
                events.remove(0);
            }
        }
        Ok(zork_agent_api::HistoryPage {
            server_time_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as i64,
            older_cursor: if !forward && has_more {
                events.first().map(|e| e.event_id.clone())
            } else {
                None
            },
            latest_cursor: events.last().map(|e| e.event_id.clone()),
            has_more,
            items: events
                .into_iter()
                .map(|e| zork_agent_api::DurableEvent {
                    event_id: e.event_id,
                    schema_version: e.schema_version,
                    batch_index: e.batch_index,
                    batch_count: e.batch_count,
                    event: e.event,
                })
                .collect(),
        })
    }

    pub async fn list_profiles(&self) -> Result<ItemList<AgentProfile>, AgentError> {
        let profiles = self.profiles.clone();
        let items = tokio::task::spawn_blocking(move || profiles.list())
            .await
            .map_err(|_| AgentError::internal())?
            .map_err(|_| AgentError::internal())?;
        Ok(ItemList { items })
    }

    pub async fn get_profile(
        &self,
        profile_id: String,
    ) -> Result<zork_profile::ProfileView, AgentError> {
        let profiles = self.profiles.clone();
        let profile = tokio::task::spawn_blocking(move || profiles.get(&profile_id))
            .await
            .map_err(|_| AgentError::internal())?
            .map_err(|_| AgentError::internal())?
            .ok_or_else(|| AgentError {
                code: ApiErrorCode::ProfileNotFound,
                message: "profile not found".into(),
            })?;
        Ok(profile)
    }

    pub async fn refresh_profile(
        &self,
        profile_id: String,
    ) -> Result<zork_profile::ProfileView, AgentError> {
        self.get_profile(profile_id.clone()).await?;
        self.profiles
            .refresh_status(&profile_id)
            .await
            .map_err(|_| AgentError::internal())?;
        self.get_profile(profile_id).await
    }

    pub async fn rename_profile(
        &self,
        profile_id: String,
        name: String,
    ) -> Result<zork_profile::ProfileView, AgentError> {
        self.get_profile(profile_id.clone()).await?;
        let profiles = self.profiles.clone();
        tokio::task::spawn_blocking(move || profiles.update_name(&profile_id, &name))
            .await
            .map_err(|_| AgentError::internal())?
            .map_err(|_| AgentError::invalid("Profile name could not be updated"))
    }
    pub async fn put_profile(
        &self,
        profile_id: String,
        body: zork_agent_api::ProfileDocument,
    ) -> Result<zork_profile::ProfileView, AgentError> {
        let body = serde_json::to_value(body).map_err(|_| AgentError::internal())?;
        let profiles = self.profiles.clone();
        let write_id = profile_id.clone();
        tokio::task::spawn_blocking(move || profiles.put(&write_id, body))
            .await
            .map_err(|_| AgentError::internal())?
            .map_err(|error| AgentError::invalid(error.to_string()))?;
        self.profiles
            .refresh_status(&profile_id)
            .await
            .map_err(|_| AgentError::internal())?;
        let profiles = self.profiles.clone();
        let profile = tokio::task::spawn_blocking(move || profiles.get(&profile_id))
            .await
            .map_err(|_| AgentError::internal())?
            .map_err(|_| AgentError::internal())?
            .ok_or_else(AgentError::internal)?;
        Ok(profile)
    }

    pub async fn delete_profile(&self, profile_id: String) -> Result<(), AgentError> {
        let profiles = self.profiles.clone();
        tokio::task::spawn_blocking(move || profiles.delete(&profile_id))
            .await
            .map_err(|_| AgentError::internal())?
            .map_err(|_| AgentError::internal())?;
        Ok(())
    }

    pub async fn discover_models(
        &self,
        profile_id: String,
    ) -> Result<zork_profile::ModelDiscovery, AgentError> {
        self.profiles
            .discover_models(&profile_id)
            .await
            .map_err(|_| {
                AgentError::invalid(
                    "Could not retrieve models; check the connection or add a model manually",
                )
            })
    }

    pub async fn update_profile_models(
        &self,
        profile_id: String,
        body: UpdateProfileModels,
    ) -> Result<zork_profile::ProfileView, AgentError> {
        let profiles = self.profiles.clone();
        let profile = tokio::task::spawn_blocking(move || {
            profiles.update_models(&profile_id, body.models, body.expected_models)
        })
        .await
        .map_err(|_| AgentError::internal())?
        .map_err(|_| AgentError::invalid("profile models could not be updated"))?;
        Ok(profile)
    }

    pub async fn refresh_profile_models(
        &self,
        profile_id: String,
    ) -> Result<zork_profile::ModelUpdate, AgentError> {
        self.get_profile(profile_id.clone()).await?;
        self.profiles
            .refresh_models(&profile_id)
            .await
            .map_err(|error| AgentError::invalid(error.to_string()))
    }

    pub async fn set_profile_model_enabled(
        &self,
        profile_id: String,
        model_id: String,
        enabled: bool,
    ) -> Result<(zork_profile::ProfileView, bool), AgentError> {
        self.get_profile(profile_id.clone()).await?;
        let profiles = self.profiles.clone();
        tokio::task::spawn_blocking(move || {
            profiles.set_model_enabled(&profile_id, &model_id, enabled)
        })
        .await
        .map_err(|_| AgentError::internal())?
        .map_err(|error| AgentError::invalid(error.to_string()))
    }

    /// Every connection starts with a current snapshot, followed only by
    /// subsequent events. Lag establishes a fresh baseline; history reads are
    /// explicit and never an SSE initialization or recovery fallback.
    pub async fn events(
        &self,
        session_id: String,
        _cursor: Option<String>,
        query: EventQuery,
    ) -> Result<
        impl futures_util::Stream<Item = Result<LiveSessionEvent, AgentError>> + Send + 'static,
        AgentError,
    > {
        let mut live = self
            .service
            .subscribe(&session_id)
            .map_err(AgentError::from)?;
        let snapshot = self.session_snapshot(&session_id).await?;
        let agent = self.clone();
        Ok(async_stream::stream! {
            let mut last = snapshot.cursor.clone();
            let mut overview_at = snapshot.cursor.clone();
            yield Ok(LiveSessionEvent::Snapshot(Arc::new(snapshot)));
            loop {
                match live.recv().await {
                    Ok(LiveSessionEvent::Durable(envelope)) => {
                        if last.as_ref().is_some_and(|cursor| envelope.event_id <= *cursor) { continue; }
                        last = Some(envelope.event_id.clone());
                        yield Ok(LiveSessionEvent::Durable(envelope));
                    }
                    Ok(LiveSessionEvent::Overview(overview)) => {
                        if overview_at.as_ref().is_some_and(|cursor| overview.cursor.as_ref().is_some_and(|next| next <= cursor)) { continue; }
                        overview_at = overview.cursor.clone();
                        yield Ok(LiveSessionEvent::Overview(overview));
                    }
                    Ok(event @ (LiveSessionEvent::TextDelta { .. } | LiveSessionEvent::OutputDelta { .. })) if query.transient => { yield Ok(event); }
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        live = match agent.service.subscribe(&session_id) { Ok(live) => live, Err(error) => { yield Err(error.into()); return; } };
                        let snapshot = match agent.session_snapshot(&session_id).await { Ok(snapshot) => snapshot, Err(error) => { yield Err(error); return; } };
                        last = snapshot.cursor.clone();
                        overview_at = snapshot.cursor.clone();
                        yield Ok(LiveSessionEvent::Snapshot(Arc::new(snapshot)));
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                }
            }
        })
    }
}

fn public_message(event: &SessionEvent) -> Option<PublicMessage> {
    match event {
        SessionEvent::InputAppended { input } => Some(PublicMessage {
            kind: MessageKind::Message,
            role: PublicRole::Mailbox,
            content: input.content.clone(),
        }),
        SessionEvent::StepCompleted {
            assistant_text,
            purpose,
            ..
        } if *purpose == crate::session::events::Purpose::Conversation
            && !assistant_text.is_empty() =>
        {
            Some(PublicMessage {
                kind: MessageKind::Message,
                role: PublicRole::Assistant,
                content: assistant_text.clone(),
            })
        }
        SessionEvent::ToolResult { result } => Some(PublicMessage {
            kind: MessageKind::Message,
            role: PublicRole::Tool,
            content: render_tool_data(result.outcome, &result.data),
        }),
        _ => None,
    }
}

fn render_tool_data(outcome: ToolOutcome, data: &serde_json::Value) -> String {
    let content = data
        .as_str()
        .map(ToOwned::to_owned)
        .or_else(|| serde_json::to_string(data).ok())
        .unwrap_or_else(|| "null".into());
    if outcome == ToolOutcome::Succeeded {
        content
    } else {
        format!("{outcome:?}: {content}")
    }
}

fn session_view(state: &SessionState) -> SessionView {
    let selection = state.selection.as_ref();
    SessionView {
        session_id: state.session_id.clone(),
        profile_id: selection
            .map(|value| value.profile_id.clone())
            .unwrap_or_default(),
        model: selection
            .map(|value| value.model.clone())
            .unwrap_or_default(),
        thinking: selection
            .map(|value| value.thinking.clone())
            .unwrap_or_default(),
        workspace: state.workspace.clone(),
        generation: state.generation.number,
        context: state.context_config.clone(),
        status: state_status(state),
    }
}

fn state_status(state: &SessionState) -> SessionStatus {
    state.session_status()
}

fn slot_status(
    status: PublicSlotStatus,
    finished: bool,
    outcome: Option<TurnOutcome>,
) -> SessionStatus {
    match status {
        PublicSlotStatus::Active => SessionStatus::Working,
        PublicSlotStatus::Idle if finished => SessionStatus::Finished,
        PublicSlotStatus::Idle => match outcome {
            Some(TurnOutcome::Failed) => SessionStatus::Failed,
            Some(TurnOutcome::Cancelled) => SessionStatus::Cancelled,
            _ => SessionStatus::Wait,
        },
        PublicSlotStatus::CircuitOpen => SessionStatus::Failed,
        PublicSlotStatus::Unavailable => SessionStatus::Unavailable,
        PublicSlotStatus::Deleting => SessionStatus::Deleting,
        PublicSlotStatus::Unchecked | PublicSlotStatus::Recovering => SessionStatus::Recovering,
    }
}

pub fn durable_event(envelope: &EventEnvelope) -> DurableEvent<&SessionEvent> {
    DurableEvent {
        event_id: envelope.event_id.clone(),
        schema_version: envelope.schema_version,
        batch_index: envelope.batch_index,
        batch_count: envelope.batch_count,
        event: &envelope.event,
    }
}

fn selection(
    profile_id: String,
    model: String,
    thinking: String,
) -> Result<crate::session::events::Selection, AgentError> {
    let profile_id = match profile_id.trim() {
        "" => "auto",
        id => id,
    }
    .to_owned();
    let model = model.trim().to_owned();
    let thinking = thinking.trim().to_owned();
    if model.is_empty() || thinking.is_empty() {
        return Err(AgentError::invalid("model and thinking are required"));
    }
    Ok(crate::session::events::Selection {
        profile_id,
        model,
        thinking,
    })
}

fn validate_selection(
    profiles: &crate::ProfileStore,
    selection: &crate::session::events::Selection,
) -> Result<(), AgentError> {
    match profiles.model_limits(selection) {
        Ok(_) => Ok(()),
        _ => Err(AgentError {
            code: ApiErrorCode::SelectionUnavailable,
            message: format!(
                "profile {} cannot resolve {}/{}",
                selection.profile_id, selection.model, selection.thinking
            ),
        }),
    }
}

fn query_error(error: QueryError) -> AgentError {
    match error {
        QueryError::InvalidSessionId(_) => SupervisorError::NotFound.into(),
        QueryError::InvalidCursor(_) | QueryError::UnknownCursor(_) => AgentError {
            code: ApiErrorCode::InvalidCursor,
            message: error.to_string(),
        },
        QueryError::Io(ref io) if io.kind() == std::io::ErrorKind::NotFound => {
            SupervisorError::NotFound.into()
        }
        _ => AgentError::internal(),
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateProfileModels {
    pub models: Vec<zork_profile::ProfileModel>,
    #[serde(default)]
    pub expected_models: Option<Vec<zork_profile::ProfileModel>>,
}
