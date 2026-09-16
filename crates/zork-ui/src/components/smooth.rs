//! Static, adaptive smooth surfaces use the same geometry as liquid controls.
//! A paint-only element: interaction and focus remain with the semantic control.
use super::liquid::{rounded_rectangle, tessellation, Contour, Pose};
use gpui::{
    div, prelude::*, px, App, Bounds, Div, Element, ElementId, GlobalElementId, Hitbox,
    InspectorElementId, LayoutId, Path, Pixels, SharedString, Size, Window,
};
use std::{cell::RefCell, rc::Rc};

pub use zork_liquid::tokens::SMOOTHING;

#[derive(Default)]
pub(crate) struct PaintCache {
    key: Option<(Size<Pixels>, f32, f32)>,
    fill: Option<Path<Pixels>>,
    border: Option<Path<Pixels>>,
}
impl PaintCache {
    pub(crate) fn paint(
        &mut self,
        bounds: Bounds<Pixels>,
        radius: f32,
        fill: gpui::Background,
        border: gpui::Hsla,
        border_width: f32,
        window: &mut Window,
    ) {
        let visible = bounds.intersect(&window.content_mask().bounds);
        if !bounds.size.width.as_f32().is_finite()
            || !bounds.size.height.as_f32().is_finite()
            || !radius.is_finite()
            || bounds.size.width < px(2.)
            || bounds.size.height < px(2.)
            || visible.size.width <= px(0.)
            || visible.size.height <= px(0.)
        {
            return;
        }
        if self.key != Some((bounds.size, radius, border_width)) {
            self.key = Some((bounds.size, radius, border_width));
            let pose = Pose::rect(
                0.,
                0.,
                bounds.size.width.as_f32() as f64,
                bounds.size.height.as_f32() as f64,
                radius as f64,
            );
            let contour = Contour {
                loops: vec![rounded_rectangle(pose, SMOOTHING)],
                sampled_points: 0,
                full_grid_points: 0,
                used_fallback: false,
                revision: 0,
            };
            self.fill = tessellation::fill_path(&contour);
            self.border = (border_width > 0.)
                .then(|| tessellation::stroke_path(&contour, border_width))
                .flatten();
        }
        let draw = |path: &Path<Pixels>, color: gpui::Background, window: &mut Window| {
            window.paint_path_at(path, bounds.origin, color);
        };
        if !fill.is_transparent() {
            if let Some(path) = &self.fill {
                draw(path, fill, window);
            }
        }
        if !border.is_transparent() {
            if let Some(path) = &self.border {
                draw(path, border.into(), window);
            }
        }
    }
}

pub struct Fill {
    inner: gpui::Stateful<Div>,
    radius: f32,
    cache: Option<Rc<RefCell<PaintCache>>>,
    key: SharedString,
    current_color: bool,
}
pub fn fill(id: impl Into<SharedString>, radius: f32) -> Fill {
    let key = id.into();
    Fill {
        inner: div().id(key.clone()).absolute().inset_0(),
        radius,
        cache: None,
        key,
        current_color: false,
    }
}
impl Fill {
    pub fn current_color(mut self) -> Self {
        self.current_color = true;
        self
    }
    pub fn hover_outline(mut self, group: SharedString, color: u32) -> Self {
        self.inner = self
            .inner
            .group_hover(group, move |v| v.border_color(gpui::rgb(color)));
        self
    }
}
impl Styled for Fill {
    fn style(&mut self) -> &mut gpui::StyleRefinement {
        self.inner.style()
    }
}
impl IntoElement for Fill {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for Fill {
    type RequestLayoutState = <gpui::Stateful<Div> as Element>::RequestLayoutState;
    type PrepaintState = Option<Hitbox>;
    fn id(&self) -> Option<ElementId> {
        Element::id(&self.inner)
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let cache = window.use_keyed_state(format!("{}-geometry", self.key), cx, |_, _| {
            Rc::new(RefCell::new(PaintCache::default()))
        });
        self.cache = Some(cache.read(cx).clone());
        self.inner.request_layout(id, inspector, window, cx)
    }
    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        state: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        self.inner
            .prepaint(id, inspector, bounds, state, window, cx)
    }
    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        hitbox: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let style = self
            .inner
            .interactivity()
            .compute_style(id, hitbox.as_ref(), window, cx);
        if style.visibility == gpui::Visibility::Hidden || style.display == gpui::Display::None {
            return;
        }
        let background = if self.current_color {
            window.text_style().color.into()
        } else {
            style
                .background
                .as_ref()
                .and_then(gpui::Fill::color)
                .unwrap_or_default()
        };
        let border = style.border_color.unwrap_or_default();
        let width = style
            .border_widths
            .top
            .to_pixels(window.rem_size())
            .as_f32();
        let background = background.opacity(style.opacity.unwrap_or(1.));
        let border = border.opacity(style.opacity.unwrap_or(1.));
        self.cache.as_ref().unwrap().borrow_mut().paint(
            bounds,
            self.radius,
            background,
            border,
            width,
            window,
        );
    }
}

/// A static smooth container. Input controls are implemented by liquid::controls;
/// this element only paints a measured background and outline.
/// Content must be inset into the contour, as with menu and dialog padding.
pub struct Surface {
    root: gpui::Stateful<Div>,
    children: Vec<gpui::AnyElement>,
    key: SharedString,
    radius: f32,
    rendered: Option<gpui::AnyElement>,
}
pub fn surface(id: impl Into<SharedString>, radius: f32) -> Surface {
    let key = id.into();
    Surface {
        root: div().id(key.clone()).relative(),
        children: vec![],
        key,
        radius,
        rendered: None,
    }
}
impl Styled for Surface {
    fn style(&mut self) -> &mut gpui::StyleRefinement {
        self.root.style()
    }
}
impl InteractiveElement for Surface {
    fn interactivity(&mut self) -> &mut gpui::Interactivity {
        self.root.interactivity()
    }
}
impl StatefulInteractiveElement for Surface {}
impl ParentElement for Surface {
    fn extend(&mut self, elements: impl IntoIterator<Item = gpui::AnyElement>) {
        self.children.extend(elements);
    }
}

// Expose the semantic ID before layout; an anonymous RenderOnce ViewElement
// would hide it from native accessibility and automation.
impl IntoElement for Surface {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for Surface {
    type RequestLayoutState = ();
    type PrepaintState = ();
    fn id(&self) -> Option<ElementId> {
        Some(self.key.clone().into())
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let placeholder = surface(self.key.clone(), self.radius);
        let value = std::mem::replace(self, placeholder);
        let mut rendered = gpui::RenderOnce::render(value, window, cx).into_any_element();
        let layout = rendered.request_layout(window, cx);
        self.rendered = Some(rendered);
        (layout, ())
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
        self.rendered.as_mut().unwrap().prepaint(window, cx);
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.rendered.as_mut().unwrap().paint(window, cx);
    }
}
impl gpui::RenderOnce for Surface {
    fn render(mut self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let background = self.root.style().background.clone();
        let border = self.root.style().border_color;
        let mut root = self.root.border_0().bg(gpui::rgba(0));
        let mut base = fill(format!("{}-fill", self.key), self.radius);
        base.style().background = background;
        root = root.child(base);
        root = root.children(self.children);
        let mut outline = fill(format!("{}-outline", self.key), self.radius)
            .border(gpui::px(crate::design::BORDER_WIDTH));
        outline.style().border_color = border;
        root.child(outline)
    }
}
