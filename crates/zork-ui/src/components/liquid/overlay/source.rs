//! Real, measured overlay sources with one capture/focus lifecycle for fixed
//! gallery triggers, adaptive controls and rich application rows.
use super::*;
use controls::ControlElement;

pub(super) struct Activation {
    pub bounds: Bounds<Pixels>,
    pub trigger: DialogTrigger,
    pub drawing: Rc<RefCell<Option<PaintRegion>>>,
    pub priority: usize,
    pub view: EntityId,
}

#[derive(Clone, Default)]
pub struct SourceBinding {
    pub(super) material: super::super::render::SourceMaterial,
    pub(super) anchor: MeasuredAnchor,
    pub(super) trigger: Rc<RefCell<Option<DialogTrigger>>>,
    pub(super) activation: Rc<Cell<u64>>,
    pub(super) pending: Rc<RefCell<Option<Activation>>>,
}
impl SourceBinding {
    pub fn bind<E: ControlElement>(
        &self,
        element: E,
        label: impl Into<SharedString>,
        style: controls::ActionStyle,
    ) -> BoundTrigger<E> {
        let id = Element::id(&element).expect("an overlay source needs a stable element ID");
        BoundTrigger {
            element: Some(element),
            rendered: None,
            id,
            label: label.into(),
            style,
            binding: self.clone(),
            focus: None,
            drawing: Default::default(),
        }
    }
    pub(super) fn attach<E: ControlElement>(
        &self,
        element: E,
        id: ElementId,
        _label: SharedString,
        style: controls::ActionStyle,
        focus: FocusHandle,
        drawing: Rc<RefCell<Option<PaintRegion>>>,
        window: &mut Window,
        cx: &mut App,
    ) -> E {
        let style = controls::ActionStyle {
            opens_panel: true,
            ..style
        };
        let key: SharedString = format!("{id:?}").into();
        let source = DialogTrigger {
            id: key.clone(),
            focus: focus.clone(),
        };
        if self.trigger.borrow().is_none() {
            *self.trigger.borrow_mut() = Some(source.clone());
        }
        let measured = Rc::new(Cell::new(Bounds::default()));
        let capture = measured.clone();
        let pending = self.pending.clone();
        let activation = self.activation.clone();
        let priority = Rc::new(Cell::new(0));
        let source_priority = priority.clone();
        let source_view = window.current_view();
        let activate: Rc<dyn Fn()> = Rc::new(move || {
            // A rich source may contain its own actions. Capture the candidate,
            // then commit it only if the host actually opens this dialog.
            *pending.borrow_mut() = Some(Activation {
                bounds: measured.get(),
                trigger: source.clone(),
                drawing: drawing.clone(),
                priority: source_priority.get(),
                view: source_view,
            });
            activation.set(activation.get().wrapping_add(1));
        });
        let mouse = activate.clone();
        let keys = activate;
        let notify = window.use_keyed_state(format!("overlay-source-{id:?}"), cx, |_, _| ());
        let trigger = self.trigger.clone();
        let anchor = self.anchor.clone();
        let material = self.material.for_owner(key.clone());
        let follows_anchor = material.clone();
        let enabled = !style.disabled && !style.busy;
        element
            .source_material(material)
            .control_focus(&focus)
            .panel_source()
            .capture_any_mouse_down(move |event, _, _| {
                if enabled && event.button == MouseButton::Left {
                    mouse();
                }
            })
            .capture_key_down(move |event, _, _| {
                if enabled
                    && !event.keystroke.modifiers.modified()
                    && matches!(event.keystroke.key.as_str(), "enter" | "space")
                {
                    keys();
                }
            })
            .control_overlay(
                canvas(
                    move |bounds, window, cx| {
                        priority.set(window.drawing_priority());
                        capture.set(bounds);
                        if trigger
                            .borrow()
                            .as_ref()
                            .is_some_and(|source| source.id == key)
                        {
                            if anchor.update(bounds, window) && follows_anchor.relocated() {
                                notify.update(cx, |_, cx| cx.notify());
                            }
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0()
                .into_any_element(),
            )
    }
}

pub struct BoundTrigger<E: ControlElement> {
    element: Option<E>,
    rendered: Option<AnyElement>,
    id: ElementId,
    label: SharedString,
    style: controls::ActionStyle,
    binding: SourceBinding,
    focus: Option<FocusHandle>,
    drawing: Rc<RefCell<Option<PaintRegion>>>,
}
impl<E: ControlElement> Styled for BoundTrigger<E> {
    fn style(&mut self) -> &mut StyleRefinement {
        self.element.as_mut().unwrap().style()
    }
}
impl<E: ControlElement> InteractiveElement for BoundTrigger<E> {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.element.as_mut().unwrap().interactivity()
    }
}
impl<E: ControlElement> StatefulInteractiveElement for BoundTrigger<E> {}
impl<E: ControlElement> ParentElement for BoundTrigger<E> {
    fn extend(&mut self, children: impl IntoIterator<Item = AnyElement>) {
        self.element.as_mut().unwrap().extend(children);
    }
}
impl<E: ControlElement> ControlElement for BoundTrigger<E> {
    fn source_material(mut self, material: super::super::render::SourceMaterial) -> Self {
        self.element = self.element.take().map(|element| element.source_material(material));
        self
    }
    fn control_focus(mut self, focus: &FocusHandle) -> Self {
        self.focus = Some(focus.clone());
        self
    }
    fn panel_source(mut self) -> Self {
        self.element = self.element.take().map(ControlElement::panel_source);
        self
    }
    fn control_overlay(mut self, overlay: AnyElement) -> Self {
        self.element = self
            .element
            .take()
            .map(|element| element.control_overlay(overlay));
        self
    }
}
impl<E: ControlElement> IntoElement for BoundTrigger<E> {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl<E: ControlElement> Element for BoundTrigger<E> {
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
        let focus = self
            .focus
            .clone()
            .unwrap_or_else(|| controls::action_focus(self.id.clone(), window, cx))
            .tab_stop(!self.style.disabled && !self.style.busy);
        let drawing = window.use_keyed_state(format!("overlay-source-drawing-{:?}", self.id), cx,
            |_, _| Rc::<RefCell<Option<PaintRegion>>>::default());
        self.drawing = drawing.read(cx).clone();
        let mut child = self
            .binding
            .attach(
                self.element.take().unwrap(),
                self.id.clone(),
                self.label.clone(),
                self.style,
                focus,
                self.drawing.clone(),
                window,
                cx,
            )
            .into_any_element();
        let layout = child.request_layout(window, cx);
        self.rendered = Some(child);
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
        let (_, region) = window.record_paint_region(|window| {
            self.rendered.as_mut().unwrap().paint(window, cx);
        });
        *self.drawing.borrow_mut() = region;
    }
}
