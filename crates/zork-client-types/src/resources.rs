//! Read-only device resource inventory. Credentials and launch commands are never included.
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::Arc};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceKind {
    Service,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resource {
    pub kind: ResourceKind,
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub status: String,
    /// Service visibility: shared/private.
    pub scope: String,
    pub revision: Option<String>,
    #[serde(default)]
    pub owner_session: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub port: Option<u16>,
}
impl Resource {
    pub fn new(
        kind: ResourceKind,
        id: String,
        name: String,
        status: String,
        scope: String,
    ) -> Self {
        Self {
            kind,
            id,
            name,
            status,
            scope,
            revision: None,
            description: String::new(),
            owner_session: None,
            url: None,
            port: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceIssue {
    pub kind: ResourceKind,
    pub error: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceCatalog {
    pub origin: String,
    pub items: Vec<Resource>,
    #[serde(default)]
    pub issues: Vec<ResourceIssue>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceFile {
    pub path: String,
    pub byte_len: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceDocument {
    pub path: String,
    pub text: String,
    pub truncated: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceDetails {
    pub title: String,
    pub description: String,
    #[serde(default)]
    pub files: Vec<ResourceFile>,
    #[serde(default)]
    pub document: Option<ResourceDocument>,
    /// Public facts only; never launch arguments, environment values or credentials.
    #[serde(default)]
    pub facts: Vec<(String, String)>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Inspection {
    Service { id: String, log: Option<String> },
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InspectionContent {
    Details(ResourceDetails),
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InspectionState {
    pub loading: bool,
    pub error: Option<String>,
    pub content: Option<Arc<InspectionContent>>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ResourceDevice {
    pub status: crate::device::DeviceStatus,
    pub id: String,
    pub name: String,
    pub catalog: Option<ResourceCatalog>,
    pub loading: bool,
    pub error: Option<String>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ResourcesData {
    pub devices: Vec<ResourceDevice>,
    pub inspections: HashMap<(String, Inspection), InspectionState>,
}

impl ResourcesData {
    pub fn rows(&self, kind: ResourceKind, node: Option<&str>) -> Vec<(usize, usize)> {
        self.devices
            .iter()
            .enumerate()
            .filter(|(_, d)| node.is_none_or(|node| node == d.id))
            .flat_map(|(di, d)| {
                d.catalog.iter().flat_map(move |c| {
                    c.items
                        .iter()
                        .enumerate()
                        .filter(move |(_, r)| r.kind == kind)
                        .map(move |(ri, _)| (di, ri))
                })
            })
            .collect()
    }
    pub fn inspection(&self, node: &str, query: &Inspection) -> Option<&InspectionState> {
        self.inspections.get(&(node.into(), query.clone()))
    }
}
