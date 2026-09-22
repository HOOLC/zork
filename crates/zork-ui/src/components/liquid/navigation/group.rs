//! Composable navigation for real trees, virtual rows and closeable tabs.
//! Rows keep normal layout; one group owns the travelling material and marker.
use super::*;
use controls::ControlElement;
use std::collections::HashMap;

#[derive(Clone)]
pub struct Group {
    state: Rc<RefCell<State>>,
    notify: Entity<()>,
    style: Style,
    material: Material,
}
impl Group {
    pub fn new(cx: &mut App) -> Self {
        Self {
            state: Default::default(),
            notify: cx.new(|_| ()),
            style: Style {
                kind: Kind::Sidebar,
                framed: false,
                parent: ZORK_UI.palette.sidebar,
                activate_on_arrow: false,
                row_radius: crate::controls::FIELD_RADIUS,
            },
            material: Material::ordinary(),
        }
    }
    pub fn keyed(id: impl Into<ElementId>, window: &mut Window, cx: &mut App) -> Self {
        window
            .use_keyed_state(id, cx, |_, cx| Self::new(cx))
            .read(cx)
            .clone()
    }
    pub fn configure(&mut self, style: Style, material: Material) {
        self.style = style;
        self.material = material;
    }
    pub fn kind(mut self, kind: Kind) -> Self {
        self.style.kind = kind;
        self
    }
    pub fn column(&self) -> Div {
        div().flex().flex_col().gap(px(2.))
    }
    pub fn section(&self, id: impl Into<ElementId>, title: impl Into<SharedString>) -> Div {
        let title = title.into();
        self.column().flex_shrink_0().child(
            div()
                .id(id)
                .h(px(20.))
                .flex_shrink_0()
                .px(px(8.))
                .flex()
                .items_center()
                .text_size(px(11.))
                .line_height(px(16.))
                .text_color(rgb(ZORK_UI.palette.muted))
                .child(title.clone())
                .automation(AutomationRole::Status, title),
        )
    }
    pub fn tab(&self, id: String, selected: bool) -> controls::Action {
        self.row(id, selected, true)
    }
    /// Same hit target and corner radius as the sliding hover, without the marker.
    pub fn plate(&self, id: impl Into<ElementId>, selected: bool) -> controls::Action {
        self.anchored(id, selected, true, true)
    }
    pub fn row(&self, id: impl Into<ElementId>, selected: bool, enabled: bool) -> controls::Action {
        self.anchored(id, selected, enabled, false)
    }
    fn anchored(
        &self,
        id: impl Into<ElementId>,
        selected: bool,
        enabled: bool,
        plate: bool,
    ) -> controls::Action {
        let id = id.into();
        controls::adaptive_action(
            id.clone(),
            "",
            controls::ActionStyle {
                quiet: true,
                icon_only: Some(false),
                disabled: !enabled,
                radius: Some(self.style.row_radius),
                hover_group: true,
                ..Default::default()
            },
            self.style.parent,
        )
        .group(format!("navigation-{id}"))
        .w_full()
        .h(px(crate::controls::CONTROL_HEIGHT))
        .px(px(7.))
        .gap_2()
        .justify_start()
        .font_weight(FontWeight::NORMAL)
        .when(self.style.kind == Kind::Tabs, |row| {
            row.role(Role::Tab).justify_center()
        })
        .control_overlay(
            RowAnchor {
                id,
                selected,
                enabled,
                plate,
                group: self.clone(),
            }
            .into_any_element(),
        )
    }
    pub fn surface(&self, content: impl IntoElement) -> GroupSurface {
        GroupSurface {
            inner: content.into_any_element(),
            group: self.clone(),
            plate: None,
            under: None,
            over: None,
        }
    }
    pub fn hovered(&self) -> Option<ElementId> {
        self.state.borrow().hot.clone()
    }
    pub fn set_hover(&self, id: Option<ElementId>, cx: &mut App) {
        if self.state.borrow().hot == id {
            return;
        }
        self.state.borrow_mut().hot = id;
        self.notify.update(cx, |_, cx| cx.notify());
    }
    pub(super) fn retain_visible_anchors(&self, ids: &std::collections::HashSet<ElementId>) {
        let mut state = self.state.borrow_mut();
        state.anchors.retain(|id, _| ids.contains(id));
        if state.hot.as_ref().is_some_and(|id| !ids.contains(id)) {
            state.hot = None;
        }
    }
    pub fn samples(&self) -> Vec<FrameSample> {
        self.state.borrow().samples.clone()
    }
    pub fn reset_samples(&self) {
        self.state.borrow_mut().samples.clear();
    }
    pub fn visible(&self) -> bool {
        self.state.borrow().visible
    }
    pub fn surfaces(&self) -> Vec<serde_json::Value> {
        let state = self.state.borrow();
        let mut values = vec![state.hover.motion.inspect()];
        if matches!(self.style.kind, Kind::Sidebar | Kind::Tabs) {
            values.push(state.indicator.motion.inspect());
        }
        values.retain(|value| !value.is_null());
        values
    }
}

#[derive(Clone, Copy)]
struct Anchor {
    bounds: Bounds<Pixels>,
    clip: Bounds<Pixels>,
    selected: bool,
    enabled: bool,
    plate: bool,
    order: u64,
}
#[derive(Default)]
struct State {
    anchors: HashMap<ElementId, Anchor>,
    hot: Option<ElementId>,
    order: u64,
    hover: Travel,
    plate: Travel,
    indicator: Travel,
    samples: Vec<FrameSample>,
    visible: bool,
}
struct Travel {
    motion: Motion,
    target: Option<ElementId>,
    bounds: Option<Bounds<Pixels>>,
    offset: Point<Pixels>,
    clip: Bounds<Pixels>,
    pose: Option<Pose>,
    open: bool,
}
impl Default for Travel {
    fn default() -> Self {
        Self {
            motion: Motion::persistent(),
            target: None,
            bounds: None,
            offset: Point::default(),
            clip: Bounds::default(),
            pose: None,
            open: false,
        }
    }
}
struct Layer {
    element: AnyElement,
    clip: Bounds<Pixels>,
}
impl Travel {
    fn frame(
        &mut self,
        target: Option<(ElementId, Bounds<Pixels>, Bounds<Pixels>)>,
        group: Bounds<Pixels>,
        radius: f32,
        color: u32,
        material: Material,
        visible: bool,
        window: &mut Window,
        cx: &mut Context<()>,
    ) -> (Option<Layer>, bool) {
        let open = target.is_some();
        let mut changed = self.open != open;
        if let Some((id, bounds, clip)) = target {
            let local = Bounds::new(bounds.origin - group.origin, bounds.size);
            if self.target.as_ref() == Some(&id) {
                if let Some(previous) = self.bounds {
                    // The same item follows scrolling immediately. A different
                    // item retargets the live pose, preserving velocity.
                    self.offset += local.origin - previous.origin;
                }
            } else {
                self.clip = self.clip.union(&clip);
                changed = true;
            }
            self.target = Some(id);
            self.bounds = Some(local);
            let pose = Pose::rect(
                (local.origin.x - self.offset.x).as_f32() as f64,
                (local.origin.y - self.offset.y).as_f32() as f64,
                bounds.size.width.as_f32().max(1.) as f64,
                bounds.size.height.as_f32().max(1.) as f64,
                radius as f64,
            );
            changed |= self.pose != Some(pose);
            self.pose = Some(pose);
            self.open = open;
            let actually_visible = visible
                && bounds.intersect(&clip).size.width > px(0.)
                && bounds.intersect(&clip).size.height > px(0.);
            self.motion.frame(
                pose,
                pose,
                false,
                true,
                material,
                actually_visible,
                window,
                cx,
            );
            if !self
                .motion
                .surface
                .as_ref()
                .is_some_and(|surface| surface.simulation.moving())
            {
                self.clip = clip;
            } else {
                self.clip = self.clip.union(&clip);
            }
        } else if let Some(pose) = self.pose {
            self.open = false;
            self.motion
                .frame(pose, pose, false, false, material, visible, window, cx);
        }
        let Some(surface) = &self.motion.surface else {
            return (None, changed);
        };
        let clip = self.clip.intersect(&group);
        if !visible
            || clip.size.width <= px(0.)
            || clip.size.height <= px(0.)
            || self.motion.progress() <= 0.001
        {
            return (None, changed);
        }
        let element = div()
            .size_full()
            .relative()
            .opacity(self.motion.progress() as f32)
            .child(surface.background_at(color, None, self.offset))
            .into_any_element();
        (Some(Layer { element, clip }), changed)
    }
}

#[derive(IntoElement)]
struct RowAnchor {
    id: ElementId,
    selected: bool,
    enabled: bool,
    plate: bool,
    group: Group,
}
impl RenderOnce for RowAnchor {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let id: ElementId = format!("{}-anchor", self.id).into();
        let released = self.group.clone();
        let released_id = self.id.clone();
        let _release = window.use_keyed_state((id.clone(), "release"), cx, |_, cx| {
            cx.on_release(move |_, cx| {
                let mut state = released.state.borrow_mut();
                state.anchors.remove(&released_id);
                if state.hot.as_ref() == Some(&released_id) {
                    state.hot = None;
                }
                drop(state);
                released.notify.update(cx, |_, cx| cx.notify());
            })
        });
        let hover = self.group.clone();
        let hover_id = self.id.clone();
        div()
            .id(id)
            .absolute()
            .inset_0()
            .on_hover(move |inside, _, cx| {
                if self.enabled && !self.plate && *inside {
                    hover.set_hover(Some(hover_id.clone()), cx);
                } else if hover.hovered().as_ref() == Some(&hover_id) {
                    hover.set_hover(None, cx);
                }
            })
            .child(
                canvas(
                    move |bounds, window, _| {
                        let mut state = self.group.state.borrow_mut();
                        state.order = state.order.wrapping_add(1);
                        let order = state.order;
                        state.anchors.insert(
                            self.id,
                            Anchor {
                                bounds,
                                clip: window.content_mask().bounds,
                                selected: self.selected,
                                enabled: self.enabled,
                                plate: self.plate,
                                order,
                            },
                        );
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0(),
            )
    }
}

/// Both surfaces share the complete group's paint boundary, including gaps and
/// distinct scroll regions, while each target keeps its actual clipping bounds.
pub struct GroupSurface {
    inner: AnyElement,
    group: Group,
    plate: Option<Layer>,
    under: Option<Layer>,
    over: Option<Layer>,
}
impl IntoElement for GroupSurface {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for GroupSurface {
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
        let _observe = window.use_keyed_state(
            format!("liquid-navigation-{:?}", self.group.notify.entity_id()),
            cx,
            |_, cx| cx.observe(&self.group.notify, |_, _, cx| cx.notify()),
        );
        (self.inner.request_layout(window, cx), ())
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
        self.inner.prepaint(window, cx);
        let state = self.group.state.clone();
        let style = self.group.style;
        let material = self.group.material;
        let (plate, under, over) = self.group.notify.update(cx, |_, cx| {
            let mut state = state.borrow_mut();
            let viewport = Bounds::new(point(px(0.), px(0.)), window.viewport_size());
            let visible_bounds = bounds
                .intersect(&window.content_mask().bounds)
                .intersect(&viewport);
            let visible = visible_bounds.size.width > px(0.) && visible_bounds.size.height > px(0.);
            state.visible = visible;
            let hot = state.hot.as_ref().and_then(|id| {
                state
                    .anchors
                    .get(id)
                    .filter(|a| a.enabled && !a.selected)
                    .map(|a| (id.clone(), *a))
            });
            let selected = state
                .anchors
                .iter()
                .filter(|(_, a)| a.selected && a.enabled)
                .max_by_key(|(_, a)| a.order)
                .map(|(id, a)| (id.clone(), *a));
            let indicator = selected
                .clone()
                .filter(|(_, a)| !a.plate)
                .and_then(|(id, a)| {
                    let marker = match style.kind {
                        Kind::Sidebar => Bounds::new(
                            point(a.bounds.left() + px(2.), a.bounds.center().y - px(7.)),
                            size(px(2.), px(14.)),
                        ),
                        Kind::Tabs => Bounds::new(
                            point(a.bounds.center().x - px(7.), a.bounds.bottom() - px(4.)),
                            size(px(14.), px(2.)),
                        ),
                        _ => return None,
                    };
                    Some((id, marker, a.clip))
                });
            let plate_target = None;
            let (plate, plate_changed) = state.plate.frame(
                plate_target,
                bounds,
                style.row_radius,
                ZORK_UI.palette.selected,
                material,
                visible,
                window,
                cx,
            );
            let (under, hover_changed) = state.hover.frame(
                hot.map(|(id, a)| (id, a.bounds, a.clip)),
                bounds,
                style.row_radius,
                crate::design::INTERACTION.neutral_hover,
                material,
                visible,
                window,
                cx,
            );
            let (over, indicator_changed) = state.indicator.frame(
                indicator,
                bounds,
                1.,
                crate::design::BRAND_ACCENT,
                material,
                visible,
                window,
                cx,
            );
            let moving = state
                .hover
                .motion
                .surface
                .as_ref()
                .is_some_and(|s| s.simulation.moving())
                || state
                    .plate
                    .motion
                    .surface
                    .as_ref()
                    .is_some_and(|s| s.simulation.moving())
                || state
                    .indicator
                    .motion
                    .surface
                    .as_ref()
                    .is_some_and(|s| s.simulation.moving());
            if visible
                && (hover_changed
                    || plate_changed
                    || indicator_changed
                    || moving
                    || state.samples.is_empty())
            {
                let motions = [
                    &state.hover.motion,
                    &state.plate.motion,
                    &state.indicator.motion,
                ];
                let surfaces: Vec<_> = motions.iter().filter_map(|m| m.surface.as_ref()).collect();
                let sample = FrameSample {
                    work_ms: motions.iter().map(|m| m.work_ms).sum(),
                    physics_ms: motions.iter().map(|m| m.physics_ms).sum(),
                    contour_ms: motions.iter().map(|m| m.work_ms - m.physics_ms).sum(),
                    controls: 1,
                    moving,
                    particles: surfaces
                        .iter()
                        .map(|s| s.simulation.particles().count())
                        .sum(),
                    sampled: surfaces.iter().map(|s| s.contour().sampled_points).sum(),
                    full_grid: surfaces.iter().map(|s| s.contour().full_grid_points).sum(),
                    fallbacks: surfaces
                        .iter()
                        .filter(|s| s.contour().used_fallback)
                        .count(),
                };
                state.samples.push(sample);
                if state.samples.len() > 3000 {
                    state.samples.drain(..1000);
                }
            }
            (plate, under, over)
        });
        self.plate = plate;
        self.under = under;
        self.over = over;
        for layer in [&mut self.plate, &mut self.under, &mut self.over]
            .into_iter()
            .flatten()
        {
            layer
                .element
                .layout_as_root(bounds.size.map(AvailableSpace::Definite), window, cx);
            window.with_content_mask(Some(ContentMask { bounds: layer.clip }), |window| {
                layer.element.prepaint_at(bounds.origin, window, cx);
            });
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
        if let Some(layer) = &mut self.plate {
            window.with_content_mask(Some(ContentMask { bounds: layer.clip }), |window| {
                layer.element.paint(window, cx)
            });
        }
        if let Some(layer) = &mut self.under {
            window.with_content_mask(Some(ContentMask { bounds: layer.clip }), |window| {
                layer.element.paint(window, cx)
            });
        }
        self.inner.paint(window, cx);
        if let Some(layer) = &mut self.over {
            window.with_content_mask(Some(ContentMask { bounds: layer.clip }), |window| {
                layer.element.paint(window, cx)
            });
        }
    }
}
