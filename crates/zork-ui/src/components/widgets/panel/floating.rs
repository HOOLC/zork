//! Floating content positioned by gpui-base.
use super::*;
use gpui_base::{Align, Placement as PopupPlacement, Positioner};

#[derive(Clone, Copy)]
pub enum Side {
    Above,
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
    visible: bool,
}
impl FloatingPanel {
    pub fn alive(&self) -> bool {
        self.visible
    }
    pub fn inspect(&self) -> serde_json::Value {
        serde_json::json!({"visible":self.visible,"moving":false})
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
        self.visible = open;
        if !open {
            return None;
        }
        let viewport = window.viewport_size();
        let visible = anchor
            .intersect(&Bounds::new(point(px(0.), px(0.)), viewport))
            .intersect(&window.content_mask().bounds);
        if visible.size.width <= px(0.) || visible.size.height <= px(0.) {
            self.visible = false;
            return None;
        }
        let id = id.into();
        let width = style.width.min((viewport.width.as_f32() - 24.).max(2.));
        let (placement, align) = match style.side {
            Side::Above => (PopupPlacement::Top, Align::Center),
            Side::Below => (PopupPlacement::Bottom, Align::Center),
            Side::Beside => (PopupPlacement::Right, Align::Start),
        };
        let mut body = div()
            .id(format!("{id}-body"))
            .role(style.role)
            .w(px(width))
            .max_h(px((viewport.height.as_f32() - 24.).max(2.)))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(content.gap))
            .p(px(content.padding))
            .rounded(px(style.radius))
            .bg(rgb(crate::design::ZORK_UI.palette.canvas))
            .border(px(crate::design::BORDER_WIDTH))
            .border_color(rgb(crate::design::UI_OUTLINE))
            .occlude()
            .children(content.sections);
        if let Some(hover) = hover {
            body = body.on_hover(move |inside, w, cx| hover(inside, w, cx));
        }
        Some(
            deferred(
                Positioner::side(anchor)
                    .placement(placement)
                    .align(align)
                    .offset(px(8.))
                    .margin(px(12.))
                    .child(body),
            )
            .with_priority(style.priority)
            .into_any_element(),
        )
    }
}
