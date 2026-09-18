//! Immutable local file submissions backed by the Station file tree.
use super::*;
use super::snapshots::Snapshot;
use serde::Serialize;
use std::io::Read;

pub const MAX_ARTIFACT_BYTES: u64 = zork_client_types::files::MAX_FILE_BYTES as u64;

#[derive(Clone, Debug, Serialize)]
pub struct Artifact {
    pub artifact_id: String,
    pub task_id: Option<String>,
    pub session_id: Option<String>,
    pub task_title: String,
    pub workspace: String,
    pub name: String,
    pub source_path: String,
    pub media_type: String,
    pub caption: Option<String>,
    pub byte_len: i64,
    pub version: i64,
    pub created_at: String,
}
const SELECT: &str = "SELECT a.artifact_id, a.task_id, a.session_id, a.title, a.workspace, a.name, a.source_path, a.media_type, a.byte_len, a.version, a.created_at, a.caption FROM artifact_file_catalog a";
fn map_artifact(row: &rusqlite::Row<'_>) -> rusqlite::Result<Artifact> {
    Ok(Artifact {
        artifact_id: row.get(0)?,
        task_id: row.get(1)?,
        session_id: row.get(2)?,
        task_title: row.get(3)?,
        workspace: row.get(4)?,
        name: row.get(5)?,
        source_path: row.get(6)?,
        media_type: row.get(7)?,
        byte_len: row.get(8)?,
        version: row.get(9)?,
        created_at: row.get(10)?,
        caption: row.get(11)?,
    })
}

pub(super) fn initialize(conn: &Connection) -> Result<()> {
    super::conversation_files::initialize(conn)?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS task_file_snapshots (
        artifact_id TEXT PRIMARY KEY,
        task_id TEXT NOT NULL REFERENCES product_tasks(task_id) ON DELETE CASCADE,
        name TEXT NOT NULL, source_path TEXT NOT NULL, media_type TEXT NOT NULL, workspace TEXT NOT NULL,
        snapshot TEXT NOT NULL, version INTEGER NOT NULL, created_at TEXT NOT NULL, caption TEXT,
        UNIQUE(task_id, source_path, version)
    );
    CREATE TABLE IF NOT EXISTS conversation_file_snapshots (
        artifact_id TEXT PRIMARY KEY, session_key TEXT NOT NULL REFERENCES sessions(key) ON DELETE CASCADE,
        name TEXT NOT NULL, source_path TEXT NOT NULL, media_type TEXT NOT NULL, workspace TEXT NOT NULL,
        snapshot TEXT NOT NULL, version INTEGER NOT NULL, created_at TEXT NOT NULL, caption TEXT,
        UNIQUE(session_key, source_path, version)
    );
    CREATE VIEW IF NOT EXISTS artifact_file_catalog AS
        SELECT a.artifact_id, a.task_id, s.id AS session_id, t.title, a.workspace, a.name, a.source_path, a.media_type, json_extract(a.snapshot,'$.byte_len') AS byte_len, a.version, a.created_at, a.caption
        FROM task_file_snapshots a JOIN product_tasks t ON t.task_id=a.task_id JOIN sessions s ON s.key=t.session_key
        UNION ALL
        SELECT a.artifact_id, NULL, s.id, COALESCE(json_extract(n.value,'$.name'),'Conversation'), a.workspace, a.name, a.source_path, a.media_type, json_extract(a.snapshot,'$.byte_len'), a.version, a.created_at, a.caption
        FROM conversation_file_snapshots a JOIN sessions s ON s.key=a.session_key LEFT JOIN node_agents n ON n.session_key=s.key;",
    )?;
    Ok(())
}

impl StationDb {
    pub fn list_artifacts(&self, task_id: Option<&str>) -> Result<Vec<Artifact>> {
        let conn = self.conn.lock().expect("db mutex");
        let mut query = conn.prepare(&format!("{SELECT} WHERE (?1 IS NULL OR a.task_id = ?1) ORDER BY a.created_at DESC, a.artifact_id DESC"))?;
        let rows = query
            .query_map([task_id], map_artifact)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn artifact_content(&self, artifact_id: &str) -> Result<Option<Vec<u8>>> {
        let conn = self.conn.lock().expect("db mutex");
        let snapshot: Option<Snapshot> = conn
            .query_row(
                "SELECT snapshot FROM task_file_snapshots WHERE artifact_id = ?1 UNION ALL SELECT snapshot FROM conversation_file_snapshots WHERE artifact_id = ?1",
                [artifact_id],
                |r| r.get(0),
            )
            .optional()?;
        drop(conn);
        snapshot.map(|snapshot| self.read_snapshot(&snapshot)).transpose()
    }

    pub fn register_artifact(
        &self,
        task_id: &str,
        file_path: &Path,
        caption: Option<&str>,
    ) -> Result<Artifact> {
        let task = self.product_task(task_id)?.context("task_not_found")?;
        if task.mesh.as_ref().is_some_and(|m| m["role"] == "owner") {
            anyhow::bail!("mesh_artifact_executor_required");
        }
        if task.mesh.is_some() && caption.is_some_and(|caption| caption.len() > 16 * 1024) {
            anyhow::bail!("mesh_artifact_caption_too_large");
        }
        let (source_path, name, media_type, content) =
            prepare_file(Path::new(&task.workspace), file_path)?;
        let snapshot = self.freeze_file(&name, &content)?;
        let mut conn = self.conn.lock().expect("db mutex");
        let tx = conn.transaction()?;
        let latest: Option<(String, i64, bool)> = tx.query_row(
            "SELECT artifact_id, version, snapshot = ?3 AND caption IS ?4 AND workspace = ?5 FROM task_file_snapshots WHERE task_id = ?1 AND source_path = ?2 ORDER BY version DESC LIMIT 1",
            params![task_id, source_path, snapshot, caption, task.workspace], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).optional()?;
        if let Some((id, _, true)) = latest.as_ref() {
            return Ok(tx.query_row(
                &format!("{SELECT} WHERE a.artifact_id = ?1"),
                [id],
                map_artifact,
            )?);
        }
        if task.mesh.is_some() {
            let count: i64 = tx.query_row(
                "SELECT COUNT(*) FROM task_file_snapshots WHERE task_id=?1",
                [task_id],
                |r| r.get(0),
            )?;
            anyhow::ensure!(count < 128, "mesh_artifact_limit_reached");
        }
        let (state, current_workspace): (String, String) = tx.query_row(
            "SELECT t.state, s.workspace_path FROM product_tasks t JOIN sessions s ON s.key = t.session_key WHERE t.task_id = ?1",
            [task_id], |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        if current_workspace != task.workspace {
            anyhow::bail!("artifact_workspace_changed");
        }
        if matches!(state.as_str(), "completed" | "cancelled") {
            anyhow::bail!("task_closed_reopen_required");
        }
        let id = format!("artifact-{}", ulid::Ulid::new());
        let version = latest.map_or(1, |(_, version, _)| version + 1);
        let now = now_rfc3339();
        tx.execute("INSERT INTO task_file_snapshots(artifact_id, task_id, name, source_path, media_type, snapshot, version, created_at, caption, workspace) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)", params![id, task_id, name, source_path, media_type, snapshot, version, now, caption, task.workspace])?;
        // New evidence invalidates an older review page; byte-identical retries above do not.
        tx.execute(
            "UPDATE product_tasks SET revision = revision + 1, updated_at = ?2 WHERE task_id = ?1",
            params![task_id, now],
        )?;
        let artifact = tx.query_row(
            &format!("{SELECT} WHERE a.artifact_id = ?1"),
            [&id],
            map_artifact,
        )?;
        tx.commit()?;
        Ok(artifact)
    }
}

pub(super) fn prepare_file(
    workspace: &Path,
    file_path: &Path,
) -> Result<(String, String, &'static str, Vec<u8>)> {
    let relative = if file_path.is_absolute() {
        match file_path.strip_prefix(workspace) {
            Ok(relative) => relative.to_path_buf(),
            Err(_) => {
                // macOS callers can use /var while the runtime records /private/var.
                // Resolve the parent alias, keep the leaf for the NOFOLLOW open below.
                let root = fs::canonicalize(workspace)?;
                let parent =
                    fs::canonicalize(file_path.parent().context("artifact_invalid_path")?)?;
                parent
                    .strip_prefix(root)
                    .map_err(|_| anyhow::anyhow!("artifact_outside_workspace"))?
                    .join(file_path.file_name().context("artifact_invalid_path")?)
            }
        }
    } else {
        file_path.to_path_buf()
    };
    let components = relative
        .components()
        .map(|c| match c {
            std::path::Component::Normal(name) => Ok(name.to_owned()),
            _ => anyhow::bail!("artifact_invalid_path"),
        })
        .collect::<Result<Vec<_>>>()?;
    if components.is_empty() {
        anyhow::bail!("artifact_invalid_path");
    }
    let relative = components.iter().collect::<PathBuf>();
    let source_path = relative
        .to_str()
        .context("artifact_invalid_path")?
        .to_owned();
    let name = relative
        .file_name()
        .and_then(|n| n.to_str())
        .context("artifact_invalid_path")?
        .to_owned();
    let content = read_workspace_file(workspace, &components)?;
    let media_type = match relative
        .extension()
        .and_then(|s| s.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("md" | "markdown") => "text/markdown",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("svg") => "image/svg+xml",
        Some("pdf") => "application/pdf",
        _ if std::str::from_utf8(&content).is_ok() && !content.contains(&0) => "text/plain",
        _ => "application/octet-stream",
    };
    Ok((source_path, name, media_type, content))
}
impl StationDb {
    pub fn register_conversation_artifact(
        &self,
        session_key: &str,
        file_path: &Path,
        caption: Option<&str>,
    ) -> Result<Artifact> {
        let session = self
            .get_binding(session_key)?
            .context("session_not_found")?;
        let SessionBindingRow::Normal(session) = session else {
            anyhow::bail!("conversation_required")
        };
        anyhow::ensure!(
            session.channel_type.as_deref() == Some("leader_chat"),
            "leader_conversation_required"
        );
        anyhow::ensure!(
            caption.is_none_or(|c| c.len() <= 16 * 1024),
            "artifact_caption_too_large"
        );
        let (source_path, name, media_type, content) =
            prepare_file(Path::new(&session.workspace_path), file_path)?;
        let snapshot = self.freeze_file(&name, &content)?;
        let mut conn = self.conn.lock().expect("db mutex");
        let tx = conn.transaction()?;
        let latest:Option<(String,i64,bool)>=tx.query_row("SELECT artifact_id,version,snapshot=?3 AND caption IS ?4 FROM conversation_file_snapshots WHERE session_key=?1 AND source_path=?2 ORDER BY version DESC LIMIT 1",params![session_key,source_path,snapshot,caption],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
        if let Some((id, _, true)) = &latest {
            return Ok(tx.query_row(
                &format!("{SELECT} WHERE a.artifact_id=?1"),
                [id],
                map_artifact,
            )?);
        }
        let id = format!("artifact-{}", ulid::Ulid::new());
        let version = latest.map_or(1, |(_, v, _)| v + 1);
        let now = now_rfc3339();
        tx.execute("INSERT INTO conversation_file_snapshots(artifact_id,session_key,name,source_path,media_type,workspace,snapshot,version,created_at,caption) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",params![id,session_key,name,source_path,media_type,session.workspace_path,snapshot,version,now,caption])?;
        let artifact = tx.query_row(
            &format!("{SELECT} WHERE a.artifact_id=?1"),
            [&id],
            map_artifact,
        )?;
        tx.commit()?;
        Ok(artifact)
    }
}

#[cfg(unix)]
fn read_workspace_file(workspace: &Path, components: &[std::ffi::OsString]) -> Result<Vec<u8>> {
    use std::os::{
        fd::{AsRawFd, FromRawFd},
        unix::ffi::OsStrExt,
    };
    // Resolve the workspace once, then walk descriptors with NOFOLLOW. A changed
    // symlink cannot turn a validated relative path into an unrelated host file.
    let mut directory =
        fs::File::open(fs::canonicalize(workspace).context("artifact_invalid_workspace")?)?;
    for (index, name) in components.iter().enumerate() {
        let name = std::ffi::CString::new(name.as_bytes()).context("artifact_invalid_path")?;
        let last = index + 1 == components.len();
        let flags = libc::O_RDONLY
            | libc::O_CLOEXEC
            | libc::O_NOFOLLOW
            | if last {
                libc::O_NONBLOCK
            } else {
                libc::O_DIRECTORY
            };
        let fd = unsafe { libc::openat(directory.as_raw_fd(), name.as_ptr(), flags) };
        if fd < 0 {
            anyhow::bail!("artifact_file_unavailable");
        }
        let mut file = unsafe { fs::File::from_raw_fd(fd) };
        if last {
            let metadata = file.metadata()?;
            if !metadata.is_file() {
                anyhow::bail!("artifact_requires_file");
            }
            if metadata.len() > MAX_ARTIFACT_BYTES {
                anyhow::bail!("artifact_too_large");
            }
            let mut bytes = Vec::new();
            (&mut file)
                .take(MAX_ARTIFACT_BYTES + 1)
                .read_to_end(&mut bytes)?;
            if bytes.len() as u64 > MAX_ARTIFACT_BYTES {
                anyhow::bail!("artifact_too_large");
            }
            let after = file.metadata()?;
            if after.len() != metadata.len() || after.modified()? != metadata.modified()? {
                anyhow::bail!("artifact_file_changed");
            }
            return Ok(bytes);
        }
        directory = file;
    }
    anyhow::bail!("artifact_invalid_path")
}
#[cfg(not(unix))]
fn read_workspace_file(_: &Path, _: &[std::ffi::OsString]) -> Result<Vec<u8>> {
    anyhow::bail!("artifact_platform_unsupported")
}

#[cfg(test)]
mod tests {
    use super::*;
    fn setup() -> (tempfile::TempDir, StationDb, String, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("project");
        let db = StationDb::open(dir.path(), &dir.path().join("workspaces")).unwrap();
        let session = db
            .create_session_at_workspace(
                EnsureSession {
                    connection_id: "local_gui",
                    platform: "local_gui",
                    channel_id: "conversation",
                    root_thread_ts: "conversation",
                    channel_type: Some("desktop"),
                    initiator_user_id: None,
                    initiator_message_ts: None,
                },
                &workspace,
            )
            .unwrap();
        let task = db.product_task_for_session(&session.key).unwrap().unwrap();
        (dir, db, task.task_id, workspace)
    }
    #[test]
    fn svg_artifacts_preserve_image_media_type_and_bytes() {
        let (_dir, db, task, workspace) = setup();
        let path = workspace.join("drawing.SVG");
        let bytes = br#"<svg xmlns="http://www.w3.org/2000/svg" width="32" height="16"/>"#;
        fs::write(&path, bytes).unwrap();
        let artifact = db.register_artifact(&task, &path, None).unwrap();
        assert_eq!(artifact.media_type, "image/svg+xml");
        assert_eq!(
            db.artifact_content(&artifact.artifact_id).unwrap().unwrap(),
            bytes
        );
    }

    #[test]
    fn versions_preserve_bytes_after_source_changes_deletion_and_restart() {
        let (dir, db, task, workspace) = setup();
        let path = workspace.join("report.md");
        fs::write(&path, "# Version one\n").unwrap();
        let first = db
            .register_artifact(&task, &path, Some("First report"))
            .unwrap();
        assert_eq!(first.version, 1);
        let revision = db.product_task(&task).unwrap().unwrap().revision;
        let retry = db
            .register_artifact(&task, &path, Some("First report"))
            .unwrap();
        assert_eq!(first.artifact_id, retry.artifact_id);
        assert_eq!(db.product_task(&task).unwrap().unwrap().revision, revision);
        fs::write(&path, "# Version two\n").unwrap();
        let second = db.register_artifact(&task, &path, None).unwrap();
        assert_eq!(second.version, 2);
        assert_eq!(
            db.product_task(&task).unwrap().unwrap().revision,
            revision + 1
        );
        fs::remove_file(path).unwrap();
        db.conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE sessions SET workspace_path = '/moved-workspace'",
                [],
            )
            .unwrap();
        assert_eq!(
            db.list_artifacts(Some(&task)).unwrap()[0].workspace,
            first.workspace
        );
        drop(db);
        let db = StationDb::open(dir.path(), &dir.path().join("workspaces")).unwrap();
        assert_eq!(db.list_artifacts(Some(&task)).unwrap().len(), 2);
        assert_eq!(
            db.artifact_content(&first.artifact_id).unwrap().unwrap(),
            b"# Version one\n"
        );
        assert_eq!(
            db.artifact_content(&second.artifact_id).unwrap().unwrap(),
            b"# Version two\n"
        );
        assert!(db.artifact_content("missing").unwrap().is_none());
    }
    #[test]
    fn closed_tasks_cannot_change_reviewed_files_and_new_files_invalidate_old_revision() {
        let (_dir, db, task, workspace) = setup();
        let current = db.product_task(&task).unwrap().unwrap();
        let path = workspace.join("result.txt");
        fs::write(&path, "delivered").unwrap();
        let file = db.register_artifact(&task, &path, None).unwrap();
        assert!(db
            .transition_task(&task, current.revision, TaskAction::Cancel)
            .is_err());
        let current = db.product_task(&task).unwrap().unwrap();
        db.transition_task(&task, current.revision, TaskAction::Cancel)
            .unwrap();
        assert_eq!(
            db.register_artifact(&task, &path, None)
                .unwrap()
                .artifact_id,
            file.artifact_id
        );
        fs::write(&path, "late replacement").unwrap();
        assert_eq!(
            db.register_artifact(&task, &path, None)
                .unwrap_err()
                .to_string(),
            "task_closed_reopen_required"
        );
        assert_eq!(
            db.artifact_content(&file.artifact_id).unwrap().unwrap(),
            b"delivered"
        );
    }
    #[test]
    #[cfg(unix)]
    fn registration_rejects_escape_symlinks_directories_and_oversized_files() {
        use std::os::unix::fs::symlink;
        let (dir, db, task, workspace) = setup();
        let outside = dir.path().join("outside.txt");
        fs::write(&outside, "unrelated").unwrap();
        assert!(db.register_artifact(&task, &outside, None).is_err());
        assert!(db
            .register_artifact(&task, Path::new("../outside.txt"), None)
            .is_err());
        symlink(&outside, workspace.join("link.txt")).unwrap();
        assert!(db
            .register_artifact(&task, Path::new("link.txt"), None)
            .is_err());
        symlink(dir.path(), workspace.join("linked-directory")).unwrap();
        assert!(db
            .register_artifact(&task, Path::new("linked-directory/outside.txt"), None)
            .is_err());
        fs::create_dir(workspace.join("folder")).unwrap();
        assert!(db
            .register_artifact(&task, Path::new("folder"), None)
            .is_err());
        let large = workspace.join("large.bin");
        fs::File::create(&large)
            .unwrap()
            .set_len(MAX_ARTIFACT_BYTES + 1)
            .unwrap();
        assert_eq!(
            db.register_artifact(&task, &large, None)
                .unwrap_err()
                .to_string(),
            "artifact_too_large"
        );
        assert!(db.list_artifacts(None).unwrap().is_empty());
        fs::write(workspace.join("real.txt"), "inside workspace").unwrap();
        symlink(&workspace, dir.path().join("workspace-alias")).unwrap();
        let alias = dir.path().join("workspace-alias/real.txt");
        assert_eq!(
            db.register_artifact(&task, &alias, None)
                .unwrap()
                .source_path,
            "real.txt"
        );
    }
}
