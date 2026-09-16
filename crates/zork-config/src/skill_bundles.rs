//! Selection metadata only; skill bodies remain ordinary files.
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Component, Path, PathBuf},
};
pub const METADATA_DIRECTORY: &str = ".zork";

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct BundleState {
    pub active: Option<BundleSelection>,
    pub disabled: BTreeSet<String>,
    pub distribution_revision: Option<String>,
    pub rollback: bool,
    /// Read-only current files beside user skills. Immutable releases remain
    /// the authority for discovery, upgrade and already returned file paths.
    pub published: BTreeMap<String, PublishedSkill>,
    /// A prepared replacement directory takes ownership only once its own
    /// complete selection is published. The old immutable paths remain valid.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub successor: Option<String>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PublishedSkill {
    pub directory: String,
    pub version: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct BundleSelection {
    pub directory: String,
    pub skills: Vec<String>,
}
pub fn skills_root(data_root: &Path) -> PathBuf {
    data_root.join("skills")
}
pub fn metadata_root(data_root: &Path, directory: &str) -> PathBuf {
    skills_root(data_root)
        .join(directory)
        .join(METADATA_DIRECTORY)
}
pub fn valid_component(value: &str) -> bool {
    !value.is_empty()
        && !value.contains(['/', '\\'])
        && !value.chars().any(char::is_control)
        && !value.starts_with('.')
        && Path::new(value).components().count() == 1
        && matches!(
            Path::new(value).components().next(),
            Some(Component::Normal(_))
        )
}
pub fn load(root: &Path) -> Result<BundleState> {
    let path = root.join(".state.json");
    let state: BundleState = match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => BundleState::default(),
        Err(e) => return Err(e.into()),
    };
    state.validate()?;
    Ok(state)
}
impl BundleState {
    pub fn validate(&self) -> Result<()> {
        if let Some(active) = &self.active {
            ensure!(
                valid_component(&active.directory)
                    && active.skills.len() == 1
                    && active.skills.iter().all(|s| valid_component(s)),
                "invalid bundled skill selection"
            );
        }
        let mut directories = BTreeSet::new();
        ensure!(
            self.published.len() <= 1
                && self.published.iter().all(|(id, entry)| valid_component(id)
                    && valid_component(&entry.directory)
                    && valid_component(&entry.version)
                    && self
                        .active
                        .as_ref()
                        .is_some_and(|active| active.directory == entry.version
                            && active.skills.contains(id))
                    && directories.insert(&entry.directory)),
            "invalid published skill selection"
        );
        ensure!(
            self.successor.as_deref().is_none_or(valid_component),
            "invalid Skill successor directory"
        );
        Ok(())
    }
    pub fn is_replaced_by(&self, directory: &str, replacement: &Self) -> bool {
        self.successor.as_deref() == Some(directory)
            && !self.published.is_empty()
            && replacement.published.len() == self.published.len()
            && replacement
                .published
                .iter()
                .all(|(id, entry)| self.published.contains_key(id) && entry.directory == directory)
    }
}
fn replaced(root: &Path, state: &BundleState) -> bool {
    let Some(directory) = &state.successor else {
        return false;
    };
    let Some(parent) = root.parent().and_then(Path::parent) else {
        return false;
    };
    load(&parent.join(directory).join(METADATA_DIRECTORY))
        .is_ok_and(|replacement| state.is_replaced_by(directory, &replacement))
}
/// A public current directory must not become a second discovery candidate or
/// a writable user source. Its selected immutable release is listed separately.
pub fn published_directory(path: &Path) -> Result<bool> {
    let metadata = path.join(METADATA_DIRECTORY);
    if metadata.join(".state.json").is_file() {
        let state = load(&metadata)?;
        return Ok(!state.published.is_empty() && !replaced(&metadata, &state)
            || metadata.join(".publication.json").is_file() && state.published.is_empty());
    }
    Ok(metadata.join(".publication.json").is_file())
}
#[derive(Default)]
pub struct ManagedCatalog {
    pub entries: Vec<(PathBuf, BundleState)>,
    pub unavailable: Vec<PathBuf>,
    pub diagnostics: Vec<String>,
}
/// Per-directory management records; no root-level bundle owns all skills.
pub fn managed(data_root: &Path) -> Result<Vec<(PathBuf, BundleState)>> {
    Ok(inspect_managed(data_root)?.entries)
}
pub fn inspect_managed(data_root: &Path) -> Result<ManagedCatalog> {
    let root = skills_root(data_root);
    if !root.exists() {
        return Ok(ManagedCatalog::default());
    }
    let mut result = ManagedCatalog::default();
    for entry in std::fs::read_dir(root)? {
        let path = entry.as_ref().ok().map(|entry| entry.path());
        let inspected = (|| -> Result<()> {
            let entry = entry?;
            if !entry.file_type()?.is_dir() || entry.file_name().to_string_lossy().starts_with('.')
            {
                return Ok(());
            }
            let metadata = entry.path().join(METADATA_DIRECTORY);
            if metadata.join(".state.json").is_file() {
                let state = load(&metadata)?;
                if !state.published.is_empty() && !replaced(&metadata, &state) {
                    result.entries.push((metadata, state));
                }
            }
            Ok(())
        })();
        if let Err(error) = inspected {
            let path = path.unwrap_or_else(|| skills_root(data_root));
            if result.diagnostics.len() < 32 {
                result
                    .diagnostics
                    .push(format!("{}: {error}", path.display()));
            }
            result.unavailable.push(path);
        }
    }
    result.entries.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(result)
}
pub fn sources(data_root: &Path) -> Result<Vec<PathBuf>> {
    let mut result = Vec::new();
    for (root, state) in managed(data_root)? {
        if let Some(active) = state.active {
            for id in &active.skills {
                if !state.disabled.contains(id) {
                    result.push(root.join(".versions").join(&active.directory).join(id));
                }
            }
        }
    }
    Ok(result)
}
