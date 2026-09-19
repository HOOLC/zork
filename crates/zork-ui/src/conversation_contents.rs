//! Conversation file/page list. Core supplies membership and search results.
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    components::text_input::{ComposerEdited, ComposerInput},
    controls as ui,
    design::{TextRole, ZORK_UI},
    resources::Text,
};
use gpui::{prelude::*, *};
use std::rc::Rc;
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Page,
    File,
}
impl Kind {
    pub fn tab_id(self) -> &'static str {
        match self {
            Self::Page => "conversation-content-pages",
            Self::File => "conversation-content-files",
        }
    }
    pub fn title_key(self) -> &'static str {
        match self {
            Self::Page => "content_pages",
            Self::File => "content_files",
        }
    }
    pub fn all_id(self) -> &'static str {
        match self {
            Self::Page => "conversation-pages-all",
            Self::File => "conversation-files-all",
        }
    }
    fn empty_key(self) -> &'static str {
        match self {
            Self::Page => "content_pages_empty",
            Self::File => "content_files_empty",
        }
    }
    fn search_id(self) -> &'static str {
        match self {
            Self::Page => "content-search-pages",
            Self::File => "content-search-files",
        }
    }
    fn list_id(self) -> &'static str {
        match self {
            Self::Page => "conversation-pages-list",
            Self::File => "conversation-artifacts",
        }
    }
}
pub struct Row {
    pub id: String,
    pub icon: &'static str,
    pub title: String,
    pub detail: String,
    pub source: Option<crate::components::liquid::overlay::SourceBinding>,
    pub open: Rc<dyn Fn(&mut App)>,
}
impl Row {
    pub fn render<V: 'static>(self, cx: &mut Context<V>) -> AnyElement {
        self.render_enabled(true, cx)
    }
    pub fn render_enabled<V: 'static>(self, enabled: bool, cx: &mut Context<V>) -> AnyElement {
        if enabled && self.source.is_some() {
            return crate::components::attachments::content_row_source(
                self.id,
                self.icon,
                self.title,
                self.detail,
                self.source,
                cx,
                move |_, cx| (self.open)(cx),
            )
            .into_any_element();
        }
        crate::components::attachments::content_row_enabled(
            self.id,
            self.icon,
            self.title,
            self.detail,
            enabled,
            cx,
            move |_, cx| (self.open)(cx),
        )
        .into_any_element()
    }
}
#[derive(Clone)]
pub struct Rows {
    pub count: usize,
    pub row: Rc<dyn Fn(usize, bool) -> Row>,
}
impl Default for Rows {
    fn default() -> Self {
        Self {
            count: 0,
            row: Rc::new(|_, _| unreachable!("empty row provider")),
        }
    }
}
pub struct Query(pub String);
pub struct List {
    kind: Kind,
    rows: Rows,
    text: Text,
    input: Entity<ComposerInput>,
    scroll: UniformListScrollHandle,
}
impl EventEmitter<Query> for List {}
impl List {
    pub fn new(kind: Kind, text: Text, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| ComposerInput::new(text.text("content_search"), cx).single_line());
        cx.subscribe(&input, |v, input, _: &ComposerEdited, cx| {
            v.scroll.scroll_to_item(0, ScrollStrategy::Top);
            cx.emit(Query(input.read(cx).value().to_owned()));
            cx.notify();
        })
        .detach();
        Self {
            kind,
            rows: Default::default(),
            text,
            input,
            scroll: Default::default(),
        }
    }
    pub fn set_rows(&mut self, rows: Rows, text: Text, cx: &mut Context<Self>) {
        self.rows = rows;
        self.text = text;
        self.input.update(cx, |input, cx| {
            input.set_placeholder(self.text.text("content_search"), cx)
        });
        cx.notify();
    }
}
impl Render for List {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let kind = self.kind;
        let count = self.rows.count;
        let rows = self.rows.clone();
        let title = self.text.text(kind.title_key());
        let empty_query = self.input.read(cx).value().trim().is_empty();
        div()
            .id(kind.tab_id())
            .size_full()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(rgb(ZORK_UI.palette.canvas))
            .child(
                div()
                    .p_4()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(ui::text_role(
                        format!("{title} · {count}"),
                        TextRole::SectionTitle,
                    ))
                    .child(
                        ui::input_control(kind.search_id(), &self.input, false, cx).automation(
                            AutomationRole::TextInput,
                            self.text.text("content_search"),
                        ),
                    ),
            )
            .child(if count == 0 {
                div()
                    .px_4()
                    .py_6()
                    .child(ui::text_role(
                        self.text.text(if empty_query {
                            kind.empty_key()
                        } else {
                            "content_search_empty"
                        }),
                        TextRole::Description,
                    ))
                    .into_any_element()
            } else {
                uniform_list(
                    kind.list_id(),
                    count,
                    cx.processor(move |_, range: std::ops::Range<usize>, _, cx| {
                        range
                            .map(|row| {
                                div()
                                    .h(px(52.))
                                    .px_2()
                                    .pb_1()
                                    .child((rows.row)(row, true).render(cx))
                            })
                            .collect()
                    }),
                )
                .track_scroll(&self.scroll)
                .flex_1()
                .min_h_0()
                .w_full()
                .automation(AutomationRole::ScrollArea, title.clone())
                .into_any_element()
            })
            .automation(AutomationRole::Status, title)
    }
}

#[derive(Clone)]
struct Groups {
    pages: Rows,
    files: Rows,
}
pub struct OpenChanged(pub bool);
pub struct All(pub Kind);
pub struct Menu {
    data: Groups,
    retired: Option<Groups>,
    text: Text,
    width: f32,
    flyout: crate::components::liquid::primitives::dialog::Flyout,
    focus: FocusHandle,
    was_open: bool,
}
impl EventEmitter<OpenChanged> for Menu {}
impl EventEmitter<All> for Menu {}
impl Menu {
    pub fn new(text: Text, cx: &mut Context<Self>) -> Self {
        Self {
            data: Groups {
                pages: Default::default(),
                files: Default::default(),
            },
            retired: None,
            text,
            width: 384.,
            flyout: crate::components::liquid::primitives::dialog::Flyout::new(cx),
            focus: cx.focus_handle().tab_stop(true),
            was_open: false,
        }
    }
    pub fn configure(
        &mut self,
        pages: Rows,
        files: Rows,
        text: Text,
        width: f32,
        cx: &mut Context<Self>,
    ) {
        let changed_trigger = (self.data.pages.count + self.data.files.count == 0)
            != (pages.count + files.count == 0)
            || self.width != width
            || self.text.text("conversation_contents") != text.text("conversation_contents");
        self.data = Groups { pages, files };
        self.text = text;
        self.width = width;
        if changed_trigger || self.flyout.is_open() || self.flyout.alive() {
            cx.notify();
        }
    }
    pub fn close(&mut self, cx: &mut Context<Self>) {
        if !self.flyout.is_open() && !self.was_open {
            return;
        }
        self.retired = Some(self.data.clone());
        self.flyout.dismiss();
        self.was_open = false;
        cx.emit(OpenChanged(false));
        cx.notify();
    }
    fn panel(
        &self,
        data: &Groups,
        interactive: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let preview_rows = ((window.viewport_size().height.as_f32() - 212.) / 104.)
            .floor()
            .clamp(1., 3.) as usize;
        let groups =
            [(Kind::Page, &data.pages), (Kind::File, &data.files)]
                .into_iter()
                .map(|(kind, rows)| {
                    let title = self.text.text(kind.title_key());
                    div()
                        .id(format!("{}-preview", kind.tab_id()))
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .h(px(28.))
                                .pl_3()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .text_size(px(12.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child(format!("{title} · {}", rows.count)),
                                )
                                .child(
                                    ui::quiet_button(
                                        kind.all_id(),
                                        self.text.text("content_view_all"),
                                        interactive,
                                        ui::IconButtonSize::Compact,
                                    )
                                    .text_size(px(11.))
                                    .on_click(cx.listener(move |v, _, _, cx| {
                                        if interactive {
                                            v.close(cx);
                                            cx.emit(All(kind));
                                        }
                                    }))
                                    .automation_enabled(
                                        interactive,
                                        AutomationRole::Button,
                                        format!("{} {title}", self.text.text("content_view_all")),
                                    ),
                                ),
                        )
                        .when(rows.count == 0, |v| {
                            v.child(div().h(px(36.)).px_3().flex().items_center().child(
                                ui::text_role(
                                    self.text.text(kind.empty_key()),
                                    TextRole::Description,
                                ),
                            ))
                        })
                        .children((0..rows.count.min(preview_rows)).map(|index| {
                            div()
                                .h(px(52.))
                                .pb_1()
                                .child((rows.row)(index, false).render_enabled(interactive, cx))
                        }))
                        .automation(AutomationRole::Status, title)
                        .into_any_element()
                })
                .collect::<Vec<_>>();
        let title = self.text.text("conversation_contents");
        div()
            .id("conversation-files-panel")
            .w_full()
            .min_w_0()
            .occlude()
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(40.))
                    .pl(px(8.))
                    .pr(px(3.))
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(px(14.))
                    .line_height(px(22.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(title.clone())
                    .child(div().flex_1())
                    .child(
                        ui::icon_button_sized(
                            "conversation-files-close",
                            interactive,
                            ui::IconButtonSize::Compact,
                        )
                        .child(ui::icon("icons/x.svg", 14.))
                        .on_click(cx.listener(move |v, _, _, cx| {
                            if interactive {
                                v.close(cx);
                            }
                        }))
                        .automation_enabled(
                            interactive,
                            AutomationRole::Button,
                            "关闭会话内容",
                        ),
                    ),
            )
            .children(groups)
            .automation(AutomationRole::Status, title)
            .into_any_element()
    }
}
impl Render for Menu {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.was_open && !self.flyout.is_open() {
            self.close(cx);
        }
        let label = self.text.text("conversation_contents");
        let button = if self.data.pages.count + self.data.files.count > 0 {
            self.flyout
                .trigger_element(
                    ui::quiet_button(
                        "conversation-files-button",
                        "",
                        true,
                        ui::IconButtonSize::Compact,
                    )
                    .text_size(px(11.))
                    .text_color(rgb(ZORK_UI.palette.muted))
                    .child(ui::icon("icons/file.svg", 14.))
                    .child(label.clone()),
                    label.clone(),
                    &self.focus,
                    true,
                    cx,
                )
                .on_click(cx.listener(|v, _, _, cx| {
                    v.was_open = v.flyout.is_open();
                    v.retired = Some(v.data.clone());
                    cx.emit(OpenChanged(v.was_open));
                    cx.notify();
                }))
                .automation(AutomationRole::Button, label.clone())
                .into_any_element()
        } else {
            div().into_any_element()
        };
        let data = if self.flyout.is_open() {
            Some(self.data.clone())
        } else {
            self.retired.clone()
        };
        self.flyout.align_end();
        let panel = data.and_then(|data| {
            self.flyout.render_content(
                "conversation-files-flyout",
                label,
                |interactive, _, window, cx| self.panel(&data, interactive, window, cx),
                self.width,
                12.,
                window,
                cx,
            )
        });
        if !self.flyout.is_open() && !self.flyout.alive() {
            self.retired = None;
        }
        div()
            .child(button)
            .when_some(panel, |v, panel| v.child(panel))
    }
}

#[cfg(feature = "stories")]
pub mod stories;
