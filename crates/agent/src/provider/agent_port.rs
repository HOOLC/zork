//! ModelGateway composition: profile resolution plus the selected provider.

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use zork_agent::session::model::{
    ModelError, ModelGateway, ModelOutcome, ModelReleaseSuggestion, ModelRequest,
    ModelStreamObserver,
};
use zork_agent::session::ports::{ModelExecutor, ProfileResolveError, ProfileResolver};
use zork_agent::ProfileStore;

pub struct AgentModelPort {
    router: Arc<dyn ModelExecutor>,
    profiles: Arc<ProfileStore>,
}

impl AgentModelPort {
    pub fn new(router: Arc<dyn ModelExecutor>, profiles: Arc<ProfileStore>) -> Self {
        Self { router, profiles }
    }

    async fn complete_auto(&self, request: &ModelRequest) -> Result<ModelOutcome, ModelError> {
        let expected = self
            .profiles
            .model_limits(&request.selection)
            .map_err(resolve_error)?;
        if request.max_output_tokens != Some(expected.max_output_tokens) {
            return Err(ModelError::InvalidSelection);
        }
        let preferred = request
            .transcript
            .iter()
            .rev()
            .filter_map(|m| m.provider_context.as_ref())
            .find(|c| c.model == request.selection.model)
            .map(|c| c.profile_id.as_str());
        let observer = Arc::new(ObservedStream {
            inner: request.stream_observer.clone(),
            emitted: AtomicBool::new(false),
        });
        // Each rejection quarantines its account, so attempts are bounded by
        // the initial catalog size even if another task edits profiles meanwhile.
        let attempts = self
            .profiles
            .list()
            .map_err(|_| ModelError::Unavailable)?
            .len();
        let mut last_error = None;
        for _ in 0..attempts {
            let lease = match self.profiles.acquire_account(
                &request.selection,
                preferred,
                request.transcript.iter().any(|m| !m.images.is_empty()),
            ) {
                Ok(lease) => lease,
                Err(error) => return Err(last_error.unwrap_or_else(|| resolve_error(error))),
            };
            let mut selection = request.selection.clone();
            selection.profile_id = lease.profile_id.clone();
            let execution = match self.profiles.resolve(&selection).await {
                Ok(execution) => execution,
                Err(error) => {
                    lease.reject(crate::profiles::unix_now());
                    last_error = Some(resolve_error(error));
                    continue;
                }
            };
            if expected.max_output_tokens > execution.limits().max_output_tokens
                || expected.context_window_tokens > execution.limits().context_window_tokens
            {
                return Err(ModelError::InvalidSelection);
            }
            let identity = crate::session::wire::ProviderContext {
                profile_id: execution.profile_id().to_owned(),
                provider: execution.provider().to_owned(),
                model: execution.model().to_owned(),
                api: execution.api().to_owned(),
                output_items: Arc::new(Vec::new()),
            };
            let actual = ModelRequest {
                session_id: request.session_id.clone(),
                generation: request.generation,
                step_id: request.step_id.clone(),
                selection,
                transcript: request.transcript.clone(),
                tools: request.tools.clone(),
                max_output_tokens: request.max_output_tokens,
                independent: request.independent,
                stream_observer: observer.clone(),
            };
            match self.router.complete(&actual, execution).await {
                Ok(mut outcome) => {
                    // Also record actual identity for protocols without opaque
                    // continuation, so automatic affinity survives restarts.
                    if outcome.provider_context.is_none() {
                        outcome.provider_context = Some(identity);
                    }
                    return Ok(outcome);
                }
                Err(error) if account_rejected(&error) => {
                    lease.reject(crate::profiles::unix_now());
                    self.router.release_session(&request.session_id);
                    if observer.emitted.load(Ordering::Relaxed) {
                        return Err(error);
                    }
                    last_error = Some(error);
                }
                Err(error) => return Err(error),
            }
        }
        Err(last_error.unwrap_or(ModelError::ProfileUnavailable))
    }
}

impl ModelGateway for AgentModelPort {
    fn complete<'a>(
        &'a self,
        request: &'a ModelRequest,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<ModelOutcome, ModelError>> + Send + 'a>,
    > {
        Box::pin(async move {
            if request.selection.is_auto() {
                return self.complete_auto(request).await;
            }
            let lease = self
                .profiles
                .reserve_explicit_account(&request.selection.profile_id);
            let execution = self
                .profiles
                .resolve(&request.selection)
                .await
                .map_err(resolve_error)?;
            if request.max_output_tokens != Some(execution.limits().max_output_tokens) {
                return Err(ModelError::InvalidSelection);
            }
            let result = self.router.complete(request, execution).await;
            if result.as_ref().err().is_some_and(account_rejected) {
                lease.reject(crate::profiles::unix_now());
            }
            result
        })
    }

    fn release(&self, suggestion: ModelReleaseSuggestion<'_>) {
        let session_id = match suggestion {
            ModelReleaseSuggestion::Session(session_id)
            | ModelReleaseSuggestion::Generation { session_id, .. } => session_id,
        };
        self.router.release_session(session_id);
    }
}

fn resolve_error(error: ProfileResolveError) -> ModelError {
    match error {
        ProfileResolveError::InvalidSelection => ModelError::InvalidSelection,
        ProfileResolveError::NotFound | ProfileResolveError::AuthUnavailable => {
            ModelError::ProfileUnavailable
        }
        ProfileResolveError::Backend => ModelError::Unavailable,
    }
}

fn account_rejected(error: &ModelError) -> bool {
    let ModelError::ProviderFailed(failure) = error else {
        return false;
    };
    matches!(failure.status_code, Some(401 | 403 | 429))
        || failure.provider_code.as_deref().is_some_and(|code| {
            matches!(
                code,
                "usage_limit_reached"
                    | "insufficient_quota"
                    | "rate_limit_exceeded"
                    | "invalid_api_key"
                    | "account_deactivated"
            )
        })
}

#[derive(Debug)]
struct ObservedStream {
    inner: Arc<dyn ModelStreamObserver>,
    emitted: AtomicBool,
}
impl ModelStreamObserver for ObservedStream {
    fn output_delta(&self, session: &str, generation: u64, step: &str, bytes: u64) {
        if bytes > 0 {
            self.emitted.store(true, Ordering::Relaxed);
        }
        self.inner.output_delta(session, generation, step, bytes);
    }
    fn text_delta(&self, session: &str, generation: u64, step: &str, text: &str) {
        if !text.is_empty() {
            self.emitted.store(true, Ordering::Relaxed);
        }
        self.inner.text_delta(session, generation, step, text);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use serde_json::json;
    use zork_agent::session::model::{SilentStreamObserver, ToolDefinition};
    use zork_agent::session::ports::{ModelExecutor, ProfileExecution};
    use zork_agent::session::wire::SessionSelection;

    use super::*;

    struct CountingExecutor(AtomicUsize);

    impl ModelExecutor for CountingExecutor {
        fn complete<'a>(
            &'a self,
            _: &'a ModelRequest,
            _: ProfileExecution,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<ModelOutcome, ModelError>> + Send + 'a>,
        > {
            self.0.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Err(ModelError::Unavailable) })
        }
    }

    #[tokio::test]
    async fn disabling_a_selected_model_blocks_the_next_provider_call() {
        let root = tempfile::tempdir().unwrap();
        let profiles = Arc::new(ProfileStore::open(root.path().to_owned(), true, false));
        profiles.put("fixture",json!({"provider":"openai-compatible","billing":"usage","base_url":"https://example.invalid/v1","models":[{"id":"model","api":"openai-completions","thinking":["off"],"default_thinking":"off","capabilities":{"input":["text"]},"limits":{"context_window_tokens":32000,"max_output_tokens":4096},"default":true}]})).unwrap();
        let executor = Arc::new(CountingExecutor(AtomicUsize::new(0)));
        let port = AgentModelPort::new(executor.clone(), profiles.clone());
        let request = ModelRequest {
            session_id: "session".into(),
            generation: 1,
            step_id: "step".into(),
            selection: SessionSelection {
                profile_id: "fixture".into(),
                model: "model".into(),
                thinking: "off".into(),
            },
            transcript: Arc::new(Vec::new()),
            tools: Arc::new(Vec::new()),
            max_output_tokens: Some(4096),
            independent: false,
            stream_observer: Arc::new(SilentStreamObserver),
        };
        assert!(matches!(
            port.complete(&request).await,
            Err(ModelError::Unavailable)
        ));
        assert_eq!(executor.0.load(Ordering::SeqCst), 1);
        profiles
            .set_model_enabled("fixture", "model", false)
            .unwrap();
        assert!(matches!(
            port.complete(&request).await,
            Err(ModelError::InvalidSelection)
        ));
        assert_eq!(executor.0.load(Ordering::SeqCst), 1);
        profiles
            .set_model_enabled("fixture", "model", true)
            .unwrap();
        assert!(matches!(
            port.complete(&request).await,
            Err(ModelError::Unavailable)
        ));
        assert_eq!(executor.0.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    // Contract: docs/design/agent-runtime.md [PROVIDER-03]
    async fn changed_limits_reject_the_request_before_the_provider_adapter() {
        let root = tempfile::tempdir().unwrap();
        let profiles = Arc::new(ProfileStore::open(root.path().to_owned(), true, false));
        profiles
            .put(
                "fixture",
                json!({
                    "provider": "openai-compatible",
                    "billing": "usage",
                    "base_url": "https://example.invalid/v1",
                    "models": [{
                        "id": "model",
                        "api": "openai-completions",
                        "thinking": ["high"],
                        "default_thinking": "high",
                        "capabilities": {"input": ["text"]},
                        "limits": {"context_window_tokens": 100_000, "max_output_tokens": 10_000},
                        "default": true
                    }]
                }),
            )
            .unwrap();
        let executor = Arc::new(CountingExecutor(AtomicUsize::new(0)));
        let port = AgentModelPort::new(executor.clone(), profiles);
        let request = ModelRequest {
            session_id: "session".into(),
            generation: 1,
            step_id: "step".into(),
            selection: SessionSelection {
                profile_id: "fixture".into(),
                model: "model".into(),
                thinking: "high".into(),
            },
            transcript: Arc::new(Vec::new()),
            tools: Arc::new(Vec::<ToolDefinition>::new()),
            max_output_tokens: Some(9_999),
            independent: false,
            stream_observer: Arc::new(SilentStreamObserver),
        };

        assert!(matches!(
            port.complete(&request).await,
            Err(ModelError::InvalidSelection)
        ));
        assert_eq!(executor.0.load(Ordering::SeqCst), 0);
    }
    struct PoolExecutor {
        calls: std::sync::Mutex<Vec<String>>,
        reject_a: bool,
        partial: bool,
        pending: bool,
    }
    impl ModelExecutor for PoolExecutor {
        fn complete<'a>(
            &'a self,
            request: &'a ModelRequest,
            execution: ProfileExecution,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<ModelOutcome, ModelError>> + Send + 'a>,
        > {
            Box::pin(async move {
                assert_eq!(request.selection.profile_id, execution.profile_id());
                self.calls
                    .lock()
                    .unwrap()
                    .push(execution.profile_id().into());
                if self.pending {
                    return std::future::pending().await;
                }
                if self.reject_a && execution.profile_id() == "a" {
                    if self.partial {
                        request.stream_observer.text_delta(
                            &request.session_id,
                            1,
                            "step",
                            "partial",
                        );
                    }
                    let mut failure = crate::session::model::ProviderFailure::new(
                        "fixture",
                        true,
                        "quota exhausted",
                    );
                    failure.status_code = Some(429);
                    return Err(ModelError::ProviderFailed(failure));
                }
                Ok(ModelOutcome {
                    text: "done".into(),
                    tool_calls: vec![],
                    provider_context: None,
                    usage: None,
                    provider_input: None,
                })
            })
        }
    }
    fn pool_fixture(
        reject_a: bool,
        partial: bool,
        pending: bool,
    ) -> (
        tempfile::TempDir,
        Arc<ProfileStore>,
        Arc<PoolExecutor>,
        ModelRequest,
    ) {
        let root = tempfile::tempdir().unwrap();
        let profiles = Arc::new(ProfileStore::open(root.path().into(), true, false));
        for (id, window, output) in [("a", 32000, 4096), ("b", 64000, 8192)] {
            profiles.put(id,json!({"provider":"openai-compatible","billing":"usage","base_url":"https://example.invalid/v1",
                "auth":{"type":"api_key","key":"fixture-secret"},
                "models":[{"id":"model","api":"openai-completions","thinking":["high"],"default_thinking":"high",
                    "capabilities":{"input":["text"]},"limits":{"context_window_tokens":window,"max_output_tokens":output}}]})).unwrap();
        }
        let executor = Arc::new(PoolExecutor {
            calls: Default::default(),
            reject_a,
            partial,
            pending,
        });
        let request = ModelRequest {
            session_id: "session".into(),
            generation: 1,
            step_id: "step".into(),
            selection: SessionSelection {
                profile_id: "auto".into(),
                model: "model".into(),
                thinking: "high".into(),
            },
            transcript: Arc::new(vec![]),
            tools: Arc::new(vec![]),
            max_output_tokens: Some(4096),
            independent: false,
            stream_observer: Arc::new(SilentStreamObserver),
        };
        (root, profiles, executor, request)
    }

    #[tokio::test]
    async fn automatic_rejection_switches_and_persists_actual_identity_with_conservative_limits() {
        let (_root, profiles, executor, mut request) = pool_fixture(true, false, false);
        let port = AgentModelPort::new(executor.clone(), profiles.clone());
        assert_eq!(
            profiles
                .model_limits(&request.selection)
                .unwrap()
                .max_output_tokens,
            4096
        );
        let outcome = port.complete(&request).await.unwrap();
        assert_eq!(*executor.calls.lock().unwrap(), vec!["a", "b"]);
        assert_eq!(request.selection.profile_id, "auto");
        assert_eq!(outcome.provider_context.as_ref().unwrap().profile_id, "b");
        request.transcript = Arc::new(vec![crate::session::wire::ProviderMessage {
            images: vec![],
            role: crate::session::wire::TranscriptRole::Assistant,
            content: outcome.text.into(),
            is_error: false,
            runtime_generated: false,
            tool_call_id: None,
            tool_calls: vec![],
            provider_context: outcome.provider_context,
        }]);
        // Fresh store has no reservations/cooldowns: durable context alone must retain B.
        let restarted = AgentModelPort::new(
            executor.clone(),
            Arc::new(ProfileStore::open(_root.path().into(), true, false)),
        );
        assert_eq!(
            restarted
                .complete(&request)
                .await
                .unwrap()
                .provider_context
                .unwrap()
                .profile_id,
            "b"
        );
        assert_eq!(*executor.calls.lock().unwrap(), vec!["a", "b", "b"]);
    }

    #[tokio::test]
    async fn explicit_account_never_switches_and_partial_output_is_not_replayed() {
        let (_root, profiles, executor, mut request) = pool_fixture(true, false, false);
        let port = AgentModelPort::new(executor.clone(), profiles);
        request.selection.profile_id = "a".into();
        assert!(matches!(
            port.complete(&request).await,
            Err(ModelError::ProviderFailed(_))
        ));
        assert_eq!(*executor.calls.lock().unwrap(), vec!["a"]);
        let (_root, profiles, executor, request) = pool_fixture(true, true, false);
        let port = AgentModelPort::new(executor.clone(), profiles);
        assert!(matches!(
            port.complete(&request).await,
            Err(ModelError::ProviderFailed(_))
        ));
        assert_eq!(*executor.calls.lock().unwrap(), vec!["a"]);
    }

    #[tokio::test]
    async fn concurrent_calls_reserve_distinct_accounts_and_cancel_releases_capacity() {
        let (_root, profiles, executor, mut request) = pool_fixture(false, false, true);
        let port = AgentModelPort::new(executor.clone(), profiles.clone());
        request.session_id = "first".into();
        let first = port.complete(&request);
        let second = port.complete(&request);
        let _ = tokio::time::timeout(std::time::Duration::from_millis(25), async {
            tokio::join!(first, second)
        })
        .await;
        assert_eq!(*executor.calls.lock().unwrap(), vec!["a", "b"]);
        let lease = profiles
            .acquire_account(&request.selection, Some("a"), false)
            .unwrap();
        assert_eq!(lease.profile_id, "a");
        // With one new A reservation and both cancelled calls released, B wins.
        assert_eq!(
            profiles
                .acquire_account(&request.selection, None, false)
                .unwrap()
                .profile_id,
            "b"
        );
    }
}
