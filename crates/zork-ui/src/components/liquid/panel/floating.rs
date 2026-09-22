//! Measured floating content with one material across anchors. Interactive hosts own focus and dismissal.
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
}
impl FloatingStyle {
    pub fn details(width: f32, side: Side) -> Self {
        Self {
            width,
            side,
            radius: crate::controls::CARD_RADIUS,
            priority: 210,
            role: Role::Tooltip,
        }
    }
}

pub type Hover = Rc<dyn Fn(&bool, &mut Window, &mut App)>;

#[derive(Default)]
pub struct FloatingPanel {
    panel: ContentPanel,
    visible: bool,
}
impl FloatingPanel {
    pub fn alive(&self) -> bool {
        self.panel.alive()
    }
    pub fn inspect(&self) -> serde_json::Value {
        self.panel.inspect()
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
        cx: &mut Context<V>,
    ) -> Option<AnyElement> {
        if !open && !self.alive() {
            return None;
        }
        let id = id.into();
        let viewport = window.viewport_size();
        let width = style.width.min((viewport.width.as_f32() - 24.).max(2.));
        let max_height = (viewport.height.as_f32() - 24.).max(2.);
        let height = self.panel.content_height().unwrap_or(2.).min(max_height);
        let clamp_x = |x: f32| x.clamp(12., (viewport.width.as_f32() - width - 12.).max(12.));
        let clamp_y = |y: f32| y.clamp(12., (viewport.height.as_f32() - height - 12.).max(12.));
        let (x, y, available) = match style.side {
            Side::Above | Side::AboveEnd(_) | Side::Below => {
                let above = (anchor.top().as_f32() - 20.).max(2.);
                let below = (viewport.height.as_f32() - anchor.bottom().as_f32() - 20.).max(2.);
                let on_top = match style.side {
                    Side::Above | Side::AboveEnd(_) => above >= height || above >= below,
                    _ => below < height && above > below,
                };
                (
                    clamp_x(match style.side {
                        Side::AboveEnd(offset) => anchor.right().as_f32() - width + offset,
                        _ => anchor.center().x.as_f32() - width / 2.,
                    }),
                    if on_top {
                        (anchor.top().as_f32() - height.min(above) - 8.).max(12.)
                    } else {
                        anchor.bottom().as_f32() + 8.
                    },
                    if on_top { above } else { below },
                )
            }
            Side::Beside => {
                let right = anchor.right().as_f32() + 4.;
                let x = if right + width <= viewport.width.as_f32() - 12. {
                    right
                } else {
                    anchor.left().as_f32() - width - 4.
                };
                (clamp_x(x), clamp_y(anchor.top().as_f32()), max_height)
            }
        };
        let padding = content.padding;
        let mut body = div()
            .id(format!("{id}-body"))
            .role(style.role)
            .w(px((width - 2. * padding).max(2.)))
            .max_h(px((available.min(max_height) - 2. * padding).max(2.)))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(content.gap))
            .children(content.sections)
            .capture_any_mouse_down(move |_, _, cx| {
                if !open {
                    cx.stop_propagation();
                }
            });
        if let Some(hover) = hover {
            body = body.on_hover(move |inside, w, cx| hover(inside, w, cx));
        }
        let viewport_bounds = Bounds::new(point(px(0.), px(0.)), viewport);
        let intersection = anchor
            .intersect(&viewport_bounds)
            .intersect(&window.content_mask().bounds);
        self.visible = intersection.size.width > px(0.) && intersection.size.height > px(0.);
        if !self.visible {
            self.panel = ContentPanel::default();
            return None;
        }
        // ContentPanel owns natural height, contour clips, retargeting and retirement.
        let panel = self.panel.render_with_visibility(
            id,
            viewport.width.as_f32(),
            Placement {
                x,
                y,
                width,
                radius: style.radius,
            },
            Content {
                sections: vec![body.into_any_element()],
                padding,
                gap: 0.,
            },
            None,
            SurfaceColors::outlined(
                crate::design::LIQUID_OUTLINE,
                crate::design::ZORK_UI.palette.canvas,
            ),
            Material::ordinary(),
            Some(open && self.visible),
            window,
            cx,
        );
        Some(
            deferred(anchored().position(point(px(0.), px(0.))).child(panel))
                .with_priority(style.priority)
                .into_any_element(),
        )
    }
}
