//! The scalar ADB snapshot also requests an OS foreground service. Revalidate
//! its current settings and authorization before a prepared frame is applied.
use super::snapshot::SnapshotWire;
use crate::adb::Controller;
use anyhow::Result;
use serde_json::Value;
use std::sync::Arc;

pub(super) struct AdbWire {
    pub wire: SnapshotWire,
    source: Arc<Controller>,
    pending: Option<Value>,
}
impl AdbWire {
    pub fn new(source: Arc<Controller>) -> Self {
        Self {
            wire: SnapshotWire::new(&source.source),
            source,
            pending: None,
        }
    }
    pub fn valid(&self) -> bool {
        self.wire.valid()
            && self
                .pending
                .as_ref()
                .is_some_and(|value| *value == self.source.snapshot())
    }
    pub fn prepare(&mut self) -> Result<Option<(Value, bool, bool)>> {
        self.source.publish();
        let batch = self.wire.prepare()?;
        self.pending = batch
            .as_ref()
            .map(|(value, _, _)| value["snapshot"].clone());
        Ok(batch)
    }
}
