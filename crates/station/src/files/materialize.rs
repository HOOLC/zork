use super::*;
use zork_mesh::node::TreeQuery;

const MAX_BYTES: u64 = 512 * 1024 * 1024;
#[derive(Default)]
pub(super) struct Cache {
    // Keep every returned path alive until Files drops; never evict a path an
    // active shell might still use. Refuse work when the fixed budget is full.
    entries: Vec<(String, tempfile::TempDir, PathBuf, u64)>,
}

impl Files {
    pub(super) async fn materialize_reference(&self, path: &Path) -> Result<PathBuf> {
        let reference = Reference::parse(&path.to_string_lossy())?;
        let node = self.node()?;
        let mut cache = self.materialized.lock().await;
        let entries = if reference.root.is_none() {
            node.tree_walk(reference.clone(), 4096).await?
        } else {
            vec![]
        };
        let object = if reference.root.is_some() {
            let page = node.tree_file(&reference, 0, 0).await?;
            // Old content roots remain readable after their path changes. Only
            // grant execution when that version still has published mode data.
            let mode = node
                .tree_object_with_mode(&reference)
                .await
                .ok()
                .and_then(|(_, mode)| mode);
            Some((
                zork_mesh::node::ObjectRef {
                    origin: reference
                        .origin
                        .clone()
                        .context("a pinned materialization requires its origin")?,
                    space: reference.space.clone(),
                    path: reference.path.clone(),
                    root: reference.root.clone().unwrap(),
                    size: page.total_size,
                },
                mode,
            ))
        } else if entries.is_empty() {
            node.tree_object_with_mode(&reference).await.ok()
        } else {
            None
        };
        if object.is_none() && entries.is_empty() && !reference.path.is_empty() {
            if reference.snapshot.is_some() {
                anyhow::ensure!(
                    node.tree_entry_at(reference.clone())
                        .await?
                        .is_some_and(|entry| entry.kind == "directory"),
                    "shared directory unavailable in selected snapshot"
                );
            } else {
                let (parent, name) = reference
                    .path
                    .rsplit_once('/')
                    .unwrap_or(("", &reference.path));
                let page = node
                    .tree_directory(TreeQuery {
                        space: reference.space.clone(),
                        path: parent.into(),
                        origin: reference.origin.clone(),
                        search: name.into(),
                        descending: false,
                        after: None,
                        limit: 128,
                    })
                    .await?;
                anyhow::ensure!(
                    page.entries
                        .iter()
                        .any(|e| e.path == reference.path && e.kind == "directory"),
                    "shared directory unavailable"
                );
            }
        }
        let identity =
            zork_mesh::content_root(&serde_json::to_vec(&(&reference, &object, &entries))?);
        if let Some((_, _, path, _)) = cache.entries.iter().find(|(key, ..)| key == &identity) {
            return Ok(path.clone());
        }
        anyhow::ensure!(
            cache.entries.len() < 64,
            "file materialization cache is full"
        );
        let size = object.as_ref().map_or(0, |(o, _)| o.size)
            + entries
                .iter()
                .filter_map(|e| e.selected.as_ref().map(|o| o.size))
                .sum::<u64>();
        anyhow::ensure!(
            size <= MAX_BYTES && cache.entries.iter().map(|e| e.3).sum::<u64>() <= MAX_BYTES - size,
            "file materialization exceeds 512 MiB cache budget"
        );
        let parent = self.root.join("state/file-materializations");
        std::fs::create_dir_all(&parent)?;
        let temp = tempfile::tempdir_in(parent)?;
        let result = if let Some((object, mode)) = object {
            let name = Path::new(&object.path)
                .file_name()
                .context("file name missing")?;
            let target = temp.path().join(name);
            let bytes = node.tree_read(&object).await?;
            write(
                target.clone(),
                bytes,
                mode.is_some_and(|mode| mode & 0o111 != 0),
            )
            .await?;
            target
        } else {
            // Metadata is frozen before downloads. Updates arriving while
            // bodies transfer cannot mix newer files into this snapshot.
            for entry in &entries {
                let relative = relative_path(&reference.path, &entry.path)?;
                let target = temp.path().join(relative);
                match entry.kind.as_str() {
                    "directory" => std::fs::create_dir_all(&target)?,
                    "file" => {
                        let object = entry.selected.as_ref().context("file version missing")?;
                        let bytes = node.tree_read(object).await?;
                        std::fs::create_dir_all(target.parent().unwrap())?;
                        write(target, bytes, entry.executable).await?;
                    }
                    "symlink" => {}
                    _ => anyhow::bail!(
                        "this subtree contains a live socket; select ordinary files for local execution"
                    ),
                }
            }
            // Create links last so they cannot redirect file writes out of
            // this private snapshot, including API-created overlapping paths.
            for entry in entries.iter().filter(|e| e.kind == "symlink") {
                let relative = relative_path(&reference.path, &entry.path)?;
                let target = entry.target.as_deref().context("symlink target missing")?;
                checked_link(relative, Path::new(target))?;
                let path = temp.path().join(relative);
                std::fs::create_dir_all(path.parent().unwrap())?;
                #[cfg(unix)]
                std::os::unix::fs::symlink(target, path)?;
                #[cfg(not(unix))]
                anyhow::bail!("symlink materialization is unavailable on this platform");
            }
            // Validate the completed link graph too: two individually
            // relative links can otherwise combine to escape the root.
            let canonical = temp.path().canonicalize()?;
            for entry in entries.iter().filter(|e| e.kind == "symlink") {
                let relative = relative_path(&reference.path, &entry.path)?;
                anyhow::ensure!(
                    temp.path()
                        .join(relative)
                        .canonicalize()?
                        .starts_with(&canonical),
                    "symlink leaves the selected subtree"
                );
            }
            temp.path().to_path_buf()
        };
        cache.entries.push((identity, temp, result.clone(), size));
        Ok(result)
    }
}
async fn write(path: PathBuf, bytes: Vec<u8>, executable: bool) -> Result<()> {
    tokio::task::spawn_blocking(move || {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        file.write_all(&bytes)?;
        let mut permissions = std::fs::metadata(&path)?.permissions();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            permissions.set_mode(if executable { 0o555 } else { 0o444 });
        }
        #[cfg(not(unix))]
        permissions.set_readonly(true);
        std::fs::set_permissions(path, permissions)?;
        Ok::<_, anyhow::Error>(())
    })
    .await?
}
fn checked_link(path: &Path, target: &Path) -> Result<()> {
    use std::path::Component;
    let mut depth = path.parent().map_or(0, |p| p.components().count());
    for part in target.components() {
        match part {
            Component::Normal(_) => depth += 1,
            Component::CurDir => {}
            Component::ParentDir => {
                anyhow::ensure!(depth > 0, "symlink leaves the selected subtree");
                depth -= 1;
            }
            _ => anyhow::bail!("absolute symlink leaves the selected subtree"),
        }
    }
    Ok(())
}

fn relative_path<'a>(prefix: &str, path: &'a str) -> Result<&'a Path> {
    let relative = if prefix.is_empty() {
        path
    } else {
        path.strip_prefix(&format!("{prefix}/"))
            .context("file escaped selected subtree")?
    };
    let path = Path::new(relative);
    anyhow::ensure!(
        !relative.is_empty()
            && path
                .components()
                .all(|part| matches!(part, std::path::Component::Normal(_))),
        "invalid materialized file path"
    );
    Ok(path)
}
