//! Draft attachments as a row of capsules inside the composer surface.
//! Order follows the host (core) order; one click opens the file, the
//! always-visible × removes it. The row scrolls horizontally when it overflows.
use crate::components::attachment_row as geometry;
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    controls as ui,
    design::{TextRole, INTERACTION, ZORK_UI},
};
use gpui::{prelude::*, *};
use std::{rc::Rc, sync::Arc};

pub enum State {
    Ready,
    /// Bytes are still being read; the label says how far, when known.
    Reading(SharedString),
    /// Reading failed; the label carries the reason and the chip offers retry.
    Failed(SharedString),
}
pub struct File {
    pub id: String,
    pub name: String,
    /// "Type · size", shown under the name.
    pub meta: String,
    pub image: Option<Arc<RenderImage>>,
    pub state: State,
    pub removable: bool,
}
pub struct Ids {
    pub root: String,
    pub file_prefix: String,
    pub remove_prefix: String,
}
#[derive(Clone)]
pub enum Action {
    Open(String),
    Remove(String),
    Retry(String),
}
pub type Handler = Rc<dyn Fn(Action, &mut Window, &mut App)>;

fn soft(color: u32, alpha: u32) -> Rgba {
    rgba((color << 8) | alpha)
}

fn thumbnail(file: &File, id: &str) -> Div {
    let p = ZORK_UI.palette;
    let slot = div()
        .relative()
        .size(px(geometry::THUMB_SIZE))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(geometry::THUMB_RADIUS))
        .bg(rgb(p.canvas))
        .text_color(rgb(p.muted));
    match (&file.state, &file.image) {
        (State::Reading(_), _) => slot.child(crate::components::loading::indicator(
            SharedString::from(format!("{id}-reading")),
            16.,
        )),
        (State::Failed(_), _) => slot
            .text_color(rgb(p.danger))
            .child(ui::icon("icons/attention.svg", 16.)),
        (State::Ready, Some(image)) => {
            let image = image.clone();
            slot.child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, _| {
                        // Cover the slot, cropping the long side, with the
                        // slot's own corners so no square edge shows.
                        let source = image.size(0);
                        let (w, h) = (
                            (i32::from(source.width) as f32).max(1.),
                            (i32::from(source.height) as f32).max(1.),
                        );
                        let scale = (bounds.size.width.as_f32() / w)
                            .max(bounds.size.height.as_f32() / h);
                        let size = gpui::size(px(w * scale), px(h * scale));
                        let image_bounds = Bounds::new(
                            bounds.center() - point(size.width / 2., size.height / 2.),
                            size,
                        );
                        let _ = window.paint_image(
                            bounds,
                            image_bounds,
                            Corners::all(px(geometry::THUMB_RADIUS)),
                            image.clone(),
                            0,
                            false,
                        );
                    },
                )
                .size_full(),
            )
        }
        (State::Ready, None) => slot.child(ui::icon("icons/file.svg", 16.)),
    }
}

fn chip(file: File, ids: &Ids, remove_label: &SharedString, handler: &Handler) -> AnyElement {
    let p = ZORK_UI.palette;
    let failed = matches!(file.state, State::Failed(_));
    let item_id = format!("{}{}", ids.file_prefix, file.id);
    let meta: SharedString = match &file.state {
        State::Ready => file.meta.clone().into(),
        State::Reading(label) | State::Failed(label) => label.clone(),
    };
    let open = handler.clone();
    let open_id = file.id.clone();
    let mut row = div()
        .id(item_id.clone())
        .h(px(geometry::CHIP_HEIGHT))
        .max_w(px(geometry::CHIP_MAX_WIDTH))
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap_2()
        .p(px(geometry::CHIP_INSET))
        .rounded(px(geometry::CHIP_HEIGHT / 2.))
        .bg(if failed {
            soft(p.danger, 0x1F)
        } else {
            rgb(p.prompt).into()
        })
        .when(!failed, |v| {
            v.hover(|s| s.bg(rgb(INTERACTION.neutral_hover)))
        })
        .child(thumbnail(&file, &item_id))
        .child(
            div()
                .min_w_0()
                .flex_1()
                .flex()
                .flex_col()
                .child(
                    div()
                        .text_size(px(12.5))
                        .line_height(px(16.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgb(p.text))
                        .truncate()
                        .child(file.name.clone()),
                )
                .child(
                    ui::text_role(meta, TextRole::Metadata)
                        .text_color(rgb(if failed { p.danger } else { p.subtle }))
                        .truncate(),
                ),
        )
        .on_click(move |_, window, cx| {
            if !failed {
                open(Action::Open(open_id.clone()), window, cx);
            }
        });
    if failed {
        let retry = handler.clone();
        let retry_id = file.id.clone();
        let control = format!("{item_id}-retry");
        row = row.child(
            ui::icon_button_sized(control.clone(), true, ui::IconButtonSize::Compact)
                .text_color(rgb(p.danger))
                .child(ui::icon("icons/reload.svg", geometry::REMOVE_GLYPH))
                .on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    retry(Action::Retry(retry_id.clone()), window, cx);
                })
                .automation(AutomationRole::Button, "重试"),
        );
    }
    if file.removable {
        let remove = handler.clone();
        let remove_id = file.id.clone();
        let control = format!("{}{}", ids.remove_prefix, file.id);
        row = row.child(
            ui::icon_button_sized(control, true, ui::IconButtonSize::Compact)
                .child(ui::icon("icons/x.svg", geometry::REMOVE_GLYPH))
                .on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    remove(Action::Remove(remove_id.clone()), window, cx);
                })
                .automation(AutomationRole::Button, remove_label.to_string()),
        );
    }
    row.automation(AutomationRole::Button, file.name).into_any_element()
}

/// `width` is the space inside the surface; the right edge fades when the
/// chips need more than that and the row scrolls horizontally.
pub fn render(
    ids: Ids,
    files: Vec<File>,
    width: f32,
    remove_label: SharedString,
    handler: Handler,
) -> Div {
    let overflow = geometry::row_width(files.iter().map(|f| (f.name.as_str(), f.meta.as_str())))
        > width;
    let chips: Vec<_> = files
        .into_iter()
        .take(geometry::MAX_FILES)
        .map(|file| chip(file, &ids, &remove_label, &handler))
        .collect();
    div()
        .relative()
        .w(px(width))
        .h(px(geometry::CHIP_HEIGHT))
        .child(
            div()
                .id(ids.root)
                .size_full()
                .flex()
                .items_center()
                .gap_2()
                .overflow_x_scroll()
                .children(chips),
        )
        .when(overflow, |v| {
            let surface = Hsla::from(rgb(crate::components::composer_layout::SURFACE_COLOR()));
            v.child(
                div()
                    .absolute()
                    .top_0()
                    .right_0()
                    .w(px(32.))
                    .h_full()
                    .bg(linear_gradient(
                        90.,
                        linear_color_stop(surface.opacity(0.), 0.),
                        linear_color_stop(surface, 1.),
                    )),
            )
        })
}
