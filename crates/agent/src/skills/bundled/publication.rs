//! Current files are a repairable view. Selection points to one complete
//! immutable Skill version, including while this view is being refreshed.
use super::*;
use zork_config::skill_bundles::PublishedSkill;

pub(super) fn skill_files(manifest: &Manifest, id: &str) -> BTreeMap<String, String> {
    manifest
        .files
        .iter()
        .filter_map(|(path, hash)| {
            path.strip_prefix(&format!("{id}/"))
                .map(|p| (p.into(), hash.clone()))
        })
        .collect()
}
pub(super) fn contents(directory: &Path) -> Option<BTreeMap<String, String>> {
    if !fs::symlink_metadata(directory).ok()?.is_dir() {
        return None;
    }
    let mut files = BTreeMap::new();
    inventory(directory, Path::new(""), &mut files).ok()?;
    Some(files)
}
pub(super) fn owned(root: &Path, id: &str, directory: &Path) -> bool {
    let Some(files) = contents(directory) else {
        return false;
    };
    let interrupted = root.join(".publication.json").is_file();
    let versions = fs::read_dir(root.join(".versions"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| read_manifest(&entry.path()).ok())
        .collect::<Vec<_>>();
    versions
        .iter()
        .any(|manifest| files == skill_files(manifest, id))
        || (interrupted
            && files.iter().all(|(path, hash)| {
                versions
                    .iter()
                    .any(|manifest| skill_files(manifest, id).get(path) == Some(hash))
            }))
}
pub(super) fn activate(data_root: &Path, root: &Path, state: &mut BundleState) -> Result<()> {
    let active = state.active.clone().context("Skill selection missing")?;
    ensure!(
        active.skills.len() == 1,
        "one metadata directory owns one Skill"
    );
    let id = &active.skills[0];
    let release = root.join(".versions").join(&active.directory);
    let manifest = read_manifest(&release)?;
    ensure!(
        intact(&release, &manifest),
        "selected Skill version is incomplete"
    );
    let public = root.parent().context("Skill directory missing")?;
    let staging = data_root
        .join("state/skill-staging")
        .join(format!("view-{}", ulid::Ulid::new()));
    let prepared = staging.join("view");
    fs::create_dir_all(&prepared)?;
    let files = skill_files(&manifest, id);
    if contents(public).as_ref() == Some(&files) && !root.join(".publication.json").exists() {
        state.published.insert(
            id.clone(),
            PublishedSkill {
                directory: public.file_name().unwrap().to_string_lossy().into(),
                version: active.directory,
            },
        );
        remove_stage(&staging);
        return save(root, state);
    }
    let result = (|| -> Result<()> {
        for relative in files.keys() {
            let target = prepared.join(relative);
            fs::create_dir_all(target.parent().unwrap())?;
            fs::copy(release.join(id).join(relative), &target)?;
        }
        ensure!(
            contents(&prepared).as_ref() == Some(&files),
            "Skill view verification failed"
        );
        // A marker distinguishes interrupted publication from external edits.
        atomic_json(&root.join(".publication.json"), &active.directory)?;
        for entry in fs::read_dir(public)? {
            let entry = entry?;
            if entry.file_name() == ".zork" {
                continue;
            }
            fs::rename(entry.path(), staging.join(entry.file_name()))?;
        }
        for entry in fs::read_dir(&prepared)? {
            let entry = entry?;
            fs::rename(entry.path(), public.join(entry.file_name()))?;
        }
        state.published.insert(
            id.clone(),
            PublishedSkill {
                directory: public.file_name().unwrap().to_string_lossy().into(),
                version: active.directory,
            },
        );
        save(root, state)?;
        fs::remove_file(root.join(".publication.json"))?;
        sync_directory(public)
    })();
    remove_stage(&staging);
    result
}
