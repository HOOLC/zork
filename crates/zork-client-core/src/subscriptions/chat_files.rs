use super::snapshot::SnapshotWire;
use crate::chat_files::{Controller, Snapshot};
use anyhow::Result;
use serde_json::Value;
use std::sync::Arc;
use zork_observe::Readiness;

pub(super) struct ChatFilesWire {
    source: Arc<Controller>,
    wire: SnapshotWire<Snapshot>,
}
impl ChatFilesWire {
    pub fn new(source: Arc<Controller>) -> Self {
        Self {
            wire: SnapshotWire::new(&source.source),
            source,
        }
    }
    pub fn signals(&self) -> Vec<Readiness> {
        self.wire.signals()
    }
    pub fn valid(&self) -> bool {
        self.source.valid() && self.wire.valid()
    }
    pub fn prepare(&mut self) -> Result<Option<(Value, bool, bool)>> {
        self.source.reconcile();
        self.wire.prepare()
    }
    pub fn finish(&mut self, applied: bool) -> bool {
        self.wire.finish(applied && self.source.valid())
    }
}
