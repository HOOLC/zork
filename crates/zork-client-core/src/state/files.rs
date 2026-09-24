use super::Device;
use anyhow::{ensure, Context, Result};
use zork_client_types::files::{self, FileRef};

impl Device {
    /// Snapshot before publishing a draft reference. A picker callback retains
    /// this Device and conversation, even if the user navigates elsewhere.
    pub fn attach_file(&self, session: &str, name: &str, bytes: &[u8]) -> Result<FileRef> {
        crate::valid_session(session)?;
        ensure!(
            bytes.len() <= files::MAX_FILE_BYTES,
            "单个附件不能超过 300 MiB"
        );
        let file = FileRef {
            id: format!("file-{}", ulid::Ulid::new()),
            name: name.into(),
            byte_len: bytes.len(),
            content_root: zork_mesh::content_root(bytes),
        };
        ensure!(file.valid(), "invalid attachment filename");
        let _serial = self.draft_gate.lock().unwrap();
        let mut draft = self.draft(session).as_ref().clone();
        draft.files.push(file.clone());
        ensure!(
            files::valid(&draft.files),
            "每条消息最多 16 个附件，共 1200 MiB"
        );
        let (store, node) = self
            .cache
            .as_ref()
            .context("persistent file storage unavailable")?;
        store.put_blob(node, &format!("upload:{}", file.id), bytes)?;
        self.save_draft_state(session, draft)?;
        Ok(file)
    }

    pub fn attach_path(&self, session: &str, path: &std::path::Path) -> Result<FileRef> {
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .context("invalid filename")?
            .to_owned();
        self.attach_named_path(session, path, &name)
    }

    /// Mobile pickers hand over a private copy whose file name is not the
    /// user's; the display name travels separately.
    pub fn attach_named_path(
        &self,
        session: &str,
        path: &std::path::Path,
        name: &str,
    ) -> Result<FileRef> {
        use std::io::Read;
        let source = std::fs::File::open(path)?;
        let before = source.metadata()?;
        ensure!(before.is_file(), "请选择普通文件");
        ensure!(
            before.len() <= files::MAX_FILE_BYTES as u64,
            "单个附件不能超过 300 MiB"
        );
        let mut bytes = Vec::new();
        (&source)
            .take(files::MAX_FILE_BYTES as u64 + 1)
            .read_to_end(&mut bytes)?;
        let after = source.metadata()?;
        ensure!(
            before.len() == after.len()
                && before.modified()? == after.modified()?
                && bytes.len() as u64 == after.len(),
            "文件读取期间发生变化，请重试"
        );
        self.attach_file(session, name, &bytes)
    }

    /// Bytes of a draft file for a bounded inline preview; never leaves the
    /// snapshot stored when the file was attached.
    pub fn draft_file_bytes(&self, session: &str, id: &str) -> Result<Vec<u8>> {
        let file = self
            .draft(session)
            .files
            .iter()
            .find(|f| f.id == id)
            .cloned()
            .context("附件已不在草稿中")?;
        ensure!(
            file.byte_len <= crate::file_io::PREVIEW_BYTES,
            "文件过大，无法预览"
        );
        let (store, node) = self
            .cache
            .as_ref()
            .context("persistent file storage unavailable")?;
        let bytes = store
            .blob(node, &format!("upload:{}", file.id))?
            .context("附件内容已丢失，请重新添加")?;
        ensure!(
            bytes.len() == file.byte_len && zork_mesh::content_root(&bytes) == file.content_root,
            "附件内容与草稿引用不一致"
        );
        Ok(bytes)
    }

    pub fn remove_file(&self, session: &str, id: &str) -> Result<()> {
        let _serial = self.draft_gate.lock().unwrap();
        let mut draft = self.draft(session).as_ref().clone();
        draft.files.retain(|f| f.id != id);
        self.save_draft_state(session, draft)
    }

    pub fn reuse_file(&self, session: &str, file: FileRef, bytes: &[u8]) -> Result<()> {
        crate::valid_session(session)?;
        ensure!(
            file.valid()
                && bytes.len() == file.byte_len
                && zork_mesh::content_root(bytes) == file.content_root,
            "attachment snapshot changed"
        );
        let _serial = self.draft_gate.lock().unwrap();
        let mut draft = self.draft(session).as_ref().clone();
        if !draft.files.iter().any(|f| f.id == file.id) {
            draft.files.push(file.clone());
        }
        ensure!(
            files::valid(&draft.files),
            "每条消息最多 16 个附件，共 1200 MiB"
        );
        let (store, node) = self
            .cache
            .as_ref()
            .context("persistent file storage unavailable")?;
        store.put_blob(node, &format!("upload:{}", file.id), bytes)?;
        self.save_draft_state(session, draft)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn named_copies_keep_display_names_and_preview_the_snapshot() {
        let root = tempfile::tempdir().unwrap();
        let store = Arc::new(crate::store::ClientStore::open(root.path()).unwrap());
        let device = Device::open(
            Arc::new(crate::api::StationClient::new("http://127.0.0.1:9", None)),
            Some((store, "node".into())),
            true,
        );
        let copy = root.path().join("picker-3f9a.tmp");
        std::fs::write(&copy, b"\x89PNG fixture").unwrap();
        let file = device
            .attach_named_path("leader", &copy, "首页草图.png")
            .unwrap();
        std::fs::remove_file(copy).unwrap();
        assert_eq!(file.name, "首页草图.png");
        assert_eq!(
            device.draft_file_bytes("leader", &file.id).unwrap(),
            b"\x89PNG fixture"
        );
        assert!(device.draft_file_bytes("other", &file.id).is_err());
        device.remove_file("leader", &file.id).unwrap();
        assert!(device.draft("leader").files.is_empty());
        assert!(device.draft_file_bytes("leader", &file.id).is_err());
    }

    #[test]
    fn snapshots_survive_source_deletion_navigation_and_restart() {
        let root = tempfile::tempdir().unwrap();
        let store = Arc::new(crate::store::ClientStore::open(root.path()).unwrap());
        let device = Device::open(
            Arc::new(crate::api::StationClient::new("http://127.0.0.1:9", None)),
            Some((store.clone(), "node".into())),
            true,
        );
        let source = root.path().join("report.txt");
        std::fs::write(&source, b"immutable").unwrap();
        let file = device.attach_path("leader", &source).unwrap();
        std::fs::remove_file(source).unwrap();
        assert!(device.draft("other").files.is_empty());
        device.edit_draft("leader", "read this".into()).unwrap();
        let queued = device.submit_draft("leader", "read this").unwrap().unwrap();
        assert!(device.draft("leader").files.is_empty());
        assert_eq!(
            files::decode(&queued.content).unwrap().1,
            vec![file.clone()]
        );
        assert_eq!(
            store
                .blob("node", &format!("upload:{}", file.id))
                .unwrap()
                .unwrap(),
            b"immutable"
        );
        assert!(store
            .blob("other-node", &format!("upload:{}", file.id))
            .unwrap()
            .is_none());
        drop(device);
        drop(store);
        let store = Arc::new(crate::store::ClientStore::open(root.path()).unwrap());
        assert_eq!(
            store.outbox("node").unwrap()[0].request_id,
            queued.request_id
        );
        assert_eq!(
            store
                .blob("node", &format!("upload:{}", file.id))
                .unwrap()
                .unwrap(),
            b"immutable"
        );
    }
}
