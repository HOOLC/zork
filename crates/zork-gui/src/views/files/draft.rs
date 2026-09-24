//! Draft attachments: a row of capsules at the top of the composer surface.
//! Order follows the core draft; one click opens the viewer, × removes.
use super::*;
use std::cell::RefCell;
use zork_ui::components::widgets::composer::files as component;

#[derive(Default)]
pub(in crate::views) struct UiState {
    pub message_previews: Rc<RefCell<super::message::PreviewCache>>,
}

/// Row key for draft thumbnails; never a transcript row index.
const DRAFT_ROW: usize = usize::MAX;

/// "Type · size" under a file name, shared by draft chips and message blocks.
pub(in crate::views) fn meta(file: &zork_client_core::files::FileRef) -> String {
    let kind = super::image::kind(&file.name);
    let size = crate::views::drive::file_size(file.byte_len as i64);
    if kind.is_empty() {
        size
    } else {
        format!("{kind} · {size}")
    }
}

impl RootView {
    /// Height the draft row adds inside the composer surface.
    pub(in crate::views) fn draft_files_band(&self) -> f32 {
        if self.draft_state.files.is_empty() && self.preparing_files == 0 {
            0.
        } else {
            zork_ui::components::attachment_row::FILES_BAND
        }
    }

    pub(in crate::views) fn render_draft_files(
        &mut self,
        width: f32,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        if self.draft_files_band() == 0. {
            return None;
        }
        let session = self.selected_session.clone().unwrap_or_default();
        let files = Arc::new(self.draft_state.files.clone());
        if self.file_ui.message_previews.borrow().missing(&files) {
            self.ensure_message_previews(&files, &session, DRAFT_ROW, cx);
        }
        let mut entries: Vec<_> = files
            .iter()
            .map(|file| component::File {
                id: file.id.clone(),
                name: file.name.clone(),
                meta: meta(file),
                image: self
                    .file_ui
                    .message_previews
                    .borrow_mut()
                    .image(file, &session, DRAFT_ROW),
                state: component::State::Ready,
                removable: true,
            })
            .collect();
        // Core reports reading only as a count of pending batches, without
        // per-file names or progress, so one indeterminate chip stands for it.
        if self.preparing_files > 0 {
            entries.push(component::File {
                id: "reading".into(),
                name: self.locale.text("preparing_files").into(),
                meta: String::new(),
                image: None,
                state: component::State::Reading(self.locale.text("files_reading").into()),
                removable: false,
            });
        }
        let root = cx.entity().downgrade();
        let handler = Rc::new(move |action, window: &mut Window, cx: &mut gpui::App| {
            let _ = root.update(cx, |v, cx| match action {
                component::Action::Open(id) => {
                    if let Some(index) = files.iter().position(|f| f.id == id) {
                        v.open_message_files(files.clone(), index, &session, window, cx);
                    }
                }
                component::Action::Remove(id) => {
                    if v.selected_session.as_deref() == Some(&session) {
                        if let Err(error) = v.core_device.remove_file(&session, &id) {
                            v.error = Some(error.to_string());
                        }
                        zork_ui::components::region::invalidate(cx, &["composer"]);
                    }
                }
                component::Action::Retry(_) => {}
            });
        });
        Some(
            component::render(
                component::Ids {
                    root: "draft-files".into(),
                    file_prefix: "draft-preview-".into(),
                    remove_prefix: "remove-".into(),
                },
                entries,
                width,
                self.locale.text("remove_attachment").into(),
                handler,
            )
            .into_any_element(),
        )
    }

    pub(in crate::views) fn release_file_previews(&mut self, cx: &mut gpui::App) {
        self.file_ui.message_previews.borrow_mut().release(cx);
    }
}
