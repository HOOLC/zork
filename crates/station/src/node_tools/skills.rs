use super::*;
use std::{fs, path::Path};
fn shared_reference(state: &AppState, path: &Path) -> Result<String> {
    let root = zork_config::skill_bundles::skills_root(&state.config.data_root);
    let canonical = root.canonicalize()?;
    let path = path
        .strip_prefix(&canonical)
        .or_else(|_| path.strip_prefix(&root))
        .context("skill is outside the Station file tree")?;
    Ok(zork_config::tree::Reference {
        space: zork_config::tree::SKILLS_SPACE.into(),
        path: path.to_string_lossy().replace('\\', "/"),
        origin: Some(
            state
                .mesh
                .get()
                .context("skill_network_unavailable")?
                .origin()
                .into(),
        ),
        root: None,
        snapshot: None,
    }
    .uri())
}
/// A live directory is described independently of the export format. Streaming
/// hashes retain optimistic revision checks without building a base64 package
/// or imposing the transfer envelope's resource count and byte limits.
fn describe_directory(directory: &Path) -> Result<(Value, String, usize)> {
    use std::io::Read;
    fn walk(
        root: &Path,
        directory: &Path,
        hash: &mut blake3::Hasher,
        resources: &mut usize,
    ) -> Result<()> {
        let mut entries = fs::read_dir(directory)?.collect::<std::io::Result<Vec<_>>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            let relative = path.strip_prefix(root)?;
            let kind = entry.file_type()?;
            if kind.is_dir() {
                serde_json::to_writer(&mut *hash, &(relative, "directory"))?;
                walk(root, &path, hash, resources)?;
            } else if kind.is_symlink() {
                serde_json::to_writer(&mut *hash, &(relative, "symlink", fs::read_link(&path)?))?;
                *resources += 1;
            } else if kind.is_file() {
                let file = fs::File::open(&path)?;
                let before = file.metadata()?;
                let mut content = blake3::Hasher::new();
                let read = std::io::copy(
                    &mut (&file).take(before.len().saturating_add(1)),
                    &mut content,
                )?;
                let after = file.metadata()?;
                ensure!(
                    read == before.len()
                        && after.len() == before.len()
                        && after.modified()? == before.modified()?,
                    "skill_revision_conflict"
                );
                #[cfg(unix)]
                let executable = {
                    use std::os::unix::fs::PermissionsExt;
                    before.permissions().mode() & 0o111 != 0
                };
                #[cfg(not(unix))]
                let executable = false;
                serde_json::to_writer(
                    &mut *hash,
                    &(
                        relative,
                        "file",
                        content.finalize().to_hex().as_str(),
                        executable,
                    ),
                )?;
                if relative != Path::new("SKILL.md") {
                    *resources += 1;
                }
            } else {
                // A socket can be published, but is never opened while
                // describing a Skill and cannot become an export resource.
                serde_json::to_writer(&mut *hash, &(relative, "special"))?;
                *resources += 1;
            }
        }
        Ok(())
    }
    let document = zork_agent::skills::read_document(&directory.join("SKILL.md"))
        .map_err(|_| anyhow::anyhow!("skill_invalid_manifest"))?;
    let metadata = zork_agent::skills::management::validate(&document)
        .map_err(|_| anyhow::anyhow!("skill_invalid_manifest"))?;
    let mut hash = blake3::Hasher::new();
    let mut resources = 0;
    walk(directory, directory, &mut hash, &mut resources)?;
    ensure!(
        zork_agent::skills::read_document(&directory.join("SKILL.md"))? == document,
        "skill_revision_conflict"
    );
    Ok((metadata, hash.finalize().to_hex().to_string(), resources))
}
#[cfg(test)]
mod directory_tests {
    use super::*;
    #[test]
    fn reference_descriptions_preserve_revisions_without_export_package_limits() -> Result<()> {
        let root = tempfile::tempdir()?;
        fs::write(
            root.path().join("SKILL.md"),
            "---\nname: shared-source\ndescription: Read shared resources\n---\nInstructions\n",
        )?;
        fs::write(root.path().join("large.bin"), vec![b'a'; 100 * 1024])?;
        for i in 0..35 {
            fs::write(root.path().join(format!("resource-{i}.txt")), b"resource")?;
        }
        let (metadata, before, count) = describe_directory(root.path())?;
        assert_eq!(metadata["name"], "shared-source");
        assert_eq!(count, 36);
        assert_eq!(describe_directory(root.path())?.1, before);
        fs::write(root.path().join("large.bin"), vec![b'b'; 100 * 1024])?;
        assert_ne!(describe_directory(root.path())?.1, before);
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("large.bin", root.path().join("linked"))?;
            assert_eq!(describe_directory(root.path())?.2, 37);
        }
        Ok(())
    }
}
fn descriptor(state: &AppState, id: &str) -> Result<Value> {
    valid_id(id)?;
    let path = zork_config::skill_bundles::skills_root(&state.config.data_root).join(id);
    ensure!(
        fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.is_dir()),
        "skill_not_installed"
    );
    let (meta, revision, resource_count) = describe_directory(&path)?;
    Ok(
        json!({"target":node_access::identity(state),"skill_id":id,"name":meta["name"],"description":meta["description"],"revision":revision,"path":path,"source":shared_reference(state,&path).ok(),"resource_count":resource_count}),
    )
}
fn bindings(state: &AppState, path: &Path) -> Result<Vec<Value>> {
    let reference = shared_reference(state, path).ok();
    Ok(state
        .db
        .node_agents()?
        .iter()
        .filter(|a| {
            a.skill_paths.iter().any(|p| {
                p == path
                    || reference
                        .as_ref()
                        .is_some_and(|r| p.to_string_lossy() == r.as_str())
            })
        })
        .map(|a| json!({"agent_id":a.id,"name":a.name}))
        .collect())
}
pub(crate) fn client_resources(
    state: &AppState,
) -> Result<Vec<zork_client_types::resources::Resource>> {
    use zork_client_types::resources::{Resource, ResourceKind, ResourceSubject};
    let _gate = state.node_tools.gate.lock().expect("managed skills");
    let root = zork_config::skill_bundles::skills_root(&state.config.data_root);
    if !root.exists() {
        return Ok(vec![]);
    }
    let mut items = vec![];
    for entry in fs::read_dir(&root)? {
        let entry = entry?;
        let id = entry.file_name().to_string_lossy().into_owned();
        if valid_id(&id).is_err()
            || !entry.file_type()?.is_dir()
            || !entry.path().join("SKILL.md").is_file()
            || zork_agent::skills::bundled::is_managed(&entry.path())
        {
            continue;
        }
        let data = descriptor(state, &id)?;
        let subjects = bindings(
            state,
            Path::new(data["path"].as_str().context("skill_invalid_path")?),
        )?
        .iter()
        .map(|agent| ResourceSubject {
            id: agent["agent_id"].as_str().unwrap_or_default().into(),
            name: agent["name"].as_str().unwrap_or_default().into(),
            origin: None,
        })
        .collect::<Vec<_>>();
        let mut item = Resource::new(
            ResourceKind::Skill,
            id,
            data["name"].as_str().unwrap_or_default().into(),
            "installed".into(),
            if subjects.is_empty() {
                "unbound"
            } else {
                "bound"
            }
            .into(),
        );
        item.description = data["description"].as_str().unwrap_or_default().into();
        item.path = data["path"].as_str().map(str::to_owned);
        item.revision = data["revision"].as_str().map(str::to_owned);
        item.resource_count = data["resource_count"].as_u64().map(|count| count as usize);
        item.subjects = subjects;
        items.push(item);
    }
    items.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
    Ok(items)
}
