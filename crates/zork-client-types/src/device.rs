//! Device-name invariant shared by client operations and stored membership.
pub fn validate_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("请输入设备名称".into());
    }
    if name.chars().count() > 64 || name.chars().any(char::is_control) {
        return Err("设备名称不能超过 64 个字符或包含控制字符".into());
    }
    Ok(name.to_owned())
}

/// Longest Mesh display name, in characters. Display names are meant to be
/// short (the defaults are A, B, C…); the machine name keeps the long form.
pub const DISPLAY_NAME_MAX: usize = 24;

/// Display-name invariant shared by the rename field, client core and the
/// Mesh authority. Uniqueness is checked against the Mesh by the authority.
pub fn validate_display_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("请输入显示名称".into());
    }
    if name.chars().any(char::is_control) {
        return Err("显示名称不能包含控制字符".into());
    }
    if name.chars().count() > DISPLAY_NAME_MAX {
        return Err(format!("显示名称不能超过 {DISPLAY_NAME_MAX} 个字符"));
    }
    Ok(name.to_owned())
}

/// Palette slot for a device's stable colour key (see `ResolvedName::color_key`
/// and `SavedNode::color_key`): `seq:N` takes slot N, so devices that joined
/// one after another never share a hue; any other key (a Mesh identity or
/// node id) hashes with FNV-1a. Android implements the same rule.
pub fn color_slot(key: &str, slots: usize) -> usize {
    if slots == 0 {
        return 0;
    }
    if let Some(seq) = key.strip_prefix("seq:").and_then(|n| n.parse::<u64>().ok()) {
        return (seq % slots as u64) as usize;
    }
    let hash = key.bytes().fold(0x811C9DC5u32, |hash, byte| {
        (hash ^ byte as u32).wrapping_mul(0x0100_0193)
    });
    hash as usize % slots
}

#[cfg(test)]
mod color_tests {
    #[test]
    fn join_order_gets_consecutive_slots_and_identities_hash_stably() {
        let slots: Vec<_> = (0..6)
            .map(|n| super::color_slot(&format!("seq:{n}"), 6))
            .collect();
        assert_eq!(slots, [0, 1, 2, 3, 4, 5]);
        assert_eq!(super::color_slot("seq:7", 6), 1);
        // Shared vector with Android's `deviceHueSlot`.
        assert_eq!(super::color_slot("mini1", 6), 1);
        assert_eq!(
            super::color_slot("key:abc", 6),
            super::color_slot("key:abc", 6)
        );
    }
}

/// A device's two names: the Mesh-wide display name every surface shows, and
/// the machine name the device registered with, kept for details and hints.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DeviceName {
    pub display: String,
    /// Registered machine name; `None` when it is the same as `display` or unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine: Option<String>,
    /// Stable colour key (join order or identity); `None` falls back to the name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}
impl DeviceName {
    pub fn new(display: impl Into<String>, machine: Option<String>) -> Self {
        let display = display.into();
        let machine = machine.filter(|m| !m.trim().is_empty() && m.trim() != display.trim());
        Self {
            display,
            machine,
            color: None,
        }
    }
    pub fn with_color(mut self, key: Option<String>) -> Self {
        self.color = key;
        self
    }
    /// Words for assistive technology and automation: both names when they differ.
    pub fn accessible(&self) -> String {
        match &self.machine {
            Some(machine) => format!("{}（{machine}）", self.display),
            None => self.display.clone(),
        }
    }
}
impl From<String> for DeviceName {
    fn from(display: String) -> Self {
        Self::new(display, None)
    }
}
impl From<&str> for DeviceName {
    fn from(display: &str) -> Self {
        Self::new(display, None)
    }
}
impl From<&String> for DeviceName {
    fn from(display: &String) -> Self {
        Self::new(display.clone(), None)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PeerInput {
    pub name: String,
    pub origin: String,
    pub address: Option<String>,
}

/// Client transport readiness is independent of an individual device's reachability.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "state", content = "error", rename_all = "snake_case")]
pub enum MeshReadiness {
    #[default]
    NotStarted,
    Preparing,
    Ready,
    Stopping,
    Stopped,
    Failed(String),
}

/// Core-owned presentation state shared by device names on every surface.
/// It describes whether this client can reach the station. The default is an
/// explicit "not connected", never an assumed connection attempt.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "state", content = "error", rename_all = "snake_case")]
pub enum DeviceStatus {
    MeshNotStarted,
    MeshPreparing,
    MeshStopping,
    MeshStopped,
    MeshFailed(String),
    /// No connection from this client exists or is being attempted.
    #[default]
    NotConnected,
    Connecting,
    Direct,
    Relay,
    Connected,
    Offline,
    Revoked,
}
