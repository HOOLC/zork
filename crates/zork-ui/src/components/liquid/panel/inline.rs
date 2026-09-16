//! The content panel in normal document flow. Layout uses the actual parent
//! width; a content change retargets the same material without remounting inputs.
use super::*;

pub struct InlinePanel {
    id: SharedString,
    root: Option<Stateful<Div>>,
    children: Vec<AnyElement>,
    clip: ContentClipBinding,
    content: Option<AnyElement>,
    background: Option<AnyElement>,
    border: Option<AnyElement>,
    radius: f32,
    colors: SurfaceColors,
}

pub fn inline(id: impl Into<SharedString>) -> InlinePanel {
    let id = id.into();
    InlinePanel {
        root: Some(
            div()
                .id(id.clone())
                .relative()
                .w_full()
                .flex()
                .flex_col()
                .p(px(16.))
                .gap(px(12.)),
        ),
        id,
        children: vec![],
        clip: Default::default(),
        content: None,
        background: None,
        border: None,
        radius: crate::controls::CARD_RADIUS,
        colors: SurfaceColors::outlined(
            crate::design::LIQUID_OUTLINE,
            crate::design::CUE_UI.palette.canvas,
        ),
    }
}
impl InlinePanel {
    pub fn radius(mut self, radius: f32) -> Self {
        self.radius = radius;
        self
    }
    pub fn colors(mut self, colors: SurfaceColors) -> Self {
        self.colors = colors;
        self
    }
}
impl Styled for InlinePanel {
    fn style(&mut self) -> &mut StyleRefinement {
        self.root.as_mut().unwrap().style()
    }
}
impl InteractiveElement for InlinePanel {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.root.as_mut().unwrap().interactivity()
    }
}
impl StatefulInteractiveElement for InlinePanel {}
impl ParentElement for InlinePanel {
    fn extend(&mut self, children: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(children);
    }
}
impl IntoElement for InlinePanel {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for InlinePanel {
    type RequestLayoutState = ();
    type PrepaintState = ();
    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone().into())
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
        let mut content = self
            .root
            .take()
            .unwrap()
            .bg(rgba(0))
            .border_0()
            .children(
                std::mem::take(&mut self.children)
                    .into_iter()
                    .map(|child| self.clip.region(child, 0., 0.)),
            )
            .into_any_element();
        let layout = content.request_layout(window, cx);
        self.content = Some(content);
        (layout, ())
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let panel = window.use_keyed_state(
            (ElementId::from(self.id.clone()), "inline-material"),
            cx,
            |_, _| ContentPanel::default(),
        );
        let target = Pose::rect(
            0.,
            0.,
            bounds.size.width.as_f32().max(2.) as f64,
            bounds.size.height.as_f32().max(2.) as f64,
            self.radius as f64,
        );
        let visible_bounds = bounds
            .intersect(&window.content_mask().bounds)
            .intersect(&Bounds::new(point(px(0.), px(0.)), window.viewport_size()));
        let visible = visible_bounds.size.width > px(0.) && visible_bounds.size.height > px(0.);
        let (background, border) = panel.update(cx, |panel, cx| {
            let mut motion = panel.motion.borrow_mut();
            motion.frame(
                target,
                target,
                false,
                true,
                Material::default(),
                visible,
                window,
                cx,
            );
            let surface = motion.surface.as_mut().unwrap();
            if !panel.initialized {
                surface.simulation.finish();
                surface.prepare();
                panel.initialized = true;
            }
            self.clip.bind(surface.content_clip());
            (
                surface
                    .background(self.colors.fill, None)
                    .into_any_element(),
                surface
                    .background_colors(None, self.colors.border, point(px(0.), px(0.)), false)
                    .into_any_element(),
            )
        });
        self.background = Some(background);
        self.border = Some(border);
        for layer in [&mut self.background, &mut self.border]
            .into_iter()
            .flatten()
        {
            layer.layout_as_root(bounds.size.map(AvailableSpace::Definite), window, cx);
            layer.prepaint_at(bounds.origin, window, cx);
        }
        self.content.as_mut().unwrap().prepaint(window, cx);
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
        self.background.as_mut().unwrap().paint(window, cx);
        self.content.as_mut().unwrap().paint(window, cx);
        self.border.as_mut().unwrap().paint(window, cx);
    }
}
