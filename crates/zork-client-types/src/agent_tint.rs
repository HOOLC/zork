//! Agent identity tints and initials, shared by zork-ui, core and Android
//! (through `zork_client_core::message_presentation`).
//!
//! Five tint slots (`zork_ui::design::AGENT_TINTS` on desktop, the same pairs
//! on Android). Each agent has a preferred slot from a hash of its id; within
//! one Chat, agents in first-appearance order take the next free slot when
//! theirs is taken. Beyond five agents slots repeat (the name and initial
//! still tell them apart).

/// Number of agent tint slots.
pub const AGENT_TINT_SLOTS: usize = 5;

/// The agent's preferred slot: 32-bit FNV-1a over the Unicode scalar values
/// of `"agent:" + id` (the prefix seeds it apart from device hues), folded as
/// `(h ^ h >> 15) % 5`. Identical to the approved prototype.
pub fn preferred_slot(agent_id: &str) -> usize {
    let hash = "agent:"
        .chars()
        .chain(agent_id.chars())
        .fold(0x811C_9DC5u32, |hash, c| {
            (hash ^ c as u32).wrapping_mul(0x0100_0193)
        });
    ((hash ^ (hash >> 15)) as usize) % AGENT_TINT_SLOTS
}

/// Tint slots of one Chat's agents.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct TintSlots {
    entries: Vec<(String, usize)>,
}

impl TintSlots {
    /// Assigns slots to agent ids in first-appearance order (repeats are
    /// ignored). Each keeps its preferred slot unless an earlier agent holds
    /// it, then takes the next free one; once all five are used, later agents
    /// keep their preferred slot.
    pub fn assign<'a>(ids: impl IntoIterator<Item = &'a str>) -> Self {
        let mut entries: Vec<(String, usize)> = Vec::new();
        for id in ids {
            if entries.iter().any(|(known, _)| known == id) {
                continue;
            }
            let preferred = preferred_slot(id);
            let slot = if entries.len() >= AGENT_TINT_SLOTS {
                preferred
            } else {
                (0..AGENT_TINT_SLOTS)
                    .map(|step| (preferred + step) % AGENT_TINT_SLOTS)
                    .find(|slot| !entries.iter().any(|(_, used)| used == slot))
                    .unwrap_or(preferred)
            };
            entries.push((id.to_owned(), slot));
        }
        Self { entries }
    }
    /// The slot of an agent; one that has not appeared in this Chat (such as
    /// the author of a quoted source outside the loaded history) uses its
    /// preferred slot.
    pub fn slot(&self, agent_id: &str) -> usize {
        self.entries
            .iter()
            .find(|(id, _)| id == agent_id)
            .map_or_else(|| preferred_slot(agent_id), |(_, slot)| *slot)
    }
    /// `(agent id, slot)` in first-appearance order.
    pub fn entries(&self) -> &[(String, usize)] {
        &self.entries
    }
}

/// The letter on an identity disc: the first character of the trimmed name,
/// uppercased when it is a Latin letter (a CJK name shows its first
/// character). `?` for an empty name.
pub fn initial(name: &str) -> String {
    match name.trim().chars().next() {
        Some(c) if c.is_ascii_alphabetic() => c.to_ascii_uppercase().to_string(),
        Some(c) => c.to_string(),
        None => "?".into(),
    }
}
