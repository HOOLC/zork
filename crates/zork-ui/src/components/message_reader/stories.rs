use super::*;
use crate::{
    components::{
        comments::{Editor, EditorRequest},
        workbench as wb,
    },
    controls,
};
pub struct Story {
    reader: Entity<Reader>,
    editor: Entity<Editor>,
    content: Content,
    pending: Option<(CommentSource, Bounds<Pixels>, FocusHandle)>,
}
impl Story {
    pub fn new(state: &str, text: crate::resources::Text, cx: &mut Context<Self>) -> Self {
        let reader = cx.new(Reader::new);
        reader.update(cx, |reader, _| {
            reader.configure(
                text.text("message_full_title").into(),
                text.text("message_copy_full").into(),
                Rc::new(|_, _| {}),
            )
        });
        let editor = cx.new(|cx| Editor::new("reader", cx));
        cx.subscribe(&reader, |v, _, event: &Selected, cx| {
            v.pending = Some((event.source.clone(), event.bounds, event.focus.clone()));
            cx.notify();
        })
        .detach();
        cx.subscribe(
            &editor,
            |_, editor, _: &crate::components::comments::Submit, cx| {
                editor.update(cx, |editor, cx| editor.dismiss(cx))
            },
        )
        .detach();
        let body = if state == "literal" {
            "# 原样消息\n**保留原文符号**".into()
        } else {
            (0..if state == "long" { 80 } else { 4 })
                .map(|i| {
                    format!(
                        "## 第 {} 节\n\n完整消息支持选中文字、复制全文与滚动阅读。\n\n",
                        i + 1
                    )
                })
                .collect::<String>()
        };
        let document = if state == "literal" {
            MessageDocument::plain(&body)
        } else {
            MessageDocument::parse(&body)
        };
        let content = Content::new(
            CommentSource {
                session_id: "demo".into(),
                message_id: Some("message".into()),
                author: Some("产品领队".into()),
                author_agent_id: Some("leader".into()),
                quote: String::new(),
            },
            body,
            document,
        );
        Self {
            reader,
            editor,
            content,
            pending: None,
        }
    }
}
impl Render for Story {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some((source, bounds, focus)) = self.pending.take() {
            self.editor.update(cx, |editor, cx| {
                editor.open_at(
                    EditorRequest {
                        source,
                        editing: None,
                        text: String::new(),
                        toolbar: true,
                    },
                    bounds,
                    Some(focus),
                    window,
                    cx,
                )
            });
        }
        let button = controls::button("message-reader-open", "阅读全文", false, true).on_click(
            cx.listener(|v, _, _, cx| {
                v.reader
                    .update(cx, |reader, cx| reader.open(v.content.clone(), cx))
            }),
        );
        let source = self.reader.read(cx).source();
        wb::column(12.)
            .child(source.bind(button, "阅读全文", controls::ActionStyle::default()).automation(AutomationRole::Button, "阅读全文"))
            .child(self.reader.clone())
            .child(self.editor.clone())
    }
}
