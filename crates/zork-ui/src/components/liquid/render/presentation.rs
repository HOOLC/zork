//! Relocate source drawing while retaining the original control and input tree.
use super::*;
use std::collections::HashMap;

#[derive(Default)]
pub(super) struct Presentation {
    pub(super) enabled: bool,
    active: Option<SharedString>,
    sources: HashMap<SharedString, SourceDrawing>,
}
struct SourceDrawing {
    view: EntityId,
    snapshot: Option<PaintSnapshot>,
}

impl SourceMaterial {
    pub(crate) fn register(&self, owner: SharedString, view: EntityId) {
        let mut state = self.presentation.borrow_mut();
        state.sources.entry(owner).or_insert(SourceDrawing {
            view, snapshot: None,
        }).view = view;
    }
    pub(crate) fn relocate_drawing(
        &self,
        enabled: bool,
        active: Option<SharedString>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let active = enabled.then_some(active).flatten();
        let mut state = self.presentation.borrow_mut();
        if state.enabled == enabled && state.active == active {
            return;
        }
        let old = state.active.clone();
        let changed_mode = state.enabled != enabled;
        state.enabled = enabled;
        state.active = active.clone();
        for (owner, drawing) in &state.sources {
            if changed_mode || old.as_ref() == Some(owner) || active.as_ref() == Some(owner) {
                window.invalidate_presentation(drawing.view, cx);
            }
        }
        // Keep only the handoff's two participants. A shared dialog can be
        // opened from an unbounded list of controls over its lifetime.
        state
            .sources
            .retain(|owner, _| old.as_ref() == Some(owner) || active.as_ref() == Some(owner));
    }

    pub(crate) fn relocated(&self) -> bool {
        let state = self.presentation.borrow();
        state.enabled && self.owner.is_some() && state.active == self.owner
    }

    pub(crate) fn capture_content(&self, child: AnyElement) -> AnyElement {
        if !self.relocated() {
            return child;
        }
        SourceContent {
            source: self.clone(),
            child,
        }
        .into_any_element()
    }

    pub(crate) fn paint_ink(&self, window: &mut Window) {
        let snapshot = {
            let state = self.presentation.borrow();
            state.active.as_ref().and_then(|owner| state.sources.get(owner))
                .and_then(|drawing| drawing.snapshot.clone())
        };
        if let Some(snapshot) = snapshot { window.paint_retained_snapshot(&snapshot); }
    }

}

struct SourceContent {
    source: SourceMaterial,
    child: AnyElement,
}
impl IntoElement for SourceContent {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for SourceContent {
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
        let owner = self.source.owner.as_ref().unwrap();
        let mut state = self.source.presentation.borrow_mut();
        let drawing = state.sources.entry(owner.clone()).or_insert(SourceDrawing {
            view: window.current_view(),
            snapshot: None,
        });
        drawing.view = window.current_view();
        drop(state);
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
        use std::hash::{Hash, Hasher};
        let owner = self.source.owner.as_ref().unwrap();
        let previous = self
            .source
            .presentation
            .borrow()
            .sources
            .get(owner)
            .and_then(|drawing| drawing.snapshot.clone());
        let mut key = std::collections::hash_map::DefaultHasher::new();
        "liquid-source-drawing".hash(&mut key);
        (Rc::as_ptr(&self.source.presentation) as usize).hash(&mut key);
        owner.hash(&mut key);
        let (_, snapshot) =
            window.capture_paint_snapshot(key.finish(), bounds, previous.as_ref(), |window| {
                self.child.paint(window, cx)
            });
        self.source
            .presentation
            .borrow_mut()
            .sources
            .get_mut(owner)
            .unwrap()
            .snapshot = Some(snapshot.clone());
        if !self.source.relocated() {
            window.paint_snapshot(&snapshot);
        }
    }
}
