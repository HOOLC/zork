//! Release provisioning and manifest validation. Skills are ordinary files.
use super::*;
use serde_json::Value;

pub fn provision_bundled(data_root: &Path) -> Result<()> {
    let files = [
        super::bundled::BundleFile {
            path: "slack/SKILL.md",
            content: include_bytes!("../../skills/slack/SKILL.md"),
        },
        super::bundled::BundleFile {
            path: "android-debugging/SKILL.md",
            content: include_bytes!("../../skills/android-debugging/SKILL.md"),
        },
        super::bundled::BundleFile {
            path: "skill-management/SKILL.md",
            content: include_bytes!("../../skills/skill-management/SKILL.md"),
        },
        super::bundled::BundleFile {
            path: "service-sharing/SKILL.md",
            content: include_bytes!("../../skills/service-sharing/SKILL.md"),
        },
        super::bundled::BundleFile {
            path: "file-sharing/SKILL.md",
            content: include_bytes!("../../skills/file-sharing/SKILL.md"),
        },
        super::bundled::BundleFile {
            path: "agent-delegation/SKILL.md",
            content: include_bytes!("../../skills/agent-delegation/SKILL.md"),
        },
    ];
    let mut groups = BTreeMap::<_, Vec<_>>::new();
    for file in files {
        groups
            .entry(file.path.split('/').next().unwrap())
            .or_default()
            .push(file);
    }
    match super::bundled::install_initial(data_root, &groups) {
        Ok(true) => return Ok(()),
        Ok(false) => {}
        Err(error) => {
            tracing::warn!(%error, "Initial Skill provisioning unavailable; checking each distribution")
        }
    }
    for (skill, files) in groups {
        if let Err(error) = super::bundled::install(data_root, &files) {
            tracing::warn!(skill, error = %error, "Skill provisioning failed; other skills remain available");
        }
    }
    Ok(())
}

fn digest(content: &str) -> String {
    format!("{:x}", Sha256::digest(content.as_bytes()))
}

pub fn validate(content: &str) -> Result<Value> {
    ensure!(
        content.len() as u64 <= MAX_FILE_BYTES,
        "SKILL.md exceeds 128 KiB"
    );
    let (name, description) = metadata(content)?;
    let mut lines = content.lines().skip(1);
    lines.by_ref().find(|line| line.trim() == "---");
    ensure!(
        lines.any(|line| !line.trim().is_empty()),
        "skill instructions must not be empty"
    );
    Ok(json!({"name":name,"description":description,"content_hash":digest(content),"valid":true}))
}
