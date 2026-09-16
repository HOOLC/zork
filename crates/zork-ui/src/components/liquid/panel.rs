//! Content-sized liquid panels. Callers supply content and placement; this
//! component owns measurement, retargeting, clipping and the exit lifetime.
use super::presentation::{FramePaint, Presentation, Recipe, Target};
use super::{
    overlay::{FrameSample, Motion},
    ContentClipBinding, Material, Pose, Surface, SurfaceColors,
};
use crate::automation::{AutomationElementExt, AutomationRole};
use gpui::{prelude::*, *};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

mod floating;
pub use floating::{FloatingPanel, FloatingStyle, Hover, Side};
mod inline;
pub use inline::{inline, InlinePanel};

#[derive(Clone, Copy)]
pub struct Placement {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub radius: f32,
}

impl Placement {
    pub fn new(x: f32, y: f32, width: f32) -> Self {
        Self {
            x,
            y,
            width,
            radius: crate::controls::CARD_RADIUS,
        }
    }

    pub fn row_inset(self, minimum: f32, height: f32) -> f32 {
        super::row_inset(self.radius as f64, minimum as f64, height as f64) as f32
    }

    pub fn inner_radius(self, inset: f32, height: f32) -> f32 {
        super::inset_radius(
            self.radius as f64,
            inset as f64,
            (self.width - 2. * inset) as f64,
            height as f64,
        ) as f32
    }
}

/// A source may become the panel or remain beside a detached panel.
pub struct Source<V: 'static> {
    pub id: SharedString,
    pub label: SharedString,
    pub pose: Pose,
    pub open: bool,
    pub retained: bool,
    pub visible: bool,
    pub focus: Option<FocusHandle>,
    pub focus_on_change: bool,
    pub style: super::controls::ActionStyle,
    pub hover: Option<Rc<dyn Fn(&mut V, bool, &mut Window, &mut Context<V>)>>,
    in_layout: bool,
    change: Rc<dyn Fn(&mut V, bool, &mut Window, &mut Context<V>)>,
}
impl<V: 'static> Source<V> {
    pub fn new(
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        pose: Pose,
        open: bool,
        retained: bool,
        change: impl Fn(&mut V, bool, &mut Window, &mut Context<V>) + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            pose,
            open,
            retained,
            visible: true,
            focus: None,
            focus_on_change: true,
            style: Default::default(),
            hover: None,
            in_layout: false,
            change: Rc::new(change),
        }
    }
    /// The original control owns its paint, input and clipping in normal flow.
    /// This panel consumes its geometry and focus handle only.
    pub fn in_layout(mut self) -> Self {
        self.in_layout = true;
        self
    }
}

/// Each section keeps its final layout and gets its own contour clip.
pub struct Content {
    pub sections: Vec<AnyElement>,
    pub padding: f32,
    pub gap: f32,
}
impl Content {
    pub fn new(sections: Vec<AnyElement>) -> Self {
        Self {
            sections,
            padding: 16.,
            gap: 12.,
        }
    }
}

#[derive(Default)]
pub struct ContentPanel {
    motion: Rc<RefCell<Motion>>,
    presentation: Presentation,
    input_gate: Option<InteractionGate>,
    source_material: super::render::SourceMaterial,
    size: ContentSize,
    initialized: bool,
    was_open: bool,
    focus: Option<FocusHandle>,
    source_focus: Option<(SharedString, FocusHandle)>,
}
impl ContentPanel {
    pub(crate) fn source_material(&self) -> super::render::SourceMaterial {
        self.source_material.clone()
    }
    pub fn alive(&self) -> bool {
        self.motion.borrow().alive() || self.presentation.opacity() > 0.
    }
    pub fn content_height(&self) -> Option<f32> {
        self.size.height(f64::INFINITY).map(|height| height as f32)
    }
    pub fn inspect(&self) -> serde_json::Value {
        let mut value = self.motion.borrow().inspect();
        if let Some(fields) = value.as_object_mut() {
            fields.insert(
                "paintOnlyFrames".into(),
                serde_json::json!(self.presentation.frames()),
            );
        }
        value
    }
    pub fn pose(&self) -> Option<Pose> {
        self.motion
            .borrow()
            .surface
            .as_ref()
            .map(|s| s.simulation.pose())
    }
    pub fn progress(&self) -> f64 {
        self.motion.borrow().progress()
    }
    pub fn samples(&self) -> std::cell::Ref<'_, [FrameSample]> {
        std::cell::Ref::map(self.motion.borrow(), |motion| motion.samples.as_slice())
    }
    pub fn reset_samples(&mut self) {
        self.motion.borrow_mut().samples.clear();
    }
    pub fn visible(&self) -> bool {
        self.motion
            .borrow()
            .surface
            .as_ref()
            .is_some_and(Surface::visible)
    }
    pub fn render<V: 'static>(
        &mut self,
        id: impl Into<SharedString>,
        canvas_width: f32,
        placement: Placement,
        content: Content,
        source: Option<Source<V>>,
        colors: SurfaceColors,
        material: Material,
        window: &mut Window,
        cx: &mut Context<V>,
    ) -> AnyElement {
        self.render_with_visibility(
            id,
            canvas_width,
            placement,
            content,
            source,
            colors,
            material,
            None,
            window,
            cx,
        )
    }
    /// Read-only hover panels can enter and retire without taking source focus.
    pub fn render_with_visibility<V: 'static>(
        &mut self,
        id: impl Into<SharedString>,
        canvas_width: f32,
        placement: Placement,
        content: Content,
        source: Option<Source<V>>,
        mut colors: SurfaceColors,
        material: Material,
        visibility: Option<bool>,
        window: &mut Window,
        cx: &mut Context<V>,
    ) -> AnyElement {
        self.presentation.begin();
        let mut motion = self.motion.borrow_mut();
        let static_content = visibility.is_some() && source.is_none();
        let id = id.into();
        let measured = self.size.height(f64::INFINITY);
        let target = Pose::rect(
            placement.x as f64,
            placement.y as f64,
            placement.width.max(2.) as f64,
            measured.unwrap_or(2.),
            placement.radius as f64,
        );
        let from = source.as_ref().map_or(target, |s| s.pose);
        let open = visibility.unwrap_or_else(|| source.as_ref().is_none_or(|s| s.open));
        let separate_layer = source.as_ref().is_some_and(|s| s.in_layout);
        let pair = source.as_ref().is_some_and(|s| s.retained);
        let focus = self
            .focus
            .get_or_insert_with(|| cx.focus_handle().tab_stop(false))
            .clone();
        let source_focus = source.as_ref().map(|source| {
            if let Some(focus) = &source.focus {
                self.source_focus = Some((source.id.clone(), focus.clone()));
                return focus.clone();
            }
            if self
                .source_focus
                .as_ref()
                .is_none_or(|(id, _)| id != &source.id)
            {
                self.source_focus = Some((source.id.clone(), cx.focus_handle().tab_stop(true)));
            }
            self.source_focus.as_ref().unwrap().1.clone()
        });
        if source.is_some() {
            colors.border = Some(
                if source_focus
                    .as_ref()
                    .is_some_and(|focus| focus.is_focused(window))
                    && window.last_input_was_keyboard()
                {
                    crate::design::INTERACTION.focus_border
                } else {
                    crate::design::LIQUID_OUTLINE
                },
            );
        }
        if source.as_ref().is_some_and(|source| source.focus_on_change) {
            if !self.was_open && open {
                // The source can leave the tree during expansion. Keep keys
                // routed through a live content scope from the first frame.
                window.focus(&focus, cx);
            } else if self.was_open && !open {
                window.focus(source_focus.as_ref().unwrap(), cx);
            }
        }
        self.was_open = open;

        if !self.initialized && measured.is_some() {
            let scheduled = motion.scheduled.clone();
            *motion = Motion {
                scheduled,
                ..Default::default()
            };
        }
        let paint_only = static_content
            && measured.is_some()
            && window.gpu_mask_layers_enabled()
            && !window.is_a11y_active();
        motion.paint_only = paint_only;
        let content_moving = static_content && self.presentation.advance(open, cx);
        if content_moving && !paint_only && !motion.scheduled.replace(true) {
            let scheduled = motion.scheduled.clone();
            let owner = cx.entity().downgrade();
            window.on_next_frame(move |_, cx| {
                scheduled.set(false);
                let _ = owner.update(cx, |_, cx| cx.notify());
            });
        }
        let material_visible = source.as_ref().is_none_or(|source| source.visible)
            && motion.surface.as_ref().is_none_or(Surface::visible);
        motion.frame(
            from,
            target,
            pair,
            open,
            material,
            material_visible,
            window,
            cx,
        );
        // Initial layout appears at its natural size. Later content changes
        // retarget the existing material, retaining its position and velocity.
        if !self.initialized && measured.is_some() {
            if source.is_none() {
                let surface = motion.surface.as_mut().unwrap();
                surface.simulation.finish();
                surface.prepare();
            }
            self.initialized = true;
        }
        let surface = motion.surface.as_ref().unwrap();
        let current = surface.simulation.pose();
        let body_part = if pair && separate_layer && (open || motion.alive()) {
            Some(self.source_material.bind(surface, from, None))
        } else {
            self.source_material.clear();
            None
        };
        let carrier = if pair {
            surface.simulation.source_pose()
        } else {
            current
        };
        let clip = if static_content {
            ContentClipBinding::fixed_layout()
        } else {
            ContentClipBinding::default()
        };
        clip.bind(surface.content_clip());
        let mut flow = div()
            .w(px(target.w as f32))
            .flex()
            .flex_col()
            .p(px(content.padding))
            .gap(px(content.gap));
        for section in content.sections {
            flow = flow.child(clip.region(section, 0., 0.));
        }
        let alpha = if static_content {
            self.presentation.opacity()
        } else if source.is_none() {
            1.
        } else {
            ((motion.progress() - 0.4) / 0.4).clamp(0., 1.) as f32
        };
        let flow = self.size.measure(
            flow,
            px(target.w as f32),
            static_content || alpha > 0.,
            motion.scheduled.clone(),
            cx.entity().into_any().downgrade(),
        );
        let bottom = (current.top() + current.h).max(from.top() + from.h);
        if static_content {
            let recipe = Recipe {
                fill: colors.fill,
                border: colors.border,
                backdrop: None,
                initial_scale: 0.96,
                fade_material: true,
            };
            let paint = FramePaint {
                motion: self.motion.clone(),
                content: self.presentation.content.clone(),
                backdrop: self.presentation.backdrop.clone(),
                source: None,
                body: None,
                offset: Default::default(),
                part: None,
                recipe,
            };
            let contents = at(target)
                .child(surface.content_clip().transformed(
                    flow,
                    recipe.scale(alpha),
                    alpha,
                    format!("{id}-retained-content"),
                    self.presentation.snapshot.clone(),
                ))
                .id(format!("{id}-content"))
                .track_focus(&focus)
                .tab_stop(false)
                .map(|view| surface.guard(view))
                .automation_enabled(open && alpha > 0., AutomationRole::Status, "内容面板");
            let gate = self
                .input_gate
                .get_or_insert_with(|| InteractionGate::new(false));
            gate.set_state(open || alpha > 0. || motion.alive(), open);
            let stage = interaction_scope(
                format!("{id}-input-scope"),
                gate.clone(),
                div()
                    .id(id.clone())
                    .relative()
                    .w(px(canvas_width))
                    .h(px(bottom.max(2.) as f32))
                    .child(paint.underlay())
                    .child(contents)
                    .child(paint.outline()),
            )
            .into_any_element();
            return if paint_only && (content_moving || motion.state.moving(&surface.simulation)) {
                let owner = cx.entity().downgrade();
                self.presentation.playback(
                    stage,
                    paint,
                    Target {
                        from,
                        to: target,
                        pair,
                        open,
                        material,
                    },
                    Rc::new(move |cx| {
                        let _ = owner.update(cx, |_, cx| cx.notify());
                    }),
                )
            } else {
                stage
            };
        }

        let mut stage = div()
            .id(id.clone())
            .relative()
            .w(px(canvas_width))
            .h(px(bottom.max(2.) as f32))
            .child(if let Some(part) = &body_part {
                part.background(Some(colors.fill), None, point(px(0.), px(0.)), false)
            } else {
                surface.background(colors.fill, None).into_any_element()
            })
            .child(surface.background_colors(None, None, point(px(0.), px(0.)), false));
        if let Some(source) = source.filter(|s| !s.in_layout && (pair || motion.progress() < 0.4)) {
            let alpha = if pair {
                1.
            } else {
                1. - (motion.progress() / 0.4).clamp(0., 1.) as f32
            };
            let focus = source_focus.unwrap();
            let activate_focus = focus.clone();
            let label = source.label.clone();
            let face = div()
                .w(px(source.pose.w as f32))
                .h(px(source.pose.h as f32))
                .flex()
                .items_center()
                .justify_center()
                .child(super::controls::action_content(
                    &ElementId::from(source.id.clone()),
                    source.label,
                    source.pose.h as f32,
                    source.style,
                ));
            // Keep the action's full hit/focus slot outside its visual mask.
            // Only the ink is clipped while the material is changing shape.
            let change = source.change;
            stage = stage.child(
                at(carrier)
                    .id(source.id)
                    .occlude()
                    .w(px(source.pose.w as f32))
                    .h(px(source.pose.h as f32))
                    .track_focus(&focus)
                    .tab_stop(true)
                    .cursor_pointer()
                    .opacity(alpha)
                    .map(|v| surface.guard(v))
                    .child(surface.inset_content(carrier, 0., face))
                    .when_some(source.hover, |v, hover| {
                        v.on_hover(cx.listener(move |view, inside: &bool, w, cx| {
                            hover(view, *inside, w, cx);
                        }))
                    })
                    .on_click(cx.listener(move |v, _, w, cx| {
                        w.focus(&activate_focus, cx);
                        change(v, !open, w, cx);
                    }))
                    .automation(AutomationRole::Button, label.to_string()),
            );
        }
        stage = stage.child(
            surface
                .content_layer(
                    format!("{id}-foreground"),
                    current,
                    colors.fill,
                    alpha,
                    flow,
                    window,
                    cx,
                )
                .id(format!("{id}-content"))
                .track_focus(&focus)
                .tab_stop(false)
                .overflow_hidden()
                .when(alpha > 0., |v| {
                    surface
                        .guard(v)
                        .capture_any_mouse_down(move |_, _, cx| {
                            if !open {
                                cx.stop_propagation();
                            }
                        })
                        .capture_key_down(move |_, _, cx| {
                            if !open {
                                cx.stop_propagation();
                            }
                        })
                })
                .automation_enabled(open && alpha > 0., AutomationRole::Status, "内容面板"),
        );
        stage
            .child(if let Some(part) = &body_part {
                part.background(None, colors.border, point(px(0.), px(0.)), false)
            } else {
                surface
                    .background_colors(None, colors.border, point(px(0.), px(0.)), false)
                    .into_any_element()
            })
            .when(visibility.is_some(), |stage| {
                stage.opacity(motion.progress() as f32)
            })
            .into_any_element()
    }
}

fn at(p: Pose) -> Div {
    div()
        .absolute()
        .left(px(p.left() as f32))
        .top(px(p.top() as f32))
        .w(px(p.w as f32))
        .h(px(p.h as f32))
}

/// Shared by inline panels and window dialogs; no caller-owned height cache.
#[derive(Clone, Default)]
pub(crate) struct ContentSize(Rc<Cell<Option<Size<Pixels>>>>);
impl ContentSize {
    pub(crate) fn height(&self, limit: f64) -> Option<f64> {
        self.0
            .get()
            .map(|size| (size.height.as_f32() as f64).clamp(2., limit))
    }
    pub(crate) fn measure(
        &self,
        child: impl IntoElement,
        width: Pixels,
        visible: bool,
        scheduled: Rc<Cell<bool>>,
        owner: AnyWeakEntity,
    ) -> impl IntoElement {
        MeasuredContent {
            child: child.into_any_element(),
            width,
            visible,
            measured: self.0.clone(),
            on_change: Box::new(move |window| {
                super::motion::schedule(&scheduled, &owner, window);
            }),
        }
    }
}

/// Measure once in the final element namespace and reuse that layout for paint.
struct MeasuredContent {
    child: AnyElement,
    width: Pixels,
    visible: bool,
    measured: Rc<Cell<Option<Size<Pixels>>>>,
    on_change: Box<dyn Fn(&mut Window)>,
}
impl IntoElement for MeasuredContent {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for MeasuredContent {
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
        let measured = self.child.layout_as_root(
            size(
                AvailableSpace::Definite(self.width),
                AvailableSpace::MaxContent,
            ),
            window,
            cx,
        );
        if self.measured.replace(Some(measured)) != Some(measured) {
            (self.on_change)(window);
        }
        let mut style = Style::default();
        style.size = measured.map(Into::into);
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
        if self.visible {
            self.child.prepaint_at(bounds.origin, window, cx);
        }
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
        if self.visible {
            self.child.paint(window, cx);
        }
    }
}
