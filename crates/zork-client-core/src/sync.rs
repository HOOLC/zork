//! One coordinator per authenticated peer. Network awaits never hold the local
//! database mutex. Only committed batches generate observer notifications.
use crate::{
    api::StationClient,
    store::{ClientStore, ReplicaApply},
};
use anyhow::{ensure, Result};
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use tokio::sync::{watch, Mutex};
use zork_client_types::sync::{Pull, Reply, Scope};

#[derive(Clone, Debug, Default)]
pub struct SyncStatus {
    pub running: bool,
    pub commits: u64,
    pub error: Option<String>,
    pub operation_error: Option<String>,
}

pub struct Coordinator {
    client: Arc<StationClient>,
    store: Arc<ClientStore>,
    peer: String,
    owner: String,
    generation: u64,
    gate: Mutex<()>,
    requested: AtomicU64,
    completed: std::sync::Mutex<HashMap<String, (u64, Option<String>)>>,
    status: watch::Sender<SyncStatus>,
}
impl Coordinator {
    pub fn new(
        client: Arc<StationClient>,
        store: Arc<ClientStore>,
        peer: String,
        authenticated_owner: String,
    ) -> Result<Arc<Self>> {
        let (status, _) = watch::channel(SyncStatus::default());
        let generation = store.replica_generation(&peer)?;
        Ok(Arc::new(Self {
            client,
            store,
            peer,
            owner: authenticated_owner,
            generation,
            gate: Mutex::new(()),
            requested: AtomicU64::new(0),
            completed: Default::default(),
            status,
        }))
    }
    pub fn subscribe(&self) -> watch::Receiver<SyncStatus> {
        self.status.subscribe()
    }

    pub async fn refresh(&self, scope: Scope) -> Result<()> {
        let ticket = self.requested.fetch_add(1, Ordering::Relaxed) + 1;
        let _gate = self.gate.lock().await;
        let covered = self.completed.lock().unwrap().get(&scope.key()).cloned();
        if let Some((_, error)) = covered.filter(|(done, _)| *done >= ticket) {
            return error.map_or(Ok(()), |error| Err(anyhow::anyhow!(error)));
        }
        self.status.send_modify(|s| {
            s.running = true;
            s.error = None;
        });
        struct Running<'a>(&'a watch::Sender<SyncStatus>);
        impl Drop for Running<'_> {
            fn drop(&mut self) {
                self.0.send_modify(|s| s.running = false);
            }
        }
        let _running = Running(&self.status);
        let operation_error = self
            .reconcile_operations()
            .await
            .err()
            .map(|e| e.to_string());
        self.status
            .send_modify(|s| s.operation_error = operation_error);
        // Requests made before this pull starts share its result. A request
        // arriving during it remains pending and causes one subsequent catch-up,
        // so a higher in-flight hint cannot be silently coalesced away.
        let covers = self.requested.load(Ordering::Relaxed);
        let key = scope.key();
        let result = self.pull(scope).await;
        self.completed.lock().unwrap().insert(
            key,
            (covers, result.as_ref().err().map(ToString::to_string)),
        );
        self.status.send_modify(|s| {
            s.running = false;
            s.error = result.as_ref().err().map(ToString::to_string);
        });
        result
    }

    pub async fn mutate(
        &self,
        intent: &zork_client_types::sync::Mutation,
    ) -> Result<zork_client_types::sync::Receipt> {
        let _gate = self.gate.lock().await;
        ensure!(
            self.store.replica_generation(&self.peer)? == self.generation,
            "sync_authorization_changed"
        );
        ensure!(
            intent
                .owner
                .as_deref()
                .is_none_or(|owner| owner == self.owner),
            "sync_owner_mismatch"
        );
        self.store.prepare_operation(&self.peer, intent)?;
        if let Some(receipt) = self
            .store
            .operation_receipt(&self.peer, &intent.request_id)?
        {
            return Ok(receipt);
        }
        if !self.store.begin_operation(&self.peer, &intent.request_id)? {
            return self
                .lookup_receipt(&intent.request_id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("操作结果尚未确认；不会自动重复提交"));
        }
        let value = self
            .client
            .node_request(
                reqwest::Method::POST,
                "/v1/node/sync/commands".into(),
                Some(serde_json::to_value(intent)?),
            )
            .await?;
        let receipt: zork_client_types::sync::Receipt = serde_json::from_value(value)?;
        ensure!(
            receipt.request_id == intent.request_id,
            "sync_receipt_id_mismatch"
        );
        self.store
            .finish_operation(&self.peer, &self.owner, self.generation, &receipt)?;
        Ok(receipt)
    }

    async fn lookup_receipt(&self, id: &str) -> Result<Option<zork_client_types::sync::Receipt>> {
        let value = self
            .client
            .node_request(
                reqwest::Method::POST,
                "/v1/node/sync/receipt".into(),
                Some(serde_json::json!({"request_id":id})),
            )
            .await?;
        let receipt: Option<zork_client_types::sync::Receipt> = serde_json::from_value(value)?;
        if let Some(receipt) = &receipt {
            ensure!(receipt.request_id == id, "sync_receipt_id_mismatch");
            self.store
                .finish_operation(&self.peer, &self.owner, self.generation, receipt)?;
        }
        Ok(receipt)
    }

    async fn reconcile_operations(&self) -> Result<()> {
        for intent in self.store.pending_operations(&self.peer)? {
            self.lookup_receipt(&intent.request_id).await?;
        }
        Ok(())
    }

    async fn pull(&self, scope: Scope) -> Result<()> {
        let mut request = self.store.replica_pull(&self.peer, &scope)?;
        let mut resets = 0;
        loop {
            let value = self
                .client
                .node_request(
                    reqwest::Method::POST,
                    "/v1/node/sync".into(),
                    Some(serde_json::to_value(&request)?),
                )
                .await?;
            let reply: Reply = serde_json::from_value(value)?;
            match reply {
                Reply::ResetRequired { owner, .. } => {
                    ensure!(owner == self.owner, "sync_owner_mismatch");
                    resets += 1;
                    ensure!(resets <= 2, "sync_repeated_reset");
                    self.store
                        .discard_replica_staging_at(&self.peer, &scope, self.generation)?;
                    request = Pull {
                        scope: scope.clone(),
                        after: None,
                        continuation: None,
                    };
                }
                Reply::Page { page } => {
                    ensure!(
                        page.through.scope == scope && page.from == request.after,
                        "sync_unrequested_range"
                    );
                    if let Some(next) = &request.continuation {
                        ensure!(
                            page.batch_id == next.batch_id && page.index == next.index,
                            "sync_unrequested_page"
                        );
                    } else {
                        ensure!(page.index == 0, "sync_unrequested_page");
                    }
                    let result = self.store.apply_replica_page_at(
                        &self.peer,
                        &self.owner,
                        self.generation,
                        &page,
                    )?;
                    if matches!(result, ReplicaApply::Published { .. }) {
                        self.status
                            .send_modify(|s| s.commits = s.commits.wrapping_add(1));
                    }
                    if page.last {
                        return Ok(());
                    }
                    request = self.store.replica_pull(&self.peer, &scope)?;
                }
            }
        }
    }
}
