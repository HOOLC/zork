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
    pub(super) fn missing(&self, files: &[FileRef]) -> bool {
        self.loading < 2
            && files.iter().any(|f| {
                super::content::kind(&f.name, "").has_thumbnail()
                    && !self.entries.contains_key(&key(f))
            })
    }
    pub(super) fn image(
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
    pub(in crate::views) fn image_count(&self) -> usize {
        self.entries.values().filter(|e| e.image.is_some()).count()
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
    pub(super) fn ensure_message_previews(
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
                    zork_ui::components::region::invalidate(cx, &["transcript", "composer"])
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
    use zork_ui::components::attachments as ui_files;
    let group_width = width.clamp(140., ui_files::IMAGE_LONG_EDGE);
    if cache.borrow().missing(&files) {
        let (files, root) = (files.clone(), root.clone());
        let session = session.to_owned();
        cx.defer(move |cx| {
            let _ = root.update(cx, |view, cx| {
                view.ensure_message_previews(&files, &session, index, cx)
            });
        });
    }
    let is_image = |f: &FileRef| super::content::kind(&f.name, "").is_image();
    let opener = |selected: usize| {
        let (root, group, session) = (root.clone(), files.clone(), session.to_owned());
        move |_: &gpui::ClickEvent, window: &mut Window, cx: &mut gpui::App| {
            cx.stop_propagation();
            let _ = root.update(cx, |v, cx| {
                v.open_message_files(group.clone(), selected, &session, window, cx)
            });
        }
    };
    let mut blocks: Vec<gpui::AnyElement> = Vec::new();
    let mut cursor = 0;
    while cursor < files.len() {
        let first = cursor;
        if !is_image(&files[first]) {
            let file = &files[first];
            blocks.push(
                ui_files::message_file_block(
                    format!("message-file-{index}-{}", file.id),
                    file.name.clone(),
                    super::draft::meta(file),
                    group_width,
                )
                .on_click(opener(first))
                .automation(AutomationRole::Button, file.name.clone())
                .into_any_element(),
            );
            cursor += 1;
            continue;
        }
        // Consecutive images form one grid: two columns with a 4 px gap,
        // rounded only on the grid's outer corners.
        let mut end = first;
        while end < files.len() && is_image(&files[end]) {
            end += 1;
        }
        cursor = end;
        if end - first == 1 {
            let file = &files[first];
            blocks.push(
                ui_files::message_image_with_padding(
                    format!("message-file-{index}-{}", file.id),
                    cache.borrow_mut().image(file, session, index),
                    group_width,
                    1.,
                )
                .on_click(opener(first))
                .automation(AutomationRole::Button, file.name.clone())
                .into_any_element(),
            );
            continue;
        }
        let gap = ui_files::IMAGE_GRID_GAP;
        let radius = px(zork_ui::design::RADIUS.block);
        let cell = (group_width - gap) / 2.;
        let rows = (end - first).div_ceil(2);
        let mut grid = div().flex().flex_col().gap(px(gap));
        for row in 0..rows {
            let top = row == 0;
            let bottom = row + 1 == rows;
            let mut line = div().flex().gap(px(gap));
            let members: Vec<usize> = (first + row * 2..(first + row * 2 + 2).min(end)).collect();
            let alone = members.len() == 1;
            for (column, selected) in members.into_iter().enumerate() {
                let file = &files[selected];
                let left = column == 0;
                let right = alone || column == 1;
                let corners = gpui::Corners {
                    top_left: if top && left { radius } else { px(0.) },
                    top_right: if top && right { radius } else { px(0.) },
                    bottom_left: if bottom && left { radius } else { px(0.) },
                    bottom_right: if bottom && right { radius } else { px(0.) },
                };
                let (w, h) = if alone {
                    (group_width, group_width * 0.5625)
                } else {
                    (cell, cell * 0.75)
                };
                line = line.child(
                    ui_files::message_image_tile(
                        format!("message-file-{index}-{}", file.id),
                        cache.borrow_mut().image(file, session, index),
                        w,
                        h,
                        corners,
                        1.,
                    )
                    .on_click(opener(selected))
                    .automation(AutomationRole::Button, file.name.clone()),
                );
            }
            grid = grid.child(line);
        }
        blocks.push(grid.into_any_element());
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
                .when(user, |v| v.items_end())
                .gap_2()
                .children(blocks),
        )
}
