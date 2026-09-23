//! Content-sized, stationary hover surfaces. Interactive hosts own focus and dismissal.
use super::*;

#[derive(Clone, Copy)]
pub enum Side {
    Above,
    AboveEnd(f32),
    Below,
    Beside,
}

pub struct FloatingStyle {
    pub width: f32,
    pub side: Side,
    pub radius: f32,
    pub priority: usize,
    pub role: Role,
    /// Minimum height used to choose a side before the content is laid out.
    pub placement_min_height: f32,
}
impl FloatingStyle {
    pub fn details(width: f32, side: Side) -> Self {
        Self {
            width,
            side,
            radius: crate::controls::CARD_RADIUS,
            priority: 210,
            role: Role::Tooltip,
            placement_min_height: 2.,
        }
    }
}

pub type Hover = Rc<dyn Fn(&bool, &mut Window, &mut App)>;

#[derive(Default)]
pub struct FloatingPanel {
    visible: bool,
}
impl FloatingPanel {
    pub fn alive(&self) -> bool {
        self.visible
    }
    pub fn inspect(&self) -> serde_json::Value {
        serde_json::json!({ "visible": self.visible })
    }
    pub fn render<V: 'static>(
        &mut self,
        id: impl Into<SharedString>,
        anchor: Bounds<Pixels>,
        open: bool,
        style: FloatingStyle,
        content: Content,
        hover: Option<Hover>,
        window: &mut Window,
        _: &mut Context<V>,
    ) -> Option<AnyElement> {
        self.visible = false;
        if !open {
            return None;
        }
        let viewport = window.viewport_size();
        let viewport_bounds = Bounds::new(point(px(0.), px(0.)), viewport);
        let intersection = anchor
            .intersect(&viewport_bounds)
            .intersect(&window.content_mask().bounds);
        if intersection.size.width <= px(0.) || intersection.size.height <= px(0.) {
            return None;
        }
        self.visible = true;

        let id = id.into();
        let width = style.width.min((viewport.width.as_f32() - 24.).max(2.));
        let max_height = (viewport.height.as_f32() - 24.).max(2.);
        let clamp_x = |x: f32| x.clamp(12., (viewport.width.as_f32() - width - 12.).max(12.));
        let (x, y, available, corner) = match style.side {
            Side::Above | Side::AboveEnd(_) | Side::Below => {
                let above = (anchor.top().as_f32() - 20.).max(2.);
                let below = (viewport.height.as_f32() - anchor.bottom().as_f32() - 20.).max(2.);
                let on_top = match style.side {
                    Side::Above | Side::AboveEnd(_) => {
                        above >= style.placement_min_height || above >= below
                    }
                    Side::Below => below < style.placement_min_height && above > below,
                    Side::Beside => unreachable!(),
                };
                (
                    clamp_x(match style.side {
                        Side::AboveEnd(offset) => anchor.right().as_f32() - width + offset,
                        _ => anchor.center().x.as_f32() - width / 2.,
                    }),
                    if on_top {
                        anchor.top().as_f32() - 8.
                    } else {
                        anchor.bottom().as_f32() + 8.
                    },
                    if on_top { above } else { below },
                    if on_top {
                        Anchor::BottomLeft
                    } else {
                        Anchor::TopLeft
                    },
                )
            }
            Side::Beside => {
                let right = anchor.right().as_f32() + 4.;
                let x = if right + width <= viewport.width.as_f32() - 12. {
                    right
                } else {
                    anchor.left().as_f32() - width - 4.
                };
                (
                    clamp_x(x),
                    anchor.top().as_f32(),
                    max_height,
                    Anchor::TopLeft,
                )
            }
        };

        let padding = content.padding;
        let mut body = div()
            .id(format!("{id}-content"))
            .role(style.role)
            .w_full()
            .max_h(px((available.min(max_height) - 2. * padding).max(2.)))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(content.gap))
            .children(content.sections);
        if let Some(hover) = hover {
            body = body.on_hover(move |inside, w, cx| hover(inside, w, cx));
        }
        let body = body.automation(AutomationRole::Status, "内容面板");
        let surface = crate::components::liquid::primitives::surface(
            id,
            style.radius,
            crate::design::ZORK_UI.palette.canvas,
            true,
        )
        .occlude()
        .w(px(width))
        .max_h(px(available.min(max_height)))
        .p(px(padding))
        .flex()
        .flex_col()
        .child(body);
        Some(
            deferred(
                anchored()
                    .position(point(px(x), px(y)))
                    .anchor(corner)
                    .snap_to_window_with_margin(px(12.))
                    .child(surface),
            )
            .with_priority(style.priority)
            .into_any_element(),
        )
    }
}
