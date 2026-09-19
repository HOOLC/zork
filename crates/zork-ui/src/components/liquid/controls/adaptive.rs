//! Intrinsic/custom-content layout for the same action renderer used by fixed
//! controls. The child tree is laid out once, then painted through the measured
//! material; editors, focus handles and event handlers are never duplicated.
use super::*;

/// Full-width controls have a fixed layout slot and build their complete child
/// after its width is known, without a placeholder frame or width estimates.
pub(super) fn fill_slot(
    id: impl Into<SharedString>,
    height: f32,
    render: impl FnOnce(Size<Pixels>, &mut Window, &mut App) -> AnyElement + 'static,
) -> Stateful<Div> {
    div()
        .id(id.into())
        .w_full()
        .h(px(height))
        .child(SizedContent {
            render: Some(Box::new(render)),
            child: None,
        })
}
struct SizedContent {
    render: Option<Box<dyn FnOnce(Size<Pixels>, &mut Window, &mut App) -> AnyElement>>,
    child: Option<AnyElement>,
}
impl IntoElement for SizedContent {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for SizedContent {
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
        let mut style = Style::default();
        style.size = size(relative(1.).into(), relative(1.).into());
        (window.request_layout(style, [], cx), ())
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
        let mut child = self.render.take().unwrap()(bounds.size, window, cx);
        child.layout_as_root(bounds.size.map(AvailableSpace::Definite), window, cx);
        child.prepaint_at(bounds.origin, window, cx);
        self.child = Some(child);
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
        self.child.as_mut().unwrap().paint(window, cx);
    }
}

pub struct Action {
    root: Option<Stateful<Div>>,
    id: ElementId,
    label: SharedString,
    appearance: ActionStyle,
    parent: u32,
    children: Vec<AnyElement>,
    overlays: Vec<AnyElement>,
    clip: super::super::ContentClipBinding,
    measured: Option<AnyElement>,
    material: Option<AnyElement>,
    source: Option<super::super::render::SourceMaterial>,
    focus: Option<FocusHandle>,
    input: Option<(Entity<ComposerInput>, bool)>,
    editor_slot: Option<AnyElement>,
    tab_stop: bool,
    opacity: Option<f32>,
}

pub type Field = Action;

/// Keep the host's editor entity, IME and selection while measuring the shared
/// input material from its final layout (including inline title editors).
pub fn adaptive_input(
    id: impl Into<ElementId>,
    input: &Entity<ComposerInput>,
    invalid: bool,
    parent: u32,
) -> Field {
    let mut field = adaptive_action(id, "", ActionStyle::default(), parent);
    field.root = field.root.take().map(|root| {
        root.h(px(crate::controls::FIELD_HEIGHT))
            .w_full()
            .px_3()
            .py(px(5.))
            .justify_start()
            .items_start()
            .line_height(px(20.))
            .text_size(px(13.))
            .font_weight(FontWeight::NORMAL)
            .role(Role::TextInput)
    });
    field.input = Some((input.clone(), invalid));
    field
}

pub fn adaptive_action(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    appearance: ActionStyle,
    parent: u32,
) -> Action {
    let id = id.into();
    let label = label.into();
    let appearance = appearance.resolved();
    let enabled = !appearance.disabled && !appearance.busy;
    Action {
        root: Some(
            div()
                .id(id.clone())
                .relative()
                .role(Role::Button)
                .aria_label(label.clone())
                .tab_stop(enabled)
                .when(appearance.busy, |v| v.aria_description("正在处理"))
                .a11y_synthetic_children(move |builder| {
                    if !enabled {
                        builder.parent_node().set_disabled();
                    }
                })
                .h(px(CONTROL_HEIGHT))
                .flex_shrink_0()
                .px(px(crate::controls::BUTTON_PADDING_X))
                .flex()
                .items_center()
                .justify_center()
                .gap(px(7.))
                .text_size(px(12.))
                .text_color(rgb(action_ink(&label, appearance)))
                .font_weight(FontWeight::MEDIUM)
                .whitespace_nowrap(),
        ),
        id,
        label,
        appearance,
        parent,
        children: vec![],
        overlays: vec![],
        clip: Default::default(),
        measured: None,
        material: None,
        source: None,
        focus: None,
        input: None,
        editor_slot: None,
        tab_stop: enabled,
        opacity: None,
    }
}

impl Styled for Action {
    fn style(&mut self) -> &mut StyleRefinement {
        self.root.as_mut().unwrap().style()
    }
}
impl Action {
    pub(super) fn material_source(mut self, source: super::super::render::SourceMaterial) -> Self {
        self.source = Some(source);
        self
    }
    pub fn radius(mut self, radius: f32) -> Self {
        self.appearance.radius = Some(radius);
        self
    }
    pub fn selected(mut self, selected: bool) -> Self {
        self.appearance.selected = selected;
        self.root = self.root.take().map(|root| root.aria_selected(selected));
        self
    }
    /// Place the owned editor alongside an adornment without mounting it twice.
    pub fn editor_slot(mut self, content: impl IntoElement) -> Self {
        self.editor_slot = Some(content.into_any_element());
        self
    }
    pub(super) fn overlay(mut self, overlay: AnyElement) -> Self {
        self.overlays.push(overlay);
        self
    }
    pub fn opens_panel(mut self) -> Self {
        self.appearance.opens_panel = true;
        self
    }
    pub fn track_focus(mut self, focus: &FocusHandle) -> Self {
        self.focus = Some(focus.clone());
        self
    }
    pub fn tab_stop(mut self, enabled: bool) -> Self {
        self.tab_stop = enabled;
        self.root = self.root.take().map(|root| root.tab_stop(enabled));
        self
    }
}
impl InteractiveElement for Action {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.root.as_mut().unwrap().interactivity()
    }
}
impl StatefulInteractiveElement for Action {}
impl ParentElement for Action {
    fn extend(&mut self, children: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(children);
    }
}
impl IntoElement for Action {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for Action {
    type RequestLayoutState = LayoutId;
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
    ) -> (LayoutId, LayoutId) {
        let enabled = !self.appearance.disabled && !self.appearance.busy;
        let focus = if let Some(focus) = &self.focus {
            focus.clone().tab_stop(self.tab_stop && enabled)
        } else if let Some((input, _)) = &self.input {
            input.read(cx).focus_handle()
        } else {
            action_focus(self.id.clone(), window, cx).tab_stop(self.tab_stop && enabled)
        };
        self.focus = Some(focus.clone());
        let mut root = self.root.take().unwrap();
        self.opacity = root.style().opacity.take();
        let mut root = root
            .bg(rgba(0))
            .border_0()
            .track_focus(&focus)
            .when(enabled, |v| v.cursor_pointer())
            .when(!enabled, |v| v.cursor_default());
        if let Some((input, _)) = &self.input {
            root = root.child(
                self.editor_slot
                    .take()
                    .unwrap_or_else(|| input.clone().into_any_element()),
            );
        } else if !self.label.is_empty()
            || self.appearance.icon.is_some()
            || self.appearance.image.is_some()
            || self.appearance.radio.is_some()
        {
            root = root.child(self.clip.region(
                action_content(
                    &self.id,
                    self.label.clone(),
                    CONTROL_HEIGHT,
                    self.appearance,
                ),
                0.,
                0.,
            ));
        }
        let children = std::mem::take(&mut self.children);
        if self.input.is_some() {
            root = root.children(children);
        } else {
            root = root.children(
                children
                    .into_iter()
                    .map(|child| self.clip.region(child, 0., 0.).into_any_element()),
            );
        }
        root = root.children(std::mem::take(&mut self.overlays));
        let mut root = root.into_any_element();
        // GPUI binds state to the complete ancestor path during layout. Enter
        // the same material/content scopes used in prepaint before measuring.
        let material_id: ElementId = format!("adaptive-material-{:?}", self.id).into();
        let layout = window.with_id(material_id.clone(), |window| {
            if self.input.is_some() {
                root.request_layout(window, cx)
            } else {
                window.with_id(action_content_scope(&material_id), |window| {
                    root.request_layout(window, cx)
                })
            }
        });
        self.measured = Some(root);
        (layout, layout)
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        layout: &mut LayoutId,
        window: &mut Window,
        cx: &mut App,
    ) {
        let content = LaidOut {
            child: self.measured.take().unwrap(),
            layout: *layout,
            size: bounds.size,
        };
        let mut material = if let Some((input, invalid)) = &self.input {
            input_content(
                format!("adaptive-material-{:?}", self.id).into(),
                input,
                bounds.size.width.as_f32().max(1.),
                bounds.size.height.as_f32().max(1.),
                *invalid,
                self.parent,
                Some(content.into_any_element()),
                window,
                cx,
            )
        } else {
            render_action_content(
                format!("adaptive-material-{:?}", self.id).into(),
                self.label.clone(),
                bounds.size.width.as_f32().max(1.),
                bounds.size.height.as_f32().max(1.),
                self.appearance,
                self.parent,
                self.focus.as_ref().unwrap().clone(),
                Some((content.into_any_element(), self.clip.clone())),
                self.source.clone(),
                window,
                cx,
            )
        }
        .when_some(self.opacity, |material, opacity| material.opacity(opacity))
        .into_any_element();
        material.layout_as_root(bounds.size.map(AvailableSpace::Definite), window, cx);
        material.prepaint_at(bounds.origin, window, cx);
        self.material = Some(material);
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut LayoutId,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.material.as_mut().unwrap().paint(window, cx);
    }
}

/// Reuse the already measured child without asking GPUI to lay out a second
/// instance. Its semantic subtree is registered only at its painted position.
struct LaidOut {
    child: AnyElement,
    layout: LayoutId,
    size: Size<Pixels>,
}
impl IntoElement for LaidOut {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for LaidOut {
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
        let mut style = Style::default();
        style.size = self.size.map(|value| value.into());
        (window.request_layout(style, [], cx), ())
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
        // layout_bounds includes the current material's offset. Remove it to
        // recover the measured tree's original coordinates before relocating.
        let origin = window.layout_bounds(self.layout).origin - window.element_offset();
        self.child.prepaint_at(
            point(bounds.left() - origin.x, bounds.top() - origin.y),
            window,
            cx,
        );
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
