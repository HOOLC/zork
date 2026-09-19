//! The same selection toolbar and comment editor for the application and fixtures.
//! Draft persistence is supplied by the subscriber; this entity owns presentation.
use super::id;
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    comments::CommentSource,
    components::{
        liquid::{
            controls::{self, ActionStyle},
            primitives::dialog::Flyout,
        },
        text_input::{ComposerEdited, ComposerInput, ComposerSubmit},
    },
    controls as ui,
    design::{TextRole, CUE_UI},
};
use gpui::{prelude::*, *};

#[derive(Clone)]
pub struct EditorRequest {
    pub source: CommentSource,
    pub editing: Option<String>,
    pub text: String,
    pub toolbar: bool,
}

#[derive(Clone)]
pub struct Submit {
    pub source: CommentSource,
    pub editing: Option<String>,
    pub text: String,
}
pub struct Closed;

pub struct Editor {
    prefix: String,
    input: Entity<ComposerInput>,
    flyout: Flyout,
    request: Option<EditorRequest>,
    retired: Option<EditorRequest>,
}

impl Editor {
    pub fn new(prefix: impl Into<String>, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| ComposerInput::new("补充你的看法…", cx));
        cx.subscribe(&input, |_, _, _: &ComposerEdited, cx| cx.notify())
            .detach();
        cx.subscribe(&input, |editor, _, _: &ComposerSubmit, cx| {
            editor.submit(cx)
        })
        .detach();
        Self {
            prefix: prefix.into(),
            input,
            flyout: Flyout::new(cx),
            request: None,
            retired: None,
        }
    }

    pub fn input(&self) -> Entity<ComposerInput> {
        self.input.clone()
    }
    pub fn is_open(&self) -> bool {
        self.flyout.is_open()
    }
    pub fn inspect(&self) -> serde_json::Value {
        self.flyout.inspect()
    }
    pub fn samples(&self) -> Vec<crate::components::liquid::overlay::FrameSample> {
        self.flyout.samples()
    }
    pub fn reset_samples(&self) {
        self.flyout.reset_samples();
    }
    pub fn visible(&self) -> bool {
        self.flyout.visible()
    }
    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.request = None;
        self.retired = None;
        self.flyout = Flyout::new(cx);
        self.input.update(cx, |input, cx| input.clear(cx));
        cx.notify();
    }

    pub fn open_at(
        &mut self,
        request: EditorRequest,
        bounds: Bounds<Pixels>,
        source: Option<FocusHandle>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.input.update(cx, |input, cx| {
            input.reset_value(request.text.clone(), cx);
            input.set_editable(false, false, cx);
        });
        self.flyout.prefer_above();
        self.flyout
            .initial_focus((!request.toolbar).then(|| self.input.read(cx).focus_handle()));
        self.flyout.open_at(
            id(&self.prefix, "comment-source"),
            "选中文字",
            bounds,
            source,
            window,
            cx,
        );
        self.retired = None;
        self.request = Some(request);
        cx.notify();
    }

    pub fn dismiss(&mut self, cx: &mut Context<Self>) {
        if let Some(mut request) = self.request.take() {
            request.text = self.input.read(cx).value().to_owned();
            self.retired = Some(request);
            cx.emit(Closed);
        }
        self.flyout.dismiss();
        self.input
            .update(cx, |input, cx| input.set_editable(true, false, cx));
        cx.notify();
    }

    fn compose(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(request) = &mut self.request {
            request.toolbar = false;
        }
        let focus = self.input.read(cx).focus_handle();
        self.flyout.initial_focus(Some(focus.clone()));
        window.focus(&focus, cx);
        cx.notify();
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        let Some(request) = self.request.as_ref().filter(|_| self.is_open()) else {
            return;
        };
        let text = self.input.read(cx).value().trim().to_owned();
        if !request.toolbar && !text.is_empty() {
            cx.emit(Submit {
                source: request.source.clone(),
                editing: request.editing.clone(),
                text,
            });
        }
    }

    fn toolbar(
        &self,
        interactive: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let action = |suffix: &str,
                      label: &str,
                      icon: &'static str,
                      window: &mut Window,
                      cx: &mut Context<Self>| {
            controls::action(
                id(&self.prefix, suffix),
                label.to_owned(),
                70.,
                32.,
                ActionStyle {
                    quiet: true,
                    icon: Some(icon),
                    disabled: !interactive,
                    ..Default::default()
                },
                CUE_UI.palette.canvas,
                window,
                cx,
            )
        };
        div()
            .id(id(&self.prefix, "selection-toolbar"))
            .flex()
            .items_center()
            .gap(px(2.))
            .child(
                action("selection-copy", "复制", "icons/copy.svg", window, cx)
                    .on_click(cx.listener(move |editor, _, _, cx| {
                        if !interactive {
                            return;
                        }
                        if let Some(request) = &editor.request {
                            cx.write_to_clipboard(ClipboardItem::new_string(
                                request.source.quote.clone(),
                            ));
                        }
                        editor.dismiss(cx);
                    }))
                    .automation_enabled(interactive, AutomationRole::Button, "复制选中文字"),
            )
            .child(
                div()
                    .w(px(crate::design::BORDER_WIDTH))
                    .h(px(12.))
                    .bg(rgb(CUE_UI.palette.border)),
            )
            .child(
                action(
                    "selection-comment",
                    "评论",
                    "icons/message-square.svg",
                    window,
                    cx,
                )
                .on_click(cx.listener(move |editor, _, window, cx| {
                    if interactive {
                        editor.compose(window, cx);
                    }
                }))
                .automation_enabled(
                    interactive,
                    AutomationRole::Button,
                    "评论选中文字",
                ),
            )
            .automation(AutomationRole::Status, "选中文字工具条")
            .into_any_element()
    }

    // Extracted from the liquid gallery's comment panel: header, quote, editor,
    // primary action. There is no fixture-specific content or rendering branch.
    fn body(
        &self,
        request: &EditorRequest,
        interactive: bool,
        width: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let header = div()
            .flex()
            .justify_between()
            .items_center()
            .child(ui::text_role(
                if request.editing.is_some() {
                    "编辑评论"
                } else {
                    "添加评论"
                },
                TextRole::Label,
            ))
            .child(
                controls::action(
                    id(&self.prefix, "comment-popover-close"),
                    "",
                    24.,
                    24.,
                    ActionStyle {
                        quiet: true,
                        icon: Some("icons/x.svg"),
                        disabled: !interactive,
                        ..Default::default()
                    },
                    CUE_UI.palette.canvas,
                    window,
                    cx,
                )
                .on_click(cx.listener(move |editor, _, _, cx| {
                    if interactive {
                        editor.dismiss(cx);
                    }
                }))
                .automation_enabled(
                    interactive,
                    AutomationRole::Button,
                    "关闭评论",
                ),
            );
        let quote = ui::text_role(
            format!("「{}」", request.source.quote),
            TextRole::Description,
        )
        .text_color(rgb(CUE_UI.palette.muted))
        .id(id(&self.prefix, "comment-selected-quote"))
        .automation(AutomationRole::Status, request.source.quote.clone());
        let input = controls::input(
            id(&self.prefix, "comment-input"),
            &self.input,
            width,
            76.,
            false,
            CUE_UI.palette.canvas,
            window,
            cx,
        )
        .automation_enabled(interactive, AutomationRole::TextInput, "评论内容");
        let enabled = interactive && !self.input.read(cx).value().trim().is_empty();
        let save = controls::action(
            id(&self.prefix, "comment-queue-add"),
            if request.editing.is_some() {
                "保存评论"
            } else {
                "添加评论"
            },
            128.,
            32.,
            ActionStyle {
                primary: true,
                disabled: !enabled,
                ..Default::default()
            },
            CUE_UI.palette.canvas,
            window,
            cx,
        )
        .on_click(cx.listener(move |editor, _, _, cx| {
            if enabled {
                editor.submit(cx);
            }
        }))
        .automation_enabled(enabled, AutomationRole::Button, "加入评论队列");
        div()
            .id(id(&self.prefix, "selection-comment-popover"))
            .w_full()
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(header)
            .child(quote)
            .child(input)
            .child(save)
            .into_any_element()
    }
}

impl EventEmitter<Submit> for Editor {}
impl EventEmitter<Closed> for Editor {}
impl Render for Editor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.request.is_some() && !self.flyout.is_open() {
            self.dismiss(cx);
        }
        let Some(request) = self.request.clone().or_else(|| self.retired.clone()) else {
            return div().into_any_element();
        };
        let input = self.input.clone();
        let width = if request.toolbar { 150. } else { 304. };
        let padding = if request.toolbar { 3. } else { 16. };
        // A previous, taller editor must not pin a newly opened toolbar below
        // the selection once its own content has been measured.
        self.flyout.prefer_above();
        let result = self.flyout.render_content(
            id(&self.prefix, "selection-flyout"),
            "选中文字操作",
            |interactive, width, window, cx| {
                input.update(cx, |input, cx| {
                    input.set_editable(!interactive || request.toolbar, false, cx)
                });
                if request.toolbar {
                    self.toolbar(interactive, window, cx)
                } else {
                    self.body(&request, interactive, width, window, cx)
                }
            },
            width,
            padding,
            window,
            cx,
        );
        if !self.flyout.is_open() && !self.flyout.alive() {
            self.retired = None;
        }
        result.unwrap_or_else(|| div().into_any_element())
    }
}
