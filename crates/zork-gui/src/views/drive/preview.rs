//! Core byte loading and platform decoding for the shared attachment viewer.
use super::super::files::content::{self, Kind};
use super::super::files::image::{self as images, DecodedImage};
use super::*;
use crate::components::message::MessageDocument;
#[derive(Default)]
pub(super) struct PreviewState {
    group: Arc<Vec<Artifact>>,
    index: usize,
    image: Option<DecodedImage>,
    image_failed: bool,
    overview: bool,
    text: Option<Arc<str>>,
    document: Option<MessageDocument>,
    source_document: Option<MessageDocument>,
    text_truncated: bool,
    return_focus: Option<FocusHandle>,
    epoch: u64,
    component: Option<Entity<zork_ui::attachment_viewer::Viewer>>,
    thumbnails: Arc<Vec<Option<Arc<gpui::RenderImage>>>>,
}
impl PreviewState {
    fn reset_file(&mut self, _: &mut gpui::App) {
        self.image = None;
        self.image_failed = false;
        self.overview = false;
        self.text = None;
        self.document = None;
        self.source_document = None;
        self.text_truncated = false;
        self.epoch = self.epoch.wrapping_add(1);
    }
}
impl RootView {
    #[cfg(feature = "headless-bench")]
    pub fn benchmark_attachment_overlay(&self, cx: &gpui::App) -> serde_json::Value {
        self.drive
            .viewer
            .component
            .as_ref()
            .map_or(serde_json::Value::Null, |view| view.read(cx).inspect())
    }

    #[cfg(feature = "headless-bench")]
    pub(super) fn seed_preview_fixture(
        &mut self,
        artifact: Artifact,
        bytes: Vec<u8>,
        kind: &str,
        cx: &mut Context<Self>,
    ) {
        self.drive.viewer.reset_file(cx);
        self.reset_preview_group(artifact.clone());
        let image = content::decode(
            &artifact.name,
            &artifact.media_type,
            &bytes,
            &cx.svg_renderer(),
            1536,
        );
        self.drive.selected = Some(artifact);
        if !matches!(kind, "loading" | "failed") {
            self.accept_preview_bytes(bytes, image, cx);
        } else {
            self.drive.bytes = None;
        }
        self.drive.preview_failed = kind == "failed";
        self.drive.preview_focused = false;
    }

    pub(super) fn reset_preview_group(&mut self, artifact: Artifact) {
        self.drive.viewer.group = Arc::new(vec![artifact]);
        self.drive.viewer.index = 0;
        self.drive.viewer.thumbnails = Default::default();
    }

    /// Thumbnails for the open group, by position, for the viewer's strip.
    pub(in crate::views) fn set_preview_thumbnails(
        &mut self,
        thumbnails: Vec<Option<Arc<gpui::RenderImage>>>,
    ) {
        self.drive.viewer.thumbnails = Arc::new(thumbnails);
    }

    pub(in crate::views) fn open_artifact_group(
        &mut self,
        group: Vec<Artifact>,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(artifact) = group.get(index).cloned() else {
            return;
        };
        self.drive.viewer.return_focus = window.focused(cx);
        self.drive.viewer.group = Arc::new(group);
        self.drive.viewer.index = index;
        self.drive.viewer.thumbnails = Default::default();
        self.load_artifact_preview(artifact, cx);
    }

    pub(super) fn load_artifact_preview(&mut self, artifact: Artifact, cx: &mut Context<Self>) {
        self.drive.files_session = None;
        self.drive.preview_focused = self.drive.selected.is_some() && self.drive.preview_focused;
        self.drive.selected = Some(artifact.clone());
        self.drive.viewer.reset_file(cx);
        self.drive.bytes = None;
        self.drive.image = None;
        self.drive.preview_failed = false;
        self.drive.notice = None;
        self.drive.request = self.drive.request.wrapping_add(1);
        let request = self.drive.request;
        let core = self.core_device.clone();
        let renderer = cx.svg_renderer();
        #[cfg(feature = "headless-bench")]
        let offline = self.benchmark_offline;
        #[cfg(not(feature = "headless-bench"))]
        let offline = false;
        let local = self.local_cache.clone();
        cx.spawn(async move |this, cx| {
            let result = if offline {
                local
                    .and_then(|(store, node)| {
                        store
                            .blob(&node, &format!("upload:{}", artifact.artifact_id))
                            .ok()
                            .flatten()
                    })
                    .ok_or_else(|| anyhow::anyhow!("snapshot unavailable"))
            } else {
                core.artifact_content(&artifact.artifact_id)
                    .await
                    .map_err(anyhow::Error::from)
            };
            let result = match result {
                Ok(bytes) => {
                    let image_bytes = bytes.clone();
                    let name = artifact.name.clone();
                    let mime = artifact.media_type.clone();
                    let image = cx
                        .background_executor()
                        .spawn(async move {
                            content::decode(&name, &mime, &image_bytes, &renderer, 1536)
                        })
                        .await;
                    Ok((bytes, image))
                }
                Err(error) => Err(error),
            };
            let _ = this.update(cx, |v, cx| {
                if v.drive.request != request {
                    return;
                }
                match result {
                    Ok((bytes, image)) => v.accept_preview_bytes(bytes, image, cx),
                    Err(_) => v.drive.preview_failed = true,
                }
                zork_ui::components::region::invalidate_all(cx);
            });
        })
        .detach();
        zork_ui::components::region::invalidate_all(cx);
    }

    fn accept_preview_bytes(
        &mut self,
        bytes: Vec<u8>,
        content: anyhow::Result<content::Content>,
        _cx: &mut Context<Self>,
    ) {
        match content {
            Ok(content) => {
                self.drive.viewer.overview = content.overview;
                self.drive.viewer.image = content.image;
                self.drive.viewer.text_truncated = content.truncated;
                if let Some(text) = content.text {
                    self.drive.viewer.source_document = Some(MessageDocument::plain(&text));
                    self.drive.viewer.document = Some(match content.kind {
                        Kind::Markdown => MessageDocument::parse(&text),
                        Kind::Code(language) => {
                            let longest = text.split(|c| c != '`').map(str::len).max().unwrap_or(0);
                            let fence = "`".repeat(longest.max(2) + 1);
                            MessageDocument::parse(&format!("{fence}{language}\n{text}\n{fence}"))
                        }
                        _ => MessageDocument::plain(&text),
                    });
                    self.drive.viewer.text = Some(text);
                }
            }
            Err(_) => self.drive.viewer.image_failed = true,
        }
        self.drive.bytes = Some(Arc::new(bytes));
    }

    fn move_preview(&mut self, step: isize, cx: &mut Context<Self>) {
        let Some(index) = self
            .drive
            .viewer
            .index
            .checked_add_signed(step)
            .filter(|i| *i < self.drive.viewer.group.len())
        else {
            return;
        };
        let artifact = self.drive.viewer.group[index].clone();
        self.drive.viewer.index = index;
        self.load_artifact_preview(artifact, cx);
    }

    pub(super) fn sync_preview_focus(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        use zork_ui::attachment_viewer::{Action, Data, Viewer};
        let open = self.has_conversation_artifact_preview();
        if !open && !self.drive.preview_focused {
            return;
        }
        let component = if let Some(view) = &self.drive.viewer.component {
            view.clone()
        } else {
            let locale = self.locale;
            let view = cx.new(|cx| {
                Viewer::new(
                    zork_ui::resources::Text(Rc::new(move |key| locale.text(key).into())),
                    cx,
                )
            });
            cx.subscribe(&view, |v, _, action: &Action, cx| {
                match action {
                    Action::Close => v.close_conversation_artifact(),
                    Action::Move(step) => v.move_preview(*step, cx),
                    Action::Save => v.save_artifact_copy(cx),
                    Action::Retry => {
                        if let Some(artifact) = &v.drive.selected {
                            v.load_artifact_preview(artifact.clone(), cx);
                        }
                    }
                    Action::Reuse => {
                        if let (Some(file), Some(bytes), Some(session)) =
                            (&v.drive.selected, &v.drive.bytes, &v.selected_session)
                        {
                            let reference = zork_client_core::files::FileRef {
                                id: file.artifact_id.clone(),
                                name: file.name.clone(),
                                byte_len: bytes.len(),
                                content_root: zork_client_core::api::content_root(bytes),
                            };
                            if let Err(error) = v.core_device.reuse_file(session, reference, bytes)
                            {
                                v.error = Some(error.to_string());
                            } else {
                                v.drive.notice = Some("preview_added_to_draft");
                            }
                        }
                    }
                }
                zork_ui::components::region::invalidate_all(cx);
            })
            .detach();
            self.drive.viewer.component = Some(view.clone());
            view
        };
        if !open && self.drive.preview_focused {
            self.drive.viewer.reset_file(cx);
        }
        self.drive.preview_focused = open;
        let info = self
            .drive
            .selected
            .as_ref()
            .filter(|_| open)
            .map(|artifact| preview_info(artifact, self.drive.viewer.image.as_ref()));
        let data = Data {
            info,
            epoch: self.drive.viewer.epoch,
            group: Arc::new(
                self.drive
                    .viewer
                    .group
                    .iter()
                    .map(|artifact| preview_info(artifact, None))
                    .collect(),
            ),
            index: self.drive.viewer.index,
            loaded: self.drive.bytes.is_some(),
            failed: self.drive.preview_failed,
            saving: self.drive.saving,
            choosing_save: self.drive.choosing_save,
            notice: self.drive.notice,
            can_reuse: self.can_send_selected()
                && self.drive.bytes.is_some()
                && self
                    .drive
                    .selected
                    .as_ref()
                    .is_some_and(|a| a.session_id.as_ref() == self.selected_session.as_ref()),
            image: self.drive.viewer.image.clone(),
            image_failed: self.drive.viewer.image_failed,
            overview: self.drive.viewer.overview,
            text: self.drive.viewer.text.clone(),
            document: self.drive.viewer.document.clone(),
            source_document: self.drive.viewer.source_document.clone(),
            text_truncated: self.drive.viewer.text_truncated,
            thumbnails: self.drive.viewer.thumbnails.clone(),
        };
        let focus = self.drive.viewer.return_focus.take();
        let locale = self.locale;
        component.update(cx, |view, cx| {
            if focus.is_some() {
                view.remember_source(focus);
            }
            view.set_data(
                data,
                zork_ui::resources::Text(Rc::new(move |key| locale.text(key).into())),
                cx,
            );
        });
    }
    pub(in crate::views) fn render_conversation_artifact_preview(
        &mut self,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> gpui::AnyElement {
        self.drive
            .viewer
            .component
            .clone()
            .map(|view| view.into_any_element())
            .unwrap_or_else(|| gpui::Empty.into_any_element())
    }
}
fn preview_info(
    artifact: &Artifact,
    image: Option<&DecodedImage>,
) -> zork_ui::attachment_viewer::Info {
    zork_ui::attachment_viewer::Info {
        id: artifact.artifact_id.clone(),
        name: artifact.name.clone(),
        image_view: content::kind(&artifact.name, &artifact.media_type).is_image(),
        subtitle: format!(
            "{}{} · {}",
            images::kind(&artifact.name),
            image
                .map(|image| format!(" · {:.0} × {:.0}", image.size.width, image.size.height))
                .unwrap_or_default(),
            file_size(artifact.byte_len)
        ),
        source_path: artifact.source_path.clone(),
        version: artifact.version,
        created_at: artifact.created_at.clone(),
    }
}
