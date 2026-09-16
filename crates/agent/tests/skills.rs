use std::{fs, path::Path};
use zork_agent::skills::{discover, read_document};

fn skill(root: &Path, path: &str, name: &str, description: &str) {
    let path = root.join(path);
    fs::create_dir_all(&path).unwrap();
    fs::write(
        path.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {description}\n---\nPrivate instructions"),
    )
    .unwrap();
}

#[test]
fn shared_device_and_agent_sources_keep_same_names_without_implicit_device_loading() {
    let temp = tempfile::tempdir().unwrap();
    let shared = temp.path().join("skills");
    let device = temp.path().join("devices/one/skills");
    let agent = temp.path().join("agents/worker/skills");
    skill(&shared, "group/build", "build", "Shared build");
    skill(&device, "build", "build", "Device build");
    skill(&agent, "build", "build", "Agent build");
    skill(
        temp.path(),
        "devices/two/skills/secret",
        "other-device",
        "Only on two",
    );
    skill(
        &shared,
        "group/build/examples/nested",
        "not-a-skill",
        "Resource subtree",
    );
    let catalog = discover(&[shared.clone()]);
    assert_eq!(catalog.skills.len(), 1);
    assert_eq!(catalog.skills[0].description, "Shared build");
    let catalog = discover(&[
        shared.clone(),
        device,
        agent,
        shared.join("group/build"),
        shared,
    ]);
    assert_eq!(catalog.skills.len(), 3);
    assert!(catalog.skills.iter().all(|s| s.name == "build"));
    assert_eq!(
        catalog
            .skills
            .iter()
            .map(|s| s.description.as_str())
            .collect::<std::collections::BTreeSet<_>>(),
        ["Agent build", "Device build", "Shared build"]
            .into_iter()
            .collect()
    );
    assert!(catalog.diagnostics.is_empty());
    assert!(!catalog.notice().contains("Private instructions"));
    assert!(read_document(&catalog.skills[0].path)
        .unwrap()
        .contains("Private instructions"));
}

#[test]
fn missing_invalid_and_oversized_files_do_not_hide_valid_skills_and_changes_refresh() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    skill(root, "valid", "valid", ">-\n  First line\n  second line");
    skill(root, "invalid", "invalid", "");
    skill(root, "oversized", "oversized", "Too large");
    fs::write(root.join("oversized/SKILL.md"), "x".repeat(128 * 1024 + 1)).unwrap();
    let paths = vec![root.to_owned(), root.join("missing")];
    let before = discover(&paths);
    assert_eq!(before.skills.len(), 1);
    assert_eq!(before.skills[0].description, "First line second line");
    assert_eq!(before.diagnostics.len(), 3);
    let path = root.join("valid/SKILL.md");
    let text = fs::read_to_string(&path).unwrap();
    fs::write(&path, format!("{text}\nUpdated body")).unwrap();
    assert_ne!(
        before.skills[0].content_hash,
        discover(&paths).skills[0].content_hash
    );
    fs::remove_file(path).unwrap();
    assert!(discover(&paths).skills.is_empty());
}

#[cfg(unix)]
#[test]
fn explicit_symlink_roots_work_without_following_child_symlinks_or_cycles() {
    use std::os::unix::fs::symlink;
    let temp = tempfile::tempdir().unwrap();
    skill(temp.path(), "source/group/skill", "real", "Real skill");
    let root = temp.path().join("source");
    symlink(&root, root.join("cycle")).unwrap();
    skill(temp.path(), "device/only", "device-only", "Device skill");
    symlink(temp.path().join("device"), root.join("device")).unwrap();
    let alias = temp.path().join("alias");
    symlink(&root, &alias).unwrap();
    let catalog = discover(&[alias]);
    assert_eq!(catalog.skills.len(), 1);
    assert_eq!(catalog.skills[0].name, "real");
}

#[tokio::test]
async fn embedded_runtime_provisions_skill_files_without_registering_skill_wrappers() {
    use std::sync::Arc;
    use zork_agent::session::tools::{ToolContext, ToolRegistry, ToolResolution, ToolVersion};
    use zork_agent::{AgentOptions, AgentRuntime};
    let temp = tempfile::tempdir().unwrap();
    skill(
        temp.path(),
        "skills/default",
        "default-skill",
        "Default source",
    );
    let registry = Arc::new(ToolRegistry::default());
    let mut runtime = AgentRuntime::start(AgentOptions {
        data_root: temp.path().to_owned(),
        fake_agent: true,
        tools: registry.clone(),
        profile_refresh_interval: None,
        ..Default::default()
    })
    .unwrap();
    for name in [
        "skill.list",
        "skill.sources",
        "skill.validate",
        "skill.write",
        "skill.archive",
        "skill.bundle",
    ] {
        assert!(registry.current_contract(name).is_none());
        assert!(registry.compatibility(name).is_some());
    }
    let context = ToolContext {
        control: None,
        session_id: "ordinary-session".into(),
        invocation_id: "invocation".into(),
        workspace: temp.path().to_string_lossy().into_owned(),
    };
    let sources = zork_config::SkillsConfig::default()
        .sources(temp.path(), &[])
        .unwrap();
    let catalog = discover(&sources);
    assert!(catalog
        .skills
        .iter()
        .any(|skill| skill.name == "default-skill"));
    assert!(catalog
        .skills
        .iter()
        .any(|skill| skill.name == "file-sharing"));
    assert!(catalog.diagnostics.is_empty());
    assert!(matches!(
        registry.resolve("skill.read", None),
        ToolResolution::Unavailable
    ));
    let guide = catalog
        .skills
        .iter()
        .find(|skill| skill.name == "skill-management")
        .unwrap();
    let path = guide.path.to_str().unwrap();
    assert!(Path::new(path).is_file());
    let ToolResolution::Ready(reader) =
        registry.resolve("file.read", Some(&ToolVersion::new("builtin-2").unwrap()))
    else {
        panic!("missing file.read");
    };
    let mut text = String::new();
    let mut offset = 0;
    loop {
        let page = reader
            .execute(
                &context,
                &serde_json::json!({"path":path,"offset":offset,"limit":128}),
            )
            .await;
        text.push_str(page.data["content"].as_str().unwrap());
        match page.data["next_offset"].as_u64() {
            Some(next) => offset = next,
            None => break,
        }
    }
    assert_eq!(text, fs::read_to_string(path).unwrap());
    assert!(text.contains("file.read"));
    runtime.shutdown().await;
}

#[cfg(target_os = "linux")]
#[test]
fn non_utf8_directory_is_reported_without_panicking_catalog_serialization() {
    use std::os::unix::ffi::OsStringExt;
    let temp = tempfile::tempdir().unwrap();
    let invalid = temp.path().join(std::ffi::OsString::from_vec(vec![255]));
    fs::create_dir(&invalid).unwrap();
    fs::write(
        invalid.join("SKILL.md"),
        "---\nname: broken\ndescription: Invalid path\n---\nbody",
    )
    .unwrap();
    skill(temp.path(), "valid", "valid", "Valid path");
    let catalog = discover(&[temp.path().to_owned()]);
    assert_eq!(catalog.skills.len(), 1);
    assert_eq!(catalog.diagnostics.len(), 1);
    assert!(catalog.notice().contains("UTF-8"));
}
