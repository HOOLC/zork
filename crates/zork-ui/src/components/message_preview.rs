//! A bounded, selectable excerpt with an integrated full-message link.
//! Measure the bounded source once per layout, before choosing the footer, so
//! the first painted frame already has its final height.
use gpui::{
    prelude::*, px, size, AnyElement, App, AvailableSpace, Bounds, ContentMask, Element, LayoutId,
    Pixels, Point, Size, Style, Window,
};

pub struct MessagePreview {
    pub body: AnyElement,
    pub footer: AnyElement,
    pub width: f32,
    pub limit: f32,
    pub more: bool,
    pub expanded: bool,
    /// Cue clips a session reply with `overflow: clip` and no gradient; the
    /// chat transcript still paints its clip fade.
    pub fade: bool,
    pub background: gpui::Hsla,
}
impl IntoElement for MessagePreview {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for MessagePreview {
    type RequestLayoutState = (Pixels, bool);
    type PrepaintState = ();
    fn id(&self) -> Option<gpui::ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        _: Option<&gpui::GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        w: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let available = size(
            AvailableSpace::Definite(px(self.width.max(1.))),
            AvailableSpace::MinContent,
        );
        let body = self.body.layout_as_root(available, w, cx);
        let more = self.expanded || self.more || body.height > px(self.limit + 0.5);
        let height = if self.expanded {
            body.height
        } else {
            body.height.min(px(self.limit))
        };
        let footer = if more {
            self.footer.layout_as_root(available, w, cx).height
        } else {
            px(0.)
        };
        let mut style = Style::default();
        style.size = size(px(self.width.max(1.)).into(), (height + footer).into());
        (w.request_layout(style, [], cx), (height, more))
    }
    fn prepaint(
        &mut self,
        _: Option<&gpui::GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        state: &mut Self::RequestLayoutState,
        w: &mut Window,
        cx: &mut App,
    ) {
        w.with_content_mask(
            Some(ContentMask {
                bounds: Bounds {
                    origin: bounds.origin,
                    size: size(bounds.size.width, state.0),
                },
            }),
            |w| {
                self.body.prepaint_at(bounds.origin, w, cx);
            },
        );
        if state.1 {
            self.footer.prepaint_at(
                Point {
                    x: bounds.origin.x,
                    y: bounds.origin.y + state.0,
                },
                w,
                cx,
            );
        }
    }
    fn paint(
        &mut self,
        _: Option<&gpui::GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        state: &mut Self::RequestLayoutState,
        _: &mut (),
        w: &mut Window,
        cx: &mut App,
    ) {
        w.with_content_mask(
            Some(ContentMask {
                bounds: Bounds {
                    origin: bounds.origin,
                    size: Size {
                        width: bounds.size.width,
                        height: state.0,
                    },
                },
            }),
            |w| self.body.paint(w, cx),
        );
        if state.1 {
            if !self.expanded && self.fade {
                let fade_height = px(40.).min(state.0);
                w.paint_quad(gpui::fill(
                    Bounds {
                        origin: Point {
                            x: bounds.origin.x,
                            y: bounds.origin.y + state.0 - fade_height,
                        },
                        size: size(bounds.size.width, fade_height),
                    },
                    gpui::linear_gradient(
                        180.,
                        gpui::linear_color_stop(self.background.opacity(0.), 0.),
                        gpui::linear_color_stop(self.background, 1.),
                    ),
                ));
            }
            self.footer.paint(w, cx);
        }
    }
}
