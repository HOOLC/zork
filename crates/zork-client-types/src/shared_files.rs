//! File kinds shared by the native and mobile tree projections.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    Directory,
    File,
}

use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    #[serde(default)]
    pub status: crate::device::DeviceStatus,
    pub id: String,
    pub name: String,
    pub online: Option<bool>,
    pub cached: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Space {
    pub id: String,
    pub name: String,
    pub sources: Vec<Source>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Version {
    pub root: String,
    pub size: u64,
    pub modified_ns: i64,
    pub sources: Vec<Source>,
    pub can_read: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    #[serde(default)]
    pub target: Option<String>,
    pub id: String,
    pub path: String,
    pub name: String,
    pub kind: EntryKind,
    pub sources: Vec<Source>,
    pub versions: Vec<Version>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Device {
    #[serde(default)]
    pub status: crate::device::DeviceStatus,
    pub id: String,
    pub name: String,
    pub local: bool,
    pub online: Option<bool>,
    pub loading: bool,
    pub error: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Location {
    pub space: String,
    pub path: String,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Layout {
    #[default]
    List,
    Grid,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Sort {
    #[default]
    Name,
    NameDescending,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EmptyState {
    NoSpaces,
    NoResults,
    Unavailable,
    Directory,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Preview {
    pub path: String,
    pub name: String,
    pub versions: Vec<Version>,
    pub selected: String,
    pub loading: bool,
    pub error: Option<String>,
    pub text: Option<String>,
    pub truncated: bool,
    pub mime: String,
    pub cached: bool,
    pub can_save: bool,
    #[serde(skip)]
    pub bytes: Option<Arc<Vec<u8>>>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct SaveState {
    pub busy: bool,
    pub ticket: Option<String>,
    pub name: String,
    pub error: Option<String>,
    pub completed: bool,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct SharedFilesData {
    pub active: bool,
    pub devices: Vec<Device>,
    pub spaces: Vec<Space>,
    pub location: Option<Location>,
    pub location_name: String,
    pub empty: Option<EmptyState>,
    pub source: Option<String>,
    pub search: String,
    pub layout: Layout,
    pub sort: Sort,
    pub entries: Arc<Vec<Entry>>,
    pub preview: Option<Preview>,
    pub loading: bool,
    pub more: bool,
    pub offline: bool,
    pub error: Option<String>,
    pub save: SaveState,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    Activate { active: bool },
    OpenSpace { space: String },
    OpenEntry { id: String },
    Back,
    ClosePreview,
    Source { peer: Option<String> },
    Search { query: String },
    Layout { layout: Layout },
    Sort { sort: Sort },
    Refresh,
    More,
    SelectVersion { root: String },
    PrepareSave,
    CancelSave { ticket: String },
    SaveFailed { ticket: String, error: String },
}
