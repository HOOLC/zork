//! Read-only Chat file selection, verified content and export lifetime.
use crate::{api::TranscriptMessage, files::FileRef, state::Device, store::ClientStore};
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use zork_observe::ValueSource;
#[cfg(test)]
mod tests;

#[derive(Clone, Default, Debug, PartialEq, Eq, Serialize)]
pub struct Snapshot {
    pub preview: Option<Preview>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Preview {
    pub key: String,
    pub peer: String,
    pub session: String,
    pub file: FileRef,
    pub mime: String,
    pub loading: bool,
    pub error: Option<String>,
    pub text: Option<String>,
    pub truncated: bool,
    pub content_ready: bool,
    pub saving: bool,
    pub save_ticket: Option<String>,
    pub saved: bool,
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    Open {
        peer: String,
        session: String,
        message: String,
        file: String,
    },
    Close {
        key: String,
    },
    PrepareSave {
        key: String,
    },
    CancelSave {
        ticket: String,
    },
}

/// Verified bytes of one small message image for an inline thumbnail. The
/// same authorization and content checks as an opened preview apply.
pub async fn inline_bytes(
    store: &ClientStore,
    device: Arc<Device>,
    peer: &str,
    session: &str,
    message: &str,
    file: &str,
) -> Result<Vec<u8>> {
    ensure!(device.bound_peer() == Some(peer), "文件不属于当前设备");
    let generation = store.replica_generation(peer)?;
    let authorized = || {
        !store.replica_revoked(peer).unwrap_or(true)
            && store.replica_generation(peer).ok() == Some(generation)
            && !device.snapshot().revoked
    };
    ensure!(authorized(), "设备访问权限已撤销");
    let message = store
        .cached_message_at(peer, session, message, generation)?
        .context("消息已不可用，请重新打开会话")?;
    let TranscriptMessage::Message { content, .. } = message;
    let (_, files) = crate::files::decode(&content).context("消息不含文件")?;
    let file = files
        .into_iter()
        .find(|entry| entry.id == file)
        .context("文件不属于该消息")?;
    ensure!(file.valid(), "无效的文件引用");
    ensure!(
        crate::file_io::view(&file).thumbnail,
        "此文件不提供内联预览"
    );
    let bytes = device.artifact_content(&file.id).await?;
    ensure!(authorized(), "设备访问权限已撤销");
    ensure!(
        bytes.len() == file.byte_len && zork_mesh::content_root(&bytes) == file.content_root,
        "文件内容与消息引用不一致"
    );
    Ok(bytes)
}

struct Selection {
    preview: Preview,
    generation: u64,
    device: Arc<Device>,
    bytes: Option<Arc<Vec<u8>>>,
    task: Option<zork_notify::Task<()>>,
    request: u64,
}

pub struct Controller {
    store: Arc<ClientStore>,
    selected: Mutex<Option<Selection>>,
    pub(crate) source: ValueSource<Snapshot>,
}

impl Controller {
    pub fn new(store: Arc<ClientStore>) -> Arc<Self> {
        Arc::new(Self {
            store,
            selected: Mutex::new(None),
            source: ValueSource::new(Snapshot::default()),
        })
    }

    fn authorized(&self, s: &Selection) -> bool {
        !self.store.replica_revoked(&s.preview.peer).unwrap_or(true)
            && self.store.replica_generation(&s.preview.peer).ok() == Some(s.generation)
            && !s.device.snapshot().revoked
    }

    pub(crate) fn valid(&self) -> bool {
        self.selected
            .lock()
            .unwrap()
            .as_ref()
            .is_none_or(|s| self.authorized(s))
    }

    pub(crate) fn reconcile(&self) {
        let mut selected = self.selected.lock().unwrap();
        if selected.as_ref().is_some_and(|s| !self.authorized(s)) {
            selected.take();
            self.source.invalidate(Snapshot::default());
        }
    }

    fn publish(&self, selected: &Option<Selection>) {
        self.source.publish(Snapshot {
            preview: selected.as_ref().map(|s| s.preview.clone()),
        });
    }

    pub fn revoke(&self, peer: &str) {
        let mut selected = self.selected.lock().unwrap();
        if selected.as_ref().is_some_and(|s| s.preview.peer == peer) {
            selected.take();
            self.source.invalidate(Snapshot::default());
        }
    }

    pub fn leave_conversation(&self, peer: Option<&str>, session: Option<&str>) {
        let mut selected = self.selected.lock().unwrap();
        if selected.as_ref().is_some_and(|s| {
            Some(s.preview.peer.as_str()) != peer || Some(s.preview.session.as_str()) != session
        }) {
            selected.take();
            self.source.invalidate(Snapshot::default());
        }
    }

    pub fn pause(&self) {
        let mut selected = self.selected.lock().unwrap();
        if let Some(s) = selected.as_mut().filter(|s| s.preview.loading) {
            s.task.take();
            s.request += 1;
            s.preview.loading = false;
            s.preview.saving = false;
            s.preview.error = Some("连接已暂停，请重新打开文件".into());
            self.publish(&selected);
        }
        // An already prepared export survives the system document picker.
    }

    pub fn apply(self: &Arc<Self>, action: Action, device: Option<Arc<Device>>) -> Result<()> {
        self.reconcile();
        let mut selected = self.selected.lock().unwrap();
        match action {
            Action::Open {
                peer,
                session,
                message,
                file,
            } => {
                let device = device.context("客户端连接已暂停，请重新连接")?;
                ensure!(
                    device.bound_peer() == Some(peer.as_str()),
                    "文件不属于当前设备"
                );
                let generation = self.store.replica_generation(&peer)?;
                let message = self
                    .store
                    .cached_message_at(&peer, &session, &message, generation)?
                    .context("消息已不可用，请重新打开会话")?;
                let TranscriptMessage::Message { content, .. } = message;
                let (_, files) = crate::files::decode(&content).context("消息不含文件")?;
                let file = files
                    .into_iter()
                    .find(|entry| entry.id == file)
                    .context("文件不属于该消息")?;
                ensure!(file.valid(), "无效的文件引用");
                let mime = crate::file_io::preview_mime(&file.name).to_owned();
                let previewable = file.byte_len <= 8 * 1024 * 1024
                    && (mime.starts_with("text/") || mime.starts_with("image/"));
                let next = Selection {
                    preview: Preview {
                        key: ulid::Ulid::new().to_string(),
                        peer,
                        session,
                        file,
                        mime,
                        loading: false,
                        error: None,
                        text: None,
                        truncated: false,
                        content_ready: false,
                        saving: false,
                        save_ticket: None,
                        saved: false,
                    },
                    generation,
                    device,
                    bytes: None,
                    task: None,
                    request: 0,
                };
                ensure!(self.authorized(&next), "设备访问权限已撤销");
                *selected = Some(next);
                self.source.invalidate(Snapshot {
                    preview: selected.as_ref().map(|s| s.preview.clone()),
                });
                if previewable {
                    self.fetch(selected.as_mut().unwrap(), false)?;
                }
            }
            Action::Close { key } => {
                if selected.as_ref().is_some_and(|s| s.preview.key == key) {
                    selected.take();
                    self.source.invalidate(Snapshot::default());
                }
            }
            Action::PrepareSave { key } => {
                let s = selected
                    .as_mut()
                    .filter(|s| s.preview.key == key)
                    .context("文件选择已失效")?;
                if s.preview.saving {
                    return Ok(());
                }
                s.preview.error = None;
                s.preview.saved = false;
                s.preview.saving = true;
                if s.bytes.is_some() {
                    s.preview.save_ticket = Some(ulid::Ulid::new().to_string());
                } else {
                    self.fetch(s, true)?;
                }
            }
            Action::CancelSave { ticket } => {
                if let Some(s) = selected
                    .as_mut()
                    .filter(|s| s.preview.save_ticket.as_deref() == Some(&ticket))
                {
                    s.preview.save_ticket = None;
                    s.preview.saving = false;
                }
            }
        }
        self.publish(&selected);
        Ok(())
    }

    fn fetch(self: &Arc<Self>, s: &mut Selection, save: bool) -> Result<()> {
        let runtime =
            tokio::runtime::Handle::try_current().context("文件操作需要客户端运行环境")?;
        s.request += 1;
        s.preview.loading = true;
        let (key, request, file, device) = (
            s.preview.key.clone(),
            s.request,
            s.preview.file.clone(),
            s.device.clone(),
        );
        let weak = Arc::downgrade(self);
        s.task = Some(zork_notify::Task(runtime.spawn(async move {
            let result = async {
                let bytes = device.artifact_content(&file.id).await?;
                ensure!(
                    bytes.len() == file.byte_len
                        && zork_mesh::content_root(&bytes) == file.content_root,
                    "文件内容与消息引用不一致"
                );
                Ok::<_, anyhow::Error>(Arc::new(bytes))
            }
            .await;
            let Some(this) = weak.upgrade() else {
                return;
            };
            let mut selected = this.selected.lock().unwrap();
            let Some(s) = selected
                .as_mut()
                .filter(|s| s.preview.key == key && s.request == request)
            else {
                return;
            };
            if !this.authorized(s) {
                selected.take();
                this.source.invalidate(Snapshot::default());
                return;
            }
            s.preview.loading = false;
            match result {
                Ok(bytes) => {
                    if s.preview.mime.starts_with("text/") {
                        if let Ok(text) = std::str::from_utf8(&bytes) {
                            let mut end = text.len().min(128 * 1024);
                            while !text.is_char_boundary(end) {
                                end -= 1;
                            }
                            s.preview.text = Some(text[..end].to_owned());
                            s.preview.truncated = end < text.len();
                        }
                    }
                    s.preview.content_ready = true;
                    s.bytes = Some(bytes);
                    if save {
                        s.preview.save_ticket = Some(ulid::Ulid::new().to_string());
                    }
                }
                Err(error) => {
                    s.preview.error = Some(error.to_string());
                    s.preview.saving = false;
                }
            }
            this.publish(&selected);
        })));
        Ok(())
    }

    pub fn preview_bytes(&self, key: &str) -> Option<Arc<Vec<u8>>> {
        let selected = self.selected.lock().unwrap();
        selected
            .as_ref()
            .filter(|s| {
                s.preview.key == key
                    && self.authorized(s)
                    && s.preview.file.byte_len <= 8 * 1024 * 1024
            })
            .and_then(|s| s.bytes.clone())
    }

    fn export_bytes(&self, ticket: &str) -> Result<Arc<Vec<u8>>> {
        let selected = self.selected.lock().unwrap();
        let s = selected
            .as_ref()
            .filter(|s| s.preview.save_ticket.as_deref() == Some(ticket))
            .context("保存请求已失效")?;
        ensure!(self.authorized(s), "设备访问权限已撤销");
        s.bytes.clone().context("文件尚未准备完成")
    }

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
        let mut selected = self.selected.lock().unwrap();
        if let Some(s) = selected
            .as_mut()
            .filter(|s| s.preview.save_ticket.as_deref() == Some(ticket))
        {
            s.preview.save_ticket = None;
            s.preview.saving = false;
            s.preview.saved = result.is_ok();
            s.preview.error = result.as_ref().err().map(ToString::to_string);
            self.publish(&selected);
        }
        result
    }
}
