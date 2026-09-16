//! User-started, ephemeral local execution. Nothing here writes Chat or sends Agent input.
#[cfg(feature = "local-scripts")]
mod engine;
mod operations;
use anyhow::{ensure, Context, Result};
pub use operations::{Intent, Operation};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::BTreeSet,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tokio::sync::oneshot;
use zork_client_types::local_script::Card;
use zork_observe::{Topics, ValueSource};

#[cfg(feature = "local-scripts")]
const DEADLINE: Duration = Duration::from_secs(300);
#[cfg(feature = "local-scripts")]
const MAX_CALLS: usize = 128;
const MAX_REPLY_BYTES: usize = operations::MAX_DATA * 2 + 2048;

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    Claim {
        run_id: String,
        request_id: String,
    },
    Complete {
        run_id: String,
        request_id: String,
        value: Option<Value>,
        error: Option<String>,
    },
    Cancel {
        run_id: String,
    },
    Dismiss {
        run_id: String,
    },
}

struct Origin {
    peer: String,
    generation: u64,
}
struct Pending {
    id: String,
    operation: Operation,
    claimed: bool,
    reply: oneshot::Sender<Result<Value>>,
}
struct Run {
    id: String,
    title: String,
    phase: &'static str,
    error: Option<String>,
    logs: Vec<String>,
    visible: bool,
    worker_active: bool,
    cancelled: Arc<AtomicBool>,
    cancel_wake: Arc<tokio::sync::Notify>,
    deadline: Instant,
    origin: Option<Origin>,
    calls: usize,
    documents: BTreeSet<String>,
    pending: Option<Pending>,
}
pub struct Controller {
    pub(crate) source: ValueSource<Value>,
    store: Arc<crate::store::ClientStore>,
    run: Mutex<Option<Run>>,
}
impl Controller {
    pub fn new(store: Arc<crate::store::ClientStore>) -> Arc<Self> {
        Arc::new(Self {
            source: ValueSource::new(json!({"run":null,"request":null,"active_run":null})),
            store,
            run: Mutex::new(None),
        })
    }
    fn publish(&self, run: &Option<Run>) {
        let value = match run {
            None => json!({"run":null,"request":null,"active_run":null}),
            Some(run) => json!({
                "active_run":(run.phase == "running").then_some(&run.id),
                "run": if run.visible { json!({"id":run.id,"title":run.title,"phase":run.phase,
                    "error":run.error,"logs":run.logs,"can_cancel":run.phase == "running"}) } else { Value::Null },
                "request":run.pending.as_ref().filter(|p| !p.claimed).map(|p| json!({"run_id":run.id,"id":p.id,"operation":p.operation})),
            }),
        };
        self.source.publish_changed(value, Topics::ALL);
    }
    pub fn snapshot(&self) -> Arc<Value> {
        self.source.read()
    }

    /// Loads code from the actual cached message, never from an activation payload.
    pub(crate) fn start_message(
        self: &Arc<Self>,
        peer: &str,
        session: &str,
        message: &str,
    ) -> Result<Value> {
        ensure!(
            cfg!(all(target_os = "android", feature = "local-scripts")),
            "Run this card on Android"
        );
        let generation = self.store.replica_generation(peer)?;
        let source = self
            .store
            .cached_message_at(peer, session, message, generation)?
            .context("Script card unavailable")?;
        let crate::api::TranscriptMessage::Message { metadata, .. } = source;
        let card = metadata
            .interaction
            .as_deref()
            .and_then(Card::parse)
            .context("Unsupported script card")?;
        self.start(
            card,
            Some(Origin {
                peer: peer.into(),
                generation,
            }),
        )
    }

    #[cfg(not(feature = "local-scripts"))]
    fn start(self: &Arc<Self>, _card: Card, _origin: Option<Origin>) -> Result<Value> {
        anyhow::bail!("Local JavaScript execution is unavailable")
    }
    #[cfg(feature = "local-scripts")]
    fn start(self: &Arc<Self>, card: Card, origin: Option<Origin>) -> Result<Value> {
        card.validate().map_err(anyhow::Error::msg)?;
        let runtime = tokio::runtime::Handle::try_current().context("Local runtime unavailable")?;
        let mut state = self.run.lock().unwrap();
        ensure!(
            state.as_ref().is_none_or(|r| !r.worker_active),
            "A local script is already running"
        );
        let id = ulid::Ulid::new().to_string();
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancel_wake = Arc::new(tokio::sync::Notify::new());
        let deadline = Instant::now() + DEADLINE;
        *state = Some(Run {
            id: id.clone(),
            title: card.title,
            phase: "running",
            error: None,
            logs: vec![],
            visible: true,
            worker_active: true,
            cancelled: cancelled.clone(),
            cancel_wake: cancel_wake.clone(),
            deadline,
            origin,
            calls: 0,
            documents: BTreeSet::new(),
            pending: None,
        });
        self.publish(&state);
        drop(state);
        let controller = self.clone();
        let run_id = id.clone();
        // QuickJS and its values never leave this worker. Native callbacks wait
        // here, while Android's main thread and core command lane remain free.
        runtime.clone().spawn_blocking(move || {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                engine::execute(
                    controller.clone(),
                    &run_id,
                    &card.source,
                    cancelled,
                    cancel_wake,
                    deadline,
                    &runtime,
                )
            }))
            .unwrap_or_else(|_| Err(anyhow::anyhow!("JavaScript runtime stopped unexpectedly")));
            controller.finish(&run_id, outcome);
        });
        Ok(json!({"run_id":id}))
    }
    fn authorized(&self, run: &Run) -> Result<()> {
        ensure!(
            run.phase == "running" && !run.cancelled.load(Ordering::Relaxed),
            "Script cancelled"
        );
        ensure!(Instant::now() < run.deadline, "Script timed out");
        if let Some(origin) = &run.origin {
            ensure!(
                !self.store.replica_revoked(&origin.peer)?
                    && self.store.replica_generation(&origin.peer)? == origin.generation,
                "Script source access changed"
            );
        }
        Ok(())
    }
    #[cfg(feature = "local-scripts")]
    fn call(
        &self,
        run_id: &str,
        operation: Operation,
        runtime: &tokio::runtime::Handle,
    ) -> Result<Value> {
        operation.validate()?;
        let (send, receive) = oneshot::channel();
        let deadline;
        {
            let mut state = self.run.lock().unwrap();
            let run = state
                .as_mut()
                .filter(|r| r.id == run_id)
                .context("Script ended")?;
            self.authorized(run)?;
            ensure!(
                run.calls < MAX_CALLS && run.pending.is_none(),
                "Script native call limit reached"
            );
            if let Operation::ContentReadText { uri } | Operation::ContentWriteText { uri, .. } =
                &operation
            {
                ensure!(
                    run.documents.contains(uri),
                    "Select this document in this script before accessing it"
                );
            }
            run.calls += 1;
            deadline = run.deadline;
            run.pending = Some(Pending {
                id: ulid::Ulid::new().to_string(),
                operation,
                claimed: false,
                reply: send,
            });
            self.publish(&state);
        }
        runtime.block_on(async {
            tokio::time::timeout_at(deadline.into(), receive)
                .await
                .context("Script timed out")?
                .context("Script cancelled")?
        })
    }
    #[cfg(feature = "local-scripts")]
    fn log(&self, id: &str, text: &str) -> Result<()> {
        let mut state = self.run.lock().unwrap();
        let run = state
            .as_mut()
            .filter(|r| r.id == id)
            .context("Script ended")?;
        self.authorized(run)?;
        // Keep output and its publication rate bounded even for a logging loop.
        ensure!(run.logs.len() < 32, "Script output limit reached");
        run.logs.push(text.chars().take(512).collect());
        self.publish(&state);
        Ok(())
    }
    #[cfg(feature = "local-scripts")]
    fn finish(&self, id: &str, outcome: Result<()>) {
        let mut state = self.run.lock().unwrap();
        if let Some(run) = state.as_mut().filter(|r| r.id == id) {
            run.worker_active = false;
            run.pending = None;
            if run.phase == "running" {
                run.phase = if outcome.is_ok() {
                    "succeeded"
                } else {
                    "failed"
                };
                run.error = outcome
                    .err()
                    .map(|e| e.to_string().chars().take(2048).collect());
            }
            self.publish(&state);
        }
    }
    pub fn apply(&self, action: Action) -> Result<Value> {
        // Measure transport values before taking the execution-state lock.
        let oversized = match &action {
            Action::Complete {
                value: Some(value), ..
            } => serde_json::to_vec(value)?.len() > MAX_REPLY_BYTES,
            _ => false,
        };
        let mut state = self.run.lock().unwrap();
        let id = match &action {
            Action::Claim { run_id, .. }
            | Action::Complete { run_id, .. }
            | Action::Cancel { run_id }
            | Action::Dismiss { run_id } => run_id,
        };
        let Some(run) = state.as_mut().filter(|r| &r.id == id) else {
            return Ok(json!({"accepted":false}));
        };
        match action {
            Action::Claim { request_id, .. } => {
                if let Err(error) = self.authorized(run) {
                    if let Some(pending) = run.pending.take() {
                        let _ = pending.reply.send(Err(error));
                    }
                    self.publish(&state);
                    return Ok(json!({"accepted":false}));
                }
                let Some(pending) = run
                    .pending
                    .as_mut()
                    .filter(|p| p.id == request_id && !p.claimed)
                else {
                    return Ok(json!({"accepted":false}));
                };
                pending.claimed = true;
                // Return the current core-owned operation, not the possibly stale frame.
                let mut operation = serde_json::to_value(&pending.operation)?;
                operation["max_bytes"] = json!(operations::MAX_DATA);
                operation["max_reply_bytes"] = json!(MAX_REPLY_BYTES);
                if matches!(pending.operation, Operation::Location) {
                    operation["timeout_ms"] = json!(30_000);
                }
                self.publish(&state);
                Ok(json!({"accepted":true,"operation":operation}))
            }
            Action::Complete {
                request_id,
                value,
                error,
                ..
            } => {
                if self.authorized(run).is_err()
                    || run
                        .pending
                        .as_ref()
                        .is_none_or(|p| p.id != request_id || !p.claimed)
                {
                    return Ok(json!({"accepted":false}));
                }
                let pending = run.pending.take().unwrap();
                let mut value = value.unwrap_or(Value::Null);
                let outcome = (|| -> Result<Value> {
                    if let Some(error) = error {
                        Err(anyhow::anyhow!(error
                            .chars()
                            .take(2048)
                            .collect::<String>()))
                    } else if oversized {
                        Err(anyhow::anyhow!("Android result too large"))
                    } else {
                        if let Operation::StartActivity {
                            intent,
                            result: true,
                        } = &pending.operation
                        {
                            if matches!(
                                intent.action.as_str(),
                                "android.intent.action.OPEN_DOCUMENT"
                                    | "android.intent.action.CREATE_DOCUMENT"
                                    | "android.intent.action.GET_CONTENT"
                            ) && value["resultCode"] == -1
                            {
                                for uri in value["data"]
                                    .as_str()
                                    .into_iter()
                                    .chain(
                                        value["uris"]
                                            .as_array()
                                            .into_iter()
                                            .flatten()
                                            .filter_map(Value::as_str),
                                    )
                                    .take(32)
                                {
                                    if operations::content_uri(uri) {
                                        run.documents.insert(uri.into());
                                    }
                                }
                            }
                        }
                        if matches!(pending.operation, Operation::ContentReadText { .. }) {
                            let hex = value["hex"].as_str().context("Missing document bytes")?;
                            ensure!(
                                hex.len() <= operations::MAX_DATA * 2 && hex.len() % 2 == 0,
                                "Document too large"
                            );
                            let bytes = hex
                                .as_bytes()
                                .chunks_exact(2)
                                .map(|b| {
                                    let hi = (b[0] as char)
                                        .to_digit(16)
                                        .context("Invalid document bytes")?;
                                    let lo = (b[1] as char)
                                        .to_digit(16)
                                        .context("Invalid document bytes")?;
                                    Ok((hi * 16 + lo) as u8)
                                })
                                .collect::<Result<Vec<_>>>()?;
                            value =
                                json!(String::from_utf8(bytes)
                                    .context("Document is not UTF-8 text")?);
                        }
                        Ok(value)
                    }
                })();
                self.publish(&state);
                let _ = pending.reply.send(outcome);
                Ok(json!({"accepted":true}))
            }
            Action::Cancel { .. } => {
                if run.phase != "running" {
                    return Ok(json!({"accepted":false}));
                }
                run.cancelled.store(true, Ordering::Relaxed);
                run.cancel_wake.notify_waiters();
                run.pending = None;
                run.phase = "cancelled";
                self.publish(&state);
                Ok(json!({"accepted":true}))
            }
            Action::Dismiss { .. } => {
                if !run.visible {
                    return Ok(json!({"accepted":false}));
                }
                run.visible = false;
                self.publish(&state);
                Ok(json!({"accepted":true}))
            }
        }
    }
}

#[cfg(all(test, feature = "local-scripts"))]
mod tests;
