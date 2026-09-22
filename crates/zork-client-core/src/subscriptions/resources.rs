use crate::{
    resources::{Inspection, InspectionContent, ResourceKind, Resources, ResourcesData},
    state::Subscription,
    store::ClientStore,
};
use anyhow::Result;
use serde_json::{json, Value};
use std::sync::Arc;
use zork_observe::{BatchId, Readiness};

/// A resource observer encodes only its selected list or detail. The shared
/// controller owns the bounded inspection cache and all transport operations.
pub(super) struct ResourcesWire {
    source: Arc<Resources>,
    store: Arc<ClientStore>,
    updates: Subscription<ResourcesData>,
    pending: Option<BatchId>,
    peers: Vec<String>,
    peer: Option<String>,
    kind: ResourceKind,
    query: Option<Inspection>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn resource_wire_filters_scope_retries_unapplied_batches_and_clears_revoked_data() {
        let root = tempfile::tempdir().unwrap();
        let store = Arc::new(ClientStore::open(root.path()).unwrap());
        let source = Resources::fixture(ResourcesData {
            devices: vec![crate::resources::ResourceDevice {
                id: "node".into(),
                name: "fixture".into(),
                catalog: Some(crate::resources::ResourceCatalog {
                    origin: "node".into(),
                    items: vec![
                        crate::resources::Resource::new(
                            ResourceKind::Mcp,
                            "tool".into(),
                            "Tools".into(),
                            "ready".into(),
                            "local".into(),
                        ),
                        crate::resources::Resource::new(
                            ResourceKind::Service,
                            "service".into(),
                            "Service".into(),
                            "running".into(),
                            "shared".into(),
                        ),
                    ],
                    issues: vec![],
                }),
                ..Default::default()
            }],
            ..Default::default()
        });
        let mut wire = crate::subscriptions::WireSubscription::from_resources(
            source,
            store.clone(),
            Some("node".into()),
            ResourceKind::Mcp,
            None,
        )
        .unwrap();
        let first = wire.prepare().unwrap().unwrap();
        assert_eq!(
            first["snapshot"]["devices"][0]["items"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(first["snapshot"]["devices"][0]["items"][0]["id"], "tool");
        wire.finish(first["batch"].as_u64().unwrap(), false);
        let retry = wire.prepare().unwrap().unwrap();
        assert_eq!(retry["from"], 0);
        assert_eq!(retry["snapshot"], first["snapshot"]);
        store.revoke_replica("node").unwrap();
        assert!(!wire.valid(retry["batch"].as_u64().unwrap()));
        let cleared = wire.prepare().unwrap().unwrap();
        assert_eq!(cleared["snapshot"]["devices"][0]["items"], json!([]));
        assert!(cleared["snapshot"]["devices"][0]["error"]
            .as_str()
            .unwrap()
            .contains("撤销"));
        assert!(wire.finish(cleared["batch"].as_u64().unwrap(), true));
    }
}
impl ResourcesWire {
    pub fn new(
        source: Arc<Resources>,
        store: Arc<ClientStore>,
        peer: Option<String>,
        kind: ResourceKind,
        query: Option<Inspection>,
    ) -> Result<Self> {
        anyhow::ensure!(query.is_none() || peer.is_some(), "请选择所属设备");
        let updates = source.subscribe();
        Ok(Self {
            source,
            store,
            updates,
            pending: None,
            peers: vec![],
            peer,
            kind,
            query,
        })
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
        for device in &self.source.snapshot().devices {
            if self.store.replica_revoked(&device.id)? {
                self.source.revoke(&device.id);
            }
        }
        let Some(batch) = self.updates.prepare() else {
            return Ok(None);
        };
        self.pending = Some(batch.id);
        let data = &batch.snapshot.value;
        let devices = data
            .devices
            .iter()
            .filter(|d| self.peer.as_ref().is_none_or(|id| id == &d.id))
            .collect::<Vec<_>>();
        self.peers = devices
            .iter()
            .filter(|d| d.catalog.is_some())
            .map(|d| d.id.clone())
            .collect();
        let inspection = self
            .peer
            .as_deref()
            .zip(self.query.as_ref())
            .map(|(peer, query)| {
                let state = data.inspection(peer, query);
                let mut value = json!({"loading":state.is_some_and(|s| s.loading),
                "error":state.and_then(|s|s.error.as_ref())});
                match state.and_then(|s| s.content.as_deref()) {
                    Some(InspectionContent::Skills(skills)) => value["skills"] = json!(skills),
                    Some(InspectionContent::Details(details)) => value["details"] = json!(details),
                    None => {}
                }
                if state.is_some_and(|s| s.content.is_some())
                    && !self.peers.iter().any(|id| id == peer)
                {
                    self.peers.push(peer.into());
                }
                value
            });
        let value = json!({"devices": devices.iter().map(|d| json!({
            "id":d.id,"name":d.name,"status":d.status,"loading":d.loading,"error":d.error,
            "items":d.catalog.as_ref().map(|c|c.items.iter().filter(|r|r.kind==self.kind).collect::<Vec<_>>()).unwrap_or_default(),
            "issues":d.catalog.as_ref().map(|c|c.issues.iter().filter(|r|r.kind==self.kind).collect::<Vec<_>>()).unwrap_or_default(),
        })).collect::<Vec<_>>(), "inspection":inspection});
        Ok(Some((json!({"snapshot":value}), batch.is_reset(), false)))
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
