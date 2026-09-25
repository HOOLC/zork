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

/// A device's two names: the Mesh-wide display name every surface shows, and
/// the machine name the device registered with, kept for details and hints.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DeviceName {
    pub display: String,
    /// Registered machine name; `None` when it is the same as `display` or unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine: Option<String>,
}
impl DeviceName {
    pub fn new(display: impl Into<String>, machine: Option<String>) -> Self {
        let display = display.into();
        let machine = machine.filter(|m| !m.trim().is_empty() && m.trim() != display.trim());
        Self { display, machine }
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
