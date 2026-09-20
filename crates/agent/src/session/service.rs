//! Thin application facade over supervisor, query and live observation.

use std::sync::Arc;
use std::time::Duration;

use super::compression::SegmentCompressor;
use super::deadline::DeadlineScheduler;
use super::events::Selection;
use super::executor::ToolExecutor;
use super::model::ModelGateway;
use super::ports::{Clock, IdGenerator};
use super::query::{QueryError, SessionQuery};
use super::runner::{RunnerDependencies, RunnerObserver, RunnerOptions};
use super::state::SessionState;
use super::store::{EventEnvelope, SessionStore};
use super::supervisor::{SessionSlotView, SessionSupervisor, SupervisorError, SupervisorOptions};
use super::tools::ToolRegistry;

#[derive(Clone)]
pub struct ServiceOptions {
    pub runner: RunnerOptions,
    pub supervisor: SupervisorOptions,
    pub tool_timeout: Option<Duration>,
    pub deadline_command_capacity: usize,
    pub deadline_wake_capacity: usize,
    pub live_event_capacity: usize,
    pub compression_retry_interval: Duration,
}

impl Default for ServiceOptions {
    fn default() -> Self {
        Self {
            runner: RunnerOptions::default(),
            supervisor: SupervisorOptions::default(),
            tool_timeout: None,
            deadline_command_capacity: 4096,
            deadline_wake_capacity: 4096,
            live_event_capacity: 256,
            compression_retry_interval: Duration::from_secs(30),
        }
    }
}

pub struct SessionService {
    supervisor: Arc<SessionSupervisor>,
    query: Arc<dyn SessionQuery>,
    deadlines: DeadlineScheduler,
    live: Arc<LiveEventHub>,
    compressor: SegmentCompressor,
}

pub struct ServiceDependencies {
    pub store: Arc<dyn SessionStore>,
    pub query: Arc<dyn SessionQuery>,
    pub model: Arc<dyn ModelGateway>,
    pub tools: Arc<ToolRegistry>,
    pub clock: Arc<dyn Clock>,
    pub ids: Arc<dyn IdGenerator>,
}

impl SessionService {
    pub fn start(dependencies: ServiceDependencies, options: ServiceOptions) -> Arc<Self> {
        let query = dependencies.query.clone();
        let compressor = SegmentCompressor::start(
            dependencies.store.clone(),
            options.compression_retry_interval,
        );
        let live = Arc::new(LiveEventHub::new(options.live_event_capacity));
        let executor = Arc::new(ToolExecutor::with_clock(
            dependencies.tools.clone(),
            dependencies.clock.clone(),
            options.tool_timeout,
        ));
        let (deadlines, deadline_wakes) = DeadlineScheduler::start(
            dependencies.clock.clone(),
            options.deadline_command_capacity,
            options.deadline_wake_capacity,
        );
        let supervisor = SessionSupervisor::start(
            RunnerDependencies {
                store: dependencies.store,
                model: dependencies.model,
                tools: dependencies.tools,
                executor,
                deadlines: deadlines.clone(),
                clock: dependencies.clock,
                ids: dependencies.ids,
                observer: live.clone(),
                compressor: compressor.clone(),
                options: options.runner,
            },
            query.clone(),
            deadline_wakes,
            options.supervisor,
        );
        Arc::new(Self {
            supervisor,
            query,
            deadlines,
            live,
            compressor,
        })
    }

    pub async fn create_session(
        &self,
        selection: Selection,
        system_prompt: Option<String>,
        workspace: String,
        context: Option<zork_config::ContextConfig>,
    ) -> Result<String, SupervisorError> {
        self.supervisor
            .create_session(selection, system_prompt, workspace, context)
            .await
    }

    pub async fn ensure_session(
        &self,
        session_id: String,
        selection: Selection,
        system_prompt: Option<String>,
        workspace: String,
        context: Option<zork_config::ContextConfig>,
    ) -> Result<String, SupervisorError> {
        self.supervisor
            .ensure_session(session_id, selection, system_prompt, workspace, context)
            .await
    }

    pub async fn submit_input(
        &self,
        session_id: &str,
        content: String,
    ) -> Result<(), SupervisorError> {
        self.supervisor.submit_input(session_id, content).await
    }

    pub async fn submit_input_id(
        &self,
        session_id: &str,
        request_id: String,
        content: String,
    ) -> Result<(), SupervisorError> {
        self.supervisor
            .submit_input_id(session_id, Some(request_id), content)
            .await
    }

    pub async fn submit_ordered_input(
        &self,
        session_id: &str,
        position: super::events::InputPosition,
        wake: bool,
        content: String,
    ) -> Result<(), SupervisorError> {
        self.supervisor
            .submit_input_delivery(session_id, None, Some(position), wake, content)
            .await
    }

    pub async fn set_selection(
        &self,
        session_id: &str,
        selection: Selection,
    ) -> Result<(), SupervisorError> {
        self.supervisor.set_selection(session_id, selection).await
    }

    pub async fn cancel(&self, session_id: &str) -> Result<(), SupervisorError> {
        self.supervisor.cancel_turn(session_id).await
    }

    pub async fn cancel_observed_turn(
        &self,
        session_id: &str,
        turn_id: String,
    ) -> Result<(), SupervisorError> {
        self.supervisor
            .cancel_observed_turn(session_id, Some(turn_id))
            .await
    }

    pub async fn set_context(
        &self,
        session_id: &str,
        config: zork_config::ContextConfig,
    ) -> Result<(), SupervisorError> {
        self.supervisor.set_context(session_id, config).await
    }

    pub async fn delete(&self, session_id: &str) -> Result<(), SupervisorError> {
        self.supervisor.delete(session_id).await
    }

    pub async fn state(&self, session_id: &str) -> Result<SessionState, SupervisorError> {
        self.supervisor.inspect(session_id).await
    }

    pub async fn overview(
        &self,
        session_id: &str,
    ) -> Result<super::state::OverviewProjection, SupervisorError> {
        self.supervisor.inspect_overview(session_id).await
    }

    pub fn sessions(&self) -> Vec<SessionSlotView> {
        self.supervisor.list()
    }

    pub fn contains(&self, session_id: &str) -> bool {
        self.supervisor.contains(session_id)
    }

    pub fn history_after(
        &self,
        session_id: &str,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<Vec<EventEnvelope>, QueryError> {
        self.query.after(session_id, cursor, limit)
    }

    pub fn history_before(
        &self,
        session_id: &str,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<Vec<EventEnvelope>, QueryError> {
        self.query.before(session_id, cursor, limit)
    }

    pub fn subscribe(&self, session_id: &str) -> Result<SessionSubscription, SupervisorError> {
        if !self.contains(session_id) {
            return Err(SupervisorError::NotFound);
        }
        Ok(self.live.subscribe(session_id))
    }

    pub async fn shutdown(&self) {
        self.supervisor.shutdown().await;
        self.deadlines.shutdown().await;
        self.supervisor.join_deadline_wakes().await;
        self.compressor.shutdown().await;
    }
}

#[derive(Clone, Debug)]
pub enum LiveSessionEvent {
    Snapshot(Arc<zork_agent_api::SessionSnapshot>),
    Overview(Arc<super::state::OverviewProjection>),
    OutputDelta {
        session_id: String,
        generation: u64,
        step_id: String,
        bytes: u64,
    },
    Durable(Arc<EventEnvelope>),
    TextDelta {
        session_id: String,
        generation: u64,
        step_id: String,
        text: String,
    },
}

pub type SessionSubscription = zork_notify::events::Events<LiveSessionEvent>;

struct LiveEventHub(zork_notify::events::EventHub<String, LiveSessionEvent>);
impl LiveEventHub {
    fn new(capacity: usize) -> Self {
        Self(zork_notify::events::EventHub::new(capacity.max(1)))
    }
    fn subscribe(&self, session_id: &str) -> SessionSubscription {
        self.0.subscribe(session_id.to_owned())
    }
}

impl RunnerObserver for LiveEventHub {
    fn state_changed(&self, state: &SessionState) {
        self.0.publish_with(&state.session_id, || {
            LiveSessionEvent::Overview(Arc::new(state.overview()))
        });
    }
    fn persisted(&self, session_id: &str, events: &[EventEnvelope]) {
        for event in events {
            if event.event.is_history_visible() {
                self.0.publish_with(session_id, || {
                    LiveSessionEvent::Durable(Arc::new(event.clone()))
                });
            }
        }
    }

    fn output_delta(&self, session_id: &str, generation: u64, step_id: &str, bytes: u64) {
        self.0
            .publish_with(session_id, || LiveSessionEvent::OutputDelta {
                session_id: session_id.to_owned(),
                generation,
                step_id: step_id.to_owned(),
                bytes,
            });
    }

    fn text_delta(&self, session_id: &str, generation: u64, step_id: &str, text: &str) {
        self.0
            .publish_with(session_id, || LiveSessionEvent::TextDelta {
                session_id: session_id.to_owned(),
                generation,
                step_id: step_id.to_owned(),
                text: text.to_owned(),
            });
    }
}
