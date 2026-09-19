//! Session ledger for Zork's durable Agent events. Association is
//! exclusively by step/invocation ID; a result can precede its start in a page.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

pub mod activity;
mod ledger;
pub mod usage;
pub use ledger::{EntryOrder, Ledger, ProjectedEntry};

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct Record {
    pub event_id: String,
    pub event: Value,
    #[serde(flatten)]
    pub metadata: serde_json::Map<String, Value>,
}
#[derive(Debug, Deserialize)]
pub struct Page {
    #[serde(default)]
    pub runtime: Option<Value>,
    pub server_time_ms: i64,
    pub items: Vec<Record>,
    pub older_cursor: Option<String>,
    pub latest_cursor: Option<String>,
    pub has_more: bool,
}
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Entry {
    pub id: String,
    pub lane: usize,
    pub action: String,
    pub summary: String,
    pub start: Option<i64>,
    pub end: Option<i64>,
    pub state: String,
    pub raw: Vec<Value>,
    pub usage: Option<Value>,
    pub model: Option<String>,
    pub outcome_summary: Option<String>,
}
impl Entry {
    pub fn duration(&self, now: i64) -> Option<i64> {
        Some(
            (self
                .end
                .or_else(|| (self.state == "running").then_some(now))?
                - self.start?)
                .max(0),
        )
    }
}
fn string(v: &Value, key: &str) -> String {
    v[key].as_str().unwrap_or_default().to_owned()
}
fn number(v: &Value, key: &str) -> Option<i64> {
    v[key].as_i64()
}
fn summary(v: &Value) -> String {
    for key in [
        "error",
        "stderr",
        "stdout",
        "command",
        "path",
        "content",
        "text",
        "message",
        "query",
        "task_id",
        "session_id",
    ] {
        if let Some(s) = v[key].as_str() {
            return s.to_owned();
        }
    }
    if v.is_null() {
        String::new()
    } else {
        v.to_string()
    }
}
pub fn entries(records: &[Record]) -> Vec<Entry> {
    entries_iter(records.iter())
}

/// Build a complete snapshot using the same reducer as the live core controller.
/// Live updates retain `Ledger` and ingest only the new page.
pub fn entries_iter<'a>(records: impl Iterator<Item = &'a Record> + Clone) -> Vec<Entry> {
    let records = records
        .map(|record| Arc::new(record.clone()))
        .collect::<Vec<_>>();
    Ledger::default()
        .ingest(&records, false)
        .into_iter()
        .map(|row| row.entry)
        .collect()
}
