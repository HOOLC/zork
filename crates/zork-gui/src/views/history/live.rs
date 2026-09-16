use super::*;
use std::collections::HashSet;
pub(super) use zork_ui::history_page::HistoryChanged;
impl gpui::EventEmitter<HistoryChanged> for RootView {}
pub(super) fn from_update(
    update: &zork_client_core::state::HistoryUpdate,
    clock: bool,
) -> HistoryChanged {
    let mut entries = HashSet::new();
    let mut structure = update.reset;
    if let Some(changes) = &update.entries {
        entries.extend(changes.changed_ids.iter().cloned());
        structure |= changes.structure_changed;
    }
    HistoryChanged {
        entries,
        structure,
        clock,
    }
}
