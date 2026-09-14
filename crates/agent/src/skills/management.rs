//! Mutations are explicit about source and directory; locks and hashes protect
//! cooperating writers while atomic rename keeps discovery from seeing drafts.
use super::*;
use crate::session::tools::{
    NoToolState, ToolContext, ToolContract, ToolExecution, ToolImplementation, ToolInstance,
    ToolRegistry, ToolVersion,
};
use serde::Deserialize;
use serde_json::Value;
use std::io::Write;

pub fn provision_bundled(data_root: &Path) -> Result<()> {
    super::bundled::install(
        data_root,
        &[
            super::bundled::BundleFile {
                path: "slack/SKILL.md",
                content: include_bytes!("../../skills/slack/SKILL.md"),
            },
            super::bundled::BundleFile {
                path: "skill-management/SKILL.md",
                content: include_bytes!("../../skills/skill-management/SKILL.md"),
            },
            super::bundled::BundleFile {
                path: "service-sharing/SKILL.md",
                content: include_bytes!("../../skills/service-sharing/SKILL.md"),
            },
        ],
    )?;
    Ok(())
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum SourceRequest {
    List,
    Add { path: PathBuf },
    Remove { path: PathBuf },
}
pub type SkillSourceManager = Arc<dyn Fn(&str, SourceRequest) -> Result<Value> + Send + Sync>;

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

#[derive(Clone, Copy)]
enum Kind {
    Sources,
    Validate,
    Write,
    Archive,
}

pub fn register(
    registry: &ToolRegistry,
    sources: SkillSources,
    manager: Option<SkillSourceManager>,
) -> Result<()> {
    let string = || json!({"type":"string","minLength":1});
    for (kind, name, description, properties, required) in [
        (Kind::Sources, "skill.sources", "Inspect effective skill directories with action=list. action=add/remove and path update only the current Agent's extra sources; removal uses the exact configured path. No files are copied or deleted. Shared/device defaults are read-only here.", json!({"action":{"type":"string","enum":["list","add","remove"]},"path":string()}), vec!["action"]),
        (Kind::Validate, "skill.validate", "Validate a complete SKILL.md draft without writing it; return metadata and content_hash. Does not validate factual instructions or execute scripts.", json!({"content":string()}), vec!["content"]),
        (Kind::Write, "skill.write", "Create or atomically update SKILL.md in an explicit configured source and relative directory. Supply the complete content. Existing manifests require their current expected_hash; omit it only for creation. Resources are preserved. Returns the new hash and catalog; same-name files remain separate candidates.", json!({"source":string(),"directory":string(),"content":string(),"expected_hash":string()}), vec!["source","directory","content"]),
        (Kind::Archive, "skill.archive", "Retire an explicit skill manifest using its current expected_hash. Move it into a hidden archive, retain resources and return archived_path plus the effective catalog. Other same-name files remain available. Restore by reading archived_path and using skill.write.", json!({"source":string(),"directory":string(),"expected_hash":string()}), vec!["source","directory","expected_hash"]),
    ] {
        registry.register(Arc::new(ToolInstance::new(ToolContract {
            name: name.into(), version: ToolVersion::new("2")?, initial_description: description.into(), detailed_description: description.into(),
            input_schema: json!({"type":"object","properties":properties,"required":required,"additionalProperties":false}),
        }, Arc::new(ManagementTool { kind, sources: sources.clone(), manager: manager.clone() }), Arc::new(NoToolState))?));
    }
    Ok(())
}

struct ManagementTool {
    kind: Kind,
    sources: SkillSources,
    manager: Option<SkillSourceManager>,
}
impl ToolImplementation for ManagementTool {
    fn execute<'a>(
        &'a self,
        context: &'a ToolContext,
        args: &'a Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolExecution> + Send + 'a>> {
        let kind = self.kind;
        let sources = self.sources.clone();
        let manager = self.manager.clone();
        let session = context.session_id.clone();
        let args = args.clone();
        Box::pin(async move {
            let result = tokio::task::spawn_blocking(move || -> Result<Value> {
                match kind {
                    Kind::Validate => validate(required(&args, "content")?),
                    Kind::Sources => {
                        let request: SourceRequest = serde_json::from_value(args)?;
                        if let Some(manager) = manager { return manager(&session, request); }
                        ensure!(matches!(request, SourceRequest::List), "This session has no editable Agent sources; node configuration owns its defaults");
                        Ok(json!({"editable":false,"sources":sources(&session)?}))
                    }
                    Kind::Write | Kind::Archive => mutate(&sources(&session)?, kind, &args),
                }
            }).await;
            match result {
                Ok(Ok(value)) => ToolExecution::success(value),
                error => {
                    let message = match error {
                        Ok(Err(e)) => e.to_string(),
                        Err(e) => e.to_string(),
                        _ => unreachable!(),
                    };
                    let mut execution = ToolExecution::success(json!({"error":message}));
                    execution.outcome = crate::session::events::ToolOutcome::Failed;
                    execution
                }
            }
        })
    }
}

fn required<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    args[key]
        .as_str()
        .filter(|s| !s.is_empty())
        .with_context(|| format!("{key} is required"))
}

fn mutate(sources: &[PathBuf], kind: Kind, args: &Value) -> Result<Value> {
    let source = PathBuf::from(required(args, "source")?);
    ensure!(
        sources.contains(&source),
        "source is not configured for this session; inspect skill.sources"
    );
    let directory = PathBuf::from(required(args, "directory")?);
    ensure!(!directory.as_os_str().is_empty() && (directory == Path::new(".") || directory.components().all(|part| {
        matches!(part, std::path::Component::Normal(name) if !name.to_string_lossy().starts_with('.'))
    })), "directory must be a relative path without hidden components or traversal");
    ensure!(
        directory.components().count() <= MAX_DEPTH,
        "directory exceeds skill discovery depth"
    );
    let draft = if matches!(kind, Kind::Write) {
        Some(validate(required(args, "content")?)?)
    } else {
        None
    };
    // Do not create missing sources implicitly: an unavailable mount must not
    // silently become an unrelated local directory.
    let root = fs::canonicalize(&source)
        .context("source directory unavailable; create or mount it first")?;
    ensure!(root.is_dir(), "source must be a directory");
    ensure!(!super::bundled::is_managed(&root), "This is a release-managed skill. Copy its directory to a custom source before editing; use skill.bundle to disable it.");
    let mut target = root.clone();
    if directory == Path::new(".") {
        ensure!(
            root.join("SKILL.md").exists() || root.join(".skill-archive").exists(),
            "use a child directory when creating a skill in a source root"
        );
    }
    for component in directory
        .components()
        .filter(|part| *part != std::path::Component::CurDir)
    {
        ensure!(
            !target.join("SKILL.md").exists() && !target.join(".skill-archive").exists(),
            "directory is inside an existing or archived skill's resource subtree"
        );
        target.push(component);
        if draft.is_some() {
            match fs::create_dir(&target) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e.into()),
            }
        }
        ensure!(
            fs::symlink_metadata(&target)?.is_dir(),
            "skill directories cannot be symlinks"
        );
    }
    let lock_path = target.join(".skill-write.lock");
    if lock_path.exists() {
        ensure!(
            fs::symlink_metadata(&lock_path)?.is_file(),
            "invalid skill lock file"
        );
    }
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)?;
    fs2::FileExt::lock_exclusive(&lock)?;
    let manifest = target.join("SKILL.md");
    let expected = args["expected_hash"].as_str();
    let previous = match fs::symlink_metadata(&manifest) {
        Ok(_) => Some(read_document(&manifest)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.into()),
    };
    match &previous {
        Some(content) => ensure!(
            expected == Some(digest(content).as_str()),
            "skill changed or expected_hash is missing; read the current manifest before updating"
        ),
        None => ensure!(
            expected.is_none() && draft.is_some(),
            "skill no longer exists; inspect the current catalog"
        ),
    }
    if let Some(draft) = draft {
        let temporary = target.join(format!(".skill-{}.tmp", ulid::Ulid::new()));
        let result = (|| -> Result<()> {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            file.write_all(required(args, "content")?.as_bytes())?;
            if previous.is_some() {
                fs::set_permissions(&temporary, fs::metadata(&manifest)?.permissions())?;
            }
            file.sync_all()?;
            fs::rename(&temporary, &manifest)?;
            Ok(())
        })();
        if temporary.exists() {
            let _ = fs::remove_file(&temporary);
        }
        result?;
        Ok(
            json!({"path":manifest,"content_hash":draft["content_hash"],"catalog":discover(sources)}),
        )
    } else {
        let archive = target.join(".skill-archive");
        fs::create_dir_all(&archive)?;
        ensure!(
            fs::symlink_metadata(&archive)?.is_dir(),
            "archive cannot be a symlink"
        );
        let archived = archive.join(format!("{}.md", ulid::Ulid::new()));
        fs::rename(&manifest, &archived)?;
        Ok(json!({"archived_path":archived,"catalog":discover(sources)}))
    }
}
