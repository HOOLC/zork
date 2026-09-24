//! Static rounded surfaces rendered by GPUI.
use gpui::{
    div, point, prelude::*, px, AnyElement, App, Bounds, Div, Element, ElementId, GlobalElementId,
    InspectorElementId, IntoElement, LayoutId, Path, PathBuilder, Pixels, SharedString, Stateful,
    Window,
};
use std::hash::{Hash, Hasher};

#[derive(gpui::IntoElement)]
pub struct Fill {
    inner: Stateful<Div>,
    current_color: bool,
}

pub fn fill(id: impl Into<SharedString>, radius: f32) -> Fill {
    Fill {
        inner: div().id(id.into()).absolute().inset_0().rounded(px(radius)),
        current_color: false,
    }
}

impl Fill {
    pub fn current_color(mut self) -> Self {
        self.current_color = true;
        self
    }
}

impl gpui::Styled for Fill {
    fn style(&mut self) -> &mut gpui::StyleRefinement {
        self.inner.style()
    }
}

impl gpui::RenderOnce for Fill {
    fn render(self, window: &mut Window, _: &mut App) -> impl gpui::IntoElement {
        if self.current_color {
            self.inner.bg(window.text_style().color)
        } else {
            self.inner
        }
    }
}

pub type Surface = Stateful<Div>;

pub fn surface(id: impl Into<SharedString>, radius: f32) -> Surface {
    div().id(id.into()).rounded(px(radius)).overflow_hidden()
}

/// Clips a scroll viewport to its actual rounded contour. GPUI's `overflow`
/// mask is rectangular, so `rounded().overflow_y_scroll()` alone is insufficient.
pub fn rounded_viewport(
    id: impl Into<SharedString>,
    radius: f32,
    child: impl IntoElement,
) -> RoundedViewport {
    let id = id.into();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    "zork-rounded-viewport".hash(&mut hasher);
    id.hash(&mut hasher);
    RoundedViewport {
        child: child.into_any_element(),
        radius,
        key: hasher.finish(),
    }
}

pub struct RoundedViewport {
    child: AnyElement,
    radius: f32,
    key: u64,
}

impl IntoElement for RoundedViewport {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for RoundedViewport {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.prepaint(window, cx);
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let radius = if self.radius.is_finite() {
            self.radius.max(0.)
        } else {
            0.
        };
        let Some(mask) = rounded_path(bounds, radius) else {
            self.child.paint(window, cx);
            return;
        };
        let scale = window.scale_factor();
        if window
            .with_masked_retained_paint(
                self.key,
                bounds,
                1.,
                1.,
                &mask,
                move || rounded_bands(bounds, radius, scale),
                |window| self.child.paint(window, cx),
            )
            .is_none()
        {
            let bands = rounded_bands(bounds, radius, scale);
            window.with_scaled_paint_clip(bounds.center(), 1., &bands, |window| {
                self.child.paint(window, cx)
            });
        }
    }
}

/// Mirrors `smooth_corner` in the GPUI Metal shader: the curve starts
/// `SMOOTH_EXTENT` radii from the corner and follows a superellipse of
/// exponent 3, approximated by a cubic whose handles are 0.783 of the extent
/// (0.5523 is the circle). Corners at the capsule limit relax to a circle.
const SMOOTH_EXTENT: f32 = 1.45;
const CIRCLE_HANDLE: f32 = 0.552_284_8;
const SMOOTH_HANDLE: f32 = 0.783_2;

pub fn smooth_corner(radius: f32, half_min: f32) -> (f32, f32) {
    if radius <= 0. {
        return (0., CIRCLE_HANDLE);
    }
    let limit = half_min.max(0.);
    let wanted = radius * SMOOTH_EXTENT;
    if wanted <= limit {
        return (wanted, SMOOTH_HANDLE);
    }
    let t = ((limit - radius) / (wanted - radius).max(1e-3)).clamp(0., 1.);
    (
        (radius + (wanted - radius) * t).min(limit),
        CIRCLE_HANDLE + (SMOOTH_HANDLE - CIRCLE_HANDLE) * t,
    )
}

fn rounded_path(bounds: Bounds<Pixels>, radius: f32) -> Option<Path<Pixels>> {
    let x = bounds.origin.x.as_f32();
    let y = bounds.origin.y.as_f32();
    let w = bounds.size.width.as_f32();
    let h = bounds.size.height.as_f32();
    if !w.is_finite() || !h.is_finite() || w <= 0. || h <= 0. {
        return None;
    }
    let (r, handle) = smooth_corner(radius, w.min(h) / 2.);
    let k = handle * r;
    let mut path = PathBuilder::fill();
    if r == 0. {
        path.move_to(point(px(x), px(y)));
        path.line_to(point(px(x + w), px(y)));
        path.line_to(point(px(x + w), px(y + h)));
        path.line_to(point(px(x), px(y + h)));
        path.close();
        return path.build().ok();
    }
    path.move_to(point(px(x + r), px(y)));
    path.line_to(point(px(x + w - r), px(y)));
    path.cubic_bezier_to(
        point(px(x + w), px(y + r)),
        point(px(x + w - r + k), px(y)),
        point(px(x + w), px(y + r - k)),
    );
    path.line_to(point(px(x + w), px(y + h - r)));
    path.cubic_bezier_to(
        point(px(x + w - r), px(y + h)),
        point(px(x + w), px(y + h - r + k)),
        point(px(x + w - r + k), px(y + h)),
    );
    path.line_to(point(px(x + r), px(y + h)));
    path.cubic_bezier_to(
        point(px(x), px(y + h - r)),
        point(px(x + r - k), px(y + h)),
        point(px(x), px(y + h - r + k)),
    );
    path.line_to(point(px(x), px(y + r)));
    path.cubic_bezier_to(
        point(px(x + r), px(y)),
        point(px(x), px(y + r - k)),
        point(px(x + r - k), px(y)),
    );
    path.close();
    path.build().ok()
}

fn rounded_bands(bounds: Bounds<Pixels>, radius: f32, scale: f32) -> Vec<Bounds<Pixels>> {
    let x = bounds.origin.x.as_f32();
    let y = bounds.origin.y.as_f32();
    let w = bounds.size.width.as_f32();
    let h = bounds.size.height.as_f32();
    let step = 1. / scale.max(1.);
    let count = (radius / step).ceil() as usize;
    let mut bands = Vec::with_capacity(count * 2 + 1);
    let mut add = |top: f32, bottom: f32, inset: f32| {
        if bottom > top && w > inset * 2. {
            bands.push(Bounds::from_corners(
                point(px(x + inset), px(y + top)),
                point(px(x + w - inset), px(y + bottom)),
            ));
        }
    };
    for i in 0..count {
        let top = i as f32 * step;
        let bottom = ((i + 1) as f32 * step).min(radius);
        let distance = radius - (top + bottom) / 2.;
        let inset = radius - (radius * radius - distance * distance).max(0.).sqrt();
        add(top, bottom, inset);
        add(h - bottom, h - top, inset);
    }
    add(radius, h - radius, 0.);
    bands
}
