use super::*;
use std::cell::RefCell;
use zork_client_core::files::FileRef;

#[derive(Default)]
pub(in crate::views) struct PreviewCache {
    entries: HashMap<String, Entry>,
    loading: usize,
    clock: u64,
    waiting_rows: HashMap<String, std::collections::HashSet<(String, usize)>>,
}
struct Entry {
    image: Option<Arc<gpui::RenderImage>>,
    ready: bool,
    used: u64,
}

fn key(file: &FileRef) -> String {
    format!("{}:{}", file.content_root, image::kind(&file.name))
}

impl PreviewCache {
    fn missing(&self, files: &[FileRef]) -> bool {
        self.loading < 2
            && files.iter().any(|f| {
                super::content::kind(&f.name, "").has_thumbnail()
                    && !self.entries.contains_key(&key(f))
            })
    }
    fn image(
        &mut self,
        file: &FileRef,
        session: &str,
        row: usize,
    ) -> Option<Arc<gpui::RenderImage>> {
        self.clock += 1;
        let key = key(file);
        if self.entries.get(&key).is_some_and(|entry| !entry.ready) {
            self.waiting_rows
                .entry(key.clone())
                .or_default()
                .insert((session.to_owned(), row));
        }
        let entry = self.entries.get_mut(&key)?;
        entry.used = self.clock;
        entry.image.clone()
    }
    pub(super) fn release(&mut self, cx: &mut gpui::App) {
        self.waiting_rows.clear();
        for (_, entry) in self.entries.drain() {
            if let Some(image) = entry.image {
                cx.drop_image(image, None);
            }
        }
    }
}

impl RootView {
    fn ensure_message_previews(
        &mut self,
        files: &[FileRef],
        session: &str,
        row: usize,
        cx: &mut Context<Self>,
    ) {
        for file in files
            .iter()
            .filter(|f| super::content::kind(&f.name, "").has_thumbnail())
        {
            let mut cache = self.file_ui.message_previews.borrow_mut();
            if cache
                .entries
                .get(&key(file))
                .is_some_and(|entry| !entry.ready)
            {
                cache
                    .waiting_rows
                    .entry(key(file))
                    .or_default()
                    .insert((session.to_owned(), row));
            }
            if cache.loading >= 2 || cache.entries.contains_key(&key(file)) {
                continue;
            }
            if cache.entries.len() >= 128 {
                let oldest = cache
                    .entries
                    .iter()
                    .filter(|(_, e)| e.ready)
                    .min_by_key(|(_, e)| e.used)
                    .map(|(k, _)| k.clone());
                if let Some(oldest) = oldest {
                    if let Some(image) = cache.entries.remove(&oldest).and_then(|e| e.image) {
                        cx.drop_image(image, None);
                    }
                }
            }
            cache.clock += 1;
            let used = cache.clock;
            cache.loading += 1;
            cache.entries.insert(
                key(file),
                Entry {
                    image: None,
                    ready: false,
                    used,
                },
            );
            cache
                .waiting_rows
                .entry(key(file))
                .or_default()
                .insert((session.to_owned(), row));
            drop(cache);
            let file = file.clone();
            let device = self.core_device.clone();
            let cache = self.file_ui.message_previews.clone();
            let renderer = cx.svg_renderer();
            #[cfg(feature = "headless-bench")]
            let offline = self.benchmark_offline;
            #[cfg(not(feature = "headless-bench"))]
            let offline = false;
            let local = self.local_cache.clone();
            cx.spawn(async move |this, cx| {
                let bytes = if offline {
                    local.and_then(|(store, node)| {
                        store
                            .blob(&node, &format!("upload:{}", file.id))
                            .ok()
                            .flatten()
                    })
                } else {
                    device.artifact_content(&file.id).await.ok()
                };
                let image = if let Some(bytes) = bytes {
                    let name = file.name.clone();
                    cx.background_executor()
                        .spawn(
                            async move { super::content::thumbnail(&name, &bytes, &renderer).ok() },
                        )
                        .await
                } else {
                    None
                };
                let mut cache = cache.borrow_mut();
                cache.loading = cache.loading.saturating_sub(1);
                if let Some(entry) = cache.entries.get_mut(&key(&file)) {
                    entry.image = image;
                    entry.ready = true;
                }
                let waiting_rows = cache.waiting_rows.remove(&key(&file)).unwrap_or_default();
                drop(cache);
                let _ = this.update(cx, |view, cx| {
                    for (session, row) in waiting_rows {
                        if view.selected_session.as_deref() == Some(&session)
                            && row < view.lines.len()
                        {
                            view.transcript_list.remeasure_items(row..row + 1);
                        }
                    }
                    zork_ui::components::region::invalidate(cx, &["transcript"])
                });
            })
            .detach();
        }
    }
}

pub(in crate::views) fn render(
    files: Arc<Vec<FileRef>>,
    session: &str,
    width: f32,
    user: bool,
    root: gpui::WeakEntity<RootView>,
    index: usize,
    cache: Rc<RefCell<PreviewCache>>,
    cx: &mut gpui::App,
) -> Div {
    let group_width = width.clamp(140., 300.);
    if cache.borrow().missing(&files) {
        let (files, root) = (files.clone(), root.clone());
        let session = session.to_owned();
        cx.defer(move |cx| {
            let _ = root.update(cx, |view, cx| {
                view.ensure_message_previews(&files, &session, index, cx)
            });
        });
    }
    let mut rows = Vec::new();
    let mut cursor = 0;
    while cursor < files.len() {
        let first = cursor;
        let is_image = super::content::kind(&files[first].name, "").is_image();
        let columns = if is_image
            && files
                .get(first + 1)
                .is_some_and(|f| super::content::kind(&f.name, "").is_image())
        {
            2
        } else {
            1
        };
        let item_width = (group_width - 12. * (columns - 1) as f32) / columns as f32;
        let mut row = div().flex().gap_3();
        for selected in first..first + columns {
            let file = &files[selected];
            let id = format!("message-file-{index}-{}", file.id);
            let item = if is_image {
                zork_ui::components::attachments::message_image_with_padding(
                    id,
                    cache.borrow_mut().image(file, session, index),
                    item_width,
                    1.,
                )
            } else {
                zork_ui::components::attachments::message_document_with_preview(
                    id,
                    file.name.clone(),
                    image::kind(&file.name),
                    group_width,
                    cache.borrow_mut().image(file, session, index),
                )
            };
            let (root, group, session) = (root.clone(), files.clone(), session.to_owned());
            row = row.child(
                item.on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    let _ = root.update(cx, |v, cx| {
                        v.open_message_files(group.clone(), selected, &session, window, cx)
                    });
                })
                .automation(AutomationRole::Button, file.name.clone()),
            );
        }
        rows.push(row);
        cursor += columns;
    }
    div()
        .w(px(width))
        .mx_auto()
        .flex()
        .pb_2()
        .when(user, |v| v.justify_end())
        .child(
            div()
                .w(px(group_width))
                .flex()
                .flex_col()
                .gap_3()
                .children(rows),
        )
}
