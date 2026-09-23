//! A bounded, selectable excerpt with an integrated full-message link.
//! Measure the bounded source once per layout, before choosing the footer, so
//! the first painted frame already has its final height.
use gpui::{
    prelude::*, px, size, AnyElement, App, AvailableSpace, Bounds, ContentMask, Element, LayoutId,
    Pixels, Point, Size, Style, Window,
};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

/// The document's actual shaped lines determine a five-line excerpt. The first
/// layout uses the same 110px provisional bound as the history page's ResizeObserver path.
#[derive(Clone)]
pub struct LinePreview {
    pub height: Rc<Cell<f32>>,
    pub notify: Rc<dyn Fn(&mut App)>,
}
thread_local! { static LINES: RefCell<Option<Vec<(f32, f32)>>> = const { RefCell::new(None) }; }
pub(super) fn record_lines(layout: &gpui::TextLayout) {
    LINES.with(|lines| {
        let mut lines = lines.borrow_mut();
        let Some(lines) = lines.as_mut() else {
            return;
        };
        if lines.len() >= 128 || layout.len() == 0 {
            return;
        }
        let mut top = layout.bounds().top().as_f32();
        let height = layout.line_height().as_f32();
        for line in layout.line_layouts() {
            let ink = (line.ascent() + line.descent()).as_f32();
            for _ in 0..=line.wrap_boundaries().len() {
                lines.push((top + (height - ink) / 2., top + (height + ink) / 2.));
                top += height;
                if lines.len() >= 128 {
                    return;
                }
            }
        }
    });
}

pub struct MessagePreview {
    pub body: AnyElement,
    pub footer: AnyElement,
    pub width: f32,
    pub limit: f32,
    pub more: bool,
    pub expanded: bool,
    /// A session reply clips without a gradient; the
    /// chat transcript still paints its clip fade.
    pub fade: bool,
    pub background: gpui::Hsla,
    pub lines: Option<LinePreview>,
}
impl IntoElement for MessagePreview {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for MessagePreview {
    type RequestLayoutState = (Pixels, bool, Pixels);
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
        let more = self.more || body.height > px(self.limit + 0.5);
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
        (w.request_layout(style, [], cx), (height, more, body.height))
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
        let previous_lines = self
            .lines
            .as_ref()
            .map(|_| LINES.with(|v| v.replace(Some(Vec::new()))));
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
        if let (Some(preview), Some(previous)) = (&self.lines, previous_lines) {
            let mut lines = LINES.with(|v| v.replace(previous)).unwrap_or_default();
            lines.sort_by(|a, b| a.0.total_cmp(&b.0));
            let mut merged: Vec<(f32, f32)> = Vec::new();
            for (top, bottom) in lines {
                if let Some(last) = merged
                    .last_mut()
                    .filter(|last| top < last.1 && bottom > last.0)
                {
                    last.1 = last.1.max(bottom);
                } else {
                    merged.push((top, bottom));
                }
            }
            let height = if merged.len() > 5 {
                (merged[4].1 - bounds.top().as_f32() + 2.).ceil()
            } else {
                state.2.as_f32()
            };
            if (preview.height.get() - height).abs() > 0.5 {
                preview.height.set(height);
                (preview.notify)(cx);
            }
        }
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
