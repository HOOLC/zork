//! Optional native initialization observations supplied by the host application.
use std::sync::OnceLock;

static OBSERVER: OnceLock<fn(&'static str)> = OnceLock::new();

/// Install an observer before creating the platform. The first observer wins.
/// It may run on a renderer worker and must not call back into application state.
pub fn set_startup_observer(observer: fn(&'static str)) {
    let _ = OBSERVER.set(observer);
}

/// Report an initialization milestone without allocating when no observer exists.
pub fn observe_startup(stage: &'static str) {
    if let Some(observer) = OBSERVER.get() {
        observer(stage);
    }
}
