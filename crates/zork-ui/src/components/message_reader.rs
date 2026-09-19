//! Full-message reading dialog with shared selection, copy and focus lifecycle.
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    comments::CommentSource,
    components::{
        message::{render_selectable_document, MessageDocument},
        selection::{SelectionContext, TranscriptSelection},
    },
    design::ZORK_UI,
    modal::ModalState,
};
use gpui::{prelude::*, *};
use std::{cell::RefCell, rc::Rc};
#[derive(Clone)]
pub struct Content {
    pub source: CommentSource,
    text: String,
    sections: Rc<Vec<MessageDocument>>,
    plain: SharedString,
    offsets: Rc<Vec<usize>>,
    scroll: ListState,
    selection: Rc<RefCell<TranscriptSelection>>,
}
impl Content {
    pub fn new(source: CommentSource, text: String, document: MessageDocument) -> Self {
        let sections = document.reader_sections();
        let plain = document.shared_plain_text();
        let mut cursor = 0;
        let offsets = sections
            .iter()
            .map(|section| {
                let text = section.shared_plain_text();
                let offset = plain
                    .get(cursor..)
                    .and_then(|tail| tail.find(text.as_ref()))
                    .map(|at| cursor + at)
                    .unwrap_or(cursor);
                cursor = (offset + text.len()).min(plain.len());
                offset
            })
            .collect();
        Self {
            source,
            text,
            scroll: ListState::new(sections.len(), ListAlignment::Top, px(300.)),
            sections: Rc::new(sections),
            plain,
            offsets: Rc::new(offsets),
            selection: Default::default(),
        }
    }
}
pub struct Selected {
    pub source: CommentSource,
    pub bounds: Bounds<Pixels>,
    pub focus: FocusHandle,
}
pub struct Closed;
pub struct Reader {
    content: Option<Content>,
    modal: ModalState,
    title: SharedString,
    copy: SharedString,
    links: Rc<dyn Fn(&str, &mut App)>,
}
impl EventEmitter<Selected> for Reader {}
impl EventEmitter<Closed> for Reader {}
impl Reader {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let mut modal = ModalState::new(cx);
        modal.retain::<Content>("message-reader-dialog", None, cx);
        Self {
            content: None,
            modal,
            title: "完整消息".into(),
            copy: "复制全文".into(),
            links: Rc::new(|url, cx| cx.open_url(url)),
        }
    }
    pub fn configure(
        &mut self,
        title: SharedString,
        copy: SharedString,
        links: Rc<dyn Fn(&str, &mut App)>,
    ) {
        self.title = title;
        self.copy = copy;
        self.links = links;
    }
    pub fn open(&mut self, content: Content, cx: &mut Context<Self>) {
        self.content = Some(content);
        cx.notify();
    }
    pub fn is_open(&self) -> bool {
        self.content.is_some()
    }
    pub fn source(&self) -> crate::components::liquid::overlay::SourceBinding {
        self.modal.source("message-reader-dialog")
    }
    pub fn dismiss(&mut self, cx: &mut Context<Self>) {
        if self.content.take().is_some() {
            cx.emit(Closed);
            cx.notify();
        }
    }
    pub fn clear_selection(&mut self, cx: &mut Context<Self>) {
        if let Some(content) = &self.content {
            content.selection.borrow_mut().clear();
            cx.notify();
        }
    }
    fn finish_selection(&mut self, event: &MouseUpEvent, cx: &mut Context<Self>) {
        let Some(content) = &self.content else {
            return;
        };
        let mut selection = content.selection.borrow_mut();
        if !selection.dragging {
            return;
        }
        let bounds = selection
            .selected_bounds()
            .unwrap_or_else(|| Bounds::new(event.position, size(px(1.), px(1.))));
        if let Some(source) = selection.finish() {
            cx.emit(Selected {
                source,
                bounds,
                focus: self.modal.focus.clone(),
            });
        }
        cx.notify();
    }
    fn body(&self, reader: &Content, cx: &mut Context<Self>) -> AnyElement {
        reader.selection.borrow_mut().begin_frame();
        let sections = reader.sections.clone();
        let selection = reader.selection.clone();
        let plain = reader.plain.clone();
        let offsets = reader.offsets.clone();
        let source = reader.source.clone();
        let focus = self.modal.focus.clone();
        let owner = cx.entity().downgrade();
        let notify: Rc<dyn Fn(&mut App)> = Rc::new(move |cx| {
            let _ = owner.update(cx, |_, cx| cx.notify());
        });
        let links = self.links.clone();
        let list = gpui::list(reader.scroll.clone(), move |index, _, _| {
            let context = SelectionContext::new(
                "message-reader-selection".into(),
                source.clone(),
                plain.clone(),
                selection.clone(),
                focus.clone(),
                notify.clone(),
            )
            .with_link_handler(links.clone())
            .with_offset(offsets[index]);
            div()
                .px_5()
                .py_2()
                .text_size(px(13.))
                .line_height(px(20.))
                .child(render_selectable_document(
                    &format!("message-reader-{index}"),
                    &sections[index],
                    &context,
                ))
                .into_any_element()
        });
        let text = reader.text.clone();
        div()
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(rgb(ZORK_UI.palette.canvas))
            .child(
                div()
                    .px_5()
                    .py_2()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(ZORK_UI.palette.muted))
                            .child(reader.source.author.clone().unwrap_or_default()),
                    )
                    .child(
                        crate::controls::quiet_button(
                            "message-copy-full",
                            self.copy.clone(),
                            self.is_open(),
                            crate::controls::IconButtonSize::Standard,
                        )
                        .on_click(cx.listener(move |v, _, _, cx| {
                            if v.is_open() {
                                cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
                            }
                        }))
                        .automation(AutomationRole::Button, self.copy.clone()),
                    ),
            )
            .child(list.flex_1().min_h_0().py_3())
            .into_any_element()
    }
}
impl Render for Reader {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.modal.sync(
            self.content.as_ref().map(|_| "message-reader-dialog"),
            window,
            cx,
        );
        let shown = self
            .modal
            .retain("message-reader-dialog", self.content.clone(), cx);
        let Some(shown) = shown else {
            return div().into_any_element();
        };
        let body = div()
            .h(px(
                (window.viewport_size().height.as_f32() - 180.).clamp(120., 660.)
            ))
            .on_mouse_move(cx.listener(|v, event: &MouseMoveEvent, _, cx| {
                if v.content
                    .as_ref()
                    .is_some_and(|content| content.selection.borrow_mut().update(event.position))
                {
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|v, event, _, cx| v.finish_selection(event, cx)),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|v, event, _, cx| v.finish_selection(event, cx)),
            )
            .child(self.body(&shown, cx));
        crate::modal::detail_modal(
            "message-reader-dialog",
            self.title.clone(),
            body,
            None,
            &self.modal,
            window,
            cx,
            true,
            |v, _, cx| v.dismiss(cx),
        )
    }
}

#[cfg(feature = "stories")]
pub mod stories;
