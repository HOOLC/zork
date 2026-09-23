pub(super) mod content;
mod fan;
pub(super) mod image;
mod layout;
pub(super) mod message;
mod preview;
mod system_preview;
use super::*;
pub(super) use fan::UiState;

impl RootView {
    fn open_message_file(
        &mut self,
        file: &zork_client_core::files::FileRef,
        session: &str,
        cx: &mut Context<Self>,
    ) {
        if self.selected_session.as_deref() != Some(session) {
            return;
        }
        let artifact = self.message_file_artifact(file, session);
        self.select_artifact(artifact, cx);
    }

    fn message_file_artifact(
        &self,
        file: &zork_client_core::files::FileRef,
        session: &str,
    ) -> crate::api::Artifact {
        self.drive
            .items
            .iter()
            .find(|a| a.artifact_id == file.id && a.session_id.as_deref() == Some(session))
            .cloned()
            .unwrap_or_else(|| {
                let media_type = match std::path::Path::new(&file.name)
                    .extension()
                    .and_then(|s| s.to_str())
                    .map(str::to_ascii_lowercase)
                    .as_deref()
                {
                    Some("png") => "image/png",
                    Some("jpg" | "jpeg") => "image/jpeg",
                    Some("svg") => "image/svg+xml",
                    Some("md" | "markdown") => "text/markdown",
                    Some("txt" | "csv" | "json" | "rs" | "py" | "log") => "text/plain",
                    _ => "application/octet-stream",
                };
                crate::api::Artifact {
                    artifact_id: file.id.clone(),
                    task_id: None,
                    session_id: Some(session.into()),
                    task_title: String::new(),
                    workspace: String::new(),
                    name: file.name.clone(),
                    source_path: file.name.clone(),
                    media_type: media_type.into(),
                    caption: None,
                    byte_len: file.byte_len as i64,
                    version: 1,
                    created_at: String::new(),
                }
            })
    }

    fn open_message_files(
        &mut self,
        files: Arc<Vec<zork_client_core::files::FileRef>>,
        selected: usize,
        session: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.selected_session.as_deref() != Some(session) {
            return;
        }
        let artifacts = files
            .iter()
            .map(|f| self.message_file_artifact(f, session))
            .collect();
        self.open_artifact_group(artifacts, selected, window, cx);
    }
    pub(super) fn choose_files(&mut self, cx: &mut Context<Self>) {
        if !self.can_send_selected() {
            return;
        }
        let Some(session) = self.selected_session.clone() else {
            return;
        };
        let device = self.core_device.clone();
        let picker = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some(self.locale.text("add_files").into()),
        });
        cx.spawn(async move |this, cx| match picker.await {
            Ok(Ok(Some(paths))) => {
                let _ = this.update(cx, |v, cx| {
                    v.preparing_files += 1;
                    zork_ui::components::region::invalidate(cx, &["composer"]);
                });
                let result = cx
                    .background_executor()
                    .spawn(async move {
                        for path in paths {
                            device.attach_path(&session, &path)?;
                        }
                        anyhow::Ok(())
                    })
                    .await;
                let _ = this.update(cx, |v, cx| {
                    v.preparing_files = v.preparing_files.saturating_sub(1);
                    if let Err(error) = result {
                        v.error = Some(error.to_string());
                    }
                    zork_ui::components::region::invalidate(cx, &["composer", "overlays"]);
                });
            }
            Ok(Ok(None)) => {}
            _ => {
                let _ = this.update(cx, |v, cx| {
                    v.error = Some("File picker failed".into());
                    zork_ui::components::region::invalidate(cx, &["composer"]);
                });
            }
        })
        .detach();
        zork_ui::components::region::invalidate(cx, &["composer", "overlays"]);
    }

    pub(super) fn attach_paths(&mut self, paths: Vec<std::path::PathBuf>, cx: &mut Context<Self>) {
        if !self.can_send_selected() {
            return;
        }
        let Some(session) = self.selected_session.clone() else {
            return;
        };
        let device = self.core_device.clone();
        self.preparing_files += 1;
        zork_ui::components::region::invalidate(cx, &["composer"]);
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    for path in paths {
                        device.attach_path(&session, &path)?;
                    }
                    anyhow::Ok(())
                })
                .await;
            let _ = this.update(cx, |v, cx| {
                v.preparing_files = v.preparing_files.saturating_sub(1);
                if let Err(error) = result {
                    v.error = Some(error.to_string());
                }
                zork_ui::components::region::invalidate(cx, &["composer"]);
            });
        })
        .detach();
    }

    pub(super) fn paste_files(&mut self, item: &gpui::ClipboardItem, cx: &mut Context<Self>) {
        if !self.can_send_selected() {
            return;
        }
        let Some(session) = self.selected_session.clone() else {
            return;
        };
        for entry in item.entries() {
            match entry {
                gpui::ClipboardEntry::ExternalPaths(paths) => {
                    self.attach_paths(paths.paths().to_vec(), cx)
                }
                gpui::ClipboardEntry::Image(image) => {
                    let image = image.clone();
                    let device = self.core_device.clone();
                    let session = session.clone();
                    let prefix = self.locale.text("clipboard_image");
                    self.preparing_files += 1;
                    zork_ui::components::region::invalidate(cx, &["composer"]);
                    cx.spawn(async move |this, cx| {
                        let result = cx
                            .background_executor()
                            .spawn(async move {
                                let extension = match image.format() {
                                    gpui::ImageFormat::Png => "png",
                                    gpui::ImageFormat::Jpeg => "jpg",
                                    gpui::ImageFormat::Webp => "webp",
                                    gpui::ImageFormat::Gif => "gif",
                                    gpui::ImageFormat::Svg => "svg",
                                    gpui::ImageFormat::Bmp => "bmp",
                                    gpui::ImageFormat::Tiff => "tiff",
                                    gpui::ImageFormat::Ico => "ico",
                                    gpui::ImageFormat::Pnm => "pnm",
                                };
                                device.attach_file(
                                    &session,
                                    &format!(
                                        "{prefix}-{}.{}",
                                        chrono::Local::now().format("%Y%m%d-%H%M%S"),
                                        extension
                                    ),
                                    image.bytes(),
                                )
                            })
                            .await;
                        let _ = this.update(cx, |v, cx| {
                            v.preparing_files = v.preparing_files.saturating_sub(1);
                            if let Err(error) = result {
                                v.error = Some(error.to_string());
                            }
                            zork_ui::components::region::invalidate(cx, &["composer"]);
                        });
                    })
                    .detach();
                }
                _ => {}
            }
        }
    }
}
