//! Full, coalescible scalar state with the same applied-batch contract as lists.
use anyhow::Result;
use serde_json::{json, Value};
use zork_observe::{BatchId, Readiness, ValueSource, ValueSubscription};

pub(super) struct SnapshotWire<T = Value> {
    updates: ValueSubscription<T>,
    pending: Option<BatchId>,
}
impl<T: serde::Serialize> SnapshotWire<T> {
    pub fn new(source: &ValueSource<T>) -> Self {
        Self {
            updates: source.subscribe(),
            pending: None,
        }
    }
    pub fn signals(&self) -> Vec<Readiness> {
        vec![self.updates.readiness()]
    }
    pub fn valid(&self) -> bool {
        self.pending.is_some_and(|id| self.updates.valid(id))
    }
    pub fn prepare(&mut self) -> Result<Option<(Value, bool, bool)>> {
        let Some(batch) = self.updates.prepare() else {
            return Ok(None);
        };
        self.pending = Some(batch.id);
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
