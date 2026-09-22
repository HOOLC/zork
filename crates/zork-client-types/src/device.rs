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
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "state", content = "error", rename_all = "snake_case")]
pub enum DeviceStatus {
    MeshNotStarted,
    MeshPreparing,
    MeshStopping,
    MeshStopped,
    MeshFailed(String),
    #[default]
    Connecting,
    Direct,
    Relay,
    Connected,
    Offline,
    Revoked,
}
