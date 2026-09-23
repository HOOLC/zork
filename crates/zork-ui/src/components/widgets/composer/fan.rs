//! Attachment preview ribbon built from ordinary GPUI buttons and images.
use crate::components::attachment_fan as geometry;
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    controls as ui,
};
use gpui::{prelude::*, *};
use std::{rc::Rc, sync::Arc};

pub struct File {
    pub id: String,
    pub name: String,
    pub image: Option<Arc<RenderImage>>,
    pub removable: bool,
}
pub struct Frame {
    pub files: Vec<File>,
    pub width: f32,
    pub height: f32,
    pub expanded: bool,
}
pub struct Ids {
    pub root: String,
    pub toggle: String,
    pub file_prefix: String,
    pub remove_prefix: String,
}
#[derive(Clone)]
pub enum Action {
    Hover(bool),
    Toggle,
    Highlight(String, bool),
    Open(String),
    Remove(String),
}
pub type Handler = Rc<dyn Fn(Action, &mut Window, &mut App)>;

pub fn render(
    ids: Ids,
    frame: Frame,
    label: SharedString,
    remove_label: SharedString,
    handler: Handler,
) -> Stateful<Div> {
    let hover = handler.clone();
    let toggle = handler.clone();
    let expanded = frame.expanded;
    let toggle_id = ids.toggle.clone();
    let mut root = div()
        .id(ids.root)
        .w(px(frame.width))
        .h(px(frame.height))
        .flex()
        .items_center()
        .gap_1()
        .rounded(px(crate::design::RADIUS.block))
        .bg(rgb(crate::design::ZORK_UI.palette.canvas))
        .border(px(crate::design::BORDER_WIDTH))
        .border_color(rgb(crate::design::UI_OUTLINE))
        .overflow_x_scroll()
        .on_hover(move |inside, window, cx| hover(Action::Hover(*inside), window, cx))
        .child(
            div()
                .id(toggle_id.clone())
                .w(px(28.))
                .h_full()
                .flex_shrink_0()
                .child(
                    gpui_base::Button::new(format!("{toggle_id}-button"))
                        .accessibility_label(label.clone())
                        .size_full()
                        .child(ui::icon("icons/file.svg", 14.))
                        .on_click(move |_, window, cx| toggle(Action::Toggle, window, cx)),
                )
                .automation(AutomationRole::Button, label.to_string()),
        );
    for file in frame.files {
        let id = file.id.clone();
        let hover_id = id.clone();
        let on_hover = handler.clone();
        let on_open = handler.clone();
        let mut tile = div()
            .relative()
            .w(px(64.))
            .h(px(64.))
            .flex_shrink_0()
            .rounded(px(crate::design::RADIUS.block - 6.))
            .overflow_hidden()
            .bg(rgb(crate::design::ZORK_UI.palette.sidebar_hover))
            .border(px(crate::design::BORDER_WIDTH))
            .border_color(rgb(crate::design::UI_OUTLINE));
        tile = tile.child(if let Some(image) = file.image {
            img(image).size_full().into_any_element()
        } else {
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(ui::icon("icons/file.svg", 24.))
                .into_any_element()
        });
        if file.removable {
            let remove = handler.clone();
            let remove_id = id.clone();
            let remove_control_id = format!("{}{}", ids.remove_prefix, id);
            tile = tile.child(
                div()
                    .id(remove_control_id.clone())
                    .absolute()
                    .right_0()
                    .top_0()
                    .size(px(geometry::REMOVE_SIZE))
                    .child(
                        gpui_base::Button::new(format!("{remove_control_id}-button"))
                            .accessibility_label(remove_label.clone())
                            .size_full()
                            .rounded(px(geometry::REMOVE_SIZE / 2.))
                            .bg(rgb(crate::design::ZORK_UI.palette.canvas))
                            .child(ui::icon("icons/x.svg", geometry::REMOVE_GLYPH))
                            .on_click(move |_, window, cx| {
                                cx.stop_propagation();
                                remove(Action::Remove(remove_id.clone()), window, cx);
                            }),
                    )
                    .automation(AutomationRole::Button, remove_label.to_string()),
            );
        }
        let item_id = format!("{}{}", ids.file_prefix, id);
        root = root.child(
            div()
                .id(item_id.clone())
                .w(px(64.))
                .h(px(64.))
                .flex_shrink_0()
                .child(
                    gpui_base::Button::new(format!("{item_id}-button"))
                        .accessibility_label(file.name.clone())
                        .size_full()
                        .on_hover(move |inside, window, cx| {
                            on_hover(Action::Highlight(hover_id.clone(), *inside), window, cx)
                        })
                        .on_click(move |_, window, cx| {
                            if expanded {
                                on_open(Action::Open(id.clone()), window, cx);
                            } else {
                                on_open(Action::Toggle, window, cx);
                            }
                        })
                        .child(tile),
                )
                .automation(AutomationRole::Button, file.name),
        );
    }
    root
}
