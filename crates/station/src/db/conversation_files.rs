//! Bounded resumable ingress into the conversation owner's durable snapshot store.
use super::*;
use super::snapshots::Snapshot;
use anyhow::ensure;
use zork_client_types::files::{FileRef, CHUNK_BYTES};
#[cfg(test)]
mod large_file_tests;

pub(super) fn initialize(conn: &Connection) -> Result<()> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS conversation_uploads (
        id TEXT PRIMARY KEY, session_key TEXT NOT NULL REFERENCES sessions(key) ON DELETE CASCADE,
        metadata TEXT NOT NULL, received INTEGER NOT NULL, updated_at INTEGER NOT NULL);
        CREATE TABLE IF NOT EXISTS conversation_upload_chunks(id TEXT NOT NULL REFERENCES conversation_uploads(id) ON DELETE CASCADE, offset INTEGER NOT NULL, content BLOB NOT NULL, PRIMARY KEY(id,offset));
        CREATE TABLE IF NOT EXISTS conversation_file_origins(id TEXT PRIMARY KEY REFERENCES conversation_file_snapshots(artifact_id) ON DELETE CASCADE, source_json TEXT NOT NULL);")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn session(db: &StationDb, root: &Path, name: &str) -> SessionRow {
        db.create_session_at_workspace(
            EnsureSession {
                connection_id: "local_gui",
                platform: "local_gui",
                channel_id: name,
                root_thread_ts: name,
                channel_type: Some("leader_chat"),
                initiator_user_id: None,
                initiator_message_ts: None,
            },
            &root.join(name),
        )
        .unwrap()
    }
    fn reference(bytes: &[u8]) -> FileRef {
        FileRef {
            id: "file-test".into(),
            name: "方案.pdf".into(),
            byte_len: bytes.len(),
            content_root: zork_mesh::content_root(bytes),
        }
    }
    #[test]
    fn svg_uploads_preserve_image_media_type_and_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let db = StationDb::open(dir.path(), &dir.path().join("workspaces")).unwrap();
        let owner = session(&db, dir.path(), "leader");
        let bytes = br#"<svg xmlns="http://www.w3.org/2000/svg" width="32" height="16"/>"#;
        let mut file = reference(bytes);
        file.name = "drawing.SVG".into();
        db.receive_file_chunk(&owner, &file, 0, bytes).unwrap();
        assert_eq!(
            db.list_artifacts(None).unwrap()[0].media_type,
            "image/svg+xml"
        );
        assert_eq!(
            db.conversation_file_bytes(&owner.key, &file).unwrap(),
            bytes
        );
    }

    #[test]
    fn upload_resumes_after_restart_and_retries_do_not_change_the_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let db = StationDb::open(dir.path(), &dir.path().join("workspaces")).unwrap();
        let owner = session(&db, dir.path(), "leader");
        let other = session(&db, dir.path(), "other");
        let bytes = vec![0xa7; CHUNK_BYTES + 37];
        let file = reference(&bytes);
        assert!(db
            .receive_file_chunk(&owner, &file, 2, &bytes[..3])
            .is_err());
        let changes = db.realtime.subscribe();
        assert_eq!(
            db.receive_file_chunk(&owner, &file, 0, &bytes[..CHUNK_BYTES])
                .unwrap(),
            CHUNK_BYTES
        );
        assert!(
            !changes.has_changed().unwrap(),
            "partial upload woke product synchronization"
        );
        assert!(db.artifact_content(&file.id).unwrap().is_none());
        assert!(db
            .receive_file_chunk(&other, &file, 0, &bytes[..CHUNK_BYTES])
            .is_err());
        drop(db);
        let db = StationDb::open(dir.path(), &dir.path().join("workspaces")).unwrap();
        assert_eq!(
            db.receive_file_chunk(&owner, &file, 0, &bytes[..CHUNK_BYTES])
                .unwrap(),
            CHUNK_BYTES
        );
        assert!(db.receive_file_chunk(&owner, &file, 0, &[0]).is_err());
        let mut changes = db.realtime.subscribe();
        assert_eq!(
            db.receive_file_chunk(&owner, &file, CHUNK_BYTES, &bytes[CHUNK_BYTES..])
                .unwrap(),
            bytes.len()
        );
        assert!(changes.has_changed().unwrap());
        changes.borrow_and_update();
        assert_eq!(
            db.receive_file_chunk(&owner, &file, 0, &bytes[..CHUNK_BYTES])
                .unwrap(),
            bytes.len()
        );
        assert!(
            !changes.has_changed().unwrap(),
            "duplicate upload woke product synchronization"
        );
        assert_eq!(
            db.conversation_file_bytes(&owner.key, &file).unwrap(),
            bytes
        );
        assert!(db.conversation_file_bytes(&other.key, &file).is_err());
        let mut forged = file.clone();
        forged.name = "another.pdf".into();
        assert!(db.conversation_file_bytes(&owner.key, &forged).is_err());
        assert_eq!(db.list_artifacts(None).unwrap().len(), 1);
    }
    #[test]
    fn rejects_hash_mismatch_and_traversal_and_accepts_empty_file() {
        let dir = tempfile::tempdir().unwrap();
        let db = StationDb::open(dir.path(), &dir.path().join("workspaces")).unwrap();
        let owner = session(&db, dir.path(), "leader");
        let file = reference(b"one");
        assert!(db.receive_file_chunk(&owner, &file, 0, b"two").is_err());
        assert!(db.artifact_content(&file.id).unwrap().is_none());
        let mut bad = file.clone();
        bad.name = "../outside".into();
        assert!(db.receive_file_chunk(&owner, &bad, 0, b"one").is_err());
        let empty = reference(b"");
        assert_eq!(db.receive_file_chunk(&owner, &empty, 0, b"").unwrap(), 0);
        assert_eq!(db.conversation_file_bytes(&owner.key, &empty).unwrap(), b"");
    }
}

impl StationDb {
    pub fn conversation_file_path(&self, key:&str, file:&FileRef)->Result<PathBuf> {
        ensure!(self.conversation_file_ref(key,&file.id)?==*file,"attachment_reference_mismatch");
        let snapshot=Snapshot{root:file.content_root.clone(),name:file.name.clone(),byte_len:file.byte_len};
        self.snapshot_range(&snapshot,0,0)?;
        self.snapshot_path(&snapshot)
    }

    pub fn conversation_file_ref(&self, key: &str, id: &str) -> Result<FileRef> {
        let conn = self.conn.lock().expect("db mutex");
        let (name,snapshot):(String,Snapshot)=conn.query_row("SELECT name,snapshot FROM conversation_file_snapshots WHERE session_key=?1 AND artifact_id=?2 UNION ALL SELECT a.name,a.snapshot FROM task_file_snapshots a JOIN product_tasks t ON t.task_id=a.task_id WHERE t.session_key=?1 AND a.artifact_id=?2",params![key,id],|r|Ok((r.get(0)?,r.get(1)?))).context("attachment_not_in_conversation")?;
        let file = FileRef {
            id: id.into(),
            name,
            byte_len: snapshot.byte_len,
            content_root: snapshot.root,
        };
        ensure!(file.valid(), "invalid_attachment");
        Ok(file)
    }

    /// Copies are owned by the destination conversation and survive source
    /// deletion. Destination IDs are deterministic for assignment retries.
    pub fn copy_message_files(
        &self,
        source: &str,
        target: &SessionRow,
        content: &str,
    ) -> Result<String> {
        use zork_client_types::files;
        let Some((text, references)) = files::decode(content) else {
            return Ok(content.into());
        };
        ensure!(files::valid(&references), "invalid_attachments");
        let mut copied = Vec::new();
        for reference in references {
            let bytes = self.conversation_file_bytes(source, &reference)?;
            let mut file = reference.clone();
            file.id = format!(
                "file-{}",
                zork_mesh::content_root(&serde_json::to_vec(&(source, &target.key, &reference))?)
            );
            self.receive_complete_file(target, &file, &bytes)?;
            self.record_file_source(&file.id, &json!({"session_key":source,"file":reference}))?;
            copied.push(file);
        }
        Ok(files::compose(&text, &copied))
    }

    pub fn record_file_source(&self, id: &str, source: &Value) -> Result<()> {
        self.conn.lock().expect("db mutex").execute(
            "INSERT OR IGNORE INTO conversation_file_origins(id,source_json) VALUES(?1,?2)",
            params![id, source.to_string()],
        )?;
        Ok(())
    }

    pub fn receive_complete_file(
        &self,
        target: &SessionRow,
        file: &FileRef,
        bytes: &[u8],
    ) -> Result<()> {
        ensure!(
            bytes.len() == file.byte_len && zork_mesh::content_root(bytes) == file.content_root,
            "attachment_hash_mismatch"
        );
        if self.artifact_content(&file.id)?.is_some() {
            ensure!(
                self.conversation_file_bytes(&target.key, file)? == bytes,
                "attachment_id_conflict"
            );
            return Ok(());
        }
        if bytes.is_empty() {
            self.receive_file_chunk(target, file, 0, bytes)?;
        }
        for (index, chunk) in bytes.chunks(CHUNK_BYTES).enumerate() {
            self.receive_file_chunk(target, file, index * CHUNK_BYTES, chunk)?;
        }
        Ok(())
    }

    pub fn receive_file_chunk(
        &self,
        session: &SessionRow,
        file: &FileRef,
        offset: usize,
        chunk: &[u8],
    ) -> Result<usize> {
        ensure!(
            file.valid() && chunk.len() <= CHUNK_BYTES,
            "invalid_attachment"
        );
        ensure!(
            offset <= file.byte_len && chunk.len() <= file.byte_len - offset,
            "invalid_attachment_offset"
        );
        let metadata = serde_json::to_string(file)?;
        let mut conn = self.conn.lock().expect("db mutex");
        let tx = conn.transaction()?;
        let existing: Option<(String, String, Snapshot)> = tx.query_row(
            "SELECT session_key,name,snapshot FROM conversation_file_snapshots WHERE artifact_id=?1 UNION ALL SELECT t.session_key,a.name,a.snapshot FROM task_file_snapshots a JOIN product_tasks t ON t.task_id=a.task_id WHERE a.artifact_id=?1", [&file.id],
            |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
        if let Some((key, name, snapshot)) = existing {
            ensure!(
                key == session.key
                    && name == file.name
                    && snapshot.byte_len == file.byte_len
                    && snapshot.root == file.content_root,
                "attachment_id_conflict"
            );
            ensure!(
                self.snapshot_range(&snapshot, offset, chunk.len())? == chunk,
                "attachment_chunk_conflict"
            );
            return Ok(snapshot.byte_len);
        }
        // Abandoned partial uploads have no message references and expire after a day.
        tx.execute(
            "DELETE FROM conversation_uploads WHERE updated_at < unixepoch()-86400",
            [],
        )?;
        let pending: Option<(String, String, usize)> = tx
            .query_row(
                "SELECT session_key,metadata,received FROM conversation_uploads WHERE id=?1",
                [&file.id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let mut received = if let Some((key, meta, received)) = pending {
            ensure!(
                key == session.key && meta == metadata,
                "attachment_id_conflict"
            );
            received
        } else {
            let count: usize =
                tx.query_row("SELECT count(*) FROM conversation_uploads", [], |r| {
                    r.get(0)
                })?;
            ensure!(count < 128, "too_many_pending_uploads");
            tx.execute("INSERT INTO conversation_uploads(id,session_key,metadata,received,updated_at) VALUES(?1,?2,?3,0,unixepoch())",params![file.id,session.key,metadata])?;
            0
        };
        ensure!(offset <= received, "attachment_offset_gap");
        if offset < received {
            let existing: Option<Vec<u8>> = tx
                .query_row(
                    "SELECT content FROM conversation_upload_chunks WHERE id=?1 AND offset=?2",
                    params![file.id, offset],
                    |r| r.get(0),
                )
                .optional()?;
            ensure!(
                existing.as_deref() == Some(chunk),
                "attachment_chunk_conflict"
            );
        } else {
            ensure!(
                !chunk.is_empty() || file.byte_len == 0,
                "empty_attachment_chunk"
            );
            tx.execute(
                "INSERT INTO conversation_upload_chunks(id,offset,content) VALUES(?1,?2,?3)",
                params![file.id, offset, chunk],
            )?;
            received += chunk.len();
        }
        if received == file.byte_len {
            let mut bytes = Vec::with_capacity(received);
            let mut query = tx.prepare(
                "SELECT content FROM conversation_upload_chunks WHERE id=?1 ORDER BY offset",
            )?;
            for chunk in query.query_map([&file.id], |r| r.get::<_, Vec<u8>>(0))? {
                bytes.extend_from_slice(&chunk?);
            }
            drop(query);
            ensure!(
                zork_mesh::content_root(&bytes) == file.content_root,
                "attachment_hash_mismatch"
            );
            let media_type = match Path::new(&file.name)
                .extension()
                .and_then(|s| s.to_str())
                .map(str::to_ascii_lowercase)
                .as_deref()
            {
                Some("png") => "image/png",
                Some("jpg" | "jpeg") => "image/jpeg",
                Some("svg") => "image/svg+xml",
                Some("pdf") => "application/pdf",
                Some("md" | "markdown") => "text/markdown",
                _ if std::str::from_utf8(&bytes).is_ok() && !bytes.contains(&0) => "text/plain",
                _ => "application/octet-stream",
            };
            let snapshot = self.freeze_file(&file.name, &bytes)?;
            tx.execute("INSERT INTO conversation_file_snapshots(artifact_id,session_key,name,source_path,media_type,workspace,snapshot,version,created_at) VALUES (?1,?2,?3,?1,?4,?5,?6,1,?7)",
                params![file.id,session.key,file.name,media_type,session.workspace_path,snapshot,now_rfc3339()])?;
            tx.execute("DELETE FROM conversation_uploads WHERE id=?1", [&file.id])?;
        } else {
            tx.execute(
                "UPDATE conversation_uploads SET received=?2,updated_at=unixepoch() WHERE id=?1",
                params![file.id, received],
            )?;
        }
        tx.commit()?;
        Ok(received)
    }

    pub fn conversation_file_bytes(&self, session_key: &str, file: &FileRef) -> Result<Vec<u8>> {
        ensure!(file.valid(), "invalid_attachment");
        let conn = self.conn.lock().expect("db mutex");
        let (name, snapshot): (String,Snapshot) = conn.query_row(
            "SELECT name,snapshot FROM conversation_file_snapshots WHERE session_key=?1 AND artifact_id=?2 UNION ALL SELECT a.name,a.snapshot FROM task_file_snapshots a JOIN product_tasks t ON t.task_id=a.task_id WHERE t.session_key=?1 AND a.artifact_id=?2",
            params![session_key,file.id], |r| Ok((r.get(0)?,r.get(1)?)))
            .context("attachment_not_in_conversation")?;
        ensure!(
            name == file.name
                && snapshot.byte_len == file.byte_len
                && snapshot.root == file.content_root,
            "attachment_reference_mismatch"
        );
        drop(conn);
        self.read_snapshot(&snapshot)
    }
}
