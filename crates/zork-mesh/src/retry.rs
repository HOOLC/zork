//! Retry cadence for discovery and enrollment failures.
use std::time::Duration;

/// 1s for five failures, then 6s for five, then 11s, and so on.
#[derive(Default)]
pub struct DiscoveryBackoff {
    failures: u64,
}
impl DiscoveryBackoff {
    pub fn after_failures(failures: u64) -> Self {
        Self { failures }
    }
    pub fn next_delay(&mut self) -> Duration {
        let secs = 1u64.saturating_add((self.failures / 5).saturating_mul(5));
        self.failures = self.failures.saturating_add(1);
        Duration::from_secs(secs)
    }
    pub fn reset(&mut self) {
        self.failures = 0;
    }
}
