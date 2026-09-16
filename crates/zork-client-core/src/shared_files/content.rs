use super::*;
use projection::content_identity;
use zork_mesh::node::ObjectRef;

pub(super) struct Export {
    ticket: String,
    bytes: Arc<Vec<u8>>,
    peers: Vec<(String, u64)>,
}
struct Selection {
    content: ObjectRef,
    sources: Vec<ObjectRef>,
    peers: Vec<(String, u64)>,
    node: MeshNode,
}
impl SharedFiles {
    pub(super) async fn open_preview(self: &Arc<Self>, entry: Entry) -> Result<()> {
        if let Some(target) = entry.target {
            let mut s = self.owned.lock().unwrap();
            self.cancel_preview(&mut s);
            s.data.preview = Some(Preview {
                path: entry.path,
                name: entry.name,
                versions: vec![],
                selected: String::new(),
                loading: false,
                error: None,
                text: Some(format!("→ {target}")),
                truncated: false,
                mime: "text/plain".into(),
                cached: false,
                can_save: false,
                bytes: None,
            });
            self.publish(&mut s);
            return Ok(());
        }
        let version = entry.versions.first().context("文件版本不可用")?.clone();
        {
            let mut s = self.owned.lock().unwrap();
            self.cancel_preview(&mut s);
            s.data.preview = Some(Preview {
                path: entry.path,
                name: entry.name,
                versions: entry.versions,
                selected: version.root.clone(),
                loading: false,
                error: None,
                text: None,
                truncated: false,
                mime: String::new(),
                cached: false,
                can_save: version.can_read,
                bytes: None,
            });
            self.publish(&mut s);
        }
        self.select_version(version.root).await
    }
    pub(super) async fn select_version(self: &Arc<Self>, root: String) -> Result<()> {
        let mut cancel = self.preview_cancel.subscribe();
        let (selection, epoch, should_fetch) = {
            let mut s = self.owned.lock().unwrap();
            self.cancel_preview(&mut s);
            let epoch = s.preview_epoch;
            let preview = s.data.preview.as_mut().context("请先打开文件")?;
            let version = preview
                .versions
                .iter()
                .find(|v| v.root == root)
                .context("文件版本不可用")?
                .clone();
            preview.selected = root;
            preview.can_save = version.can_read;
            preview.bytes = None;
            preview.text = None;
            preview.error = None;
            preview.cached = false;
            preview.mime = mime(&preview.name).into();
            preview.truncated = false;
            let should_fetch = version.can_read
                && version.size <= 8 * 1024 * 1024
                && (preview.mime.starts_with("text/") || preview.mime.starts_with("image/"));
            preview.loading = should_fetch;
            if !version.can_read {
                preview.error = Some(
                    if version.size > crate::files::MAX_FILE_BYTES as u64 {
                        "此文件超过 300 MiB，暂不支持读取"
                    } else {
                        "来源设备离线，且尚无缓存副本"
                    }
                    .into(),
                );
            }
            let selection = self.selection(&s)?;
            cancel.checkpoint();
            self.publish(&mut s);
            (selection, epoch, should_fetch)
        };
        if !should_fetch {
            return Ok(());
        }
        let result = tokio::select! {
            result=self.fetch(&selection)=>result,
            _=cancel.changed()=>return Ok(()),
        };
        let mut s = self.owned.lock().unwrap();
        if s.preview_epoch != epoch || !self.selection_valid(&s, &selection) {
            return Ok(());
        }
        let preview = s.data.preview.as_mut().context("预览已关闭")?;
        preview.loading = false;
        match result {
            Ok((bytes, cached)) => {
                if preview.mime.starts_with("text/") {
                    let limit = bytes.len().min(128 * 1024);
                    match std::str::from_utf8(&bytes) {
                        Ok(text) => {
                            let mut end = limit;
                            while !text.is_char_boundary(end) {
                                end -= 1;
                            }
                            preview.text = Some(text[..end].into());
                            preview.truncated = end < bytes.len();
                        }
                        Err(_) => {
                            preview.error =
                                Some("该文件的编码暂不支持预览，可以保存副本后打开".into())
                        }
                    }
                }
                preview.bytes = Some(bytes);
                preview.cached = cached;
            }
            Err(error) => preview.error = Some(error.to_string()),
        }
        self.publish(&mut s);
        Ok(())
    }
    fn selection(&self, s: &Owned) -> Result<Selection> {
        let location = s.data.location.as_ref().context("共享空间已关闭")?;
        let preview = s.data.preview.as_ref().context("请先打开文件")?;
        let version = preview
            .versions
            .iter()
            .find(|v| v.root == preview.selected)
            .context("所选版本已不再发布")?;
        let sources = version
            .sources
            .iter()
            .map(|source| ObjectRef {
                origin: source.id.clone(),
                space: location.space.clone(),
                path: preview.path.clone(),
                root: version.root.clone(),
                size: version.size,
            })
            .collect::<Vec<_>>();
        Ok(Selection {
            content: sources.first().cloned().context("来源不可用")?,
            sources,
            peers: s
                .bindings
                .iter()
                .map(|b| (b.id.clone(), b.generation))
                .collect(),
            node: s.node.clone().context("文件网络尚未就绪")?,
        })
    }
    fn selection_valid(&self, s: &Owned, selection: &Selection) -> bool {
        selection.peers.iter().any(|(id, g)| {
            s.bindings.iter().any(|b| &b.id == id && &b.generation == g)
                && !self.store.replica_revoked(id).unwrap_or(true)
        }) && selection
            .sources
            .iter()
            .any(|object| s.data.devices.iter().any(|d| d.id == object.origin))
    }
    async fn fetch(&self, selection: &Selection) -> Result<(Arc<Vec<u8>>, bool)> {
        ensure!(
            selection.content.size <= crate::files::MAX_FILE_BYTES as u64,
            "文件超过 300 MiB"
        );
        let identity = content_identity(
            &selection.content.space,
            &selection.content.path,
            &selection.content.root,
        );
        {
            let s = self.owned.lock().unwrap();
            ensure!(self.selection_valid(&s, selection), "文件访问已失效");
            if let Some(bytes) = s.cache.get(&identity) {
                return Ok((bytes.clone(), true));
            }
        }
        let mut error = anyhow::anyhow!("文件内容暂不可用");
        for object in &selection.sources {
            match selection.node.tree_read(object).await {
                Ok(bytes) => {
                    let mut s = self.owned.lock().unwrap();
                    ensure!(self.selection_valid(&s, selection), "文件访问已失效");
                    let bytes = Arc::new(bytes);
                    if bytes.len() <= 8 * 1024 * 1024 {
                        while s.cache.len() >= 8 {
                            if let Some(key) = s.cache.keys().next().cloned() {
                                s.cache.remove(&key);
                            } else {
                                break;
                            }
                        }
                        s.cache.insert(identity, bytes.clone());
                    }
                    return Ok((bytes, false));
                }
                Err(next) => error = next,
            }
        }
        Err(error)
    }
    pub(super) async fn prepare_save(self: &Arc<Self>) -> Result<()> {
        let (selection, ticket, existing) = {
            let mut s = self.owned.lock().unwrap();
            if s.data.save.busy {
                return Ok(());
            }
            let preview = s.data.preview.as_ref().context("请先打开文件")?;
            ensure!(preview.can_save, "当前版本暂不可读取");
            let existing = preview.bytes.clone();
            let selection = self.selection(&s)?;
            let ticket = ulid::Ulid::new().to_string();
            let name = preview.name.clone();
            s.data.save = SaveState {
                busy: true,
                ticket: None,
                name,
                error: None,
                completed: false,
            };
            s.export = None;
            self.publish(&mut s);
            (selection, ticket, existing)
        };
        let result = match existing {
            Some(bytes) => Ok((bytes, false)),
            None => self.fetch(&selection).await,
        };
        let mut s = self.owned.lock().unwrap();
        if !self.selection_valid(&s, &selection) {
            s.data.save = SaveState::default();
            self.publish(&mut s);
            return Ok(());
        }
        match result {
            Ok((bytes, _)) => {
                s.export = Some(Export {
                    ticket: ticket.clone(),
                    bytes,
                    peers: selection.peers.clone(),
                });
                s.data.save.ticket = Some(ticket);
            }
            Err(e) => {
                s.data.save.busy = false;
                s.data.save.error = Some(e.to_string());
            }
        }
        self.publish(&mut s);
        Ok(())
    }
    pub(super) fn cancel_save(&self, ticket: &str, error: Option<String>) {
        let mut s = self.owned.lock().unwrap();
        if s.export.as_ref().is_some_and(|e| e.ticket == ticket) {
            s.export = None;
            s.data.save.busy = false;
            s.data.save.ticket = None;
            s.data.save.error = error;
            self.publish(&mut s);
        }
    }
    fn export_bytes(&self, ticket: &str) -> Result<Arc<Vec<u8>>> {
        let s = self.owned.lock().unwrap();
        let export = s
            .export
            .as_ref()
            .filter(|e| e.ticket == ticket)
            .context("保存操作已取消")?;
        ensure!(
            export.peers.iter().any(|(peer, g)| s
                .bindings
                .iter()
                .any(|b| &b.id == peer && &b.generation == g)
                && !self.store.replica_revoked(peer).unwrap_or(true)),
            "设备访问权限已撤销"
        );
        Ok(export.bytes.clone())
    }
    /// Platform adapters supply a destination; content selection, authorization,
    /// progress and completion remain in core. Called on a blocking IO worker.
    pub fn write_copy(&self, ticket: &str, writer: &mut impl std::io::Write) -> Result<()> {
        let result = (|| -> Result<()> {
            let bytes = self.export_bytes(ticket)?;
            for chunk in bytes.chunks(64 * 1024) {
                self.export_bytes(ticket)?;
                writer.write_all(chunk)?;
            }
            writer.flush()?;
            Ok(())
        })();
        let mut s = self.owned.lock().unwrap();
        if s.export.as_ref().is_some_and(|e| e.ticket == ticket) {
            s.data.save.completed = result.is_ok();
            s.data.save.error = result.as_ref().err().map(|e| e.to_string());
            s.data.save.busy = false;
            s.data.save.ticket = None;
            s.export = None;
            self.publish(&mut s);
        }
        result
    }
    pub fn save_copy(&self, ticket: &str, path: &std::path::Path) -> Result<()> {
        use std::io::Write;
        let parent = path.parent().context("保存位置无效")?;
        let temporary = parent.join(format!(".zork-save-{}", ulid::Ulid::new()));
        let result = (|| -> Result<()> {
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            let bytes = self.export_bytes(ticket)?;
            for chunk in bytes.chunks(64 * 1024) {
                self.export_bytes(ticket)?;
                file.write_all(chunk)?;
            }
            file.sync_all()?;
            self.export_bytes(ticket)?;
            std::fs::rename(&temporary, path)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        let mut s = self.owned.lock().unwrap();
        if s.export.as_ref().is_some_and(|e| e.ticket == ticket) {
            s.data.save.completed = result.is_ok();
            s.data.save.error = result.as_ref().err().map(|e| e.to_string());
            s.data.save.busy = false;
            s.data.save.ticket = None;
            s.export = None;
            self.publish(&mut s);
        }
        result
    }
    /// The Android renderer obtains only the current preview's already-fetched
    /// bytes. This local read cannot start a download or change versions.
    pub fn preview_bytes(&self, root: &str) -> Option<Arc<Vec<u8>>> {
        let s = self.owned.lock().unwrap();
        let selection = self.selection(&s).ok()?;
        if !self.selection_valid(&s, &selection) {
            return None;
        }
        s.data
            .preview
            .as_ref()
            .filter(|p| p.selected == root)
            .and_then(|p| p.bytes.clone())
    }
}

fn mime(name: &str) -> &'static str {
    match name
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "txt" | "md" | "json" | "jsonl" | "ndjson" | "toml" | "yaml" | "yml" | "rs" | "kt"
        | "java" | "js" | "ts" | "tsx" | "jsx" | "py" | "sh" | "log" | "css" | "html" | "svg"
        | "xml" | "csv" => "text/plain",
        _ => "application/octet-stream",
    }
}
