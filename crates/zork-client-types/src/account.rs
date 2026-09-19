//! Public account presentation. Credentials stay in the native core store.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    #[default]
    Idle,
    Starting,
    Waiting,
    SigningOut,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    pub origin: String,
    pub subject: Option<String>,
    pub email: Option<String>,
    pub authenticated: bool,
    pub pending_revocations: usize,
    pub phase: Phase,
    pub login_url: Option<String>,
    pub error: Option<String>,
}
impl Snapshot {
    pub fn busy(&self) -> bool {
        self.phase != Phase::Idle
    }
}
