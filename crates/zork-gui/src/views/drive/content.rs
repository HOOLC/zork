//! Conversation content tabs. Core owns membership, ordering and search results.
use super::*;
use zork_client_core::pages::{ContentIndex, ContentKind};

pub(super) struct ContentTabState {
    view: Entity<zork_ui::conversation_contents::List>,
    query: String,
    source: Option<Arc<Vec<ContentIndex>>>,
    rows: Arc<Vec<ContentIndex>>,
}

pub(in crate::views) fn tab_id(kind: ContentKind) -> &'static str {
    match kind {
        ContentKind::Page => "conversation-content-pages",
        ContentKind::File => "conversation-content-files",
    }
}

pub(super) fn title_key(kind: ContentKind) -> &'static str {
    match kind {
        ContentKind::Page => "content_pages",
        ContentKind::File => "content_files",
    }
}

fn icon(kind: ContentKind) -> &'static str {
    match kind {
        ContentKind::Page => "browser/globe.svg",
        ContentKind::File => "icons/file.svg",
    }
}

impl RootView {
    pub(in crate::views) fn active_content_kind(&self, cx: &gpui::App) -> Option<ContentKind> {
        [ContentKind::Page, ContentKind::File]
            .into_iter()
            .find(|kind| self.browser.read(cx).is_native_page_active(tab_id(*kind)))
    }

    pub(in crate::views) fn close_content_tab(&mut self, id: &str) {
        if let Some(kind) = [ContentKind::Page, ContentKind::File]
            .into_iter()
            .find(|kind| tab_id(*kind) == id)
        {
            self.drive.tabs.remove(&(self.browser_host(), kind));
            self.regions.retain(|key| key != tab_id(kind));
        }
    }

    fn ensure_content_tab(&mut self, kind: ContentKind, cx: &mut Context<Self>) {
        let key = (self.browser_host(), kind);
        if self.drive.tabs.contains_key(&key) {
            return;
        }
        let locale = self.locale;
        let view = cx.new(|cx| zork_ui::conversation_contents::List::new(component_kind(kind),
            zork_ui::resources::Text(Rc::new(move |key| locale.text(key).into())), cx));
        let search_key = key.clone();
        cx.subscribe(&view, move |view, _, event: &zork_ui::conversation_contents::Query, cx| {
            if let Some(tab) = view.drive.tabs.get_mut(&search_key) {
                tab.query = event.0.clone(); tab.source = None;
            }
            zork_ui::components::region::invalidate(cx, &[tab_id(search_key.1)]);
        }).detach();
        self.drive.tabs.insert(
            key,
            ContentTabState {
                view,
                query: String::new(),
                source: None,
                rows: Default::default(),
            },
        );
    }

    pub(in crate::views) fn open_content_tab(&mut self, kind: ContentKind, cx: &mut Context<Self>) {
        if self.selected_session.is_none() {
            return;
        }
        self.ensure_content_tab(kind, cx);
        self.close_conversation_files(cx);
        let host = self.browser_host();
        let title = self.locale.text(title_key(kind)).to_owned();
        self.browser.update(cx, |browser, cx| {
            browser.set_host(host, cx);
            browser.open_native_page(
                crate::browser::NativePage {
                    id: tab_id(kind).into(),
                    title,
                    icon: icon(kind),
                },
                cx,
            );
        });
        zork_ui::components::region::invalidate_all(cx);
    }

    pub(in crate::views) fn render_content_page(
        &mut self,
        kind: ContentKind,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        self.ensure_content_tab(kind, cx);
        let entries = self.conversation_contents().entries(kind).clone();
        let key = (self.browser_host(), kind);
        let tab = self.drive.tabs.get_mut(&key).expect("content tab");
        if tab
            .source
            .as_ref()
            .is_none_or(|old| !Arc::ptr_eq(old, &entries))
        {
            tab.rows = zork_client_core::pages::filter_contents(
                &entries,
                &self.drive.items,
                &self.drive.pages,
                &tab.query,
            );
            tab.source = Some(entries);
        }
        let rows = tab.rows.clone();
        let view = tab.view.clone();
        let rows = self.content_rows(rows, cx);
        let locale = self.locale;
        view.update(cx, |view, cx| view.set_rows(rows, zork_ui::resources::Text(Rc::new(move |key| locale.text(key).into())), cx));
        view
    }

    pub(super) fn content_rows(&self, entries: Arc<Vec<ContentIndex>>, cx: &Context<Self>) -> zork_ui::conversation_contents::Rows {
        let items = self.drive.items.clone(); let pages = self.drive.pages.clone(); let locale = self.locale;
        let owner = cx.entity().downgrade(); let session = self.selected_session.clone();
        let source = self.drive.viewer.source.clone();
        #[cfg(feature = "headless-bench")]
        let indices = self.benchmark_artifact_indices.clone();
        #[cfg(feature = "headless-bench")]
        let count = self.benchmark_artifact_cards.clone();
        zork_ui::conversation_contents::Rows { count: entries.len(), row: Rc::new(move |row, full| {
            let index = entries[row];
            #[cfg(feature = "headless-bench")]
            {
                let key = match index { ContentIndex::File(i) => i, ContentIndex::Page(i) => items.len() + i };
                let mut visible = indices.borrow_mut(); visible.insert(key); count.set(visible.len());
            }
            let owner = owner.clone(); let session = session.clone();
            match index {
                ContentIndex::File(i) => {
                    let artifact = items[i].clone();
                    let detail = if artifact.version > 1 { format!("{} · v{}", file_size(artifact.byte_len), artifact.version) } else { file_size(artifact.byte_len) };
                    let prefix = if full { "content-file" } else { "conversation-artifact" };
                    zork_ui::conversation_contents::Row { id: format!("{prefix}-{}", artifact.artifact_id), icon: "icons/file.svg", title: artifact.name.clone(), detail, source: Some(source.clone()),
                        open: Rc::new(move |cx| { let _ = owner.update(cx, |v, cx| {
                            if v.selected_session == session { v.select_artifact(artifact.clone(), cx); }
                        }); }) }
                }
                ContentIndex::Page(i) => {
                    let reference = &pages.references[i]; let page = reference.page.clone();
                    let detail = if reference.source_session_id.is_some() { locale.text("content_handed_page").into() } else { page.description.clone() };
                    let prefix = if full { "content-page" } else { "conversation-page" };
                    zork_ui::conversation_contents::Row { id: format!("{prefix}-{}", reference.id), icon: "browser/globe.svg", title: page.title.clone(), detail, source: None,
                        open: Rc::new(move |cx| { let _ = owner.update(cx, |v, cx| {
                            if v.selected_session == session { v.open_page(page.clone(), cx); }
                        }); }) }
                }
            }
        }) }
    }
}
fn component_kind(kind: ContentKind) -> zork_ui::conversation_contents::Kind {
    match kind { ContentKind::Page => zork_ui::conversation_contents::Kind::Page, ContentKind::File => zork_ui::conversation_contents::Kind::File }
}
