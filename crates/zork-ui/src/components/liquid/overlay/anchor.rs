//! Layout owns window placement; springs only own geometry within that placement.
use super::*;

pub(in crate::components::liquid) struct PopupPlacement {
    pub bounds: Bounds<Pixels>,
    pub available_height: f32,
    pub maximum_height: f32,
    above: bool,
}

/// Fit beside the source's vertical extent, preserving its reachable hit slot.
/// A bottom-edge clamp alone can put a menu over its own sibling triggers.
fn fit_popup(
    source: Bounds<Pixels>,
    desired: Size<Pixels>,
    viewport: Size<Pixels>,
    gap: f32,
    preferred: Option<bool>,
) -> PopupPlacement {
    let margin = 12.;
    let below = (viewport.height.as_f32() - source.bottom().as_f32() - gap - margin).max(2.);
    let above = (source.top().as_f32() - gap - margin).max(2.);
    let on_top = match preferred {
        Some(true) if desired.height.as_f32() <= above => true,
        Some(false) if desired.height.as_f32() <= below => false,
        _ => desired.height.as_f32() > below && above > below,
    };
    let available_height = if on_top { above } else { below };
    let width = desired
        .width
        .as_f32()
        .min(viewport.width.as_f32() - 2. * margin)
        .max(2.);
    let height = desired.height.as_f32().min(available_height).max(2.);
    let x = source.left().as_f32().clamp(
        margin,
        (viewport.width.as_f32() - width - margin).max(margin),
    );
    let y = if on_top {
        source.top().as_f32() - gap - height
    } else {
        source.bottom().as_f32() + gap
    };
    PopupPlacement {
        bounds: Bounds::new(point(px(x), px(y)), size(px(width), px(height))),
        available_height,
        maximum_height: above.max(below),
        above: on_top,
    }
}

#[derive(Clone, Default)]
pub(in crate::components::liquid) struct MeasuredAnchor {
    pub bounds: Rc<Cell<Bounds<Pixels>>>,
    pub visible: Rc<Cell<bool>>,
    pub side: Rc<Cell<Option<bool>>>,
}
impl MeasuredAnchor {
    pub fn reset_placement(&self) {
        self.side.set(None);
    }
    pub fn fit(
        &self,
        source: Bounds<Pixels>,
        desired: Size<Pixels>,
        viewport: Size<Pixels>,
        gap: f32,
    ) -> PopupPlacement {
        let placement = fit_popup(source, desired, viewport, gap, self.side.get());
        self.side.set(Some(placement.above));
        placement
    }
    pub fn update(&self, bounds: Bounds<Pixels>, window: &Window) -> bool {
        let moved = self.bounds.replace(bounds) != bounds;
        let visible = bounds_visible(bounds, window);
        self.visible.replace(visible) != visible || moved
    }
    /// Input handlers receive geometry measured in the prior frame. They have
    /// no render-phase content mask; the window is the remaining clip boundary.
    pub fn set_window_bounds(&self, bounds: Bounds<Pixels>, window: &Window) {
        self.bounds.set(bounds);
        let visible = bounds.intersect(&Bounds::new(point(px(0.), px(0.)), window.viewport_size()));
        self.visible
            .set(visible.size.width > px(0.) && visible.size.height > px(0.));
    }
    pub fn measure<V: 'static>(&self, active: bool, cx: &Context<V>) -> impl IntoElement {
        let anchor = self.clone();
        let owner = cx.entity().downgrade();
        canvas(
            move |bounds, window, _| {
                // Closed controls keep current hit/activation geometry without
                // requesting a second layout on every page scroll.
                if anchor.update(bounds, window) && active {
                    // Placement was computed before this frame's source layout.
                    // A notify during prepaint cannot wake another draw; refit
                    // once after the measured position has been committed.
                    let owner = owner.clone();
                    window.on_next_frame(move |_, cx| {
                        let _ = owner.update(cx, |_, cx| cx.notify());
                    });
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0()
    }
    pub fn layer(&self, child: impl IntoElement, priority: usize) -> AnyElement {
        deferred(AnchorLayer {
            child: child.into_any_element(),
            anchor: self.clone(),
        })
        .with_priority(priority)
        .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::MeasuredAnchor;
    use gpui::{
        canvas, div, prelude::*, px, size, AppContext, Bounds, Context, Pixels, Render,
        TestAppContext, Window,
    };
    use std::{cell::Cell, rc::Rc};

    struct Fixture {
        anchor: MeasuredAnchor,
        top: Option<Pixels>,
        panel: Rc<Cell<Bounds<Pixels>>>,
    }
    impl Render for Fixture {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let placement = self.anchor.fit(
                self.anchor.bounds.get(),
                size(px(280.), px(225.)),
                window.viewport_size(),
                8.,
            );
            let panel = self.panel.clone();
            div()
                .size_full()
                .relative()
                .child(
                    div()
                        .absolute()
                        .left(px(42.))
                        .top(self.top.unwrap_or(window.viewport_size().height - px(50.)))
                        .w(px(112.))
                        .h(px(32.))
                        .child(self.anchor.measure(true, cx)),
                )
                .child(
                    div()
                        .absolute()
                        .left(placement.bounds.origin.x)
                        .top(placement.bounds.origin.y)
                        .w(placement.bounds.size.width)
                        .h(placement.bounds.size.height)
                        .child(
                            canvas(move |bounds, _, _| panel.set(bounds), |_, _, _, _| {})
                                .absolute()
                                .inset_0(),
                        ),
                )
        }
    }
    #[test]
    fn active_anchor_refits_after_prepaint_moves_and_then_stops() {
        let mut cx = TestAppContext::single();
        let panel = Rc::new(Cell::new(Bounds::default()));
        let window = cx.add_window(|_, _| Fixture {
            anchor: Default::default(),
            top: None,
            panel: panel.clone(),
        });
        cx.run_until_parked();
        cx.update_window(window.into(), |_, window, cx| {
            assert_eq!(window.simulate_next_frame(cx), 1)
        })
        .unwrap();
        cx.run_until_parked();
        window
            .update(&mut cx, |view, _, cx| {
                assert!(panel.get().bottom() < view.anchor.bounds.get().top());
                view.top = Some(px(44.));
                cx.notify();
            })
            .unwrap();
        cx.run_until_parked();
        cx.update_window(window.into(), |_, window, cx| {
            assert_eq!(window.simulate_next_frame(cx), 1)
        })
        .unwrap();
        cx.run_until_parked();
        assert!(panel.get().top() >= px(76.));
        cx.update_window(window.into(), |_, window, cx| {
            assert_eq!(window.simulate_next_frame(cx), 0)
        })
        .unwrap();
    }
}

struct AnchorLayer {
    child: AnyElement,
    anchor: MeasuredAnchor,
}

/// Paint in the destination layer from the first frame. Closing keeps the
/// material in a source-relative coordinate frame measured during prepaint.
pub(super) fn destination_layer(
    child: AnyElement,
    anchor: Rc<Cell<Bounds<Pixels>>>,
    reference: Option<Point<Pixels>>,
    offset: Rc<Cell<Point<Pixels>>>,
    priority: usize,
) -> AnyElement {
    deferred(
        anchored()
            .position(point(px(0.), px(0.)))
            .child(DestinationLayer {
                child,
                anchor,
                reference,
                offset,
            }),
    )
    .with_priority(priority)
    .into_any_element()
}

struct DestinationLayer {
    child: AnyElement,
    anchor: Rc<Cell<Bounds<Pixels>>>,
    reference: Option<Point<Pixels>>,
    offset: Rc<Cell<Point<Pixels>>>,
}
impl IntoElement for DestinationLayer {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for DestinationLayer {
    type RequestLayoutState = ();
    type PrepaintState = ();
    fn id(&self) -> Option<ElementId> {
        None
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
        self.offset
            .set(self.reference.map_or(point(px(0.), px(0.)), |origin| {
                self.anchor.get().origin - origin
            }));
        self.child.prepaint(window, cx);
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
        self.child.paint(window, cx);
    }
}
impl IntoElement for AnchorLayer {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for AnchorLayer {
    type RequestLayoutState = ();
    type PrepaintState = bool;
    fn id(&self) -> Option<ElementId> {
        None
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
        let child = self.child.request_layout(window, cx);
        let style = Style {
            position: Position::Absolute,
            ..Default::default()
        };
        (window.request_layout(style, [child], cx), ())
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> bool {
        let visible = self.anchor.visible.get();
        if visible {
            // Deferred prepaint runs after the source measured this frame's
            // scrolled layout, so paint and hit regions move together immediately.
            let offset = self.anchor.bounds.get().origin - bounds.origin;
            window.with_element_offset(offset, |window| self.child.prepaint(window, cx));
        }
        visible
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        visible: &mut bool,
        window: &mut Window,
        cx: &mut App,
    ) {
        if *visible {
            self.child.paint(window, cx);
        }
    }
}
