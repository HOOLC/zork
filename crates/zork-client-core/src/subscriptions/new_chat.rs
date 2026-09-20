use super::snapshot::SnapshotWire;
use crate::state::{Device, NewChat};
use anyhow::Result;
use serde_json::Value;
use std::sync::Arc;
use zork_observe::Readiness;

pub(super) struct NewChatWire {
    source: Arc<NewChat>,
    device: Arc<Device>,
    wire: SnapshotWire<zork_client_types::new_chat::Snapshot>,
}
impl NewChatWire {
    pub fn new(device: Arc<Device>) -> Self {
        let source = device.new_chat();
        Self {
            wire: SnapshotWire::new(&source.view),
            source,
            device,
        }
    }
    pub fn signals(&self) -> Vec<Readiness> {
        self.wire.signals()
    }
    pub fn valid(&self) -> bool {
        self.wire.valid()
    }
    pub fn prepare(&mut self) -> Result<Option<(Value, bool, bool)>> {
        if self.device.snapshot().revoked && !self.source.view.read().revoked {
            self.source
                .view
                .invalidate(zork_client_types::new_chat::Snapshot {
                    revoked: true,
                    error: Some("设备访问权限已撤销".into()),
                    ..Default::default()
                });
        }
        Ok(self.wire.prepare()?.map(|(value, reset, _)| {
            let revoked = value["snapshot"]["revoked"] == true;
            (value, reset, revoked)
        }))
    }
    pub fn finish(&mut self, applied: bool) -> bool {
        self.wire.finish(applied)
    }
}
