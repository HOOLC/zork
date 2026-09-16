use super::change::HistoryChange;
use std::sync::Arc;
use zork_client_types::history::{Entry, EntryOrder, Ledger, ProjectedEntry, Record};
use zork_observe::{List, ListEdit};

/// The writer's order index shares the same positions as the published list.
/// Persistent sequence edits avoid renumbering every entry after a prepend or
/// a late start that moves an orphan result earlier in the timeline.
#[derive(Default)]
pub(super) struct Projection {
    ledger: Ledger,
    order: List<EntryOrder>,
    by_id: imbl::HashMap<String, EntryOrder>,
}

/// Immutable ID lookup captured with the entries in a published history version.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HistoryLookup {
    order: List<EntryOrder>,
    by_id: imbl::HashMap<String, EntryOrder>,
}
impl HistoryLookup {
    pub fn index_of(&self, id: &str) -> Option<usize> {
        let key = self.by_id.get(id)?;
        let (mut low, mut high) = (0, self.order.len());
        while low < high {
            let mid = low + (high - low) / 2;
            if self.order[mid] < *key {
                low = mid + 1;
            } else {
                high = mid;
            }
        }
        (self.order.get(low) == Some(key)).then_some(low)
    }
}

impl Projection {
    pub(super) fn lookup(&self) -> HistoryLookup {
        HistoryLookup {
            order: self.order.clone(),
            by_id: self.by_id.clone(),
        }
    }
    pub fn ingest(
        &mut self,
        records: &[Arc<Record>],
        older: bool,
        entries: &mut List<Entry>,
    ) -> HistoryChange {
        let updates = self.ledger.ingest(records, older);
        let mut changes = HistoryChange::default();
        if self.order.is_empty() {
            let mut inserted = List::new();
            for ProjectedEntry { order, entry } in updates {
                changes.ids.insert(entry.id.clone());
                self.by_id.insert(entry.id.clone(), order);
                self.order.push(order);
                inserted.push(entry);
            }
            if !inserted.is_empty() {
                changes.structure = true;
                Self::edit(
                    entries,
                    &mut changes,
                    ListEdit {
                        remove: 0..0,
                        insert: inserted,
                    },
                );
            }
            return changes;
        }
        let mut updates = updates.into_iter().peekable();
        while let Some(ProjectedEntry { order, entry }) = updates.next() {
            if !self.by_id.contains_key(&entry.id) {
                let position = self.position(order);
                let upper = self.order.get(position).copied();
                let mut keys = List::new();
                let mut rows = List::new();
                let mut next = Some(ProjectedEntry { order, entry });
                while let Some(ProjectedEntry { order, entry }) = next {
                    changes.ids.insert(entry.id.clone());
                    self.by_id.insert(entry.id.clone(), order);
                    keys.push(order);
                    rows.push(entry);
                    next = if updates.peek().is_some_and(|row| {
                        !self.by_id.contains_key(&row.entry.id)
                            && upper.is_none_or(|upper| row.order < upper)
                    }) {
                        updates.next()
                    } else {
                        None
                    };
                }
                // A contiguous page is one tree splice, rather than a tree
                // split/join for every row in the page.
                self.order.splice(position..position, keys);
                changes.structure = true;
                Self::edit(
                    entries,
                    &mut changes,
                    ListEdit {
                        remove: position..position,
                        insert: rows,
                    },
                );
                continue;
            }
            if let Some(previous) = self.by_id.get(&entry.id).copied() {
                let before = self.position(previous);
                let after = self.position(order);
                let after = after - usize::from(after > before);
                if after == before {
                    // The stable tie-breaker can change when a start is loaded
                    // from an earlier page, without moving the displayed row.
                    self.order.set_shared(before, Arc::new(order));
                    self.by_id.insert(entry.id.clone(), order);
                    if entries[before] == entry {
                        continue;
                    }
                    changes.ids.insert(entry.id.clone());
                    Self::edit(
                        entries,
                        &mut changes,
                        ListEdit {
                            remove: before..before + 1,
                            insert: vec![entry].into(),
                        },
                    );
                    continue;
                }
                self.order.remove(before);
                Self::edit(
                    entries,
                    &mut changes,
                    ListEdit {
                        remove: before..before + 1,
                        insert: List::new(),
                    },
                );
            }
            let position = self.position(order);
            changes.structure = true;
            changes.ids.insert(entry.id.clone());
            self.by_id.insert(entry.id.clone(), order);
            self.order.splice(position..position, vec![order].into());
            Self::edit(
                entries,
                &mut changes,
                ListEdit {
                    remove: position..position,
                    insert: vec![entry].into(),
                },
            );
        }
        changes
    }

    fn position(&self, key: EntryOrder) -> usize {
        if self.order.last().is_some_and(|last| *last < key) {
            return self.order.len();
        }
        if self.order.first().is_some_and(|first| *first >= key) {
            return 0;
        }
        let mut low = 0;
        let mut high = self.order.len();
        while low < high {
            let mid = low + (high - low) / 2;
            if self.order[mid] < key {
                low = mid + 1;
            } else {
                high = mid;
            }
        }
        low
    }

    fn edit(entries: &mut List<Entry>, changes: &mut HistoryChange, edit: ListEdit<Entry>) {
        assert!(edit.apply(entries));
        ListEdit::push_coalesced(&mut changes.edits, edit);
    }
}
