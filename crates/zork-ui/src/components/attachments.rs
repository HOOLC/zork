//! Compact file identity shared by conversations, file lists and component examples.
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    controls as ui,
    design::CUE_UI,
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

/// Show ordinary images at their natural aspect ratio. Only extreme panoramas
/// and long captures are cropped; portrait images shrink in width to stay compact.
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
    let p = CUE_UI.palette;
    let ratio = image
        .as_ref()
        .map(|image| {
            let size = image.size(0);
            (i32::from(size.width) as f32 - padding * 2.).max(1.)
                / (i32::from(size.height) as f32 - padding * 2.).max(1.)
        })
        .unwrap_or(1.)
        .clamp(1. / 3., 3.);
    let height = (width / ratio).min(300.);
    let width = height * ratio;
    ui::quiet_button(id, "", true, ui::IconButtonSize::Standard)
        .radius(ui::CARD_RADIUS)
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

/// Non-image attachments use the same quiet 48 px file identity as other
/// message controls, with an explicit whole-row opening affordance.
pub fn message_document(
    id: impl Into<gpui::ElementId>,
    name: String,
    kind: String,
    width: f32,
) -> ui::Action {
    message_document_with_preview(id, name, kind, width, None)
}

pub fn message_document_with_preview(
    id: impl Into<gpui::ElementId>,
    name: String,
    kind: String,
    width: f32,
    preview: Option<std::sync::Arc<gpui::RenderImage>>,
) -> ui::Action {
    ui::button(id, "", false, true)
        .radius(ui::COMPACT_CARD_RADIUS)
        .font_weight(gpui::FontWeight::NORMAL)
        .w(px(width))
        .max_w_full()
        .h(px(if preview.is_some() { 72. } else { 48. }))
        .px_3()
        .flex()
        .items_center()
        .gap_3()

        .when(preview.is_none(), |v| {
            v.child(ui::icon("icons/file.svg", 20.))
        })
        .when_some(preview, |v, image| {
            let source = image.size(0);
            let ratio = (i32::from(source.width) - 2).max(1) as f32
                / (i32::from(source.height) - 2).max(1) as f32;
            let height = 52.;
            let width = (height * ratio).min(44.);
            v.child(
                div()
                    .w(px(44.))
                    .h(px(52.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        gpui::canvas(
                            |_, _, _| (),
                            move |bounds, _, window, _| {
                                let gutter =
                                    bounds.size.width / (i32::from(source.width) - 2).max(1) as f32;
                                let image_bounds = gpui::Bounds::new(
                                    bounds.origin - gpui::point(gutter, gutter),
                                    bounds.size + gpui::size(gutter * 2., gutter * 2.),
                                );
                                let _ = window.paint_image(
                                    bounds,
                                    image_bounds,
                                    gpui::Corners::all(px(3.)),
                                    image.clone(),
                                    0,
                                    false,
                                );
                            },
                        )
                        .w(px(width))
                        .h(px(width / ratio)),
                    ),
            )
        })
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(ui::text_role(name.clone(), crate::design::TextRole::Label).truncate())
                .child(ui::text_role(kind, crate::design::TextRole::Metadata).truncate()),
        )
        .child(ui::icon("icons/arrow-right.svg", 14.))
}

/// A compact file row for an on-demand list, without the attachment-chip outline.
pub fn row<V: 'static>(
    id: impl Into<gpui::ElementId>,
    name: String,
    meta: String,
    cx: &Context<V>,
    open: impl Fn(&mut V, &mut Context<V>) + 'static,
) -> impl IntoElement {
    row_source(id, name, meta, None, cx, open)
}

pub fn row_source<V: 'static>(
    id: impl Into<gpui::ElementId>,
    name: String,
    meta: String,
    source: Option<crate::components::liquid::overlay::SourceBinding>,
    cx: &Context<V>,
    open: impl Fn(&mut V, &mut Context<V>) + 'static,
) -> gpui::AnyElement {
    let label = format!("{name}, {meta}");
    let p = CUE_UI.palette;
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
                        .text_size(px(10.))
                        .line_height(px(14.))
                        .text_color(rgb(p.muted))
                        .child(meta),
                ),
        )
        .on_click(cx.listener(move |v, _, _, cx| open(v, cx)))
        .map(|control| match source {
            Some(source) => source.bind(control, label.clone(), ui::ActionStyle { quiet: true, ..Default::default() }).automation(AutomationRole::Button, label).into_any_element(),
            None => control.automation(AutomationRole::Button, label).into_any_element(),
        })
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
    content_row_source(id, icon_path, name, meta, None, cx, open)
}

pub fn content_row_source<V: 'static>(
    id: impl Into<gpui::ElementId>,
    icon_path: &'static str,
    name: String,
    meta: String,
    source: Option<crate::components::liquid::overlay::SourceBinding>,
    cx: &Context<V>,
    open: impl Fn(&mut V, &mut Context<V>) + 'static,
) -> impl IntoElement {
    content_row_control(id, icon_path, name, meta, source, true, cx, open)
}

pub fn content_row_enabled<V: 'static>(
    id: impl Into<gpui::ElementId>, icon_path: &'static str, name: String, meta: String,
    enabled: bool, cx: &Context<V>, open: impl Fn(&mut V, &mut Context<V>) + 'static,
) -> impl IntoElement {
    content_row_control(id, icon_path, name, meta, None, enabled, cx, open)
}

fn content_row_control<V: 'static>(
    id: impl Into<gpui::ElementId>, icon_path: &'static str, name: String, meta: String,
    source: Option<crate::components::liquid::overlay::SourceBinding>, enabled: bool,
    cx: &Context<V>, open: impl Fn(&mut V, &mut Context<V>) + 'static,
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
        .on_click(cx.listener(move |view, _, _, cx| { if enabled { open(view, cx); } }))
        .map(|row| match source {
            Some(source) => source.bind(row, name.clone(), ui::ActionStyle { quiet: true, icon: Some(icon_path), ..Default::default() }).automation_enabled(enabled, AutomationRole::Button, name).into_any_element(),
            None => row.automation_enabled(enabled, AutomationRole::Button, name).into_any_element(),
        })
}

pub fn card<V: 'static>(
    id: impl Into<gpui::ElementId>,
    name: String,
    meta: String,
    enabled: bool,
    cx: &Context<V>,
    open: impl Fn(&mut V, &mut Context<V>) + 'static,
) -> impl IntoElement {
    card_source(id, name, meta, enabled, None, cx, open)
}

pub fn card_source<V: 'static>(
    id: impl Into<gpui::ElementId>,
    name: String,
    meta: String,
    enabled: bool,
    source: Option<crate::components::liquid::overlay::SourceBinding>,
    cx: &Context<V>,
    open: impl Fn(&mut V, &mut Context<V>) + 'static,
) -> gpui::AnyElement {
    let label = format!("查看附件 {name}");
    let p = CUE_UI.palette;
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
                        .text_size(px(10.))
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
        .map(|control| match source {
            Some(source) => source.bind(control, label.clone(), ui::ActionStyle { disabled: !enabled, ..Default::default() }).automation_enabled(enabled, AutomationRole::Button, label).into_any_element(),
            None => control.automation_enabled(enabled, AutomationRole::Button, label).into_any_element(),
        })
}
