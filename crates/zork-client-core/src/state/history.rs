use crate::api::StationClient;
use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
};
use zork_client_types::history::{Entry, Page, Record};
use zork_observe::{
    BatchId, Changes, Cursor, JournalLimits, List, ListEdit, Readiness, Source, Topics,
};
mod change;
mod projection;
use change::HistoryChange;
pub use projection::HistoryLookup;
use projection::Projection;

#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize)]
pub struct HistoryRuntime {
    pub model: Option<String>,
    pub thinking: Option<String>,
    pub context_tokens: Option<u64>,
    pub context_limit: Option<u64>,
    pub profile: Option<crate::api::ProfileInfo>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct HistoryData {
    pub runtime: Option<HistoryRuntime>,
    pub records: List<Record>,
    pub entries: List<Entry>,
    pub lookup: HistoryLookup,
    pub older: Option<String>,
    pub latest: Option<String>,
    pub loading: bool,
    pub loading_older: bool,
    pub loaded: bool,
    pub error: Option<String>,
    pub clock_offset_ms: i64,
    pub revoked: bool,
}
impl HistoryData {
    /// A bounded recent execution excerpt, newest first. Current work is
    /// presented separately; inputs and successful model reasoning stay in history.
    /// This is deliberately not an all-time summary of partially loaded pages.
    pub fn highlights(&self) -> impl Iterator<Item = &Entry> {
        let mut recent: Vec<_> = self
            .entries
            .iter()
            .rev()
            .take(100)
            .filter(|entry| {
                entry.state != "running"
                    && (entry.lane == 2
                        || (entry.lane == 1
                            && matches!(entry.state.as_str(), "failed" | "timed_out")))
            })
            .collect();
        recent
            .sort_by_key(|entry| std::cmp::Reverse(entry.end.or(entry.start).unwrap_or(i64::MIN)));
        recent.into_iter().take(2)
    }
}
pub struct HistoryUpdate {
    pub state: Arc<HistoryData>,
    pub entries: Option<HistoryEntries>,
    pub prepended: bool,
    pub reset: bool,
    pub batch: Option<BatchId>,
    pub cursor: Cursor,
}

pub struct HistoryEntries {
    pub edits: Vec<ListEdit<Entry>>,
    pub changed_ids: HashSet<String>,
    pub structure_changed: bool,
}

#[cfg(test)]
mod preview_tests {
    use super::*;

    #[test]
    fn highlights_are_recent_completed_executions_not_model_text_or_input() {
        let entry = |id: &str, lane, state: &str| Entry {
            id: id.into(),
            lane,
            action: "exec".into(),
            summary: String::new(),
            start: None,
            end: Some(if id == "done" {
                3
            } else if id == "failed" {
                2
            } else {
                1
            }),
            state: state.into(),
            raw: vec![],
            usage: None,
            model: None,
            outcome_summary: None,
        };
        let mut data = HistoryData {
            entries: vec![
                entry("old", 2, "succeeded"),
                entry("failed", 1, "failed"),
                entry("done", 2, "succeeded"),
                entry("input", 0, "received"),
                entry("model", 1, "succeeded"),
                entry("current", 2, "running"),
            ]
            .into(),
            ..Default::default()
        };
        assert_eq!(
            data.highlights().map(|e| e.id.as_str()).collect::<Vec<_>>(),
            ["done", "failed"]
        );
        data.entries
            .append((0..100).map(|_| entry("model", 1, "succeeded")).collect());
        assert_eq!(
            data.highlights().count(),
            0,
            "a hover must not scan unbounded history"
        );
    }
}
pub struct HistorySubscription {
    source: zork_observe::Subscription<HistoryData, HistoryChange>,
    first: Option<String>,
    prepared: Option<(BatchId, Option<String>)>,
}
impl HistorySubscription {
    pub fn valid(&self, batch: BatchId) -> bool {
        self.source.valid(batch)
    }
    pub fn reset(&mut self) {
        self.prepared = None;
        self.source.reset();
    }
    pub fn readiness(&self) -> Readiness {
        self.source.readiness()
    }
    pub async fn ready(&mut self) -> Result<(), zork_observe::Closed> {
        self.source.ready().await
    }
    pub fn prepare(&mut self) -> Option<HistoryUpdate> {
        let batch = self.source.prepare()?;
        let mut edits = Vec::new();
        let mut changed_ids = HashSet::new();
        let mut structure_changed = false;
        if let Changes::Delta { records, .. } = &batch.changes {
            for record in records {
                for edit in &record.value.edits {
                    ListEdit::push_coalesced(&mut edits, edit.clone());
                }
                changed_ids.extend(record.value.ids.iter().cloned());
                structure_changed |= record.value.structure;
            }
        }
        let state = batch.snapshot.value.clone();
        let first = state.entries.first().map(|entry| entry.id.clone());
        let prepended = first != self.first;
        self.prepared = Some((batch.id, first));
        Some(HistoryUpdate {
            state,
            entries: (!edits.is_empty()).then_some(HistoryEntries {
                edits,
                changed_ids,
                structure_changed,
            }),
            prepended,
            reset: batch.is_reset(),
            batch: Some(batch.id),
            cursor: batch.snapshot.cursor,
        })
    }
    pub fn acknowledge(&mut self, batch: BatchId) -> bool {
        if self.prepared.as_ref().is_none_or(|(id, _)| *id != batch)
            || !self.source.acknowledge(batch)
        {
            return false;
        }
        self.first = self.prepared.take().unwrap().1;
        true
    }
    pub fn discard(&mut self, batch: BatchId) -> bool {
        if !self.source.discard(batch) {
            return false;
        }
        self.prepared = None;
        true
    }
    pub fn snapshot(&mut self) -> HistoryUpdate {
        if let Some(update) = self.prepare() {
            self.acknowledge(update.batch.unwrap());
            return update;
        }
        let state = self.source.snapshot();
        HistoryUpdate {
            state: state.value,
            cursor: state.cursor,
            entries: None,
            prepended: false,
            reset: false,
            batch: None,
        }
    }
    pub async fn changed(&mut self) -> Option<HistoryUpdate> {
        loop {
            self.ready().await.ok()?;
            if let Some(update) = self.prepare() {
                self.acknowledge(update.batch.unwrap());
                return Some(update);
            }
        }
    }
}
struct Pending {
    state: HistoryData,
    refresh: bool,
    older: bool,
    running: bool,
    changes: HistoryChange,
    seen: HashSet<String>,
    projection: Projection,
}
impl Pending {
    fn ingest(&mut self, records: Vec<Record>, older: bool) {
        let incoming = records
            .into_iter()
            .filter(|record| self.seen.insert(record.event_id.clone()))
            .map(Arc::new)
            .collect::<Vec<_>>();
        if incoming.is_empty() {
            return;
        }
        self.changes = self
            .projection
            .ingest(&incoming, older, &mut self.state.entries);
        self.state.lookup = self.projection.lookup();
        let records = List::from_shared(incoming);
        if older {
            self.state.records.splice(0..0, records);
        } else {
            self.state.records.append(records);
        }
    }
}
pub struct History {
    client: Arc<StationClient>,
    session: String,
    owned: Mutex<Pending>,
    state: Source<HistoryData, HistoryChange>,
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}
impl Drop for History {
    fn drop(&mut self) {
        if let Some(task) = self.task.get_mut().unwrap().take() {
            task.abort();
        }
    }
}
impl History {
    /// Exercise the production page reducer without starting network IO.
    #[cfg(any(test, feature = "headless-bench"))]
    pub fn seed_records(&self, records: Vec<Record>, older: bool) {
        let mut owned = self.owned.lock().unwrap();
        owned.ingest(records, older);
        self.publish(&mut owned);
    }

    #[cfg(any(test, feature = "headless-bench"))]
    pub fn seed(&self, mut state: HistoryData) {
        let mut owned = self.owned.lock().unwrap();
        owned.seen = state
            .records
            .iter()
            .map(|record| record.event_id.clone())
            .collect();
        owned.projection = Projection::default();
        let records = (0..state.records.len())
            .map(|i| state.records.shared(i).unwrap())
            .collect::<Vec<_>>();
        state.entries.clear();
        owned.projection.ingest(&records, false, &mut state.entries);
        state.lookup = owned.projection.lookup();
        owned.state = state.clone();
        owned.changes = HistoryChange::default();
        self.state.replace(state);
    }

    pub(super) fn new(client: Arc<StationClient>, session: String) -> Arc<Self> {
        Arc::new(Self {
            client,
            session,
            owned: Mutex::new(Pending {
                state: Default::default(),
                refresh: false,
                older: false,
                running: false,
                changes: HistoryChange::default(),
                seen: HashSet::new(),
                projection: Projection::default(),
            }),
            state: Source::new(HistoryData::default(), JournalLimits::default()),
            task: Mutex::new(None),
        })
    }
    pub fn subscribe(&self) -> HistorySubscription {
        HistorySubscription {
            source: self.state.subscribe(),
            first: None,
            prepared: None,
        }
    }
    fn publish(&self, owned: &mut Pending) {
        let before = self.state.snapshot().value;
        let state = &owned.state;
        let mut topics = Topics::NONE;
        if !owned.changes.edits.is_empty() {
            topics |= Topics::new(1);
        }
        if !before.records.ptr_eq(&state.records) {
            topics |= Topics::new(2);
        }
        if before.runtime != state.runtime
            || before.older != state.older
            || before.latest != state.latest
            || before.loading != state.loading
            || before.loading_older != state.loading_older
            || before.loaded != state.loaded
            || before.error != state.error
            || before.clock_offset_ms != state.clock_offset_ms
            || before.revoked != state.revoked
        {
            topics |= Topics::new(4);
        }
        if topics.is_empty() {
            return;
        }
        let changes = std::mem::take(&mut owned.changes);
        let bytes = changes.bytes();
        self.state
            .publish(owned.state.clone(), changes, topics, bytes);
    }
    pub(super) fn revoke_content(&self) {
        if let Some(task) = self.task.lock().unwrap().take() {
            task.abort();
        }
        let mut owned = self.owned.lock().unwrap();
        if owned.state.revoked {
            return;
        }
        owned.state = HistoryData {
            revoked: true,
            error: Some("设备访问权限已撤销".into()),
            ..Default::default()
        };
        owned.seen.clear();
        owned.projection = Projection::default();
        owned.changes = HistoryChange::default();
        owned.running = false;
        owned.older = false;
        owned.refresh = false;
        self.state.invalidate(owned.state.clone());
    }
    pub fn refresh_if_observed(self: &Arc<Self>) {
        if self.state.observed() {
            self.load(false);
        }
    }
    pub fn load(self: &Arc<Self>, older: bool) {
        {
            let mut owned = self.owned.lock().unwrap();
            if owned.state.revoked {
                return;
            }
            if owned.running {
                if older {
                    owned.older = true
                } else {
                    owned.refresh = true
                };
                return;
            }
            if older && owned.state.older.is_none() {
                return;
            }
            owned.running = true;
            owned.state.loading = true;
            owned.state.loading_older = older;
            self.publish(&mut owned);
        }
        let weak = Arc::downgrade(self);
        *self.task.lock().unwrap() = Some(self.client.spawn(async move {
            let mut older = older;
            loop {
                let Some(history) = weak.upgrade() else {
                    return;
                };
                let cursor = {
                    let owned = history.owned.lock().unwrap();
                    if older {
                        owned.state.older.clone()
                    } else {
                        owned.state.latest.clone()
                    }
                };
                let mut query = reqwest::Url::parse("http://localhost/").unwrap();
                query.query_pairs_mut().append_pair("limit", "100");
                if let Some(cursor) = &cursor {
                    query
                        .query_pairs_mut()
                        .append_pair(if older { "before" } else { "after" }, cursor);
                }
                let path = format!(
                    "/v1/im/sessions/{}/history?{}",
                    history.session,
                    query.query().unwrap()
                );
                let result = history
                    .client
                    .node_request(reqwest::Method::GET, path, None)
                    .await
                    .map_err(|e| e.to_string())
                    .and_then(|v| serde_json::from_value::<Page>(v).map_err(|e| e.to_string()));
                let mut owned = history.owned.lock().unwrap();
                if owned.state.revoked {
                    return;
                }
                match result {
                    Ok(page) => {
                        let initial = !owned.state.loaded;
                        owned.state.clock_offset_ms =
                            page.server_time_ms - crate::store::delivery_now_ms() as i64;
                        owned.state.loaded = true;
                        owned.state.error = None;
                        if older || initial {
                            owned.state.older = page.older_cursor;
                        }
                        if !older {
                            if let Some(latest) = page.latest_cursor {
                                owned.state.latest = Some(latest);
                            }
                        }
                        owned.ingest(page.items, older);
                        if !older && cursor.is_some() && page.has_more {
                            owned.refresh = true;
                        }
                    }
                    Err(error) => owned.state.error = Some(error),
                }
                let next = if std::mem::take(&mut owned.older) {
                    Some(true)
                } else if std::mem::take(&mut owned.refresh) {
                    Some(false)
                } else {
                    None
                };
                owned.running = next.is_some();
                owned.state.loading = owned.running;
                owned.state.loading_older = next == Some(true);
                history.publish(&mut owned);
                let Some(next) = next else {
                    return;
                };
                older = next;
            }
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::FutureExt;

    fn commit(history: &History, records: Vec<Record>, older: bool) {
        let mut owned = history.owned.lock().unwrap();
        owned.ingest(records, older);
        history.publish(&mut owned);
    }

    fn consume(reader: &mut HistorySubscription, mirror: &mut List<Entry>) -> HistoryUpdate {
        let update = reader.prepare().unwrap();
        if update.reset {
            *mirror = update.state.entries.clone();
        } else if let Some(changes) = &update.entries {
            for edit in &changes.edits {
                assert!(edit.apply(mirror));
            }
        }
        assert_eq!(*mirror, update.state.entries);
        assert_eq!(
            mirror.iter().collect::<Vec<_>>(),
            zork_client_types::history::entries_iter(update.state.records.iter())
                .iter()
                .collect::<Vec<_>>()
        );
        assert!(reader.acknowledge(update.batch.unwrap()));
        update
    }

    #[test]
    fn paged_results_move_by_id_and_readers_converge_after_cancel_and_journal_reset() {
        use serde_json::json;
        let history = History::new(
            Arc::new(StationClient::new("http://127.0.0.1:9", None)),
            "pages".into(),
        );
        let records = (0..1000).map(|i| {
            let group = i / 5;
            let at = 1000 - i / 10; // ties plus late starts change timeline positions
            let event = match i % 5 {
                0 => json!({"kind":"selection_changed","selection":{"model":format!("m{group}")}}),
                1 => json!({"kind":"step_started","step_id":format!("s{group}"),"started_at_ms":at}),
                2 => json!({"kind":"tool_result","result":{"invocation_id":format!("c{group}"),"tool":"shell.run","outcome":"succeeded","finished_at_ms":at+10,"data":{"stdout":"done"}}}),
                3 => json!({"kind":"step_completed","step_id":format!("s{group}"),"completed_at_ms":at+5,"invocations":[{"invocation_id":format!("c{group}"),"tool":"shell.run","started_at_ms":at-1,"arguments":{"command":"pwd"}}]}),
                _ => json!({"kind":"input_appended","input":{"content":"input","received_at_ms":at}}),
            };
            Record { event_id: format!("e{i}"), event, metadata: Default::default() }
        }).collect::<Vec<_>>();
        let mut fast = history.subscribe();
        let mut slow = history.subscribe();
        let mut fast_mirror = List::new();
        let mut slow_mirror = List::new();
        consume(&mut fast, &mut fast_mirror);
        consume(&mut slow, &mut slow_mirror);
        let (mut start, mut end) = (497, 513);
        commit(&history, records[start..end].to_vec(), false);
        let pending = slow.prepare().unwrap();
        consume(&mut fast, &mut fast_mirror);
        for round in 0..70 {
            let older = round % 2 == 0;
            let page = if older {
                let before = start.saturating_sub(17);
                let page = records[before..start].to_vec();
                start = before;
                page
            } else {
                let after = (end + 19).min(records.len());
                let page = records[end..after].to_vec();
                end = after;
                page
            };
            if page.is_empty() {
                continue;
            }
            commit(&history, page.clone(), older);
            consume(&mut fast, &mut fast_mirror);
            commit(&history, page, older); // duplicate/overlap never changes a version
            assert!(fast.prepare().is_none());
        }
        assert!(slow.discard(pending.batch.unwrap()));
        consume(&mut slow, &mut slow_mirror);
        assert_eq!(slow_mirror, fast_mirror);
        for i in 0..600 {
            commit(
                &history,
                vec![Record {
                    event_id: format!("burst{i}"),
                    event: json!({"kind":"input_appended","input":{"content":"burst","received_at_ms":2000+i}}),
                    metadata: Default::default(),
                }],
                false,
            );
            let update = fast.snapshot();
            for edit in &update.entries.unwrap().edits {
                assert!(edit.apply(&mut fast_mirror));
            }
            assert!(
                fast_mirror.ptr_eq(&update.state.entries)
                    || fast_mirror.len() == update.state.entries.len()
            );
        }
        assert!(consume(&mut slow, &mut slow_mirror).reset);
        assert_eq!(slow_mirror, fast_mirror);
        assert_eq!(history.owned.lock().unwrap().seen.len(), 1600);
    }

    #[test]
    fn sparse_history_changes_replay_after_cancel_and_revoke_invalidates_in_flight_rows() {
        let history = History::new(
            Arc::new(StationClient::new("http://127.0.0.1:9", None)),
            "test".into(),
        );
        let records = (0..10_000).map(|i| Record { event_id: format!("e{i}"), metadata: Default::default(),
            event: serde_json::json!({"kind":"step_started","step_id":format!("s{i}"),"started_at_ms":i}) }).collect::<Vec<_>>();
        {
            let mut owned = history.owned.lock().unwrap();
            owned.ingest(records, false);
            history.publish(&mut owned);
        }
        let mut reader = history.subscribe();
        let mut signals = reader.readiness();
        signals.changed().now_or_never().unwrap().unwrap();
        let initial = reader.prepare().unwrap();
        let mut mirror = initial.state.entries.clone();
        let unchanged = mirror.shared(4000).unwrap();
        reader.acknowledge(initial.batch.unwrap());
        for step in 0..2 {
            let mut owned = history.owned.lock().unwrap();
            owned.ingest(vec![Record { event_id: format!("completed{step}"), metadata: Default::default(),
                event: serde_json::json!({"kind":"step_completed","step_id":format!("s{}", if step == 0 {41} else {9000}),"completed_at_ms":10001,"assistant_text":"changed"}) }], false);
            history.publish(&mut owned);
        }
        let update = reader.prepare().unwrap();
        let changes = update.entries.as_ref().unwrap();
        assert_eq!(changes.edits.len(), 2);
        assert_eq!(
            changes.edits.iter().map(|e| e.insert.len()).sum::<usize>(),
            2
        );
        assert!(!changes.structure_changed);
        assert!(reader.discard(update.batch.unwrap()));
        let replay = reader.prepare().unwrap();
        for edit in &replay.entries.as_ref().unwrap().edits {
            assert!(edit.apply(&mut mirror));
        }
        assert_eq!(mirror, replay.state.entries);
        assert!(Arc::ptr_eq(&unchanged, &mirror.shared(4000).unwrap()));
        history.revoke_content();
        assert!(!reader.acknowledge(replay.batch.unwrap()));
        let clear = reader.prepare().unwrap();
        assert!(
            clear.reset
                && clear.state.revoked
                && clear.state.entries.is_empty()
                && clear.state.records.is_empty()
        );
        assert!(reader.acknowledge(clear.batch.unwrap()));
    }

    #[test]
    fn runtime_changes_publish_without_changing_the_history_rows() {
        let history = History::new(
            Arc::new(StationClient::new("http://127.0.0.1:9", None)),
            "test".into(),
        );
        let mut subscription = history.subscribe();
        let initial = subscription.snapshot();
        assert!(initial.entries.is_none());
        let mut owned = history.owned.lock().unwrap();
        owned.state = HistoryData {
            runtime: Some(HistoryRuntime {
                model: Some("current-model".into()),
                context_tokens: Some(1234),
                context_limit: Some(256000),
                ..Default::default()
            }),
            ..Default::default()
        };
        history.publish(&mut owned);
        drop(owned);
        let update = subscription.snapshot();
        assert!(update.entries.is_none());
        assert!(!update.prepended);
        let runtime = update.state.runtime.as_ref().unwrap();
        assert_eq!(runtime.context_tokens, Some(1234));
        assert_eq!(runtime.model.as_deref(), Some("current-model"));
    }
}
