//! Station file consumers read the same published tree as the file browser.
use crate::{db::GatewayDb, mesh::MeshService};
use anyhow::{Context, Result};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
};
use zork_agent::{
    session::ports::{FilePage, FileSystem, SystemFileSystem},
    skills::{Skill, SkillCatalog, ARCHIVE_DIRECTORY},
};
use zork_config::tree::Reference;
use zork_mesh::node::MeshNode;
mod materialize;

pub struct Files {
    root: PathBuf,
    db: Arc<GatewayDb>,
    mesh: Arc<OnceLock<Arc<MeshService>>>,
    materialized: tokio::sync::Mutex<materialize::Cache>,
    catalog_snapshots: std::sync::Mutex<std::collections::HashMap<String, Reference>>,
}
impl Files {
    pub fn new(root: PathBuf, db: Arc<GatewayDb>, mesh: Arc<OnceLock<Arc<MeshService>>>) -> Self {
        Self {
            root,
            db,
            mesh,
            materialized: Default::default(),
            catalog_snapshots: Default::default(),
        }
    }
    pub(crate) fn node(&self) -> Result<MeshNode> {
        Ok(self
            .mesh
            .get()
            .context("Station file network is starting")?
            .file_tree())
    }
    /// Discovery runs on the runner's blocking executor. Only manifests are
    /// fetched; scripts and other resources remain demand reads.
    pub fn catalog(&self, session: &str) -> Result<SkillCatalog> {
        self.catalog_sources(&self.db.skill_paths_for_session(session)?)
    }
    pub fn catalog_sources(&self, extra: &[PathBuf]) -> Result<SkillCatalog> {
        let settings = zork_config::load_config(&self.root)?.skills;
        let paths = settings.sources(&self.root, extra)?;
        let local = paths
            .iter()
            .filter(|p| !p.to_string_lossy().starts_with("synch://"))
            .cloned()
            .collect::<Vec<_>>();
        let mut catalog = zork_agent::skills::discover(&local);
        let mut references = paths
            .iter()
            .filter(|p| p.to_string_lossy().starts_with("synch://"))
            .map(|p| Reference::parse(&p.to_string_lossy()).map(|r| (r, false)))
            .collect::<Result<Vec<_>>>()?;
        references.push((
            Reference {
                space: zork_config::tree::SKILLS_SPACE.into(),
                path: String::new(),
                origin: None,
                root: None,
                snapshot: None,
            },
            true,
        ));
        let Ok(node) = self.node() else {
            return Ok(catalog);
        };
        let runtime = tokio::runtime::Handle::current();
        let own = runtime.block_on(node.identity())?;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(8);
        let mut remaining = 4096usize;
        let mut seen = std::collections::BTreeSet::new();
        for (reference, skip_own) in references {
            let result = runtime.block_on(async {
                tokio::time::timeout_at(
                    deadline,
                    self.discover_reference(
                        &node,
                        reference,
                        skip_own,
                        &own,
                        &mut remaining,
                        &mut seen,
                        &mut catalog,
                    ),
                )
                .await
                .context("Skill discovery timed out")?
            });
            if let Err(error) = result {
                diagnostic(&mut catalog, error.to_string());
            }
        }
        catalog
            .skills
            .sort_by(|a, b| a.name.cmp(&b.name).then(a.path.cmp(&b.path)));
        Ok(catalog)
    }
    async fn stable_skill_snapshot(
        &self,
        node: &MeshNode,
        current: Reference,
    ) -> Result<Reference> {
        let mut identity = current.clone();
        identity.snapshot = None;
        identity.root = None;
        let key = identity.uri();
        let previous = self.catalog_snapshots.lock().unwrap().get(&key).cloned();
        if let Some(previous) = previous {
            if node
                .tree_same_directory(previous.clone(), current.clone())
                .await
                .unwrap_or(false)
            {
                return Ok(previous);
            }
        }
        let mut snapshots = self.catalog_snapshots.lock().unwrap();
        if snapshots.len() >= 512 {
            snapshots.clear();
        }
        snapshots.insert(key, current.clone());
        Ok(current)
    }
    async fn discover_reference(
        &self,
        node: &MeshNode,
        reference: Reference,
        skip_own: bool,
        own: &str,
        remaining: &mut usize,
        seen: &mut std::collections::BTreeSet<PathBuf>,
        catalog: &mut SkillCatalog,
    ) -> Result<()> {
        let origins = match &reference.origin {
            Some(origin) => vec![origin.clone()],
            None => node
                .tree_space(&reference.space)
                .await?
                .spaces
                .into_iter()
                .flat_map(|space| space.origins)
                .collect(),
        };
        for origin in origins {
            if skip_own && origin == own {
                continue;
            }
            let source = node
                .tree_snapshot(Reference {
                    origin: Some(origin),
                    ..reference.clone()
                })
                .await?;
            let mut pending = std::collections::VecDeque::from([(source, 0usize, None)]);
            while let Some((directory, depth, mut after)) = pending.pop_front() {
                if *remaining == 0 || catalog.skills.len() >= 256 {
                    catalog
                        .continuations
                        .push(continuation(&directory, after.as_ref()));
                    catalog.continuations.extend(
                        pending
                            .into_iter()
                            .take(32usize.saturating_sub(catalog.continuations.len()))
                            .map(|(source, _, after)| continuation(&source, after.as_ref())),
                    );
                    diagnostic(catalog,"Skill discovery page limit reached; remaining directories are in continuations".into());
                    break;
                }
                *remaining -= 1;
                let result: Result<()> = async {
                    let mut selected = directory.clone();
                    if let Some(state) = Self::bundle_state(node, &directory).await? {
                        let mut replaced = false;
                        if let Some(successor) = &state.successor {
                            let parent = directory
                                .path
                                .rsplit_once('/')
                                .map_or("", |(parent, _)| parent);
                            let next = Reference {
                                path: if parent.is_empty() {
                                    successor.clone()
                                } else {
                                    format!("{parent}/{successor}")
                                },
                                ..directory.clone()
                            };
                            if let Ok(Some(next)) = Self::bundle_state(node, &next).await {
                                replaced = state.is_replaced_by(successor, &next);
                            }
                        }
                        if !state.published.is_empty() && !replaced {
                            let active = state
                                .active
                                .context("managed Skill has no active version")?;
                            anyhow::ensure!(
                                active.skills.len() == 1
                                    && zork_config::skill_bundles::valid_component(
                                        &active.directory
                                    ),
                                "invalid Skill selection"
                            );
                            let id = &active.skills[0];
                            anyhow::ensure!(
                                zork_config::skill_bundles::valid_component(id),
                                "invalid Skill ID"
                            );
                            if state.disabled.contains(id) {
                                return Ok(());
                            }
                            selected = directory
                                .child(&format!(".zork/.versions/{}/{id}", active.directory));
                        }
                    }
                    let document = selected.child("SKILL.md");
                    if let Some(entry) = node.tree_entry_at(document.clone()).await? {
                        anyhow::ensure!(entry.kind == "file", "SKILL.md must be a regular file");
                        let object = entry.selected.context("Skill content missing")?;
                        anyhow::ensure!(object.size <= 128 * 1024, "SKILL.md exceeds 128 KiB");
                        if reference.snapshot.is_none() {
                            selected = self.stable_skill_snapshot(node, selected).await?;
                        }
                        let document = Reference {
                            root: Some(object.root),
                            ..selected.child("SKILL.md")
                        };
                        let path = PathBuf::from(document.uri());
                        if seen.insert(path.clone()) {
                            let page = node.tree_file(&document, 0, 128 * 1024).await?;
                            catalog.skills.push(
                                Skill::from_document(
                                    path,
                                    selected.uri().into(),
                                    &String::from_utf8(page.bytes)?,
                                )
                                .with_context(|| document.uri())?,
                            );
                        }
                        return Ok(());
                    }
                    if node
                        .tree_entry_at(directory.child(ARCHIVE_DIRECTORY))
                        .await?
                        .is_some()
                        || depth >= 8
                    {
                        return Ok(());
                    }
                    let page = node
                        .tree_directory_at(directory.clone(), after.take(), 128)
                        .await?;
                    for entry in page.entries {
                        if entry.kind == "directory"
                            && !entry.path.rsplit('/').next().unwrap_or("").starts_with('.')
                        {
                            if pending.len() < *remaining {
                                pending.push_back((
                                    Reference {
                                        path: entry.path,
                                        ..directory.clone()
                                    },
                                    depth + 1,
                                    None,
                                ));
                            } else if catalog.continuations.len() < 32 {
                                catalog.continuations.push(continuation(
                                    &Reference {
                                        path: entry.path,
                                        ..directory.clone()
                                    },
                                    None,
                                ));
                            }
                        }
                    }
                    if let Some(after) = page.next {
                        pending.push_back((directory.clone(), depth, Some(after)));
                    }
                    Ok(())
                }
                .await;
                if let Err(error) = result {
                    diagnostic(catalog, format!("{}: {error:#}", directory.uri()));
                }
            }
        }
        Ok(())
    }
    async fn bundle_state(
        node: &MeshNode,
        directory: &Reference,
    ) -> Result<Option<zork_config::skill_bundles::BundleState>> {
        let reference = directory.child(".zork/.state.json");
        let Some(entry) = node.tree_entry_at(reference.clone()).await? else {
            return Ok(None);
        };
        anyhow::ensure!(
            entry.kind == "file",
            "Skill metadata must be a regular file"
        );
        let page = node.tree_file(&reference, 0, 128 * 1024).await?;
        anyhow::ensure!(
            page.next_offset.is_none(),
            "Skill metadata exceeds read bound"
        );
        let state: zork_config::skill_bundles::BundleState = serde_json::from_slice(&page.bytes)?;
        state.validate()?;
        Ok(Some(state))
    }
    fn local(path: &Path) -> std::io::Result<()> {
        if path.to_string_lossy().starts_with("synch://") {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "A shared file reference is read-only; edit its owning Station or make an explicit local copy",
            ));
        }
        Ok(())
    }
}

impl FileSystem for Files {
    fn list_page_async(
        self: Arc<Self>,
        path: PathBuf,
        cursor: Option<String>,
        limit: usize,
    ) -> zork_agent::session::ports::FileFuture<serde_json::Value> {
        Box::pin(async move {
            if !path.to_string_lossy().starts_with("synch://") {
                return Arc::new(SystemFileSystem)
                    .list_page_async(path, cursor, limit)
                    .await;
            }
            let result: Result<_> = async {
                let node=self.node()?;
                let mut reference=Reference::parse(&path.to_string_lossy())?;
                anyhow::ensure!(reference.root.is_none(),"directory listing needs a directory reference");
                if reference.origin.is_some() {reference=node.tree_snapshot(reference).await?;}
                let after=cursor.map(|cursor|serde_json::from_str(&cursor)).transpose()?;
                let page=if reference.snapshot.is_some() {node.tree_directory_at(reference.clone(),after,limit).await?} else {
                    node.tree_directory(zork_mesh::node::TreeQuery{space:reference.space.clone(),path:reference.path.clone(),origin:None,search:String::new(),descending:false,after,limit}).await?
                };
                let entries=page.entries.into_iter().map(|entry|{
                    let mut child=Reference{path:entry.path.clone(),root:None,..reference.clone()};
                    if let Some(object)=&entry.selected {child.origin=Some(object.origin.clone());child.root=Some(object.root.clone());}
                    let mut value=serde_json::to_value(&entry).expect("tree entry");value["path"]=child.uri().into();value
                }).collect::<Vec<_>>();
                Ok(serde_json::json!({"path":reference.uri(),"entries":entries,"next_cursor":page.next.map(|cursor|serde_json::to_string(&cursor)).transpose()?}))
            }.await;
            result.map_err(std::io::Error::other)
        })
    }
    fn materialize(
        self: Arc<Self>,
        path: PathBuf,
    ) -> zork_agent::session::ports::FileFuture<PathBuf> {
        Box::pin(async move {
            self.materialize_reference(&path)
                .await
                .map_err(std::io::Error::other)
        })
    }
    fn read_page_async(
        self: Arc<Self>,
        path: PathBuf,
        offset: u64,
        limit: usize,
    ) -> zork_agent::session::ports::FileFuture<FilePage> {
        Box::pin(async move {
            if !path.to_string_lossy().starts_with("synch://") {
                return tokio::task::spawn_blocking(move || {
                    SystemFileSystem.read_page(&path, offset, limit)
                })
                .await
                .map_err(std::io::Error::other)?;
            }
            let result = async {
                let reference = Reference::parse(&path.to_string_lossy())?;
                let page = self.node()?.tree_file(&reference, offset, limit).await?;
                Ok::<_, anyhow::Error>(FilePage {
                    bytes: page.bytes,
                    offset: page.offset,
                    total_size: page.total_size,
                    next_offset: page.next_offset,
                })
            }
            .await;
            result.map_err(std::io::Error::other)
        })
    }
    fn create_dir_all(&self, path: &Path) -> std::io::Result<()> {
        Self::local(path)?;
        SystemFileSystem.create_dir_all(path)
    }
    fn create(&self, path: &Path) -> std::io::Result<Box<dyn std::io::Write + Send>> {
        Self::local(path)?;
        SystemFileSystem.create(path)
    }
    fn read_page(&self, path: &Path, offset: u64, limit: usize) -> std::io::Result<FilePage> {
        if !path.to_string_lossy().starts_with("synch://") {
            return SystemFileSystem.read_page(path, offset, limit);
        }
        let result = (|| -> Result<FilePage> {
            let reference = Reference::parse(&path.to_string_lossy())?;
            let page = tokio::runtime::Handle::current()
                .block_on(self.node()?.tree_file(&reference, offset, limit))?;
            Ok(FilePage {
                bytes: page.bytes,
                offset: page.offset,
                total_size: page.total_size,
                next_offset: page.next_offset,
            })
        })();
        result.map_err(std::io::Error::other)
    }
    fn read_to_string(&self, path: &Path) -> std::io::Result<String> {
        if !path.to_string_lossy().starts_with("synch://") {
            return SystemFileSystem.read_to_string(path);
        }
        let page = self.read_page(path, 0, 128 * 1024)?;
        if page.next_offset.is_some() {
            return Err(std::io::Error::other("shared text exceeds 128 KiB"));
        }
        String::from_utf8(page.bytes).map_err(std::io::Error::other)
    }
    fn write(&self, path: &Path, contents: &[u8]) -> std::io::Result<()> {
        Self::local(path)?;
        SystemFileSystem.write(path, contents)
    }
    fn tail(&self, path: &Path, max_bytes: usize) -> std::io::Result<Vec<u8>> {
        if !path.to_string_lossy().starts_with("synch://") {
            return SystemFileSystem.tail(path, max_bytes);
        }
        let page = self.read_page(path, 0, 0)?;
        Ok(self
            .read_page(
                path,
                page.total_size.saturating_sub(max_bytes as u64),
                max_bytes,
            )?
            .bytes)
    }
}

fn continuation(
    reference: &Reference,
    cursor: Option<&zork_mesh::node::TreeCursor>,
) -> serde_json::Value {
    let mut value = serde_json::json!({"path":reference.uri()});
    if let Some(cursor) = cursor {
        value["cursor"] = serde_json::to_string(cursor)
            .expect("directory cursor")
            .into();
    }
    value
}

fn diagnostic(catalog: &mut SkillCatalog, text: String) {
    if catalog.diagnostics.len() < 32 {
        catalog.diagnostics.push(text);
    }
}
