//! Agent identity tints and initials, shared by zork-ui, core and Android
//! (through `zork_client_core::message_presentation`).
//!
//! Five tint slots (`zork_ui::design::AGENT_TINTS` on desktop, the same pairs
//! on Android). An agent's slot is a pure function of its id ([`slot`]), so
//! it is the same in the transcript, the chat list, the chat header, reply
//! lines and composer drafts, and never moves as history loads. Two agents
//! in one Chat may share a slot; the maker mark and the name tell them apart.

/// Number of agent tint slots.
pub const AGENT_TINT_SLOTS: usize = 5;

/// The agent's tint slot: 32-bit FNV-1a over the Unicode scalar values of
/// `"agent:" + id` (the prefix seeds it apart from device hues), folded as
/// `(h ^ h >> 15) % 5`. Identical to the approved prototype's preferred slot.
/// Depends on nothing but the id; there is no per-Chat collision avoidance.
pub fn slot(agent_id: &str) -> usize {
    let hash = "agent:"
        .chars()
        .chain(agent_id.chars())
        .fold(0x811C_9DC5u32, |hash, c| {
            (hash ^ c as u32).wrapping_mul(0x0100_0193)
        });
    ((hash ^ (hash >> 15)) as usize) % AGENT_TINT_SLOTS
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slots_match_the_prototype() {
        // Vectors computed with the prototype's JavaScript `tintSlots`.
        for (id, expected) in [
            ("planner", 4),
            ("builder", 1),
            ("review", 3),
            ("tester", 3),
            ("docs", 0),
            ("ops", 3),
            ("design", 0),
            ("审阅助手", 4),
            ("key:abc/worker", 2),
        ] {
            assert_eq!(slot(id), expected, "{id}");
        }
    }

    #[test]
    fn slots_are_in_range() {
        assert!((0..200).all(|n| slot(&format!("agent-{n}")) < AGENT_TINT_SLOTS));
    }
}
