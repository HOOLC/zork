//! Generic reads of Synch's published tree. There is no business-directory
//! registry: filesystem and API sources, from every live origin, use this path.
use super::*;
use std::collections::{BTreeMap, BTreeSet};
use synch_core::EntryKind;
use synch_store::VersionSet;
mod snapshot;
#[cfg(test)]
mod tests;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreeCatalog {
    pub spaces: Vec<TreeSpace>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreeSpace {
    pub id: String,
    pub origins: Vec<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreeVersion {
    pub root: String,
    pub size: u64,
    pub modified_ns: i64,
    pub origins: Vec<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreeEntry {
    pub path: String,
    pub kind: String,
    pub origins: Vec<String>,
    pub versions: Vec<TreeVersion>,
    pub target: Option<String>,
    pub executable: bool,
    pub selected: Option<ObjectRef>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TreeCursor {
    pub revision: String,
    pub path: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TreeQuery {
    pub space: String,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub origin: Option<String>,
    #[serde(default)]
    pub search: String,
    #[serde(default)]
    pub descending: bool,
    pub after: Option<TreeCursor>,
    #[serde(default = "page_size")]
    pub limit: usize,
}
fn page_size() -> usize {
    128
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreePage {
    pub revision: String,
    pub entries: Vec<TreeEntry>,
    pub next: Option<TreeCursor>,
}
pub struct TreeChunk {
    pub bytes: Vec<u8>,
    pub offset: u64,
    pub total_size: u64,
    pub next_offset: Option<u64>,
}

fn origins(node: &Node, selected: Option<&str>) -> Result<BTreeSet<String>> {
    let mut origins = node
        .store()
        .trusted_origins(synch_core::now_ns())?
        .into_iter()
        .map(|o| o.to_string())
        .collect::<BTreeSet<_>>();
    origins.insert(node.origin().to_string());
    if let Some(selected) = selected {
        ensure!(origins.contains(selected), "tree origin is not trusted");
        origins.retain(|origin| origin == selected);
    }
    Ok(origins)
}
fn database(node: &Node) -> Result<rusqlite::Connection> {
    let connection = rusqlite::Connection::open_with_flags(
        node.store().db_path(),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.busy_timeout(Duration::from_secs(5))?;
    Ok(connection)
}
fn checked(query: &TreeQuery) -> Result<()> {
    // Use Synch's own namespace/path parser, including for spaces unknown to Zork.
    synch_core::dir_prefix(&query.space, &query.path)?;
    ensure!(
        query.path.len() <= 4096 && query.search.len() <= 255 && (1..=128).contains(&query.limit),
        "invalid tree query"
    );
    ensure!(
        query.path.is_empty()
            || query
                .path
                .split('/')
                .all(|p| !p.is_empty() && !matches!(p, "." | "..")),
        "invalid tree path"
    );
    Ok(())
}
fn scope(query: &TreeQuery) -> (String, Option<String>) {
    if query.path.is_empty() {
        (String::new(), None)
    } else {
        (format!("{}/", query.path), Some(format!("{}0", query.path)))
    }
}
fn revision(
    connection: &rusqlite::Connection,
    query: &TreeQuery,
    allowed: &BTreeSet<String>,
) -> Result<String> {
    let (prefix, upper) = scope(query);
    let mut hash = blake3::Hasher::new();
    serde_json::to_writer(
        &mut hash,
        &(
            &query.space,
            &query.path,
            &query.origin,
            &query.search,
            query.descending,
            allowed,
        ),
    )?;
    // A max sequence/count pair misses a late metadata slice whose sequence is
    // below a previously received sibling. Hash the scoped records in order;
    // memory stays bounded even for a large directory.
    let mut statement=connection.prepare("SELECT json_array(origin_id,path,kind,size,mtime_ns,unix_mode,hex(content),seq,symlink_target) FROM entries WHERE space=?1 AND path>=?2 AND (?3 IS NULL OR path<?3) AND origin_id IN (SELECT value FROM json_each(?4)) ORDER BY origin_id,path")?;
    let mut rows = statement.query(rusqlite::params![
        query.space,
        prefix,
        upper,
        serde_json::to_string(allowed)?
    ])?;
    while let Some(row) = rows.next()? {
        let bytes = row.get_ref(0)?.as_bytes()?;
        hash.update(&(bytes.len() as u64).to_le_bytes());
        hash.update(bytes);
    }
    Ok(hash.finalize().to_hex().to_string())
}

impl MeshNode {
    async fn tree_catch_up(&self, origin: Option<&str>) -> Result<bool> {
        let Some(origin) = origin else {
            return Ok(false);
        };
        let node = self.engine()?;
        if node.origin().to_string() == origin {
            return Ok(false);
        }
        let Some(key) = origin.strip_prefix("key:") else {
            return Ok(false);
        };
        let peer = NodeId::from_z32(key)?;
        tokio::time::timeout(CALL_TIMEOUT, async {
            self.refresh_peer_route(origin).await?;
            node.sync_with_peer(&peer).await?;
            Ok::<_, anyhow::Error>(())
        })
        .await??;
        Ok(true)
    }
    async fn tree_range(
        &self,
        root: &synch_core::Hash,
        origin: Option<&str>,
        offset: u64,
        limit: u64,
    ) -> Result<synch_engine::PreparedRange> {
        let node = self.engine()?;
        tokio::time::timeout(artifact_timeout(limit), async {
            match node.prepare_root_range(root, offset, Some(limit)).await {
                Err(EngineError::NotFound(_)) if self.tree_catch_up(origin).await? => {
                    Ok(node.prepare_root_range(root, offset, Some(limit)).await?)
                }
                result => Ok::<_, anyhow::Error>(result?),
            }
        })
        .await?
    }
    /// Flat metadata for explicit subtree consumers such as Skill discovery
    /// and materialization. A bound is mandatory; no contents are fetched here.
    pub async fn tree_walk(
        &self,
        reference: zork_config::tree::Reference,
        limit: usize,
    ) -> Result<Vec<TreeEntry>> {
        ensure!((1..=4096).contains(&limit), "invalid tree walk limit");
        if reference.snapshot.is_some() {
            return self.snapshot_walk(reference, limit).await;
        }
        self.tree_walk_excluding(reference, limit, None).await
    }
    /// Exclude a relative directory component before applying the metadata
    /// budget. Explicit roots inside that directory remain addressable.
    pub async fn tree_walk_excluding(
        &self,
        reference: zork_config::tree::Reference,
        limit: usize,
        excluded_directory: Option<String>,
    ) -> Result<Vec<TreeEntry>> {
        ensure!((1..=4096).contains(&limit), "invalid tree walk limit");
        ensure!(
            excluded_directory
                .as_ref()
                .is_none_or(|name| !name.is_empty()
                    && name != "."
                    && name != ".."
                    && !name.contains(['/', '\\'])
                    && !name.chars().any(char::is_control)),
            "invalid excluded directory"
        );
        let query = TreeQuery {
            space: reference.space,
            path: reference.path,
            origin: reference.origin,
            search: String::new(),
            descending: false,
            after: None,
            limit: 128,
        };
        checked(&query)?;
        self.blocking(move |node| {
            let allowed = origins(&node, query.origin.as_deref())?;
            let connection = database(&node)?;
            let before = revision(&connection, &query, &allowed)?;
            let (prefix, upper) = scope(&query);
            // Apply the origin selection before the bound; other origins may
            // publish a much larger subtree at this same path.
            let paths = connection.prepare("SELECT DISTINCT path FROM entries WHERE space=?1 AND path>=?2 AND (?3 IS NULL OR path<?3) AND kind<>3 AND origin_id IN (SELECT value FROM json_each(?4)) AND (?6 IS NULL OR instr('/'||substr(path,length(?2)+1)||'/', '/'||?6||'/')=0) ORDER BY path LIMIT ?5")?
                .query_map(rusqlite::params![query.space,prefix,upper,serde_json::to_string(&allowed)?,(limit+1) as i64,excluded_directory],|row|row.get::<_,String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            ensure!(
                paths.len() <= limit,
                "tree subtree exceeds metadata bound; choose a smaller directory"
            );
            let now = node.store().read_instant()?;
            let mut result = Vec::new();
            for path in paths {
                let mut set = node.versions(&query.space, &path)?;
                set.entries
                    .retain(|entry| allowed.contains(&entry.origin.to_string()));
                let set = VersionSet::from_entries(&set.space, &set.path, set.entries, now);
                if !set.exists() {
                    continue;
                }
                let selected = node.resolve_set(&set, &VersionPolicy::Newest, now)?;
                let kind = match selected.kind {
                    EntryKind::File => "file",
                    EntryKind::Dir => "directory",
                    EntryKind::Socket => "socket",
                    EntryKind::Symlink => "symlink",
                    EntryKind::Tombstone => unreachable!(),
                };
                result.push(TreeEntry {
                    selected: selected.content.map(|root| ObjectRef {
                        origin: selected.origin.to_string(),
                        space: query.space.clone(),
                        path: set.path.clone(),
                        root: root.to_string(),
                        size: selected.size,
                    }),
                    path: set.path,
                    kind: kind.into(),
                    target: selected.symlink_target,
                    executable: selected.unix_mode.is_some_and(|mode| mode & 0o111 != 0),
                    origins: set
                        .entries
                        .iter()
                        .filter(|e| e.kind != EntryKind::Tombstone)
                        .map(|e| e.origin.to_string())
                        .collect(),
                    versions: set
                        .versions
                        .into_iter()
                        .filter_map(|v| {
                            v.content.map(|root| TreeVersion {
                                root: root.to_string(),
                                size: v.size,
                                modified_ns: v.mtime_ns,
                                origins: v.attestors.into_iter().map(|o| o.to_string()).collect(),
                            })
                        })
                        .collect(),
                });
            }
            ensure!(
                origins(&node, query.origin.as_deref())? == allowed
                    && revision(&connection, &query, &allowed)? == before,
                "shared_directory_changed"
            );
            Ok(result)
        })
        .await
    }
    pub async fn tree_object(&self, reference: &zork_config::tree::Reference) -> Result<ObjectRef> {
        self.tree_object_with_mode(reference)
            .await
            .map(|(object, _)| object)
    }
    /// Select the content and its published permissions in the same read.
    pub async fn tree_object_with_mode(
        &self,
        reference: &zork_config::tree::Reference,
    ) -> Result<(ObjectRef, Option<u32>)> {
        if reference.snapshot.is_some() {
            return self.snapshot_object(reference.clone()).await;
        }
        match self.resolve_tree_object(reference).await {
            Err(error)
                if error
                    .downcast_ref::<EngineError>()
                    .is_some_and(|e| matches!(e, EngineError::NotFound(_)))
                    && self.tree_catch_up(reference.origin.as_deref()).await? =>
            {
                self.resolve_tree_object(reference).await
            }
            result => result,
        }
    }
    async fn resolve_tree_object(
        &self,
        reference: &zork_config::tree::Reference,
    ) -> Result<(ObjectRef, Option<u32>)> {
        let reference = reference.clone();
        self.blocking(move |node| {
            let allowed = origins(&node, reference.origin.as_deref())?;
            let mut set = node.versions(&reference.space, &reference.path)?;
            set.entries
                .retain(|entry| allowed.contains(&entry.origin.to_string()));
            let now = node.store().read_instant()?;
            let set = VersionSet::from_entries(&reference.space, &reference.path, set.entries, now);
            let selected = if let Some(root) = &reference.root {
                let root: synch_core::Hash = root.parse()?;
                set.entries
                    .iter()
                    .find(|entry| entry.content == Some(root))
                    .cloned()
                    .context("selected file version is no longer published")?
            } else {
                node.resolve_set(&set, &VersionPolicy::Newest, now)?
            };
            ensure!(selected.kind.has_content(), "tree reference is not a file");
            Ok((
                ObjectRef {
                    origin: selected.origin.to_string(),
                    space: reference.space,
                    path: reference.path,
                    root: selected
                        .content
                        .context("file content missing")?
                        .to_string(),
                    size: selected.size,
                },
                selected.unix_mode,
            ))
        })
        .await
    }
    /// Bounded, verified random reads for Agent file tools. References with a
    /// content root remain usable after the path advances to a new version.
    pub async fn tree_file(
        &self,
        reference: &zork_config::tree::Reference,
        offset: u64,
        limit: usize,
    ) -> Result<TreeChunk> {
        ensure!(limit <= MAX_ARTIFACT, "file read exceeds 300 MiB");
        let origin = reference.origin.clone();
        self.blocking(move |node| {
            origins(&node, origin.as_deref())?;
            Ok(())
        })
        .await?;
        let node = self.engine()?;
        let root = if reference.snapshot.is_some() {
            self.snapshot_object(reference.clone())
                .await?
                .0
                .root
                .parse()?
        } else {
            match &reference.root {
                Some(root) => root.parse()?,
                None => self.tree_object(reference).await?.root.parse()?,
            }
        };
        let head = self
            .tree_range(&root, reference.origin.as_deref(), 0, 0)
            .await?;
        let start = offset.min(head.size);
        let range = self
            .tree_range(&root, reference.origin.as_deref(), start, limit as u64)
            .await?;
        let bytes = node
            .cas_backend()
            .read_range(root, range.start, range.len())
            .await?;
        let origin = reference.origin.clone();
        self.blocking(move |node| {
            origins(&node, origin.as_deref())?;
            Ok(())
        })
        .await?;
        Ok(TreeChunk {
            bytes,
            offset,
            total_size: range.size,
            next_offset: (range.end < range.size).then_some(range.end),
        })
    }
    pub async fn tree_catalog(&self) -> Result<TreeCatalog> {
        self.blocking(|node| {
            let allowed=origins(&node,None)?;
            let database=database(&node)?;
            let mut statement=database.prepare("SELECT DISTINCT space,origin_id FROM entries WHERE kind<>3 AND origin_id IN (SELECT value FROM json_each(?1)) ORDER BY space,origin_id")?;
            let rows=statement.query_map([serde_json::to_string(&allowed)?], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?;
            let mut spaces=BTreeMap::<String,BTreeSet<String>>::new();
            for row in rows { let (space,origin)=row?; spaces.entry(space).or_default().insert(origin); }
            for source in node.store().sources()? {
                spaces.entry(source.space).or_default().insert(node.origin().to_string());
            }
            Ok(TreeCatalog { spaces:spaces.into_iter().map(|(id,origins)|TreeSpace{id,origins:origins.into_iter().collect()}).collect() })
        }).await
    }
    /// Query one business namespace directly; never enumerate unrelated spaces.
    pub async fn tree_space(&self, space: &str) -> Result<TreeCatalog> {
        let space = space.to_owned();
        self.blocking(move |node| {
            let allowed = origins(&node, None)?;
            let connection = database(&node)?;
            let mut statement = connection.prepare("SELECT DISTINCT origin_id FROM entries WHERE space=?1 AND kind<>3 AND origin_id IN (SELECT value FROM json_each(?2)) ORDER BY origin_id")?;
            let mut peers = statement.query_map(rusqlite::params![space,serde_json::to_string(&allowed)?], |r| r.get::<_,String>(0))?.collect::<rusqlite::Result<BTreeSet<_>>>()?;
            if node.store().source(&space)?.is_some() { peers.insert(node.origin().to_string()); }
            Ok(TreeCatalog { spaces: if peers.is_empty() { vec![] } else { vec![TreeSpace { id: space, origins: peers.into_iter().collect() }] } })
        }).await
    }

    pub async fn tree_directory(&self, query: TreeQuery) -> Result<TreePage> {
        checked(&query)?;
        let limit = query.limit;
        self.directory_page(query, limit, true).await
    }

    /// Refresh a client's already-loaded directory range in one metadata pass.
    /// This local API is not a network frame; framed callers still use the
    /// 128-entry, byte-bounded tree_directory endpoint.
    pub async fn tree_directory_window(&self, query: TreeQuery, limit: usize) -> Result<TreePage> {
        checked(&query)?;
        ensure!(
            (1..=8192).contains(&limit) && query.after.is_none(),
            "invalid tree window"
        );
        self.directory_page(query, limit, false).await
    }

    async fn directory_page(
        &self,
        query: TreeQuery,
        limit: usize,
        framed: bool,
    ) -> Result<TreePage> {
        self.blocking(move |node| {
            let allowed=origins(&node,query.origin.as_deref())?;
            let mut connection=database(&node)?;
            let tx=connection.transaction()?;
            let current=revision(&tx,&query,&allowed)?;
            ensure!(query.after.as_ref().is_none_or(|c|c.revision==current),"shared_directory_changed");
            let (prefix,upper)=scope(&query);
            // Parent directories of API-written paths are implicit. Grouping
            // the first component also orders `a/child` before sibling `a.txt`.
            // Only metadata is scanned, and only one bounded page is allocated.
            let order=if query.descending {"DESC"} else {"ASC"};
            let compare=if query.descending {"<"} else {">"};
            let sql=format!("WITH children AS (SELECT DISTINCT CASE WHEN instr(substr(path,length(?2)+1),'/')=0 THEN path ELSE substr(path,1,length(?2)+instr(substr(path,length(?2)+1),'/')-1) END AS child FROM entries WHERE space=?1 AND path>=?2 AND path<>?2 AND (?3 IS NULL OR path<?3) AND kind<>3 AND origin_id IN (SELECT value FROM json_each(?4))) SELECT child FROM children WHERE instr(lower(substr(child,length(?2)+1)),lower(?5))>0 AND (?6 IS NULL OR child {compare} ?6) ORDER BY child {order} LIMIT ?7");
            let paths={
                let mut statement=tx.prepare(&sql)?;
                let rows=statement.query_map(rusqlite::params![query.space,prefix,upper,serde_json::to_string(&allowed)?,query.search,query.after.as_ref().map(|c|&c.path),(limit+1) as i64],|r|r.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
                rows
            };
            let mut entries=Vec::new();
            let mut encoded=0;
            for path in paths.iter().take(limit) {
                let mut set=node.versions(&query.space,path)?;
                set.entries.retain(|e|allowed.contains(&e.origin.to_string()));
                set=VersionSet::from_entries(&query.space,path,set.entries,node.store().read_instant()?);
                let entry=if set.exists() {
                    let selected=node.resolve_set(&set,&VersionPolicy::Newest,node.store().read_instant()?)?;
                    let kind=match selected.kind { EntryKind::Dir=>"directory",EntryKind::File=>"file",EntryKind::Symlink=>"symlink",EntryKind::Socket=>"socket",EntryKind::Tombstone=>unreachable!() };
                    TreeEntry {
                        selected:selected.content.map(|root|ObjectRef{origin:selected.origin.to_string(),space:query.space.clone(),path:path.clone(),root:root.to_string(),size:selected.size}),
                        path:path.clone(),kind:kind.into(),executable:selected.unix_mode.is_some_and(|mode|mode&0o111!=0),target:selected.symlink_target,
                        origins:set.entries.iter().filter(|e|e.kind!=EntryKind::Tombstone).map(|e|e.origin.to_string()).collect(),
                        versions:set.versions.into_iter().filter_map(|v|v.content.map(|root|TreeVersion{root:root.to_string(),size:v.size,modified_ns:v.mtime_ns,origins:v.attestors.into_iter().map(|o|o.to_string()).collect()})).collect(),
                    }
                } else {
                    let mut statement=tx.prepare("SELECT DISTINCT origin_id FROM entries WHERE space=?1 AND path>=?2 AND path<?3 AND kind<>3 AND origin_id IN (SELECT value FROM json_each(?4)) ORDER BY origin_id")?;
                    let children=statement.query_map(rusqlite::params![query.space,format!("{path}/"),format!("{path}0"),serde_json::to_string(&allowed)?],|r|r.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
                    TreeEntry {path:path.clone(),kind:"directory".into(),origins:children,versions:vec![],target:None,executable:false,selected:None}
                };
                if framed {
                    encoded+=serde_json::to_vec(&entry)?.len();
                    if encoded>112*1024 && !entries.is_empty() {break;}
                    ensure!(encoded<120*1024,"tree entry exceeds frame bound");
                }
                entries.push(entry);
            }
            tx.commit()?;
            ensure!(origins(&node,query.origin.as_deref())?==allowed && revision(&connection,&query,&allowed)?==current,"shared_directory_changed");
            let next=if paths.len()>entries.len() { entries.last().map(|e|TreeCursor{revision:current.clone(),path:e.path.clone()}) } else {None};
            Ok(TreePage{revision:current,entries,next})
        }).await
    }

    /// Read the immutable content selected from a published path. Content may
    /// come from any holder; the requested root never follows a newer path.
    pub async fn tree_read(&self, object: &ObjectRef) -> Result<Vec<u8>> {
        ensure!(object.size <= MAX_ARTIFACT as u64, "file exceeds 300 MiB");
        let origin = object.origin.clone();
        self.blocking(move |node| {
            origins(&node, Some(&origin))?;
            Ok(())
        })
        .await?;
        let node = self.engine()?;
        let root = object.root.parse()?;
        let range = self
            .tree_range(&root, Some(&object.origin), 0, object.size)
            .await?;
        ensure!(range.size == object.size, "object size mismatch");
        let bytes = node.cas_backend().read_range(root, 0, object.size).await?;
        object.verify(&bytes)?;
        let origin = object.origin.clone();
        self.blocking(move |node| {
            origins(&node, Some(&origin))?;
            Ok(())
        })
        .await?;
        Ok(bytes)
    }
}
