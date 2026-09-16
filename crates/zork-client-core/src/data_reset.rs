//! Explicit, client-local reset. The host supplies process/OS capabilities;
//! confirmation, duplicate exclusion and observable completion belong to core.
use anyhow::{ensure, Result};
use serde::Serialize;
use std::sync::Mutex;
use zork_observe::{ValueSource, ValueSubscription};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    #[default]
    Idle,
    Clearing,
    Restarting,
    Failed,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Snapshot {
    pub phase: Phase,
    pub error: Option<String>,
}
impl Snapshot {
    pub fn busy(&self) -> bool {
        matches!(self.phase, Phase::Clearing | Phase::Restarting)
    }
}

pub struct Controller {
    operation: Mutex<()>,
    pub(crate) source: ValueSource<Snapshot>,
}
impl Default for Controller {
    fn default() -> Self {
        Self {
            operation: Mutex::new(()),
            source: ValueSource::new(Snapshot::default()),
        }
    }
}
impl Controller {
    pub fn subscribe(&self) -> ValueSubscription<Snapshot> {
        self.source.subscribe()
    }
    pub fn snapshot(&self) -> std::sync::Arc<Snapshot> {
        self.source.read()
    }

    /// Called on a host worker, not a display thread. Once accepted, closing
    /// an observer does not cancel the operation. Success asks the host to exit;
    /// it is not a claim that files still open in this process were deleted.
    pub fn clear(&self, confirmed: bool, host: impl FnOnce() -> Result<()>) -> Result<()> {
        ensure!(confirmed, "请先确认清空本机数据");
        let _operation = self
            .operation
            .try_lock()
            .map_err(|_| anyhow::anyhow!("正在清空数据"))?;
        ensure!(!self.snapshot().busy(), "正在清空数据");
        self.source.publish(Snapshot {
            phase: Phase::Clearing,
            error: None,
        });
        match host() {
            Ok(()) => {
                self.source.publish(Snapshot {
                    phase: Phase::Restarting,
                    error: None,
                });
                Ok(())
            }
            Err(error) => {
                self.source.publish(Snapshot {
                    phase: Phase::Failed,
                    error: Some(error.to_string()),
                });
                Err(error)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};

    #[test]
    fn cancellation_and_duplicate_confirmation_do_not_invoke_the_host() {
        let reset = Arc::new(Controller::default());
        assert!(reset.clear(false, || panic!("unconfirmed reset")).is_err());
        assert_eq!(reset.snapshot().phase, Phase::Idle);
        let entered = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let worker = {
            let (reset, entered, release) = (reset.clone(), entered.clone(), release.clone());
            std::thread::spawn(move || {
                reset.clear(true, || {
                    entered.wait();
                    release.wait();
                    Ok(())
                })
            })
        };
        entered.wait();
        assert!(reset.snapshot().busy());
        assert!(reset.clear(true, || panic!("duplicate reset")).is_err());
        drop(reset.subscribe());
        release.wait();
        worker.join().unwrap().unwrap();
        assert_eq!(reset.snapshot().phase, Phase::Restarting);
        assert!(reset.clear(true, || panic!("already accepted")).is_err());
    }

    #[test]
    fn a_host_failure_is_visible_and_can_be_retried() {
        let reset = Controller::default();
        assert!(reset
            .clear(true, || anyhow::bail!("node still running"))
            .is_err());
        assert_eq!(reset.snapshot().phase, Phase::Failed);
        assert_eq!(
            reset.snapshot().error.as_deref(),
            Some("node still running")
        );
        reset.clear(true, || Ok(())).unwrap();
        assert!(reset.snapshot().error.is_none());
    }

    #[test]
    fn observation_discard_keeps_the_applied_baseline_and_does_not_start_a_reset() {
        let reset = Controller::default();
        let mut wire = crate::subscriptions::WireSubscription::from_data_reset(&reset.source);
        let opening = wire.prepare().unwrap().unwrap();
        assert_eq!(opening["snapshot"]["phase"], "idle");
        assert!(wire.finish(opening["batch"].as_u64().unwrap(), false));
        assert!(reset
            .clear(true, || anyhow::bail!("cannot stop node"))
            .is_err());
        let failed = wire.prepare().unwrap().unwrap();
        assert_eq!(failed["from"], 0);
        assert_eq!(failed["snapshot"]["phase"], "failed");
        assert_eq!(failed["snapshot"]["error"], "cannot stop node");
        assert!(wire.finish(failed["batch"].as_u64().unwrap(), true));
        assert!(wire.prepare().unwrap().is_none());
    }
}
