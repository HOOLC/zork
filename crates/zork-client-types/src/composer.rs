//! Read-only presentation contract used by the composer and its offline fixtures.
#[derive(Clone, Copy, Default, Debug, serde::Serialize)]
pub struct Capabilities {
    pub editable: bool,
    pub stop: bool,
    pub enabled: bool,
}
#[derive(Clone, Debug, serde::Serialize)]
pub struct File {
    pub id: u64,
    pub name: String,
}
#[derive(Clone, Default, Debug, serde::Serialize)]
pub struct Snapshot {
    pub capabilities: Capabilities,
    pub text: String,
    pub files: Vec<File>,
    pub events: Vec<String>,
}
pub enum Intent {
    Inspect,
    Edit(String),
    Primary,
    Submit,
    Files(Vec<String>),
    RemoveFile(u64),
    Scenario(usize),
}
/// An offline host supplies this port. No UI implementation of product policy.
pub trait Controller {
    fn apply(&mut self, intent: Intent) -> Snapshot;
}
