//! The complete native capability boundary. JavaScript cannot construct raw JNI calls.
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub const MAX_DATA: usize = 48 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Intent {
    pub action: String,
    pub data: Option<String>,
    pub mime_type: Option<String>,
    pub package: Option<String>,
    #[serde(default)]
    pub categories: Vec<String>,
    #[serde(default)]
    pub extras: BTreeMap<String, Value>,
    pub chooser_title: Option<String>,
    #[serde(default)]
    pub flags: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "method", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    Info,
    StartActivity {
        intent: Intent,
        #[serde(default)]
        result: bool,
    },
    RequestPermissions {
        permissions: Vec<String>,
    },
    HasPermission {
        permission: String,
    },
    ClipboardWrite {
        text: String,
    },
    ClipboardRead,
    ClipboardClear,
    Torch {
        enabled: bool,
        camera_id: Option<String>,
    },
    VolumeGet {
        stream: String,
    },
    VolumeSet {
        stream: String,
        index: u32,
        show_ui: bool,
    },
    Vibrate {
        milliseconds: u64,
    },
    SettingsGet {
        namespace: String,
        key: String,
    },
    SettingsCanWrite,
    SettingsPut {
        key: String,
        value: i32,
    },
    ContentReadText {
        uri: String,
    },
    ContentWriteText {
        uri: String,
        text: String,
    },
    Location,
}

fn text(value: &str, max: usize) -> bool {
    !value.is_empty() && value.len() <= max && !value.contains('\0')
}
pub fn content_uri(uri: &str) -> bool {
    text(uri, 4096) && uri.starts_with("content://")
}
fn permission(value: &str) -> bool {
    value.strip_prefix("android.permission.").is_some_and(|p| {
        matches!(
            p,
            "CAMERA"
                | "ACCESS_COARSE_LOCATION"
                | "ACCESS_FINE_LOCATION"
                | "CALL_PHONE"
                | "POST_NOTIFICATIONS"
        )
    })
}
fn stream(value: &str) -> bool {
    matches!(value, "music" | "alarm" | "ring" | "notification")
}

impl Operation {
    pub fn validate(&self) -> Result<()> {
        let valid = match self {
            Self::StartActivity { intent, .. } => {
                text(&intent.action, 512)
                    && intent.data.as_ref().is_none_or(|s| text(s, 8192) && !s.starts_with("file:"))
                    && intent.mime_type.as_ref().is_none_or(|s| text(s, 256))
                    && intent.package.as_ref().is_none_or(|s| text(s, 256))
                    && intent.chooser_title.as_ref().is_none_or(|s| text(s, 512))
                    && intent.categories.len() <= 16 && intent.categories.iter().all(|s| text(s, 256))
                    // Only URI access grants; task/process flags stay with the platform adapter.
                    && intent.flags & !(1 | 2 | 64 | 128) == 0
                    && intent.extras.len() <= 32 && intent.extras.iter().all(|(k, v)| text(k, 512) && match v {
                        Value::String(s) => s.len() <= 16384 && !s.contains('\0'),
                        Value::Bool(_) | Value::Number(_) => true,
                        Value::Array(items) => items.len() <= 32 && items.iter().all(|v| v.as_str().is_some_and(|s| text(s, 4096))),
                        Value::Object(o) => o.len() == 1 && o.get("uri").and_then(Value::as_str).is_some_and(content_uri),
                        _ => false,
                    })
            }
            Self::RequestPermissions { permissions } => {
                !permissions.is_empty()
                    && permissions.len() <= 10
                    && permissions.iter().all(|p| permission(p))
            }
            Self::HasPermission { permission: p } => permission(p),
            Self::ClipboardWrite { text } => text.len() <= MAX_DATA && !text.contains('\0'),
            Self::Torch { camera_id, .. } => camera_id.as_ref().is_none_or(|s| text(s, 256)),
            Self::VolumeGet { stream: s } => stream(s),
            Self::VolumeSet {
                stream: s, index, ..
            } => stream(s) && *index <= i32::MAX as u32,
            Self::Vibrate { milliseconds } => *milliseconds > 0 && *milliseconds <= 10_000,
            Self::SettingsGet { namespace, key } => {
                matches!(namespace.as_str(), "global" | "secure" | "system") && text(key, 256)
            }
            Self::SettingsPut { key, value } => match key.as_str() {
                "screen_brightness" => (0..=255).contains(value),
                "screen_brightness_mode" | "accelerometer_rotation" => (0..=1).contains(value),
                "user_rotation" => (0..=3).contains(value),
                "screen_off_timeout" => (1_000..=1_800_000).contains(value),
                _ => false,
            },
            Self::ContentReadText { uri } => content_uri(uri),
            Self::ContentWriteText { uri, text } => content_uri(uri) && text.len() <= MAX_DATA,
            Self::Info
            | Self::ClipboardRead
            | Self::ClipboardClear
            | Self::SettingsCanWrite
            | Self::Location => true,
        };
        ensure!(valid, "Invalid Android operation parameters");
        Ok(())
    }
}
