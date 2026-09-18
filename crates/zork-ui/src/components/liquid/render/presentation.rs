//! One persistent drawing node for a source control and its animation layer.
use super::*;

#[derive(Default)]
pub(super) struct Presentation {
    active: Option<(SharedString, Rc<RefCell<SourceDrawing>>)>,
}

#[derive(Default)]
pub(super) struct SourceDrawing {
    node: PaintNode,
    control: Option<PaintSnapshot>,
    ink: Option<PaintSnapshot>,
}

impl SourceMaterial {
    pub(crate) fn select_drawing(&self, source: &Self) {
        self.presentation.borrow_mut().active = source.owner.clone()
            .zip(source.drawing.clone());
    }

    pub(crate) fn release_drawing(&self) {
        self.presentation.borrow_mut().active = None;
    }

    pub(crate) fn follows_drawing(&self) -> bool {
        self.presentation.borrow().active.as_ref()
            .is_some_and(|(owner, _)| Some(owner) == self.owner.as_ref())
    }

    pub(crate) fn capture_content(&self, child: AnyElement) -> AnyElement {
        let Some(drawing) = &self.drawing else { return child; };
        SourceContent { drawing: drawing.clone(), child }.into_any_element()
    }

    /// The original control submits its node on every paint, including while
    /// the dialog is open. Frame composition selects one placement of it.
    pub(crate) fn paint_control<R>(
        &self,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        paint: impl FnOnce(&mut Window) -> R,
    ) -> R {
        let Some(drawing) = &self.drawing else { return paint(window); };
        let (node, previous) = {
            let drawing = drawing.borrow();
            (drawing.node, drawing.control.clone())
        };
        let (result, snapshot) = window.capture_paint_snapshot(
            drawing_key(node, false), bounds, previous.as_ref(), paint,
        );
        drawing.borrow_mut().control = Some(snapshot.clone());
        window.paint_node(node, &snapshot);
        result
    }

    pub(crate) fn paint_ink(&self, window: &mut Window) {
        let active = self.presentation.borrow().active.clone();
        if let Some((_, drawing)) = active {
            let drawing = drawing.borrow();
            if let Some(snapshot) = &drawing.ink {
                // The shared material paints the surface. This placement of
                // the same node supplies its ink, without the static surface
                // or a second composition of translucent text/icon pixels.
                window.paint_node(drawing.node, snapshot);
            }
        }
    }
}

fn drawing_key(node: PaintNode, ink: bool) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut key = std::collections::hash_map::DefaultHasher::new();
    "liquid-source-node".hash(&mut key);
    node.hash(&mut key);
    ink.hash(&mut key);
    key.finish()
}

struct SourceContent {
    drawing: Rc<RefCell<SourceDrawing>>,
    child: AnyElement,
}
impl IntoElement for SourceContent {
    type Element = Self;
    fn into_element(self) -> Self { self }
}
impl Element for SourceContent {
    type RequestLayoutState = ();
    type PrepaintState = ();
    fn id(&self) -> Option<ElementId> { None }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> { None }
    fn request_layout(
        &mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>,
        window: &mut Window, cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }
    fn prepaint(
        &mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>,
        _: Bounds<Pixels>, _: &mut (), window: &mut Window, cx: &mut App,
    ) {
        self.child.prepaint(window, cx);
    }
    fn paint(
        &mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>, _: &mut (), _: &mut (), window: &mut Window, cx: &mut App,
    ) {
        let (node, previous) = {
            let drawing = self.drawing.borrow();
            (drawing.node, drawing.ink.clone())
        };
        let (_, snapshot) = window.capture_paint_snapshot(
            drawing_key(node, true), bounds, previous.as_ref(), |window| self.child.paint(window, cx),
        );
        self.drawing.borrow_mut().ink = Some(snapshot.clone());
        window.paint_snapshot(&snapshot);
    }
}
