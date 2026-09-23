//! Zork presentation on gpui-base's native button.
use super::*;
use std::rc::Rc;

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
    inner: Option<gpui_base::Button>,
    rendered: Option<AnyElement>,
    id: ElementId,
    label: SharedString,
    appearance: ActionStyle,
    children: Vec<AnyElement>,
    overlays: Vec<AnyElement>,
    handlers: Vec<Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>>,
}

/// Text fields own a non-button surface; editing, focus and IME stay in ComposerInput.
pub struct Field {
    inner: Stateful<Div>,
    input: Entity<ComposerInput>,
    editor_slot: Option<AnyElement>,
    children: Vec<AnyElement>,
    invalid: bool,
}

pub fn adaptive_input(
    id: impl Into<ElementId>,
    input: &Entity<ComposerInput>,
    invalid: bool,
    parent: u32,
) -> Field {
    Field {
        inner: div()
            .id(id.into())
            .min_h(px(crate::controls::FIELD_HEIGHT))
            .w_full()
            .min_w_0()
            .flex()
            .items_center()
            .px_3()
            .py(px(5.))
            .line_height(px(20.))
            .text_size(px(13.))
            .font_weight(FontWeight::NORMAL)
            .rounded(px(crate::controls::FIELD_RADIUS))
            .overflow_hidden()
            .bg(rgb(if invalid {
                crate::design::FORM.error_surface
            } else {
                parent
            }))
            .border(px(crate::design::BORDER_WIDTH)),
        input: input.clone(),
        editor_slot: None,
        children: vec![],
        invalid,
    }
}
impl Field {
    pub fn editor_slot(mut self, content: impl IntoElement) -> Self {
        self.editor_slot = Some(content.into_any_element());
        self
    }
}
impl Styled for Field {
    fn style(&mut self) -> &mut StyleRefinement {
        self.inner.style()
    }
}
impl InteractiveElement for Field {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.inner.interactivity()
    }
}
impl StatefulInteractiveElement for Field {}
impl ParentElement for Field {
    fn extend(&mut self, children: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(children);
    }
}
impl Field {
    fn prepare(&mut self, window: &mut Window, cx: &mut App) {
        let input = self.input.read(cx);
        let focus = input.focus_handle();
        let disabled = input.is_disabled();
        let focused = !disabled && focus.contains_focused(window, cx);
        let form = &crate::design::FORM;
        let border = match (self.invalid, focused) {
            (true, true) => form.error_focus_border,
            (true, false) => form.error_border,
            (false, true) => form.focus_border,
            (false, false) => UI_OUTLINE,
        };
        let inner = std::mem::replace(&mut self.inner, div().id("field-layout"));
        self.inner = inner
            .border_color(rgb(border))
            .when(disabled, |v| {
                v.cursor_default().text_color(rgb(ZORK_UI.palette.subtle))
            })
            .when(!disabled, |v| {
                v.cursor_text()
                    .hover(|v| {
                        v.border_color(rgb(if self.invalid || focused {
                            border
                        } else {
                            form.hover_border
                        }))
                    })
                    .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                        window.focus(&focus, cx)
                    })
            })
            .child(
                self.editor_slot
                    .take()
                    .unwrap_or_else(|| self.input.clone().into_any_element()),
            )
            .children(std::mem::take(&mut self.children));
    }
}

impl IntoElement for Field {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for Field {
    type RequestLayoutState = <Stateful<Div> as Element>::RequestLayoutState;
    type PrepaintState = <Stateful<Div> as Element>::PrepaintState;
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
        self.prepare(window, cx);
        self.inner.request_layout(id, inspector, window, cx)
    }
    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        self.inner
            .prepaint(id, inspector, bounds, layout, window, cx)
    }
    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.inner
            .paint(id, inspector, bounds, layout, prepaint, window, cx)
    }
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
    let solid = appearance.primary && !appearance.select_trigger;
    let soft = appearance.variant == Some(ButtonVariant::Soft)
        || (appearance.quiet && appearance.selected);
    let icon_only = appearance
        .icon_only
        .unwrap_or(label.is_empty() && !appearance.select_trigger);
    let danger = appearance.variant == Some(ButtonVariant::Danger);
    let fill = if appearance.disabled {
        INTERACTION.neutral_pressed
    } else if danger {
        ZORK_UI.palette.danger
    } else if solid {
        BRAND_ACCENT
    } else if soft {
        ZORK_UI.palette.selected
    } else if appearance.quiet || icon_only {
        parent
    } else {
        ZORK_UI.palette.canvas
    };
    let radius = appearance.radius.unwrap_or_else(|| {
        if appearance.select_trigger {
            crate::controls::FIELD_RADIUS
        } else {
            crate::controls::BUTTON_RADIUS
        }
    });
    let outlined = !(soft || solid || appearance.quiet || appearance.radio.is_some() || icon_only);
    let ink = action_ink(&label, appearance);
    let selected_color = ZORK_UI.palette.selected;
    let mut button = gpui_base::Button::new(id.clone())
        .disabled(!enabled)
        .selected(appearance.selected)
        .accessibility_label(label.clone())
        .min_h(px(CONTROL_HEIGHT))
        .py(px(6.))
        .line_height(px(18.))
        .flex_shrink_0()
        .px(px(if appearance.select_trigger || appearance.leading {
            0.
        } else {
            crate::controls::BUTTON_PADDING_X
        }))
        .flex()
        .items_center()
        .justify_center()
        .gap(px(7.))
        .text_size(px(if appearance.select_trigger || appearance.leading {
            13.
        } else {
            12.
        }))
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgb(ink))
        .whitespace_nowrap()
        .rounded(px(radius.max(0.)))
        .overflow_hidden()
        .bg(rgb(fill))
        .border(px(crate::design::BORDER_WIDTH))
        .border_color(rgba(0))
        .styles(|styles| styles.selected(|state| state.bg(rgb(selected_color))));
    if outlined {
        button = button
            .border(px(crate::design::BORDER_WIDTH))
            .border_color(rgb(if appearance.selected || appearance.expanded {
                crate::controls::FIELD_FOCUS_BORDER
            } else {
                UI_OUTLINE
            }));
    }
    if enabled && !appearance.select_trigger {
        button = button
            .hover(|v| {
                v.bg(rgb(if danger {
                    INTERACTION.danger_hover
                } else if solid {
                    INTERACTION.accent_hover
                } else {
                    INTERACTION.neutral_hover
                }))
            })
            .active(|v| {
                v.bg(rgb(if danger {
                    INTERACTION.danger_pressed
                } else if solid {
                    INTERACTION.accent_pressed
                } else {
                    INTERACTION.neutral_pressed
                }))
            });
    }
    button = button.focus_visible(|v| {
        v.border(px(crate::design::BORDER_WIDTH))
            .border_color(rgb(crate::controls::FIELD_FOCUS_BORDER))
    });
    Action {
        inner: Some(button),
        rendered: None,
        id,
        label,
        appearance,
        children: Vec::new(),
        overlays: Vec::new(),
        handlers: Vec::new(),
    }
}

impl Styled for Action {
    fn style(&mut self) -> &mut StyleRefinement {
        self.inner.as_mut().unwrap().style()
    }
}
impl Action {
    pub fn radius(mut self, radius: f32) -> Self {
        self.appearance.radius = Some(radius);
        self.inner = self.inner.take().map(|button| button.rounded(px(radius)));
        self
    }
    pub fn selected(mut self, selected: bool) -> Self {
        self.appearance.selected = selected;
        self.inner = self.inner.take().map(|button| button.selected(selected));
        self
    }
    pub(super) fn overlay(mut self, overlay: AnyElement) -> Self {
        self.overlays.push(overlay);
        self
    }
    pub fn track_focus(mut self, focus: &FocusHandle) -> Self {
        self.inner = self.inner.take().map(|button| button.track_focus(focus));
        self
    }
    pub fn tab_stop(mut self, enabled: bool) -> Self {
        self.inner = self.inner.take().map(|button| button.tab_stop(enabled));
        self
    }
    pub fn role(mut self, role: Role) -> Self {
        self.inner = self.inner.take().map(|button| button.role(role));
        self
    }
    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.handlers.push(Rc::new(handler));
        self
    }
}
impl gpui_base::Selectable for Action {
    fn selected(self, selected: bool) -> Self {
        Action::selected(self, selected)
    }
    fn is_selected(&self) -> bool {
        self.appearance.selected
    }
}
impl InteractiveElement for Action {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.inner.as_mut().unwrap().interactivity()
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
        let mut button = self.inner.take().unwrap();
        let handlers = std::mem::take(&mut self.handlers);
        if !handlers.is_empty() {
            button = button.on_click(move |event, window, cx| {
                for handler in &handlers {
                    handler(event, window, cx);
                }
            });
        }
        if self.appearance.select_trigger && !self.appearance.disabled && !self.appearance.busy {
            button = button.hover(|v| v.border_color(rgb(crate::design::FORM.hover_border)));
        }
        // Respect the caller's final size, including compact actions.
        let height = match button.style().size.height {
            Some(Length::Definite(DefiniteLength::Absolute(length))) => {
                length.to_pixels(window.rem_size()).as_f32()
            }
            _ => CONTROL_HEIGHT,
        };
        if !self.label.is_empty()
            || self.appearance.icon.is_some()
            || self.appearance.image.is_some()
            || self.appearance.trailing.is_some()
            || self.appearance.radio.is_some()
            || self.appearance.busy
        {
            button = button.child(action_content(
                &self.id,
                self.label.clone(),
                height,
                self.appearance,
            ));
        }
        button = button
            .children(std::mem::take(&mut self.children))
            .children(std::mem::take(&mut self.overlays));
        let mut rendered = button.into_any_element();
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
