//! Publish a new installation as complete directories, with two disk barriers.
//!
//! macOS fsync submits one file's data and metadata to the device. F_FULLFSYNC
//! then flushes the whole device cache. Submit all prepared files first, flush
//! once before making them visible, then flush the committed parent directory.
//! https://developer.apple.com/videos/play/wwdc2019/419/
use super::*;
use std::{
    ffi::CString,
    os::unix::{ffi::OsStrExt, io::AsRawFd},
};
use zork_config::skill_bundles::PublishedSkill;

struct PreparedSkill {
    id: String,
    path: PathBuf,
    state: BundleState,
}

struct Batch {
    path: PathBuf,
    barrier: fs::File,
    skills: Vec<PreparedSkill>,
}

impl Drop for Batch {
    fn drop(&mut self) {
        remove_stage(&self.path);
    }
}

fn write_new(path: &Path, bytes: &[u8], readonly: bool) -> Result<()> {
    fs::create_dir_all(path.parent().context("initial Skill file parent")?)?;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(bytes)?;
    if readonly {
        let mut permissions = file.metadata()?.permissions();
        permissions.set_readonly(true);
        file.set_permissions(permissions)?;
    }
    Ok(())
}

fn submit(path: &Path) -> Result<()> {
    let file = fs::File::open(path)?;
    loop {
        if unsafe { libc::fsync(file.as_raw_fd()) } == 0 {
            return Ok(());
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::Interrupted {
            return Err(error).with_context(|| format!("sync {}", path.display()));
        }
    }
}

fn submit_tree(path: &Path) -> Result<()> {
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_dir() {
            submit_tree(&entry.path())?;
        } else {
            ensure!(kind.is_file(), "initial Skill contains a non-regular file");
            submit(&entry.path())?;
        }
    }
    submit(path)
}

fn rename_new(source: &Path, target: &Path) -> std::io::Result<()> {
    let source = CString::new(source.as_os_str().as_bytes())?;
    let target = CString::new(target.as_os_str().as_bytes())?;
    if unsafe { libc::renamex_np(source.as_ptr(), target.as_ptr(), libc::RENAME_EXCL) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

impl Batch {
    fn prepare(public: &Path, groups: &BTreeMap<&str, Vec<BundleFile<'_>>>) -> Result<Self> {
        // Staying on the public directory's filesystem also supports a source
        // root mounted elsewhere. Hidden preparation is not a discovery source.
        let path = public.join(format!(".initial-{}", ulid::Ulid::new()));
        fs::create_dir(&path)?;
        let barrier = match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path.join(".flush"))
        {
            Ok(file) => file,
            Err(error) => {
                let _ = fs::remove_dir(&path);
                return Err(error.into());
            }
        };
        let mut batch = Self {
            path,
            barrier,
            skills: Vec::new(),
        };
        for (id, files) in groups {
            let manifest = manifest(files)?;
            ensure!(manifest.skills == [*id], "invalid initial Skill group");
            let path = batch.path.join(id);
            let version = format!("{}-{}", &manifest.revision[..12], ulid::Ulid::new());
            let release = path.join(".zork/.versions").join(&version);
            for file in files {
                let relative = file
                    .path
                    .strip_prefix(&format!("{id}/"))
                    .context("Skill file prefix")?;
                write_new(&release.join(file.path), file.content, true)?;
                write_new(&path.join(relative), file.content, true)?;
            }
            write_new(
                &release.join(MANIFEST),
                &serde_json::to_vec_pretty(&manifest)?,
                true,
            )?;
            ensure!(
                intact(&release, &manifest),
                "initial Skill version verification failed"
            );
            ensure!(
                publication::contents(&path).as_ref()
                    == Some(&publication::skill_files(&manifest, id)),
                "initial Skill view verification failed"
            );
            let state = BundleState {
                active: Some(BundleSelection {
                    directory: version.clone(),
                    skills: vec![(*id).into()],
                }),
                distribution_revision: Some(manifest.revision),
                published: BTreeMap::from([(
                    (*id).into(),
                    PublishedSkill {
                        directory: (*id).into(),
                        version,
                    },
                )]),
                ..Default::default()
            };
            state.validate()?;
            write_new(
                &path.join(".zork/.state.json"),
                &serde_json::to_vec_pretty(&state)?,
                false,
            )?;
            batch.skills.push(PreparedSkill {
                id: (*id).into(),
                path,
                state,
            });
        }
        Ok(batch)
    }

    fn seal(&self, public: &Path) -> Result<()> {
        submit_tree(&self.path)?;
        submit(public)?;
        submit(public.parent().context("Skill root parent")?)?;
        // This regular file's full flush also commits the previously submitted
        // payloads, permissions and directory entries on the same device.
        self.barrier.sync_all()?;
        Ok(())
    }

    fn publish_one(&mut self, public: &Path, index: usize) -> Result<()> {
        let skill = &mut self.skills[index];
        loop {
            let target = public.join(&skill.state.published[&skill.id].directory);
            match rename_new(&skill.path, &target) {
                Ok(()) => return Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    // A user can create a directory while preparation is in
                    // progress. Never replace it, including an empty directory.
                    skill.state.published.get_mut(&skill.id).unwrap().directory =
                        format!("{}-{}", skill.id, ulid::Ulid::new());
                    let state = skill.path.join(".zork/.state.json");
                    fs::write(&state, serde_json::to_vec_pretty(&skill.state)?)?;
                    submit(&state)?;
                    self.barrier.sync_all()?;
                }
                Err(error) => return Err(error).context("publish initial Skill"),
            }
        }
    }

    fn commit(&mut self, public: &Path) -> Result<()> {
        self.seal(public)?;
        zork_config::startup::mark("agent.skills_initial_sealed");
        for index in 0..self.skills.len() {
            self.publish_one(public, index)?;
        }
        submit(&self.path)?;
        submit(public)?;
        self.barrier.sync_all()?;
        zork_config::startup::mark("agent.skills_initial_published");
        Ok(())
    }
}

pub(super) fn install(public: &Path, groups: &BTreeMap<&str, Vec<BundleFile<'_>>>) -> Result<()> {
    let mut batch = Batch::prepare(public, groups)?;
    zork_config::startup::mark("agent.skills_initial_prepared");
    batch.commit(public)
}

#[cfg(test)]
mod tests {
    use super::*;
    const DOCUMENT: &[u8] =
        b"---\nname: guide\ndescription: Read a guide\n---\nComplete instructions\n";

    fn groups() -> BTreeMap<&'static str, Vec<BundleFile<'static>>> {
        BTreeMap::from([(
            "guide",
            vec![
                BundleFile {
                    path: "guide/SKILL.md",
                    content: DOCUMENT,
                },
                BundleFile {
                    path: "guide/scripts/run.sh",
                    content: b"echo complete\n",
                },
            ],
        )])
    }

    #[test]
    fn prepared_files_are_hidden_and_publication_preserves_a_racing_user_directory() {
        let temp = tempfile::tempdir().unwrap();
        let public = zork_config::skill_bundles::skills_root(temp.path());
        fs::create_dir(&public).unwrap();
        let mut batch = Batch::prepare(&public, &groups()).unwrap();
        assert!(zork_config::skill_bundles::sources(temp.path())
            .unwrap()
            .is_empty());
        fs::create_dir(public.join("guide")).unwrap();
        fs::write(public.join("guide/SKILL.md"), b"user content").unwrap();
        batch.commit(&public).unwrap();
        drop(batch);
        assert_eq!(
            fs::read(public.join("guide/SKILL.md")).unwrap(),
            b"user content"
        );
        let sources = zork_config::skill_bundles::sources(temp.path()).unwrap();
        assert_eq!(sources.len(), 1);
        assert_eq!(fs::read(sources[0].join("SKILL.md")).unwrap(), DOCUMENT);
        assert_eq!(
            fs::read(sources[0].join("scripts/run.sh")).unwrap(),
            b"echo complete\n"
        );
        let listed = manage(temp.path(), Request::List).unwrap();
        assert_ne!(
            listed["skills"][0]["directory"],
            public.join("guide").to_str().unwrap()
        );
        assert_eq!(fs::read_dir(&public).unwrap().count(), 2);
    }

    #[test]
    fn interrupted_batch_only_advertises_complete_versions() {
        let temp = tempfile::tempdir().unwrap();
        let public = zork_config::skill_bundles::skills_root(temp.path());
        fs::create_dir(&public).unwrap();
        let mut files = groups();
        files.insert(
            "later",
            vec![BundleFile {
                path: "later/SKILL.md",
                content: DOCUMENT,
            }],
        );
        let mut batch = Batch::prepare(&public, &files).unwrap();
        batch.seal(&public).unwrap();
        batch.publish_one(&public, 0).unwrap();
        // The batch's final parent flush and cleanup have not happened yet.
        let sources = zork_config::skill_bundles::sources(temp.path()).unwrap();
        assert_eq!(sources.len(), 1);
        let prior = sources[0].clone();
        assert_eq!(
            fs::read(prior.join("scripts/run.sh")).unwrap(),
            b"echo complete\n"
        );
        drop(batch);
        super::super::install(temp.path(), &groups()["guide"]).unwrap();
        assert_eq!(
            zork_config::skill_bundles::sources(temp.path()).unwrap(),
            sources
        );
        assert!(prior.join("SKILL.md").is_file());
    }
}
