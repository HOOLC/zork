//! Compact file identity shared by conversations, file lists and component examples.
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    controls as ui,
    design::ZORK_UI,
};
use gpui::{div, prelude::*, px, rgb, Context};

/// Paint the image directly on the preview canvas, with no surrounding frame.
pub fn image_viewport(
    image: std::sync::Arc<gpui::RenderImage>,
    image_size: gpui::Size<gpui::Pixels>,
    content_size: gpui::Size<gpui::Pixels>,
) -> gpui::AnyElement {
    gpui::canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let viewport = bounds.intersect(&window.content_mask().bounds);
            let image_bounds = gpui::Bounds::new(
                bounds.center() - gpui::point(image_size.width / 2., image_size.height / 2.),
                image_size,
            );
            let _ = window.paint_image(
                viewport,
                image_bounds,
                gpui::Corners::default(),
                image.clone(),
                0,
                false,
            );
        },
    )
    .w(content_size.width)
    .h(content_size.height)
    .into_any_element()
}

/// Show ordinary images at their natural aspect ratio, long edge at most
/// [`IMAGE_LONG_EDGE`]. Only extreme panoramas and long captures are cropped;
/// portrait images shrink in width to stay compact.
pub fn message_image(
    id: impl Into<gpui::ElementId>,
    image: Option<std::sync::Arc<gpui::RenderImage>>,
    width: f32,
) -> ui::Action {
    message_image_with_padding(id, image, width, 0.)
}

/// `padding` is the duplicated source-pixel gutter around the visible image.
/// It protects linear atlas sampling without adding a visible border.
pub fn message_image_with_padding(
    id: impl Into<gpui::ElementId>,
    image: Option<std::sync::Arc<gpui::RenderImage>>,
    width: f32,
    padding: f32,
) -> ui::Action {
    let p = ZORK_UI.palette;
    let ratio = image
        .as_ref()
        .map(|image| {
            let size = image.size(0);
            (i32::from(size.width) as f32 - padding * 2.).max(1.)
                / (i32::from(size.height) as f32 - padding * 2.).max(1.)
        })
        .unwrap_or(1.)
        .clamp(1. / 3., 3.);
    let height = (width.min(IMAGE_LONG_EDGE) / ratio).min(IMAGE_LONG_EDGE);
    let width = height * ratio;
    ui::quiet_button(id, "", true, ui::IconButtonSize::Standard)
        .radius(crate::design::RADIUS.block)
        .p_0()
        .w(px(width))
        .h(px(height))
        .flex_shrink_0()
        // Only the loading placeholder has a background. Painting a second
        // rounded surface under the image leaves a grey antialiased rim.
        .when(image.is_none(), |v| v.child(div().size_full().bg(rgb(p.sidebar))))
        .when_some(image, |v, image| {
            v.child(
                gpui::canvas(
                    |_, _, _| (),
                    move |bounds, _, window, _| {
                        let source = image.size(0);
                        let source_width = (i32::from(source.width) as f32 - padding * 2.).max(1.);
                        let source_height = (i32::from(source.height) as f32 - padding * 2.).max(1.);
                        let scale = (bounds.size.width.as_f32() / source_width)
                            .max(bounds.size.height.as_f32() / source_height);
                        let size = gpui::size(
                            px((source_width + padding * 2.) * scale),
                            px((source_height + padding * 2.) * scale),
                        );
                        let image_bounds = gpui::Bounds::new(
                            bounds.center() - gpui::point(size.width / 2., size.height / 2.), size,
                        );
                        let _ = window.paint_image(bounds, image_bounds,
                            gpui::Corners::default(), image.clone(), 0, false);
                    },
                ).size_full(),
            )
        })
}

/// Longest edge of an image shown inline in a message.
pub const IMAGE_LONG_EDGE: f32 = 320.;
/// Gap between images that share a message grid.
pub const IMAGE_GRID_GAP: f32 = 4.;

/// One image in a multi-image grid: covers its cell, cropping the long side.
/// `corners` rounds only the cells on the grid's outer edge, so the grid reads
/// as one block with the block radius. `padding` is the duplicated source
/// gutter, as in [`message_image_with_padding`].
pub fn message_image_tile(
    id: impl Into<gpui::ElementId>,
    image: Option<std::sync::Arc<gpui::RenderImage>>,
    width: f32,
    height: f32,
    corners: gpui::Corners<gpui::Pixels>,
    padding: f32,
) -> gpui::Stateful<gpui::Div> {
    let p = ZORK_UI.palette;
    div()
        .id(id)
        .w(px(width))
        .h(px(height))
        .flex_shrink_0()
        .cursor_pointer()
        .when(image.is_none(), |v| {
            v.bg(rgb(p.sidebar))
                .rounded_tl(corners.top_left)
                .rounded_tr(corners.top_right)
                .rounded_bl(corners.bottom_left)
                .rounded_br(corners.bottom_right)
        })
        .when_some(image, |v, image| {
            v.child(
                gpui::canvas(
                    |_, _, _| (),
                    move |bounds, _, window, _| {
                        let source = image.size(0);
                        let source_width = (i32::from(source.width) as f32 - padding * 2.).max(1.);
                        let source_height =
                            (i32::from(source.height) as f32 - padding * 2.).max(1.);
                        let scale = (bounds.size.width.as_f32() / source_width)
                            .max(bounds.size.height.as_f32() / source_height);
                        let size = gpui::size(
                            px((source_width + padding * 2.) * scale),
                            px((source_height + padding * 2.) * scale),
                        );
                        let image_bounds = gpui::Bounds::new(
                            bounds.center() - gpui::point(size.width / 2., size.height / 2.),
                            size,
                        );
                        let _ = window.paint_image(bounds, image_bounds, corners, image.clone(), 0, false);
                    },
                )
                .size_full(),
            )
        })
}

/// Short type badge for a file block: the extension, "MD" for Markdown.
pub fn file_badge(name: &str) -> String {
    let ext = std::path::Path::new(name)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_ascii_uppercase();
    match ext.as_str() {
        "MARKDOWN" => "MD".into(),
        "" => "FILE".into(),
        _ => ext.chars().take(4).collect(),
    }
}

/// Non-image attachments as a file block: type badge, name and "type · size".
/// The whole block opens the preview; there is no second expand step.
pub fn message_file_block(
    id: impl Into<gpui::ElementId>,
    name: String,
    meta: String,
    width: f32,
) -> ui::Action {
    let p = ZORK_UI.palette;
    let badge = file_badge(&name);
    let document = badge == "PDF";
    let block = crate::design::RADIUS.block;
    let inset = 10.;
    ui::button(id, "", false, true)
        .radius(block)
        .font_weight(gpui::FontWeight::NORMAL)
        .w(px(width))
        .max_w_full()
        .h(px(40. + 2. * inset))
        .p(px(inset))
        .flex()
        .items_center()
        .justify_start()
        .gap_3()
        .child(
            div()
                .size(px(40.))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                // A step below the block radius, so the badge reads as a
                // tile rather than a pill inside the 10 px inset.
                .rounded(px(block - 6.))
                .bg(if document {
                    gpui::rgba((p.danger << 8) | 0x1F)
                } else {
                    rgb(p.prompt).into()
                })
                .text_color(rgb(if document { p.danger } else { p.muted }))
                .text_size(px(12.))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .child(badge),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(
                    div()
                        .text_size(px(13.))
                        .line_height(px(18.))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(rgb(p.text))
                        .truncate()
                        .child(name),
                )
                .child(ui::text_role(meta, crate::design::TextRole::Metadata).truncate()),
        )
}

/// Kept for component examples: a file block labelled with its kind.
pub fn message_document(
    id: impl Into<gpui::ElementId>,
    name: String,
    kind: String,
    width: f32,
) -> ui::Action {
    message_file_block(id, name, kind, width)
}

/// A compact file row for an on-demand list, without the attachment-chip outline.
pub fn row<V: 'static>(
    id: impl Into<gpui::ElementId>,
    name: String,
    meta: String,
    cx: &Context<V>,
    open: impl Fn(&mut V, &mut Context<V>) + 'static,
) -> gpui::AnyElement {
    let label = format!("{name}, {meta}");
    let p = ZORK_UI.palette;
    ui::quiet_button(id, "", true, ui::IconButtonSize::Standard)
        .radius(ui::FIELD_RADIUS)
        .font_weight(gpui::FontWeight::NORMAL)
        .w_full()
        .min_w_0()
        .h(px(48.))
        .px(px(10.))
        .flex()
        .items_center()
        .gap(px(10.))
        .child(ui::icon("icons/file.svg", 18.))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(
                    div()
                        .text_size(px(12.))
                        .line_height(px(18.))
                        .truncate()
                        .child(name),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .line_height(px(14.))
                        .text_color(rgb(p.muted))
                        .child(meta),
                ),
        )
        .on_click(cx.listener(move |v, _, _, cx| open(v, cx)))
        .automation(AutomationRole::Button, label)
        .into_any_element()
}

/// A compact content-menu row shared by conversation files and delivered pages.
pub fn content_row<V: 'static>(
    id: impl Into<gpui::ElementId>,
    icon_path: &'static str,
    name: String,
    meta: String,
    cx: &Context<V>,
    open: impl Fn(&mut V, &mut Context<V>) + 'static,
) -> impl IntoElement {
    content_row_control(id, icon_path, name, meta, true, cx, open)
}

pub fn content_row_enabled<V: 'static>(
    id: impl Into<gpui::ElementId>,
    icon_path: &'static str,
    name: String,
    meta: String,
    enabled: bool,
    cx: &Context<V>,
    open: impl Fn(&mut V, &mut Context<V>) + 'static,
) -> impl IntoElement {
    content_row_control(id, icon_path, name, meta, enabled, cx, open)
}

fn content_row_control<V: 'static>(
    id: impl Into<gpui::ElementId>,
    icon_path: &'static str,
    name: String,
    meta: String,
    enabled: bool,
    cx: &Context<V>,
    open: impl Fn(&mut V, &mut Context<V>) + 'static,
) -> impl IntoElement {
    ui::quiet_button(id, "", enabled, ui::IconButtonSize::Standard)
        .w_full()
        .h(px(48.))
        .radius(ui::FIELD_RADIUS)
        .px(px(12.))
        .gap(px(10.))
        .justify_start()
        .child(ui::icon(icon_path, 18.))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(ui::text_role(name.clone(), crate::design::TextRole::Body).truncate())
                .child(ui::text_role(meta, crate::design::TextRole::Metadata).truncate()),
        )
        .on_click(cx.listener(move |view, _, _, cx| {
            if enabled {
                open(view, cx);
            }
        }))
        .automation_enabled(enabled, AutomationRole::Button, name)
}

pub fn card<V: 'static>(
    id: impl Into<gpui::ElementId>,
    name: String,
    meta: String,
    enabled: bool,
    cx: &Context<V>,
    open: impl Fn(&mut V, &mut Context<V>) + 'static,
) -> gpui::AnyElement {
    let label = format!("查看附件 {name}");
    let p = ZORK_UI.palette;
    ui::button(id, "", false, enabled)
        .radius(ui::COMPACT_CARD_RADIUS)
        .font_weight(gpui::FontWeight::NORMAL)
        .w(px(236.))
        .max_w_full()
        .h(px(48.))
        .px(px(12.))
        .flex()
        .items_center()
        .gap(px(10.))
        .child(ui::icon("icons/file.svg", 18.))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(
                    div()
                        .text_size(px(12.))
                        .line_height(px(18.))
                        .truncate()
                        .child(name),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .line_height(px(14.))
                        .text_color(rgb(p.muted))
                        .truncate()
                        .child(meta),
                ),
        )
        .on_click(cx.listener(move |v, _, _, cx| {
            if enabled {
                open(v, cx);
            }
        }))
        .automation_enabled(enabled, AutomationRole::Button, label)
        .into_any_element()
}
