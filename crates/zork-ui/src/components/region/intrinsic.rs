//! Retain an entity's measured height without fixing its next content update
//! to that old height. Input and paint reuse remain in GPUI's ordinary view.
use super::*;
use gpui::{
    AnyElement, Element, GlobalElementId, InspectorElementId, IntoElement, LayoutId, Pixels,
    TextStyle,
};
use std::{cell::RefCell, rc::Rc};

#[derive(PartialEq)]
struct LayoutKey {
    width: Pixels,
    rem: Pixels,
    text: TextStyle,
}

/// A view-owned intrinsic measurement. Width, text context and entity
/// invalidation remeasure it; inherited paint-only changes must invalidate it.
#[derive(Clone, Default)]
pub struct IntrinsicCache {
    layout: Rc<Measured>,
    key: Rc<RefCell<Option<LayoutKey>>>,
    #[cfg(feature = "headless-bench")]
    counts: Rc<Counts>,
}
impl IntrinsicCache {
    pub fn invalidate(&self) {
        self.layout.dirty.set(true);
    }
    /// Wrap the complete view result so the cache records actual layout.
    pub fn measure(&self, child: impl IntoElement) -> AnyElement {
        MeasuredElement {
            inner: child.into_any_element(),
            layout: self.layout.clone(),
            #[cfg(feature = "headless-bench")]
            counts: self.counts.clone(),
        }
        .into_any_element()
    }
    pub fn element<V: Render>(&self, view: Entity<V>, width: Pixels) -> AnyElement {
        IntrinsicView {
            view,
            width,
            cache: self.clone(),
        }
        .into_any_element()
    }
}
struct IntrinsicView<V: Render> {
    view: Entity<V>,
    width: Pixels,
    cache: IntrinsicCache,
}
impl<V: Render> IntoElement for IntrinsicView<V> {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl<V: Render> Element for IntrinsicView<V> {
    type RequestLayoutState = AnyElement;
    type PrepaintState = ();
    fn id(&self) -> Option<gpui::ElementId> {
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
    ) -> (LayoutId, AnyElement) {
        let key = LayoutKey {
            width: self.width,
            rem: window.rem_size(),
            text: window.text_style(),
        };
        let changed = self.cache.key.borrow().as_ref() != Some(&key);
        *self.cache.key.borrow_mut() = Some(key);
        let size = self.cache.layout.size.get();
        let measure = changed
            || self.cache.layout.dirty.get()
            || size.is_none()
            || window.view_needs_render(self.view.entity_id());
        let mut element = RegionElement {
            rasterized: false,
            view: self.view.clone(),
            measured: Some(self.cache.layout.clone()),
            style: (!measure).then(|| {
                StyleRefinement::default()
                    .w(self.width)
                    .h(size.unwrap().height)
                    .flex_shrink_0()
            }),
        }
        .into_any_element();
        (element.request_layout(window, cx), element)
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: gpui::Bounds<Pixels>,
        element: &mut AnyElement,
        window: &mut Window,
        cx: &mut App,
    ) {
        element.prepaint(window, cx);
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: gpui::Bounds<Pixels>,
        element: &mut AnyElement,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        element.paint(window, cx);
    }
}

#[cfg(all(test, feature = "headless-bench"))]
mod tests {
    use super::IntrinsicCache;
    use gpui::{AppContext, Context, Entity, Render, TestAppContext, Window, div, prelude::*, px};
    use std::{cell::Cell, rc::Rc};
    struct Child {
        cache: IntrinsicCache,
        height: f32,
        renders: Rc<Cell<usize>>,
    }
    impl Render for Child {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            self.renders.set(self.renders.get() + 1);
            self.cache
                .measure(div().w_full().h(px(self.height)).child("Measured content"))
        }
    }
    struct Root {
        child: Entity<Child>,
        width: f32,
        text: f32,
    }
    impl Render for Root {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let cache = self.child.read(cx).cache.clone();
            div()
                .w(px(self.width))
                .text_size(px(self.text))
                .child(cache.element(self.child.clone(), px(self.width)))
        }
    }
    #[test]
    fn independent_redraws_reuse_height_and_content_changes_remeasure() {
        let mut cx = TestAppContext::single();
        let cache = IntrinsicCache::default();
        let renders = Rc::new(Cell::new(0));
        let window = cx.add_window(|_, cx| Root {
            child: cx.new(|_| Child {
                cache: cache.clone(),
                height: 40.,
                renders: renders.clone(),
            }),
            width: 200.,
            text: 13.,
        });
        cx.run_until_parked();
        let count = renders.get();
        for _ in 0..12 {
            window.update(&mut cx, |_, _, cx| cx.notify()).unwrap();
            cx.run_until_parked();
        }
        assert_eq!(renders.get(), count);
        window
            .update(&mut cx, |root, _, cx| {
                root.child.update(cx, |view, cx| {
                    view.height = 120.;
                    cx.notify();
                })
            })
            .unwrap();
        cx.run_until_parked();
        assert_eq!(cache.layout.size.get().unwrap().height, px(120.));
        assert!(renders.get() > count);
        let count = renders.get();
        for _ in 0..4 {
            window.update(&mut cx, |_, _, cx| cx.notify()).unwrap();
            cx.run_until_parked();
        }
        assert_eq!(
            renders.get(),
            count,
            "The measured frame is immediately reusable"
        );
        window
            .update(&mut cx, |root, _, cx| {
                root.width = 280.;
                root.text = 20.;
                cx.notify();
            })
            .unwrap();
        cx.run_until_parked();
        assert_eq!(cache.layout.size.get().unwrap().width, px(280.));
        assert!(renders.get() > count);
    }
    struct Nested {
        child: Entity<Child>,
        cache: IntrinsicCache,
    }
    impl Render for Nested {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            self.cache.measure(div().w_full().child(self.child.clone()))
        }
    }
    struct PreReadingRoot {
        nested: Entity<Nested>,
        pre_read: bool,
    }
    impl Render for PreReadingRoot {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let nested = self.nested.read(cx);
            let cache = nested.cache.clone();
            if self.pre_read {
                let _ = nested.child.read(cx).height;
                self.pre_read = false;
            }
            div()
                .w(px(200.))
                .child(cache.element(self.nested.clone(), px(200.)))
        }
    }
    #[test]
    fn cached_descendant_keeps_notifications_after_a_parent_pre_read() {
        let mut cx = TestAppContext::single();
        let cache = IntrinsicCache::default();
        let window = cx.add_window(|_, cx| PreReadingRoot {
            nested: cx.new(|cx| Nested {
                child: cx.new(|_| Child {
                    cache: IntrinsicCache::default(),
                    height: 40.,
                    renders: Default::default(),
                }),
                cache: cache.clone(),
            }),
            pre_read: true,
        });
        cx.run_until_parked();
        for _ in 0..4 {
            window.update(&mut cx, |_, _, cx| cx.notify()).unwrap();
            cx.run_until_parked();
        }
        window
            .update(&mut cx, |root, _, cx| {
                let child = root.nested.read(cx).child.clone();
                child.update(cx, |v, cx| {
                    v.height = 120.;
                    cx.notify();
                });
            })
            .unwrap();
        cx.run_until_parked();
        assert_eq!(cache.layout.size.get().unwrap().height, px(120.));
    }
    struct StatefulChild {
        cache: IntrinsicCache,
        creations: Rc<Cell<usize>>,
    }
    impl Render for StatefulChild {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let creations = self.creations.clone();
            let state = window.use_keyed_state("measured-child-state", cx, move |_, _| {
                creations.set(creations.get() + 1);
                42usize
            });
            self.cache
                .measure(div().w_full().h(px(40.)).child(state.read(cx).to_string()))
        }
    }
    struct StatefulRoot(Entity<StatefulChild>);
    impl Render for StatefulRoot {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let cache = self.0.read(cx).cache.clone();
            div()
                .w(px(200.))
                .child(cache.element(self.0.clone(), px(200.)))
        }
    }
    #[test]
    fn intrinsic_measurement_preserves_keyed_state_across_cached_frames() {
        let mut cx = TestAppContext::single();
        let creations = Rc::new(Cell::new(0));
        let window = cx.add_window(|_, cx| {
            StatefulRoot(cx.new(|_| StatefulChild {
                cache: IntrinsicCache::default(),
                creations: creations.clone(),
            }))
        });
        cx.run_until_parked();
        assert_eq!(creations.get(), 1);
        for _ in 0..4 {
            window.update(&mut cx, |_, _, cx| cx.notify()).unwrap();
            cx.run_until_parked();
        }
        window
            .update(&mut cx, |root, _, cx| {
                root.0.update(cx, |_, cx| cx.notify())
            })
            .unwrap();
        cx.run_until_parked();
        assert_eq!(
            creations.get(),
            1,
            "Replaying an intrinsic cache discarded its keyed control state"
        );
    }
    type TextProbe = Rc<std::cell::RefCell<Option<std::sync::Weak<gpui::LineLayout>>>>;
    struct LateText(TextProbe);
    impl IntoElement for LateText {
        type Element = Self;
        fn into_element(self) -> Self {
            self
        }
    }
    impl gpui::Element for LateText {
        type RequestLayoutState = ();
        type PrepaintState = ();
        fn id(&self) -> Option<gpui::ElementId> {
            None
        }
        fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
            None
        }
        fn request_layout(
            &mut self,
            _: Option<&gpui::GlobalElementId>,
            _: Option<&gpui::InspectorElementId>,
            window: &mut Window,
            _: &mut gpui::App,
        ) -> (gpui::LayoutId, ()) {
            let probe = self.0.clone();
            let run = window.text_style().to_run("measured text resource".len());
            (
                window.request_measured_layout(gpui::Style::default(), move |_, _, window, _| {
                    let line = window.text_system().layout_line(
                        "measured text resource",
                        px(14.),
                        std::slice::from_ref(&run),
                        None,
                    );
                    *probe.borrow_mut() = Some(std::sync::Arc::downgrade(&line));
                    gpui::size(line.width, px(20.))
                }),
                (),
            )
        }
        fn prepaint(
            &mut self,
            _: Option<&gpui::GlobalElementId>,
            _: Option<&gpui::InspectorElementId>,
            _: gpui::Bounds<gpui::Pixels>,
            _: &mut (),
            _: &mut Window,
            _: &mut gpui::App,
        ) {
        }
        fn paint(
            &mut self,
            _: Option<&gpui::GlobalElementId>,
            _: Option<&gpui::InspectorElementId>,
            _: gpui::Bounds<gpui::Pixels>,
            _: &mut (),
            _: &mut (),
            _: &mut Window,
            _: &mut gpui::App,
        ) {
        }
    }
    struct TextChild {
        cache: IntrinsicCache,
        probe: TextProbe,
    }
    impl Render for TextChild {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            self.cache.measure(LateText(self.probe.clone()))
        }
    }
    struct TextRoot(Entity<TextChild>);
    impl Render for TextRoot {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let cache = self.0.read(cx).cache.clone();
            div()
                .w(px(200.))
                .child(cache.element(self.0.clone(), px(200.)))
        }
    }
    #[test]
    fn intrinsic_measurement_keeps_text_layouts_used_by_taffy() {
        let mut cx = TestAppContext::single();
        let probe: TextProbe = Default::default();
        let window = cx.add_window(|_, cx| {
            TextRoot(cx.new(|_| TextChild {
                cache: Default::default(),
                probe: probe.clone(),
            }))
        });
        cx.run_until_parked();
        assert!(probe.borrow().as_ref().unwrap().upgrade().is_some());
        for _ in 0..4 {
            window.update(&mut cx, |_, _, cx| cx.notify()).unwrap();
            cx.run_until_parked();
        }
        assert!(
            probe.borrow().as_ref().unwrap().upgrade().is_some(),
            "Cached intrinsic text lost its shaped layout"
        );
    }
}
