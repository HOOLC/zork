use crate::{
    shared_files::{SharedFiles, SharedFilesData},
    state::Subscription,
    store::ClientStore,
};
use anyhow::Result;
use serde_json::{json, Value};
use std::sync::Arc;
use zork_observe::{BatchId, Readiness};

pub(super) struct SharedFilesWire {
    source: Arc<SharedFiles>,
    store: Arc<ClientStore>,
    updates: Subscription<SharedFilesData>,
    pending: Option<BatchId>,
    peers: Vec<String>,
}
impl SharedFilesWire {
    pub fn new(source: Arc<SharedFiles>, store: Arc<ClientStore>) -> Self {
        let updates = source.subscribe();
        Self {
            source,
            store,
            updates,
            pending: None,
            peers: vec![],
        }
    }
    pub fn signals(&self) -> Vec<Readiness> {
        vec![self.updates.readiness()]
    }
    pub fn valid(&self) -> bool {
        self.pending.is_some_and(|id| self.updates.valid(id))
            && self
                .peers
                .iter()
                .all(|peer| !self.store.replica_revoked(peer).unwrap_or(true))
    }
    pub fn prepare(&mut self) -> Result<Option<(Value, bool, bool)>> {
        for peer in self.source.access_peers() {
            if self.store.replica_revoked(&peer)? {
                self.source.revoke(&peer);
            }
        }
        let Some(batch) = self.updates.prepare() else {
            return Ok(None);
        };
        self.pending = Some(batch.id);
        self.peers = self.source.access_peers();
        Ok(Some((
            json!({"snapshot":batch.snapshot.value}),
            batch.is_reset(),
            false,
        )))
    }
    pub fn finish(&mut self, applied: bool) -> bool {
        self.pending.take().is_some_and(|id| {
            if applied {
                self.updates.acknowledge(id)
            } else {
                self.updates.discard(id)
            }
        })
    }
}
