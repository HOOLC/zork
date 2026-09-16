//! Mutable indexes belong to the controller. Published transcript roots share
//! immutable records; prepending/removing history never renumbers all IDs.
use super::{ConversationData, DeliveryState};
use crate::{
    api::TranscriptMessage,
    transcript::{transcript_line_from, TranscriptLine},
};
use std::{
    collections::{HashMap, HashSet},
    ops::{Deref, DerefMut},
    sync::Arc,
};
use zork_observe::{List, ListEdit};

/// Read-only ID lookup captured in the same version as the transcript root.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TranscriptLookup {
    by_id: imbl::HashMap<String, i64>,
    positions: imbl::Vector<i64>,
}

impl TranscriptLookup {
    pub fn len(&self) -> usize {
        self.positions.len()
    }
    pub fn is_empty(&self) -> bool {
        self.positions.is_empty()
    }
    pub fn index_of(&self, id: &str) -> Option<usize> {
        self.by_id
            .get(id)
            .and_then(|position| self.positions.binary_search(position).ok())
    }
}

pub(super) struct Owned {
    pub data: ConversationData,
    pub bytes: usize,
    pub edits: Vec<ListEdit<TranscriptLine>>,
    pub replaced: bool,
    pub cache_backed: bool,
    /// Oldest consumed source record, including folded interaction results.
    pub source_head: Option<String>,
    pending_interactions: HashMap<String, crate::interactions::ResultEvent>,
    pub interaction_submissions: HashMap<String, crate::interactions::Submission>,
    interaction_errors: HashMap<String, std::collections::BTreeMap<String, String>>,
    pub(super) login_views: HashMap<String, crate::interactions::LoginView>,
    anonymous: HashSet<i64>,
    front: i64,
    back: i64,
}

impl Deref for Owned {
    type Target = ConversationData;
    fn deref(&self) -> &Self::Target {
        &self.data
    }
}
impl DerefMut for Owned {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.data
    }
}

pub(super) fn row_bytes(line: &TranscriptLine) -> usize {
    let TranscriptLine::Message {
        content, metadata, ..
    } = line;
    std::mem::size_of::<TranscriptLine>()
        + 4 * std::mem::size_of::<usize>()
        + content.capacity()
        + [
            &metadata.id,
            &metadata.source_epoch,
            &metadata.created_at,
            &metadata.author_agent_id,
            &metadata.author_name,
            &metadata.author_avatar,
            &metadata.device,
            &metadata.chat_id,
            &metadata.reply_to,
        ]
        .into_iter()
        .flatten()
        .map(String::capacity)
        .sum::<usize>()
        + metadata.interaction.as_deref().map_or(0, json_bytes)
        + metadata.interaction_result.as_ref().map_or(0, |r| {
            std::mem::size_of::<crate::interactions::Resolution>()
                + r.request_message_id.capacity()
                + r.response_id.capacity()
                + r.actor.capacity()
                + json_bytes(&r.output)
        })
        + metadata
            .interaction_view
            .as_ref()
            .map_or(0, |card| card.estimated_bytes())
        + metadata
            .id
            .as_ref()
            .map_or(0, |id| id.len() + std::mem::size_of::<String>())
}

impl Owned {
    pub fn new(mut data: ConversationData) -> Self {
        data.lookup = TranscriptLookup::default();
        let source_head = data.lines.first().and_then(|line| {
            let TranscriptLine::Message { metadata, .. } = line;
            metadata.id.clone()
        });
        let mut owned = Self {
            data,
            bytes: 0,
            edits: Vec::new(),
            replaced: false,
            cache_backed: false,
            source_head,
            pending_interactions: HashMap::new(),
            interaction_submissions: HashMap::new(),
            interaction_errors: HashMap::new(),
            login_views: HashMap::new(),
            anonymous: HashSet::new(),
            front: -1,
            back: 0,
        };
        for (index, line) in owned.data.lines.iter().enumerate() {
            let position = i64::try_from(index).expect("transcript position exhausted");
            owned.data.lookup.positions.push_back(position);
            let TranscriptLine::Message { metadata, .. } = line;
            if let Some(id) = &metadata.id {
                owned.data.lookup.by_id.insert(id.clone(), position);
            } else {
                owned.anonymous.insert(position);
            }
            owned.bytes += row_bytes(line);
        }
        owned.back = i64::try_from(owned.data.lines.len()).expect("transcript position exhausted");
        owned
    }

    #[cfg(any(test, feature = "headless-bench"))]
    pub fn replace(&mut self, data: ConversationData) {
        let cache_backed = self.cache_backed;
        *self = Self::new(data);
        self.cache_backed = cache_backed;
        self.replaced = true;
    }

    pub fn clear_messages(&mut self) {
        self.data.lines = List::new();
        self.data.lookup = TranscriptLookup::default();
        self.data.deliveries.clear();
        self.source_head = None;
        self.anonymous = HashSet::new();
        self.pending_interactions.clear();
        self.interaction_submissions.clear();
        self.interaction_errors.clear();
        self.login_views.clear();
        self.front = -1;
        self.back = 0;
        self.bytes = 0;
        self.edits.clear();
        self.replaced = true;
    }

    pub fn index_of(&self, id: &str) -> Option<usize> {
        self.data
            .lookup
            .by_id
            .get(id)
            .and_then(|position| self.data.lookup.positions.binary_search(position).ok())
    }

    fn matching(&self, line: &TranscriptLine, legacy: bool) -> Option<usize> {
        let TranscriptLine::Message {
            role,
            content,
            metadata,
        } = line;
        if let Some(id) = &metadata.id {
            if let Some(index) = self.index_of(id) {
                return Some(index);
            }
            if !legacy {
                return None;
            }
            // Old cached anonymous rows can acquire a gateway identity from a
            // history page. Modern ID-bearing records never need this scan.
            return self
                .anonymous
                .iter()
                .filter_map(|position| {
                    let index = self.data.lookup.positions.binary_search(position).ok()?;
                    let TranscriptLine::Message {
                        role: old_role,
                        content: old_content,
                        ..
                    } = &self.data.lines[index];
                    (old_role == role && old_content == content).then_some(index)
                })
                .min();
        }
        if !legacy {
            return None;
        }
        self.data.lines.iter().position(|old| {
            matches!(old, TranscriptLine::Message {
            role: old_role, content: old_content, ..
        } if role == old_role && content == old_content)
        })
    }

    fn register(&mut self, position: i64, line: &TranscriptLine) {
        let TranscriptLine::Message { metadata, .. } = line;
        if let Some(id) = &metadata.id {
            self.data.lookup.by_id.insert(id.clone(), position);
        } else {
            self.anonymous.insert(position);
        }
    }

    fn unregister(&mut self, position: i64, line: &TranscriptLine) {
        let TranscriptLine::Message { metadata, .. } = line;
        if let Some(id) = &metadata.id {
            self.data.lookup.by_id.remove(id);
        } else {
            self.anonymous.remove(&position);
        }
    }

    fn insert(&mut self, line: Arc<TranscriptLine>, front: bool) {
        let index = if front { 0 } else { self.data.lines.len() };
        let position = if front {
            let position = self.front;
            self.front = self
                .front
                .checked_sub(1)
                .expect("transcript position exhausted");
            self.data.lookup.positions.push_front(position);
            self.data.lines.push_front_shared(line.clone());
            position
        } else {
            let position = self.back;
            self.back = self
                .back
                .checked_add(1)
                .expect("transcript position exhausted");
            self.data.lookup.positions.push_back(position);
            self.data.lines.push_shared(line.clone());
            position
        };
        self.register(position, &line);
        self.bytes += row_bytes(&line);
        ListEdit::push_coalesced(
            &mut self.edits,
            ListEdit {
                remove: index..index,
                insert: List::from_shared([line]),
            },
        );
    }

    fn update(&mut self, index: usize, line: Arc<TranscriptLine>) {
        let old = self.data.lines.shared(index).unwrap();
        if old == line {
            return;
        }
        let position = self.data.lookup.positions[index];
        self.unregister(position, &old);
        self.register(position, &line);
        self.bytes = self.bytes.saturating_sub(row_bytes(&old)) + row_bytes(&line);
        self.data.lines.set_shared(index, line.clone());
        ListEdit::push_coalesced(
            &mut self.edits,
            ListEdit {
                remove: index..index + 1,
                insert: List::from_shared([line]),
            },
        );
    }

    /// Returns true only for a new row; an echo can update metadata without
    /// creating another arrival, and an identical retry produces no edit.
    pub fn delivered(&mut self, line: TranscriptLine) -> bool {
        let TranscriptLine::Message { metadata, .. } = &line;
        if let Some(id) = &metadata.id {
            self.set_delivery(id, None);
        }
        let line = Arc::new(line);
        if let Some(index) = self.matching(&line, false) {
            self.update(index, line);
            false
        } else {
            self.insert(line, false);
            true
        }
    }

    pub fn remove_id(&mut self, id: &str) {
        self.data.deliveries.remove(id);
        let Some(index) = self.index_of(id) else {
            return;
        };
        let old = self.data.lines.remove(index);
        let position = self.data.lookup.positions.remove(index);
        self.unregister(position, &old);
        self.bytes = self.bytes.saturating_sub(row_bytes(&old));
        ListEdit::push_coalesced(
            &mut self.edits,
            ListEdit {
                remove: index..index + 1,
                insert: List::new(),
            },
        );
    }

    pub fn prepend(&mut self, items: &[TranscriptMessage]) {
        for line in self.project_items(items).into_iter().rev() {
            let TranscriptLine::Message { metadata, .. } = &line;
            if let Some(id) = &metadata.id {
                self.set_delivery(id, None);
            }
            if self.matching(&line, true).is_none() {
                self.insert(Arc::new(line), true);
            }
        }
    }

    pub fn merge(&mut self, items: &[TranscriptMessage], arrival_start: usize) {
        let arrival_ids: HashSet<_> = items
            .iter()
            .skip(arrival_start)
            .filter_map(|item| {
                let TranscriptMessage::Message { metadata, .. } = item;
                metadata.id.as_deref()
            })
            .collect();
        let incoming = self
            .project_items(items)
            .into_iter()
            .map(Arc::new)
            .collect::<Vec<_>>();
        for line in &incoming {
            let TranscriptLine::Message { metadata, .. } = line.as_ref();
            if let Some(id) = &metadata.id {
                self.set_delivery(id, None);
            }
        }
        let prefix = incoming
            .iter()
            .position(|line| self.matching(line, true).is_some())
            .unwrap_or(0);
        let mut arrivals = HashSet::new();
        for line in &incoming {
            let TranscriptLine::Message { metadata, .. } = line.as_ref();
            if let Some(id) = &metadata.id {
                if arrival_ids.contains(id.as_str())
                    && !self.data.lookup.by_id.contains_key(id)
                    && arrivals.insert(id.clone())
                {
                    self.data.message_activity.record(id.clone());
                }
            }
        }
        for line in incoming[..prefix].iter().rev() {
            if let Some(index) = self.matching(line, true) {
                self.update(index, line.clone());
            } else {
                self.insert(line.clone(), true);
            }
        }
        for line in incoming.into_iter().skip(prefix) {
            if let Some(index) = self.matching(&line, true) {
                self.update(index, line);
            } else {
                self.insert(line, false);
            }
        }
    }

    pub fn set_delivery(&mut self, id: &str, delivery: Option<DeliveryState>) {
        if self.data.deliveries.get(id) == delivery.as_ref() {
            return;
        }
        match delivery {
            Some(delivery) => {
                self.data.deliveries.insert(id.into(), delivery);
            }
            None => {
                self.data.deliveries.remove(id);
            }
        }
        if let Some(index) = self.index_of(id) {
            ListEdit::push_coalesced(
                &mut self.edits,
                ListEdit {
                    remove: index..index + 1,
                    insert: List::from_shared([self.data.lines.shared(index).unwrap()]),
                },
            );
        }
    }

    pub fn delivered_message(&mut self, message: &TranscriptMessage) -> bool {
        self.project_items(std::slice::from_ref(message))
            .into_iter()
            .any(|line| self.delivered(line))
    }

    pub fn update_cached_interactions(&mut self, items: &[TranscriptMessage]) {
        for message in items {
            let TranscriptMessage::Message { metadata, .. } = message;
            if let Some(index) = metadata.id.as_deref().and_then(|id| self.index_of(id)) {
                if let Some(mut line) = transcript_line_from(message) {
                    self.decorate_interaction(&mut line);
                    self.update(index, Arc::new(line));
                }
            }
        }
    }

    fn project_items(&mut self, items: &[TranscriptMessage]) -> Vec<TranscriptLine> {
        for message in items {
            let TranscriptMessage::Message { metadata, .. } = message;
            let Some(result) = crate::interactions::result_event(metadata) else {
                continue;
            };
            if let Some(index) = self.index_of(&result.result.request_message_id) {
                let mut line = self.lines[index].clone();
                let TranscriptLine::Message { metadata, .. } = &mut line;
                match crate::interactions::merge_result(metadata, &result) {
                    Ok(true) => {
                        self.decorate_interaction(&mut line);
                        self.update(index, Arc::new(line));
                    }
                    Err(error) => self.error = Some(error.to_string()),
                    _ => {}
                }
            } else if !self.cache_backed {
                // In-memory fixtures use the same reducer; production defers
                // off-screen roots to the durable cache instead of retaining
                // an unbounded duplicate of those results in this controller.
                let old = self
                    .pending_interactions
                    .get(&result.result.request_message_id);
                if old.is_some_and(|old| {
                    old.handler != result.handler || old.request_id != result.request_id
                }) {
                    self.error = Some("interaction_business_mismatch".into());
                    continue;
                }
                if old.is_none_or(|old| old.result.revision < result.result.revision) {
                    self.pending_interactions
                        .insert(result.result.request_message_id.clone(), result);
                }
            }
        }
        items
            .iter()
            .filter_map(|message| {
                let mut line = transcript_line_from(message)?;
                let TranscriptLine::Message { metadata, .. } = &mut line;
                if crate::interactions::request(metadata).is_some() {
                    if let Some(id) = metadata.id.clone() {
                        let result = self.pending_interactions.remove(&id).or_else(|| {
                            let old = &self.lines[self.index_of(&id)?];
                            let TranscriptLine::Message { metadata, .. } = old;
                            crate::interactions::cached_result(metadata)
                        });
                        if let Some(result) = result {
                            if let Err(error) = crate::interactions::merge_result(metadata, &result)
                            {
                                self.error = Some(error.to_string());
                            }
                        }
                    }
                }
                self.decorate_interaction(&mut line);
                Some(line)
            })
            .collect()
    }

    fn decorate_interaction(&self, line: &mut TranscriptLine) {
        let TranscriptLine::Message { metadata, .. } = line;
        let Some(id) = metadata.id.as_ref() else {
            return;
        };
        metadata.interaction_view = crate::interactions::view(
            metadata,
            self.interaction_submissions.get(id),
            self.interaction_errors
                .get(id)
                .unwrap_or(&Default::default()),
        );
        if let Some(view) = self.login_views.get(id) {
            crate::interactions::provider_login::decorate(metadata, view);
        }
    }

    pub fn set_interaction_errors(
        &mut self,
        id: &str,
        errors: std::collections::BTreeMap<String, String>,
    ) {
        if errors.is_empty() {
            self.interaction_errors.remove(id);
        } else {
            self.interaction_errors.insert(id.into(), errors);
        }
        self.refresh_interaction(id);
    }

    pub(super) fn set_login_view(
        &mut self,
        id: &str,
        view: Option<crate::interactions::LoginView>,
    ) {
        if self.login_views.get(id) == view.as_ref() {
            return;
        }
        if let Some(view) = view {
            self.login_views.insert(id.into(), view);
        } else {
            self.login_views.remove(id);
        }
        self.refresh_interaction(id);
    }

    pub fn set_interaction_submissions(
        &mut self,
        submissions: HashMap<String, crate::interactions::Submission>,
    ) {
        let mut ids = HashSet::new();
        for (id, value) in &submissions {
            if self.interaction_submissions.get(id) != Some(value) {
                ids.insert(id.clone());
            }
        }
        for id in self.interaction_submissions.keys() {
            if !submissions.contains_key(id) {
                ids.insert(id.clone());
            }
        }
        self.interaction_submissions = submissions;
        for id in ids {
            self.refresh_interaction(&id);
        }
    }

    fn refresh_interaction(&mut self, id: &str) {
        if let Some(index) = self.index_of(id) {
            let mut line = self.lines[index].clone();
            self.decorate_interaction(&mut line);
            self.update(index, Arc::new(line));
        }
    }
}

fn json_bytes(value: &serde_json::Value) -> usize {
    std::mem::size_of::<serde_json::Value>()
        + match value {
            serde_json::Value::String(s) => s.capacity(),
            serde_json::Value::Array(v) => v.iter().map(json_bytes).sum(),
            serde_json::Value::Object(v) => {
                v.iter().map(|(k, v)| k.capacity() + json_bytes(v)).sum()
            }
            _ => 0,
        }
}
