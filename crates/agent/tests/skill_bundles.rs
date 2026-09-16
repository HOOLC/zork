use std::{
    fs,
    path::{Path, PathBuf},
};
use zork_agent::skills::{
    self,
    bundled::{self, BundleFile, Request},
};

fn doc(text: &str) -> String {
    format!("---\nname: shared-name\ndescription: {text}\n---\n{text}\n")
}
fn install(root: &Path, text: &str) -> String {
    let guide = doc(text);
    let script = format!("echo {text}\n");
    bundled::install(
        root,
        &[
            BundleFile {
                path: "guide/SKILL.md",
                content: guide.as_bytes(),
            },
            BundleFile {
                path: "guide/scripts/run.sh",
                content: script.as_bytes(),
            },
        ],
    )
    .unwrap()
}
fn release(root: &Path, version: &str) -> PathBuf {
    zork_config::skill_bundles::skills_root(root)
        .join("guide/.zork/.versions")
        .join(version)
}
fn sources(root: &Path) -> Vec<PathBuf> {
    zork_config::skill_bundles::sources(root).unwrap()
}

fn status(root: &Path) -> serde_json::Value {
    bundled::manage(root, Request::List).unwrap()["skills"][0].clone()
}

#[test]
fn fresh_builtins_are_readable_before_provisioning_returns() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    skills::management::provision_bundled(root).unwrap();
    let listed = bundled::manage(root, Request::List).unwrap();
    for id in ["slack", "android-debugging", "skill-management", "service-sharing", "file-sharing"] {
        let entry = listed["skills"].as_array().unwrap().iter()
            .find(|entry| entry["skill"] == id).unwrap();
        let directory = PathBuf::from(entry["directory"].as_str().unwrap());
        assert!(fs::read_to_string(directory.join("SKILL.md")).unwrap().contains("description:"));
        let state = zork_config::skill_bundles::load(&directory.join(".zork")).unwrap();
        let active = state.active.unwrap();
        assert!(directory.join(".zork/.versions").join(active.directory).join(id).join("SKILL.md").is_file());
    }
}

#[test]
fn complete_updates_keep_old_files_and_rollback_persists_until_a_new_distribution() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let first = install(root, "first");
    assert_eq!(install(root, "first"), first);
    let second = install(root, "second");
    assert_ne!(first, second);
    assert_eq!(
        fs::read_to_string(sources(root)[0].join("scripts/run.sh")).unwrap(),
        "echo second\n"
    );
    assert_eq!(
        fs::read_to_string(release(root, &first).join("guide/SKILL.md")).unwrap(),
        doc("first")
    );
    bundled::manage(
        root,
        Request::Rollback {
            skill: "guide".into(),
            version: first.clone(),
        },
    )
    .unwrap();
    assert_eq!(install(root, "second"), first);
    assert_eq!(
        fs::read_to_string(sources(root)[0].join("SKILL.md")).unwrap(),
        doc("first")
    );
    let third = install(root, "third");
    assert_ne!(third, first);
    let state = status(root);
    assert_eq!(state["active"]["directory"], third);
    assert_eq!(state["rollback"], false);
    assert_eq!(state["versions"].as_array().unwrap().len(), 3);
}

#[test]
fn disable_survives_updates_restart_and_rollback_without_touching_custom_files() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let first = install(root, "first");
    let custom = zork_config::skill_bundles::skills_root(root).join("my-copy");
    fs::create_dir_all(custom.join("scripts")).unwrap();
    fs::copy(
        sources(root)[0].join("scripts/run.sh"),
        custom.join("scripts/run.sh"),
    )
    .unwrap();
    fs::write(custom.join("SKILL.md"), doc("custom")).unwrap();
    bundled::manage(
        root,
        Request::Disable {
            skill: "guide".into(),
        },
    )
    .unwrap();
    install(root, "second");
    assert!(sources(root).is_empty());
    bundled::manage(
        root,
        Request::Rollback {
            skill: "guide".into(),
            version: first,
        },
    )
    .unwrap();
    assert!(sources(root).is_empty());
    assert_eq!(
        fs::read_to_string(custom.join("SKILL.md")).unwrap(),
        doc("custom")
    );
    assert_eq!(
        fs::read_to_string(custom.join("scripts/run.sh")).unwrap(),
        "echo first\n"
    );
    let catalog = skills::discover(
        &zork_config::SkillsConfig::default()
            .sources(root, &[])
            .unwrap(),
    );
    assert_eq!(catalog.skills.len(), 1);
    assert_eq!(
        catalog.skills[0].path,
        fs::canonicalize(custom.join("SKILL.md")).unwrap()
    );
    bundled::manage(
        root,
        Request::Enable {
            skill: "guide".into(),
        },
    )
    .unwrap();
    assert_eq!(sources(root).len(), 1);
    assert!(bundled::manage(
        root,
        Request::Disable {
            skill: "unknown".into()
        }
    )
    .is_err());
}

#[test]
fn invalid_release_is_replaced_without_creating_custom_backups() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let first = install(root, "first");
    let file = release(root, &first).join("guide/scripts/run.sh");
    fs::remove_file(&file).unwrap();
    fs::write(&file, "invalid release resource").unwrap();
    install(root, "second");
    assert_eq!(
        fs::read_dir(zork_config::skill_bundles::skills_root(root))
            .unwrap()
            .count(),
        1
    ); // management data lives inside the current Skill
    assert_eq!(
        fs::read_to_string(sources(root)[0].join("scripts/run.sh")).unwrap(),
        "echo second\n"
    );
    assert!(bundled::manage(
        root,
        Request::Rollback {
            skill: "guide".into(),
            version: first
        }
    )
    .is_err());
}

#[test]
fn one_skill_directory_keeps_current_files_and_immutable_old_sources() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let first = install(root, "first");
    let skills_root = zork_config::skill_bundles::skills_root(root);
    let current = skills_root.join("guide/SKILL.md");
    assert_eq!(fs::read_to_string(&current).unwrap(), doc("first"));
    assert!(fs::symlink_metadata(&current).unwrap().is_file());
    let pinned = sources(root)[0].join("SKILL.md");
    install(root, "second");
    assert_eq!(fs::read_to_string(&current).unwrap(), doc("second"));
    assert_eq!(fs::read_to_string(pinned).unwrap(), doc("first"));
    let catalog = skills::discover(
        &zork_config::SkillsConfig::default()
            .sources(root, &[])
            .unwrap(),
    );
    assert!(catalog.diagnostics.is_empty(), "{:?}", catalog.diagnostics);
    assert_eq!(catalog.skills.len(), 1);
    bundled::manage(
        root,
        Request::Rollback {
            skill: "guide".into(),
            version: first,
        },
    )
    .unwrap();
    assert_eq!(fs::read_to_string(&current).unwrap(), doc("first"));
    for obsolete in ["bundled-skills", "custom-skills", "managed-skills"] {
        assert!(!zork_config::files_root(root).join(obsolete).exists());
    }
}

#[test]
fn same_directory_names_and_user_edits_are_preserved_on_upgrade() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let skills_root = zork_config::skill_bundles::skills_root(root);
    fs::create_dir_all(skills_root.join("guide")).unwrap();
    fs::write(skills_root.join("guide/SKILL.md"), doc("user")).unwrap();
    install(root, "first");
    let metadata = zork_config::skill_bundles::managed(root).unwrap()[0]
        .0
        .clone();
    let state = zork_config::skill_bundles::load(&metadata).unwrap();
    let published = skills_root.join(&state.published["guide"].directory);
    assert_ne!(published, skills_root.join("guide"));
    fs::remove_file(published.join("SKILL.md")).unwrap();
    fs::write(published.join("SKILL.md"), doc("edited")).unwrap();
    install(root, "second");
    assert_eq!(
        fs::read_to_string(skills_root.join("guide/SKILL.md")).unwrap(),
        doc("user")
    );
    assert_eq!(
        fs::read_to_string(published.join("SKILL.md")).unwrap(),
        doc("edited")
    );
    let catalog = skills::discover(
        &zork_config::SkillsConfig::default()
            .sources(root, &[])
            .unwrap(),
    );
    assert_eq!(catalog.skills.len(), 3);
    assert!(catalog.diagnostics.is_empty(), "{:?}", catalog.diagnostics);
}

#[test]
fn rollback_rejects_external_changes_without_changing_selection_or_files() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    let first = install(root, "first");
    let second = install(root, "second");
    let file = root.join("skills/guide/scripts/run.sh");
    fs::remove_file(&file).unwrap();
    fs::write(&file, "user's only copy").unwrap();
    assert!(bundled::manage(
        root,
        Request::Rollback {
            skill: "guide".into(),
            version: first
        }
    )
    .is_err());
    assert_eq!(fs::read_to_string(file).unwrap(), "user's only copy");
    assert_eq!(status(root)["active"]["directory"], second);
    assert_eq!(status(root)["rollback"], false);
}

#[test]
fn a_replacement_directory_preserves_disabled_selection_history_and_pending_handoff() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    let first = install(root, "first");
    let second = install(root, "second");
    bundled::manage(
        root,
        Request::Rollback {
            skill: "guide".into(),
            version: first.clone(),
        },
    )
    .unwrap();
    bundled::manage(
        root,
        Request::Disable {
            skill: "guide".into(),
        },
    )
    .unwrap();
    let old_metadata = root.join("skills/guide/.zork");
    let mut previous = zork_config::skill_bundles::load(&old_metadata).unwrap();
    let file = root.join("skills/guide/scripts/run.sh");
    fs::remove_file(&file).unwrap();
    fs::write(&file, "preserved edit").unwrap();
    assert_eq!(install(root, "second"), first);
    assert!(sources(root).is_empty());
    assert_eq!(fs::read_to_string(&file).unwrap(), "preserved edit");
    let state = status(root);
    assert_eq!(state["active"]["directory"], first);
    assert_eq!(state["rollback"], true);
    assert_eq!(state["versions"].as_array().unwrap().len(), 2);
    let new_directory = PathBuf::from(state["directory"].as_str().unwrap());
    assert_ne!(new_directory, root.join("skills/guide"));
    assert!(release(root, &second).join("guide/SKILL.md").is_file());
    // Simulate a stop after the successor committed, before the old record
    // was retired. Discovery must still offer only the successor's selection.
    previous.successor = Some(new_directory.file_name().unwrap().to_str().unwrap().into());
    fs::write(
        old_metadata.join(".state.json"),
        serde_json::to_vec(&previous).unwrap(),
    )
    .unwrap();
    assert_eq!(zork_config::skill_bundles::managed(root).unwrap().len(), 1);
    assert!(!zork_config::skill_bundles::published_directory(&root.join("skills/guide")).unwrap());
    assert_eq!(install(root, "second"), first);
    assert!(sources(root).is_empty());
    bundled::manage(
        root,
        Request::Enable {
            skill: "guide".into(),
        },
    )
    .unwrap();
    bundled::manage(
        root,
        Request::Rollback {
            skill: "guide".into(),
            version: second,
        },
    )
    .unwrap();
    assert_eq!(
        fs::read_to_string(sources(root)[0].join("scripts/run.sh")).unwrap(),
        "echo second\n"
    );
    assert_eq!(fs::read_to_string(file).unwrap(), "preserved edit");
}

#[test]
fn malformed_management_metadata_is_diagnosed_without_blocking_healthy_skills() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    install(root, "first");
    let bad = root.join("skills/unrelated/.zork/.state.json");
    fs::create_dir_all(bad.parent().unwrap()).unwrap();
    fs::write(&bad, "{").unwrap();
    skills::management::provision_bundled(root).unwrap();
    let paths = zork_config::SkillsConfig::default()
        .sources(root, &[])
        .unwrap();
    let catalog = skills::discover(&paths);
    assert!(catalog
        .skills
        .iter()
        .any(|skill| skill.name == "shared-name"));
    assert!(catalog
        .skills
        .iter()
        .any(|skill| skill.name == "skill-management"));
    assert!(catalog
        .diagnostics
        .iter()
        .any(|message| message.contains("unrelated")));
    let status = bundled::manage(root, Request::List).unwrap();
    assert!(status["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .any(|message| message.as_str().unwrap().contains("unrelated")));
    assert_eq!(fs::read_to_string(bad).unwrap(), "{");
}

#[test]
fn a_damaged_builtin_is_not_recreated_with_a_reset_selection() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    skills::management::provision_bundled(root).unwrap();
    bundled::manage(
        root,
        Request::Disable {
            skill: "skill-management".into(),
        },
    )
    .unwrap();
    let public = root.join("skills/skill-management/SKILL.md");
    fs::remove_file(&public).unwrap();
    fs::write(&public, doc("customized management guide")).unwrap();
    skills::management::provision_bundled(root).unwrap();
    let listed = bundled::manage(root, Request::List).unwrap();
    let moved = listed["skills"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["skill"] == "skill-management")
        .unwrap();
    let state = PathBuf::from(moved["directory"].as_str().unwrap()).join(".zork/.state.json");
    fs::write(&state, "{").unwrap();
    skills::management::provision_bundled(root).unwrap();
    let listed = bundled::manage(root, Request::List).unwrap();
    assert!(!listed["skills"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["skill"] == "skill-management"));
    assert!(listed["skills"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["skill"] == "service-sharing"));
    assert_eq!(fs::read_to_string(state).unwrap(), "{");
}

#[test]
fn restart_repairs_missing_or_stale_current_files_from_selected_complete_release() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let first = install(root, "first");
    install(root, "second");
    let current = zork_config::skill_bundles::skills_root(root).join("guide");
    fs::remove_file(current.join("SKILL.md")).unwrap();
    fs::copy(
        release(root, &first).join("guide/SKILL.md"),
        current.join("SKILL.md"),
    )
    .unwrap();
    fs::remove_file(current.join("scripts/run.sh")).unwrap();
    fs::copy(
        release(root, &first).join("guide/scripts/run.sh"),
        current.join("scripts/run.sh"),
    )
    .unwrap();
    install(root, "second");
    assert_eq!(
        fs::read_to_string(current.join("SKILL.md")).unwrap(),
        doc("second")
    );
    fs::remove_dir_all(&current).unwrap();
    install(root, "second");
    assert_eq!(
        fs::read_to_string(current.join("scripts/run.sh")).unwrap(),
        "echo second\n"
    );
}

#[test]
fn invalid_or_partial_releases_never_replace_active_selection() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let first = install(root, "first");
    for files in [
        vec![BundleFile {
            path: "../escape",
            content: b"bad",
        }],
        vec![BundleFile {
            path: "guide/script.sh",
            content: b"missing manifest",
        }],
        vec![BundleFile {
            path: "guide/SKILL.md",
            content: b"invalid",
        }],
    ] {
        assert!(bundled::install(root, &files).is_err());
        assert_eq!(status(root)["active"]["directory"], first);
    }
    // A half-written directory is not offered as a rollback candidate.
    fs::create_dir_all(
        zork_config::skill_bundles::skills_root(root).join("guide/.zork/.stage-interrupted/guide"),
    )
    .unwrap();
    fs::write(
        zork_config::skill_bundles::skills_root(root)
            .join("guide/.zork/.stage-interrupted/guide/SKILL.md"),
        doc("partial"),
    )
    .unwrap();
    assert_eq!(status(root)["versions"].as_array().unwrap().len(), 1);
    assert!(bundled::manage(
        root,
        Request::Rollback {
            skill: "guide".into(),
            version: "../escape".into()
        }
    )
    .is_err());
}

#[test]
fn failure_while_staging_preserves_the_previous_complete_release() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let first = install(root, "first");
    let content = doc("broken new version");
    let result = bundled::install(
        root,
        &[
            BundleFile {
                path: "guide/SKILL.md",
                content: content.as_bytes(),
            },
            BundleFile {
                path: "guide/scripts",
                content: b"a file, not a directory",
            },
            BundleFile {
                path: "guide/scripts/run.sh",
                content: b"cannot be staged",
            },
        ],
    );
    assert!(result.is_err());
    assert_eq!(status(root)["active"]["directory"], first);
    assert_eq!(
        fs::read_to_string(sources(root)[0].join("scripts/run.sh")).unwrap(),
        "echo first\n"
    );
    assert!(
        !fs::read_dir(zork_config::skill_bundles::skills_root(root).join("guide/.zork"))
            .unwrap()
            .any(|entry| entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".stage-"))
    );
}

#[test]
fn each_skill_updates_and_rolls_back_without_changing_its_neighbors() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let first = install(root, "first");
    let other = doc("other");
    bundled::install(
        root,
        &[BundleFile {
            path: "other/SKILL.md",
            content: other.as_bytes(),
        }],
    )
    .unwrap();
    let old_other = fs::read(root.join("skills/other/.zork/.state.json")).unwrap();
    install(root, "second");
    bundled::manage(
        root,
        Request::Rollback {
            skill: "guide".into(),
            version: first,
        },
    )
    .unwrap();
    assert_eq!(
        fs::read(root.join("skills/other/.zork/.state.json")).unwrap(),
        old_other
    );
    assert_eq!(
        fs::read_to_string(root.join("skills/guide/SKILL.md")).unwrap(),
        doc("first")
    );
    assert!(!root.join("skills/.zork").exists());
}
