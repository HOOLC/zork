use super::*;
use synch_core::{FileEntry, Hash};
use synch_mpt::Trie;
use zork_config::tree::Reference;

fn selected_root(node: &Node, reference: &Reference) -> Result<Hash> {
    let origin: OriginId = reference
        .origin
        .as_deref()
        .context("snapshot origin missing")?
        .parse()?;
    origins(node, Some(&origin.to_string()))?;
    let root: Hash = reference
        .snapshot
        .as_deref()
        .context("snapshot missing")?
        .parse()?;
    ensure!(
        node.store().head_root_origins(&root)?.contains(&origin),
        "file snapshot is no longer available"
    );
    Ok(root)
}

fn entry(reference: &Reference, path: String, file: FileEntry) -> TreeEntry {
    let origin = reference.origin.clone().unwrap();
    let selected = file.content.map(|root| ObjectRef {
        origin: origin.clone(),
        space: reference.space.clone(),
        path: path.clone(),
        root: root.to_string(),
        size: file.size,
    });
    TreeEntry {
        path,
        kind: match file.kind {
            EntryKind::File => "file",
            EntryKind::Dir => "directory",
            EntryKind::Symlink => "symlink",
            EntryKind::Socket => "socket",
            EntryKind::Tombstone => "tombstone",
        }
        .into(),
        versions: selected
            .as_ref()
            .map(|object| TreeVersion {
                root: object.root.clone(),
                size: object.size,
                modified_ns: file.mtime_ns,
                origins: vec![origin.clone()],
            })
            .into_iter()
            .collect(),
        origins: vec![origin],
        target: file.symlink_target,
        executable: file.unix_mode.is_some_and(|mode| mode & 0o111 != 0),
        selected,
    }
}

impl MeshNode {
    /// Reuse a selection when changes elsewhere in the origin do not touch
    /// this directory. Structural diff prunes unchanged resource subtrees.
    pub async fn tree_same_directory(
        &self,
        previous: Reference,
        current: Reference,
    ) -> Result<bool> {
        ensure!(
            previous.space == current.space
                && previous.path == current.path
                && previous.origin == current.origin,
            "snapshot scopes differ"
        );
        self.blocking(move |node| {
            let old = selected_root(&node, &previous)?;
            let new = selected_root(&node, &current)?;
            let scope = synch_mpt::Scope::of(&synch_core::ScopeKeys {
                prefixes: vec![synch_core::dir_prefix(&current.space, &current.path)?],
                exact: vec![],
            });
            let mut changed = false;
            let result = Trie::new(node.store().as_ref())
                .for_each_resolved_change_scoped::<anyhow::Error, _>(old, new, &scope, |_| {
                    changed = true;
                    anyhow::bail!("directory changed")
                });
            if changed {
                return Ok(false);
            }
            result?;
            Ok(true)
        })
        .await
    }
    pub async fn tree_entry_at(&self, reference: Reference) -> Result<Option<TreeEntry>> {
        self.blocking(move |node| {
            let root = selected_root(&node, &reference)?;
            let bytes = Trie::new(node.store().as_ref()).get(
                root,
                &synch_core::file_key(&reference.space, &reference.path)?,
            )?;
            let Some(bytes) = bytes else {
                return Ok(None);
            };
            let file: FileEntry = synch_core::record::decode(&bytes)?;
            Ok((file.kind != EntryKind::Tombstone)
                .then(|| entry(&reference, reference.path.clone(), file)))
        })
        .await
    }
    /// Capture an already published origin tree. Relative references inherit
    /// this root; subsequent reads never silently select a newer tree.
    pub async fn tree_snapshot(&self, mut reference: Reference) -> Result<Reference> {
        self.blocking(move |node| {
            if reference.snapshot.is_some() {
                selected_root(&node, &reference)?;
                return Ok(reference);
            }
            let origin: OriginId = reference
                .origin
                .as_deref()
                .context("choose a file origin before taking a snapshot")?
                .parse()?;
            origins(&node, Some(&origin.to_string()))?;
            reference.snapshot = Some(
                node.store()
                    .complete_head(&origin)?
                    .context("file source is not published yet")?
                    .root
                    .to_string(),
            );
            Ok(reference)
        })
        .await
    }

    pub(super) async fn snapshot_object(
        &self,
        reference: Reference,
    ) -> Result<(ObjectRef, Option<u32>)> {
        self.blocking(move |node| {
            let root = selected_root(&node, &reference)?;
            let bytes = Trie::new(node.store().as_ref())
                .get(
                    root,
                    &synch_core::file_key(&reference.space, &reference.path)?,
                )?
                .context("file absent in selected snapshot")?;
            let file: FileEntry = synch_core::record::decode(&bytes)?;
            ensure!(file.kind.has_content(), "snapshot entry is not a file");
            let content = file
                .content
                .context("snapshot file content missing")?
                .to_string();
            ensure!(
                reference.root.as_ref().is_none_or(|root| root == &content),
                "content does not belong to selected snapshot"
            );
            Ok((
                ObjectRef {
                    origin: reference.origin.unwrap(),
                    space: reference.space,
                    path: reference.path,
                    root: content,
                    size: file.size,
                },
                file.unix_mode,
            ))
        })
        .await
    }

    /// The normal directory operation against a fixed tree, with a scope-bound
    /// cursor. Resource subtrees are skipped by key range, not enumerated.
    pub async fn tree_directory_at(
        &self,
        reference: Reference,
        after: Option<TreeCursor>,
        limit: usize,
    ) -> Result<TreePage> {
        ensure!((1..=128).contains(&limit), "invalid directory page size");
        self.blocking(move |node| {
            let root = selected_root(&node, &reference)?;
            let revision = crate::content_root(&serde_json::to_vec(&reference)?);
            ensure!(
                after
                    .as_ref()
                    .is_none_or(|cursor| cursor.revision == revision),
                "directory cursor belongs to another snapshot or scope"
            );
            let trie = Trie::new(node.store().as_ref());
            let prefix = synch_core::dir_prefix(&reference.space, &reference.path)?;
            let parent = if reference.path.is_empty() {
                String::new()
            } else {
                format!("{}/", reference.path)
            };
            if let Some(cursor) = &after {
                ensure!(
                    cursor
                        .path
                        .strip_prefix(&parent)
                        .is_some_and(|child| !child.is_empty() && !child.contains('/')),
                    "invalid directory cursor path"
                );
            }
            let mut last = after.map(|cursor| cursor.path);
            let mut scan_after = last
                .as_ref()
                .map(|path| synch_core::file_key(&reference.space, path))
                .transpose()?;
            let mut children = Vec::new();
            let mut visits = 0;
            let mut first_live =
                |prefix: &[u8], after: Option<&[u8]>| -> Result<Option<(Vec<u8>, FileEntry)>> {
                    let mut after = after.map(<[u8]>::to_vec);
                    loop {
                        visits += 1;
                        ensure!(visits <= 200_000, "directory exceeds metadata query budget");
                        let Some((key, bytes)) =
                            trie.scan(root, prefix, after.as_deref(), Some(1))?.pop()
                        else {
                            return Ok(None);
                        };
                        let file: FileEntry = synch_core::record::decode(&bytes)?;
                        if file.kind != EntryKind::Tombstone {
                            return Ok(Some((key, file)));
                        }
                        after = Some(key);
                    }
                };
            let mut absent_prefixes = BTreeSet::new();
            while children.len() < limit + 1 {
                let Some((key, mut file)) = first_live(&prefix, scan_after.as_deref())? else {
                    break;
                };
                let relative = std::str::from_utf8(&key[prefix.len()..])?;
                let name = relative.split('/').next().unwrap();
                let mut child = format!("{parent}{name}");
                if last.as_ref().is_some_and(|last| child <= *last) {
                    let mut skip = synch_core::dir_prefix(&reference.space, &child)?;
                    skip.push(255);
                    scan_after = Some(skip);
                    continue;
                }
                let mut directory = relative.contains('/');
                // An implicit `a/child` sorts after `a.txt` as a raw key,
                // while the direct child `a` must sort before `a.txt`.
                // Probe only those shorter names whose next byte is below
                // '/', instead of sorting the entire directory on every page.
                for (index, _) in name
                    .char_indices()
                    .filter(|(index, next)| *index > 0 && *next < '/')
                {
                    let shorter = &name[..index];
                    if matches!(shorter, "." | "..") {
                        continue;
                    }
                    let candidate = format!("{parent}{shorter}");
                    if last.as_ref().is_some_and(|last| candidate <= *last)
                        || absent_prefixes.contains(&candidate)
                    {
                        continue;
                    }
                    let scope = synch_core::dir_prefix(&reference.space, &candidate)?;
                    if first_live(&scope, None)?.is_some() {
                        child = candidate;
                        directory = true;
                        break;
                    }
                    absent_prefixes.insert(candidate);
                }
                if directory {
                    file = FileEntry::tombstone(0, 0, None);
                    file.kind = EntryKind::Dir;
                }
                scan_after = Some(synch_core::file_key(&reference.space, &child)?);
                last = Some(child.clone());
                children.push(entry(&reference, child, file));
            }
            let count = children.len();
            let mut entries = Vec::new();
            let mut encoded = 0;
            for entry in children.into_iter().take(limit) {
                encoded += serde_json::to_vec(&entry)?.len();
                if encoded > 112 * 1024 && !entries.is_empty() {
                    break;
                }
                ensure!(encoded < 120 * 1024, "tree entry exceeds frame bound");
                entries.push(entry);
            }
            let more = count > entries.len();
            let next = more.then(|| TreeCursor {
                revision: revision.clone(),
                path: entries.last().unwrap().path.clone(),
            });
            Ok(TreePage {
                revision,
                entries,
                next,
            })
        })
        .await
    }

    pub(super) async fn snapshot_walk(
        &self,
        reference: Reference,
        limit: usize,
    ) -> Result<Vec<TreeEntry>> {
        let mut pending = vec![reference];
        let mut entries = Vec::new();
        while let Some(directory) = pending.pop() {
            let mut after = None;
            loop {
                let page = self
                    .tree_directory_at(directory.clone(), after, 128)
                    .await?;
                for item in page.entries {
                    ensure!(entries.len() < limit, "tree subtree exceeds metadata bound");
                    if item.kind == "directory" {
                        pending.push(Reference {
                            path: item.path.clone(),
                            ..directory.clone()
                        });
                    }
                    entries.push(item);
                }
                after = page.next;
                if after.is_none() {
                    break;
                }
            }
        }
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(entries)
    }
}
