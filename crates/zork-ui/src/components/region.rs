//! Explicit retained view boundaries. Invalidations name presentation regions;
//! data change detection remains in the core subscription that called them.
use gpui::{
    prelude::*, App, Context, Entity, EntityId, Global, StyleRefinement, WeakEntity, Window,
};
use std::collections::HashMap;

mod intrinsic;
pub use intrinsic::IntrinsicCache;

#[derive(Default)]
struct Registry(HashMap<EntityId, HashMap<String, (EntityId, std::rc::Rc<std::cell::Cell<bool>>)>>);
impl Global for Registry {}

pub fn invalidate<T: 'static>(cx: &mut Context<T>, names: &[&str]) {
    let owner = cx.entity_id();
    let ids = cx
        .try_global::<Registry>()
        .and_then(|registry| registry.0.get(&owner))
        .map(|regions| {
            regions
                .iter()
                .filter(|(name, _)| names.is_empty() || names.contains(&name.as_str()))
                .map(|(_, (id, dirty))| {
                    dirty.set(true);
                    *id
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    for id in ids {
        App::notify(cx, id);
    }
    cx.notify();
}
pub fn invalidate_all<T: 'static>(cx: &mut Context<T>) {
    invalidate(cx, &[]);
}
/// Marks every retained region in the app dirty, for changes such as the theme
/// that affect all of them at once.
pub fn invalidate_every(cx: &mut App) {
    let ids = cx
        .try_global::<Registry>()
        .map(|registry| {
            registry
                .0
                .values()
                .flat_map(|regions| regions.values())
                .map(|(id, dirty)| {
                    dirty.set(true);
                    *id
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    for id in ids {
        cx.notify(id);
    }
}

type Renderer<T> = Box<dyn Fn(&mut T, &mut Window, &mut Context<T>) -> gpui::AnyElement>;
struct Region<T: 'static> {
    parent: WeakEntity<T>,
    render: Renderer<T>,
    layout: std::rc::Rc<Measured>,
    #[cfg(feature = "headless-bench")]
    counts: std::rc::Rc<Counts>,
}
impl<T: 'static> Render for Region<T> {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let element = self
            .parent
            .update(cx, |parent, cx| (self.render)(parent, window, cx))
            .unwrap_or_else(|_| gpui::Empty.into_any_element());
        #[cfg(feature = "headless-bench")]
        self.counts.render.set(self.counts.render.get() + 1);
        MeasuredElement {
            inner: element,
            layout: self.layout.clone(),
            #[cfg(feature = "headless-bench")]
            counts: self.counts.clone(),
        }
        .into_any_element()
    }
}
/// A small, owner-scoped set of independently cached regions. Call `retain` for
/// dynamic row keys so removed records do not retain views or event handlers.
pub struct Regions<T: 'static> {
    views: HashMap<String, Entity<Region<T>>>,
}
impl<T: 'static> Default for Regions<T> {
    fn default() -> Self {
        Self {
            views: HashMap::new(),
        }
    }
}
impl<T: 'static> Regions<T> {
    fn ensure(
        &mut self,
        name: &str,
        cx: &mut Context<T>,
        render: impl Fn(&mut T, &mut Window, &mut Context<T>) -> gpui::AnyElement + 'static,
    ) -> Entity<Region<T>> {
        if cx.try_global::<Registry>().is_none() {
            cx.set_global(Registry::default());
        }
        let parent = cx.entity().downgrade();
        let owner = cx.entity_id();
        let key = name.to_owned();
        let view = self.views.entry(key.clone()).or_insert_with(|| {
            cx.new(|cx| {
                let id = cx.entity_id();
                let layout = std::rc::Rc::new(Measured::default());
                cx.global_mut::<Registry>()
                    .0
                    .entry(owner)
                    .or_default()
                    .insert(key.clone(), (id, layout.dirty.clone()));
                let release_key = key.clone();
                cx.on_release(move |_, cx| {
                    if let Some(registry) =
                        cx.try_global::<crate::automation::element::AutomationRegistryGlobal>()
                    {
                        registry.0.forget_region(id);
                    }
                    if let Some(registry) = cx.try_global::<Registry>() {
                        if registry
                            .0
                            .get(&owner)
                            .and_then(|v| v.get(&release_key))
                            .is_some_and(|(existing, _)| *existing == id)
                        {
                            let registry = cx.global_mut::<Registry>();
                            if let Some(regions) = registry.0.get_mut(&owner) {
                                regions.remove(&release_key);
                                if regions.is_empty() {
                                    registry.0.remove(&owner);
                                }
                            }
                        }
                    }
                })
                .detach();
                Region {
                    parent,
                    render: Box::new(render),
                    layout,
                    #[cfg(feature = "headless-bench")]
                    counts: std::rc::Rc::new(Counts::default()),
                }
            })
        });
        view.clone()
    }
    /// Render through the same region identity without replaying its paint.
    /// Invalidate this region before switching back to cached rendering.
    pub fn uncached(
        &mut self,
        name: &str,
        cx: &mut Context<T>,
        render: impl Fn(&mut T, &mut Window, &mut Context<T>) -> gpui::AnyElement + 'static,
    ) -> gpui::AnyElement {
        let view = self.ensure(name, cx, render);
        RegionElement {
            rasterized: false,
            measured: Some(view.read(cx).layout.clone()),
            view,
            style: None,
        }
        .into_any_element()
    }
    /// Lay out normally so independently cached descendants can update,
    /// while retaining the region's pixels for subsequent paint-only frames.
    pub fn gpu_uncached(
        &mut self,
        name: &str,
        cx: &mut Context<T>,
        render: impl Fn(&mut T, &mut Window, &mut Context<T>) -> gpui::AnyElement + 'static,
    ) -> gpui::AnyElement {
        let view = self.ensure(name, cx, render);
        RegionElement {
            rasterized: true,
            measured: Some(view.read(cx).layout.clone()),
            view,
            style: None,
        }
        .into_any_element()
    }
    pub fn element(
        &mut self,
        name: &str,
        style: StyleRefinement,
        cx: &mut Context<T>,
        render: impl Fn(&mut T, &mut Window, &mut Context<T>) -> gpui::AnyElement + 'static,
    ) -> gpui::AnyElement {
        let view = self.ensure(name, cx, render);
        RegionElement {
            rasterized: false,
            measured: Some(view.read(cx).layout.clone()),
            view,
            style: Some(style),
        }
        .into_any_element()
    }
    /// Preserve intrinsic height on the frame that changes content/width, then
    /// reuse that measured height on subsequent unrelated frames. No guessed
    /// text heights or fixed-height clipping are introduced by caching.
    pub fn auto_height(
        &mut self,
        name: &str,
        width_key: f32,
        cx: &mut Context<T>,
        render: impl Fn(&mut T, &mut Window, &mut Context<T>) -> gpui::AnyElement + 'static,
    ) -> gpui::AnyElement {
        self.auto_height_inner(name, width_key, cx, render, false)
    }
    fn auto_height_inner(
        &mut self,
        name: &str,
        width_key: f32,
        cx: &mut Context<T>,
        render: impl Fn(&mut T, &mut Window, &mut Context<T>) -> gpui::AnyElement + 'static,
        rasterized: bool,
    ) -> gpui::AnyElement {
        // Cached views lay out their content as a separate root. Own the
        // width/stretch contract here so intrinsic children keep the same
        // alignment as they have in the parent's normal flex layout.
        let view = self.ensure(name, cx, move |parent, window, cx| {
            gpui::div()
                .w_full()
                .flex_shrink_0()
                .flex()
                .flex_col()
                .child(render(parent, window, cx))
                .into_any_element()
        });
        let layout = view.read(cx).layout.clone();
        let width_changed = layout.width.replace(width_key) != width_key;
        if layout.dirty.get() || width_changed || layout.size.get().is_none() {
            RegionElement {
                rasterized,
                view,
                style: None,
                measured: Some(layout),
            }
            .into_any_element()
        } else {
            RegionElement {
                rasterized,
                measured: Some(layout.clone()),
                view,
                style: Some(
                    StyleRefinement::default()
                        .w_full()
                        .h(layout.size.get().unwrap().height)
                        .flex_shrink_0(),
                ),
            }
            .into_any_element()
        }
    }
    pub fn retain(&mut self, keep: impl Fn(&str) -> bool) {
        self.views.retain(|key, _| keep(key));
    }
    #[cfg(feature = "headless-bench")]
    pub fn counters(&self, cx: &App) -> HashMap<String, [usize; 4]> {
        self.views
            .iter()
            .map(|(key, view)| {
                let c = &view.read(cx).counts;
                (
                    key.clone(),
                    [
                        c.render.get(),
                        c.layout.get(),
                        c.prepaint.get(),
                        c.paint.get(),
                    ],
                )
            })
            .collect()
    }
}
#[cfg(feature = "headless-bench")]
#[derive(Default)]
struct Counts {
    render: std::cell::Cell<usize>,
    layout: std::cell::Cell<usize>,
    prepaint: std::cell::Cell<usize>,
    paint: std::cell::Cell<usize>,
}
#[derive(Default)]
struct Measured {
    size: std::cell::Cell<Option<gpui::Size<gpui::Pixels>>>,
    width: std::cell::Cell<f32>,
    dirty: std::rc::Rc<std::cell::Cell<bool>>,
    prepaints: std::cell::Cell<usize>,
}
struct MeasuredElement {
    inner: gpui::AnyElement,
    layout: std::rc::Rc<Measured>,
    #[cfg(feature = "headless-bench")]
    counts: std::rc::Rc<Counts>,
}
impl IntoElement for MeasuredElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl gpui::Element for MeasuredElement {
    type RequestLayoutState = ();
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
    ) -> (gpui::LayoutId, ()) {
        #[cfg(feature = "headless-bench")]
        self.counts.layout.set(self.counts.layout.get() + 1);
        (self.inner.request_layout(w, cx), ())
    }
    fn prepaint(
        &mut self,
        _: Option<&gpui::GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: gpui::Bounds<gpui::Pixels>,
        _: &mut (),
        w: &mut Window,
        cx: &mut App,
    ) {
        self.layout
            .prepaints
            .set(self.layout.prepaints.get().wrapping_add(1));
        self.layout.size.set(Some(bounds.size));
        self.layout.dirty.set(false);
        #[cfg(feature = "headless-bench")]
        self.counts.prepaint.set(self.counts.prepaint.get() + 1);
        self.inner.prepaint(w, cx);
    }
    fn paint(
        &mut self,
        _: Option<&gpui::GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        _: gpui::Bounds<gpui::Pixels>,
        _: &mut (),
        _: &mut (),
        w: &mut Window,
        cx: &mut App,
    ) {
        #[cfg(feature = "headless-bench")]
        self.counts.paint.set(self.counts.paint.get() + 1);
        self.inner.paint(w, cx);
    }
}

/// Track a separately observed child view inside a cached region, including
/// deferred popovers in its automation replay. The child controls its redraws.
pub fn tracked_view<V: Render>(view: Entity<V>) -> gpui::AnyElement {
    RegionElement {
        rasterized: false,
        view,
        style: None,
        measured: None,
    }
    .into_any_element()
}

pub fn forget_on_release<V: 'static>(cx: &Context<V>) {
    let id = cx.entity_id();
    cx.on_release(move |_, cx| {
        if let Some(registry) =
            cx.try_global::<crate::automation::element::AutomationRegistryGlobal>()
        {
            registry.0.forget_region(id);
        }
    })
    .detach();
}

struct RegionElement<V: Render> {
    rasterized: bool,
    view: Entity<V>,
    style: Option<StyleRefinement>,
    measured: Option<std::rc::Rc<Measured>>,
}
impl<V: Render> IntoElement for RegionElement<V> {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl<V: Render> gpui::Element for RegionElement<V> {
    type RequestLayoutState = gpui::AnyElement;
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
        window: &mut Window,
        cx: &mut App,
    ) -> (gpui::LayoutId, gpui::AnyElement) {
        // GPUI 1.17 does not replay AccessKit nodes/action handlers with cached
        // paint ranges. Preserve the full accessibility tree when AT is active.
        let mut element = if window.is_a11y_active() {
            let mut carrier = gpui::div();
            if let Some(style) = &self.style {
                *carrier.style() = style.clone();
            }
            carrier.child(self.view.clone()).into_any_element()
        } else if let Some(style) = &self.style {
            let element = self.view.clone().cached(style.clone());
            if self.rasterized {
                element.rasterized().into_any_element()
            } else {
                element.into_any_element()
            }
        } else if self.measured.is_some() {
            let element = self.view.clone().measure_cached();
            if self.rasterized {
                element.rasterized().into_any_element()
            } else {
                element.into_any_element()
            }
        } else {
            self.view.clone().into_any_element()
        };
        let layout = element.request_layout(window, cx);
        (layout, element)
    }
    fn prepaint(
        &mut self,
        _: Option<&gpui::GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        _: gpui::Bounds<gpui::Pixels>,
        element: &mut gpui::AnyElement,
        window: &mut Window,
        cx: &mut App,
    ) {
        let registry = cx
            .try_global::<crate::automation::element::AutomationRegistryGlobal>()
            .map(|r| r.0.clone());
        let id = self.view.entity_id();
        let window_id = window.window_handle().window_id();
        let measured = self.measured.clone();
        let before = measured.as_ref().map(|v| v.prepaints.get());
        if let Some(registry) = &registry {
            registry.begin_region(window_id, id);
        }
        element.prepaint(window, cx);
        if let Some(registry) = registry {
            registry.end_region(
                window_id,
                id,
                measured
                    .as_ref()
                    .is_some_and(|v| Some(v.prepaints.get()) == before),
            );
        }
    }
    fn paint(
        &mut self,
        _: Option<&gpui::GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        _: gpui::Bounds<gpui::Pixels>,
        element: &mut gpui::AnyElement,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        element.paint(window, cx);
    }
}

#[cfg(all(test, feature = "headless-bench"))]
mod tests {
    use super::*;
    use crate::automation::element::{
        AutomationRegistry, AutomationRegistryGlobal, AutomationRoot,
    };
    use crate::automation::{AutomationElementExt, AutomationRole};
    use gpui::{px, Animation, AnimationExt, AppContext, TestAppContext};
    struct Motion;
    impl Render for Motion {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            gpui::div().size(px(20.)).with_animation(
                "motion",
                Animation::new(std::time::Duration::from_secs(60)),
                |v, t| v.opacity(t),
            )
        }
    }
    struct Fixture {
        motion: Entity<Motion>,
        regions: Regions<Self>,
        height: f32,
    }
    impl Render for Fixture {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            gpui::div()
                .size_full()
                .flex()
                .flex_col()
                .child(self.motion.clone())
                .child(self.regions.auto_height("static", 300., cx, |v, _, _| {
                    gpui::div().w_full().h(px(v.height)).into_any_element()
                }))
        }
    }
    #[test]
    fn animation_reuses_intrinsic_region_and_its_own_update_remeasures_it() {
        let mut cx = TestAppContext::single();
        let window = cx.add_window(|_, cx| Fixture {
            motion: cx.new(|_| Motion),
            regions: Regions::default(),
            height: 40.,
        });
        cx.run_until_parked();
        // Populate the measured size and retained view's first cached frame.
        for _ in 0..3 {
            window
                .update(&mut cx, |_, w, cx| w.simulate_next_frame(cx))
                .unwrap();
            cx.run_until_parked();
        }
        let before = window
            .update(&mut cx, |v, _, cx| v.regions.counters(cx)["static"])
            .unwrap();
        for _ in 0..120 {
            window
                .update(&mut cx, |_, w, cx| assert!(w.simulate_next_frame(cx) > 0))
                .unwrap();
            cx.run_until_parked();
        }
        let after = window
            .update(&mut cx, |v, _, cx| v.regions.counters(cx)["static"])
            .unwrap();
        assert_eq!(before, after, "animation rebuilt an unrelated region");
        window
            .update(&mut cx, |v, _, cx| {
                v.height = 120.;
                invalidate(cx, &["static"]);
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(&mut cx, |v, _, cx| {
                let cached = v.regions.views["static"].read(cx);
                assert_eq!(
                    cached.layout.size.get().unwrap().height,
                    px(120.),
                    "height changes were clipped by caching"
                );
                assert!(cached.counts.render.get() > before[0]);
            })
            .unwrap();
    }

    struct PopoverFixture {
        regions: Regions<Self>,
        show: bool,
        popup: Entity<Popover>,
    }
    struct Popover {
        label: String,
    }
    impl Render for Popover {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            gpui::deferred(
                gpui::div()
                    .id("popover-control")
                    .size(px(40.))
                    .automation(AutomationRole::Button, self.label.clone()),
            )
        }
    }
    impl Render for PopoverFixture {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.regions.element(
                "panel",
                StyleRefinement::default().size_full(),
                cx,
                |v, _, _| {
                    gpui::div()
                        .size_full()
                        .when(v.show, |row| row.child(tracked_view(v.popup.clone())))
                        .into_any_element()
                },
            )
        }
    }
    #[test]
    fn cached_deferred_controls_remain_discoverable_and_disappear_when_closed() {
        let mut cx = TestAppContext::single();
        let registry = AutomationRegistry::new();
        cx.update(|cx| cx.set_global(AutomationRegistryGlobal(registry.clone())));
        let mut root = None;
        let window = cx.add_window(|_, cx| {
            let view = cx.new(|cx| PopoverFixture {
                regions: Default::default(),
                show: true,
                popup: cx.new(|cx| {
                    forget_on_release(cx);
                    Popover {
                        label: "Popover".into(),
                    }
                }),
            });
            root = Some(view.clone());
            AutomationRoot::new(view)
        });
        let root = root.unwrap();
        cx.run_until_parked();
        for _ in 0..4 {
            cx.update_window(window.into(), |_, w, cx| w.draw(cx).clear(cx))
                .unwrap();
            cx.run_until_parked();
            assert!(
                registry
                    .snapshot(false)
                    .elements
                    .iter()
                    .any(|e| e.id == "popover-control"),
                "cached deferred target vanished"
            );
        }
        root.read_with(&cx, |v, _| v.popup.clone())
            .update(&mut cx, |v, cx| {
                v.label = "Updated from its own source".into();
                cx.notify();
            });
        cx.run_until_parked();
        for _ in 0..4 {
            cx.update_window(window.into(), |_, w, cx| w.draw(cx).clear(cx))
                .unwrap();
            cx.run_until_parked();
            assert!(registry
                .snapshot(false)
                .elements
                .iter()
                .any(|e| e.id == "popover-control" && e.label == "Updated from its own source"));
        }
        root.update(&mut cx, |v, cx| {
            v.show = false;
            invalidate(cx, &["panel"]);
        });
        cx.run_until_parked();
        cx.update_window(window.into(), |_, w, cx| w.draw(cx).clear(cx))
            .unwrap();
        assert!(
            !registry
                .snapshot(true)
                .elements
                .iter()
                .any(|e| e.id == "popover-control"),
            "closed popover left a stale target"
        );
    }
}

#[cfg(test)]
mod nested_cache_tests {
    use gpui::{
        div, prelude::*, px, AppContext, Context, Entity, Render, StyleRefinement, TestAppContext,
        Window,
    };
    use std::{cell::Cell, rc::Rc};
    struct Child(Rc<Cell<usize>>);
    impl Render for Child {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            self.0.set(self.0.get() + 1);
            div().size_full().children(
                (0usize..8).map(|i| div().id(("nested-control", i)).focusable().child("control")),
            )
        }
    }
    struct Parent(Entity<Child>);
    impl Render for Parent {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            self.0
                .clone()
                .cached(StyleRefinement::default().size_full())
        }
    }
    struct Root {
        parent: Entity<Parent>,
        leading: bool,
        cached: bool,
    }
    impl Render for Root {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .relative()
                .children(
                    (0usize..if self.leading { 128 } else { 0 })
                        .map(|i| div().id(("leading", i)).absolute().size(px(1.)).focusable()),
                )
                .child(if self.cached {
                    self.parent
                        .clone()
                        .cached(StyleRefinement::default().size_full())
                        .into_any_element()
                } else {
                    self.parent.clone().into_any_element()
                })
        }
    }
    #[test]
    fn nested_cache_rebuilds_after_ancestor_replay_changes_frame_indices() {
        let mut cx = TestAppContext::single();
        let count = Rc::new(Cell::new(0));
        let window = cx.add_window(|_, cx| {
            let child = cx.new(|_| Child(count.clone()));
            Root {
                parent: cx.new(|_| Parent(child)),
                leading: true,
                cached: true,
            }
        });
        cx.run_until_parked();
        window
            .update(&mut cx, |root, _, cx| {
                root.leading = false;
                cx.notify();
            })
            .unwrap();
        cx.run_until_parked();
        let before = count.get();
        window
            .update(&mut cx, |root, _, cx| {
                root.cached = false;
                cx.notify();
            })
            .unwrap();
        cx.run_until_parked();
        assert!(count.get() > before, "stale child ranges were reused after its ancestor replayed them into different indices");
    }
}
