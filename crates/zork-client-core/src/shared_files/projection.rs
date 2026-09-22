use super::*;
use std::collections::BTreeSet;

pub(super) fn content_identity(space: &str, path: &str, root: &str) -> String {
    serde_json::to_string(&(space, path, root)).expect("file identity")
}
impl SharedFiles {
    pub(super) fn rebuild(&self, s: &mut Owned) {
        let visible = s
            .catalog
            .spaces
            .iter()
            .flat_map(|space| space.origins.iter().cloned())
            .collect::<BTreeSet<_>>();
        let online = |id: &str| {
            if s.data.offline {
                return None;
            }
            s.bindings
                .iter()
                .filter(|b| b.origin.as_deref() == Some(id))
                .filter_map(|b| b.online)
                .max()
        };
        let status = |id: &str| {
            s.bindings
                .iter()
                .find(|b| b.origin.as_deref() == Some(id))
                .map(|b| b.status.clone())
                .unwrap_or_default()
        };
        let source = |id: &str, cached: bool| Source {
            id: id.into(),
            name: s.names.get(id).cloned().unwrap_or_else(|| id.into()),
            online: online(id),
            status: status(id),
            cached,
        };
        s.data.devices = visible
            .iter()
            .map(|id| Device {
                id: id.clone(),
                name: s.names.get(id).cloned().unwrap_or_else(|| id.clone()),
                online: online(id),
                status: status(id),
                ..Default::default()
            })
            .collect();
        let needle = s.data.search.to_lowercase();
        s.data.spaces = s
            .catalog
            .spaces
            .iter()
            .filter(|space| {
                s.data
                    .source
                    .as_ref()
                    .is_none_or(|id| space.origins.contains(id))
            })
            .filter(|space| s.data.location.is_some() || space.id.to_lowercase().contains(&needle))
            .map(|space| Space {
                id: space.id.clone(),
                name: space.id.clone(),
                sources: space.origins.iter().map(|id| source(id, false)).collect(),
            })
            .collect();
        if s.data.sort == Sort::NameDescending {
            s.data.spaces.reverse();
        }
        s.data.location_name = s
            .data
            .location
            .as_ref()
            .map(|l| l.space.clone())
            .unwrap_or_default();
        let entries = s
            .page
            .as_ref()
            .map(|page| {
                page.entries
                    .iter()
                    .map(|entry| {
                        let space = s
                            .data
                            .location
                            .as_ref()
                            .map(|l| l.space.as_str())
                            .unwrap_or("");
                        let mut versions = entry
                            .versions
                            .iter()
                            .rev()
                            .map(|version| {
                                let cached = s.cache.contains_key(&content_identity(
                                    space,
                                    &entry.path,
                                    &version.root,
                                ));
                                Version {
                                    root: version.root.clone(),
                                    size: version.size,
                                    modified_ns: version.modified_ns,
                                    sources: version
                                        .origins
                                        .iter()
                                        .map(|id| source(id, cached))
                                        .collect(),
                                    can_read: version.size <= crate::files::MAX_FILE_BYTES as u64
                                        && (!s.data.offline || cached),
                                }
                            })
                            .collect::<Vec<_>>();
                        if let Some(selected) = &entry.selected {
                            versions.sort_by_key(|version| version.root != selected.root);
                        }
                        if let Some(id) = &s.data.source {
                            for version in &mut versions {
                                version.sources.retain(|s| &s.id == id);
                            }
                        }
                        Entry {
                            id: entry.path.clone(),
                            path: entry.path.clone(),
                            name: entry.path.rsplit('/').next().unwrap_or("").into(),
                            kind: if entry.kind == "directory" {
                                EntryKind::Directory
                            } else {
                                EntryKind::File
                            },
                            sources: entry.origins.iter().map(|id| source(id, false)).collect(),
                            versions,
                            target: entry.target.clone(),
                        }
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if s.data.entries.as_ref() != &entries {
            s.data.entries = Arc::new(entries);
        }
        s.data.more = s.page.as_ref().is_some_and(|p| p.next.is_some())
            && s.data.entries.len() < MAX_DIRECTORY_ENTRIES;
        if s.page.as_ref().is_some_and(|p| p.next.is_some())
            && s.data.entries.len() >= MAX_DIRECTORY_ENTRIES
        {
            s.data.error = Some("目录内容较多，请搜索名称以缩小范围".into());
        }
        let count = if s.data.location.is_some() {
            s.data.entries.len()
        } else {
            s.data.spaces.len()
        };
        s.data.empty = if count > 0 || s.data.loading {
            None
        } else if s.data.error.is_some() || s.data.offline {
            Some(EmptyState::Unavailable)
        } else if !s.data.search.is_empty() {
            Some(EmptyState::NoResults)
        } else if s.data.location.is_none() {
            Some(EmptyState::NoSpaces)
        } else {
            Some(EmptyState::Directory)
        };
        if s.data.preview.as_ref().is_some_and(|preview| {
            !preview.versions.is_empty()
                && !preview
                    .versions
                    .iter()
                    .flat_map(|v| v.sources.iter())
                    .any(|source| visible.contains(&source.id))
        }) {
            s.data.preview = None;
            s.export = None;
            s.data.save = SaveState::default();
            self.cancel_preview(s);
        }
    }
}
