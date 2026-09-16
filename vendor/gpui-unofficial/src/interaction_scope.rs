//! Live input eligibility for retained content. Visual opacity remains the
//! composition's responsibility, so cached pixels can be prepared while hidden.
use crate::{
    AnyElement, App, Bounds, Element, ElementId, GlobalElementId, InspectorElementId, IntoElement,
    LayoutId, Pixels, Window,
};
use std::sync::{
    Arc,
    atomic::{AtomicU8, Ordering},
};

/// A stable interaction flag shared by cached input records and their owner.
#[derive(Clone, Debug)]
pub struct InteractionGate(Arc<AtomicU8>);

impl InteractionGate {
    /// Create a gate for a retained subtree.
    pub fn new(enabled: bool) -> Self {
        Self(Arc::new(AtomicU8::new(if enabled { 3 } else { 0 })))
    }
    /// Change eligibility without rebuilding the cached input records.
    pub fn set_enabled(&self, enabled: bool) {
        self.set_state(enabled, enabled);
    }
    /// Whether the owner currently allows interaction.
    pub fn enabled(&self) -> bool {
        self.0.load(Ordering::Relaxed) & 2 != 0
    }
    /// Set visual presence and interaction independently during an exit fade.
    pub fn set_state(&self, visible: bool, enabled: bool) {
        self.0.store(
            u8::from(visible) | (u8::from(visible && enabled) << 1),
            Ordering::Relaxed,
        );
    }
    /// Whether the retained content is visually present.
    pub fn visible(&self) -> bool {
        self.0.load(Ordering::Relaxed) & 1 != 0
    }
    /// Whether every containing scope is visually present.
    pub fn visible_in(gates: Option<&[Self]>) -> bool {
        gates.is_none_or(|gates| gates.iter().all(Self::visible))
    }
    /// Whether all containing scopes allow interaction.
    pub fn allows(gates: Option<&[Self]>) -> bool {
        gates.is_none_or(|gates| gates.iter().all(Self::enabled))
    }
}
impl PartialEq for InteractionGate {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for InteractionGate {}

/// Keep a hidden subtree's layout and paint cache while removing its input
/// eligibility. Keep the gate identity stable between renders; animate visual
/// opacity separately. Closed scopes are also hidden from accessibility.
pub fn interaction_scope(
    id: impl Into<ElementId>,
    gate: InteractionGate,
    child: impl IntoElement,
) -> InteractionScope {
    InteractionScope {
        id: id.into(),
        gate,
        child: child.into_any_element(),
    }
}

/// An input scope around retained content.
pub struct InteractionScope {
    id: ElementId,
    gate: InteractionGate,
    child: AnyElement,
}
impl IntoElement for InteractionScope {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for InteractionScope {
    type RequestLayoutState = ();
    type PrepaintState = ();
    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
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
        (
            window.with_interaction_gate(self.gate.clone(), |window| {
                self.child.request_layout(window, cx)
            }),
            (),
        )
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
        window.with_interaction_gate(self.gate.clone(), |window| self.child.prepaint(window, cx));
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
        window.with_interaction_gate(self.gate.clone(), |window| self.child.paint(window, cx));
    }
    fn a11y_role(&self) -> Option<accesskit::Role> {
        Some(accesskit::Role::GenericContainer)
    }
    fn write_a11y_info(&self, node: &mut accesskit::Node) {
        if !self.gate.enabled() {
            node.set_hidden();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prelude::*;
    use crate::{
        AppContext, Context, Entity, HitboxBehavior, HitboxId, KeyDownEvent, Keystroke,
        MouseMoveEvent, PlatformInput, Point, Render, StyleRefinement, TestAppContext, anchored,
        canvas, deferred, div, point, px,
    };
    use crate::{FocusHandle, MouseButton, MouseDownEvent, MouseUpEvent};
    use std::{cell::Cell, rc::Rc};

    struct Fixture {
        gate: InteractionGate,
        nested: bool,
        back: Rc<Cell<Option<HitboxId>>>,
        front: Rc<Cell<Option<HitboxId>>>,
    }
    impl Render for Fixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let back = self.back.clone();
            let front = self.front.clone();
            let foreground = canvas(
                move |bounds, window, _| {
                    front.set(Some(
                        window.insert_hitbox(bounds, HitboxBehavior::BlockMouse).id,
                    ));
                },
                |_, _, _, _| {},
            )
            .w(px(100.))
            .h(px(100.));
            let foreground = if self.nested {
                deferred(anchored().position(Point::default()).child(foreground)).into_any_element()
            } else {
                foreground.into_any_element()
            };
            div()
                .relative()
                .size(px(100.))
                .child(
                    canvas(
                        move |bounds, window, _| {
                            back.set(Some(
                                window.insert_hitbox(bounds, HitboxBehavior::BlockMouse).id,
                            ));
                        },
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .inset_0(),
                )
                .child(interaction_scope("scope", self.gate.clone(), foreground))
        }
    }
    #[test]
    fn retained_interaction_gate_controls_hits_capture_and_deferred_scopes() {
        for nested in [false, true] {
            let mut cx = TestAppContext::single();
            let gate = InteractionGate::new(false);
            let back = Rc::new(Cell::new(None));
            let front = Rc::new(Cell::new(None));
            let window = cx.add_window(|_, _| Fixture {
                gate: gate.clone(),
                nested,
                back: back.clone(),
                front: front.clone(),
            });
            cx.update(|_| {});
            cx.update_window(window.into(), |_, window, _| {
                let at = point(px(10.), px(10.));
                assert_eq!(
                    window.rendered_frame.hit_test(at).ids.as_slice(),
                    &[back.get().unwrap()]
                );
                gate.set_enabled(true);
                assert_eq!(
                    window.rendered_frame.hit_test(at).ids.as_slice(),
                    &[front.get().unwrap()]
                );
                window.capture_pointer(front.get().unwrap());
                gate.set_enabled(false);
                assert!(!front.get().unwrap().is_hovered(window));
                assert_eq!(
                    window.rendered_frame.hit_test(at).ids.as_slice(),
                    &[back.get().unwrap()]
                );
            })
            .unwrap();
        }
    }

    struct ModeView {
        reads_mode: bool,
        renders: Rc<Cell<usize>>,
    }
    impl Render for ModeView {
        fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            self.renders.set(self.renders.get() + 1);
            div()
                .size_full()
                .child(if self.reads_mode && window.last_input_was_keyboard() {
                    "keyboard"
                } else {
                    "mouse"
                })
        }
    }
    struct ModeRoot {
        mode: Entity<ModeView>,
        fixed: Entity<ModeView>,
        clicks: Rc<Cell<usize>>,
    }
    impl Render for ModeRoot {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let clicks = self.clicks.clone();
            div()
                .flex()
                .flex_col()
                .w(px(100.))
                .h(px(150.))
                .child(
                    self.mode
                        .clone()
                        .cached(StyleRefinement::default().w(px(100.)).h(px(50.))),
                )
                .child(
                    self.fixed
                        .clone()
                        .cached(StyleRefinement::default().w(px(100.)).h(px(50.))),
                )
                .child(
                    div()
                        .id("test-click")
                        .w(px(100.))
                        .h(px(50.))
                        .on_click(move |_, _, _| clicks.set(clicks.get() + 1)),
                )
        }
    }
    #[test]
    fn input_modality_only_invalidates_views_that_read_it() {
        let mut cx = TestAppContext::single();
        let mode = Rc::new(Cell::new(0));
        let fixed = Rc::new(Cell::new(0));
        let window = cx.add_window(|_, cx| ModeRoot {
            clicks: Rc::default(),
            mode: cx.new(|_| ModeView {
                reads_mode: true,
                renders: mode.clone(),
            }),
            fixed: cx.new(|_| ModeView {
                reads_mode: false,
                renders: fixed.clone(),
            }),
        });
        cx.update(|_| {});
        let baseline = (mode.get(), fixed.get());
        let key = || {
            PlatformInput::KeyDown(KeyDownEvent {
                keystroke: Keystroke::parse("a").unwrap(),
                is_held: false,
                prefer_character_input: false,
            })
        };
        cx.update(|cx| {
            cx.update_window(window.into(), |_, w, cx| w.dispatch_event(key(), cx))
                .unwrap()
        });
        assert_eq!((mode.get(), fixed.get()), (baseline.0 + 1, baseline.1));
        cx.update(|cx| {
            cx.update_window(window.into(), |_, w, cx| {
                w.dispatch_event(
                    PlatformInput::MouseMove(MouseMoveEvent {
                        position: point(px(500.), px(500.)),
                        pressed_button: None,
                        modifiers: Default::default(),
                    }),
                    cx,
                )
            })
            .unwrap()
        });
        assert_eq!((mode.get(), fixed.get()), (baseline.0 + 2, baseline.1));
        window
            .update(&mut cx, |root, _, cx| {
                root.mode.update(cx, |v, cx| {
                    v.reads_mode = false;
                    cx.notify()
                })
            })
            .unwrap();
        cx.update(|_| {});
        let before = mode.get();
        cx.update(|cx| {
            cx.update_window(window.into(), |_, w, cx| w.dispatch_event(key(), cx))
                .unwrap()
        });
        assert_eq!(
            mode.get(),
            before,
            "a view that stopped reading the mode must retain its cache"
        );
        assert_eq!(fixed.get(), baseline.1);
    }
    #[test]
    fn click_state_does_not_invalidate_unrelated_cached_views() {
        let mut cx = TestAppContext::single();
        let mode = Rc::new(Cell::new(0));
        let fixed = Rc::new(Cell::new(0));
        let clicks = Rc::new(Cell::new(0));
        let window = cx.add_window(|_, cx| ModeRoot {
            mode: cx.new(|_| ModeView {
                reads_mode: false,
                renders: mode.clone(),
            }),
            fixed: cx.new(|_| ModeView {
                reads_mode: false,
                renders: fixed.clone(),
            }),
            clicks: clicks.clone(),
        });
        cx.update(|_| {});
        let position = point(px(10.), px(110.));
        for event in [
            PlatformInput::MouseDown(MouseDownEvent {
                position,
                button: MouseButton::Left,
                modifiers: Default::default(),
                click_count: 1,
                first_mouse: false,
            }),
            PlatformInput::MouseUp(MouseUpEvent {
                position,
                button: MouseButton::Left,
                modifiers: Default::default(),
                click_count: 1,
            }),
        ] {
            cx.update(|cx| {
                cx.update_window(window.into(), |_, w, cx| w.dispatch_event(event, cx))
                    .unwrap()
            });
        }
        assert_eq!(clicks.get(), 1);
        assert_eq!((mode.get(), fixed.get()), (1, 1));
    }

    struct FocusView {
        focus: FocusHandle,
        contains: bool,
        within: bool,
        renders: Rc<Cell<usize>>,
    }
    impl Render for FocusView {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.renders.set(self.renders.get() + 1);
            let active = if self.within {
                self.focus.within_focused(window, cx)
            } else if self.contains {
                self.focus.contains_focused(window, cx)
            } else {
                self.focus.is_focused(window)
            };
            let element = div()
                .size_full()
                .child(if active { "focused" } else { "idle" });
            if self.contains || self.within {
                element.into_any_element()
            } else {
                element.track_focus(&self.focus).into_any_element()
            }
        }
    }
    struct FocusRoot {
        first_inside: bool,
        parent: FocusHandle,
        outside: FocusHandle,
        first: Entity<FocusView>,
        second: Entity<FocusView>,
        within: Entity<FocusView>,
    }
    impl Render for FocusRoot {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let style = StyleRefinement::default().w(px(100.)).h(px(50.));
            let inside = if self.first_inside {
                self.first.clone().cached(style.clone()).into_any_element()
            } else {
                div().w(px(100.)).h(px(50.)).into_any_element()
            };
            let outside = if self.first_inside {
                div().w(px(100.)).h(px(50.)).into_any_element()
            } else {
                self.first.clone().cached(style.clone()).into_any_element()
            };
            div()
                .flex()
                .flex_col()
                .child(
                    div()
                        .id("parent")
                        .track_focus(&self.parent)
                        .child(inside)
                        .child(self.second.clone().cached(style.clone())),
                )
                .child(self.within.clone().cached(style))
                .child(outside)
                .child(div().id("outside").track_focus(&self.outside).size(px(10.)))
        }
    }
    #[test]
    fn retained_focus_queries_invalidate_only_changed_results() {
        let mut cx = TestAppContext::single();
        let counts = [
            Rc::new(Cell::new(0)),
            Rc::new(Cell::new(0)),
            Rc::new(Cell::new(0)),
        ];
        let window = cx.add_window(|_, cx| {
            let parent = cx.focus_handle();
            FocusRoot {
                first_inside: true,
                first: cx.new(|cx| FocusView {
                    focus: cx.focus_handle(),
                    contains: false,
                    within: false,
                    renders: counts[0].clone(),
                }),
                second: cx.new(|cx| FocusView {
                    focus: cx.focus_handle(),
                    contains: false,
                    within: false,
                    renders: counts[1].clone(),
                }),
                within: cx.new(|_| FocusView {
                    focus: parent.clone(),
                    contains: true,
                    within: false,
                    renders: counts[2].clone(),
                }),
                outside: cx.focus_handle(),
                parent,
            }
        });
        cx.update(|_| {});
        let values = || counts.each_ref().map(|count| count.get());
        assert_eq!(values(), [1, 1, 1]);
        let (first, second, outside) = window
            .read_with(&cx, |root, cx| {
                (
                    root.first.read(cx).focus.clone(),
                    root.second.read(cx).focus.clone(),
                    root.outside.clone(),
                )
            })
            .unwrap();
        for (focus, expected) in [
            (&first, [2, 1, 2]),
            (&second, [3, 2, 2]),
            (&outside, [3, 3, 3]),
        ] {
            cx.update(|cx| {
                cx.update_window(window.into(), |_, w, cx| w.focus(focus, cx))
                    .unwrap()
            });
            assert_eq!(values(), expected);
        }
        cx.update(|cx| cx.update_window(window.into(), |_, w, _| w.blur()).unwrap());
        assert_eq!(values(), [3, 3, 3]);
    }
    #[test]
    fn retained_focus_within_tracks_reparenting_without_focus_change() {
        let mut cx = TestAppContext::single();
        let counts = [
            Rc::new(Cell::new(0)),
            Rc::new(Cell::new(0)),
            Rc::new(Cell::new(0)),
        ];
        let window = cx.add_window(|_, cx| {
            let parent = cx.focus_handle();
            FocusRoot {
                first_inside: true,
                first: cx.new(|cx| FocusView {
                    focus: cx.focus_handle(),
                    contains: false,
                    within: false,
                    renders: counts[0].clone(),
                }),
                second: cx.new(|cx| FocusView {
                    focus: cx.focus_handle(),
                    contains: false,
                    within: false,
                    renders: counts[1].clone(),
                }),
                within: cx.new(|_| FocusView {
                    focus: parent.clone(),
                    contains: true,
                    within: false,
                    renders: counts[2].clone(),
                }),
                outside: cx.focus_handle(),
                parent,
            }
        });
        cx.update(|_| {});
        let first = window
            .read_with(&cx, |root, cx| root.first.read(cx).focus.clone())
            .unwrap();
        cx.update(|cx| {
            cx.update_window(window.into(), |_, w, cx| w.focus(&first, cx))
                .unwrap()
        });
        assert_eq!(counts[2].get(), 2);
        window
            .update(&mut cx, |root, _, cx| {
                root.first_inside = false;
                cx.notify()
            })
            .unwrap();
        cx.update(|_| {});
        cx.update(|cx| {
            cx.update_window(window.into(), |_, w, cx| w.simulate_next_frame(cx))
                .unwrap()
        });
        assert_eq!(
            counts[2].get(),
            3,
            "the retained focus-within result must update after the focused node moved out"
        );
        cx.update_window(window.into(), |_, w, _| assert!(first.is_focused(w)))
            .unwrap();
    }
    #[test]
    fn retained_within_focus_tracks_moved_descendants_with_stable_focus_path() {
        let mut cx = TestAppContext::single();
        let counts = [
            Rc::new(Cell::new(0)),
            Rc::new(Cell::new(0)),
            Rc::new(Cell::new(0)),
        ];
        let window = cx.add_window(|_, cx| {
            let parent = cx.focus_handle();
            let first = cx.new(|cx| FocusView {
                focus: cx.focus_handle(),
                contains: false,
                within: false,
                renders: counts[0].clone(),
            });
            let target = first.read(cx).focus.clone();
            FocusRoot {
                first_inside: true,
                first,
                second: cx.new(|cx| FocusView {
                    focus: cx.focus_handle(),
                    contains: false,
                    within: false,
                    renders: counts[1].clone(),
                }),
                within: cx.new(|_| FocusView {
                    focus: target,
                    contains: false,
                    within: true,
                    renders: counts[2].clone(),
                }),
                outside: cx.focus_handle(),
                parent,
            }
        });
        cx.update(|_| {});
        let parent = window
            .read_with(&cx, |root, _| root.parent.clone())
            .unwrap();
        cx.update(|cx| {
            cx.update_window(window.into(), |_, w, cx| w.focus(&parent, cx))
                .unwrap()
        });
        assert_eq!(counts[2].get(), 2);
        window
            .update(&mut cx, |root, _, cx| {
                root.first_inside = false;
                cx.notify()
            })
            .unwrap();
        cx.update(|_| {});
        cx.update(|cx| {
            cx.update_window(window.into(), |_, w, cx| w.simulate_next_frame(cx))
                .unwrap()
        });
        assert_eq!(counts[2].get(), 3);
        cx.update_window(window.into(), |_, w, _| assert!(parent.is_focused(w)))
            .unwrap();
    }
}
