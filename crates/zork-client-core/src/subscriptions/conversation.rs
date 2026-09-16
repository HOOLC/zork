use crate::{
    api::AgentStatus,
    conversation::{can_send, project_payload},
    state::{self, ConversationData, Device, Domains},
    store::ClientStore,
    transcript::{Transcript, TranscriptLine},
};
use anyhow::Result;
use serde_json::{json, Value};
use std::sync::Arc;
use zork_observe::{BatchId, List, ListEdit, Readiness};

struct Pending {
    device: Option<BatchId>,
    navigation: Option<BatchId>,
    conversation: Option<BatchId>,
    draft: Option<BatchId>,
    window: Transcript,
    total: usize,
    start: usize,
    anchor: Option<(String, usize)>,
}
pub(super) struct ConversationWire {
    peer: String,
    id: Option<String>,
    device: Arc<Device>,
    store: Arc<ClientStore>,
    conversation: Option<Arc<state::Conversation>>,
    device_updates: state::DeviceSubscription,
    navigation_updates: state::Subscription<state::NavigationData>,
    conversation_updates: Option<state::ConversationSubscription>,
    draft_updates: Option<state::Subscription<state::Draft>>,
    window: Transcript,
    total: usize,
    start: usize,
    anchor: Option<(String, usize)>,
    limit: usize,
    pending: Option<Pending>,
}
impl ConversationWire {
    pub(super) fn new(
        peer: String,
        id: Option<String>,
        device: Arc<Device>,
        store: Arc<ClientStore>,
    ) -> Result<Self> {
        if let Some(id) = &id {
            crate::valid_session(id)?;
        }
        let device_updates = device.subscribe_domains(
            Domains::CONNECTION | Domains::SESSIONS | Domains::AGENTS | Domains::TASKS,
        );
        let conversation = id.as_ref().map(|id| device.conversation(id));
        let navigation_updates = device.navigation();
        let conversation_updates = conversation.as_ref().map(|c| {
            use state::ConversationTopics as T;
            c.subscribe_topics(
                T::MESSAGES | T::ACTIVITY | T::PARTICIPANTS | T::LOADING | T::ARRIVALS,
            )
        });
        let draft_updates = id.as_ref().map(|id| device.subscribe_draft(id));
        Ok(Self {
            peer,
            id,
            device,
            store,
            conversation,
            device_updates,
            navigation_updates,
            conversation_updates,
            draft_updates,
            window: Transcript::new(),
            total: 0,
            start: 0,
            anchor: None,
            limit: 100,
            pending: None,
        })
    }
    pub(super) fn signals(&self) -> Vec<Readiness> {
        let mut signals = vec![
            self.device_updates.readiness(),
            self.navigation_updates.readiness(),
        ];
        if let Some(updates) = &self.conversation_updates {
            signals.push(updates.readiness());
        }
        if let Some(updates) = &self.draft_updates {
            signals.push(updates.readiness());
        }
        signals
    }
    pub(super) fn prepare(&mut self) -> Result<Option<(Value, bool, bool)>> {
        debug_assert!(self.pending.is_none());
        // Sending/withdrawing publishes transcript and draft under this same
        // gate. Capture roots here, then release it before DTO conversion.
        let (device, navigation, conversation, draft) = {
            let _transaction = self.device.draft_gate.lock().unwrap();
            (
                self.device_updates.prepare(),
                self.navigation_updates.prepare(),
                self.conversation_updates.as_mut().and_then(|s| s.prepare()),
                self.draft_updates.as_mut().and_then(|s| s.prepare()),
            )
        };
        if device.is_none() && navigation.is_none() && conversation.is_none() && draft.is_none() {
            return Ok(None);
        }
        let revoked = self.device.snapshot().revoked
            || conversation.as_ref().is_some_and(|c| c.state.revoked);
        let reset = revoked || conversation.as_ref().is_some_and(|c| c.reset);
        let mut value = json!({"peer":self.peer, "session":self.id, "unified_transcript":true});
        if let Some(navigation) = &navigation {
            value["navigation"] = json!(navigation.snapshot.value);
        }
        if let Some(update) = &device {
            let d = &update.state;
            if update.reset {
                value["nodes"] = json!(self.store.nodes()?);
            }
            if update.domains.contains(Domains::SESSIONS) {
                let summary = self
                    .id
                    .as_ref()
                    .and_then(|id| d.sessions.iter().find(|s| &s.session_id == id));
                value["can_send"] = json!(summary.is_some_and(can_send));
                value["can_stop"] = json!(summary.is_some_and(crate::composer::can_stop));
            }
            if self.id.is_none() {
                value["connected"] = json!(d.online == Some(true));
                value["error"] = json!(d.connection_error);
            }
        }
        let mut window = self.window.clone();
        let mut total = self.total;
        let mut start = self.start;
        let mut anchor = self.anchor.clone();
        if let Some(update) = &conversation {
            let c = &update.state;
            total = c.lines.len();
            let range = self.range(c);
            start = range.start;
            if self.anchor.is_some() && !c.loading_older {
                anchor = c
                    .lines
                    .get(start)
                    .and_then(message_id)
                    .map(|id| (id.to_owned(), 0));
            }
            if update.activity_changed {
                value["activity"] = json!(c.activity);
                value["stop_pending"] = json!(c.stop_pending);
                value["running"] = json!(matches!(
                    c.activity.as_ref(),
                    Some(
                        AgentStatus::Live { .. }
                            | AgentStatus::Thinking
                            | AgentStatus::ToolsStarted { .. }
                            | AgentStatus::ToolsWaiting { .. }
                            | AgentStatus::Waiting { .. }
                    )
                ));
            }
            if update.participants_changed {
                value["participants"] = json!(c.participants);
            }
            if update.loading_changed {
                value["connected"] = json!(c.connected);
                value["error"] = json!(c.error);
                value["loaded"] = json!(c.loaded);
                value["loading_older"] = json!(c.loading_older);
            }
            if let Some(edits) = &update.messages {
                if update.reset {
                    window = c.lines.slice(range.clone());
                    value["messages"] =
                        json!(window.iter().map(|line| row(line, c)).collect::<Vec<_>>());
                } else {
                    let (next, edits) =
                        project(&self.window, self.start, edits, &c.lines, range.clone());
                    window = next;
                    value["message_edits"] = json!(edits
                        .iter()
                        .map(
                            |edit| json!({"start":edit.remove.start, "end":edit.remove.end,
                        "insert":edit.insert.iter().map(|line| row(line, c)).collect::<Vec<_>>()})
                        )
                        .collect::<Vec<_>>());
                }
            }
            if update.loading_changed || update.messages.is_some() {
                value["older_cursor"] = if start > 0 {
                    json!("window")
                } else {
                    json!(c.older_cursor)
                };
                value["message_total"] = json!(total);
                value["window_start"] = json!(start);
                value["newer_available"] = json!(range.end < total);
            }
            if update.message_arrivals.count > 0 {
                value["message_arrivals"] = json!(update.message_arrivals);
            }
        }
        if let Some(draft) = &draft {
            value["draft_document"] = json!(draft.snapshot.value);
        }
        if revoked {
            window.clear();
            total = 0;
            start = 0;
            anchor = None;
            value = json!({"peer":self.peer,"session":self.id,"unified_transcript":true,
                "navigation":state::NavigationData::default(),
                "revoked":true,"messages":[],"agents":[],"sessions":[],"tasks_by_leader":{},"participants":[],
                "connected":false,"running":false,"can_send":false,"can_stop":false,"activity":null,
                "draft_document":state::Draft::default(),"older_cursor":null,"newer_available":false,"loading_older":false,"error":"设备访问权限已撤销"});
        }
        self.pending = Some(Pending {
            device: device.and_then(|u| u.batch),
            navigation: navigation.map(|u| u.id),
            conversation: conversation.and_then(|u| u.batch),
            draft: draft.map(|u| u.id),
            window,
            total,
            start,
            anchor,
        });
        Ok(Some((json!({"state":value}), reset, revoked)))
    }
    pub(super) fn valid(&self) -> bool {
        self.pending.as_ref().is_some_and(|p| {
            p.device.is_none_or(|id| self.device_updates.valid(id))
                && p.navigation
                    .is_none_or(|id| self.navigation_updates.valid(id))
                && p.conversation
                    .is_none_or(|id| self.conversation_updates.as_ref().unwrap().valid(id))
                && p.draft
                    .is_none_or(|id| self.draft_updates.as_ref().unwrap().valid(id))
        })
    }
    pub(super) fn finish(&mut self, applied: bool) -> bool {
        let Some(pending) = self.pending.take() else {
            return false;
        };
        let mut valid = true;
        if let Some(id) = pending.navigation {
            valid &= if applied {
                self.navigation_updates.acknowledge(id)
            } else {
                self.navigation_updates.discard(id)
            };
        }
        if let Some(id) = pending.device {
            valid &= if applied {
                self.device_updates.acknowledge(id)
            } else {
                self.device_updates.discard(id)
            };
        }
        if let Some(id) = pending.conversation {
            let s = self.conversation_updates.as_mut().unwrap();
            valid &= if applied {
                s.acknowledge(id)
            } else {
                s.discard(id)
            };
        }
        if let Some(id) = pending.draft {
            let s = self.draft_updates.as_mut().unwrap();
            valid &= if applied {
                s.acknowledge(id)
            } else {
                s.discard(id)
            };
        }
        if applied && valid {
            self.window = pending.window;
            self.total = pending.total;
            self.start = pending.start;
            self.anchor = pending.anchor;
        }
        if !valid {
            self.navigation_updates.reset();
            self.device_updates.reset();
            if let Some(s) = &mut self.conversation_updates {
                s.reset();
            }
            if let Some(s) = &mut self.draft_updates {
                s.reset();
            }
        }
        valid
    }
    pub(super) fn older(&mut self) {
        self.limit = self.limit.saturating_add(100);
        self.anchor = self
            .window
            .first()
            .and_then(message_id)
            .map(|id| (id.to_owned(), 100));
        if let Some(conversation) = &self.conversation {
            if self.start == 0 {
                conversation.load_older();
            }
        }
        if let Some(updates) = &mut self.conversation_updates {
            updates.reset();
        }
    }
    pub(super) fn newer(&mut self) {
        if self.anchor.is_none() {
            return;
        }
        self.limit = self.limit.saturating_add(100);
        if let Some(updates) = &mut self.conversation_updates {
            updates.reset();
        }
    }
    pub(super) fn window_anchor(&mut self, anchor: Option<String>) -> Result<()> {
        if self
            .anchor
            .as_ref()
            .map(|(id, before)| (id.as_str(), *before))
            == anchor.as_deref().map(|id| (id, 0))
        {
            return Ok(());
        }
        if let (Some(id), Some(conversation)) = (&anchor, &self.conversation) {
            anyhow::ensure!(
                conversation.snapshot().lookup.index_of(id).is_some(),
                "消息已移出当前范围，请重试"
            );
        }
        self.anchor = anchor.map(|id| (id, 0));
        if let Some(updates) = &mut self.conversation_updates {
            updates.reset();
        }
        Ok(())
    }
    fn range(&self, state: &ConversationData) -> std::ops::Range<usize> {
        if let Some((id, before)) = &self.anchor {
            // A deleted anchor moves to the next surviving displayed record.
            let start = state
                .lookup
                .index_of(id)
                .map(|at| at.saturating_sub(*before))
                .or_else(|| {
                    self.window
                        .iter()
                        .filter_map(message_id)
                        .find_map(|id| state.lookup.index_of(id))
                });
            if let Some(start) = start {
                return start..start.saturating_add(self.limit).min(state.lines.len());
            }
        }
        state.lines.len().saturating_sub(self.limit)..state.lines.len()
    }
}

fn message_id(line: &TranscriptLine) -> Option<&str> {
    let TranscriptLine::Message { metadata, .. } = line;
    metadata.id.as_deref()
}

fn row(line: &TranscriptLine, state: &ConversationData) -> Value {
    let TranscriptLine::Message {
        role,
        content,
        metadata,
    } = line;
    let mut value = json!({"type":"message", "role":role, "content":content});
    value.as_object_mut().unwrap().extend(
        serde_json::to_value(metadata)
            .unwrap()
            .as_object()
            .unwrap()
            .clone(),
    );
    if let Some(delivery) = metadata.id.as_ref().and_then(|id| state.deliveries.get(id)) {
        value["pending"] = json!(true);
        value["request_id"] = json!(delivery.request_id);
        value["attempted"] = json!(delivery.attempted);
        value["delivery_status"] = json!(delivery.status);
        value["delivery_error"] = json!(delivery.error);
    }
    project_payload(&mut value);
    if let Some(card) = &metadata.interaction_view {
        value["interaction_card"] = json!(card);
    }
    value
}

/// Mechanically clip ordered producer splices to an anchored or tail window. Ordinary
/// appends encode one row plus one removal, regardless of loaded history size.
pub(super) fn project<T>(
    window: &List<T>,
    mut offset: usize,
    changes: &[ListEdit<T>],
    current: &List<T>,
    range: std::ops::Range<usize>,
) -> (List<T>, Vec<ListEdit<T>>) {
    let previous_len = window.len();
    let mut window = window.clone();
    let mut edits = Vec::new();
    fn stage<T>(window: &mut List<T>, edits: &mut Vec<ListEdit<T>>, edit: ListEdit<T>) {
        assert!(edit.apply(window));
        ListEdit::push_coalesced(edits, edit);
    }
    for change in changes {
        if change.remove.end <= offset {
            offset = offset + change.insert.len() - change.remove.len();
            continue;
        }
        if change.remove.start >= offset + window.len() {
            continue;
        }
        let start = change.remove.start.saturating_sub(offset);
        let end = change.remove.end.saturating_sub(offset).min(window.len());
        if change.remove.start < offset {
            offset = change.remove.start;
        }
        let edit = ListEdit {
            remove: start..end,
            insert: change.insert.clone(),
        };
        stage(&mut window, &mut edits, edit);
    }
    if window.is_empty() || range.start >= offset + window.len() || range.end <= offset {
        let window = current.slice(range);
        let edit = ListEdit {
            remove: 0..previous_len,
            insert: window.clone(),
        };
        return (window, vec![edit]);
    }
    if range.start > offset {
        let count = range.start - offset;
        stage(
            &mut window,
            &mut edits,
            ListEdit {
                remove: 0..count,
                insert: List::new(),
            },
        );
        offset = range.start;
    }
    if offset + window.len() > range.end {
        let remove = range.end - offset..window.len();
        stage(
            &mut window,
            &mut edits,
            ListEdit {
                remove,
                insert: List::new(),
            },
        );
    }
    if offset > range.start {
        let edit = ListEdit {
            remove: 0..0,
            insert: current.slice(range.start..offset),
        };
        stage(&mut window, &mut edits, edit);
        offset = range.start;
    }
    if offset + window.len() < range.end {
        let edit = ListEdit {
            remove: window.len()..window.len(),
            insert: current.slice(offset + window.len()..range.end),
        };
        stage(&mut window, &mut edits, edit);
    }
    // A burst larger than the subscribed window need not cross the wire first
    // merely to be removed by a later splice in this same batch.
    if edits.iter().map(|e| e.insert.len()).sum::<usize>() > range.len().saturating_mul(2) {
        edits = vec![ListEdit {
            remove: 0..previous_len,
            insert: window.clone(),
        }];
    }
    (window, edits)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn window_splices_match_authority_through_sparse_edits_and_large_bursts() {
        let mut full: List<_> = (0..100_000).collect();
        let mut mirror = full.slice(full.len() - 100..full.len());
        let mut random = 17_u64;
        let mut offset = full.len() - 100;
        for turn in 0..1000 {
            let mut edits = Vec::new();
            for n in 0..4 {
                random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
                let start = match n {
                    0 => full.len(),
                    1 => random as usize % (full.len() + 1),
                    _ => full.len().saturating_sub(random as usize % 180),
                };
                let end = (start + (random >> 24) as usize % 5).min(full.len());
                let count = if turn % 50 == 0 {
                    300
                } else {
                    random as usize % 5
                };
                let edit = ListEdit {
                    remove: start..end,
                    insert: (0..count).map(|i| turn * 1000 + i).collect(),
                };
                assert!(edit.apply(&mut full));
                edits.push(edit);
            }
            let start = if turn % 3 == 0 {
                full.len() / 2
            } else {
                full.len().saturating_sub(100)
            };
            let range = start..(start + 100).min(full.len());
            let (next, edits) = project(&mirror, offset, &edits, &full, range.clone());
            offset = start;
            for edit in edits {
                assert!(edit.apply(&mut mirror));
            }
            assert_eq!(mirror, next);
            assert_eq!(mirror, full.slice(range));
        }
    }
}
