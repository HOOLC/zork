//! Complete attachment fan. Hosts supply immutable preview images and sampled
//! presentation poses; content loading, file authority and mutations stay in core.
use crate::components::attachment_fan::{self as geometry, Opening};
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    controls as ui,
};
use gpui::{prelude::*, *};
use std::{rc::Rc, sync::Arc};

pub struct File {
    pub id: String,
    pub name: String,
    pub pose: geometry::Pose,
    pub image: Option<Arc<RenderImage>>,
    pub visible: bool,
    pub departing: bool,
    pub active: bool,
    pub removable: bool,
}
pub struct Frame {
    pub files: Vec<File>,
    pub width: f32,
    pub height: f32,
    pub opening: Opening,
    pub expanded: f32,
    pub rim: bool,
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
    render_with_source(ids, frame, label, remove_label, handler, None)
}

/// Bind the actual thumbnail control and its pixels to a consuming preview.
pub fn render_with_source(
    ids: Ids,
    frame: Frame,
    label: SharedString,
    remove_label: SharedString,
    handler: Handler,
    source: Option<super::super::overlay::SourceBinding>,
) -> Stateful<Div> {
    let width = frame.width;
    let height = frame.height;
    let opening = frame.opening;
    let below = geometry::clip_below(&opening, frame.expanded);
    let hover = handler.clone();
    let toggle = handler.clone();
    let mut files = frame.files;
    files.sort_by_key(|file| frame.expanded > 0.98 && file.active);
    div()
        .id(ids.root)
        .relative()
        .w(px(width))
        .h(px(height + below))
        .overflow_hidden()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_hover(move |inside, window, cx| hover(Action::Hover(*inside), window, cx))
        .on_click(move |_, window, cx| {
            cx.stop_propagation();
            toggle(Action::Toggle, window, cx);
        })
        .child(
            div()
                .id(ids.toggle)
                .absolute()
                .top_0()
                .w_full()
                .h(px(12.))
                .automation(AutomationRole::Button, label),
        )
        .children(files.into_iter().filter(|file| file.visible).map(|file| {
            let pose = file.pose;
            let extent = pose.half_extent();
            let highlight = handler.clone();
            let highlight_id = file.id.clone();
            let activate = handler.clone();
            let activate_id = file.id.clone();
            let keyboard = handler.clone();
            let keyboard_id = file.id.clone();
            let departing = file.departing;
            let mut hit = div()
                .id(format!("{}{}", ids.file_prefix, file.id))
                .absolute()
                .left(px(width * 0.5 + pose.center.x - extent.x))
                .top(px(height + pose.center.y - extent.y))
                .w(px(extent.x * 2.))
                .h(px(extent.y * 2.))
                .when(!departing, |v| {
                    v.cursor_pointer().focusable().tab_stop(true)
                })
                .on_hover(move |hovered, w, cx| {
                    highlight(Action::Highlight(highlight_id.clone(), *hovered), w, cx)
                })
                .on_click(move |_, w, cx| {
                    cx.stop_propagation();
                    if !departing {
                        activate(Action::Open(activate_id.clone()), w, cx);
                    }
                })
                .on_key_down(move |event, w, cx| {
                    if !departing && matches!(event.keystroke.key.as_str(), "enter" | "space") {
                        keyboard(Action::Open(keyboard_id.clone()), w, cx);
                        cx.stop_propagation();
                    }
                });
            hit = hit.child(
                clipped_image(
                    file.image,
                    pose,
                    opening,
                    0.,
                    height,
                    pose.width / (216. / 384.),
                    pose.height / (304. / 384.),
                )
                .absolute()
                .left(px(-(width * 0.5 + pose.center.x - extent.x)))
                .top(px(-(height + pose.center.y - extent.y)))
                .w(px(width))
                .h(px(height + below)),
            );
            if file.removable && !departing {
                let remove = handler.clone();
                let id = file.id.clone();
                hit = hit.child(
                    ui::button(format!("{}{}", ids.remove_prefix, file.id), "", false, true)
                        .absolute()
                        .right(px(-geometry::REMOVE_SIZE * 0.5))
                        .top(px(-geometry::REMOVE_SIZE * 0.5))
                        .size(px(geometry::REMOVE_SIZE))
                        .radius(geometry::REMOVE_SIZE * 0.5)
                        .p_0()
                        .child(ui::icon("icons/x.svg", geometry::REMOVE_GLYPH))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(move |_, w, cx| {
                            cx.stop_propagation();
                            remove(Action::Remove(id.clone()), w, cx);
                        })
                        .automation(AutomationRole::Button, remove_label.clone()),
                );
            }
            match source.as_ref().filter(|_| !departing) {
                Some(source) => source
                    .bind(
                        hit,
                        file.name.clone(),
                        ui::ActionStyle {
                            quiet: true,
                            ..Default::default()
                        },
                    )
                    .automation_enabled(true, AutomationRole::Button, file.name)
                    .into_any_element(),
                None => hit
                    .automation_enabled(!departing, AutomationRole::Button, file.name)
                    .into_any_element(),
            }
        }))
        .when(frame.rim, |fan| {
            fan.child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, _| {
                        let rim = opening.translated(gpui::point(
                            bounds.center().x.as_f32(),
                            bounds.top().as_f32() + height,
                        ));
                        let lower = &rim.hole[2..6];
                        let point = |p: gpui::Point<f32>| gpui::point(px(p.x), px(p.y));
                        let mut path = PathBuilder::stroke(px(
                            crate::components::liquid_composer::SLOT_BORDER_WIDTH,
                        ));
                        path.move_to(point(lower[0][0]));
                        for curve in lower {
                            path.cubic_bezier_to(point(curve[3]), point(curve[1]), point(curve[2]));
                        }
                        if let Ok(path) = path.build() {
                            window.paint_path(
                                path,
                                rgb(crate::components::liquid_composer::BORDER_COLOR),
                            );
                        }
                    },
                )
                .absolute()
                .size_full(),
            )
        })
}

// Clip only image pixels. Painting an opaque front patch here would erase
// the lower half of the aperture border even where there is no paper.
fn clipped_image(
    image: Option<Arc<gpui::RenderImage>>,
    pose: geometry::Pose,
    opening: Opening,
    release: f32,
    fan_height: f32,
    image_width: f32,
    image_height: f32,
) -> impl IntoElement + gpui::Styled {
    gpui::canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            // Derive both the image and clipping rim from one stable layout
            // origin. Nested rounded layout bounds jump at the final upright pose.
            let origin = gpui::point(
                bounds.center().x.as_f32(),
                bounds.top().as_f32() + fan_height,
            );
            let bounds = gpui::Bounds::new(
                gpui::point(
                    px(origin.x + pose.center.x - image_width * 0.5),
                    px(origin.y + pose.center.y - image_height * 0.5),
                ),
                gpui::size(px(image_width), px(image_height)),
            );
            let origin = gpui::point(origin.x, origin.y + release);
            let opening = opening.translated(origin);
            let lower = &opening.hole[2..6];
            let edge = |x: f32| {
                if x >= lower[0][0].x {
                    return lower[0][0].y;
                }
                if x <= lower[3][3].x {
                    return lower[3][3].y;
                }
                for curve in lower {
                    if x <= curve[0].x && x >= curve[3].x {
                        let (mut lo, mut hi) = (0., 1.);
                        for _ in 0..12 {
                            let mid = (lo + hi) * 0.5;
                            if geometry::sample(*curve, mid).x > x {
                                lo = mid;
                            } else {
                                hi = mid;
                            }
                        }
                        return geometry::sample(*curve, (lo + hi) * 0.5).y;
                    }
                }
                lower[0][0].y
            };
            let scale = window.scale_factor();
            let min_y = lower.iter().flatten().map(|p| p.y).fold(f32::MAX, f32::min);
            let max_y = lower.iter().flatten().map(|p| p.y).fold(f32::MIN, f32::max);
            if bounds.top().as_f32() >= max_y {
                return;
            }
            let paint = |window: &mut Window| {
                // The rotated thumbnail has transparent padding. Exclude its
                // outer texel so linear atlas sampling cannot pick up a
                // neighbouring tile along the quad's edge.
                window.with_content_mask(
                    Some(gpui::ContentMask {
                        bounds: bounds.inset(px(1.)),
                    }),
                    |window| {
                        if let Some(image) = &image {
                            let _ = window.paint_image(
                                bounds,
                                bounds,
                                gpui::Corners::default(),
                                image.clone(),
                                0,
                                false,
                            );
                        } else {
                            let (sin, cos) = pose.angle.to_radians().sin_cos();
                            let points = [(-0.5, -0.5), (0.5, -0.5), (0.5, 0.5), (-0.5, 0.5)].map(
                                |(x, y)| {
                                    let (x, y) = (x * pose.width, y * pose.height);
                                    gpui::point(
                                        bounds.center().x + px(x * cos - y * sin),
                                        bounds.center().y + px(x * sin + y * cos),
                                    )
                                },
                            );
                            let mut path = gpui::PathBuilder::fill();
                            path.move_to(points[0]);
                            for point in &points[1..] {
                                path.line_to(*point);
                            }
                            path.close();
                            if let Ok(path) = path.build() {
                                window.paint_path(path, rgb(0xFFFFFF));
                            }
                        }
                    },
                );
            };
            if bounds.bottom().as_f32() <= min_y {
                paint(window);
                return;
            }
            let top = (min_y * scale).floor() / scale;
            if top > bounds.top().as_f32() {
                window.with_content_mask(
                    Some(gpui::ContentMask {
                        bounds: gpui::Bounds::new(
                            bounds.origin,
                            gpui::size(bounds.size.width, px(top) - bounds.top()),
                        ),
                    }),
                    |window| paint(window),
                );
            }
            let band_top = top.max(bounds.top().as_f32());
            let first = (bounds.left().as_f32() * scale).floor() as i32;
            let last = (bounds.right().as_f32() * scale).ceil() as i32;
            let cutoff = |column: i32| {
                (edge((column as f32 + 0.5) / scale).min(bounds.bottom().as_f32()) * scale * 8.)
                    .round()
                    / (scale * 8.)
            };
            let mut start = first;
            let mut y = cutoff(first);
            for column in first + 1..=last {
                let next = if column == last {
                    f32::NAN
                } else {
                    cutoff(column)
                };
                if next != y {
                    if y > band_top {
                        window.with_content_mask(
                            Some(gpui::ContentMask {
                                bounds: gpui::Bounds::new(
                                    gpui::point(px(start as f32 / scale), px(band_top)),
                                    gpui::size(
                                        px((column - start) as f32 / scale),
                                        px(y - band_top),
                                    ),
                                ),
                            }),
                            |window| paint(window),
                        );
                    }
                    start = column;
                    y = next;
                }
            }
        },
    )
}
